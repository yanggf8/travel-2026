mod common;
use common::{bin, db_exec, is_credless, nanos, db_exec_teardown, Guard, seed_plan, teardown_plan};

use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Dependency-order teardown for omiyage + seeded ref rows (non-plan-keyed).
/// Also deletes destination_poi_tags (worklist tests seed tags).
fn omi_teardown(slug: &str) {
    let s = slug.replace('\'', "''");
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_omiyage_locations WHERE slug='{s}'; \
         DELETE FROM destination_omiyage_items WHERE slug='{s}'; \
         DELETE FROM destination_poi_tags WHERE slug='{s}'; \
         DELETE FROM destination_pois WHERE slug='{s}'; \
         DELETE FROM destination_config WHERE slug='{s}';"
    ));
}

/// Seed destination_config + one destination_pois seller.
/// Columns verified against scripts/schema.sql — NO invented `source` column.
fn seed_dest_and_poi(slug: &str, poi: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    let s = slug.replace('\'', "''");
    let p = poi.replace('\'', "''");
    db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{s}', 'Omi Test', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois \
           (slug, poi_id, title, area, nearest_station, hours, address, source_url, fetched_at, confidence) \
           VALUES ('{s}', '{p}', 'Test Depachika', 'namba', 'Namba', '10:00-20:00', NULL, \
                   'https://example.com/poi', '2026-07-12T00:00:00Z', 'verified');"
    ))
    .is_some()
}

#[test]
fn omiyage_tables_exist_after_migrate() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    // both tables queryable with the specified columns (empty is fine)
    assert!(
        db_exec(
            "SELECT slug,item_id,name,category,notes,source_url,fetched_at,confidence \
             FROM destination_omiyage_items LIMIT 0"
        )
        .is_some(),
        "destination_omiyage_items must have the 8 columns"
    );
    assert!(
        db_exec(
            "SELECT slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence \
             FROM destination_omiyage_locations LIMIT 0"
        )
        .is_some(),
        "destination_omiyage_locations must have the 7 columns"
    );
}

#[test]
fn add_omiyage_creates_item_and_location() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_dest_{n}");
    let poi = format!("omi_poi_{n}");
    // Guard FIRST — right after ids bound, BEFORE seed (set_poi_coords.rs:62-68)
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug); // defensive pre-clean
    if !seed_dest_and_poi(&slug, &poi) {
        eprintln!("credless on seed");
        return;
    }

    let (ok, _o, e) = run(&[
        "add-omiyage",
        &slug,
        "tokyo_banana",
        "--name",
        "Tokyo Banana",
        "--category",
        "和菓子",
        "--buy-at",
        &poi,
        "--item-source-url",
        "https://www.tokyobanana.jp/",
        "--item-confidence",
        "verified",
        "--location-source-url",
        "https://example.com/floor",
        "--location-confidence",
        "verified",
    ]);
    assert!(ok, "add-omiyage should succeed; err={e}");

    let it = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_items \
         WHERE slug='{slug}' AND item_id='tokyo_banana'"
    ))
    .expect("count items");
    assert_eq!(it.scalar().as_deref(), Some("1"));

    let lo = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='tokyo_banana' AND poi_id='{poi}'"
    ))
    .expect("count locations");
    assert_eq!(lo.scalar().as_deref(), Some("1"));
}

