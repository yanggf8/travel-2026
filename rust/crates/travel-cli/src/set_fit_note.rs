//! `travel set-fit-note` — write the dashboard FIT comparison paragraph or one
//! agency's reason, and optionally mark that agency Recommended.
//!
//! Audited: one timeline `plan_events` row (`fit_note_set` / `fit_note_cleared`),
//! its KV, `operation_runs`, and a `plans.version` bump. The note itself is a
//! `plan_fit_notes` upsert. Lowest-price and price-delta badges are not stored.

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_timeline_sort_order, now_db_datetime, now_rfc3339,
    read_version, record_operation, resolve_active_destination,
};
use travel_db::repo::plan_fit_notes::{self, FitNote};

const MAX_BODY: usize = 1500;

#[derive(Debug)]
struct Parsed {
    /// `None` is the section comparison paragraph (`source_id ''`).
    source_id: Option<String>,
    zh: Option<String>,
    en: Option<String>,
    room_zh: Option<String>,
    room_en: Option<String>,
    recommend: bool,
    clear_recommend: bool,
    clear: bool,
    dest: Option<String>,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse(args)?;
    let conn = crate::db::connect_write().await?;

    conn.execute("BEGIN", libsql::params![])
        .await
        .map_err(|e| format!("set-fit-note BEGIN failed: {e}"))?;

    let outcome = write_note(&conn, &plan_id, &parsed).await;
    match outcome {
        Ok(lines) => {
            conn.execute("COMMIT", libsql::params![])
                .await
                .map_err(|e| format!("set-fit-note COMMIT failed: {e}"))?;
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", libsql::params![]).await;
            Err(e)
        }
    }
}

async fn write_note(
    conn: &libsql::Connection,
    plan_id: &str,
    parsed: &Parsed,
) -> Result<Vec<String>, String> {
    let destination = resolve_active_destination(conn, plan_id, parsed.dest.as_deref()).await?;
    let source_id = parsed.source_id.clone().unwrap_or_default();
    let source_label = if source_id.is_empty() {
        "(comparison)".to_string()
    } else {
        source_id.clone()
    };
    let now_db = now_db_datetime();
    let now_iso = now_rfc3339();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    if parsed.clear {
        let affected = plan_fit_notes::delete(conn, plan_id, &destination, &source_id).await?;
        if affected == 0 {
            return Err(format!(
                "set-fit-note: no note to clear for {plan_id} / {destination} source={source_label}"
            ));
        }
        emit_audit(
            conn,
            plan_id,
            "fit_note_cleared",
            &[
                ("destination", destination.clone()),
                ("source", source_label.clone()),
            ],
            &format!("set-fit-note clear {destination} source={source_label}"),
            version_before,
            version_after,
            &now_db,
            &now_iso,
        )
        .await?;
        return Ok(vec![format!(
            "✅ Cleared FIT note {plan_id} / {destination} source={source_label}"
        )]);
    }

    let existing = plan_fit_notes::get(conn, plan_id, &destination, &source_id).await?;
    let body_zh = match &parsed.zh {
        Some(z) => z.clone(),
        None => existing
            .as_ref()
            .map(|n| n.body_zh.clone())
            .unwrap_or_default(),
    };
    let body_en = match &parsed.en {
        Some(e) => e.clone(),
        None => existing
            .as_ref()
            .map(|n| n.body_en.clone())
            .unwrap_or_default(),
    };
    let room_zh = match &parsed.room_zh {
        Some(r) => r.clone(),
        None => existing
            .as_ref()
            .map(|n| n.room_zh.clone())
            .unwrap_or_default(),
    };
    let room_en = match &parsed.room_en {
        Some(r) => r.clone(),
        None => existing
            .as_ref()
            .map(|n| n.room_en.clone())
            .unwrap_or_default(),
    };
    let recommended = if parsed.recommend {
        1
    } else if parsed.clear_recommend {
        0
    } else {
        existing.as_ref().map(|n| n.recommended).unwrap_or(0)
    };
    if existing.is_none() && body_zh.is_empty() {
        return Err("set-fit-note: a new note needs --zh \"<text>\"".to_string());
    }
    if body_zh.is_empty() && body_en.is_empty() {
        return Err("set-fit-note: the note text is empty".to_string());
    }
    if recommended == 1 && source_id.is_empty() {
        return Err(
            "set-fit-note: --recommend applies to one agency (--source <id>), not the comparison paragraph"
                .to_string(),
        );
    }
    if recommended == 1 {
        plan_fit_notes::clear_other_recommended(conn, plan_id, &destination, &source_id, &now_db)
            .await?;
    }
    let note = FitNote {
        recommended,
        body_zh: body_zh.clone(),
        body_en: body_en.clone(),
        room_zh: room_zh.clone(),
        room_en: room_en.clone(),
    };
    plan_fit_notes::upsert(conn, plan_id, &destination, &source_id, &note, &now_db).await?;

    let kv = vec![
        ("destination", destination.clone()),
        ("source", source_label.clone()),
        ("recommended", recommended.to_string()),
        ("body_zh", body_zh.clone()),
        ("body_en", body_en.clone()),
        ("room_zh", room_zh.clone()),
        ("room_en", room_en.clone()),
    ];
    emit_audit(
        conn,
        plan_id,
        "fit_note_set",
        &kv,
        &format!("set-fit-note {destination} source={source_label} recommended={recommended}"),
        version_before,
        version_after,
        &now_db,
        &now_iso,
    )
    .await?;

    let rec = if recommended == 1 { "yes" } else { "no" };
    let mut lines = vec![
        format!("✅ FIT note saved"),
        format!("plan: {plan_id}"),
        format!("destination: {destination}"),
        format!("source: {source_label}"),
        format!("recommended: {rec}"),
    ];
    if !body_zh.is_empty() {
        lines.push(format!("zh: {body_zh}"));
    }
    if !body_en.is_empty() {
        lines.push(format!("en: {body_en}"));
    }
    if !room_zh.is_empty() {
        lines.push(format!("room_zh: {room_zh}"));
    }
    if !room_en.is_empty() {
        lines.push(format!("room_en: {room_en}"));
    }
    Ok(lines)
}

