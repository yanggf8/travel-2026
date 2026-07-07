# Reject-Unknown-Flags Hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or executing-plans. Steps use `- [ ]` checkboxes.

**Goal:** Stop ~10 mutation commands from silently swallowing unknown `--flags` (a typo'd `--dry-run` → real write; a typo'd `--proven` → silently writes proven=0). Apply ONE shared `reject_unknown_flags` helper, each command with its correct value/bool flag classification.

**Architecture:** Hoist the existing proven helper from `set_route_segment.rs:298` into `plan_resolver.rs` as `pub(crate)`, then call it near the top of each command's parse. No behavior change except rejecting unknown flags.

**Pipeline:** Codex-designed, Claude-corroborated (every flag classification + the connect-before-parse caveat verified against source 2026-07-07). Grok/Claude implement, Claude reviews serialized.

## Global Constraints
- Helper signature (verbatim, already proven): `pub(crate) fn reject_unknown_flags(args: &[String], value_flags: &[&str], bool_flags: &[&str]) -> Result<(), String>`. `value_flags` consume the NEXT token (even if it starts with `--`); `bool_flags` are value-less; any other `--token` → `Err("unknown argument: {a}")`.
- **Classification correctness is the #1 risk:** misclassifying a VALUE flag as BOOL makes the helper treat its value token as an unknown flag → falsely rejects a valid invocation. Every list below is source-verified.
- Plain-text, fail-loud, agent-first. Commit per group with `git commit -F` (not -m). Commit only this plan's pathspecs.
- Tests: real-Turso `common::` harness. Unknown-flag test = run binary with a typo'd flag, assert exit!=0 + stderr contains `unknown argument`. Valid-invocation regression guard where a command's misclassification risk is real.
- `./bin/travel` is RELEASE — rebuild before live smoke.

## CONNECT-BEFORE-PARSE caveat (source-verified, main.rs)
`mark-booked` / `sync-bookings` / `fetch-weather` are dispatched AFTER `main.rs` calls `plan_resolver::resolve_plan_id(rest)` (opens Turso even with `--plan-id`). So a binary unknown-flag test for these is NOT credless-hermetic unless the reject runs BEFORE the resolver. **Fix for these 3:** call `reject_unknown_flags` in the `main.rs` dispatch arm BEFORE `resolve_plan_id(rest)`, using that command's flag lists. (The other commands parse-before-connect inside `run()`, so a module-top call suffices.)

---

### Task 1: Hoist the helper into `plan_resolver.rs`

**Files:** Modify `plan_resolver.rs` (add pub(crate) fn + unit tests), `set_route_segment.rs` (delete private copy, re-point 2 call sites).

- [ ] **Step 1: Unit tests in `plan_resolver.rs`** (no DB):
```rust
#[test] fn reject_unknown_flags_errors_on_bogus() {
    assert!(reject_unknown_flags(&["--bogus".into()], &[], &[]).is_err());
}
#[test] fn reject_unknown_flags_value_flag_consumes_dashdash_value() {
    // --note "--weird" : the value may itself start with -- and must NOT be flagged
    assert!(reject_unknown_flags(&["--note".into(), "--weird".into()], &["--note"], &[]).is_ok());
}
#[test] fn reject_unknown_flags_bool_flag_does_not_consume_next() {
    // --dry-run is bool; the following --bogus must still be rejected
    assert!(reject_unknown_flags(&["--dry-run".into(), "--bogus".into()], &[], &["--dry-run"]).is_err());
}
#[test] fn reject_unknown_flags_positionals_pass() {
    assert!(reject_unknown_flags(&["zzsrc".into(), "fit".into()], &[], &[]).is_ok());
}
```
- [ ] **Step 2: run → FAIL** (`cargo test -p travel-cli plan_resolver` — fn not found).
- [ ] **Step 3:** Move the fn from `set_route_segment.rs:298` into `plan_resolver.rs`, make it `pub(crate)`, keep the doc comment.
- [ ] **Step 4:** In `set_route_segment.rs` delete the private copy, re-point its 2 call sites to `crate::plan_resolver::reject_unknown_flags(...)`.
- [ ] **Step 5: run → PASS** (`cargo test -p travel-cli plan_resolver` + `cargo build -p travel-cli`). Run `set_route_segment` tests to confirm no regression.
- [ ] **Step 6: commit** — `cli: hoist unknown-flag rejection helper into plan_resolver`

---

### Task 2 (B1): `mark-booked` — HIGHEST (--dry-run typo → real write)

**Files:** `main.rs` (dispatch arm preflight), `mark_booked.rs`, `tests/mark_booked.rs`.
Flags — VALUE: `--dest` + the 5 resolver value flags; BOOL: `--dry-run`.

- [ ] **Step 1: failing test** in `tests/mark_booked.rs`:
```rust
#[test]
fn mark_booked_rejects_unknown_flag_before_write() {
    let out = Command::new(bin())
        .args(["mark-booked", "--dest", "zz", "--dry-rnu"])
        .env("TRAVEL_PLAN_ID", "zz-no-db")
        .output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("unknown argument: --dry-rnu"), "stderr={stderr}");
}
```
- [ ] **Step 2: run → FAIL** (currently `--dry-rnu` ignored → dry_run=false → proceeds to resolver/DB).
- [ ] **Step 3:** In `main.rs` `mark-booked` dispatch arm, BEFORE `resolve_plan_id(rest)`:
```rust
crate::plan_resolver::reject_unknown_flags(
    rest,
    &["--dest", "--plan-id", "--plan-path", "--travel-date", "--travel-start", "--travel-end"],
    &["--dry-run"],
)?;
```
- [ ] **Step 4: run → PASS** (background, serialized).
- [ ] **Step 5: commit** — `cli: reject unknown flags in mark-booked (--dry-run typo → real write)`

