#!/usr/bin/env bash
# Dashboard cutover: reclaim trip-dashboard.yanggf.workers.dev for the -rs worker.
# Driver for docs/plans/2026-07-02-dashboard-cutover-ts-to-rs.md (+ deploy-day-checklist.md).
#
# What it automates: the GATE-0 dry-run, the two prod deploys, the 3 scriptable secrets
# (PUBLIC_ORIGIN literal + TURSO_URL/TURSO_TOKEN from .env), and the smoke-test URLs.
# What it CANNOT automate (wrangler can't read existing secret values / browser+GitHub):
#   - the GitHub OAuth-app callback URL change (browser) — prompted as a hard gate
#   - SESSION_SECRET / GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET — you PASTE them (must match -rs)
#
# Usage:  cd workers/trip-dashboard-rs && ./scripts/deploy-cutover.sh
#         ./scripts/deploy-cutover.sh --rollback     # redeploy the TS worker to restore the URL
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RS_DIR="$REPO_ROOT/workers/trip-dashboard-rs"
TS_DIR="$REPO_ROOT/workers/trip-dashboard"
PROD_URL="https://trip-dashboard.yanggf.workers.dev"
RS_URL="https://trip-dashboard-rs.yanggf.workers.dev"

pause() { echo; read -r -p "▶ $1 [enter to continue, Ctrl-C to abort] " _; echo; }
gate()  { echo; read -r -p "⛔ GATE — $1 (y/N) " a; [[ "$a" == "y" || "$a" == "Y" ]] || { echo "Gate failed → aborting. Roll back with: $0 --rollback"; exit 1; }; }

if [[ "${1:-}" == "--rollback" ]]; then
  echo "== ROLLBACK: redeploy the TS worker as trip-dashboard =="
  cd "$TS_DIR"; unset CLOUDFLARE_API_TOKEN; npx wrangler deploy
  echo "✅ TS worker redeployed — $PROD_URL restored."
  exit 0
fi

cd "$RS_DIR"; unset CLOUDFLARE_API_TOKEN

# --- Pre-flight ---
echo "== Pre-flight: git pull + GATE 0 (prod dry-run build) =="
( cd "$REPO_ROOT" && git pull --no-rebase )
npx wrangler deploy --env production --dry-run   # must exit 0
gate "GATE 0: dry-run printed 'Total Upload …' and exited 0?"

# --- Phase 1a: baseline -rs on its own URL ---
echo "== 1a — verify the -rs worker is healthy on its own URL first =="
echo "   Open: $RS_URL           (owner login works, a plan renders)"
echo "   Open: $RS_URL/?plan=okinawa-2026&token=<valid share token>   (logged-out window renders)"
gate "GATE 1a: -rs is healthy on its own URL? (if broken, do NOT cut over)"

# --- Phase 1b: GitHub OAuth callback (MANUAL, browser, do FIRST) ---
echo "== 1b — point the GitHub OAuth app at the target URL (the #1 gotcha) =="
echo "   GitHub → Settings → Developer settings → OAuth Apps → dashboard app →"
echo "   Authorization callback URL: ADD  $PROD_URL/auth/callback   (keep the -rs one too)"
pause "Done the GitHub OAuth callback change?"

# --- Phase 1c: deploy -rs under the production name (reclaims the URL) ---
echo "== 1c — deploy -rs under name=trip-dashboard (reclaims the URL from the TS worker) =="
npx wrangler deploy --env production

# --- Phase 1d: production-env secrets ---
echo "== 1d — set production-env secrets (env-scoped; top-level do NOT carry over) =="
TU="$(grep '^TURSO_URL='   "$REPO_ROOT/.env" | cut -d= -f2-)"
TT="$(grep '^TURSO_TOKEN=' "$REPO_ROOT/.env" | cut -d= -f2-)"
[[ -n "$TU" && -n "$TT" ]] || { echo "TURSO_URL/TURSO_TOKEN missing from $REPO_ROOT/.env"; exit 1; }
printf '%s' "$PROD_URL" | npx wrangler secret put PUBLIC_ORIGIN --env production   # ← the CHANGED one
printf '%s' "$TU"       | npx wrangler secret put TURSO_URL     --env production
printf '%s' "$TT"       | npx wrangler secret put TURSO_TOKEN   --env production
echo "   The next 3 must MATCH the -rs worker's values (wrangler can't copy them) — paste when prompted:"
npx wrangler secret put SESSION_SECRET       --env production
npx wrangler secret put GITHUB_CLIENT_ID     --env production
npx wrangler secret put GITHUB_CLIENT_SECRET --env production

# --- Phase 1e: redeploy so secrets take effect ---
echo "== 1e — redeploy so the new secrets take effect =="
npx wrangler deploy --env production

# --- Phase 1f: smoke test on the reclaimed URL ---
echo "== 1f — smoke test the RECLAIMED URL: $PROD_URL =="
echo "   [ ] owner login → OAuth callback → owner dashboard renders (proves PUBLIC_ORIGIN/callback)"
echo "   [ ] logout works"
echo "   [ ] $PROD_URL/?plan=<slug>&token=<share>  renders logged-out"
echo "   [ ] a per-day map image loads (R2 MAPS)"
echo "   [ ] $PROD_URL/?lang=en  renders English"
gate "GATE 1: all 5 smoke tests pass? (if login fails → run '$0 --rollback')"

echo
echo "✅ Cutover LIVE — $PROD_URL now served by the -rs worker."
echo "   Leave the TS worker dormant as instant rollback ('$0 --rollback')."
echo "   When confident (recommend ~a day): cd $TS_DIR && npx wrangler delete   (optional)"
echo "   Next: Phase 2 (D1 pilot) — ./scripts/deploy-d1-pilot.sh  (only after this is stable)."