#[test]
fn add_omiyage_second_seller_keeps_item_unchanged() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_dest2_{n}");
    let poi1 = format!("omi_poi_a_{n}");
    let poi2 = format!("omi_poi_b_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_dest_and_poi(&slug, &poi1) {
        return;
    }
    // second POI same slug (real columns only)
    let s = slug.replace('\'', "''");
    let p2 = poi2.replace('\'', "''");
    if db_exec(&format!(
        "INSERT INTO destination_pois \
           (slug, poi_id, title, area, nearest_station, source_url, fetched_at, confidence) \
           VALUES ('{s}', '{p2}', 'Second Seller', 'umeda', 'Umeda', \
                   'https://example.com/poi2', '2026-07-12T00:00:00Z', 'verified');"
    ))
    .is_none()
    {
        return;
    }

    // first seller + full item bundle
    let (ok1, _, e1) = run(&[
        "add-omiyage", &slug, "item_x",
        "--name", "Item X", "--category", "名產",
        "--buy-at", &poi1,
        "--item-source-url", "https://example.com/item",
        "--item-confidence", "verified",
        "--location-source-url", "https://example.com/loc1",
        "--location-confidence", "verified",
        "--notes", "keep-me",
    ]);
    assert!(ok1, "first add; err={e1}");

    let before = db_exec(&format!(
        "SELECT name||'|'||category||'|'||COALESCE(notes,'')||'|'||source_url||'|'||confidence \
         FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .and_then(|r| r.scalar())
    .expect("item snapshot");

    // second seller: item bundle OMITTED
    let (ok2, _, e2) = run(&[
        "add-omiyage", &slug, "item_x",
        "--buy-at", &poi2,
        "--location-source-url", "https://example.com/loc2",
        "--location-confidence", "reviewed",
    ]);
    assert!(ok2, "second seller; err={e2}");

    let after = db_exec(&format!(
        "SELECT name||'|'||category||'|'||COALESCE(notes,'')||'|'||source_url||'|'||confidence \
         FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .and_then(|r| r.scalar())
    .expect("item after");
    assert_eq!(before, after, "item row must be byte-identical after 2nd seller");

    let nloc = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .expect("loc count");
    assert_eq!(nloc.scalar().as_deref(), Some("2"));

    // sub-case: mismatched SUPPLIED bundle → fail, no 3rd location, item still unchanged
    let (ok_bad, _, e_bad) = run(&[
        "add-omiyage", &slug, "item_x",
        "--name", "WRONG NAME",
        "--buy-at", &poi1,
        "--location-source-url", "https://example.com/loc3",
        "--location-confidence", "verified",
    ]);
    assert!(!ok_bad, "mismatched name must fail; stderr={e_bad}");
    let after_bad = db_exec(&format!(
        "SELECT name||'|'||category||'|'||COALESCE(notes,'')||'|'||source_url||'|'||confidence \
         FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .and_then(|r| r.scalar())
    .expect("item after mismatch");
    assert_eq!(before, after_bad);
    let nloc2 = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .expect("loc count after mismatch");
    assert_eq!(nloc2.scalar().as_deref(), Some("2"));
}

#[test]
fn add_omiyage_same_seller_re_add_refreshes_fetched_at() {
    // Spec L147 / L71: same (slug,item_id,poi_id) re-add → still 1 location row, fetched_at refreshed.
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_re_{n}");
    let poi = format!("omi_poi_re_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }

    let (ok1, _, e1) = run(&[
        "add-omiyage", &slug, "re_item",
        "--name", "Re Item", "--category", "藥妝",
        "--buy-at", &poi,
        "--item-source-url", "https://example.com/re-item",
        "--item-confidence", "verified",
        "--location-source-url", "https://example.com/re-loc",
        "--location-confidence", "verified",
    ]);
    assert!(ok1, "first; err={e1}");

    let fa1 = db_exec(&format!(
        "SELECT fetched_at FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .and_then(|r| r.scalar())
    .expect("fetched_at1");

    std::thread::sleep(std::time::Duration::from_millis(20));

    // re-add same seller (item bundle optional; omit)
    let (ok2, _, e2) = run(&[
        "add-omiyage", &slug, "re_item",
        "--buy-at", &poi,
        "--location-source-url", "https://example.com/re-loc-v2",
        "--location-confidence", "reviewed",
    ]);
    assert!(ok2, "re-add; err={e2}");

    let nloc = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .expect("count");
    assert_eq!(nloc.scalar().as_deref(), Some("1"));

    let fa2 = db_exec(&format!(
        "SELECT fetched_at FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .and_then(|r| r.scalar())
    .expect("fetched_at2");
    assert_ne!(fa1, fa2, "fetched_at must refresh on same-seller re-add");

    let conf = db_exec(&format!(
        "SELECT confidence FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .and_then(|r| r.scalar());
    assert_eq!(conf.as_deref(), Some("reviewed"));
}

/// Atomic / zero-partial: every fail path leaves 0 rows on both tables.
/// (Spec L148: failure create → 0 item + 0 location. True mid-tx ROLLBACK is
/// implemented in write_item_and_location; CLI-level proof is zero residual rows
/// on all validation failures below + the matrix.)
#[test]
fn add_omiyage_fail_loud_matrix() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_fail_{n}");
    let poi = format!("omi_poi_fail_{n}");
    let other_slug = format!("omi_other_{n}");
    let _g = Guard::new({
        let (s1, s2) = (slug.clone(), other_slug.clone());
        move || {
            omi_teardown(&s1);
            omi_teardown(&s2);
        }
    });
    omi_teardown(&slug);
    omi_teardown(&other_slug);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }
    // wrong-slug POI: POI exists under other_slug only
    if !seed_dest_and_poi(&other_slug, "poi_elsewhere") {
        return;
    }

    let assert_zero = |label: &str| {
        let it = db_exec(&format!(
            "SELECT COUNT(*) FROM destination_omiyage_items WHERE slug='{slug}'"
        ))
        .expect("items");
        let lo = db_exec(&format!(
            "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}'"
        ))
        .expect("locs");
        assert_eq!(it.scalar().as_deref(), Some("0"), "{label}: items");
        assert_eq!(lo.scalar().as_deref(), Some("0"), "{label}: locations");
    };

    // unknown dest
    {
        let (ok, _, e) = run(&[
            "add-omiyage", "no_such_dest_xyz", "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "unknown dest; stderr={e}");
    }

    // unknown POI (same slug, poi missing)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", "no_such_poi",
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "unknown poi; stderr={e}");
        assert_zero("unknown poi");
    }

    // wrong-slug POI (poi exists only on other_slug)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", "poi_elsewhere",
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "wrong-slug poi; stderr={e}");
        assert_zero("wrong-slug poi");
    }

    // blank name on new item
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "   ", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "blank name; stderr={e}");
        assert_zero("blank name");
    }

    // blank category on new item
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "blank category; stderr={e}");
        assert_zero("blank category");
    }

    // missing --location-source-url
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "missing loc url; stderr={e}");
        assert_zero("missing loc url");
    }

    // bad --location-confidence 'guess'
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "guess",
        ]);
        assert!(!ok, "bad confidence; stderr={e}");
        assert_zero("bad confidence");
    }

    // non-http url (must start with http:// or https://)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "ftp://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "non-http url; stderr={e}");
        assert_zero("non-http url");
    }

    // missing flag value
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", // no value
        ]);
        assert!(!ok, "missing flag value; stderr={e}");
        assert_zero("missing flag value");
    }

    // excess positional
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1", "extra_pos",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "excess positional; stderr={e}");
        assert_zero("excess positional");
    }

    // unknown flag
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
            "--not-a-real-flag", "x",
        ]);
        assert!(!ok, "unknown flag; stderr={e}");
        assert_zero("unknown flag");
    }

    // --plan-id (friendly reject; explicit parse arm — not generic reject_unknown_flags alone)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
            "--plan-id", "some-plan",
        ]);
        assert!(!ok, "--plan-id; stderr={e}");
        assert!(
            e.contains("plan-id") || e.contains("--plan-id") || e.contains("no --plan-id"),
            "friendly --plan-id message; stderr={e}"
        );
        assert_zero("--plan-id");
    }

    // new-item missing bundle (no --name/--category/--item-source-url/--item-confidence)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--buy-at", &poi,
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "new-item missing bundle; stderr={e}");
        assert_zero("new-item missing bundle");
    }
}

