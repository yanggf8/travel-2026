#!/usr/bin/env bash
# snapshot-maps.sh — render per-day + plan ROUTE maps (numbered markers + a
# connecting polyline, auto-framed) and upload the PNGs to the R2 bucket the
# dashboard worker serves from.
#
# Keyless: a self-contained Leaflet page (OSM tiles, Leaflet from unpkg CDN) is
# generated per day, screenshotted via chromeport (a CDP *client*) attached to an
# isolated Chrome this script ACQUIRES from the gwebcdb per-agent allocator (Chrome
# is launched detached so it persists across our subprocess calls; released on exit).
# No Google Maps key. Stops come from TWO sources, in itinerary order:
#   1. activities → destination_pois (sightseeing POIs already geocoded), and
#   2. day_route_segments place names (hotel/airport/restaurant/mall/district)
#      resolved to coords via a keyless Nominatim/OSM geocode CACHED in Turso
#      (route_place_geocodes) — so days with no sightseeing POI (e.g. arrival/
#      departure/shopping days) still get a real route map.
# The plan overview draws each day as its OWN colored polyline (days are NOT
# chained end-to-end), so it reads as N day-routes, not one tangled line.
#
# FAIL-LOUD / no-garbage: a failed or undersized screenshot ABORTS that file's
# upload (never a 1-byte PNG). Every expected key's outcome (uploaded / skipped /
# failed + byte size) is written to the Turso `map_artifacts` manifest, which the
# `check-maps-fresh` lint reads. The freshness stamp is only recorded if the run
# did not fail.
#
# Requires: ~/b/gwebcdb checkout (per-agent Chrome allocator) + a healthy WSLg
#           Chrome; wrangler authenticated; curl (for Nominatim); ./bin/chromeport
#           db query/exec. The script acquires/releases its own Chrome — do NOT
#           pre-start one or run start-chrome-cdp-wslg.sh.
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
MIN_PNG_BYTES=200            # anything smaller is a failed/garbage capture
NOMINATIM="https://nominatim.openstreetmap.org/search"
UA="travel-2026-snapshot-maps/1.0 (keyless route geocoder; contact: yanggf)"
# Geocode context appended to every route-place query to disambiguate (the trip
# is in Naha, Okinawa — without this, "おもろまち"/"イオン那覇" geocode poorly).
GEO_CONTEXT="Naha, Okinawa, Japan"
# Per-day polyline colors for the overview (Leaflet-friendly, high-contrast).
DAY_COLORS=( "#e6194b" "#3cb44b" "#4363d8" "#f58231" "#911eb4" "#008080" "#9a6324" )

# Clear stale outputs so a previous run's PNGs can't be re-uploaded (C3).
rm -rf "$OUT"; mkdir -p "$OUT"

FAILED=0   # set if any required capture/upload fails → suppress the freshness stamp
declare -a MANIFEST_KEYS=()   # keys we wrote a manifest row for

# --- acquire an ISOLATED, PERSISTENT Chrome via the gwebcdb per-agent allocator ---
# The harness reaps any process backgrounded inside a Bash call, so we CANNOT start
# our own long-lived Chrome (nohup/setsid/disown all die when the call returns). The
# allocator launches Chrome detached from Python (start_new_session=True) so it
# survives across this script's many chromeport subprocess calls. Chrome picks its own
# port; we point chromeport (a CDP *client*) at it via CHROMEPORT_CDP_ENDPOINT — no
# more hardcoded :9222, no shared-tab collisions with another agent's Chrome.
GWEBCDB="${GWEBCDB_DIR:-$HOME/b/gwebcdb}"
echo "== acquire isolated Chrome session (gwebcdb) =="
SESSION_OUT="$(cd "$GWEBCDB" && timeout 45 python bridge/chrome_session.py acquire 2>&1)" || {
  echo "ERROR: chrome_session.py acquire failed:"; printf '%s\n' "$SESSION_OUT"; exit 1; }
