#!/usr/bin/env bash
# snapshot-maps.sh — render per-day + plan ROUTE maps (numbered markers + a
# connecting polyline, auto-framed) and upload the PNGs to the R2 bucket the
# dashboard worker serves from.
#
# Keyless: a self-contained Leaflet page (OSM tiles, Leaflet from unpkg CDN) is
# generated per day from that day's stop coordinates, then screenshotted via
# chromeport (CDP → real Chrome). No Google Maps API key. Replaces the old
# blank-basemap screenshots (which showed no stops and no route, and used
# hardcoded per-plan centers).
#
# Requires: real Chrome reachable at :9222 (./bin/chromeport browser doctor),
#           wrangler authenticated to Cloudflare, ./bin/chromeport with `screenshot`,
#           Turso creds reachable by ./bin/chromeport db query.
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

# --- pull every stop with coordinates, in itinerary order, as: day<TAB>lat<TAB>lon ---
# An activity links to a POI by the durable poi_id FK, else by exact title match
# (mirrors the worker's attach_stops). Only POIs with real lat/lon are plotted.
echo "== query stop coordinates from Turso =="
ROWS=$($CHROMEPORT db query "SELECT a.day_number, p.lat, p.lon \
  FROM activities a \
  JOIN destination_pois p \
    ON (p.poi_id = a.poi_id OR (a.poi_id IS NULL AND p.title = a.title)) \
   AND p.slug = '${DEST}' \
  WHERE a.plan_id = '${PLAN}' AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
  ORDER BY a.day_number, a.sort_order")

# Strip the TSV header line and the trailing "(N rows)" footer; keep day/lat/lon.
COORDS=$(printf '%s\n' "$ROWS" \
  | awk -F'\t' 'NR>1 && NF>=3 && $2!="" && $3!="" && $1 ~ /^[0-9]+$/ {print $1"\t"$2"\t"$3}')

if [ -z "$COORDS" ]; then
  echo "ERROR: no stop coordinates found for ${PLAN}/${DEST} — nothing to plot." >&2
  echo "       (Check destination_pois.lat/lon and activity↔POI linkage.)" >&2
  exit 1
fi

DAYS=$(printf '%s\n' "$COORDS" | cut -f1 | sort -un)
echo "   days with stops: $(printf '%s' "$DAYS" | tr '\n' ' ')"

# --- build one Leaflet HTML for a set of [lat,lon] points and screenshot it ---
# $1 = output base name (e.g. "day-1" / "plan"); reads "lat,lon" lines on stdin.
render_map() {
  local name="$1"
  local pts; pts="$(cat)"            # newline-separated "lat,lon"
  [ -z "$pts" ] && { echo "   skip ${name}: no points"; return; }

  # JS array literal: [[lat,lon],[lat,lon],...]
  local arr; arr="$(printf '%s\n' "$pts" | awk -F',' 'NF==2{printf "[%s,%s],",$1,$2}')"
  arr="[${arr%,}]"

  # The page is screenshotted via a percent-encoded data: URL (NOT file://) so it
  # works regardless of where Chrome runs — a Windows-host Chrome cannot read a
  # WSL /tmp file:// path. The HTML still travels inline over CDP. A debug copy is
  # also written to $OUT/<name>.html for inspection.
  local html="${OUT}/${name}.html"
  cat > "$html" <<HTML
<!doctype html><html><head><meta charset="utf-8">
<link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css">
<script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
<style>
  html,body{margin:0;padding:0;height:100%}
  #map{position:absolute;inset:0;background:#eef}
  .leaflet-control-attribution,.leaflet-control-zoom{display:none}
  .num{font:700 13px/24px sans-serif;color:#fff;text-align:center}
</style></head><body><div id="map"></div><script>
  var pts = ${arr};
  var map = L.map('map',{zoomControl:false,attributionControl:false});
  L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png',{maxZoom:19}).addTo(map);
  if (pts.length === 1) {
    map.setView(pts[0], 15);
  } else {
    map.fitBounds(pts, {padding:[45,45]});
  }
  // route polyline (connect-the-dots, itinerary order)
  if (pts.length > 1) {
    L.polyline(pts,{color:'#e23',weight:4,opacity:.85}).addTo(map);
  }
  // numbered markers
  pts.forEach(function(p,i){
    L.marker(p,{icon:L.divIcon({className:'',html:
      '<div style="width:24px;height:24px;border-radius:50%;background:#1e66f5;'+
      'border:2px solid #fff;box-shadow:0 1px 3px rgba(0,0,0,.4)" class="num">'+(i+1)+'</div>',
      iconSize:[24,24],iconAnchor:[12,12]})}).addTo(map);
  });
  // signal readiness for the screenshotter once tiles settle
  setTimeout(function(){document.title='MAP_READY';}, 1200);
</script></body></html>
HTML

  # Encode the HTML into a data: URL (filesystem-independent — works whether
  # Chrome is on this host or a Windows host across the WSL boundary).
  local data_url
  data_url="$(python3 -c 'import urllib.parse,sys; print("data:text/html,"+urllib.parse.quote(open(sys.argv[1]).read()))' "$html")"
  # Longer wait: the page must pull Leaflet from the CDN + OSM tiles before paint.
  $CHROMEPORT screenshot "$data_url" --out "${OUT}/${name}.png" \
    --width 640 --height 440 --wait 9000 | grep -E 'screenshot|error' || true
  sleep 1
}

echo "== render per-day route maps =="
for d in $DAYS; do
  printf '%s\n' "$COORDS" | awk -F'\t' -v d="$d" '$1==d{print $2","$3}' | render_map "day-${d}"
done

echo "== render plan-wide route map (all stops) =="
printf '%s\n' "$COORDS" | awk -F'\t' '{print $2","$3}' | render_map "plan"

echo "== upload to R2 ($BUCKET) =="
for f in "${OUT}"/*.png; do
  name=$(basename "$f")
  unset CLOUDFLARE_API_TOKEN
  npx wrangler r2 object put "${BUCKET}/${PLAN}/${name}" --file "$f" --content-type image/png --remote \
    | grep -iE 'upload|creating' || true
done

echo "== record snapshot timestamp (for the check-maps-fresh lint) =="
"$TRAVEL" mark-maps-snapshotted "$PLAN" || echo "warning: could not record snapshot timestamp (is ./bin/travel built?)"

echo "== done: route maps at ${BUCKET}/${PLAN}/{plan,day-<n>}.png =="
