use std::path::PathBuf;

use libsql::{Connection, params};
use sha2::{Digest, Sha256};
use travel_db::repo::plan_documents::{self, PlanDocument};

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_dest_process_sort_order, next_timeline_sort_order,
    now_db_datetime, now_rfc3339, read_version, record_operation, resolve_active_destination,
};

pub const ADD_USAGE: &str = "travel add-document <doc_key> --plan-id P --kind K --label EN --label-zh ZH --file PATH [--day N]... [--sort N]";
pub const REMOVE_USAGE: &str = "travel remove-document <doc_key> --plan-id P";
pub const LIST_USAGE: &str = "travel documents [--plan-id P]";

const BUCKET: &str = "trip-dashboard-vouchers";
const PROCESS: &str = "process_5_daily_itinerary";
const MAX_PDF_BYTES: usize = 10 * 1024 * 1024;

#[derive(Default)]
struct AddArgs {
    doc_key: String,
    kind: Option<String>,
    label: Option<String>,
    label_zh: Option<String>,
    file: Option<PathBuf>,
    days: Vec<i64>,
    sort_order: i64,
    dest: Option<String>,
}

fn check_doc_key(value: &str) -> Result<(), String> {
    let valid = (1..=48).contains(&value.len())
        && (value.as_bytes()[0].is_ascii_lowercase() || value.as_bytes()[0].is_ascii_digit())
        && value
            .as_bytes()
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if valid {
        Ok(())
    } else {
        Err("doc_key must match ^[a-z0-9][a-z0-9-]{0,47}$".into())
    }
}

fn check_kind(value: &str) -> Result<(), String> {
    if matches!(
        value,
        "flight_eticket" | "hotel_voucher" | "tour_voucher" | "rail_ticket"
    ) {
        Ok(())
    } else {
        Err("--kind must be flight_eticket, hotel_voucher, tour_voucher, or rail_ticket".into())
    }
}

fn check_label(flag: &str, value: &str, max: usize) -> Result<(), String> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max {
        Err(format!("{flag} must be 1-{max} chars (got {count})"))
    } else {
        Ok(())
    }
}

fn check_pdf(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > MAX_PDF_BYTES {
        return Err(format!(
            "PDF size must be 1 byte..10 MiB (got {} bytes)",
            bytes.len()
        ));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err("file is not a PDF (bytes must start with %PDF-)".into());
    }
    Ok(())
}

fn r2_key(plan_id: &str, doc_key: &str) -> String {
    format!("{plan_id}/doc-{doc_key}.pdf")
}

