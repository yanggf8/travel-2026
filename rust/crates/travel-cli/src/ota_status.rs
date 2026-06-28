// `travel ota-status [--type flight|hotel|fit|group_tour]` - DB-native provider coverage view.
//
// This is the single queryable answer to "which providers work for which product types?"
// (replacing the CLAUDE.md OTA status prose). Reads the normalized coverage catalog and
// renders a plain-text table: agent-first, no JSON.

use crate::db;

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel ota-status [--type flight|hotel|fit|group_tour]");
        return Ok(());
    }
    let type_filter = args
        .iter()
        .position(|a| a == "--type")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let conn = db::connect_read().await?;

    // Coverage plus source name plus block-reason description, ordered by type then source.
    let mut sql = String::from(
        "SELECT c.product_type, c.source_id, s.name, c.proven, \
                COALESCE(c.proven_at,''), COALESCE(c.method,''), \
                COALESCE(b.description, c.blocked_reason_code, ''), COALESCE(c.search_url,'') \
         FROM ota_source_coverage c \
         LEFT JOIN ota_sources s ON s.source_id = c.source_id \
         LEFT JOIN coverage_block_reasons b ON b.code = c.blocked_reason_code",
    );
    let params: Vec<libsql::Value> = if let Some(ref t) = type_filter {
        sql.push_str(" WHERE c.product_type = ?1");
        vec![t.clone().into()]
    } else {
        vec![]
    };
    sql.push_str(" ORDER BY c.product_type, c.source_id");

    let mut rows = conn
        .query(&sql, libsql::params_from_iter(params))
        .await
        .map_err(|e| format!("ota_source_coverage query failed: {e}"))?;

    println!("OTA provider coverage (from DB):");
    println!(
        "  {:<11} {:<18} {:<7} {:<11} {:<11} {}",
        "TYPE", "SOURCE", "PROVEN", "PROVEN_AT", "METHOD", "BLOCKED / URL"
    );
    let mut any = false;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        any = true;
        let ptype: String = row.get(0).unwrap_or_default();
        let source: String = row.get(1).unwrap_or_default();
        let name: String = row.get(2).unwrap_or_default();
        let proven: i64 = row.get(3).unwrap_or(0);
        let proven_at: String = row.get(4).unwrap_or_default();
        let method: String = row.get(5).unwrap_or_default();
        let blocked: String = row.get(6).unwrap_or_default();
        let url: String = row.get(7).unwrap_or_default();
        let proven_str = if proven == 1 { "yes" } else { "no" };
        let tail = if !blocked.is_empty() {
            format!("blocked: {blocked}")
        } else {
            url
        };
        // The source_id is the key identifier: print it in full.
        let src_disp = if name.is_empty() {
            source
        } else {
            format!("{source} ({name})")
        };
        println!(
            "  {:<11} {:<18} {:<7} {:<11} {:<11} {}",
            ptype, src_disp, proven_str, proven_at, method, tail
        );
    }
    if !any {
        println!("  (no coverage rows; populate via `travel set-ota-coverage`)");
    }
    Ok(())
}
