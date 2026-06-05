use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::target::TargetId;
use chrono::Utc;
use futures_util::StreamExt;
use serde::Serialize;
use std::env;
use std::fmt;
use std::fs;
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:9222";
const DEFAULT_CAPTURE_DIR: &str = "scrapes/captures";
const SETTLE_TIMEOUT_SECS: u64 = 18;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("travel-scraper: {err}");
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
        }) => run_async(capture_snapshot(&cli.endpoint, page_index, source_id, out)),
        Command::Scrape(ScrapeCommand::Url {
            url,
            source_id,
            out,
        }) => run_async(scrape_url(&cli.endpoint, url, source_id, out)),
    }
}

struct Cli {
    endpoint: String,
    command: Command,
}

enum Command {
    Browser(BrowserCommand),
    Scrape(ScrapeCommand),
}

enum BrowserCommand {
    Doctor,
    Pages,
    Snapshot {
        page_index: usize,
        source_id: Option<String>,
        out: Option<PathBuf>,
    },
}

enum ScrapeCommand {
    Url {
        url: String,
        source_id: String,
        out: Option<PathBuf>,
    },
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Self, CliError> {
        let mut endpoint = env::var("TRAVEL_SCRAPER_CDP_ENDPOINT")
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
                    }),
                })
            }
            [group, cmd, url, rest @ ..] if group == "scrape" && cmd == "url" => {
                let source_id = option_value(rest, "--source")
                    .ok_or_else(|| CliError::usage("scrape url requires --source <id>"))?
                    .to_string();
                Ok(Self {
                    endpoint,
                    command: Command::Scrape(ScrapeCommand::Url {
                        url: url.to_string(),
                        source_id,
                        out: option_value(rest, "--out").map(PathBuf::from),
                    }),
                })
            }
            _ => Err(CliError::usage(usage())),
        }
    }
}

fn usage() -> &'static str {
    "Usage:\n  travel-scraper [--endpoint http://127.0.0.1:9222] browser doctor\n  travel-scraper [--endpoint http://127.0.0.1:9222] browser pages\n  travel-scraper [--endpoint http://127.0.0.1:9222] browser snapshot --page <N> [--source <id>] [--out <path-or-dir>]\n  travel-scraper [--endpoint http://127.0.0.1:9222] scrape url <url> --source <id> [--out <dir>]\n\nEnv:\n  TRAVEL_SCRAPER_CDP_ENDPOINT overrides the default endpoint.\n"
}

fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let index = args.iter().position(|arg| arg == name)?;
    args.get(index + 1).map(String::as_str)
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
    let capture = capture_page(&page, &source).await?;
    write_capture_and_report(capture, out.as_deref())
}

async fn scrape_url(
    endpoint: &str,
    url: String,
    source_id: String,
    out: Option<PathBuf>,
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
    let capture = capture_page(&page, &source_id).await?;
    write_capture_and_report(capture, out.as_deref())
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
}

impl Drop for CdpSession {
    fn drop(&mut self) {
        self.handler.abort();
    }
}

#[derive(Serialize)]
struct TravelCapture {
    schema: &'static str,
    source_id: String,
    captured_at: String,
    url: String,
    title: String,
    raw_text: String,
    links: Vec<CaptureLink>,
    tables: Vec<Vec<Vec<String>>>,
    html: Option<String>,
    screenshot_path: Option<String>,
}

#[derive(Serialize, serde::Deserialize)]
struct CaptureLink {
    text: String,
    href: String,
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

async fn capture_page(
    page: &chromiumoxide::Page,
    source_id: &str,
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

    Ok(TravelCapture {
        schema: "travel-capture-v1",
        source_id: source_id.to_string(),
        captured_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        url,
        title,
        raw_text,
        links,
        tables,
        html: None,
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

fn write_capture_and_report(capture: TravelCapture, out: Option<&Path>) -> Result<(), CliError> {
    let path = capture_output_path(out, &capture.source_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::runtime(format!(
                "failed to create capture directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    let json = serde_json::to_string_pretty(&capture)
        .map_err(|err| CliError::runtime(format!("failed to serialize capture JSON: {err}")))?;
    fs::write(&path, json).map_err(|err| {
        CliError::runtime(format!("failed to write capture {}: {err}", path.display()))
    })?;

    println!("capture\t{}", path.display());
    println!("title\t{}", capture.title);
    println!("url\t{}", capture.url);
    println!("raw_text_chars\t{}", capture.raw_text.chars().count());
    println!("snippet\t{}", content_snippet(&capture.raw_text));
    Ok(())
}

fn capture_output_path(out: Option<&Path>, source_id: &str) -> Result<PathBuf, CliError> {
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let filename = format!("{}-{timestamp}.json", sanitize_filename(source_id));
    match out {
        Some(path) if path.extension().and_then(|ext| ext.to_str()) == Some("json") => {
            Ok(path.to_path_buf())
        }
        Some(path) => Ok(path.join(filename)),
        None => Ok(Path::new(DEFAULT_CAPTURE_DIR).join(filename)),
    }
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
    } else {
        "unknown"
    }
    .to_string()
}

fn content_snippet(raw_text: &str) -> String {
    let collapsed = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "HOTEL",
        "AZAT",
        "China Airlines",
        "中華航空",
        "華航",
        "Naha",
        "那霸",
        "5天",
        "6/12",
        "2026/06/12",
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