fn parse_add(args: &[String]) -> Result<AddArgs, String> {
    let mut out = AddArgs::default();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if crate::plan_resolver::is_resolver_flag(arg) {
            if args.get(i + 1).is_none() {
                return Err(format!("missing value for {arg}"));
            }
            i += 2;
            continue;
        }
        if arg == "--sort" {
            out.sort_order = args
                .get(i + 1)
                .ok_or("missing value for --sort")?
                .parse::<i64>()
                .map_err(|_| "--sort must be an integer")?;
            i += 2;
            continue;
        }
        if arg == "--day" {
            let day = args
                .get(i + 1)
                .ok_or("missing value for --day")?
                .parse::<i64>()
                .ok()
                .filter(|day| *day >= 1)
                .ok_or("--day must be a positive integer")?;
            out.days.push(day);
            i += 2;
            continue;
        }
        let slot = match arg {
            "--kind" => Some(&mut out.kind),
            "--label" => Some(&mut out.label),
            "--label-zh" => Some(&mut out.label_zh),
            "--dest" => Some(&mut out.dest),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = Some(
                args.get(i + 1)
                    .ok_or_else(|| format!("missing value for {arg}"))?
                    .clone(),
            );
            i += 2;
        } else if arg == "--file" {
            out.file = Some(PathBuf::from(
                args.get(i + 1).ok_or("missing value for --file")?,
            ));
            i += 2;
        } else if arg.starts_with("--") {
            return Err(format!("unknown argument: {arg}"));
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    if positional.len() != 1 {
        return Err(format!("expected exactly one <doc_key>\nUsage: {ADD_USAGE}"));
    }
    out.doc_key = positional.remove(0);
    out.days.sort_unstable();
    out.days.dedup();
    Ok(out)
}

async fn in_tx<T, F>(conn: &Connection, name: &str, body: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    conn.execute("BEGIN", params![])
        .await
        .map_err(|e| format!("{name} BEGIN failed: {e}"))?;
    match body.await {
        Ok(value) => {
            conn.execute("COMMIT", params![])
                .await
                .map_err(|e| format!("{name} COMMIT failed: {e}"))?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", params![]).await;
            Err(error)
        }
    }
}

async fn audit(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    event: &str,
    command: &str,
    document: &PlanDocument,
) -> Result<(), String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let before = read_version(conn, plan_id).await?;
    let kv = [
        ("doc_key", document.doc_key.clone()),
        ("kind", document.kind.clone()),
        ("sha256", document.sha256.clone()),
    ];
    let dp = next_dest_process_sort_order(conn, plan_id, dest, PROCESS).await?;
    insert_event(conn, plan_id, "dest_process", dest, PROCESS, dp, event, &now_iso, None, None).await?;
    insert_kv_rows(conn, plan_id, "dest_process", dest, PROCESS, dp, &kv).await?;
    let tl = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(conn, plan_id, "timeline", "", PROCESS, tl, event, &now_iso, None, None).await?;
    insert_kv_rows(conn, plan_id, "timeline", "", PROCESS, tl, &kv).await?;
    record_operation(
        conn,
        plan_id,
        command,
        &format!("{} {}", event, document.doc_key),
        before,
        before + 1,
        &now_db,
    )
    .await?;
    Ok(())
}

fn upload(path: &PathBuf, key: &str) -> Result<(), String> {
    if std::env::var("TRAVEL_DOCUMENTS_DRY_R2").as_deref() == Ok("1") {
        eprintln!("dry-run: would upload {key}");
        return Ok(());
    }
    let status = crate::snapshot_maps::wrangler_command()
        .args(["wrangler", "r2", "object", "put", &format!("{BUCKET}/{key}"), "--file"])
        .arg(path)
        .args(["--content-type", "application/pdf", "--remote"])
        .status()
        .map_err(|e| format!("failed to run Wrangler upload for {key}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Wrangler upload failed for {key} (exit {status}); no DB row was written"))
    }
}

fn upload_validated_bytes(
    bytes: &[u8],
    plan_id: &str,
    doc_key: &str,
    key: &str,
) -> Result<(), String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("cannot timestamp the PDF upload: {e}"))?
        .as_nanos();
    let temp_path = std::env::temp_dir().join(format!(
        "travel-document-{plan_id}-{doc_key}-{}-{nanos}.pdf",
        std::process::id()
    ));
    // Owner-only (0600) and never an existing path: the PDF carries booking
    // codes and passenger names. Clean up on every exit path, write errors too.
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)
            .map_err(|e| format!("cannot create temporary PDF {}: {e}", temp_path.display()))?
    };
    // From here on the file is ours: remove it on every exit path.
    let written = std::io::Write::write_all(&mut file, bytes);
    drop(file);
    let result = match written {
        Ok(()) => upload(&temp_path, key),
        Err(e) => Err(format!("cannot write temporary PDF {}: {e}", temp_path.display())),
    };
    let _ = std::fs::remove_file(&temp_path);
    result
}

