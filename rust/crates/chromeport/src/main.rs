use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::browser::GetBrowserCommandLineParams;
use chromiumoxide::cdp::browser_protocol::target::TargetId;
use chrono::Utc;
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
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
            capture_id,
            source_id,
            dry_run,
            allow_source_override,
        }) => run_async(parse_capture(
            capture_id,
            source_id,
            dry_run,
            allow_source_override,
        )),
        Command::Verify {
            source_id,
            capture_id,
            allow_source_override,
        } => run_async(verify_capture(
            source_id,
            capture_id,
            allow_source_override,
        )),
        Command::ParserRules(ParserRulesCommand::SeedDefaults) => {
            run_async(seed_default_parser_rules())
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
    Parse(ParseCommand),
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
    "Usage:\n  chromeport [--endpoint http://127.0.0.1:9222] browser doctor\n  chromeport [--endpoint http://127.0.0.1:9222] browser pages\n  chromeport [--endpoint http://127.0.0.1:9222] browser snapshot --page <N> [--source <id>] [--html]\n  chromeport [--endpoint http://127.0.0.1:9222] fetch url <url> --source <id> [--html]\n  chromeport [--endpoint http://127.0.0.1:9222] fetch interact <url> --source <id> [--step <kind>]... [--html] [--i-understand-profile]\n  chromeport [--endpoint http://127.0.0.1:9222] screenshot <url> --out <path.png> [--width <px>] [--height <px>] [--wait <ms>] [--full-page]\n  chromeport parse capture <capture-id> --source <id> [--dry-run]\n  chromeport verify <source-id> <capture-id>\n  chromeport parser rules seed-defaults\n  chromeport db query <sql>\n  chromeport db exec <sql>\n  chromeport db token-status <read|write|secrets>\n\nCaptures are stored as rows in the Turso `captures` table (plain text, no JSON files).\n`parse capture <capture-id>` reads raw_text from Turso, parses via parser_rules, and\nimports offers straight to Turso (`--dry-run` prints plain text instead).\n`verify <source-id> <capture-id>` is read-only: it prints rule regexes and extracted field status.\n\nSteps:\n  --step 'fill:SEL=VALUE'\n  --step 'click:SEL'\n  --step 'wait:MS'\n  --step 'waitfor:SEL'\n\nEnv:\n  CHROMEPORT_CDP_ENDPOINT overrides the default endpoint.\n  Turso credentials are resolved through minted tier tokens via turso-util; run `turso auth login` if token resolution fails.\n"
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
// Parser stage: Turso captures.raw_text + Turso parser_rules -> canonical offer rows.
//
// Pure transformation after rule lookup: no browser, no CDP. `--dry-run`
// renders plain text; normal mode imports directly into Turso offers:
//   { source_id, package_type, url, scraped_at,
//     dates:{departure_date,return_date,duration_nights},
//     price:{per_person,price_total,currency},
//     flight:{outbound,return}, hotel:{name} }
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct OfferDates {
    #[serde(default)]
    departure_date: String,
    #[serde(default)]
    return_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    duration_nights: Option<u32>,
}

#[derive(Serialize, Deserialize, Default)]
struct OfferPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    per_person: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    price_total: Option<i64>,
    #[serde(default)]
    currency: String,
}

#[derive(Serialize, Deserialize, Default)]
struct OfferFlightLeg {
    #[serde(skip_serializing_if = "String::is_empty")]
    #[serde(default)]
    flight_number: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    #[serde(default)]
    airline: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    #[serde(default)]
    date: String,
}

#[derive(Serialize, Deserialize, Default)]
struct OfferFlight {
    #[serde(default)]
    outbound: OfferFlightLeg,
    #[serde(rename = "return")]
    #[serde(default)]
    return_leg: OfferFlightLeg,
}

#[derive(Serialize, Deserialize, Default)]
struct OfferHotel {
    #[serde(default)]
    name: String,
}

#[derive(Serialize, Deserialize)]
struct CanonicalOfferOut {
    source_id: String,
    package_type: String,
    url: String,
    scraped_at: String,
    dates: OfferDates,
    price: OfferPrice,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    flight: Option<OfferFlight>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    hotel: Option<OfferHotel>,
}

async fn seed_default_parser_rules() -> Result<(), CliError> {
    let db = turso::TravelDb::connect_write()
        .await
        .map_err(CliError::runtime)?;
    db.ensure_parser_rules_table()
        .await
        .map_err(CliError::runtime)?;

    let mut changed = 0_u64;
    for rule in default_parser_rules() {
        changed += db
            .upsert_parser_rule(&rule)
            .await
            .map_err(CliError::runtime)?;
        println!(
            "seeded\t{}\tcustom={}",
            rule.source_id, rule.has_custom_parser
        );
    }
    println!("rows_affected\t{changed}");
    Ok(())
}