#[test]
fn add_omiyage_no_audit_even_with_travel_plan_id() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let plan = format!("zz-omi-plan-{n}");
    let slug = format!("omi_audit_{n}");
    let poi = format!("omi_poi_audit_{n}");

    // Combined Guard: plan-keyed + omiyage/ref rows
    let _g = Guard::new({
        let (plan, slug) = (plan.clone(), slug.clone());
        move || {
            omi_teardown(&slug);
            teardown_plan(&plan, &slug);
        }
    });
    omi_teardown(&slug);
    teardown_plan(&plan, &slug);
    seed_plan(&plan, &slug, 7);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }

    let ver_before = db_exec(&format!(
        "SELECT version FROM plans WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .expect("version before");
    let runs_before = db_exec(&format!(
        "SELECT COUNT(*) FROM operation_runs WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());
    let events_before = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_events WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());

    let out = Command::new(bin())
        .args([
            "add-omiyage",
            &slug,
            "audit_item",
            "--name",
            "Audit Item",
            "--category",
            "名產",
            "--buy-at",
            &poi,
            "--item-source-url",
            "https://example.com/a",
            "--item-confidence",
            "verified",
            "--location-source-url",
            "https://example.com/al",
            "--location-confidence",
            "verified",
        ])
        .env("TRAVEL_PLAN_ID", &plan)
        .output()
        .expect("run with TRAVEL_PLAN_ID");
    assert!(
        out.status.success(),
        "add should succeed with TRAVEL_PLAN_ID set; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ver_after = db_exec(&format!(
        "SELECT version FROM plans WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .expect("version after");
    assert_eq!(ver_before, ver_after, "plans.version must not bump");

    let runs_after = db_exec(&format!(
        "SELECT COUNT(*) FROM operation_runs WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());
    assert_eq!(runs_before, runs_after, "no operation_runs row");

    let events_after = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_events WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());
    assert_eq!(events_before, events_after, "no plan_events row");
}

// ── Task 4: query-omiyage ──────────────────────────────────────────────────

#[test]
fn query_omiyage_round_trip_grouped_with_dual_provenance() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_q_{n}");
    let poi = format!("omi_poi_q_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }

    // two items, different categories (order: 名產 before 和菓子 if we use ASCII... use explicit categories)
    let (ok_a, _, e_a) = run(&[
        "add-omiyage", &slug, "item_b",
        "--name", "Banana Cake", "--category", "和菓子",
        "--buy-at", &poi,
        "--item-source-url", "https://example.com/banana",
        "--item-confidence", "verified",
        "--location-source-url", "https://example.com/floor-b",
        "--location-confidence", "verified",
        "--notes", "classic",
    ]);
    assert!(ok_a, "{e_a}");
    let (ok_b, _, e_b) = run(&[
        "add-omiyage", &slug, "item_a",
        "--name", "Senbei Pack", "--category", "名產",
        "--buy-at", &poi,
        "--item-source-url", "https://example.com/senbei",
        "--item-confidence", "reviewed",
        "--location-source-url", "https://example.com/floor-a",
        "--location-confidence", "reviewed",
    ]);
    assert!(ok_b, "{e_b}");

    let (ok, stdout, e) = run(&["query-omiyage", "--slug", &slug]);
    assert!(ok, "query; stderr={e}");
    // item_id printed
    assert!(stdout.contains("item_a") && stdout.contains("item_b"), "item_ids: {stdout}");
    // both category headers / labels present
    assert!(stdout.contains("名產") && stdout.contains("和菓子"), "categories: {stdout}");
    // POI title/area/station from join
    assert!(stdout.contains("Test Depachika"), "poi title: {stdout}");
    assert!(stdout.contains("namba") || stdout.contains("Namba"), "area/station: {stdout}");
    // dual provenance: item URLs once + location URLs
    assert!(stdout.contains("https://example.com/banana"), "item provenance: {stdout}");
    assert!(stdout.contains("https://example.com/floor-b"), "loc provenance: {stdout}");
    // nullable address → '—' (seed left address NULL)
    assert!(stdout.contains('—') || stdout.contains("—"), "nullable dash: {stdout}");
    // deterministic order from repo ORDER BY i.category, i.name, i.item_id, ...
    // Lock relative order of the two category labels in stdout (whichever UTF-8 sort puts first).
    let i_meisan = stdout.find("名產").expect("名產");
    let i_wagashi = stdout.find("和菓子").expect("和菓子");
    let cat_order_ok = if "名產" < "和菓子" {
        i_meisan < i_wagashi
    } else {
        i_wagashi < i_meisan
    };
    assert!(cat_order_ok, "category order must follow ORDER BY i.category; stdout={stdout}");
}

#[test]
fn query_omiyage_unknown_vs_empty() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_empty_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    // known dest, zero omiyage rows
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{slug}', 'Empty Omi', 'Asia/Tokyo', 'JPY', 'taiwan');"
    ))
    .is_none()
    {
        return;
    }

    let (ok_empty, out_e, err_e) = run(&["query-omiyage", "--slug", &slug]);
    assert!(!ok_empty, "empty dest must fail");
    let msg_empty = format!("{out_e}{err_e}");
    assert!(
        msg_empty.contains("no sourced") || msg_empty.contains("no omiyage") || msg_empty.contains("empty"),
        "empty message: {msg_empty}"
    );

    let (ok_unk, out_u, err_u) = run(&["query-omiyage", "--slug", "no_such_dest_omiyage_zzz"]);
    assert!(!ok_unk, "unknown dest must fail");
    let msg_unk = format!("{out_u}{err_u}");
    assert!(
        msg_unk.contains("unknown") || msg_unk.contains("not found") || msg_unk.contains("destination_config"),
        "unknown message: {msg_unk}"
    );
    // different messages
    assert_ne!(
        msg_empty.trim(),
        msg_unk.trim(),
        "unknown vs empty must differ"
    );
}

#[test]
fn query_omiyage_corrupt_orphan_fails() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_orph_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{slug}', 'Orph', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','orphan_item','O','名產',NULL,'https://example.com/i','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','orphan_item','missing_poi',NULL,'https://example.com/l','2026-07-12T00:00:00Z','verified');"
    ))
    .is_none()
    {
        return;
    }

    let (ok, _, e) = run(&["query-omiyage", "--slug", &slug]);
    assert!(!ok, "orphan POI must fail query, not silently omit; stderr={e}");
}