fn delete_object(key: &str) -> Result<(), String> {
    if std::env::var("TRAVEL_DOCUMENTS_DRY_R2").as_deref() == Ok("1") {
        eprintln!("dry-run: would delete {key}");
        return Ok(());
    }
    let retry = format!("npx wrangler r2 object delete {BUCKET}/{key} --remote");
    let status = crate::snapshot_maps::wrangler_command()
        .args(["wrangler", "r2", "object", "delete", &format!("{BUCKET}/{key}"), "--remote"])
        .status()
        .map_err(|e| format!("R2 delete failed after the DB row was removed ({e}); retry exactly: {retry}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "R2 delete failed after the DB row was removed (exit {status}); retry exactly: {retry}"
        ))
    }
}

pub async fn run_add(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_add(args)?;
    check_doc_key(&parsed.doc_key)?;
    let kind = parsed.kind.as_deref().ok_or("--kind is required")?;
    check_kind(kind)?;
    let label = parsed.label.as_deref().ok_or("--label is required")?;
    let label_zh = parsed.label_zh.as_deref().ok_or("--label-zh is required")?;
    check_label("--label", label, 60)?;
    check_label("--label-zh", label_zh, 30)?;
    let file = parsed.file.as_ref().ok_or("--file is required")?;
    let bytes = std::fs::read(file)
        .map_err(|e| format!("cannot read PDF {}: {e}", file.display()))?;
    check_pdf(&bytes)?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let key = r2_key(&plan_id, &parsed.doc_key);

    let conn = crate::db::connect_write().await?;
    if !plan_documents::plan_exists(&conn, &plan_id).await? {
        return Err(format!("plan {plan_id} does not exist"));
    }
    let dest = resolve_active_destination(&conn, &plan_id, parsed.dest.as_deref()).await?;
    for day in &parsed.days {
        if !plan_documents::trip_day_exists(&conn, &plan_id, *day).await? {
            return Err(format!("day {day} is not a trip day of plan {plan_id}"));
        }
    }
    let was_update = plan_documents::get(&conn, &plan_id, &parsed.doc_key)
        .await?
        .is_some();
    upload_validated_bytes(&bytes, &plan_id, &parsed.doc_key, &key)?;

    let document = PlanDocument {
        plan_id: plan_id.clone(),
        doc_key: parsed.doc_key.clone(),
        kind: kind.to_string(),
        label: label.to_string(),
        label_zh: label_zh.to_string(),
        sort_order: parsed.sort_order,
        r2_key: key,
        sha256,
        size_bytes: bytes.len() as i64,
        days: parsed.days.clone(),
        ..Default::default()
    };
    let event = if was_update { "document_updated" } else { "document_added" };
    if let Err(error) = in_tx(&conn, "add-document", async {
        plan_documents::upsert(&conn, &document, &now_db_datetime()).await?;
        plan_documents::replace_days(&conn, &plan_id, &parsed.doc_key, &parsed.days).await?;
        audit(&conn, &plan_id, &dest, event, "add-document", &document).await
    })
    .await
    {
        return Err(format!(
            "R2 now holds the new PDF at {} but the DB write failed ({error}); the dashboard may serve the new file with stale metadata. Re-run the same add-document command to make the DB match, or run remove-document {} to drop it.",
            document.r2_key, document.doc_key
        ));
    }
    let days = if parsed.days.is_empty() {
        "-".to_string()
    } else {
        parsed.days.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
    };
    println!(
        "{} {} ({}) days={} {} bytes sha256={} -> /voucher/{}",
        if was_update { "updated" } else { "added" },
        document.doc_key,
        document.kind,
        days,
        document.size_bytes,
        &document.sha256[..12],
        document.r2_key
    );
    Ok(())
}