fn default_parser_rules() -> Vec<turso::ParserRule> {
    let fetched_at = Some(now_iso());
    vec![
        turso::ParserRule {
            source_id: "settour".to_string(),
            product_kind: "fit".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2}).*?~.*?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"共(\d+)晚".to_string(),
            nights_is_days: false,
            price_marker: "機加酒未稅總價".to_string(),
            price_amount_rx: r"([\d,]{4,})".to_string(),
            price_basis: "total".to_string(),
            pax_divisor: 2,
            flight_rx: r"([A-Z]{2}\d{3,4})".to_string(),
            hotel_anchor_rx: r"飯店.*入住".to_string(),
            airline_rx: String::new(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: true,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "liontravel".to_string(),
            product_kind: "fit".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2}).*?~.*?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"共(\d+)晚".to_string(),
            nights_is_days: false,
            price_marker: "總金額".to_string(),
            price_amount_rx: r"(?:TWD\s*)?([\d,]{4,})".to_string(),
            price_basis: "total".to_string(),
            pax_divisor: 2,
            flight_rx: r"([A-Z]{2}\d{3,4})".to_string(),
            hotel_anchor_rx: r"飯店".to_string(),
            airline_rx: String::new(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "lifetour".to_string(),
            product_kind: "fit".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2}).*?~.*?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)\s*天\s*(\d+)\s*夜".to_string(),
            nights_is_days: false,
            price_marker: "NT$".to_string(),
            price_amount_rx: r"([\d,]{4,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 2,
            flight_rx: r"([A-Z]{1,2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"^住宿$".to_string(),
            airline_rx: String::new(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "besttour".to_string(),
            product_kind: "fit".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2}).*?~.*?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)\s*[天日]".to_string(),
            nights_is_days: true,
            price_marker: "售價".to_string(),
            price_amount_rx: r"(?:NT?\$?\s*)?([\d,]{4,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 2,
            flight_rx: r"([A-Z]{1,2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"^(住宿|飯店|旅館|酒店)$".to_string(),
            airline_rx: String::new(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "travel4u".to_string(),
            product_kind: "fit".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2}).*?~.*?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"([５６４７\d]+)\s*[天日]".to_string(),
            nights_is_days: true,
            price_marker: "售價".to_string(),
            price_amount_rx: r"(?:NT?\$?\s*)?([\d,]{4,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 2,
            flight_rx: r"([A-Z]{1,2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"^(住宿|飯店|旅館|酒店)$".to_string(),
            airline_rx: String::new(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "tigerair".to_string(),
            product_kind: "flight".to_string(),
            date_range_rx: r"(?:出發日期\s*)?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)".to_string(),
            nights_is_days: false,
            price_marker: "TWD".to_string(),
            price_amount_rx: r"([\d,]{3,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 1,
            flight_rx: r"\b(IT\s*\d{3,4})\b".to_string(),
            hotel_anchor_rx: r"^$".to_string(),
            airline_rx: r"(台灣虎航|Tigerair Taiwan|Tigerair)".to_string(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "google_flights".to_string(),
            product_kind: "flight".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)".to_string(),
            nights_is_days: false,
            price_marker: "$".to_string(),
            price_amount_rx: r"([\d,]{3,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 1,
            flight_rx: r"([A-Z]{2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"^$".to_string(),
            airline_rx: r"(捷星日本航空|捷星航空|樂桃航空|亞洲航空 X|亞洲航空|長榮航空|全日空航空|中華航空|日本航空|星宇航空|台灣虎航|酷航|泰越捷航空|泰國獅航|國泰航空|華信航空)".to_string(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "trip".to_string(),
            product_kind: "flight".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)".to_string(),
            nights_is_days: false,
            price_marker: "US$".to_string(),
            price_amount_rx: r"([\d,]{2,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 1,
            flight_rx: r"([A-Z]{2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"^$".to_string(),
            airline_rx: r"(?m)^([A-Za-z][A-Za-z .'-]+(?:Airlines|Airways|Aviation|Air|Jetstar|Peach|Scoot|Tigerair)?)$".to_string(),
            hotel_name_rx: String::new(),
            currency: "USD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "agoda".to_string(),
            product_kind: "hotel".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2}).*?~.*?(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)\s*晚".to_string(),
            nights_is_days: false,
            price_marker: "NT$".to_string(),
            price_amount_rx: r"([\d,]{3,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 1,
            flight_rx: r"([A-Z]{2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"(飯店|酒店|旅館|民宿|青旅|Hotel|Inn|Resort|Hostel|House|Suites?)"
                .to_string(),
            airline_rx: String::new(),
            hotel_name_rx: r"(?m)^(.+?)\s*\([A-Za-z\s&'.,-]*(?:Hotel|Inn|Resort|Hostel|House|Suites?)[A-Za-z\s&'.,-]*\)".to_string(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at: fetched_at.clone(),
        },
        turso::ParserRule {
            source_id: "eztravel".to_string(),
            product_kind: "flight".to_string(),
            date_range_rx: r"(\d{4}/\d{2}/\d{2})".to_string(),
            nights_rx: r"(\d+)".to_string(),
            nights_is_days: false,
            price_marker: "TWD".to_string(),
            price_amount_rx: r"([\d,]{3,})".to_string(),
            price_basis: "per_person".to_string(),
            pax_divisor: 1,
            flight_rx: r"([A-Z]{2}\d{2,4})".to_string(),
            hotel_anchor_rx: r"^$".to_string(),
            airline_rx: r"(台灣虎航|中華航空|長榮航空|星宇航空|樂桃航空|酷航|捷星航空|國泰航空|全日空|日本航空)".to_string(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: false,
            source_url: None,
            fetched_at,
        },
    ]
}

