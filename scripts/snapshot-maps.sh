#!/usr/bin/env bash
# snapshot-maps.sh — capture per-day + plan OSM map PNGs via chromeport (CDP → Windows Chrome)
# and upload them to the R2 bucket the dashboard worker serves from.
#
# Keyless: screenshots openstreetmap.org centered on each day's stop coordinates.
# Requires: real Windows Chrome reachable at :9222 (./bin/chromeport browser doctor),
#           wrangler authenticated to Cloudflare, ./bin/chromeport built with `screenshot`.
#
# Usage: scripts/snapshot-maps.sh <plan-id> <dest-slug>
#   e.g. scripts/snapshot-maps.sh okinawa-2026 okinawa_2026
#
# Map key convention (MUST match render/map.rs + the worker /map/* route):
#   <plan-id>/plan.png  and  <plan-id>/day-<n>.png
set -euo pipefail

PLAN="${1:?usage: snapshot-maps.sh <plan-id> <dest-slug>}"
DEST="${2:?usage: snapshot-maps.sh <plan-id> <dest-slug>}"
BUCKET="trip-dashboard-maps"
OUT="/tmp/${PLAN}-maps"
TRAVEL="./bin/travel"
CHROMEPORT="./bin/chromeport"
mkdir -p "$OUT"

echo "== chromeport / Chrome reachability =="
$CHROMEPORT browser doctor >/dev/null || { echo "Chrome not reachable at :9222"; exit 1; }

# Per-day centers come from the average of that day's stop coordinates
# (activity.title joined to destination_pois.lat/lon). Days with no sightseeing
# stops (arrival/departure) fall back to the plan-wide center.
#
# NOTE: centers/zooms below were computed for okinawa-2026 from its stop spread.
# For a different plan, recompute: query the per-day coords and pick a center+zoom
# that fits each day's extent (tighter spread → higher zoom).
#   SELECT a.day_number, p.lat, p.lon FROM activities a
#   JOIN destination_pois p ON p.title=a.title AND p.slug='<DEST>'
#   WHERE a.plan_id='<PLAN>' ORDER BY a.day_number;
#
# Format: name|lat|lon|zoom
CENTERS=$(cat <<EOF
plan|26.217|127.701|12
day-1|26.2145|127.7080|13
day-2|26.2212|127.7042|14
day-3|26.2137|127.6900|15
day-4|26.2141|127.6880|13
day-5|26.2145|127.7080|13
EOF
)

echo "== capture =="
while IFS='|' read -r name lat lon zoom; do
  [ -z "$name" ] && continue
  url="https://www.openstreetmap.org/#map=${zoom}/${lat}/${lon}"
  $CHROMEPORT screenshot "$url" --out "${OUT}/${name}.png" --width 640 --height 440 --wait 4500 \
    | grep -E 'screenshot|error' || true
  sleep 1
done <<< "$CENTERS"

echo "== upload to R2 ($BUCKET) =="
for f in "${OUT}"/*.png; do
  name=$(basename "$f")
  unset CLOUDFLARE_API_TOKEN
  npx wrangler r2 object put "${BUCKET}/${PLAN}/${name}" --file "$f" --content-type image/png --remote \
    | grep -iE 'upload|creating' || true
done

echo "== record snapshot timestamp (for the check-maps-fresh lint) =="
"$TRAVEL" mark-maps-snapshotted "$PLAN" || echo "warning: could not record snapshot timestamp (is ./bin/travel built?)"

echo "== done: maps at ${BUCKET}/${PLAN}/{plan,day-1..N}.png =="