#[test]
fn query_omiyage_rejects_plan_id_help_and_typo() {
    // --help: no Turso needed if parse is pre-connect; still fine if connect later
    let (ok_h, out_h, err_h) = run(&["query-omiyage", "--help"]);
    assert!(ok_h, "help should succeed");
    let help = format!("{out_h}{err_h}");
    assert!(
        help.contains("Usage") || help.contains("usage") || help.contains("query-omiyage"),
        "help text: {help}"
    );

    let (ok_p, _, e_p) = run(&["query-omiyage", "--slug", "x", "--plan-id", "p"]);
    assert!(!ok_p, "--plan-id rejected; stderr={e_p}");
    assert!(
        e_p.contains("plan-id") || e_p.contains("--plan-id"),
        "friendly plan-id: {e_p}"
    );

    let (ok_t, _, e_t) = run(&["query-omiyage", "--slug", "x", "--slugg", "y"]);
    assert!(!ok_t, "typo flag rejected; stderr={e_t}");
}

#[test]
fn validate_data_catches_omiyage_orphans_and_missing_provenance() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_val_{n}");
    let poi = format!("omi_poi_val_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);

    // --- empty stays valid: no omiyage rows for this slug (and we don't require global non-empty) ---
    // Run validate and only assert our slug doesn't appear in omiyage errors; global exit may still
    // fail on unrelated live data — so assert on stdout/stderr *lines about omiyage* for our slug.

    // Seed a multi-corrupt state via db_exec:
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{slug}', 'Val', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence) \
           VALUES ('{slug}', '{poi}', 'P', 'https://example.com/p', '2026-07-12T00:00:00Z', 'verified'); \
         /* (a) location with missing parent item */ \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','ghost_item','{poi}',NULL,'https://example.com/l','2026-07-12T00:00:00Z','verified'); \
         /* (b) location with missing POI */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','has_bad_poi','N','名產',NULL,'https://example.com/i','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','has_bad_poi','missing_poi',NULL,'https://example.com/l2','2026-07-12T00:00:00Z','verified'); \
         /* (c) item with no location */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','lonely','L','名產',NULL,'https://example.com/i2','2026-07-12T00:00:00Z','verified'); \
         /* (d) blank source_url on an item */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','blank_url','B','名產',NULL,'','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','blank_url','{poi}',NULL,'https://example.com/l3','2026-07-12T00:00:00Z','verified'); \
         /* (e) confidence='guess' */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','bad_conf','C','名產',NULL,'https://example.com/i3','2026-07-12T00:00:00Z','guess'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','bad_conf','{poi}',NULL,'https://example.com/l4','2026-07-12T00:00:00Z','verified');"
    ))
    .is_none()
    {
        return;
    }

    // (f) item.slug NOT IN destination_config — arm Guard FIRST, then seed phantom slug
    let phantom = format!("omi_phantom_{n}");
    let _g2 = Guard::new({
        let p = phantom.clone();
        move || omi_teardown(&p)
    });
    if db_exec(&format!(
        "INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{phantom}','ph','P','名產',NULL,'https://example.com/ph','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{phantom}','ph','no_poi',NULL,'https://example.com/phl','2026-07-12T00:00:00Z','verified');"
    ))
    .is_none()
    {
        return;
    }

    let (ok, stdout, stderr) = run(&["validate", "data"]);
    let all = format!("{stdout}{stderr}");
    // Do not assert global exit code alone (live DB may have other issues). Assert each class is reported:
    assert!(
        all.contains("ghost_item") || all.contains("orphan") || all.contains("parent"),
        "orphan location/item: {all}"
    );
    assert!(
        all.contains("missing_poi") || all.contains("has_bad_poi") || all.contains("poi"),
        "orphan seller POI: {all}"
    );
    assert!(
        all.contains("lonely") || all.contains("no location") || all.contains("without location"),
        "item without location: {all}"
    );
    assert!(
        all.contains("blank_url") || all.contains("source_url") || all.contains("provenance"),
        "blank provenance: {all}"
    );
    assert!(
        all.contains("bad_conf") || all.contains("guess") || all.contains("confidence"),
        "bad confidence: {all}"
    );
    assert!(
        all.contains(&phantom) || all.contains("destination_config") || all.contains("slug"),
        "item.slug not in destination_config: {all}"
    );
    let _ = ok; // global may be non-zero; class coverage is the lock
}