/// Parse a stored capture (by capture_id, read from the Turso captures table)
/// into offers and import them straight to Turso — no JSON file boundary.
/// `--dry-run` prints the parsed offers as plain text instead of importing.
async fn parse_capture(
    capture_id: String,
    source_id: String,
    dry_run: bool,
    allow_source_override: bool,
) -> Result<(), CliError> {
    let db = if dry_run {
        turso::TravelDb::connect_read().await
    } else {
        turso::TravelDb::connect_write().await
    }
    .map_err(CliError::runtime)?;

    // Read the capture's raw_text from Turso (plain text, not a JSON file).
    let (stored_source, url, raw_text) = db
        .get_capture(&capture_id)
        .await
        .map_err(CliError::runtime)?
        .ok_or_else(|| {
            CliError::runtime(format!(
                "no capture with capture_id='{capture_id}' in Turso; run a fetch/snapshot first"
            ))
        })?;
    if stored_source != source_id && !allow_source_override {
        return Err(CliError::runtime(format!(
            "capture '{capture_id}' was captured as source '{stored_source}' but --source is '{source_id}'; \
             pass --allow-source-override to parse it under a different source"
        )));
    }
    let capture = TravelCapture {
        schema: default_schema(),
        source_id: source_id.clone(),
        captured_at: now_iso(),
        url,
        title: String::new(),
        raw_text,
        links: Vec::new(),
        tables: Vec::new(),
        html: None,
        screenshot_path: None,
    };

    let rule = db
        .parser_rule(&source_id)
        .await
        .map_err(CliError::runtime)?
        .ok_or_else(|| {
            CliError::runtime(format!(
                "missing parser_rules row for source_id='{source_id}'; run `chromeport parser rules seed-defaults` or insert a rule row"
            ))
        })?;

    let offers = if rule.has_custom_parser {
        match source_id.as_str() {
            "settour" => parse_settour(&capture)?,
            other => {
                return Err(CliError::runtime(format!(
                    "parser_rules has_custom_parser=1 for source '{other}', but no Rust override is implemented"
                )));
            }
        }
    } else {
        parse_generic(&capture, &rule)?
    };

    if offers.is_empty() {
        return Err(CliError::runtime(
            "parser produced 0 offers from the capture (structured failure — check the captured raw_text)",
        ));
    }

    if dry_run {
        // plain-text view of what would be imported
        for o in &offers {
            print_offer_plain(o);
        }
        println!("offers\t{}", offers.len());
        println!("dry_run\ttrue");
        return Ok(());
    }

    // Import straight to Turso — no offers JSON file.
    let mut imported = 0u64;
    for o in &offers {
        let row = offer_to_row(o, Some(format!("capture:{capture_id}")));
        if row
            .scraped_at
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err(CliError::runtime(format!(
                "offer '{}' missing scraped_at (required by offers PK)",
                row.id
            )));
        }
        imported += db.insert_offer(&row).await.map_err(CliError::runtime)?;
    }
    println!("offers\t{}", offers.len());
    println!("imported\t{imported}");
    Ok(())
}

/// Verify one stored capture against one parser_rules row without importing.
async fn verify_capture(
    source_id: String,
    capture_id: String,
    allow_source_override: bool,
) -> Result<(), CliError> {
    let db = turso::TravelDb::connect_read()
        .await
        .map_err(CliError::runtime)?;
    let (stored_source, url, raw_text) = db
        .get_capture(&capture_id)
        .await
        .map_err(CliError::runtime)?
        .ok_or_else(|| {
            CliError::runtime(format!(
                "no capture with capture_id='{capture_id}' in Turso; run a fetch/snapshot first"
            ))
        })?;
    if stored_source != source_id && !allow_source_override {
        return Err(CliError::runtime(format!(
            "capture '{capture_id}' was captured as source '{stored_source}' but verify --source is '{source_id}'; \
             pass --allow-source-override to verify it under a different source"
        )));
    }
    let rule = db
        .parser_rule(&source_id)
        .await
        .map_err(CliError::runtime)?
        .ok_or_else(|| {
            CliError::runtime(format!(
                "missing parser_rules row for source_id='{source_id}'; run `chromeport parser rules seed-defaults` or insert a rule row"
            ))
        })?;
    let capture = TravelCapture {
        schema: default_schema(),
        source_id: source_id.clone(),
        captured_at: now_iso(),
        url,
        title: String::new(),
        raw_text,
        links: Vec::new(),
        tables: Vec::new(),
        html: None,
        screenshot_path: None,
    };

    println!("verify\t{source_id}\t{capture_id}");
    println!("capture_source\t{stored_source}");
    println!("url\t{}", capture.url);
    println!("raw_text_chars\t{}", capture.raw_text.chars().count());
    println!("snippet\t{}", content_snippet(&capture.raw_text));
    print_rule_report(&rule);
    print_field_report("depart_return", verify_date_range(&capture.raw_text, &rule));
    print_field_report("nights", verify_nights(&capture.raw_text, &rule));
    print_field_report("price", verify_price(&capture.raw_text, &rule));
    print_field_report("flight", verify_flight(&capture.raw_text, &rule));
    print_field_report("airline", verify_airline(&capture.raw_text, &rule));
    print_field_report("hotel", verify_hotel(&capture.raw_text, &rule));

    match if rule.has_custom_parser {
        match source_id.as_str() {
            "settour" => parse_settour(&capture),
            other => Err(CliError::runtime(format!(
                "parser_rules has_custom_parser=1 for source '{other}', but no Rust override is implemented"
            ))),
        }
    } else {
        parse_generic(&capture, &rule)
    } {
        Ok(offers) if offers.is_empty() => {
            println!("overall\tMISSING\tparser produced 0 offers");
        }
        Ok(offers) => {
            println!("overall\tOK\toffers={}", offers.len());
            for offer in &offers {
                print_offer_plain(offer);
            }
        }
        Err(err) => {
            println!("overall\tMISSING\t{err}");
        }
    }

    Ok(())
}

