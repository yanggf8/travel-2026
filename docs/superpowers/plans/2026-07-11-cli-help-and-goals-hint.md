# CLI `--help` parity + populate `--goals` hint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two agent-first UX gaps the real-data redrill surfaced: (F2) four read/query commands reject `--help` with `unknown flag` instead of printing Usage; (F3) `populate-itinerary`'s missing-`--goals` error doesn't tell the agent where to find available clusters.

**Architecture:** F2 — intercept `--help`/`-h` in each of the four commands' `main.rs` dispatch arms BEFORE calling `<module>::parse`, using the EXISTING shared `wants_help(rest, usage) -> bool` helper (main.rs:828) that 23 other arms already use (e.g. populate-itinerary at main.rs:639). One line per arm: `if wants_help(rest, "<usage>") { return Ok(()); }`. Keeps the fix in one file, consistent with the codebase convention, and does not touch the four modules' arg-parsing (avoids the trap of breaking positional/plan resolution). F3 — append a one-line pointer to the `requires --goals` error string in `populate_itinerary.rs`.

**Tech Stack:** Rust (travel-cli command dispatch), real-Turso-optional integration test on `tests/common/mod.rs` (these are read-only commands; `--help` prints before any DB connect, so the test needs no creds).

## Global Constraints

- **Agent-first plain text** — Usage goes to stdout as plain text; no JSON.
- **Fail loud** — a REAL unknown flag (typo) must still error; only `--help`/`-h` is intercepted.
- **No behavior change to real invocations** — a command with valid args must behave exactly as before; only `--help`/`-h` short-circuits to Usage.
- **Corroborated module bindings (do NOT trust the findings doc's module names — they were wrong).** The four commands bind, in `main.rs`, to:
  - `query-offers` (main.rs:164) → `offers::OffersArgs::parse` (offers.rs; unknown-flag at offers.rs:49). NOT `db_query_offers.rs` (that's the separate `db query-offers` subcommand, which already has --help).
  - `query-destination-ref` (main.rs:168) → `destination_ref::DestRefArgs::parse` (destination_ref.rs:36).
  - `query-bookings` (main.rs:172) → `bookings::QueryBookingsArgs::parse` (bookings.rs:42).
  - `check-freshness` (main.rs:176) → `freshness::FreshnessArgs::parse` (freshness.rs:52).
- **Pattern to copy — the SHARED helper, not inline `any(...)`:** `wants_help(rest: &[String], usage: &str) -> bool` (main.rs:828) prints `Usage:\n  {usage}` and returns true. 23 arms already use it (e.g. `populate-itinerary` at main.rs:639: `if wants_help(rest, "...") { return Ok(()); }`). Add ONE such line as the first statement of each of the four arms. Do NOT hand-roll `if rest.iter().any(...)` — use `wants_help` for consistency. Note `wants_help` already prepends `Usage:\n  ` so the `usage` string you pass is just the body (no leading `Usage:`).
- **Tests** — a `--help` round-trip test (run binary with `--help`, assert exit 0 + Usage on stdout + no "unknown flag") + a preserved-error test (a real typo'd flag still errors). Runs without Turso creds (help prints pre-connect). Serialized not required (no DB writes), but harmless.
- **Pipeline** — Grok 4.5 implements task-by-task against these tests; Claude reviews every line + corroborates vs source + verifies. Commit explicit pathspecs only.

---

## File Structure

- `rust/crates/travel-cli/src/main.rs` — add a `--help`/`-h` interceptor to the four dispatch arms (query-offers:164, query-destination-ref:168, query-bookings:172, check-freshness:176). Each prints that command's Usage. (F2)
- `rust/crates/travel-cli/src/populate_itinerary.rs:365` — append the cluster-listing pointer to the `requires --goals` error. (F3)
- `rust/crates/travel-cli/tests/cli_help_parity.rs` — NEW test file: `--help` prints Usage (exit 0, no "unknown flag") for all four commands + a typo still errors + populate's --goals error names query-destination-ref.

---

## Task 1 (commit 1) — F2: four commands print Usage on `--help`

**Files:**
- Modify: `rust/crates/travel-cli/src/main.rs` (dispatch arms at :164, :168, :172, :176)
- Test: `rust/crates/travel-cli/tests/cli_help_parity.rs` (new)

**Interfaces:**
- Consumes: nothing new — reuses the existing `import-offers`/set-tod interceptor pattern.
- Produces: `query-offers|query-destination-ref|query-bookings|check-freshness --help` (and `-h`) → prints a `Usage:` block to stdout and returns `Ok(())` (exit 0), never reaching `::parse`.

- [ ] **Step 1: Write the failing test**

Create `rust/crates/travel-cli/tests/cli_help_parity.rs`. Uses only the binary (no DB — `--help` prints before any connect), so no creds/teardown needed. Get the binary path the same way the other tests do (`mod common; use common::bin;` — `bin()` returns the binary path).

```rust
mod common;
use common::bin;
use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn help_prints_usage_for_query_commands() {
    for cmd in ["query-offers", "query-destination-ref", "query-bookings", "check-freshness"] {
        for flag in ["--help", "-h"] {
            let (ok, stdout, stderr) = run(&[cmd, flag]);
            let combined = format!("{stdout}{stderr}");
            assert!(ok, "{cmd} {flag} should exit 0; stderr={stderr}");
            assert!(
                combined.contains("Usage") || combined.contains("usage"),
                "{cmd} {flag} should print Usage; got: {combined}"
            );
            assert!(
                !combined.contains("unknown flag"),
                "{cmd} {flag} must NOT say 'unknown flag'; got: {combined}"
            );
        }
    }
}

#[test]
fn real_typo_flag_still_errors() {
    // A genuine unknown flag must still fail loud (we only intercept --help/-h).
    let (ok, _stdout, stderr) = run(&["query-offers", "--totally-bogus-flag"]);
    assert!(!ok, "a real unknown flag must still error");
    assert!(
        stderr.contains("unknown flag") || stderr.contains("Error"),
        "a real unknown flag must fail loud; stderr={stderr}"
    );
}
```

> `bin()` is defined in `tests/common/mod.rs` (path to the release/debug binary). If the four commands try to connect to Turso even for `--help` in the current code (they should not — but verify), the test would hang/fail on creds; the whole point of Task 1 is that `--help` short-circuits BEFORE `::parse`/connect, so post-fix this test is creds-free. If `real_typo_flag_still_errors` needs creds to reach the unknown-flag branch, it doesn't — `::parse` runs before any connect and errors on the bad flag synchronously.

- [ ] **Step 2: Run test to verify it fails**

Build debug + run:
```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test cli_help_parity -- --nocapture
```
Expected: `help_prints_usage_for_query_commands` FAILS — the four commands currently return `unknown flag for <cmd>: --help` (non-zero exit, stderr has "unknown flag"). `real_typo_flag_still_errors` PASSES already.

- [ ] **Step 3: Add the `--help` interceptor to the four dispatch arms**

In `rust/crates/travel-cli/src/main.rs`, add `if wants_help(rest, "<usage-body>") { return Ok(()); }` as the first statement of each of the four arms, before `::parse`. `wants_help` (main.rs:828) already prepends `Usage:\n  `, so pass only the body. Draw the flag list from each module's `parse` match arms (VERIFY against source — a wrong flag in Usage is worse than none):

The flag lists below are the EXACT flags each module's `parse` accepts (verified against the source `match` arms — `--destination` is an alias of `--dest`, omitted from Usage for brevity):

```rust
        [cmd, rest @ ..] if cmd == "query-offers" => {
            if wants_help(rest, "travel query-offers [--dest <slug>] [--region <r>] [--start <date>] [--end <date>] [--source <s>] [--max-price <twd>] [--limit <n>]") { return Ok(()); }
            let opts = offers::OffersArgs::parse(rest)?;
            offers::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "query-destination-ref" || cmd == "destination-ref" => {
            if wants_help(rest, "travel query-destination-ref --slug <destination_slug>\n  (lists areas, clusters, POIs, transit, tips for a registered destination — e.g. tokyo_2026)") { return Ok(()); }
            let opts = destination_ref::DestRefArgs::parse(rest)?;
            destination_ref::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "query-bookings" => {
            if wants_help(rest, "travel query-bookings [--dest <slug>] [--category <c>] [--status <s>] [--max <n>] [--trip-id <id>]") { return Ok(()); }
            let opts = bookings::QueryBookingsArgs::parse(rest)?;
            bookings::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "check-freshness" => {
            if wants_help(rest, "travel check-freshness --source <s> [--dest <slug>] [--region <r>] [--start <date>] [--end <date>] [--max-age <hours>] [--plan-id <id>]") { return Ok(()); }
            let opts = freshness::FreshnessArgs::parse(rest)?;
            freshness::run(&opts).await
        }
```

> The flag lists above are VERIFIED (grepped `"--..."` from each module 2026-07-11): offers.rs = dest/region/start/end/source/max-price/limit; destination_ref.rs = slug; bookings.rs = dest/category/status/max/trip-id; freshness.rs = source/dest/region/start/end/max-age/plan-id. Copy them verbatim. Do NOT change the modules; only the four `main.rs` arms. Use `wants_help`, not inline `any(...)`.

- [ ] **Step 4: Build + run test to verify it passes**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test cli_help_parity -- --nocapture
```
Expected: both tests PASS. Also spot-check no other command regressed:
```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test reject_flags_bookings_weather 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
cd /home/yanggf/b/travel-2026
git add rust/crates/travel-cli/src/main.rs rust/crates/travel-cli/tests/cli_help_parity.rs
git commit -F - <<'EOF'
fix(cli): query-offers/bookings/destination-ref/check-freshness print Usage on --help (F2)

The four read/query commands rejected --help with "unknown flag" (their arg
parsers have no --help arm), so an agent's first exploration step returned an
error instead of Usage. Intercept --help/-h in each main.rs dispatch arm before
::parse (same pattern as import-offers/set-tod), printing Usage and returning
Ok(()). A real typo'd flag still fails loud. Found in the tokyo-sep real-data
redrill. Modules unchanged (no risk to positional/plan parsing).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 2 (commit 2) — F3: populate `--goals` error points to the cluster list

**Files:**
- Modify: `rust/crates/travel-cli/src/populate_itinerary.rs:365`
- Test: `rust/crates/travel-cli/tests/cli_help_parity.rs` (add one test — the error text; no creds needed, error prints before connect)

**Interfaces:**
- Produces: `populate-itinerary` with no `--goals` → error string ends with `— list available clusters with: travel query-destination-ref --slug <dest>`.

- [ ] **Step 1: Write the failing test**

CORROBORATED ORDERING (re-verified 2026-07-11, live): the `requires --goals` error lives INSIDE `parse_args` (`populate_itinerary.rs:365`, within the `fn parse_args` at :322), and `run()` calls `parse_args(args)` as its FIRST line (:66) — BEFORE `connect_write` (:68). And `resolve_plan_id` with an explicit `--plan-id` returns that id WITHOUT touching Turso. So the `--goals` error fires SYNCHRONOUSLY, pre-connect, pre-DB — **NO seed, NO creds needed**. Verified live: `populate-itinerary --plan-id some-plan-xyz` (unseeded, no creds) → stderr `populate-itinerary requires --goals ...`, exit 1. (This is the simpler creds-free test; do NOT seed a plan — that was an earlier over-correction.)

Add to `tests/cli_help_parity.rs`:

```rust
#[test]
fn populate_missing_goals_points_to_cluster_list() {
    // The --goals error is emitted by parse_args before any Turso connect, so
    // no creds / no seed needed. An explicit --plan-id avoids plan ambiguity.
    let (ok, _stdout, stderr) = run(&["populate-itinerary", "--plan-id", "no-such-plan-xyz"]);
    assert!(!ok, "missing --goals must error (exit non-zero)");
    assert!(
        stderr.contains("requires --goals"),
        "expected the --goals error; got: {stderr}"
    );
    assert!(
        stderr.contains("query-destination-ref"),
        "the --goals error must point to query-destination-ref; got: {stderr}"
    );
}
```

> This is a plain `#[test]` (no tokio, no harness beyond the local `run()`), because the target error is synchronous and DB-free. The three assertions are the lock: exits non-zero, says "requires --goals", and names "query-destination-ref".

- [ ] **Step 2: Run test to verify it fails**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test cli_help_parity populate_missing_goals -- --nocapture
```
Expected: FAIL — current error is `populate-itinerary requires --goals "<cluster1,cluster2,...>"` with no `query-destination-ref` pointer.

- [ ] **Step 3: Append the pointer to the error string**

In `rust/crates/travel-cli/src/populate_itinerary.rs:365`, change:
```rust
        goals_opt.ok_or_else(|| "populate-itinerary requires --goals \"<cluster1,cluster2,...>\"".to_string())?;
```
to:
```rust
        goals_opt.ok_or_else(|| {
            "populate-itinerary requires --goals \"<cluster1,cluster2,...>\"\n  \
             — list available clusters with: travel query-destination-ref --slug <dest>"
                .to_string()
        })?;
```
(Match the surrounding indentation/style; the `— list …` continuation mirrors the 0-added error at :306-308 which already points to `query-destination-ref`.)

- [ ] **Step 4: Build + run test to verify it passes**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test cli_help_parity -- --nocapture
```
Expected: all three tests PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/yanggf/b/travel-2026
git add rust/crates/travel-cli/src/populate_itinerary.rs rust/crates/travel-cli/tests/cli_help_parity.rs
git commit -F - <<'EOF'
fix(cli): populate-itinerary --goals error points to the cluster list (F3)

The missing-`--goals` error didn't tell the agent where to find available
clusters (the 0-added error already did). Append
"— list available clusters with: travel query-destination-ref --slug <dest>".
With F2 fixed, that command is now --help-explorable too. Found in the
tokyo-sep real-data redrill.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Live smoke (after both commits)

```bash
cd /home/yanggf/b/travel-2026
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
# F2: all four now print Usage
for c in query-offers query-destination-ref query-bookings check-freshness; do
  echo "== $c --help =="; ./bin/travel $c --help | head -2
done
# F3: the goals error points to the cluster list
./bin/travel populate-itinerary --plan-id tokyo-sep-2026 2>&1 | grep -A1 "requires --goals"
```
Expected: four Usage blocks (no "unknown flag"); the populate error names `query-destination-ref`.

## Acceptance

- F2: `query-offers|query-destination-ref|query-bookings|check-freshness --help` (and `-h`) print Usage + exit 0 + no "unknown flag"; a real typo'd flag still fails loud. Modules unchanged.
- F3: missing-`--goals` error ends with the `query-destination-ref --slug <dest>` pointer.
- `cli_help_parity.rs` green; `reject_flags_bookings_weather` (and other) regressions green.
- Live smoke confirms both on the real `tokyo-sep-2026` plan.
