use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::browser::GetBrowserCommandLineParams;
use chromiumoxide::cdp::browser_protocol::target::TargetId;
use chrono::Utc;
use futures_util::StreamExt;
use serde::Serialize;
use std::env;
use std::fmt;
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use turso_util::TokenTier;

mod turso;

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:9222";
const SETTLE_TIMEOUT_SECS: u64 = 18;

/// chromeport's OTA capture→parse path is RETIRED — OTA scraping + extraction moved to gwebcdb's
/// Python bridge tools on WSLg (see CLAUDE.md "URL Routing"). chromeport now only serves the
/// browser/screenshot/db subcommands used by `snapshot-maps`. Its `parser_rules` reader/writer
/// still assumed the OLD single-PK `(source_id)` schema and would break against the re-keyed
/// `(source_id, product_type)` table (review finding F3, 2026-06-29), so the OTA-parse subcommands
/// fail loud rather than corrupt the schema or write half a row.
const OTA_PARSE_RETIRED: &str = "chromeport OTA parse/verify is RETIRED — OTA capture+parse moved \
to gwebcdb on WSLg (python bridge/ota_capture.py + bridge/ota_cli.py parse). See CLAUDE.md \"URL \
Routing\". chromeport now only provides browser/screenshot/db for snapshot-maps.";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("chromeport: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

fn run(args: Vec<String>) -> Result<(), CliError> {
    let cli = Cli::parse(args)?;
    match cli.command {
        Command::Browser(BrowserCommand::Doctor) => browser_doctor(&cli.endpoint),
        Command::Browser(BrowserCommand::Pages) => browser_pages(&cli.endpoint),
        Command::Browser(BrowserCommand::Snapshot {
            page_index,
            source_id,
            out,
            include_html,
        }) => run_async(capture_snapshot(
            &cli.endpoint,
            page_index,
            source_id,
            out,
            include_html,
        )),
        Command::Fetch(FetchCommand::Url {
            url,
            source_id,
            out,
            include_html,
        }) => run_async(fetch_url(&cli.endpoint, url, source_id, out, include_html)),
        Command::Fetch(FetchCommand::Interact {
            url,
            source_id,
            out,
            include_html,
            steps,
            profile_override,
        }) => run_async(fetch_interact(
            &cli.endpoint,
            url,
            source_id,
            out,
            include_html,
            steps,
            profile_override,
        )),
        Command::Screenshot {
            url,
            out,
            width,
            height,
            wait_ms,
            full_page,
        } => run_async(screenshot_url(
            &cli.endpoint,
            url,
            out,
            width,
            height,
            wait_ms,
            full_page,
        )),
        Command::Parse(ParseCommand::Capture {
            capture_id: _,
            source_id: _,
            dry_run: _,
            allow_source_override: _,
        }) => Err(CliError::runtime(OTA_PARSE_RETIRED)),
        Command::Verify {
            source_id: _,
            capture_id: _,
            allow_source_override: _,
        } => Err(CliError::runtime(OTA_PARSE_RETIRED)),
        Command::ParserRules(ParserRulesCommand::SeedDefaults) => {
            Err(CliError::runtime(OTA_PARSE_RETIRED))
        }
        Command::Db(DbCommand::Query { sql }) => run_async(db_query(sql)),
        Command::Db(DbCommand::Exec { sql }) => run_async(db_exec(sql)),
        Command::Db(DbCommand::TokenStatus { tier }) => db_token_status(tier),
    }
}

struct Cli {
    endpoint: String,
    command: Command,
}

enum Command {
    Browser(BrowserCommand),
    Fetch(FetchCommand),
    Screenshot {
        url: String,
        out: PathBuf,
        width: Option<u32>,
        height: Option<u32>,
        wait_ms: Option<u64>,
        full_page: bool,
    },
    // RETIRED (gwebcdb owns OTA parse/verify; see OTA_PARSE_RETIRED). These variants still parse
    // their args so the commands fail loud with a clear message instead of an arg error — their
    // fields are intentionally unread.
    #[allow(dead_code)]
    Parse(ParseCommand),
    #[allow(dead_code)]
    Verify {
        source_id: String,
        capture_id: String,
        allow_source_override: bool,
    },
    ParserRules(ParserRulesCommand),
    Db(DbCommand),
}

enum ParserRulesCommand {
    SeedDefaults,
}

enum DbCommand {
    Query { sql: String },
    Exec { sql: String },
    TokenStatus { tier: TokenTier },
}

// RETIRED — see OTA_PARSE_RETIRED. Args still parse so the command fails loud, not on arg errors.
#[allow(dead_code)]
enum ParseCommand {
    Capture {
        capture_id: String,
        source_id: String,
        dry_run: bool,
        allow_source_override: bool,
    },
}

enum BrowserCommand {
    Doctor,
    Pages,
    Snapshot {
        page_index: usize,
        source_id: Option<String>,
        out: Option<PathBuf>,
        include_html: bool,
    },
}

enum FetchCommand {
    Url {
        url: String,
        source_id: String,
        out: Option<PathBuf>,
        include_html: bool,
    },
    Interact {
        url: String,
        source_id: String,
        out: Option<PathBuf>,
        include_html: bool,
        steps: Vec<InteractionStep>,
        profile_override: bool,
    },
}

enum InteractionStep {
    Fill { selector: String, value: String },
    Click { selector: String },
    Wait { ms: u64 },
    WaitFor { selector: String },
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Self, CliError> {
        let mut endpoint = env::var("CHROMEPORT_CDP_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
        let mut positional = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--endpoint" => {
                    i += 1;
                    endpoint = args
                        .get(i)
                        .ok_or_else(|| CliError::usage("--endpoint requires a value"))?
                        .to_string();
                }
                "--help" | "-h" => return Err(CliError::help(usage())),
                arg => positional.push(arg.to_string()),
            }
            i += 1;
        }

        match positional.as_slice() {
            [group, cmd] if group == "browser" && cmd == "doctor" => Ok(Self {
                endpoint,
                command: Command::Browser(BrowserCommand::Doctor),
            }),
            [group, cmd] if group == "browser" && cmd == "pages" => Ok(Self {
                endpoint,
                command: Command::Browser(BrowserCommand::Pages),
            }),
            [group, cmd, rest @ ..] if group == "browser" && cmd == "snapshot" => {
                let page_raw = option_value(rest, "--page")
                    .ok_or_else(|| CliError::usage("browser snapshot requires --page <N>"))?;
                let page_index = page_raw
                    .parse::<usize>()
                    .map_err(|_| CliError::usage("--page must be a non-negative integer"))?;
                Ok(Self {
                    endpoint,
                    command: Command::Browser(BrowserCommand::Snapshot {
                        page_index,
                        source_id: option_value(rest, "--source").map(str::to_string),
                        out: option_value(rest, "--out").map(PathBuf::from),
                        include_html: has_flag(rest, "--html"),
                    }),
                })
            }
            [group, cmd, url, rest @ ..] if group == "fetch" && cmd == "url" => {
                let source_id = option_value(rest, "--source")
                    .ok_or_else(|| CliError::usage("fetch url requires --source <id>"))?
                    .to_string();
                Ok(Self {
                    endpoint,
                    command: Command::Fetch(FetchCommand::Url {
                        url: url.to_string(),
                        source_id,
                        out: option_value(rest, "--out").map(PathBuf::from),
                        include_html: has_flag(rest, "--html"),
                    }),
                })
            }
            [group, cmd, url, rest @ ..] if group == "fetch" && cmd == "interact" => {
                let source_id = option_value(rest, "--source")
                    .ok_or_else(|| CliError::usage("fetch interact requires --source <id>"))?
                    .to_string();
                let steps = option_values(rest, "--step")
                    .into_iter()
                    .map(parse_interaction_step)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self {
                    endpoint,
                    command: Command::Fetch(FetchCommand::Interact {
                        url: url.to_string(),
                        source_id,
                        out: option_value(rest, "--out").map(PathBuf::from),
                        include_html: has_flag(rest, "--html"),
                        steps,
                        profile_override: has_flag(rest, "--i-understand-profile"),
                    }),
                })
            }
            [cmd, url, rest @ ..] if cmd == "screenshot" => {
                let out = option_value(rest, "--out")
                    .ok_or_else(|| CliError::usage("screenshot requires --out <path.png>"))?;
                let width = match option_value(rest, "--width") {
                    Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                        CliError::usage("--width must be a non-negative integer")
                    })?),
                    None => None,
                };
                let height = match option_value(rest, "--height") {
                    Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                        CliError::usage("--height must be a non-negative integer")
                    })?),
                    None => None,
                };
                let wait_ms = match option_value(rest, "--wait") {
                    Some(raw) => Some(raw.parse::<u64>().map_err(|_| {
                        CliError::usage("--wait must be a non-negative integer (milliseconds)")
                    })?),
                    None => None,
                };
                Ok(Self {
                    endpoint,
                    command: Command::Screenshot {
                        url: url.to_string(),
                        out: PathBuf::from(out),
                        width,
                        height,
                        wait_ms,
                        full_page: has_flag(rest, "--full-page"),
                    },
                })
            }
            [group, cmd, capture_id, rest @ ..] if group == "parse" && cmd == "capture" => {
                let source_id = option_value(rest, "--source")
                    .ok_or_else(|| CliError::usage("parse capture requires --source <id>"))?
                    .to_string();
                Ok(Self {
                    endpoint,
                    command: Command::Parse(ParseCommand::Capture {
                        capture_id: capture_id.to_string(),
                        source_id,
                        dry_run: has_flag(rest, "--dry-run"),
                        allow_source_override: has_flag(rest, "--allow-source-override"),
                    }),
                })
            }
            [cmd, source_id, capture_id, rest @ ..] if cmd == "verify" => Ok(Self {
                endpoint,
                command: Command::Verify {
                    source_id: source_id.to_string(),
                    capture_id: capture_id.to_string(),
                    allow_source_override: has_flag(rest, "--allow-source-override"),
                },
            }),
            [group, subgroup, cmd]
                if group == "parser" && subgroup == "rules" && cmd == "seed-defaults" =>
            {
                Ok(Self {
                    endpoint,
                    command: Command::ParserRules(ParserRulesCommand::SeedDefaults),
                })
            }
            [group, cmd, sql] if group == "db" && cmd == "query" => Ok(Self {
                endpoint,
                command: Command::Db(DbCommand::Query {
                    sql: sql.to_string(),
                }),
            }),
            [group, cmd, sql] if group == "db" && cmd == "exec" => Ok(Self {
                endpoint,
                command: Command::Db(DbCommand::Exec {
                    sql: sql.to_string(),
                }),
            }),
            [group, cmd, tier] if group == "db" && cmd == "token-status" => Ok(Self {
                endpoint,
                command: Command::Db(DbCommand::TokenStatus {
                    tier: parse_token_tier(tier)?,
                }),
            }),
            _ => Err(CliError::usage(usage())),
        }
    }
}