fn print_rule_report(rule: &turso::ParserRule) {
    println!("rule_source\t{}", rule.source_id);
    println!("rule_product_kind\t{}", rule.product_kind);
    println!("rule_has_custom_parser\t{}", rule.has_custom_parser);
    println!("rule_date_range_rx\t{}", rule.date_range_rx);
    println!("rule_nights_rx\t{}", rule.nights_rx);
    println!("rule_price_marker\t{}", rule.price_marker);
    println!("rule_price_amount_rx\t{}", rule.price_amount_rx);
    println!("rule_price_basis\t{}", rule.price_basis);
    println!("rule_pax_divisor\t{}", rule.pax_divisor);
    println!("rule_flight_rx\t{}", rule.flight_rx);
    println!("rule_airline_rx\t{}", rule.airline_rx);
    println!("rule_hotel_anchor_rx\t{}", rule.hotel_anchor_rx);
    println!("rule_hotel_name_rx\t{}", rule.hotel_name_rx);
}

fn print_field_report(name: &str, result: VerifyField) {
    match result {
        VerifyField::Ok(value) => println!("field\t{name}\tOK\t{value}"),
        VerifyField::Missing(reason) => println!("field\t{name}\tMISSING\t{reason}"),
        VerifyField::NotRequired(reason) => println!("field\t{name}\tOK\tnot-required:{reason}"),
    }
}

enum VerifyField {
    Ok(String),
    Missing(String),
    NotRequired(String),
}

fn verify_date_range(raw_text: &str, rule: &turso::ParserRule) -> VerifyField {
    match extract_rule_date_range(raw_text, rule) {
        Ok((depart, ret)) => VerifyField::Ok(format!("{depart}→{ret}")),
        Err(err) => VerifyField::Missing(err.to_string()),
    }
}

fn verify_nights(raw_text: &str, rule: &turso::ParserRule) -> VerifyField {
    if rule.product_kind == "flight" {
        return VerifyField::NotRequired("product_kind=flight".to_string());
    }
    match extract_rule_nights(raw_text, rule) {
        Ok(value) => VerifyField::Ok(value.to_string()),
        Err(err) => VerifyField::Missing(err.to_string()),
    }
}

fn verify_price(raw_text: &str, rule: &turso::ParserRule) -> VerifyField {
    match extract_rule_price(raw_text, rule) {
        Ok((total, pp)) => VerifyField::Ok(format!(
            "pp={pp}\ttotal={}",
            total
                .map(|value| value.to_string())
                .unwrap_or_else(|| "".to_string())
        )),
        Err(err) => VerifyField::Missing(err.to_string()),
    }
}

fn verify_flight(raw_text: &str, rule: &turso::ParserRule) -> VerifyField {
    if rule.product_kind == "hotel" {
        return VerifyField::NotRequired("product_kind=hotel".to_string());
    }
    match extract_rule_flights(raw_text, rule) {
        Ok((Some(outbound), Some(return_leg))) => {
            VerifyField::Ok(format!("{outbound}/{return_leg}"))
        }
        Ok((Some(outbound), None)) => VerifyField::Ok(outbound),
        Ok((None, Some(return_leg))) => VerifyField::Ok(return_leg),
        Ok((None, None)) if rule.product_kind == "flight" => {
            VerifyField::Missing("flight rule found no flight number".to_string())
        }
        Ok((None, None)) => VerifyField::Missing("no flight number matched".to_string()),
        Err(err) => VerifyField::Missing(err.to_string()),
    }
}

fn verify_airline(raw_text: &str, rule: &turso::ParserRule) -> VerifyField {
    if rule.product_kind == "hotel" {
        return VerifyField::NotRequired("product_kind=hotel".to_string());
    }
    match extract_rule_airline(raw_text, rule) {
        Ok(Some(value)) => VerifyField::Ok(value),
        Ok(None) if rule.airline_rx.trim().is_empty() => {
            VerifyField::NotRequired("airline_rx empty".to_string())
        }
        Ok(None) if rule.product_kind == "flight" => {
            match extract_rule_flights(raw_text, rule)
                .ok()
                .and_then(|value| value.0)
            {
                Some(flight) => VerifyField::Ok(
                    infer_airline_from_flight_number(&flight)
                        .unwrap_or_else(|| "flight-number-only".to_string()),
                ),
                None => VerifyField::Missing("airline rule found no airline".to_string()),
            }
        }
        Ok(None) => VerifyField::Missing("no airline matched".to_string()),
        Err(err) => VerifyField::Missing(err.to_string()),
    }
}