#[allow(clippy::too_many_arguments)]
async fn emit_audit(
    conn: &libsql::Connection,
    plan_id: &str,
    event: &str,
    kv: &[(&str, String)],
    summary: &str,
    version_before: i64,
    version_after: i64,
    now_db: &str,
    now_iso: &str,
) -> Result<(), String> {
    let sort_order = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn, plan_id, "timeline", "", "", sort_order, event, now_iso, None, None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "timeline", "", "", sort_order, kv).await?;
    record_operation(
        conn,
        plan_id,
        "set-fit-note",
        summary,
        version_before,
        version_after,
        now_db,
    )
    .await?;
    Ok(())
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut source: Option<String> = None;
    let mut zh: Option<String> = None;
    let mut en: Option<String> = None;
    let mut room_zh: Option<String> = None;
    let mut room_en: Option<String> = None;
    let mut recommend = false;
    let mut clear_recommend = false;
    let mut clear = false;
    let mut dest: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                let v = need_value(args, &mut i, "--source")?;
                source = Some(normalize_source(&v)?);
            }
            "--zh" => {
                zh = Some(need_body(args, &mut i, "--zh")?);
            }
            "--en" => {
                en = Some(need_body(args, &mut i, "--en")?);
            }
            "--room-zh" => {
                room_zh = Some(need_body(args, &mut i, "--room-zh")?);
            }
            "--room-en" => {
                room_en = Some(need_body(args, &mut i, "--room-en")?);
            }
            "--dest" => {
                dest = Some(need_value(args, &mut i, "--dest")?);
            }
            "--recommend" => recommend = true,
            "--clear-recommend" => clear_recommend = true,
            "--clear" => clear = true,
            f if crate::plan_resolver::is_resolver_flag(f) => {
                i += 1;
                if i >= args.len() {
                    return Err(format!("set-fit-note: missing value for {f}"));
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("set-fit-note: unknown argument: {other}"));
            }
            other => {
                return Err(format!(
                    "set-fit-note: unexpected argument '{other}'. Usage: set-fit-note [--source <id>] --zh \"<text>\" [--en \"<text>\"] [--recommend] [--dest <slug>]"
                ));
            }
        }
        i += 1;
    }
    if recommend && clear_recommend {
        return Err("set-fit-note: --recommend and --clear-recommend cannot be combined".into());
    }
    if clear && (zh.is_some() || en.is_some() || room_zh.is_some() || room_en.is_some() || recommend || clear_recommend) {
        return Err(
            "set-fit-note: --clear removes the note; do not combine it with --zh, --en, or --recommend"
                .into(),
        );
    }
    if recommend && source.as_deref().unwrap_or("").is_empty() {
        return Err(
            "set-fit-note: --recommend applies to one agency (--source <id>), not the comparison paragraph"
                .into(),
        );
    }
    if !clear && zh.is_none() && en.is_none() && room_zh.is_none() && room_en.is_none() && !recommend && !clear_recommend {
        return Err("set-fit-note: pass --zh \"<text>\", --room-zh \"<size>\", --recommend, or --clear".into());
    }
    Ok(Parsed {
        source_id: source,
        zh,
        en,
        room_zh,
        room_en,
        recommend,
        clear_recommend,
        clear,
        dest,
    })
}