fn usage() -> &'static str {
    "Usage:\n  chromeport [--endpoint http://127.0.0.1:9222] browser doctor\n  chromeport [--endpoint http://127.0.0.1:9222] browser pages\n  chromeport [--endpoint http://127.0.0.1:9222] browser snapshot --page <N> [--source <id>] [--html]\n  chromeport [--endpoint http://127.0.0.1:9222] fetch url <url> --source <id> [--html]\n  chromeport [--endpoint http://127.0.0.1:9222] fetch interact <url> --source <id> [--step <kind>]... [--html] [--i-understand-profile]\n  chromeport [--endpoint http://127.0.0.1:9222] screenshot <url> --out <path.png> [--width <px>] [--height <px>] [--wait <ms>] [--full-page]\n  chromeport db query <sql>\n  chromeport db exec <sql>\n  chromeport db token-status <read|write|secrets>\n\nRETIRED: `parse capture` / `verify` / `parser rules seed-defaults` — OTA capture+parse moved to\ngwebcdb on WSLg (python bridge/ota_capture.py + bridge/ota_cli.py parse; see CLAUDE.md \"URL Routing\").\nchromeport now only provides browser/screenshot/db (used by snapshot-maps).\n\nCaptures are stored as rows in the Turso `captures` table (plain text, no JSON files).\n\nSteps:\n  --step 'fill:SEL=VALUE'\n  --step 'click:SEL'\n  --step 'wait:MS'\n  --step 'waitfor:SEL'\n\nEnv:\n  CHROMEPORT_CDP_ENDPOINT overrides the default endpoint.\n  Turso credentials are resolved through minted tier tokens via turso-util; run `turso auth login` if token resolution fails.\n"
}