#[test]
fn validate_data_empty_omiyage_stays_valid_for_slug() {
    // Empty omiyage tables must NOT produce an empty-table Error (spec L81).
    // Do NOT add omiyage to validate.rs:1005 checklist.
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let (_, stdout, stderr) = run(&["validate", "data"]);
    let all = format!("{stdout}{stderr}");
    for line in all.lines() {
        let lower = line.to_lowercase();
        if lower.contains("omiyage")
            && (lower.contains("is empty")
                || lower.contains("no rows")
                || lower.contains("no live seeder"))
        {
            panic!("empty omiyage must not be an integrity error: {line}");
        }
    }
}

// ── Task 1: omiyage-worklist (read-only discovery) ─────────────────────────

/// Seed config + poi + an omiyage tag with a notes hint. Returns false if credless.
/// Columns match scripts/schema.sql (notes exists; no invented `source` column —
/// same contract as seed_dest_and_poi).
fn seed_omiyage_tagged_poi(slug: &str, poi: &str, notes: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    let s = slug.replace('\'', "''");
    let p = poi.replace('\'', "''");
    let nt = notes.replace('\'', "''");
    db_exec(&format!(
        "INSERT INTO destination_config (slug,display_name,timezone,currency,origin) \
           VALUES ('{s}','Omi','Asia/Tokyo','JPY','taiwan');\
         INSERT INTO destination_pois \
           (slug,poi_id,title,area,nearest_station,notes,source_url,fetched_at,confidence) \
           VALUES ('{s}','{p}','Test Depachika','namba','Namba','{nt}',\
                   'https://example.com/poi','2026-07-12T00:00:00Z','verified');\
         INSERT INTO destination_poi_tags (slug,poi_id,tag,sort_order) \
           VALUES ('{s}','{p}','omiyage',0);"
    ))
    .is_some()
}

