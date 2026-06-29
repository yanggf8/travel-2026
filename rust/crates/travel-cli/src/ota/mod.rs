mod claim;
mod common;
mod enqueue;
mod observations;
mod parse;
mod regex_parse;
mod write_offers;

pub async fn dispatch(sub: &str, rest: &[String]) -> Result<(), String> {
    match sub {
        "enqueue" => enqueue::run(rest).await,
        "claim" => claim::run_claim(rest).await,
        "heartbeat" => claim::run_heartbeat(rest).await,
        "finish" => claim::run_finish(rest).await,
        "reap-stale" => claim::run_reap_stale(rest).await,
        "parse" => parse::run(rest).await,
        "write-offers" => write_offers::run(rest).await,
        "observations" => observations::run(rest).await,
        _ => Err(format!(
            "unknown ota subcommand: {sub}\n\
             Usage: travel ota {{enqueue|claim|heartbeat|finish|reap-stale|parse|write-offers|observations}} ..."
        )),
    }
}