fn parse_token_tier(raw: &str) -> Result<TokenTier, CliError> {
    match raw {
        "read" => Ok(TokenTier::Read),
        "write" => Ok(TokenTier::Write),
        "secrets" => Ok(TokenTier::Secrets),
        _ => Err(CliError::usage(
            "db token-status requires tier read, write, or secrets",
        )),
    }
}

fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let index = args.iter().position(|arg| arg == name)?;
    args.get(index + 1).map(String::as_str)
}

fn option_values<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter_map(|pair| {
            if pair[0] == name {
                Some(pair[1].as_str())
            } else {
                None
            }
        })
        .collect()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn parse_interaction_step(raw: &str) -> Result<InteractionStep, CliError> {
    if let Some(rest) = raw.strip_prefix("fill:") {
        let (selector, value) = rest
            .rsplit_once('=')
            .ok_or_else(|| CliError::usage("fill step must be fill:SEL=VALUE"))?;
        if selector.trim().is_empty() {
            return Err(CliError::usage("fill step selector cannot be empty"));
        }
        return Ok(InteractionStep::Fill {
            selector: selector.to_string(),
            value: value.to_string(),
        });
    }
    if let Some(selector) = raw.strip_prefix("click:") {
        if selector.trim().is_empty() {
            return Err(CliError::usage("click step selector cannot be empty"));
        }
        return Ok(InteractionStep::Click {
            selector: selector.to_string(),
        });
    }
    if let Some(value) = raw.strip_prefix("wait:") {
        let ms = value
            .parse::<u64>()
            .map_err(|_| CliError::usage("wait step must be wait:MS with integer milliseconds"))?;
        return Ok(InteractionStep::Wait { ms });
    }
    if let Some(selector) = raw.strip_prefix("waitfor:") {
        if selector.trim().is_empty() {
            return Err(CliError::usage("waitfor step selector cannot be empty"));
        }
        return Ok(InteractionStep::WaitFor {
            selector: selector.to_string(),
        });
    }
    Err(CliError::usage(format!(
        "unknown interaction step '{raw}'; expected fill:, click:, wait:, or waitfor:"
    )))
}

fn run_async<F>(future: F) -> Result<(), CliError>
where
    F: std::future::Future<Output = Result<(), CliError>>,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| CliError::runtime(format!("failed to start async runtime: {err}")))?;
    runtime.block_on(future)
}

fn browser_doctor(endpoint: &str) -> Result<(), CliError> {
    let version = cdp_get(endpoint, "/json/version")?;
    let browser = json_string_value(&version, "Browser").unwrap_or_else(|| "unknown".to_string());
    let ws = json_string_value(&version, "webSocketDebuggerUrl").unwrap_or_else(|| "-".to_string());
    let pages = fetch_pages(endpoint)?;

    println!("endpoint\t{endpoint}");
    println!("browser\t{browser}");
    println!("websocket\t{ws}");
    println!("pages\t{}", pages.len());

    if !is_loopback_endpoint(endpoint) {
        println!(
            "warning\tcdp endpoint is not localhost/127.0.0.1; do not expose CDP to LAN or internet"
        );
    }
    if pages.is_empty() {
        return Err(CliError::runtime(
            "CDP is reachable, but no pages are open in Chrome",
        ));
    }

    Ok(())
}

fn browser_pages(endpoint: &str) -> Result<(), CliError> {
    let pages = fetch_pages(endpoint)?;
    if pages.is_empty() {
        println!("No pages found");
        return Ok(());
    }

    for (index, page) in pages.iter().enumerate() {
        println!("[{index}] {}", page.title);
        println!("    {}", page.url);
        println!("    id={} type={}", page.id, page.kind);
    }
    Ok(())
}

async fn capture_snapshot(
    endpoint: &str,
    page_index: usize,
    source_id: Option<String>,
    out: Option<PathBuf>,
    include_html: bool,
) -> Result<(), CliError> {
    if !is_loopback_endpoint(endpoint) {
        println!(
            "warning\tcdp endpoint is not localhost/127.0.0.1; do not expose CDP to LAN or internet"
        );
    }
    let rest_pages = fetch_pages(endpoint)?;
    let rest_page = rest_pages
        .get(page_index)
        .ok_or_else(|| CliError::runtime(format!("no page at index {page_index}")))?;
    if rest_page.kind != "page" {
        return Err(CliError::runtime(format!(
            "target at index {page_index} is type '{}', not page",
            rest_page.kind
        )));
    }

    let mut cdp = CdpSession::connect(endpoint).await?;
    let page = cdp.page_for_rest_target(rest_page).await?;
    let source = source_id.unwrap_or_else(|| infer_source_id(&rest_page.url));
    let capture = capture_page(&page, &source, include_html).await?;
    write_capture_and_report(capture, out.as_deref()).await
}