pub async fn run_remove(args: &[String], plan_id: String) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if crate::plan_resolver::is_resolver_flag(&args[i]) {
            if args.get(i + 1).is_none() {
                return Err(format!("missing value for {}", args[i]));
            }
            i += 2;
        } else if args[i].starts_with("--") {
            return Err(format!("unknown argument: {}", args[i]));
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    if positional.len() != 1 {
        return Err(format!("expected exactly one <doc_key>\nUsage: {REMOVE_USAGE}"));
    }
    let doc_key = &positional[0];
    check_doc_key(doc_key)?;
    let conn = crate::db::connect_write().await?;
    if !plan_documents::plan_exists(&conn, &plan_id).await? {
        return Err(format!("plan {plan_id} does not exist"));
    }
    let dest = resolve_active_destination(&conn, &plan_id, None).await?;
    let document = plan_documents::get(&conn, &plan_id, doc_key)
        .await?
        .ok_or_else(|| format!("document {doc_key} does not exist for plan {plan_id}"))?;
    in_tx(&conn, "remove-document", async {
        plan_documents::delete(&conn, &plan_id, doc_key).await?;
        audit(&conn, &plan_id, &dest, "document_removed", "remove-document", &document).await
    })
    .await?;
    delete_object(&document.r2_key)?;
    println!("removed {doc_key} -> /voucher/{}", document.r2_key);
    Ok(())
}

pub async fn run_list(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|arg| !crate::plan_resolver::is_resolver_flag(arg)) {
        let mut i = 0;
        while i < args.len() {
            if crate::plan_resolver::is_resolver_flag(&args[i]) {
                i += 2;
            } else {
                return Err(format!("unexpected argument: {}\nUsage: {LIST_USAGE}", args[i]));
            }
        }
    }
    let conn = crate::db::connect_read().await?;
    if !plan_documents::plan_exists(&conn, &plan_id).await? {
        return Err(format!("plan {plan_id} does not exist"));
    }
    let documents = plan_documents::list(&conn, &plan_id).await?;
    println!("DOC_KEY\tKIND\tDAYS\tLABEL_ZH\tSIZE\tSHA256\tR2_KEY");
    for document in documents {
        let days = if document.days.is_empty() {
            "-".to_string()
        } else {
            document.days.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
        };
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            document.doc_key,
            document.kind,
            days,
            document.label_zh,
            document.size_bytes,
            &document.sha256[..12.min(document.sha256.len())],
            document.r2_key
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn doc_key_pattern() {
        for valid in ["a", "0", "flight-1", &"a".repeat(48)] {
            assert!(check_doc_key(valid).is_ok(), "{valid}");
        }
        for invalid in ["", "-a", "A", "a_b", "a.b", &"a".repeat(49)] {
            assert!(check_doc_key(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn label_limits_are_character_counts() {
        assert!(check_label("--label", &"x".repeat(60), 60).is_ok());
        assert!(check_label("--label", &"x".repeat(61), 60).is_err());
        assert!(check_label("--label-zh", &"票".repeat(30), 30).is_ok());
        assert!(check_label("--label-zh", &"票".repeat(31), 30).is_err());
        assert!(check_label("--label", " ", 60).is_err());
    }

    #[test]
    fn pdf_signature_and_size_are_checked() {
        assert!(check_pdf(b"%PDF-1.7").is_ok());
        assert!(check_pdf(b"").is_err());
        assert!(check_pdf(b"<html>").is_err());
        assert!(check_pdf(&vec![b'x'; MAX_PDF_BYTES + 1]).is_err());
    }

    #[test]
    fn day_parsing_is_repeatable_positive_and_deduplicated() {
        let parsed = parse_add(&strings(&[
            "flight", "--day", "2", "--day", "1", "--day", "2", "--kind", "flight_eticket",
        ]))
        .unwrap();
        assert_eq!(parsed.days, [1, 2]);
        assert!(parse_add(&strings(&["flight", "--day", "0"])).is_err());
        assert!(parse_add(&strings(&["flight", "--day", "x"])).is_err());
        assert!(parse_add(&strings(&["flight", "--day"])).is_err());
    }

    #[test]
    fn r2_key_has_the_canonical_shape() {
        assert_eq!(r2_key("kyoto-nov-2026", "flight"), "kyoto-nov-2026/doc-flight.pdf");
    }
}
