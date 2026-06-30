mod claim;
mod common;
mod enqueue;
mod observations;
mod write_offers;

// Agent-first extraction: the coding agent IS the parser. The CLI fetches the capture
// (gwebcdb) and persists the agent's offers (`write-offers`); there is no in-CLI text parser.
// The former regex/custom-parser path (`parse` + `regex_parse` + `settour_parse` + the
// `parser_rules` table) is retired — see the OTA section in CLAUDE.md.
pub async fn dispatch(sub: &str, rest: &[String]) -> Result<(), String> {
    match sub {
        "enqueue" => enqueue::run(rest).await,
        "claim" => claim::run_claim(rest).await,
        "heartbeat" => claim::run_heartbeat(rest).await,
        "finish" => claim::run_finish(rest).await,
        "reap-stale" => claim::run_reap_stale(rest).await,
        "write-offers" => write_offers::run(rest).await,
        "observations" => observations::run(rest).await,
        "parse" => Err(
            "Error: `travel ota parse` (regex/custom parser) is retired. The coding agent is the \
             parser: read the capture's raw_text and pass offers as TSV to `travel ota write-offers`."
                .to_string(),
        ),
        _ => Err(format!(
            "unknown ota subcommand: {sub}\n\
             Usage: travel ota {{enqueue|claim|heartbeat|finish|reap-stale|write-offers|observations}} ..."
        )),
    }
}