async fn fetch_url(
    endpoint: &str,
    url: String,
    source_id: String,
    out: Option<PathBuf>,
    include_html: bool,
) -> Result<(), CliError> {
    if !is_loopback_endpoint(endpoint) {
        println!(
            "warning\tcdp endpoint is not localhost/127.0.0.1; do not expose CDP to LAN or internet"
        );
    }
    let rest_pages = fetch_pages(endpoint)?;
    let page_index = rest_pages
        .iter()
        .position(|page| page.kind == "page")
        .ok_or_else(|| CliError::runtime("CDP is reachable, but no page target is open"))?;
    let rest_page = rest_pages[page_index].clone();

    let mut cdp = CdpSession::connect(endpoint).await?;
    let page = cdp.page_for_rest_target(&rest_page).await?;
    println!("navigating\t{url}");
    if let Err(err) = page.goto(url).await {
        println!("warning\tCDP Page.navigate did not complete cleanly: {err}");
    }
    settle_rendered_page(&page).await?;
    let capture = capture_page(&page, &source_id, include_html).await?;
    write_capture_and_report(capture, out.as_deref()).await
}

const DEFAULT_SCREENSHOT_WIDTH: u32 = 640;
const DEFAULT_SCREENSHOT_HEIGHT: u32 = 440;
const DEFAULT_SCREENSHOT_WAIT_MS: u64 = 3000;

async fn screenshot_url(
    endpoint: &str,
    url: String,
    out: PathBuf,
    width: Option<u32>,
    height: Option<u32>,
    wait_ms: Option<u64>,
    full_page: bool,
) -> Result<(), CliError> {
    use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams;

    if !is_loopback_endpoint(endpoint) {
        println!(
            "warning\tcdp endpoint is not localhost/127.0.0.1; do not expose CDP to LAN or internet"
        );
    }
    let rest_pages = fetch_pages(endpoint)?;
    let page_index = rest_pages
        .iter()
        .position(|page| page.kind == "page")
        .ok_or_else(|| CliError::runtime("CDP is reachable, but no page target is open"))?;
    let rest_page = rest_pages[page_index].clone();

    let mut cdp = CdpSession::connect(endpoint).await?;
    let page = cdp.page_for_rest_target(&rest_page).await?;

    // Apply a viewport override so map tiles render at a deterministic size.
    let view_w = width.unwrap_or(DEFAULT_SCREENSHOT_WIDTH);
    let view_h = height.unwrap_or(DEFAULT_SCREENSHOT_HEIGHT);
    println!("viewport\t{view_w}x{view_h}");
    if let Err(err) = page
        .execute(SetDeviceMetricsOverrideParams::new(
            i64::from(view_w),
            i64::from(view_h),
            1.0,
            false,
        ))
        .await
    {
        println!("warning\tEmulation.setDeviceMetricsOverride failed: {err}");
    }

    println!("navigating\t{url}");
    if let Err(err) = page.goto(url).await {
        println!("warning\tCDP Page.navigate did not complete cleanly: {err}");
    }
    settle_rendered_page(&page).await?;

    // Wait for the page to signal readiness via document.title. A map page (see
    // snapshot-maps.sh) sets MAP_READY only AFTER its tiles have loaded+decoded+painted,
    // and MAP_FAILED on a tile error/timeout — so the shot never fires on a blank tile
    // layer. `--wait` is the MAX wait: for a page that never sets the sentinel (any
    // non-map page) this just sleeps the full duration, preserving the old behavior.
    let wait = wait_ms.unwrap_or(DEFAULT_SCREENSHOT_WAIT_MS);
    wait_for_map_ready_or_timeout(&page, wait).await?;

    let params = ScreenshotParams::builder()
        .format(CaptureScreenshotFormat::Png)
        .full_page(full_page)
        .build();
    let bytes = page
        .screenshot(params)
        .await
        .map_err(|err| CliError::runtime(format!("CDP Page.captureScreenshot failed: {err}")))?;

    std::fs::write(&out, &bytes).map_err(|err| {
        CliError::runtime(format!(
            "failed to write screenshot to {}: {err}",
            out.display()
        ))
    })?;

    println!("screenshot\t{}\t{} bytes", out.display(), bytes.len());
    Ok(())
}

async fn fetch_interact(
    endpoint: &str,
    url: String,
    source_id: String,
    out: Option<PathBuf>,
    include_html: bool,
    steps: Vec<InteractionStep>,
    profile_override: bool,
) -> Result<(), CliError> {
    if !is_loopback_endpoint(endpoint) {
        println!(
            "warning\tcdp endpoint is not localhost/127.0.0.1; do not expose CDP to LAN or internet"
        );
    }
    let rest_pages = fetch_pages(endpoint)?;
    let page_index = rest_pages
        .iter()
        .position(|page| page.kind == "page")
        .ok_or_else(|| CliError::runtime("CDP is reachable, but no page target is open"))?;
    let rest_page = rest_pages[page_index].clone();

    let mut cdp = CdpSession::connect(endpoint).await?;
    guard_interactive_profile(&mut cdp, profile_override).await?;
    let page = cdp.page_for_rest_target(&rest_page).await?;
    println!("navigating\t{url}");
    if let Err(err) = page.goto(url).await {
        println!("warning\tCDP Page.navigate did not complete cleanly: {err}");
    }
    settle_rendered_page(&page).await?;
    execute_interaction_steps(&page, &steps).await?;
    settle_rendered_page(&page).await?;
    let capture = capture_page(&page, &source_id, include_html).await?;
    write_capture_and_report(capture, out.as_deref()).await
}

struct CdpSession {
    browser: Browser,
    handler: tokio::task::JoinHandle<()>,
}

impl CdpSession {
    async fn connect(endpoint: &str) -> Result<Self, CliError> {
        let (browser, mut handler) = Browser::connect(endpoint.to_string())
            .await
            .map_err(|err| {
                CliError::runtime(format!(
                    "failed to connect to Chrome CDP WebSocket through {endpoint}: {err}. Launch Windows Chrome with remote debugging and retry."
                ))
            })?;
        let handler = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        Ok(Self { browser, handler })
    }