CDP_PORT="$(printf '%s\n' "$SESSION_OUT" | sed -n 's/^port'$'\t''//p')"
[ -n "$CDP_PORT" ] || { echo "ERROR: could not read port from acquire output:"; printf '%s\n' "$SESSION_OUT"; exit 1; }
export CHROMEPORT_CDP_ENDPOINT="http://127.0.0.1:${CDP_PORT}"
echo "   chrome on ${CHROMEPORT_CDP_ENDPOINT} (isolated profile; released on exit)"

# Always release the session on exit (success, failure, or interrupt) so we never
# leak a detached Chrome. release is idempotent / safe if the session is already gone.
cleanup() { (cd "$GWEBCDB" && python bridge/chrome_session.py release --port "$CDP_PORT" >/dev/null 2>&1) || true; }
trap cleanup EXIT

echo "== chromeport / Chrome reachability =="
$CHROMEPORT browser doctor >/dev/null || { echo "Chrome not reachable at ${CHROMEPORT_CDP_ENDPOINT}"; exit 1; }

now_iso() { date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo "1970-01-01T00:00:00Z"; }
sql_escape() { printf '%s' "$1" | sed "s/'/''/g"; }

# Record one map_artifacts manifest row (status: uploaded|skipped|failed).
record_artifact() {
  local key="$1" status="$2" size="${3:-0}" reason="${4:-}"
  local p; p="$(sql_escape "$PLAN")"
  local k; k="$(sql_escape "$key")"
  local r; r="$(sql_escape "$reason")"
  local ts; ts="$(now_iso)"
  $CHROMEPORT db exec "INSERT INTO map_artifacts (plan_id, map_key, byte_size, sha256, status, skip_reason, generated_at) \
    VALUES ('$p','$k',$size,NULL,'$status',$( [ -z "$r" ] && echo NULL || echo "'$r'" ),'$ts') \
    ON CONFLICT(plan_id, map_key) DO UPDATE SET byte_size=excluded.byte_size, status=excluded.status, \
      skip_reason=excluded.skip_reason, generated_at=excluded.generated_at" >/dev/null 2>&1 \
    || echo "   warn: could not write manifest row for $key"
  MANIFEST_KEYS+=("$key")
}