---

### Task 3 (B2): `sync-bookings` + (B5) `fetch-weather` — the other 2 connect-before-parse

**Files:** `main.rs` (2 dispatch arms), `tests/` for each.

**sync-bookings** — VALUE: 5 resolver flags + `--trip-id`; BOOL: `--dry-run`. Preflight in dispatch arm:
```rust
crate::plan_resolver::reject_unknown_flags(rest,
    &["--plan-id","--plan-path","--travel-date","--travel-start","--travel-end","--trip-id"],
    &["--dry-run"])?;
```
**fetch-weather** — VALUE: `--dest` + 5 resolver flags; BOOL: `--all`. Preflight in dispatch arm:
```rust
crate::plan_resolver::reject_unknown_flags(rest,
    &["--dest","--plan-id","--plan-path","--travel-date","--travel-start","--travel-end"],
    &["--all"])?;
```
- [ ] Failing test per command (typo'd `--dry-runn` / `--al`), FAIL, add preflight, PASS. (fetch-weather valid path calls curl — the UNKNOWN-flag test rejects before connect so it's hermetic; do NOT add a live valid-weather test.)
- [ ] **commit** — `cli: reject unknown flags in sync-bookings and fetch-weather`

---

### Task 4 (B3): `set_ota_catalog.rs` — all 5 subcommands (parse-before-connect, module-top calls)

Each `run_set_*` calls the helper with ITS OWN list (a flag valid for coverage is unknown for source). Source-verified classifications:

| subcommand | VALUE flags | BOOL flags |
|---|---|---|
| set-ota-source | `--name`, `--status` | none |
| set-ota-coverage | `--proven-at`, `--method`, `--search-url`, `--blocked` | `--proven` |
| set-ota-region | none (positionals only) | none |
| set-ota-workflow | `--nav`, `--url-template`, `--capture-url-contains`, `--settle-ms`, `--settle-marker`, `--note` | none |
| set-ota-url-param | none (positionals only) | none |

- [ ] Failing test per subcommand (typo, e.g. `--provenn` for coverage, `--dry-run` for region/url-param which have no flags at all), FAIL, add `crate::plan_resolver::reject_unknown_flags(args, VALUE, BOOL)?;` near the top of each `run_set_*` (after the `--help` guard, before positional parsing), PASS.
- [ ] Valid regression guards: existing `set_coverage_proven_requires_date_and_method` + workflow test cover full valid invocations; add round-trip for region/url-param if absent.
- [ ] **commit** — `cli: reject unknown flags in ota catalog mutations (5 subcommands)`

---

### Task 5 (B4a): offer import/promotion — `promote-offers`, `import-offers`

Both parse-before-connect (main.rs calls their own `parse_args`). Module-top calls.
- **promote-offers** — VALUE: 5 resolver + `--dest`,`--source`,`--start`,`--end`,`--pax`; BOOL: `--from-offers`,`--dry-run`.
- **import-offers** — VALUE: `--dest`,`--dir`,`--files`,`--start`,`--end`,`--pax`,`--note`; BOOL: `--dry-run`. (NOTE: import-offers takes NO resolver `--plan-id`? Confirm — it's dest-scoped. If it accepts `--plan-id`, include the resolver flags.)
- [ ] Failing test each (typo), FAIL, add reject call, PASS. **commit** — `cli: reject unknown flags in offer import and promotion`

---

### Task 6 (B4b): manual tour-offer commands

All parse-before-connect. All flags are VALUE (no bool flags):
- **add-offer** — `--run,--kind,--title,--hotel,--price,--flight,--depart,--return,--seats,--nights,--baggage,--source,--url,--region,--note`
- **add-besttour-offer** — `--run,--url,--price,--hotel,--depart,--return,--seats,--note`
- **add-lifetour-offer** — `--run,--url,--price,--hotel,--nights,--depart,--return,--seats,--note`
- **import-tour-group-offers** — `--run,--file`
- [ ] Failing test each (typo a value flag, e.g. `--sourc`), FAIL, add reject call (all-value, empty bool list), PASS. **commit** — `cli: reject unknown flags in tour-offer mutations`

---

## Final Verification
```bash
cd rust && cargo test -p travel-cli plan_resolver          # helper unit tests
cargo test -p travel-cli --test mark_booked -- --test-threads=1   # (background) + the other touched tests
cargo build -p travel-cli --release && cp target/release/travel ../bin/travel
# live smoke: a typo must fail loud
./bin/travel mark-booked --dest tokyo_2026 --dry-rnu    # expect: "unknown argument: --dry-rnu", exit!=0
```
End property: every listed command rejects an unknown `--flag` with `unknown argument: <flag>` and exit!=0; every VALID invocation still parses (classification correct); plain text, no behavior change otherwise.