    async fn page_for_rest_target(
        &mut self,
        rest_page: &CdpPage,
    ) -> Result<chromiumoxide::Page, CliError> {
        self.browser.fetch_targets().await.map_err(|err| {
            CliError::runtime(format!("failed to fetch existing CDP targets: {err}"))
        })?;
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(page) = self.browser.get_page(TargetId::new(&rest_page.id)).await {
            return Ok(page);
        }

        let pages = self.browser.pages().await.map_err(|err| {
            CliError::runtime(format!("failed to fetch attachable CDP pages: {err}"))
        })?;
        for page in pages {
            let page_url = page.url().await.unwrap_or_default().unwrap_or_default();
            if page_url == rest_page.url {
                return Ok(page);
            }
        }

        Err(CliError::runtime(format!(
            "CDP page target {} ({}) was listed by /json/list but not attachable",
            rest_page.id, rest_page.url
        )))
    }

    async fn browser_command_line(&mut self) -> Result<Vec<String>, CliError> {
        let response = self
            .browser
            .execute(GetBrowserCommandLineParams::default())
            .await
            .map_err(|err| {
                CliError::runtime(format!(
                    "failed to read Browser.getBrowserCommandLine for profile guard: {err}"
                ))
            })?;
        Ok(response.result.arguments)
    }
}

impl Drop for CdpSession {
    fn drop(&mut self) {
        self.handler.abort();
    }
}

#[derive(Serialize, serde::Deserialize)]
struct TravelCapture {
    #[serde(default = "default_schema")]
    schema: String,
    source_id: String,
    #[serde(default)]
    captured_at: String,
    url: String,
    #[serde(default)]
    title: String,
    raw_text: String,
    #[serde(default)]
    links: Vec<CaptureLink>,
    #[serde(default)]
    tables: Vec<Vec<Vec<String>>>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    screenshot_path: Option<String>,
}

fn default_schema() -> String {
    "travel-capture-v1".to_string()
}

#[derive(Serialize, serde::Deserialize)]
struct CaptureLink {
    text: String,
    href: String,
}

