#!/usr/bin/env bash
# snapshot-maps.sh — render per-day + plan ROUTE maps (numbered markers + a
# connecting polyline, auto-framed) and upload the PNGs to the R2 bucket the
# dashboard worker serves from.
#
# Keyless: a self-contained Leaflet page (CARTO Positron basemap — © OpenStreetMap,
# © CARTO; Leaflet from unpkg CDN) is
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
# OSRM public demo — keyless road-routing (Tier 2). No SLA / rate limits / key;
# driving profile only (see plan). Every call fails SOFT to a straight line and is
# cached per-leg in Turso (route_road_legs/_points) so a re-run makes zero calls.
OSRM_BASE="https://router.project-osrm.org/route/v1/driving"
OSRM_PROFILE="driving"
OSRM_PROVIDER="osrm-demo"
ROAD_DP=5                     # coord precision for the leg cache key (~1.1m)
# Geocode context appended to every route-place query to disambiguate (the trip
# is in Naha, Okinawa — without this, "おもろまち"/"イオン那覇" geocode poorly).
GEO_CONTEXT="Naha, Okinawa, Japan"
# Per-day polyline colors for the overview (Leaflet-friendly, high-contrast).
DAY_COLORS=( "#e6194b" "#3cb44b" "#4363d8" "#f58231" "#911eb4" "#008080" "#9a6324" )

# Clear stale outputs so a previous run's PNGs can't be re-uploaded (C3).
rm -rf "$OUT"; mkdir -p "$OUT"

FAILED=0   # set if any required capture/upload fails → suppress the freshness stamp
declare -a MANIFEST_KEYS=()   # keys we wrote a manifest row for
# Per-day coords computed ONCE in the per-day loop and reused for the overview, so
# the overview doesn't re-run every day's POI/segment SELECTs (+ a cache lookup per
# place) through fresh chromeport subprocesses. Key = day number; value = the
# newline-joined "lat,lon" lines (empty string for an un-mappable day).
declare -A DAY_COORDS=()
# Per-day road geometry (OSRM), computed once in the per-day loop and reused for the
# overview so the overview makes ZERO extra OSRM calls. Value = "lat,lon lat,lon ..."
# (empty = no road geometry → straight-line fallback for that day).
declare -A DAY_ROADS=()

# --- acquire an ISOLATED, PERSISTENT Chrome via the gwebcdb per-agent allocator ---
# The harness reaps any process backgrounded inside a Bash call, so we CANNOT start
# our own long-lived Chrome (nohup/setsid/disown all die when the call returns). The
# allocator launches Chrome detached from Python (start_new_session=True) so it
# survives across this script's many chromeport subprocess calls. Chrome picks its own
# port; we point chromeport (a CDP *client*) at it via CHROMEPORT_CDP_ENDPOINT — no
# more hardcoded :9222, no shared-tab collisions with another agent's Chrome.
GWEBCDB="${GWEBCDB_DIR:-$HOME/b/gwebcdb}"
# Fail with a CLEAR message if the gwebcdb allocator checkout is missing, rather than
# a generic "acquire failed" that points the operator at Chrome/CDP. (.env + gwebcdb
# are provisioned out-of-band per CLAUDE.md.)
[ -f "$GWEBCDB/bridge/chrome_session.py" ] || {
  echo "ERROR: gwebcdb checkout not found at '$GWEBCDB' (need bridge/chrome_session.py)."
  echo "       Set GWEBCDB_DIR or clone ~/b/gwebcdb. This script self-acquires its Chrome from it."; exit 1; }
echo "== acquire isolated Chrome session (gwebcdb) =="
# python3 throughout (the rest of this script uses python3; a host with only python3
# and no `python` alias would otherwise fail at this first step only).
SESSION_OUT="$(cd "$GWEBCDB" && timeout 45 python3 bridge/chrome_session.py acquire 2>&1)" || {
  echo "ERROR: chrome_session.py acquire failed:"; printf '%s\n' "$SESSION_OUT"; exit 1; }