#[test]
fn worklist_prints_tagged_poi_notes_and_template_writes_nothing() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_wl_{n}");
    let poi = format!("omi_wlp_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_omiyage_tagged_poi(&slug, &poi, "Premium omiyage: Yoku Moku, Tokyo Banana") {
        return;
    }

    let (ok, out, e) = run(&["omiyage-worklist", "--slug", &slug]);
    assert!(ok, "worklist should succeed; e={e}");
    assert!(out.contains(&poi), "prints poi_id: {out}");
    assert!(out.contains("Yoku Moku"), "prints notes verbatim: {out}");
    assert!(
        out.contains("WARNING") || out.contains("hint"),
        "warns notes are hints: {out}"
    );
    assert!(
        out.contains("add-omiyage") && out.contains(&slug),
        "prints add-omiyage template with slug: {out}"
    );
    assert!(
        out.contains("already sourced") || out.contains("0"),
        "shows already-sourced count: {out}"
    );

    // WRITES NOTHING: no omiyage rows created for this slug.
    let items = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_items WHERE slug='{slug}'"
    ))
    .unwrap();
    assert_eq!(
        items.scalar().as_deref(),
        Some("0"),
        "worklist must write NO items"
    );
    let locs = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}'"
    ))
    .unwrap();
    assert_eq!(
        locs.scalar().as_deref(),
        Some("0"),
        "worklist must write NO locations"
    );
}