/// Wait until the page signals readiness via document.title, bounded by `wait_ms`.
///
/// A map page sets `MAP_READY` (tiles loaded+decoded+painted) or `MAP_FAILED:<reason>`
/// (tile error / timeout). We return as soon as `MAP_READY` appears, error on
/// `MAP_FAILED`, and otherwise keep waiting up to the deadline. For a page that never
/// sets either sentinel (any non-map page), this sleeps the full `wait_ms` — preserving
/// the previous blind-wait behavior. `wait_ms == 0` returns immediately.
async fn wait_for_map_ready_or_timeout(
    page: &chromiumoxide::Page,
    wait_ms: u64,
) -> Result<(), CliError> {
    if wait_ms == 0 {
        return Ok(());
    }
    println!("waiting\t{wait_ms}ms (max; returns on MAP_READY)");
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        // get_title() failures are transient (page mid-navigation) — treat as "not yet".
        let title = page
            .get_title()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        if title.starts_with("MAP_READY") {
            println!("ready\t{title}");
            return Ok(());
        }
        if title.starts_with("MAP_FAILED") {
            return Err(CliError::runtime(format!("map render failed: {title}")));
        }
        if tokio::time::Instant::now() >= deadline {
            // No sentinel before the deadline: non-map page (full wait elapsed) OR a map
            // page whose tiles never settled. Capture anyway — the fail-loud PNG guard
            // downstream rejects a truly broken capture.
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn settle_rendered_page(page: &chromiumoxide::Page) -> Result<(), CliError> {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(SETTLE_TIMEOUT_SECS);
    let mut last_len = 0usize;
    let mut stable_count = 0usize;
    while tokio::time::Instant::now() < deadline {
        let raw_text = evaluate_string(page, "() => document.body ? document.body.innerText : ''")
            .await
            .unwrap_or_default();
        let len = raw_text.trim().len();
        if len > 0 && len == last_len {
            stable_count += 1;
            if stable_count >= 2 {
                return Ok(());
            }
        } else {
            stable_count = 0;
            last_len = len;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}

async fn execute_interaction_steps(
    page: &chromiumoxide::Page,
    steps: &[InteractionStep],
) -> Result<(), CliError> {
    for (index, step) in steps.iter().enumerate() {
        let step_index = index + 1;
        match step {
            InteractionStep::Fill { selector, value } => {
                println!("step\t{step_index}\tfill\t{selector}");
                let element = page.find_element(selector).await.map_err(|err| {
                    CliError::runtime(format!(
                        "step {step_index} failed: selector not found for fill '{selector}': {err}"
                    ))
                })?;
                element.focus().await.map_err(|err| {
                    CliError::runtime(format!(
                        "step {step_index} failed: could not focus '{selector}': {err}"
                    ))
                })?;
                set_control_value(page, selector, value, step_index).await?;
            }
            InteractionStep::Click { selector } => {
                println!("step\t{step_index}\tclick\t{selector}");
                let element = page.find_element(selector).await.map_err(|err| {
                    CliError::runtime(format!(
                        "step {step_index} failed: selector not found for click '{selector}': {err}"
                    ))
                })?;
                element.click().await.map_err(|err| {
                    CliError::runtime(format!(
                        "step {step_index} failed: could not click '{selector}': {err}"
                    ))
                })?;
            }
            InteractionStep::Wait { ms } => {
                println!("step\t{step_index}\twait\t{ms}ms");
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
            InteractionStep::WaitFor { selector } => {
                println!("step\t{step_index}\twaitfor\t{selector}");
                wait_for_selector(page, selector, step_index).await?;
            }
        }
    }
    Ok(())
}

async fn set_control_value(
    page: &chromiumoxide::Page,
    selector: &str,
    value: &str,
    step_index: usize,
) -> Result<(), CliError> {
    let selector_json = serde_json::to_string(selector)
        .map_err(|err| CliError::runtime(format!("failed to encode selector JSON: {err}")))?;
    let value_json = serde_json::to_string(value)
        .map_err(|err| CliError::runtime(format!("failed to encode fill value JSON: {err}")))?;
    let function = format!(
        r#"() => {{
  const el = document.querySelector({selector_json});
  if (!el) return "__CHROMEPORT_MISSING__";
  const value = {value_json};
  const tag = (el.tagName || "").toLowerCase();
  const oldValue = el.value;
  if (tag === "input" || tag === "textarea" || tag === "select") {{
    const proto = tag === "textarea"
      ? HTMLTextAreaElement.prototype
      : tag === "select"
        ? HTMLSelectElement.prototype
        : HTMLInputElement.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(proto, "value");
    if (descriptor && descriptor.set) {{
      descriptor.set.call(el, value);
    }} else {{
      el.value = value;
    }}
  }} else if (el.isContentEditable) {{
    el.textContent = value;
  }} else {{
    return "__CHROMEPORT_UNSUPPORTED__:" + tag;
  }}
  if (el._valueTracker) {{
    el._valueTracker.setValue(oldValue);
  }}
  el.dispatchEvent(new Event("input", {{ bubbles: true }}));
  el.dispatchEvent(new Event("change", {{ bubbles: true }}));
  return tag + ":" + (el.value || el.textContent || "");
}}"#
    );
    let result = evaluate_string(page, &function).await?;
    if result == "__CHROMEPORT_MISSING__" {
        return Err(CliError::runtime(format!(
            "step {step_index} failed: selector disappeared before fill '{selector}'"
        )));
    }
    if let Some(tag) = result.strip_prefix("__CHROMEPORT_UNSUPPORTED__:") {
        return Err(CliError::runtime(format!(
            "step {step_index} failed: selector '{selector}' resolved to unsupported fill target '{tag}'"
        )));
    }
    Ok(())
}

async fn wait_for_selector(
    page: &chromiumoxide::Page,
    selector: &str,
    step_index: usize,
) -> Result<(), CliError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while tokio::time::Instant::now() < deadline {
        match page.find_element(selector).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_error = Some(err.to_string());
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    Err(CliError::runtime(format!(
        "step {step_index} failed: selector not found within 10s for waitfor '{selector}'{}",
        last_error.map(|err| format!(": {err}")).unwrap_or_default()
    )))
}

async fn guard_interactive_profile(
    cdp: &mut CdpSession,
    profile_override: bool,
) -> Result<(), CliError> {
    match cdp.browser_command_line().await {
        Ok(arguments) => {
            if profile_arguments_show_dedicated_profile(&arguments) {
                println!("profile_guard\tok\tdedicated automation profile detected");
                Ok(())
            } else if profile_override {
                println!(
                    "warning\tprofile guard could not confirm a dedicated automation profile; proceeding because --i-understand-profile was provided"
                );
                Ok(())
            } else {
                let user_data_dir = browser_user_data_dir(&arguments).unwrap_or_else(|| "-".into());
                Err(CliError::runtime(format!(
                    "interactive fetch refused: connected Chrome is not confirmed as the dedicated automation profile (user-data-dir={user_data_dir}). Relaunch Chrome with C:\\chrome-profiles\\travel-browser or pass --i-understand-profile to override for this run."
                )))
            }
        }
        Err(err) if profile_override => {
            println!(
                "warning\tprofile guard could not read Chrome command line ({err}); proceeding because --i-understand-profile was provided"
            );
            Ok(())
        }
        Err(err) => Err(CliError::runtime(format!(
            "interactive fetch refused: could not confirm the dedicated automation profile: {err}. Relaunch Chrome with --enable-automation and --user-data-dir=C:\\chrome-profiles\\travel-browser, or pass --i-understand-profile to override for this run."
        ))),
    }
}

fn profile_arguments_show_dedicated_profile(arguments: &[String]) -> bool {
    let Some(user_data_dir) = browser_user_data_dir(arguments) else {
        return false;
    };
    let lower = user_data_dir.to_ascii_lowercase().replace('\\', "/");
    lower.contains("chrome-profiles/")
        && (lower.contains("/travel-browser") || lower.contains("/codex-browser"))
}

fn browser_user_data_dir(arguments: &[String]) -> Option<String> {
    arguments.iter().find_map(|arg| {
        arg.strip_prefix("--user-data-dir=")
            .or_else(|| arg.strip_prefix("/user-data-dir="))
            .map(str::to_string)
    })
}

async fn capture_page(
    page: &chromiumoxide::Page,
    source_id: &str,
    include_html: bool,
) -> Result<TravelCapture, CliError> {
    let title = page
        .get_title()
        .await
        .map_err(|err| CliError::runtime(format!("failed to read page title: {err}")))?
        .unwrap_or_default();
    let url = page
        .url()
        .await
        .map_err(|err| CliError::runtime(format!("failed to read page URL: {err}")))?
        .unwrap_or_default();
    let raw_text =
        evaluate_string(page, "() => document.body ? document.body.innerText : ''").await?;
    let links = evaluate_links(page).await?;
    let tables = evaluate_tables(page).await?;
    let html = if include_html {
        Some(
            evaluate_string(
                page,
                "() => document.documentElement ? document.documentElement.outerHTML : ''",
            )
            .await?,
        )
    } else {
        None
    };

    Ok(TravelCapture {
        schema: "travel-capture-v1".to_string(),
        source_id: source_id.to_string(),
        captured_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        url,
        title,
        raw_text,
        links,
        tables,
        html,
        screenshot_path: None,
    })
}

async fn evaluate_string(page: &chromiumoxide::Page, function: &str) -> Result<String, CliError> {
    page.evaluate_function(function)
        .await
        .map_err(|err| CliError::runtime(format!("Runtime.evaluate failed: {err}")))?
        .into_value::<String>()
        .map_err(|err| CliError::runtime(format!("Runtime.evaluate returned non-string: {err}")))
}

async fn evaluate_links(page: &chromiumoxide::Page) -> Result<Vec<CaptureLink>, CliError> {
    page.evaluate_function(
        r#"() => Array.from(document.querySelectorAll("a[href]")).slice(0, 1000).map((a) => ({
  text: (a.innerText || a.textContent || "").trim(),
  href: a.href || a.getAttribute("href") || ""
}))"#,
    )
    .await
    .map_err(|err| CliError::runtime(format!("failed to evaluate links: {err}")))?
    .into_value::<Vec<CaptureLink>>()
    .map_err(|err| CliError::runtime(format!("links capture returned unexpected shape: {err}")))
}