# acquire has now launched a detached Chrome + written session.json. Arm the release
# trap IMMEDIATELY — BEFORE parsing the port — so an early exit (e.g. a malformed
# port line below) can't leak the Chrome we just acquired. cleanup releases by
# whichever id we managed to parse (session name preferred; port as fallback), warns
# (does not silently swallow) if release fails, and is bounded by a timeout so a hung
# release can't wedge the EXIT trap (and thus the whole script's exit) forever.
SESSION_NAME="$(printf '%s\n' "$SESSION_OUT" | sed -n 's/^session'$'\t''//p')"
CDP_PORT="$(printf '%s\n' "$SESSION_OUT" | sed -n 's/^port'$'\t''//p')"
cleanup() {
  local sel=""
  if [ -n "${SESSION_NAME:-}" ]; then sel="--session $SESSION_NAME"
  elif [ -n "${CDP_PORT:-}" ]; then sel="--port $CDP_PORT"
  else return 0; fi
  (cd "$GWEBCDB" && timeout 20 python3 bridge/chrome_session.py release $sel >/dev/null 2>&1) \
    || echo "   WARN: could not release Chrome session ($sel) — check: cd $GWEBCDB && python3 bridge/chrome_session.py list" >&2
}
trap cleanup EXIT
[ -n "$CDP_PORT" ] || { echo "ERROR: could not read port from acquire output:"; printf '%s\n' "$SESSION_OUT"; exit 1; }
export CHROMEPORT_CDP_ENDPOINT="http://127.0.0.1:${CDP_PORT}"
echo "   chrome on ${CHROMEPORT_CDP_ENDPOINT} (isolated profile; released on exit)"

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

