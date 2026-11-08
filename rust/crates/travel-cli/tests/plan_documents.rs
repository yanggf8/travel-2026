use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn cli(args: &[&str], plan_id: &str) -> (bool, String) {
    let out = Command::new(bin())
        .args(args)
        .args(["--plan-id", plan_id])
        .env("TRAVEL_DOCUMENTS_DRY_R2", "1")
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run {args:?}: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn scalar(sql: &str) -> String {
    db_exec(sql).and_then(|rows| rows.scalar()).unwrap_or_default()
}

#[test]
fn document_add_update_validation_list_and_remove() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan_id = format!("zz_test_{tag}_doc");
    let dest = format!("zz_test_{tag}_docdest");
    let pdf = std::env::temp_dir().join(format!("zz-test-{tag}-doc.pdf"));
    let bad = std::env::temp_dir().join(format!("zz-test-{tag}-bad.pdf"));
    let _g = Guard::new({
        let (plan_id, dest, pdf, bad) = (plan_id.clone(), dest.clone(), pdf.clone(), bad.clone());
        move || {
            let _ = std::fs::remove_file(&pdf);
            let _ = std::fs::remove_file(&bad);
            teardown_plan(&plan_id, &dest);
        }
    });
    seed_plan(&plan_id, &dest, 0);
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'T1', 'arrival', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 2, '2026-09-02', 'T2', 'departure', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, variant_of) \
           VALUES ('{plan_id}', '{dest}', 3, '2026-09-01', 'V', 'arrival', 'draft', 1);"
    ))
    .expect("seed trip and variant days");
    std::fs::write(&pdf, b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\n%%EOF\n").expect("write PDF fixture");
    std::fs::write(&bad, b"not a PDF").expect("write non-PDF fixture");
    let pdf = pdf.to_str().unwrap();
    let bad = bad.to_str().unwrap();

    let (ok, out) = cli(
        &["add-document", "flight", "--kind", "flight_eticket", "--label", "Flight e-ticket",
          "--label-zh", "電子機票", "--file", pdf, "--day", "1", "--day", "2", "--sort", "4"],
        &plan_id,
    );
    assert!(ok, "add failed: {out}");
    assert!(out.contains("added flight (flight_eticket) days=1,2"), "out={out}");
    assert!(out.contains(&format!("-> /voucher/{plan_id}/doc-flight.pdf")), "out={out}");
    assert!(out.contains("dry-run: would upload"), "out={out}");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_documents WHERE plan_id='{plan_id}'")), "1");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_document_days WHERE plan_id='{plan_id}' AND doc_key='flight'")), "2");
    assert_eq!(scalar(&format!("SELECT sort_order AS n FROM plan_documents WHERE plan_id='{plan_id}' AND doc_key='flight'")), "4");
    assert_eq!(scalar(&format!("SELECT version AS n FROM plans WHERE plan_id='{plan_id}'")), "1");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan_id}' AND event='document_added'")), "2");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_event_data WHERE plan_id='{plan_id}' AND key='doc_key' AND value='flight'")), "2");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{plan_id}' AND command_type='add-document'")), "1");

    db_exec(&format!(
        "UPDATE plan_documents SET created_at='2000-01-01 00:00:00' WHERE plan_id='{plan_id}' AND doc_key='flight'"
    ))
    .expect("set distinguishable created_at");
    let (ok, out) = cli(
        &["add-document", "flight", "--kind", "flight_eticket", "--label", "Updated e-ticket",
          "--label-zh", "更新機票", "--file", pdf, "--day", "1"],
        &plan_id,
    );
    assert!(ok, "re-add failed: {out}");
    assert!(out.contains("updated flight (flight_eticket) days=1"), "out={out}");
    assert_eq!(scalar(&format!("SELECT created_at AS v FROM plan_documents WHERE plan_id='{plan_id}' AND doc_key='flight'")), "2000-01-01 00:00:00");
    assert_eq!(scalar(&format!("SELECT label AS v FROM plan_documents WHERE plan_id='{plan_id}' AND doc_key='flight'")), "Updated e-ticket");
    assert_eq!(scalar(&format!("SELECT group_concat(day_number) AS v FROM plan_document_days WHERE plan_id='{plan_id}' AND doc_key='flight'")), "1");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan_id}' AND event='document_updated'")), "2");
    assert_eq!(scalar(&format!("SELECT version AS n FROM plans WHERE plan_id='{plan_id}'")), "2");

    let (ok, out) = cli(&["documents"], &plan_id);
    assert!(ok, "list failed: {out}");
    assert!(out.contains("flight\tflight_eticket\t1\t更新機票"), "out={out}");
    assert!(!out.contains('{'), "list output must be plain text, not JSON: {out}");

    let (ok, out) = cli(
        &["add-document", "variant-doc", "--kind", "tour_voucher", "--label", "Tour",
          "--label-zh", "旅遊券", "--file", pdf, "--day", "3"],
        &plan_id,
    );
    assert!(!ok && out.contains("not a trip day"), "variant day must fail: {out}");
    assert!(!out.contains("would upload"), "invalid day must fail before R2: {out}");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_documents WHERE plan_id='{plan_id}'")), "1");
    assert_eq!(scalar(&format!("SELECT version AS n FROM plans WHERE plan_id='{plan_id}'")), "2");

    let (ok, out) = cli(
        &["add-document", "bad-doc", "--kind", "tour_voucher", "--label", "Bad",
          "--label-zh", "錯誤", "--file", bad],
        &plan_id,
    );
    assert!(!ok && out.contains("%PDF-"), "non-PDF must fail: {out}");
    assert!(!out.contains("would upload"), "non-PDF must fail before R2: {out}");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_documents WHERE plan_id='{plan_id}'")), "1");
    assert_eq!(scalar(&format!("SELECT version AS n FROM plans WHERE plan_id='{plan_id}'")), "2");

    let (ok, out) = cli(&["remove-document", "flight"], &plan_id);
    assert!(ok, "remove failed: {out}");
    assert!(out.contains("dry-run: would delete"), "out={out}");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_documents WHERE plan_id='{plan_id}'")), "0");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_document_days WHERE plan_id='{plan_id}'")), "0");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan_id}' AND event='document_removed'")), "2");
    assert_eq!(scalar(&format!("SELECT version AS n FROM plans WHERE plan_id='{plan_id}'")), "3");

    let (ok, out) = cli(&["remove-document", "flight"], &plan_id);
    assert!(!ok && out.contains("does not exist"), "missing remove must fail: {out}");
    assert_eq!(scalar(&format!("SELECT version AS n FROM plans WHERE plan_id='{plan_id}'")), "3");
}