async fn evaluate_tables(page: &chromiumoxide::Page) -> Result<Vec<Vec<Vec<String>>>, CliError> {
    page.evaluate_function(
        r#"() => Array.from(document.querySelectorAll("table,[role='table']")).slice(0, 100).map((table) =>
  Array.from(table.querySelectorAll("tr,[role='row']")).slice(0, 500).map((row) =>
    Array.from(row.querySelectorAll("th,td,[role='columnheader'],[role='cell']")).slice(0, 100).map((cell) =>
      (cell.innerText || cell.textContent || "").trim()
    )
  )
)"#,
    )
    .await
    .map_err(|err| CliError::runtime(format!("failed to evaluate tables: {err}")))?
    .into_value::<Vec<Vec<Vec<String>>>>()
    .map_err(|err| CliError::runtime(format!("tables capture returned unexpected shape: {err}")))
}

/// Store a capture as a row in the Turso `captures` table (raw_text is plain
/// text). No JSON file is written. Prints the capture_id to use with `parse`.
async fn write_capture_and_report(
    capture: TravelCapture,
    _out: Option<&Path>,
) -> Result<(), CliError> {
    let captured_at = if capture.captured_at.is_empty() {
        now_iso()
    } else {
        capture.captured_at.clone()
    };
    let capture_id = format!(
        "{}-{}",
        sanitize_filename(&capture.source_id),
        captured_at.replace([':', '-'], "")
    );

    let db = turso::TravelDb::connect_write()
        .await
        .map_err(CliError::runtime)?;
    db.insert_capture(
        &capture_id,
        &capture.source_id,
        &capture.url,
        &capture.title,
        &captured_at,
        &capture.raw_text,
    )
    .await
    .map_err(CliError::runtime)?;

    println!("capture_id\t{capture_id}");
    println!("title\t{}", capture.title);
    println!("url\t{}", capture.url);
    println!("raw_text_chars\t{}", capture.raw_text.chars().count());
    println!("snippet\t{}", content_snippet(&capture.raw_text));
    Ok(())
}

fn sanitize_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "capture".to_string()
    } else {
        sanitized
    }
}

fn infer_source_id(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("liontravel.com") {
        "liontravel"
    } else if lower.contains("settour.com") {
        "settour"
    } else if lower.contains("besttour.com") {
        "besttour"
    } else if lower.contains("lifetour.com") {
        "lifetour"
    } else if lower.contains("travel4u.com") {
        "travel4u"
    } else if lower.contains("tigerair") {
        "tigerair"
    } else if lower.contains("agoda") {
        "agoda"
    } else if lower.contains("google.com/travel/flights") {
        "google_flights"
    } else if lower.contains("trip.com") {
        "trip"
    } else if lower.contains("eztravel.com.tw") {
        "eztravel"
    } else {
        "unknown"
    }
    .to_string()
}

fn content_snippet(raw_text: &str) -> String {
    let collapsed = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "China Airlines",
        "中華航空",
        "華航",
        "Naha",
        "那霸",
        "沖繩",
        "OKA",
        "TPE",
        "出發日期",
        "機票",
        "航班",
        "飯店",
    ] {
        if let Some(index) = collapsed.find(needle) {
            return snippet_around_byte_index(&collapsed, index, 180, 420);
        }
    }
    collapsed.chars().take(600).collect()
}

fn snippet_around_byte_index(text: &str, byte_index: usize, before: usize, after: usize) -> String {
    let mut char_positions: Vec<usize> = text.char_indices().map(|(idx, _)| idx).collect();
    char_positions.push(text.len());
    let center = char_positions
        .iter()
        .position(|idx| *idx >= byte_index)
        .unwrap_or(0);
    let start_char = center.saturating_sub(before);
    let end_char = (center + after).min(char_positions.len().saturating_sub(1));
    text[char_positions[start_char]..char_positions[end_char]].to_string()
}

fn fetch_pages(endpoint: &str) -> Result<Vec<CdpPage>, CliError> {
    let body = cdp_get(endpoint, "/json/list")?;
    Ok(parse_pages(&body))
}

#[derive(Clone, Debug)]
struct CdpPage {
    id: String,
    kind: String,
    title: String,
    url: String,
}

fn parse_pages(body: &str) -> Vec<CdpPage> {
    split_top_level_objects(body)
        .into_iter()
        .map(|object| CdpPage {
            id: json_string_value(object, "id").unwrap_or_default(),
            kind: json_string_value(object, "type").unwrap_or_default(),
            title: json_string_value(object, "title").unwrap_or_default(),
            url: json_string_value(object, "url").unwrap_or_default(),
        })
        .filter(|page| page.kind == "page" || !page.url.is_empty())
        .collect()
}

