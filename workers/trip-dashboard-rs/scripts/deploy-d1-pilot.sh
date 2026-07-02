#!/usr/bin/env bash
# D1 read-mirror pilot (compare-only; D1 NEVER serves). Run ONLY after the cutover is stable.
# Driver for docs/plans/2026-07-02-dashboard-d1-mirror-pilot.md (+ deploy-day-checklist.md Phase 2).
#
# Automates: D1 create, generating the pilot-seed SQL (CREATE TABLE + INSERTs for plans+date_anchors
# from live Turso), loading it, the deploy, and printing the /diag/d1-compare URL.
# You do manually: paste the printed database_id into wrangler.toml (2b) — the binding block is a
# TOML edit the script can't safely auto-insert; it's prompted with the exact snippet.
#
# Usage:  cd workers/trip-dashboard-rs && ./scripts/deploy-d1-pilot.sh
#         ./scripts/deploy-d1-pilot.sh --teardown
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RS_DIR="$REPO_ROOT/workers/trip-dashboard-rs"
PROD_URL="https://trip-dashboard.yanggf.workers.dev"
DB="trip-dashboard-mirror"
SEED="$RS_DIR/build/d1-pilot-seed.sql"
TRAVEL="$REPO_ROOT/bin/travel"

pause() { echo; read -r -p "▶ $1 [enter to continue, Ctrl-C to abort] " _; echo; }
gate()  { echo; read -r -p "⛔ GATE — $1 (y/N) " a; [[ "$a" == "y" || "$a" == "Y" ]] || { echo "Gate failed → aborting."; exit 1; }; }

cd "$RS_DIR"; unset CLOUDFLARE_API_TOKEN

if [[ "${1:-}" == "--teardown" ]]; then
  echo "== TEARDOWN: set D1_COMPARE_ENABLED=0 in wrangler.toml + deploy, then delete the mirror =="
  pause "Set D1_COMPARE_ENABLED=\"0\" (or remove) under [env.production.vars] in wrangler.toml, then continue"
  npx wrangler deploy --env production
  npx wrangler d1 delete "$DB"
  echo "✅ D1 pilot torn down (compare route now 404s; mirror destroyed). Turso remains sole source of truth."
  exit 0
fi

# Turso creds for reading current rows
TU="$(grep '^TURSO_URL='   "$REPO_ROOT/.env" | cut -d= -f2-)"
TT="$(grep '^TURSO_TOKEN=' "$REPO_ROOT/.env" | cut -d= -f2-)"
export TRAVEL_TURSO_URL="$TU" TRAVEL_TURSO_READ_TOKEN="$TT" TRAVEL_TURSO_WRITE_TOKEN="$TT"

# --- 2a: create the D1 database ---
echo "== 2a — create the D1 database =="
npx wrangler d1 create "$DB" || echo "(if it already exists, that's fine — reuse its id)"

# --- 2b: bind it + set the flag (MANUAL TOML edit — prompted with the snippet) ---
echo "== 2b — add the binding + flag to wrangler.toml (paste the database_id printed above) =="
cat <<'SNIP'
   Add to workers/trip-dashboard-rs/wrangler.toml:

     [[env.production.d1_databases]]
     binding = "MIRROR_DB"
     database_name = "trip-dashboard-mirror"
     database_id = "<paste the id from 2a>"

   And under the EXISTING [env.production.vars] add:   D1_COMPARE_ENABLED = "1"
SNIP
pause "Edited wrangler.toml with the binding + D1_COMPARE_ENABLED=\"1\"?"

# --- 2c: build the pilot-seed SQL (CREATE TABLE + INSERTs) from live Turso — AUTOMATED ---
# DDL comes from the live DB's own sqlite_master (the authoritative source — schema.sql lacks `plans`,
# and `db schema` prints a column summary, not DDL). newlines→spaces so each is one D1 statement.
echo "== 2c — generate the pilot-seed SQL for plans + date_anchors from live Turso =="
mkdir -p "$(dirname "$SEED")"
{
  "$TRAVEL" db exec "SELECT replace(sql, char(10), ' ') || ';' AS ddl FROM sqlite_master WHERE type='table' AND name='plans'"        | sed -n 's/^ddl: //p'
  "$TRAVEL" db exec "SELECT replace(sql, char(10), ' ') || ';' AS ddl FROM sqlite_master WHERE type='table' AND name='date_anchors'" | sed -n 's/^ddl: //p'
  # current rows → INSERTs (both tables tiny; quote() escapes). db exec prints "col: value" text; strip prefix.
  "$TRAVEL" db exec "SELECT 'INSERT INTO plans(plan_id,schema_version,version,deleted_at) VALUES(' || quote(plan_id) || ',' || quote(schema_version) || ',' || COALESCE(version,0) || ',' || COALESCE(quote(deleted_at),'NULL') || ');' AS sql FROM plans" \
    | sed -n 's/^sql: //p'
  "$TRAVEL" db exec "SELECT 'INSERT INTO date_anchors(plan_id,destination,start_date,end_date,days) VALUES(' || quote(plan_id) || ',' || quote(destination) || ',' || quote(start_date) || ',' || quote(end_date) || ',' || COALESCE(days,0) || ');' AS sql FROM date_anchors" \
    | sed -n 's/^sql: //p'
} > "$SEED"
echo "   Wrote $SEED ($(wc -l < "$SEED") lines). Review it, then load:"
echo "   --- head ---"; head -8 "$SEED"
gate "Seed SQL looks right (2 CREATE TABLEs + INSERTs)?"

# --- load schema+rows into D1 ---
echo "== load the seed into D1 =="
npx wrangler d1 execute "$DB" --file "$SEED"

# --- 2d: deploy + point at the compare route ---
echo "== 2d — deploy, then open the compare report =="
npx wrangler deploy --env production
echo
echo "   Open (owner logged in):  $PROD_URL/diag/d1-compare"
echo "   → plain-text report:  table  turso_rows  d1_rows  verdict"
gate "GATE 2: the route returned the report (not 404/403)? (404=flag/binding not picked up; 403=not owner)"

echo
echo "✅ D1 pilot live. Read the delta:"
echo "   MATCH on both tables → dialect delta trivial for these reads (a future D1-read phase is scopeable)."
echo "   ROW COUNT/COLUMN SET/VALUE DIFFERS → that line names the exact delta to fix. Record it; decision is yours."
echo "   Teardown when done:  ./scripts/deploy-d1-pilot.sh --teardown"