fn need_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    let v = args
        .get(*i)
        .ok_or_else(|| format!("set-fit-note: missing value for {flag}"))?;
    if v.starts_with('-') && v != "-" {
        return Err(format!("set-fit-note: missing value for {flag}"));
    }
    Ok(v.clone())
}

fn need_body(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    let v = need_value(args, i, flag)?;
    let trimmed = v.trim();
    if trimmed.is_empty() {
        return Err(format!("set-fit-note: {flag} is empty"));
    }
    if trimmed.chars().count() > MAX_BODY {
        return Err(format!(
            "set-fit-note: {flag} is longer than {MAX_BODY} characters"
        ));
    }
    Ok(trimmed.to_string())
}

/// `compare` (and omitting --source) is the section paragraph. An agency id is
/// the lowercase slug already stored on `offers.source_id`.
fn normalize_source(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("compare") {
        return Ok(String::new());
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        || s.len() > 40
    {
        return Err(format!(
            "set-fit-note: --source '{s}' must be a lowercase agency id (e.g. lifetour) or 'compare'"
        ));
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_compare_paragraph_and_agency_pick() {
        let compare = parse(&s(&["--zh", "三家比較", "--plan-id", "osaka-nov-2026"])).unwrap();
        assert!(compare.source_id.is_none());
        assert_eq!(compare.zh.as_deref(), Some("三家比較"));
        assert!(!compare.recommend);

        let pick = parse(&s(&[
            "--source",
            "lifetour",
            "--zh",
            "最低價",
            "--en",
            "Lowest",
            "--recommend",
        ]))
        .unwrap();
        assert_eq!(pick.source_id.as_deref(), Some("lifetour"));
        assert!(pick.recommend);
        assert_eq!(pick.en.as_deref(), Some("Lowest"));
    }

    #[test]
    fn parse_rejects_recommend_on_the_comparison_paragraph() {
        let err = parse(&s(&["--source", "compare", "--zh", "x", "--recommend"])).unwrap_err();
        assert!(err.contains("--recommend"), "{err}");
    }

    #[test]
    fn parse_rejects_clear_combined_with_text() {
        let err = parse(&s(&["--source", "lifetour", "--clear", "--zh", "x"])).unwrap_err();
        assert!(err.contains("--clear"), "{err}");
    }

    #[test]
    fn parse_rejects_unknown_flag_and_bad_source() {
        assert!(parse(&s(&["--bogus"])).unwrap_err().contains("unknown"));
        assert!(
            parse(&s(&["--source", "LifeTour", "--zh", "x"]))
                .unwrap_err()
                .contains("lowercase")
        );
    }
}