fn split_top_level_objects(input: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(s) = start.take() {
                            objects.push(&input[s..=idx]);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    objects
}

fn json_string_value(input: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut index = 0usize;
    while let Some(pos) = input[index..].find(&needle) {
        index += pos + needle.len();
        let rest = input[index..].trim_start();
        if !rest.starts_with(':') {
            continue;
        }
        let rest = rest[1..].trim_start();
        if !rest.starts_with('"') {
            continue;
        }
        return parse_json_string(rest);
    }
    None
}

fn parse_json_string(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = input.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if escaped {
            match ch {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(value) = u16::from_str_radix(&hex, 16) {
                        if let Some(decoded) = char::from_u32(value as u32) {
                            out.push(decoded);
                        }
                    }
                }
                other => out.push(other),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

fn cdp_get(endpoint: &str, path: &str) -> Result<String, CliError> {
    let parsed = Endpoint::parse(endpoint)?;
    let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port)).map_err(|err| {
        CliError::runtime(format!(
            "failed to connect to {}:{}: {err}. Launch Windows Chrome with remote debugging and retry.",
            parsed.host, parsed.port
        ))
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| CliError::runtime(format!("failed to set read timeout: {err}")))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| CliError::runtime(format!("failed to set write timeout: {err}")))?;

    let request_path = format!("{}{}", parsed.base_path, path);
    let request = format!(
        "GET {request_path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        parsed.host, parsed.port
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| CliError::runtime(format!("failed to write HTTP request: {err}")))?;

    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                response_bytes.extend_from_slice(&buf[..n]);
                if response_has_complete_body(&response_bytes)
                    || body_only_json_is_complete(&response_bytes)
                {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    IoErrorKind::WouldBlock | IoErrorKind::TimedOut | IoErrorKind::Interrupted
                ) =>
            {
                if response_bytes.is_empty() {
                    return Err(CliError::runtime(format!(
                        "failed to read HTTP response: {err}"
                    )));
                }
                break;
            }
            Err(err) => {
                return Err(CliError::runtime(format!(
                    "failed to read HTTP response: {err}"
                )));
            }
        }
    }
    let response = String::from_utf8_lossy(&response_bytes).to_string();

    if let Some((head, body)) = response.split_once("\r\n\r\n") {
        if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
            let status = head.lines().next().unwrap_or("unknown status");
            return Err(CliError::runtime(format!("CDP endpoint returned {status}")));
        }
        return Ok(body.to_string());
    }

    let trimmed = response.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(response);
    }

    Err(CliError::runtime("invalid HTTP response from CDP endpoint"))
}

fn response_has_complete_body(bytes: &[u8]) -> bool {
    let Some(header_end) = find_header_end(bytes) else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let Some(content_len) = header_content_length(&head) else {
        return false;
    };
    bytes.len().saturating_sub(header_end + 4) >= content_len
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn header_content_length(head: &str) -> Option<usize> {
    for line in head.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                return value.trim().parse::<usize>().ok();
            }
        }
    }
    None
}

fn body_only_json_is_complete(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim_start();
    let Some(open) = trimmed.chars().next() else {
        return false;
    };
    let close = match open {
        '{' => '}',
        '[' => ']',
        _ => return false,
    };

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut saw_open = false;
    for ch in trimmed.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            c if c == open => {
                depth += 1;
                saw_open = true;
            }
            c if c == close => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
                if depth == 0 && saw_open {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[derive(Debug)]
struct Endpoint {
    host: String,
    port: u16,
    base_path: String,
}

impl Endpoint {
    fn parse(raw: &str) -> Result<Self, CliError> {
        let rest = raw
            .strip_prefix("http://")
            .ok_or_else(|| CliError::usage("only http:// CDP endpoints are supported for now"))?;
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = authority.rsplit_once(':').ok_or_else(|| {
            CliError::usage("endpoint must include host and port, e.g. http://127.0.0.1:9222")
        })?;
        let port = port
            .parse::<u16>()
            .map_err(|_| CliError::usage("endpoint port must be a valid u16"))?;
        let base_path = if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        };
        Ok(Self {
            host: host.to_string(),
            port,
            base_path,
        })
    }
}

// ---------------------------------------------------------------------------
async fn db_query(sql: String) -> Result<(), CliError> {
    let db = turso::TravelDb::connect_read()
        .await
        .map_err(CliError::runtime)?;
    // Plain-text table output (no JSON): tab-separated header + rows.
    let (cols, rows) = db.query_table(&sql).await.map_err(CliError::runtime)?;
    if rows.is_empty() {
        println!("(0 rows)");
        return Ok(());
    }
    println!("{}", cols.join("\t"));
    for row in &rows {
        // newlines in cells would break the line-oriented table; flatten them
        let cells: Vec<String> = row.iter().map(|c| c.replace('\n', " ")).collect();
        println!("{}", cells.join("\t"));
    }
    println!("({} rows)", rows.len());
    Ok(())
}

async fn db_exec(sql: String) -> Result<(), CliError> {
    let db = turso::TravelDb::connect_write()
        .await
        .map_err(CliError::runtime)?;
    let changed = db.exec(&sql).await.map_err(CliError::runtime)?;
    println!("rows_affected\t{changed}");
    Ok(())
}

fn db_token_status(tier: TokenTier) -> Result<(), CliError> {
    let status = turso::TravelDb::token_status(tier).map_err(CliError::runtime)?;
    println!("db\t{}", status.db);
    println!("tier\t{}", status.tier);
    println!("source\t{}", status.source);
    println!("issued_at\t{}", status.issued_at);
    println!("expires_at\t{}", status.expires_at);
    println!("cache_path\t{}", status.cache_path);
    Ok(())
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("http://127.0.0.1:")
        || endpoint.starts_with("http://localhost:")
        || endpoint.starts_with("http://[::1]:")
}

#[derive(Debug)]
struct CliError {
    message: String,
    kind: ErrorKind,
}

#[derive(Debug)]
enum ErrorKind {
    Usage,
    Help,
    Runtime,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::Usage,
        }
    }

    fn help(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::Help,
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::Runtime,
        }
    }

    fn exit_code(&self) -> u8 {
        match self.kind {
            ErrorKind::Help => 0,
            ErrorKind::Usage => 2,
            ErrorKind::Runtime => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