# --- geocode a route place name → "lat,lon" (cached in route_place_geocodes) ---
# Echoes "lat,lon" on success, nothing on failure. Honors Nominatim ≤1 req/s.
geocode_place() {
  local place="$1"
  [ -z "$place" ] && return 0
  # normalized cache key (lowercased, trimmed, context-appended)
  local key; key="$(printf '%s' "$place" | tr 'A-Z' 'a-z' | sed 's/^ *//;s/ *$//')|${GEO_CONTEXT}"
  local kq; kq="$(sql_escape "$key")"

  # 1. cache hit?
  local cached
  cached="$($CHROMEPORT db query "SELECT lat, lon, review FROM route_place_geocodes \
    WHERE query_key='$kq'" 2>/dev/null | awk -F'\t' 'NR>1 && $1 ~ /^-?[0-9]+\.?[0-9]*$/ {print $1","$2; exit}')"
  if [ -n "$cached" ]; then printf '%s' "$cached"; return 0; fi
  # cached failure (row exists, lat empty) → don't re-hit
  local known_fail
  known_fail="$($CHROMEPORT db query "SELECT 1 FROM route_place_geocodes WHERE query_key='$kq' AND lat IS NULL" 2>/dev/null | awk 'NR>1{print;exit}')"
  [ -n "$known_fail" ] && return 0

  # 2. live Nominatim (rate-limited, identifying UA, JSON; context-disambiguated)
  sleep 1.1
  local q; q="${place}, ${GEO_CONTEXT}"
  local resp
  resp="$(curl -s --max-time 20 -H "User-Agent: ${UA}" \
    --data-urlencode "q=${q}" --data-urlencode "format=jsonv2" \
    --data-urlencode "limit=1" --data-urlencode "addressdetails=0" \
    -G "$NOMINATIM" 2>/dev/null || true)"
  local lat lon disp osmid osmtype
  lat="$(printf '%s' "$resp" | grep -oE '"lat":"[^"]*"' | head -1 | sed 's/.*:"//;s/"//')"
  lon="$(printf '%s' "$resp" | grep -oE '"lon":"[^"]*"' | head -1 | sed 's/.*:"//;s/"//')"
  disp="$(printf '%s' "$resp" | grep -oE '"display_name":"[^"]*"' | head -1 | sed 's/.*:"//;s/"$//')"
  osmid="$(printf '%s' "$resp" | grep -oE '"osm_id":[0-9]*' | head -1 | sed 's/.*://')"
  osmtype="$(printf '%s' "$resp" | grep -oE '"osm_type":"[^"]*"' | head -1 | sed 's/.*:"//;s/"//')"
  local ts; ts="$(now_iso)"
  local rp; rp="$(sql_escape "$place")"
  local dq; dq="$(sql_escape "$disp")"

  if [ -n "$lat" ] && [ -n "$lon" ]; then
    $CHROMEPORT db exec "INSERT INTO route_place_geocodes \
      (query_key, raw_place, lat, lon, display_name, osm_id, osm_type, provider, confidence, review, failure_reason, fetched_at) \
      VALUES ('$kq','$rp',$lat,$lon,'$dq','${osmid}','${osmtype}','nominatim','ok',0,NULL,'$ts') \
      ON CONFLICT(query_key) DO UPDATE SET lat=excluded.lat, lon=excluded.lon, display_name=excluded.display_name, fetched_at=excluded.fetched_at" \
      >/dev/null 2>&1 || true
    printf '%s,%s' "$lat" "$lon"
  else
    # cache the miss so we don't re-hit Nominatim for the same unresolved place
    $CHROMEPORT db exec "INSERT INTO route_place_geocodes \
      (query_key, raw_place, lat, lon, display_name, osm_id, osm_type, provider, confidence, review, failure_reason, fetched_at) \
      VALUES ('$kq','$rp',NULL,NULL,NULL,NULL,NULL,'nominatim',NULL,0,'no_result','$ts') \
      ON CONFLICT(query_key) DO UPDATE SET failure_reason='no_result', fetched_at=excluded.fetched_at" \
      >/dev/null 2>&1 || true
    return 0
  fi
}

# --- collect ORDERED stop coords for a day, from POIs first, else route places ---
# Writes "lat,lon" lines to stdout in itinerary order. Falls back to geocoding the
# day_route_segments place sequence (from_place of seg 0, then each to_place).
day_coords() {
  local d="$1"
  # 1. sightseeing POIs linked to this day's activities (already geocoded)
  local poi
  poi="$($CHROMEPORT db query "SELECT p.lat, p.lon \
    FROM activities a JOIN destination_pois p \
      ON (p.poi_id = a.poi_id OR (a.poi_id IS NULL AND p.title = a.title)) AND p.slug='${DEST}' \
    WHERE a.plan_id='${PLAN}' AND a.day_number=${d} AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
    ORDER BY a.sort_order" 2>/dev/null \
    | awk -F'\t' 'NR>1 && $1 ~ /^-?[0-9]+\.?[0-9]*$/ && $2 ~ /^-?[0-9]+\.?[0-9]*$/ {print $1","$2}')"
  if [ -n "$poi" ]; then printf '%s\n' "$poi"; return 0; fi

  # 2. no POI stops → build the route from day_route_segments place names (geocoded).
  # Reject the chromeport "(N rows)" footer: a real segment row has a numeric
  # sort_order in $1; the footer line does not.
  local seg_places
  seg_places="$($CHROMEPORT db query "SELECT sort_order, from_place, to_place \
    FROM day_route_segments WHERE plan_id='${PLAN}' AND day_number=${d} ORDER BY sort_order" 2>/dev/null \
    | awk -F'\t' 'NR>1 && $1 ~ /^[0-9]+$/ {print $2"\n"$3}')"   # from + to of each segment, in order
  [ -z "$seg_places" ] && return 0
  # de-dup consecutive repeats (to_place of seg i == from_place of seg i+1)
  local prev="" place coord
  printf '%s\n' "$seg_places" | while IFS= read -r place; do
    [ -z "$place" ] && continue
    [ "$place" = "$prev" ] && continue
    prev="$place"
    coord="$(geocode_place "$place")"
    [ -n "$coord" ] && printf '%s\n' "$coord"
  done
}

# --- build a Leaflet HTML from labeled polylines + screenshot it ---
# stdin: lines "lat,lon,COLOR" (markers, in order). For per-day maps all one
# color; for the overview, each day's segment carries its day color. A polyline
# is drawn per contiguous same-color run (so days aren't chained together).
render_map() {
  local name="$1"
  local rows; rows="$(cat)"
  [ -z "$rows" ] && { echo "   skip ${name}: no points"; return 1; }

  # JS: pts = [[lat,lon,"color"],...]
  local arr; arr="$(printf '%s\n' "$rows" | awk -F',' 'NF>=2{c=($3==""?"#e23":$3); printf "[%s,%s,\"%s\"],",$1,$2,c}')"
  arr="[${arr%,}]"

  local html="${OUT}/${name}.html"
  cat > "$html" <<HTML
<!doctype html><html><head><meta charset="utf-8">
<link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css">
<script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
<style>
  html,body{margin:0;padding:0;height:100%}
  #map{position:absolute;inset:0;background:#eef}
  .leaflet-control-attribution,.leaflet-control-zoom{display:none}
  /* Keyless tile attribution is burned in below (.credit) since the Leaflet
     attribution control is disabled — CARTO/OSM tiles require credit. */
  .credit{position:absolute;right:3px;bottom:2px;z-index:1000;font:10px/13px sans-serif;
    color:#555;background:rgba(255,255,255,.7);padding:0 4px;border-radius:3px}
</style></head><body><div id="map"></div>
<div class="credit">© OpenStreetMap, © CARTO</div><script>
  var pts = ${arr};
  var ll = pts.map(function(p){return [p[0],p[1]];});
  var map = L.map('map',{zoomControl:false,attributionControl:false});
  // CARTO Positron — keyless, muted basemap so route + pins read clearly (no API key).
  L.tileLayer('https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}{r}.png',{subdomains:'abcd',maxZoom:20}).addTo(map);
  L.control.scale({imperial:false,position:'bottomleft'}).addTo(map);
  if (ll.length === 1) { map.setView(ll[0], 14); } else { map.fitBounds(ll, {padding:[45,45]}); }
  // polyline per contiguous same-color run (each day = its own line; no chaining).
  // White casing under a dashed colored line: reads as a planned connector, not a road.
  var run=[], runColor=null;
  function flush(){
    if(run.length>1){
      L.polyline(run,{color:'#fff',weight:7,opacity:.7}).addTo(map);                  // casing
      L.polyline(run,{color:runColor,weight:4,opacity:.9,dashArray:'6,8'}).addTo(map); // dashed route
    }
    run=[];
  }
  pts.forEach(function(p){ var c=p[2]; if(c!==runColor){flush(); runColor=c;} run.push([p[0],p[1]]); });
  flush();
  // numbered teardrop pins (global order), day-colored, anchored at the TIP (bottom-center)
  pts.forEach(function(p,i){
    var c=p[2];
    var svg='<svg width="28" height="40" viewBox="0 0 28 40" xmlns="http://www.w3.org/2000/svg">'+
      '<path d="M14 39 C5 26 1 20 1 13 A13 13 0 0 1 27 13 C27 20 23 26 14 39 Z" '+
      'fill="'+c+'" stroke="#fff" stroke-width="2"/>'+
      '<text x="14" y="18" text-anchor="middle" font-family="sans-serif" font-size="13" '+
      'font-weight="700" fill="#fff">'+(i+1)+'</text></svg>';
    L.marker([p[0],p[1]],{icon:L.divIcon({className:'',html:svg,
      iconSize:[28,40],iconAnchor:[14,40]})}).addTo(map);
  });
  setTimeout(function(){document.title='MAP_READY';}, 1200);
</script></body></html>
HTML

  local data_url
  data_url="$(python3 -c 'import urllib.parse,sys; print("data:text/html,"+urllib.parse.quote(open(sys.argv[1]).read()))' "$html")"
  $CHROMEPORT screenshot "$data_url" --out "${OUT}/${name}.png" \
    --width 640 --height 440 --wait 9000 2>&1 | grep -E 'screenshot|error' || true
  sleep 1
  # FAIL-LOUD: a real PNG must exist, start with the PNG magic, and exceed the
  # min size. Otherwise remove the stub so it can never be uploaded.
  local f="${OUT}/${name}.png"
  if [ ! -s "$f" ]; then echo "   FAIL ${name}: no screenshot produced"; rm -f "$f"; return 1; fi
  local sz; sz=$(wc -c < "$f")
  local magic; magic=$(head -c 4 "$f" | xxd -p 2>/dev/null)
  if [ "$magic" != "89504e47" ] || [ "$sz" -lt "$MIN_PNG_BYTES" ]; then
    echo "   FAIL ${name}: invalid/undersized PNG (${sz}B, magic=${magic})"; rm -f "$f"; return 1
  fi
  echo "   ok ${name}: ${sz}B"
  return 0
}

# Render a day map, upload on success, and record the manifest row either way.
process_day() {
  local d="$1" key="day-${d}.png" color="${DAY_COLORS[$(( (d-1) % ${#DAY_COLORS[@]} ))]}"
  local coords; coords="$(day_coords "$d")"
  if [ -z "$coords" ]; then
    echo "   skip day-${d}: no mappable stops (no POI link, no geocodable route place)"
    record_artifact "$key" "skipped" 0 "no mappable stops"
    return 0
  fi
  # tag each coord with the day color for render_map
  if printf '%s\n' "$coords" | awk -F',' -v c="$color" 'NF>=2{print $1","$2","c}' | render_map "day-${d}"; then
    upload_and_record "$key" "${OUT}/day-${d}.png"
  else
    record_artifact "$key" "failed" 0 "screenshot failed"; FAILED=1
  fi
}

upload_and_record() {
  local key="$1" file="$2"
  local sz; sz=$(wc -c < "$file")
  unset CLOUDFLARE_API_TOKEN
  if npx wrangler r2 object put "${BUCKET}/${PLAN}/${key}" --file "$file" --content-type image/png --remote 2>&1 | grep -iqE 'upload|creating'; then
    echo "   uploaded ${key} (${sz}B)"
    record_artifact "$key" "uploaded" "$sz" ""
  else
    echo "   FAIL upload ${key}"
    record_artifact "$key" "failed" "$sz" "upload failed"; FAILED=1
  fi
}

# --- enumerate the plan's days (so we record skipped rows for un-mappable days too) ---
ALL_DAYS=$($CHROMEPORT db query "SELECT day_number FROM days WHERE plan_id='${PLAN}' ORDER BY day_number" 2>/dev/null \
  | awk -F'\t' 'NR>1 && $1 ~ /^[0-9]+$/ {print $1}')
if [ -z "$ALL_DAYS" ]; then echo "ERROR: no days for ${PLAN}" >&2; exit 1; fi
echo "   plan days: $(printf '%s' "$ALL_DAYS" | tr '\n' ' ')"

echo "== render per-day route maps =="
for d in $ALL_DAYS; do process_day "$d"; done

echo "== render plan overview (each day its own colored route) =="
# Build the overview from every day's coords, each tagged with that day's color.
OVERVIEW="$(for d in $ALL_DAYS; do
  color="${DAY_COLORS[$(( (d-1) % ${#DAY_COLORS[@]} ))]}"
  day_coords "$d" | awk -F',' -v c="$color" 'NF>=2{print $1","$2","c}'
done)"
if [ -n "$OVERVIEW" ] && printf '%s\n' "$OVERVIEW" | render_map "plan"; then
  upload_and_record "plan.png" "${OUT}/plan.png"
else
  echo "   skip plan.png: no mappable stops across any day"
  record_artifact "plan.png" "skipped" 0 "no mappable stops"
fi

echo "== record snapshot timestamp =="
if [ "$FAILED" -eq 0 ]; then
  "$TRAVEL" mark-maps-snapshotted "$PLAN" || echo "warning: could not record snapshot timestamp"
else
  echo "   NOT stamping freshness — one or more maps failed (see manifest / check-maps-fresh)."
fi

echo "== done: maps + manifest at ${BUCKET}/${PLAN}/ — run: ./bin/travel check-maps-fresh --plan-id ${PLAN} =="
