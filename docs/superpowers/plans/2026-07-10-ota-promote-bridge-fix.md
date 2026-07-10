# OTA scrape→write→promote Bridge Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a real scraped offer flow all the way into a plan — `ota write-offers` must stamp the offer's `destination` (so `promote-offers --dest` finds it), and `promote-offers` must write a flight leg even when only the outbound flight is known (google_flights gives outbound + round-trip total, no paired return).

**Architecture:** Two independent blockers on the same bridge, two commits. Task 1 (#C) adds a required `--dest <slug>` to `ota write-offers`: validate the slug against `destination_config`, read the job's `region_label`/`region_code` from `ota_job_params`, and thread both into `parsed_to_offer_row` so `offers.destination`/`offers.region` land non-NULL. Task 2 (#1) changes `PlanOfferWrite.flights` from `Option<[…; 2]>` to `Vec<PlanOfferFlightWrite>` so an outbound-only offer writes one leg instead of zero; downstream (`select_offer.has_flight()` = non-empty, `flight_legs::replace_from_offer` iterates any slice) already accepts it.

**Tech Stack:** Rust (travel-cli command layer + travel-db repo layer), libsql/Turso, real-Turso integration tests on `tests/common/mod.rs`.

## Global Constraints

- **Agent-first plain text** — stdout is plain text / TSV lines, never JSON.
- **Fail loud, no fallback** — a bad/absent slug THROWS; never silently default a destination.
- **Audit triad stays in `cascade::common`** — domain writes go through `travel-db` repos; `plan_events`/`operation_runs`/`plans.version` are written only by `record_operation`. No repo writes the audit triad. (Neither task touches the audit triad — both are pure domain-write/arg changes.)
- **No region→slug automap** — there is no `region → destination_slug` mapping in the schema (`destination_config` has only `slug`/`ref_id`); do not invent one. The slug is passed explicitly via `--dest`.
- **No time-parsing in Task 2** — `PlanOfferFlightWrite` stays `{ direction, flight_number }`. The scraped times are lost upstream (in `offers`/TSV), not here; writing them is out of scope.
- **Behavior-lock tests** — real-Turso, on the `tests/common/mod.rs` harness (`bin`, `db_exec(sql) -> Option<Rows>`, `seed_plan(plan, dest, version)`, `teardown_plan`, `Guard`, `nanos`, `is_credless(&stderr)`). Arm the RAII `Guard` right after the plan-id is bound; never leave a trailing teardown. Non-plan-keyed rows a test seeds (global `offers`, `captures`, `ota_jobs`/`ota_job_params`, `destination_config`) MUST be torn down locally (before `teardown_plan`).
- **Run integration tests serialized in the BACKGROUND** — `cargo test ... -- --test-threads=1`; a foreground timeout SIGTERMs the test mid-run and the `Guard` `Drop` never fires (leaks a prod row).
- **`./bin/travel` is the RELEASE binary** — after a code change, `cargo build --release -p travel-cli && cp target/release/travel bin/travel` (or `make build`) before any CLI smoke, or you test a stale binary.
- **Pipeline** — Codex designed/planned; Grok implements task-by-task against these tests; Claude reviews every line + corroborates vs source + runs the serialized verify. Commit explicit pathspecs only.

---

## File Structure

- `rust/crates/travel-cli/src/ota/write_offers.rs` — add `--dest` parse + slug validation + region resolution from job params; pass both into the row mapper. (Task 1)
- `rust/crates/travel-cli/src/ota/common.rs` — `parsed_to_offer_row` gains `destination: &str` + `region: Option<&str>` params; replaces the hardcoded `region: None, destination: None`. (Task 1)
- `rust/crates/travel-cli/src/shaping.rs`, `src/search_compare.rs` — usage-string updates that echo the `ota write-offers` invocation. (Task 1, mechanical)
- `CLAUDE.md` — usage line for `ota write-offers` gains `--dest <slug>`. (Task 1, mechanical)
- `rust/crates/travel-cli/tests/ota_write_offers.rs` — extend with the #C behavior-lock. (Task 1)
- `rust/crates/travel-db/src/repo/plan_offers.rs` — `PlanOfferWrite.flights: Vec<PlanOfferFlightWrite>`; insert loop iterates the vec. (Task 2)
- `rust/crates/travel-cli/src/promote_offers.rs` — build `flights` as a vec: both→2, outbound-only→1, else empty; update the stale comment. (Task 2)
- `rust/crates/travel-cli/tests/promote_offers.rs` — extend with the #1 behavior-lock. (Task 2)

---

## Task 1 (commit 1) — #C: `ota write-offers --dest <slug>` stamps `offers.destination`/`region`

**Files:**
- Modify: `rust/crates/travel-cli/src/ota/write_offers.rs:188-221` (arg parse), `:265-291` (call site)
- Modify: `rust/crates/travel-cli/src/ota/common.rs:375-423` (`parsed_to_offer_row` signature + body)
- Modify: `rust/crates/travel-cli/src/shaping.rs:312`, `rust/crates/travel-cli/src/search_compare.rs:278`, `CLAUDE.md` (usage strings — mechanical)
- Test: `rust/crates/travel-cli/tests/ota_write_offers.rs`

**Interfaces:**
- Consumes: `ota_jobs::get_params(conn, job_id) -> Result<Vec<(String, String)>, String>` (`ota_jobs.rs:105`) — returns `(param_key, param_value)` pairs incl. `region_label`/`region_code` persisted by `ota enqueue` (`enqueue.rs:56,61`).
- Produces: `parsed_to_offer_row(source_id, product_type, destination: &str, region: Option<&str>, url, capture_id, scraped_at, departure_date, return_date, nights, price_per_person, currency, hotel_name, airline, flight_outbound, flight_return, job_id, attempt_id, parser_method, capture_checksum, parser_rule_checksum, normalizer_version) -> OfferRow` — the two new params (`destination`, `region`) are inserted right after `product_type` (keeping the existing arg order otherwise). All in-repo callers updated in this task.

**Why the params go right after `product_type`:** they are the offer's identity/classification fields, adjacent to `source_id`/`product_type`; placing them there keeps the call site readable and the diff localized. The only caller is `write_offers.rs:268`.

- [ ] **Step 1: Write the failing test**

Add to `rust/crates/travel-cli/tests/ota_write_offers.rs` (follow the existing seed pattern in that file — `db migrate` first, since `ota_job_params` CHECK is widened at runtime; seed `destination_config` + `ota_sources` + `captures` + a claimed `ota_jobs` with `ota_job_params(region_label, region_code)`). Use a unique nanos-suffixed plan/dest/job/source/capture so parallel-safe. Arm the `Guard` right after the ids are bound. Teardown deletes the seeded `ota_job_params`/global `offers`/capture/job/source/`destination_config` rows, then `teardown_plan`.

```rust
#[tokio::test]
async fn write_offers_dest_stamps_destination_and_region_and_promotes() {
    // --- seed (nanos-unique) ---
    let n = common::nanos();
    let dest = format!("wo_dest_{n}");            // destination_config.slug
    let plan = format!("wo-plan-{n}");
    let source = format!("wo_src_{n}");
    let cap = format!("wo_cap_{n}");
    let job = format!("wo_job_{n}");
    let tok = format!("wo_tok_{n}");

    let Some(_) = common::db_exec("SELECT 1") else { eprintln!("credless — skip"); return; };

    // widened ota_job_params CHECK is a runtime migration
    let _ = common::bin(&["db", "migrate"]);

    let _g = common::Guard::new({
        let (plan, dest, source, cap, job) =
            (plan.clone(), dest.clone(), source.clone(), cap.clone(), job.clone());
        move || {
            common::db_exec_teardown(&format!("DELETE FROM ota_job_params WHERE job_id='{job}'"));
            common::db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job}'"));
            common::db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job}'"));
            common::db_exec_teardown(&format!("DELETE FROM captures WHERE capture_id='{cap}'"));
            common::db_exec_teardown(&format!("DELETE FROM offers WHERE source_id='{source}'"));
            common::db_exec_teardown(&format!("DELETE FROM ota_sources WHERE source_id='{source}'"));
            common::db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug='{dest}'"));
            common::teardown_plan(&plan, &dest);
        }
    });

    common::seed_plan(&plan, &dest, 1);
    common::db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, origin) \
         VALUES ('{dest}','WO Dest','TPE')"
    ));
    common::db_exec(&format!(
        "INSERT INTO ota_sources (source_id, display_name, enabled) \
         VALUES ('{source}','WO Src',1)"
    ));
    common::db_exec(&format!(
        "INSERT INTO captures (capture_id, source_id, url, raw_text, captured_at) \
         VALUES ('{cap}','{source}','https://x/y?prod=1','type\tprice_per_person\n', '2026-07-10 00:00:00')"
    ));
    // claimed job with region params
    common::db_exec(&format!(
        "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, \
             claimed_by, max_attempts, attempts) \
         VALUES ('{job}','{source}','flight','claimed','{tok}','tester',3,0)"
    ));
    common::db_exec(&format!(
        "INSERT INTO ota_job_params (job_id, param_key, param_value) \
         VALUES ('{job}','region_label','Kansai'),('{job}','region_code','KIX')"
    ));

    // TSV: one outbound-only flight offer
    let tsv = std::env::temp_dir().join(format!("wo_{n}.tsv"));
    std::fs::write(&tsv,
        "type\tprice_per_person\tdeparture_date\treturn_date\tnights\tairline\tflight_outbound\tflight_return\thotel_name\tcurrency\n\
         flight\t10386\t2026-08-05\t2026-08-09\t4\tJetstar\tGK25\t\t\tTWD\n").unwrap();

    // --- act: write-offers WITH --dest ---
    let out = common::bin(&[
        "ota","write-offers",&job,
        "--capture",&cap,"--claim-token",&tok,
        "--tsv",tsv.to_str().unwrap(),"--dest",&dest,
    ]);
    assert!(out.status.success(), "write-offers failed: {}", String::from_utf8_lossy(&out.stderr));

    // --- assert: offer landed with destination + region ---
    let rows = common::db_exec(&format!(
        "SELECT destination, region FROM offers WHERE source_id='{source}'"
    )).expect("rows");
    assert_eq!(rows.len(), 1, "expected exactly 1 offer");
    assert_eq!(rows[0][0], dest, "offers.destination must be the --dest slug");
    assert_eq!(rows[0][1], "Kansai", "offers.region must be region_label from job params");

    // --- assert: promote-offers --dest now finds it ---
    let prom = common::bin(&[
        "promote-offers","--from-offers","--dest",&dest,"--plan-id",&plan,
    ]);
    assert!(prom.status.success(), "promote failed: {}", String::from_utf8_lossy(&prom.stderr));
    let po = common::db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offers WHERE plan_id='{plan}' AND destination='{dest}'"
    )).expect("rows");
    assert_eq!(po[0][0], "1", "promoted offer must land in plan_offers");
}
```

> Adjust column names in the seed INSERTs to match the real schema if any differ — check with `./bin/travel db schema ota_jobs` / `db schema captures` / `db schema offers` before running. The assertions (destination = slug, region = region_label, 1 promoted row) are the lock and must not change.

- [ ] **Step 2: Run test to verify it fails**

Run (background, serialized):
```bash
cd rust && cargo test -p travel-cli --test ota_write_offers write_offers_dest_stamps -- --test-threads=1 --nocapture
```
Expected: FAIL — either a compile error (`--dest` unknown flag → `write-offers` treats it as an extra positional, or `parsed_to_offer_row` arity), or the assertion `offers.destination must be the --dest slug` (currently `parsed_to_offer_row` hardcodes `destination: None`).

- [ ] **Step 3: Change `parsed_to_offer_row` to take `destination` + `region`**

In `rust/crates/travel-cli/src/ota/common.rs`, change the signature (insert two params after `product_type`) and the body:

```rust
pub fn parsed_to_offer_row(
    source_id: &str,
    product_type: &str,
    destination: &str,
    region: Option<&str>,
    url: &str,
    capture_id: &str,
    scraped_at: &str,
    departure_date: Option<&str>,
    return_date: Option<&str>,
    nights: Option<i64>,
    price_per_person: i64,
    currency: &str,
    hotel_name: Option<&str>,
    airline: Option<&str>,
    flight_outbound: Option<&str>,
    flight_return: Option<&str>,
    job_id: &str,
    attempt_id: &str,
    parser_method: &str,
    capture_checksum: &str,
    parser_rule_checksum: Option<&str>,
    normalizer_version: &str,
) -> OfferRow {
    let product_code = product_code_from_url(url).unwrap_or_default();
    OfferRow {
        id: offer_row_id(source_id, &product_code, departure_date, nights),
        source_file: Some(format!("capture:{capture_id}")),
        source_id: source_id.to_string(),
        offer_type: offer_row_kind(product_type).to_string(),
        price_per_person: Some(price_per_person),
        currency: Some(currency.to_string()),
        region: region.map(|r| r.to_string()),
        destination: Some(destination.to_string()),
        departure_date: ne(departure_date),
        // ... rest unchanged ...
```

(Everything from `departure_date:` onward is unchanged — only the two lines `region:` / `destination:` change, plus the two new signature params.)

- [ ] **Step 4: Add `--dest` parse + slug validation + region resolution in `write_offers.rs`**

In `rust/crates/travel-cli/src/ota/write_offers.rs`:

a. Add `--dest` to the positional-exclusion list at `:189` so the slug value isn't mistaken for the `<job_id>` positional:
```rust
let positional = common::positionals(args, &["--capture", "--claim-token", "--tsv", "--dest"]);
```

b. Add a `dest` accumulator + arm in the arg loop (alongside `--tsv` at `:212`):
```rust
    let mut dest: Option<String> = None;
    // ... in the while loop:
            "--dest" => {
                dest = Some(args.get(i + 1).ok_or("missing --dest")?.clone());
                i += 2;
            }
```

c. After the three existing `.ok_or(...)?` requires (`:219-221`), require `--dest`:
```rust
    let dest = dest.ok_or("Error: --dest <slug> is required")?;
```

d. After `db::connect_write()` + `ota_jobs::get` (`:223-226`), validate the slug (fail loud) and read region from job params:
```rust
    // Validate --dest against destination_config (fail loud on a bad slug).
    {
        let mut r = conn
            .query(
                "SELECT 1 FROM destination_config WHERE slug = ?1",
                libsql::params![dest.clone()],
            )
            .await
            .map_err(|e| e.to_string())?;
        if r.next().await.map_err(|e| e.to_string())?.is_none() {
            return Err(format!("Error: --dest '{dest}' is not a registered destination"));
        }
    }
    // Region for the offer row: region_label if present, else region_code, else NULL.
    let params = ota_jobs::get_params(&conn, job_id).await?;
    let region = params
        .iter()
        .find(|(k, _)| k == "region_label")
        .or_else(|| params.iter().find(|(k, _)| k == "region_code"))
        .map(|(_, v)| v.clone());
```
(Add `libsql` to the `use` list if not present, and `ota_jobs` is already imported at `:8`.)

e. Thread `&dest` + `region.as_deref()` into the `parsed_to_offer_row` call (`:268`, insert after `&source_id, &p.product_type,`):
```rust
            common::parsed_to_offer_row(
                &source_id,
                &p.product_type,
                &dest,
                region.as_deref(),
                url,
                // ... rest unchanged ...
```

- [ ] **Step 5: Update usage strings (mechanical)**

- `write_offers.rs:192` usage: append ` --dest <slug>`.
- `shaping.rs:312` and `search_compare.rs:278`: wherever the `ota write-offers …` example is echoed, append ` --dest <slug>`.
- `CLAUDE.md`: the `ota write-offers` line(s) in URL Routing + CLI Quick Reference gain ` --dest <slug>`.

Grep to find them all:
```bash
grep -rn "ota write-offers" rust/crates/travel-cli/src CLAUDE.md
```

- [ ] **Step 6: Build (release) + run test to verify it passes**

```bash
cd rust && cargo build --release -p travel-cli && cp target/release/travel ../bin/travel
cd rust && cargo test -p travel-cli --test ota_write_offers -- --test-threads=1 --nocapture
```
Expected: PASS — the new test + all existing `ota_write_offers` regressions green (existing tests that call `write-offers` without `--dest` must now supply it OR expect the new "required" error — update those call sites/expectations if the existing suite has them; the flight↔hotel compatibility test at `:399-444` still passes because `--dest` is orthogonal to the type guard).

> If existing tests in this file call `write-offers` without `--dest`, they will now fail with "Error: --dest <slug> is required". Update each to pass a valid seeded `--dest` (they already seed a destination or can seed one). This is expected — the flag is required by design (fail-loud).

- [ ] **Step 7: Commit**

```bash
git add rust/crates/travel-cli/src/ota/write_offers.rs \
        rust/crates/travel-cli/src/ota/common.rs \
        rust/crates/travel-cli/src/shaping.rs \
        rust/crates/travel-cli/src/search_compare.rs \
        rust/crates/travel-cli/tests/ota_write_offers.rs \
        CLAUDE.md
git commit -F - <<'EOF'
fix(ota): write-offers --dest stamps offers.destination/region (#C)

Real scraped offers landed with destination=NULL, so promote-offers --dest
(WHERE destination=?1) found zero rows. write-offers now takes a required
--dest <slug>, validates it against destination_config (fail loud), and
stores region from the job's enqueue params (region_label > region_code).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 2 (commit 2) — #1: outbound-only offer writes one flight leg → P3 populated

**Files:**
- Modify: `rust/crates/travel-db/src/repo/plan_offers.rs:27` (field type), `:147-168` (insert loop)
- Modify: `rust/crates/travel-cli/src/promote_offers.rs:407-419` (build vec), `:376-378` (stale comment)
- Test: `rust/crates/travel-cli/tests/promote_offers.rs`

**Interfaces:**
- Consumes: `PlanOfferFlightWrite { direction: String, flight_number: String }` (unchanged, `plan_offers.rs:40`).
- Produces: `PlanOfferWrite.flights: Vec<PlanOfferFlightWrite>` (was `Option<[PlanOfferFlightWrite; 2]>`). Empty vec = no legs; 1 element = outbound only; 2 = outbound + return. The only constructor is `build_plan_offer_write` (`promote_offers.rs:378`); the only consumer is `plan_offers::insert_offer` (`plan_offers.rs:78,147`). Downstream already accepts any leg count: `select_offer.has_flight()` = `!legs.is_empty()` (`select_offer.rs:94`), `flight_legs::replace_from_offer` iterates any slice.

- [ ] **Step 1: Write the failing test**

Add to `rust/crates/travel-cli/tests/promote_offers.rs` (follow the file's existing seed/teardown pattern; global `offers` row torn down via `teardown_offers` or `db_exec_teardown`, plan rows via `teardown_plan`). Guard armed right after ids bound.

```rust
#[tokio::test]
async fn promote_outbound_only_offer_writes_one_leg_and_populates_p3() {
    let n = common::nanos();
    let dest = format!("po_dest_{n}");
    let plan = format!("po-plan-{n}");
    let offer_id = format!("po_offer_{n}");
    let source = format!("po_src_{n}");

    let Some(_) = common::db_exec("SELECT 1") else { eprintln!("credless — skip"); return; };

    let _g = common::Guard::new({
        let (plan, dest, offer_id) = (plan.clone(), dest.clone(), offer_id.clone());
        move || {
            common::db_exec_teardown(&format!("DELETE FROM offers WHERE id='{offer_id}'"));
            common::teardown_plan(&plan, &dest);
        }
    });

    common::seed_plan(&plan, &dest, 1);
    // one global offer: outbound flight only, no return
    common::db_exec(&format!(
        "INSERT INTO offers (id, source_id, offer_type, destination, price_per_person, currency, \
             departure_date, return_date, flight_outbound, flight_return, airline, scraped_at) \
         VALUES ('{offer_id}','{source}','flight','{dest}',10386,'TWD', \
             '2026-08-05','2026-08-09','GK25',NULL,'Jetstar','2026-07-10 00:00:00')"
    ));

    // promote → expect exactly one outbound leg
    let prom = common::bin(&["promote-offers","--from-offers","--dest",&dest,"--plan-id",&plan]);
    assert!(prom.status.success(), "promote failed: {}", String::from_utf8_lossy(&prom.stderr));
    let legs = common::db_exec(&format!(
        "SELECT direction, flight_number FROM plan_offer_flights \
         WHERE plan_id='{plan}' AND destination='{dest}' AND offer_id='{offer_id}'"
    )).expect("rows");
    assert_eq!(legs.len(), 1, "outbound-only offer must write exactly one leg");
    assert_eq!(legs[0][0], "outbound");
    assert_eq!(legs[0][1], "GK25");

    // select-offer → P3 populated (has_flight true because legs non-empty)
    let sel = common::bin(&["select-offer",&offer_id,"2026-08-05","--plan-id",&plan]);
    assert!(sel.status.success(), "select failed: {}", String::from_utf8_lossy(&sel.stderr));
    let p3 = common::db_exec(&format!(
        "SELECT status FROM process_statuses \
         WHERE plan_id='{plan}' AND process_id='process_3_transportation'"
    )).expect("rows");
    assert_eq!(p3[0][0], "populated", "P3 must be populated from the outbound leg");
}
```

> Verify the seed columns against `./bin/travel db schema offers` and `db schema process_statuses` before running; adjust `select-offer` positional syntax to match its real signature (`select-offer <offer-id> <date>`). The three assertions (1 leg, direction=outbound, P3=populated) are the lock.

- [ ] **Step 2: Run test to verify it fails**

```bash
cd rust && cargo test -p travel-cli --test promote_offers promote_outbound_only -- --test-threads=1 --nocapture
```
Expected: FAIL — `assert_eq!(legs.len(), 1)` gets 0, because `build_plan_offer_write` currently maps `(Some, None) => None` (`promote_offers.rs:418`) so no legs are written, and `has_flight()` is false so P3 prints "no flight — nothing to populate".

- [ ] **Step 3: Change `PlanOfferWrite.flights` to a `Vec` in the repo**

In `rust/crates/travel-db/src/repo/plan_offers.rs:27`:
```rust
    pub flights: Vec<PlanOfferFlightWrite>,
```

And the insert loop at `:147-168`:
```rust
    // plan_offer_flights — one row per leg the caller supplied (0, 1, or 2).
    for leg in &write.flights {
        conn.execute(
            "INSERT INTO plan_offer_flights \
                (plan_id, destination, offer_id, direction, flight_number, airline, \
                 airline_code, departure_code, departure_time, arrival_code, arrival_time, \
                 updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, NULL, NULL, NULL, ?6)",
            libsql::params![
                write.plan_id.clone(),
                write.destination.clone(),
                write.offer_id.clone(),
                leg.direction.clone(),
                leg.flight_number.clone(),
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
```
(The `if let Some(ref legs) = write.flights` wrapper is removed; iterate the vec directly.)

- [ ] **Step 4: Build the vec in `promote_offers.rs`**

Replace `promote_offers.rs:407-419`:
```rust
    let flights = match (&o.flight_outbound, &o.flight_return) {
        (Some(outbound), Some(ret)) => vec![
            PlanOfferFlightWrite {
                direction: "outbound".to_string(),
                flight_number: outbound.clone(),
            },
            PlanOfferFlightWrite {
                direction: "return".to_string(),
                flight_number: ret.clone(),
            },
        ],
        (Some(outbound), None) => vec![PlanOfferFlightWrite {
            direction: "outbound".to_string(),
            flight_number: outbound.clone(),
        }],
        _ => Vec::new(),
    };
```
And update the doc comment at `:376-378` (or the inline comment near the match) so it no longer says "only if BOTH legs present" — e.g. "Build flight legs: both → 2, outbound-only → 1 (google_flights gives outbound + round-trip total, no paired return), neither → none."

- [ ] **Step 5: Build (release) + run test to verify it passes**

```bash
cd rust && cargo build --release -p travel-cli && cp target/release/travel ../bin/travel
cd rust && cargo test -p travel-cli --test promote_offers -- --test-threads=1 --nocapture
```
Expected: PASS — new test + existing `promote_offers` regressions green. (Any existing test that asserted "outbound+return → 2 legs" still passes: the `(Some,Some)` arm is unchanged. Any that implicitly relied on `(Some,None) => 0 legs` must be updated — search the file for such an assertion.)

- [ ] **Step 6: Full-crate build to catch other `flights:` construction sites**

```bash
cd rust && cargo build -p travel-cli -p travel-db
```
Expected: clean. If any other code constructs `PlanOfferWrite { flights: Some([...]) }` or `flights: None`, the compiler flags it — fix to `vec![...]` / `Vec::new()`. (Per the spec, `promote_offers::build_plan_offer_write` is the ONLY constructor and `ImportPlanOfferWrite` is a separate struct — so there should be none, but let the compiler confirm.)

- [ ] **Step 7: Commit**

```bash
git add rust/crates/travel-db/src/repo/plan_offers.rs \
        rust/crates/travel-cli/src/promote_offers.rs \
        rust/crates/travel-cli/tests/promote_offers.rs
git commit -F - <<'EOF'
fix(ota): promote outbound-only flight offer as one leg (#1)

google_flights gives an outbound flight + round-trip total but no paired
return, so real flight offers had flight_return=NULL and promote-offers
wrote zero legs -> P3 never populated. PlanOfferWrite.flights is now a Vec:
(outbound, return) -> 2, (outbound, None) -> 1, neither -> 0. Downstream
(select_offer.has_flight, flight_legs::replace_from_offer) already accepts
any non-empty leg set. No time-parsing (out of scope).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Live smoke (after both commits) — verify on the real drill data

The real drill `osaka-aug-2026` has real offers with `destination=NULL` (the #C symptom). After the fix:

```bash
# 1. Backfill destination on the existing real offers (one-shot; they were written before --dest existed)
./bin/travel db exec "UPDATE offers SET destination='osaka_kyoto_2026' WHERE destination IS NULL AND source_id LIKE '%google_flights%' OR source_id LIKE '%agoda%'"
# (scope the WHERE to exactly the drill's offer ids — inspect first with: db exec \"SELECT id, source_id, destination FROM offers WHERE destination IS NULL\")

# 2. Promote → should now find them
./bin/travel promote-offers --from-offers --dest osaka_kyoto_2026 --plan-id osaka-aug-2026

# 3. select-offer the outbound-only Jetstar flight → P3 populated
./bin/travel select-offer <offer_id> 2026-08-05 --plan-id osaka-aug-2026
./bin/travel status --full --plan-id osaka-aug-2026   # P3 = populated
```

Expected: the outbound-only real flight offer promotes with one leg and drives P3 to populated — the exact chain that was broken. This is a manual confirmation, not a test.

## Acceptance

- #C: `ota write-offers --dest <slug>` writes `offers.destination = slug`, `offers.region = region_label`; a bad slug fails loud; `promote-offers --dest` then finds + promotes the offer. New behavior-lock green; existing `ota_write_offers` regressions green.
- #1: an outbound-only flight offer promotes to exactly one `plan_offer_flights` leg (direction=outbound), and `select-offer` drives P3 to `populated`. New behavior-lock green; existing `promote_offers` regressions green.
- Full-crate `cargo build -p travel-cli -p travel-db` clean.
- Live smoke on `osaka-aug-2026` confirms the real broken chain now completes.