# --- OSRM road geometry for ONE leg (from "lat,lon" → to "lat,lon") -----------------
# Echoes a space-separated "lat,lon lat,lon ..." road polyline on success, NOTHING on
# any failure (fail-soft → caller draws a straight connector for that color). Cached
# per-leg in Turso so a re-run makes zero OSRM calls; failures are cached too so we
# don't re-hit. NEVER sets FAILED / touches the PNG path — routing is best-effort.
road_leg() {
  local from="$1" to="$2"
  local flat="${from%,*}" flon="${from#*,}" tlat="${to%,*}" tlon="${to#*,}"
  [ -n "$flat" ] && [ -n "$flon" ] && [ -n "$tlat" ] && [ -n "$tlon" ] || return 0
  # canonicalized leg key (fixed precision + provider + profile)
  local key; key="$(printf '%.*f,%.*f>%.*f,%.*f|%s|%s' \
    "$ROAD_DP" "$flat" "$ROAD_DP" "$flon" "$ROAD_DP" "$tlat" "$ROAD_DP" "$tlon" \
    "$OSRM_PROVIDER" "$OSRM_PROFILE" 2>/dev/null)"
  [ -n "$key" ] || return 0
  local kq; kq="$(sql_escape "$key")"

  # 1. cache hit? (status ok → return stored points; cached failure → return nothing)
  local st
  st="$($CHROMEPORT db query "SELECT status FROM route_road_legs WHERE leg_key='$kq'" 2>/dev/null \
    | awk -F'\t' 'NR>1 && ($1=="ok"||$1=="error"){print $1; exit}')"
  if [ "$st" = "ok" ]; then
    $CHROMEPORT db query "SELECT lat, lon FROM route_road_leg_points WHERE leg_key='$kq' ORDER BY point_order" 2>/dev/null \
      | awk -F'\t' 'NR>1 && $1 ~ /^-?[0-9]+\.?[0-9]*$/ && $2 ~ /^-?[0-9]+\.?[0-9]*$/ {printf "%s%s,%s",(n++?" ":""),$1,$2} END{if(n)print ""}'
    return 0
  fi
  [ -n "$st" ] && return 0   # cached error → fail-soft, don't re-hit

  # 2. live OSRM (rate-limited, identifying UA; OSRM wants lon,lat;lon,lat)
  sleep 0.8
  local ts; ts="$(now_iso)"
  local resp pts
  resp="$(curl -s --max-time 15 -H "User-Agent: ${UA}" \
    "${OSRM_BASE}/${flon},${flat};${tlon},${tlat}?overview=full&geometries=geojson" 2>/dev/null || true)"
  # parse GeoJSON coordinates [lon,lat] → "lat,lon lat,lon ..." with a JSON parser.
  # Line 1 = space-joined "lat,lon" vertices; line 2 = route distance in metres (or
  # empty). Both on STDOUT so neither the geometry nor the distance is discarded.
  local parsed
  parsed="$(printf '%s' "$resp" | python3 -c '
import sys,json
try:
    d=json.load(sys.stdin)
    if d.get("code")=="Ok" and d.get("routes"):
        r=d["routes"][0]
        print(" ".join(f"{lat},{lon}" for lon,lat in r["geometry"]["coordinates"]))
        print(r.get("distance",""))
except Exception:
    pass
' 2>/dev/null)"
  pts="$(printf '%s\n' "$parsed" | sed -n '1p')"
  local dist; dist="$(printf '%s\n' "$parsed" | sed -n '2p')"

  if [ -n "$pts" ]; then
    # Build the child-point VALUES list keying the comma + point_order on the EMITTED
    # count (m), not NR — so a malformed/blank vertex can't produce a leading comma or
    # a point_order gap. Only numeric lat,lon rows are emitted. END{print m} on stderr
    # gives the SAME count we store (single source of truth for point_count).
    local values pcount
    values="$(printf '%s' "$pts" | tr ' ' '\n' | awk -F',' -v k="$kq" '
      $1 ~ /^-?[0-9]+\.?[0-9]*$/ && $2 ~ /^-?[0-9]+\.?[0-9]*$/ {
        printf "%s(\x27%s\x27,%d,%s,%s)",(m++?",":""),k,m-1,$1,$2 }
      END{ print m+0 > "/dev/stderr" }' 2>/tmp/.road_m_$$)"
    pcount="$(cat /tmp/.road_m_$$ 2>/dev/null || echo 0)"; rm -f /tmp/.road_m_$$
    [ -n "$pcount" ] || pcount=0
    # distance_m: numeric or NULL (never interpolate a non-numeric token into SQL)
    local dsql="NULL"; case "$dist" in ''|*[!0-9.]*) dsql="NULL";; *) dsql="$dist";; esac
    # store header (point_count + distance from the SAME parse, no second pass)
    $CHROMEPORT db exec "INSERT INTO route_road_legs \
      (leg_key, from_lat, from_lon, to_lat, to_lon, provider, profile, status, point_count, distance_m, failure_reason, fetched_at) \
      VALUES ('$kq',$flat,$flon,$tlat,$tlon,'$OSRM_PROVIDER','$OSRM_PROFILE','ok',$pcount,$dsql,NULL,'$ts') \
      ON CONFLICT(leg_key) DO UPDATE SET status='ok', point_count=$pcount, distance_m=$dsql, failure_reason=NULL, fetched_at=excluded.fetched_at" \
      >/dev/null 2>&1 || true
    # rewrite child points in ONE batched multi-row INSERT (not 382 subprocess calls)
    $CHROMEPORT db exec "DELETE FROM route_road_leg_points WHERE leg_key='$kq'" >/dev/null 2>&1 || true
    [ -n "$values" ] && $CHROMEPORT db exec \
      "INSERT OR REPLACE INTO route_road_leg_points (leg_key, point_order, lat, lon) VALUES $values" \
      >/dev/null 2>&1 || true
    printf '%s\n' "$pts"
  else
    # cache the failure so we don't re-hit; emit nothing → straight-line fallback
    $CHROMEPORT db exec "INSERT INTO route_road_legs \
      (leg_key, from_lat, from_lon, to_lat, to_lon, provider, profile, status, point_count, distance_m, failure_reason, fetched_at) \
      VALUES ('$kq',$flat,$flon,$tlat,$tlon,'$OSRM_PROVIDER','$OSRM_PROFILE','error',0,NULL,'no_osrm_route','$ts') \
      ON CONFLICT(leg_key) DO UPDATE SET status='error', failure_reason='no_osrm_route', fetched_at=excluded.fetched_at" \
      >/dev/null 2>&1 || true
    return 0
  fi
}