fn verify_hotel(raw_text: &str, rule: &turso::ParserRule) -> VerifyField {
    if rule.product_kind == "flight" {
        return VerifyField::NotRequired("product_kind=flight".to_string());
    }
    match extract_rule_hotel(raw_text, rule) {
        Ok(value) => VerifyField::Ok(value),
        Err(err) => VerifyField::Missing(err.to_string()),
    }
}

/// Plain-text (tab-separated) rendering of one parsed offer — no JSON.
fn print_offer_plain(o: &CanonicalOfferOut) {
    let flight = o.flight.as_ref();
    let outbound = flight
        .map(|f| f.outbound.flight_number.as_str())
        .unwrap_or("");
    let airline = flight.map(|f| f.outbound.airline.as_str()).unwrap_or("");
    println!(
        "{}\t{}\t{}→{}\t{}n\tpp={}\ttotal={}\t{}\tflight={}\tairline={}",
        o.source_id,
        o.package_type,
        o.dates.departure_date,
        o.dates.return_date,
        o.dates.duration_nights.unwrap_or(0),
        o.price.per_person.unwrap_or(0),
        o.price.price_total.unwrap_or(0),
        o.hotel.as_ref().map(|h| h.name.as_str()).unwrap_or(""),
        outbound,
        airline,
    );
}

fn parse_generic(
    capture: &TravelCapture,
    rule: &turso::ParserRule,
) -> Result<Vec<CanonicalOfferOut>, CliError> {
    let raw_text = &capture.raw_text;
    let kind = rule.product_kind.as_str();
    let (depart, ret) = extract_rule_date_range(raw_text, rule)?;
    let nights = if kind == "flight" {
        None
    } else {
        Some(extract_rule_nights(raw_text, rule)?)
    };
    let (price_total, per_person) = extract_rule_price(raw_text, rule)?;
    let flights = extract_rule_flights(raw_text, rule)?;
    let airline = extract_rule_airline(raw_text, rule)?;
    let hotel = if kind == "flight" {
        None
    } else {
        Some(extract_rule_hotel(raw_text, rule)?)
    };

    if kind == "flight" && flights.0.is_none() && airline.is_none() {
        return Err(CliError::runtime(format!(
            "parser rule '{}' product_kind=flight requires a flight number from flight_rx or airline from airline_rx",
            rule.source_id
        )));
    }

    let flight = if flights.0.is_some() || flights.1.is_some() || airline.is_some() {
        let outbound_number = flights.0.clone().unwrap_or_default();
        let return_number = flights.1.clone().unwrap_or_default();
        let outbound_airline = airline
            .clone()
            .or_else(|| infer_airline_from_flight_number(&outbound_number))
            .unwrap_or_default();
        let return_airline = if return_number.is_empty() {
            String::new()
        } else {
            airline
                .clone()
                .or_else(|| infer_airline_from_flight_number(&return_number))
                .unwrap_or_default()
        };
        Some(OfferFlight {
            outbound: OfferFlightLeg {
                airline: outbound_airline,
                flight_number: outbound_number,
                date: depart.clone(),
            },
            return_leg: OfferFlightLeg {
                airline: return_airline,
                flight_number: return_number,
                date: ret.clone(),
            },
        })
    } else {
        None
    };

    Ok(vec![CanonicalOfferOut {
        source_id: rule.source_id.clone(),
        package_type: rule.product_kind.clone(),
        url: capture.url.clone(),
        scraped_at: if capture.captured_at.is_empty() {
            now_iso()
        } else {
            capture.captured_at.clone()
        },
        dates: OfferDates {
            departure_date: depart,
            return_date: ret,
            duration_nights: nights,
        },
        price: OfferPrice {
            per_person: Some(per_person),
            price_total,
            currency: rule.currency.clone(),
        },
        flight,
        hotel: hotel.map(|name| OfferHotel { name }),
    }])
}

fn extract_rule_date_range(
    raw_text: &str,
    rule: &turso::ParserRule,
) -> Result<(String, String), CliError> {
    let rx = compile_rule_regex(rule, "date_range_rx", &rule.date_range_rx)?;
    let caps = rx.captures(raw_text).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' could not extract dates with date_range_rx={:?}",
            rule.source_id, rule.date_range_rx
        ))
    })?;
    let depart_raw = caps.get(1).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' date_range_rx must capture departure date in group 1",
            rule.source_id
        ))
    })?;
    let return_raw = caps.get(2);
    if return_raw.is_none() && rule.product_kind != "flight" {
        return Err(CliError::runtime(format!(
            "parser rule '{}' date_range_rx must capture return/check-out date in group 2 for product_kind={}",
            rule.source_id, rule.product_kind
        )));
    }
    Ok((
        normalize_rule_date(depart_raw.as_str(), rule)?,
        match return_raw {
            Some(value) => normalize_rule_date(value.as_str(), rule)?,
            None => String::new(),
        },
    ))
}