#[test]
fn worklist_already_sourced_count_reflects_existing() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_wls_{n}");
    let poi = format!("omi_wlsp_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_omiyage_tagged_poi(&slug, &poi, "Tokyo Banana") {
        return;
    }
    // add one real omiyage row at this POI via the real command
    let (ok_a, _, ea) = run(&[
        "add-omiyage",
        &slug,
        "tokyo_banana",
        "--name",
        "Tokyo Banana",
        "--category",
        "和菓子",
        "--buy-at",
        &poi,
        "--item-source-url",
        "https://example.com/i",
        "--item-confidence",
        "verified",
        "--location-source-url",
        "https://example.com/l",
        "--location-confidence",
        "verified",
    ]);
    assert!(ok_a, "{ea}");
    let (ok, out, e) = run(&["omiyage-worklist", "--slug", &slug]);
    assert!(ok, "e={e}");
    // already-sourced count for this POI should be 1
    assert!(
        out.contains("already sourced here: 1") || out.contains(": 1"),
        "count reflects the 1 existing: {out}"
    );
}

#[test]
fn worklist_no_tagged_poi_fails_loud() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_wln_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    // config exists but NO omiyage-tagged POI
    if db_exec(&format!(
        "INSERT INTO destination_config (slug,display_name,timezone,currency,origin) \
           VALUES ('{slug}','Omi','Asia/Tokyo','JPY','taiwan')"
    ))
    .is_none()
    {
        return;
    }
    let (ok, _o, e) = run(&["omiyage-worklist", "--slug", &slug]);
    assert!(!ok, "no omiyage-tagged POI must fail loud");
    assert!(
        e.contains("no omiyage") || e.contains("no omiyage-tagged") || e.contains("tag"),
        "message: {e}"
    );
}

#[test]
fn worklist_unknown_dest_and_plan_id_and_help() {
    let (ok_u, _o, e_u) = run(&["omiyage-worklist", "--slug", "no_such_dest_wl_zzz"]);
    assert!(!ok_u, "unknown dest fails");
    assert!(
        e_u.contains("unknown") || e_u.contains("destination_config"),
        "unknown msg: {e_u}"
    );
    let (ok_p, _o, e_p) = run(&["omiyage-worklist", "--slug", "x", "--plan-id", "p"]);
    assert!(!ok_p, "--plan-id rejected");
    assert!(e_p.contains("plan-id"), "friendly: {e_p}");
    let (ok_h, out_h, err_h) = run(&["omiyage-worklist", "--help"]);
    assert!(
        ok_h && (format!("{out_h}{err_h}").contains("Usage")
            || format!("{out_h}{err_h}").contains("omiyage-worklist")),
        "help"
    );
    let (ok_t, _o, _e) = run(&["omiyage-worklist", "--slug", "x", "--slugg", "y"]);
    assert!(!ok_t, "typo flag rejected");
}