# --- road geometry for a whole DAY, emitted as ONE routes-file line PER LEG ---------
# $1 = COLOR for this day. stdin = ordered "lat,lon" lines. stdout = one line per
# consecutive stop pair:
#   "COLOR<TAB>road<TAB>lat,lon lat,lon ..."     (leg routed → solid road polyline), or
#   "COLOR<TAB>straight<TAB>from_lat,from_lon to_lat,to_lon"  (leg unroutable → dashed)
# Per-LEG (not concatenated): a single unroutable middle hop is drawn as its OWN dashed
# connector instead of teleporting the road across the gap, and every leg gets a line.
# Fail-soft throughout (road_leg never errors; an empty leg → the straight variant).
road_geometry() {
  local color="$1"
  local coords; coords="$(cat)"
  [ -z "$coords" ] && return 0
  # walk consecutive stop pairs with a "previous" cursor — no array indexing, so it
  # is unambiguously safe under `set -u` even for 0/1-stop input.
  local prev="" cur leg
  while IFS= read -r cur; do
    [ -n "$cur" ] || continue
    if [ -n "$prev" ]; then
      leg="$(road_leg "$prev" "$cur")"
      if [ -n "$leg" ]; then
        printf '%s\troad\t%s\n' "$color" "$leg"
      else
        printf '%s\tstraight\t%s %s\n' "$color" "$prev" "$cur"
      fi
    fi
    prev="$cur"
  done <<< "$coords"
  return 0   # always success; "no road, only straights" is normal fail-soft
}