fn extract_rule_nights(raw_text: &str, rule: &turso::ParserRule) -> Result<u32, CliError> {
    let rx = compile_rule_regex(rule, "nights_rx", &rule.nights_rx)?;
    let caps = rx.captures(raw_text).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' could not extract nights with nights_rx={:?}",
            rule.source_id, rule.nights_rx
        ))
    })?;
    let group_index = if rule.nights_is_days {
        1
    } else {
        caps.len().saturating_sub(1)
    };
    let raw = caps.get(group_index).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' nights_rx must capture a numeric value",
            rule.source_id
        ))
    })?;
    let value = digits_only(raw.as_str()).parse::<u32>().map_err(|_| {
        CliError::runtime(format!(
            "parser rule '{}' captured invalid nights value {:?}",
            rule.source_id,
            raw.as_str()
        ))
    })?;
    if rule.nights_is_days {
        Ok(value.saturating_sub(1))
    } else {
        Ok(value)
    }
}

fn extract_rule_price(
    raw_text: &str,
    rule: &turso::ParserRule,
) -> Result<(Option<i64>, i64), CliError> {
    let marker_index = raw_text.find(&rule.price_marker).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' could not find price_marker={:?}",
            rule.source_id, rule.price_marker
        ))
    })?;
    let price_scope = &raw_text[marker_index + rule.price_marker.len()..];
    let rx = compile_rule_regex(rule, "price_amount_rx", &rule.price_amount_rx)?;
    let caps = rx.captures(price_scope).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' could not extract price with price_amount_rx={:?} after marker {:?}",
            rule.source_id, rule.price_amount_rx, rule.price_marker
        ))
    })?;
    let raw_amount = caps.get(1).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' price_amount_rx must capture amount in group 1",
            rule.source_id
        ))
    })?;
    let amount = parse_amount(raw_amount.as_str()).ok_or_else(|| {
        CliError::runtime(format!(
            "parser rule '{}' captured invalid price amount {:?}",
            rule.source_id,
            raw_amount.as_str()
        ))
    })?;

    match rule.price_basis.as_str() {
        "total" => {
            if rule.pax_divisor <= 0 {
                return Err(CliError::runtime(format!(
                    "parser rule '{}' has invalid pax_divisor={}",
                    rule.source_id, rule.pax_divisor
                )));
            }
            Ok((
                Some(amount),
                (amount + rule.pax_divisor - 1) / rule.pax_divisor,
            ))
        }
        "per_person" => {
            let total = if rule.pax_divisor > 0 {
                Some(amount * rule.pax_divisor)
            } else {
                None
            };
            Ok((total, amount))
        }
        other => Err(CliError::runtime(format!(
            "parser rule '{}' has unsupported price_basis={other:?}; expected total or per_person",
            rule.source_id
        ))),
    }
}

fn extract_rule_flights(
    raw_text: &str,
    rule: &turso::ParserRule,
) -> Result<(Option<String>, Option<String>), CliError> {
    let rx = compile_rule_regex(rule, "flight_rx", &rule.flight_rx)?;
    let mut found = Vec::new();
    for caps in rx.captures_iter(raw_text) {
        let value = caps
            .get(1)
            .or_else(|| caps.get(0))
            .map(|m| m.as_str().trim().replace(' ', ""))
            .unwrap_or_default();
        if !value.is_empty() && !found.contains(&value) {
            found.push(value);
        }
    }
    Ok((found.first().cloned(), found.get(1).cloned()))
}

fn extract_rule_hotel(raw_text: &str, rule: &turso::ParserRule) -> Result<String, CliError> {
    if !rule.hotel_name_rx.trim().is_empty() {
        let rx = compile_rule_regex(rule, "hotel_name_rx", &rule.hotel_name_rx)?;
        if let Some(caps) = rx.captures(raw_text) {
            if let Some(value) = caps.get(1).or_else(|| caps.get(0)) {
                let name = value.as_str().trim();
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
        return Err(CliError::runtime(format!(
            "parser rule '{}' could not extract hotel with hotel_name_rx={:?}",
            rule.source_id, rule.hotel_name_rx
        )));
    }
    let rx = compile_rule_regex(rule, "hotel_anchor_rx", &rule.hotel_anchor_rx)?;
    let lines: Vec<&str> = raw_text.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        if !rx.is_match(line.trim()) {
            continue;
        }
        for next in lines.iter().skip(idx + 1) {
            let name = next.trim();
            if !name.is_empty() {
                return Ok(name.to_string());
            }
        }
    }
    Err(CliError::runtime(format!(
        "parser rule '{}' could not extract hotel; no non-empty line after hotel_anchor_rx={:?}",
        rule.source_id, rule.hotel_anchor_rx
    )))
}

fn extract_rule_airline(
    raw_text: &str,
    rule: &turso::ParserRule,
) -> Result<Option<String>, CliError> {
    if rule.airline_rx.trim().is_empty() {
        return Ok(None);
    }
    let rx = compile_rule_regex(rule, "airline_rx", &rule.airline_rx)?;
    Ok(rx.captures(raw_text).and_then(|caps| {
        caps.get(1)
            .or_else(|| caps.get(0))
            .map(|m| m.as_str().trim().to_string())
            .filter(|value| !value.is_empty())
    }))
}

fn compile_rule_regex(
    rule: &turso::ParserRule,
    field: &str,
    pattern: &str,
) -> Result<Regex, CliError> {
    Regex::new(pattern).map_err(|err| {
        CliError::runtime(format!(
            "parser rule '{}' has invalid {field} regex {:?}: {err}",
            rule.source_id, pattern
        ))
    })
}

fn normalize_rule_date(value: &str, rule: &turso::ParserRule) -> Result<String, CliError> {
    let normalized = value.trim().replace('/', "-");
    let bytes = normalized.as_bytes();
    let valid = bytes.len() == 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit);
    if valid {
        Ok(normalized)
    } else {
        Err(CliError::runtime(format!(
            "parser rule '{}' captured unsupported date format {:?}; expected YYYY/MM/DD or YYYY-MM-DD",
            rule.source_id, value
        )))
    }
}

fn parse_amount(value: &str) -> Option<i64> {
    let digits = digits_only(value);
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

fn digits_only(value: &str) -> String {
    value.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

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

fn offer_to_row(offer: &CanonicalOfferOut, source_file: Option<String>) -> turso::OfferRow {
    let hotel_name = offer
        .hotel
        .as_ref()
        .map(|hotel| hotel.name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let airline = offer.flight.as_ref().and_then(|flight| {
        let airline = flight.outbound.airline.trim();
        if !airline.is_empty() {
            Some(airline.to_string())
        } else {
            infer_airline_from_flight_number(&flight.outbound.flight_number)
        }
    });

    turso::OfferRow {
        id: offer_row_id(offer),
        source_file,
        source_id: offer.source_id.clone(),
        kind: offer_row_kind(&offer.package_type).to_string(),
        name: None,
        price_per_person: offer.price.per_person,
        currency: if offer.price.currency.trim().is_empty() {
            None
        } else {
            Some(offer.price.currency.clone())
        },
        region: infer_region(&offer.url),
        destination: infer_destination(&offer.url),
        departure_date: non_empty_string(&offer.dates.departure_date),
        return_date: non_empty_string(&offer.dates.return_date),
        nights: offer.dates.duration_nights.map(i64::from),
        availability: None,
        hotel_name,
        airline,
        scraped_at: non_empty_string(&offer.scraped_at),
    }
}

fn offer_row_kind(package_type: &str) -> &str {
    match package_type {
        "flight" => "flight",
        "hotel" => "hotel",
        _ => "package",
    }
}

fn offer_row_id(offer: &CanonicalOfferOut) -> String {
    let product_code = product_code_from_url(&offer.url).unwrap_or_else(|| "unknown".to_string());
    let mut parts = vec![offer.source_id.clone(), sanitize_id_part(&product_code)];
    if !offer.dates.departure_date.is_empty() {
        parts.push(offer.dates.departure_date.replace('-', ""));
    }
    if let Some(nights) = offer.dates.duration_nights {
        parts.push(format!("{nights}n"));
    }
    parts.join("_")
}

fn product_code_from_url(url: &str) -> Option<String> {
    let base = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    let last = base
        .split('/')
        .filter(|part| !part.is_empty())
        .next_back()?;
    let cleaned = last
        .strip_suffix(".html")
        .or_else(|| last.strip_suffix(".htm"))
        .unwrap_or(last);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn infer_airline_from_flight_number(flight_number: &str) -> Option<String> {
    if flight_number.starts_with("IT") {
        Some("台灣虎航".to_string())
    } else if flight_number.starts_with("CI") {
        Some("中華航空".to_string())
    } else if flight_number.starts_with("BR") {
        Some("長榮航空".to_string())
    } else if flight_number.starts_with("JX") {
        Some("星宇航空".to_string())
    } else if flight_number.starts_with("MM") {
        Some("樂桃航空".to_string())
    } else if flight_number.starts_with("TR") {
        Some("酷航".to_string())
    } else if flight_number.starts_with("GK") {
        Some("捷星航空".to_string())
    } else {
        None
    }
}

fn infer_destination(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("kix") || lower.contains("%e4%ba%ac%e9%83%bd") {
        Some("osaka_2026".to_string())
    } else {
        None
    }
}

fn infer_region(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("%e4%ba%ac%e9%83%bd") {
        Some("kyoto".to_string())
    } else if lower.contains("kix") {
        Some("kansai".to_string())
    } else {
        None
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn sanitize_id_part(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// settour FIT parser: raw rendered text -> canonical offer rows.
/// The fit.settour.com.tw product page renders one offer (cheapest flight+hotel
/// combo) per capture. Extracts: dates, nights, flights, total/per-person price,
/// hotel name, product_kind=fit.
fn parse_settour(capture: &TravelCapture) -> Result<Vec<CanonicalOfferOut>, CliError> {
    let rt = &capture.raw_text;

    // Dates: "2026/06/20(六)~2026/06/24(三)"
    let (depart, ret) = extract_date_range(rt);
    // Nights: "共4晚" or "共5日"
    let nights = extract_nights(rt);
    // Total price (機加酒未稅總價 ... NNN,NNN). Per-person derived if adtCount known.
    let total = extract_settour_total(rt);
    // Flight numbers: IT212 / IT211 etc.
    let flights = extract_flight_numbers(rt);
    // Hotel: first non-empty hotel-ish name line; fall back to capture title scan.
    let hotel = extract_settour_hotel(rt);

    if depart.is_empty() {
        return Ok(vec![]); // structured empty — caller turns this into a loud error
    }

    // settour FIT default is 2 pax (adtCount=2 in our flow); per_person = total/2,
    // rounded to nearest (matches the Turso record's rounding: 36587/2 → 18294).
    let per_person = total.map(|t| (t + 1) / 2);

    let flight = if flights.0.is_some() || flights.1.is_some() {
        Some(OfferFlight {
            outbound: OfferFlightLeg {
                flight_number: flights.0.clone().unwrap_or_default(),
                airline: String::new(),
                date: depart.clone(),
            },
            return_leg: OfferFlightLeg {
                flight_number: flights.1.clone().unwrap_or_default(),
                airline: String::new(),
                date: ret.clone(),
            },
        })
    } else {
        None
    };

    let offer = CanonicalOfferOut {
        source_id: "settour".to_string(),
        package_type: "fit".to_string(),
        url: capture.url.clone(),
        scraped_at: if capture.captured_at.is_empty() {
            now_iso()
        } else {
            capture.captured_at.clone()
        },
        dates: OfferDates {
            departure_date: depart,
            return_date: ret,
            duration_nights: nights,
        },
        price: OfferPrice {
            per_person,
            price_total: total,
            currency: "TWD".to_string(),
        },
        flight,
        hotel: hotel.map(|name| OfferHotel { name }),
    };

    Ok(vec![offer])
}

/// "2026/06/20(六)~2026/06/24(三)" -> ("2026-06-20", "2026-06-24").
/// Char-safe: scans a sliding window of 10 chars looking for YYYY/MM/DD.
fn extract_date_range(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut dates: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 10 <= chars.len() {
        let w = &chars[i..i + 10];
        let is_date = w[0].is_ascii_digit()
            && w[1].is_ascii_digit()
            && w[2].is_ascii_digit()
            && w[3].is_ascii_digit()
            && w[4] == '/'
            && w[5].is_ascii_digit()
            && w[6].is_ascii_digit()
            && w[7] == '/'
            && w[8].is_ascii_digit()
            && w[9].is_ascii_digit();
        if is_date {
            let s: String = w.iter().collect::<String>().replace('/', "-");
            if !dates.contains(&s) {
                dates.push(s);
            }
            i += 10;
        } else {
            i += 1;
        }
    }
    let depart = dates.first().cloned().unwrap_or_default();
    let ret = dates.get(1).cloned().unwrap_or_default();
    (depart, ret)
}

/// "共4晚" or "共5日" -> Some(nights). 共N日 means N-1 nights.
fn extract_nights(text: &str) -> Option<u32> {
    if let Some(n) = capture_after(text, "共", "晚") {
        return n.parse::<u32>().ok();
    }
    if let Some(n) = capture_after(text, "共", "日") {
        return n.parse::<u32>().ok().map(|d| d.saturating_sub(1));
    }
    None
}

/// "機加酒未稅總價" followed by an amount like "36,587".
fn extract_settour_total(text: &str) -> Option<i64> {
    let idx = text.find("機加酒未稅總價")?;
    let rest = &text[idx..];
    extract_first_amount(rest)
}

/// First number-with-commas amount (>= 4 digits) in `text`.
fn extract_first_amount(text: &str) -> Option<i64> {
    let mut digits = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            started = true;
        } else if ch == ',' && started {
            continue;
        } else if started {
            if digits.len() >= 4 {
                return digits.parse::<i64>().ok();
            }
            digits.clear();
            started = false;
        }
    }
    if digits.len() >= 4 {
        digits.parse::<i64>().ok()
    } else {
        None
    }
}

/// Flight numbers like IT212 / IT211 — returns (outbound, return) = (first, second distinct).
fn extract_flight_numbers(text: &str) -> (Option<String>, Option<String>) {
    let mut found: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i + 2 < chars.len() {
        if chars[i].is_ascii_uppercase()
            && chars[i + 1].is_ascii_uppercase()
            && chars[i + 2].is_ascii_digit()
        {
            let mut code = String::new();
            code.push(chars[i]);
            code.push(chars[i + 1]);
            let mut j = i + 2;
            while j < chars.len() && chars[j].is_ascii_digit() {
                code.push(chars[j]);
                j += 1;
            }
            if code.len() >= 4 && code.len() <= 6 && !found.contains(&code) {
                found.push(code);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    (found.first().cloned(), found.get(1).cloned())
}

/// settour hotel name: the line immediately AFTER the stay-block anchor
/// "飯店 <City> 入住：…". The anchor line carries the check-in/out dates; the
/// next non-empty line is the real hotel name (e.g. "微笑飯店京都烏丸五條").
fn extract_settour_hotel(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("飯店") && t.contains("入住") {
            // next non-empty line = hotel name
            for next in lines.iter().skip(i + 1) {
                let n = next.trim();
                if n.is_empty() {
                    continue;
                }
                return Some(n.to_string());
            }
        }
    }
    None
}

/// Substring between `start` marker and `end` marker, taking only digits in between.
fn capture_after(text: &str, start: &str, end: &str) -> Option<String> {
    let s = text.find(start)? + start.len();
    let rest = &text[s..];
    let e = rest.find(end)?;
    let mid: String = rest[..e].chars().filter(|c| c.is_ascii_digit()).collect();
    if mid.is_empty() { None } else { Some(mid) }
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