# --- build a Leaflet HTML from labeled polylines + screenshot it ---
# stdin: lines "lat,lon,COLOR" — the STOPS (numbered markers, in order). For per-day
# maps all one color; for the overview, each stop carries its day color.
# $2 (optional): path to a ROUTES file — one drawable line PER LEG, format
#   "COLOR<TAB>KIND<TAB>lat,lon lat,lon ..." where KIND is `road` (drawn SOLID cased)
#   or `straight` (drawn DASHED cased). When a routes file is present it is the COMPLETE
#   per-leg line set, so the per-color straight-line fallback below is used ONLY when no
#   routes file is given at all. Markers are ALWAYS drawn from the stops.
render_map() {
  local name="$1"
  local routes_file="${2:-}"
  local rows; rows="$(cat)"
  [ -z "$rows" ] && { echo "   skip ${name}: no points"; return 1; }

  # JS: pts = [[lat,lon,"color"],...]  (stops → markers + no-routes-file fallback line)
  local arr; arr="$(printf '%s\n' "$rows" | awk -F',' 'NF>=2{c=($3==""?"#e23":$3); printf "[%s,%s,\"%s\"],",$1,$2,c}')"
  arr="[${arr%,}]"

  # JS: roads = [["color","kind",[[lat,lon],...]],...]  — one entry per LEG. Each routes
  # line is "COLOR<TAB>KIND<TAB>lat,lon lat,lon ..."; only numeric vertices are kept.
  local roads="[]" have_routes=0
  if [ -n "$routes_file" ] && [ -s "$routes_file" ]; then
    have_routes=1
    roads="$(awk -F'\t' 'NF>=3{
      printf "[\"%s\",\"%s\",[",$1,$2;
      n=split($3,v," "); m=0;
      for(i=1;i<=n;i++){ split(v[i],c,",");
        if(c[1] ~ /^-?[0-9]+\.?[0-9]*$/ && c[2] ~ /^-?[0-9]+\.?[0-9]*$/){
          printf "%s[%s,%s]",(m++?",":""),c[1],c[2]; } }
      printf "]],";
    }' "$routes_file")"
    roads="[${roads%,}]"
  fi

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
  var roads = ${roads};        // [[color,kind,[[lat,lon],...]],...]  one entry per LEG
  var haveRoutes = ${have_routes};
  var ll = pts.map(function(p){return [p[0],p[1]];});
  document.title = 'MAP_LOADING';
  var map = L.map('map',{zoomControl:false,attributionControl:false});
  L.control.scale({imperial:false,position:'bottomleft'}).addTo(map);

  // --- tile-load readiness: signal MAP_READY only AFTER the visible CARTO tiles have
  // actually loaded + decoded + painted (not on a blind timer), so the screenshot can
  // never fire on a blank/half-painted basemap. tileerror / timeout → MAP_FAILED.
  var readyDone = false, tileFailed = 0;
  function finishMap(state, reason){ if(readyDone) return; readyDone=true;
    document.title = state + (reason ? ':' + reason : ''); }
  function visibleLoadedCartoTiles(){
    var box = map.getContainer().getBoundingClientRect();
    return Array.prototype.slice.call(document.querySelectorAll('#map .leaflet-tile-loaded'))
      .filter(function(img){ var r=img.getBoundingClientRect();
        return img.complete && img.naturalWidth>0 && img.naturalHeight>0 &&
          img.src.indexOf('basemaps.cartocdn.com/light_all/')!==-1 &&
          r.width>0 && r.height>0 && r.right>box.left && r.left<box.right &&
          r.bottom>box.top && r.top<box.bottom; });
  }
  function markReadyAfterDecodeAndPaint(tiles){
    Promise.all(tiles.map(function(img){ return img.decode?img.decode().catch(function(){}):Promise.resolve(); }))
      .then(function(){ requestAnimationFrame(function(){ requestAnimationFrame(function(){
        finishMap('MAP_READY','tiles='+tiles.length); }); }); });
  }
  function waitForVisibleTiles(){
    var deadline = performance.now()+1500;
    (function poll(){
      if(readyDone) return;
      if(tileFailed>0){ finishMap('MAP_FAILED','tileerror='+tileFailed); return; }
      var tiles = visibleLoadedCartoTiles();
      if(tiles.length>0){ markReadyAfterDecodeAndPaint(tiles); return; }
      if(performance.now()<deadline) requestAnimationFrame(poll);
      else finishMap('MAP_FAILED','no-visible-loaded-tiles');
    })();
  }
  // CARTO Positron — keyless, muted basemap so route + pins read clearly (no API key).
  var baseLayer = L.tileLayer('https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}{r}.png',{subdomains:'abcd',maxZoom:20});
  baseLayer.on('tileerror', function(){ tileFailed++; });
  baseLayer.on('load', waitForVisibleTiles);
  // hard safety cap so a stuck tile can't hang the capture forever.
  setTimeout(function(){ finishMap('MAP_FAILED','tile-timeout'); }, 14000);

  // frame to include both stops AND road geometry FIRST (so tiles load for the final
  // view), THEN add the base layer so its 'load' reflects the framed viewport.
  var allll = ll.slice();
  roads.forEach(function(r){ r[2].forEach(function(pt){ allll.push(pt); }); });
  if (allll.length === 1) { map.setView(allll[0], 14); } else { map.fitBounds(allll, {padding:[45,45]}); }
  baseLayer.addTo(map);
  // Per-LEG lines from the routes file: kind 'road' → SOLID cased; kind 'straight' →
  // DASHED cased (an unroutable hop drawn as its own honest connector — no teleport, no
  // gap). Each leg is its own polyline, so a failed middle leg can't chain across a gap.
  roads.forEach(function(r){
    var color=r[0], kind=r[1], line=r[2];
    if (line.length < 2) return;
    L.polyline(line,{color:'#fff',weight:7,opacity:.7}).addTo(map);                     // casing
    if (kind==='road') {
      L.polyline(line,{color:color,weight:4,opacity:.9}).addTo(map);                    // solid road
    } else {
      L.polyline(line,{color:color,weight:4,opacity:.9,dashArray:'6,8'}).addTo(map);    // dashed connector
    }
  });
  // Fallback dashed straight line per contiguous same-color run — used ONLY when there
  // is no routes file at all (the routes file, when present, is the complete leg set).
  var run=[], runColor=null;
  function flush(){
    if(!haveRoutes && run.length>1){
      L.polyline(run,{color:'#fff',weight:7,opacity:.7}).addTo(map);                    // casing
      L.polyline(run,{color:runColor,weight:4,opacity:.9,dashArray:'6,8'}).addTo(map);  // dashed connector
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
</script></body></html>
HTML

  local data_url
  data_url="$(python3 -c 'import urllib.parse,sys; print("data:text/html,"+urllib.parse.quote(open(sys.argv[1]).read()))' "$html")"
  # --wait is now a MAX: chromeport returns as soon as the page sets MAP_READY (tiles
  # painted) and errors on MAP_FAILED. 20s > the page's 14s tile-timeout, so a slow-but-
  # loading map still captures instead of being cut off; a healthy map returns in ~1-2s.
  $CHROMEPORT screenshot "$data_url" --out "${OUT}/${name}.png" \
    --width 640 --height 440 --wait 20000 2>&1 | grep -E 'screenshot|error|ready' || true
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
  DAY_COORDS[$d]="$coords"   # cache for the overview (computed once here)
  if [ -z "$coords" ]; then
    echo "   skip day-${d}: no mappable stops (no POI link, no geocodable route place)"
    record_artifact "$key" "skipped" 0 "no mappable stops"
    return 0
  fi
  # road-follow the day's stops → one "COLOR<TAB>KIND<TAB>verts" routes line PER LEG
  # (road or straight). Fail-soft: unroutable legs come back as 'straight' lines.
  local road; road="$(printf '%s\n' "$coords" | road_geometry "$color")"
  DAY_ROADS[$d]="$road"   # cache the per-leg routes lines for the overview (no 2nd OSRM pass)
  local rf="${OUT}/day-${d}.routes"
  : > "$rf"
  [ -n "$road" ] && printf '%s\n' "$road" > "$rf"
  # tag each coord with the day color for render_map (stops = markers)
  if printf '%s\n' "$coords" | awk -F',' -v c="$color" 'NF>=2{print $1","$2","c}' | render_map "day-${d}" "$rf"; then
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
# Build the overview from each day's coords + road geometry, each tagged with that
# day's color. Reuses DAY_COORDS / DAY_ROADS cached by process_day above — no second
# round of per-day POI/segment/geocode queries OR OSRM calls.
PLAN_RF="${OUT}/plan.routes"; : > "$PLAN_RF"
OVERVIEW="$(for d in $ALL_DAYS; do
  [ -n "${DAY_COORDS[$d]:-}" ] || continue
  color="${DAY_COLORS[$(( (d-1) % ${#DAY_COLORS[@]} ))]}"
  # DAY_ROADS already holds complete color-tagged per-leg "COLOR<TAB>KIND<TAB>verts"
  # lines for this day — append them directly (no re-wrapping).
  [ -n "${DAY_ROADS[$d]:-}" ] && printf '%s\n' "${DAY_ROADS[$d]}" >> "$PLAN_RF"
  printf '%s\n' "${DAY_COORDS[$d]}" | awk -F',' -v c="$color" 'NF>=2{print $1","$2","c}'
done)"
if [ -n "$OVERVIEW" ] && printf '%s\n' "$OVERVIEW" | render_map "plan" "$PLAN_RF"; then
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
