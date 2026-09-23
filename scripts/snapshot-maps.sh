#!/usr/bin/env bash
# snapshot-maps.sh — render per-day + plan ROUTE maps (numbered markers + a
# connecting polyline, auto-framed) and upload the PNGs to the R2 bucket the
# dashboard worker serves from.
#
# Keyless: a self-contained Leaflet page (OpenStreetMap standard basemap — © OpenStreetMap
# contributors; Leaflet from unpkg CDN) is
# generated per day, screenshotted via chromeport (a CDP *client*) attached to an
# isolated Chrome this script ACQUIRES from the gwebcdb per-agent allocator (Chrome
# is launched detached so it persists across our subprocess calls; released on exit).
# No Google Maps key. Stops come from TWO sources, in itinerary order:
#   1. activities → destination_pois (sightseeing POIs already geocoded), and
#   2. day_route_segments place names (hotel/airport/restaurant/mall/district)
#      resolved to coords via a keyless Nominatim/OSM geocode CACHED in Turso
#      (route_place_geocodes) — so days with no sightseeing POI (e.g. arrival/
#      departure/shopping days) still get a real route map.
# The plan overview leaves hotels and airports out of its bounds so sightseeing
# pins stay readable. A separate plan-logistics map shows those hotel/airport points.
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
#   <plan-id>/plan.png, <plan-id>/plan-logistics.png, and <plan-id>/day-<n>.png
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
# Geocode context appended to every route-place query to disambiguate a bare place.
# A composite display name such as "Osaka + Kyoto" is too broad for Nominatim and can
# return unrelated coordinates; prefer an explicit override, then destination-specific
# city context, then destination_config.display_name.
GEO_CITY="$($CHROMEPORT db query "SELECT display_name FROM destination_config \
  WHERE slug='$(printf '%s' "$DEST" | sed "s/'/''/g")'" 2>/dev/null \
  | awk -F'\t' 'NR>1 && $1!="" {print $1; exit}')"
if [ -n "${MAPS_GEO_CONTEXT:-}" ]; then
  GEO_CONTEXT="$MAPS_GEO_CONTEXT"
elif [[ "${DEST,,}" == *kyoto* ]]; then
  GEO_CONTEXT="Kyoto, Japan"
elif [ -n "$GEO_CITY" ]; then
  GEO_CONTEXT="${GEO_CITY}, Japan"
else
  echo "WARN: no destination_config.display_name for '$DEST' — geocoding with bare 'Japan' context." >&2
  GEO_CONTEXT="Japan"
fi
echo "   geo-context: ${GEO_CONTEXT}"
# Per-day polyline colors for the overview (Leaflet-friendly, high-contrast).
DAY_COLORS=( "#e6194b" "#3cb44b" "#4363d8" "#f58231" "#911eb4" "#008080" "#9a6324" )

# Clear stale outputs so a previous run's PNGs can't be re-uploaded (C3).
rm -rf "$OUT"; mkdir -p "$OUT"

FAILED=0   # set if any required capture/upload fails → suppress the freshness stamp
declare -a MANIFEST_KEYS=()   # keys we wrote a manifest row for
# Per-day sightseeing coords (hotel/airport endpoints excluded), computed once in
# the per-day loop so the overview doesn't re-run every day's SELECTs. Key = day
# number; value = the newline-joined "lat,lon" lines (empty string for an
# un-mappable day).
declare -A DAY_OVERVIEW_COORDS=()

# --- use a supplied CDP endpoint, or acquire an isolated Chrome via gwebcdb ---
# Supplying CHROMEPORT_CDP_ENDPOINT lets operators use an already-running headless
# Chrome when WSLg launch is unavailable. The caller owns that browser's lifecycle.
# Otherwise the allocator launches a detached, isolated Chrome and we release it on exit.
if [ -n "${CHROMEPORT_CDP_ENDPOINT:-}" ]; then
  echo "== use supplied Chrome session =="
  echo "   chrome at ${CHROMEPORT_CDP_ENDPOINT} (caller-managed lifecycle)"
else
  GWEBCDB="${GWEBCDB_DIR:-$HOME/b/gwebcdb}"
  # Some existing checkouts keep the allocator under archive/bridge; recognize that
  # layout automatically while preferring the current root-level bridge when present.
  if [ ! -f "$GWEBCDB/bridge/chrome_session.py" ] && \
     [ -f "$GWEBCDB/archive/bridge/chrome_session.py" ]; then
    GWEBCDB="$GWEBCDB/archive"
  fi
  [ -f "$GWEBCDB/bridge/chrome_session.py" ] || {
    echo "ERROR: gwebcdb allocator not found under '$GWEBCDB' (need bridge/chrome_session.py)."
    echo "       Set GWEBCDB_DIR, or provide CHROMEPORT_CDP_ENDPOINT for an existing Chrome."; exit 1; }
  echo "== acquire isolated Chrome session (gwebcdb) =="
  SESSION_OUT="$(cd "$GWEBCDB" && timeout 45 python3 bridge/chrome_session.py acquire 2>&1)" || {
    echo "ERROR: chrome_session.py acquire failed:"; printf '%s\n' "$SESSION_OUT"; exit 1; }
  SESSION_NAME="$(printf '%s\n' "$SESSION_OUT" | sed -n 's/^session'$'\t''//p')"
  CDP_PORT="$(printf '%s\n' "$SESSION_OUT" | sed -n 's/^port'$'\t''//p')"
  cleanup() {
    local selector=""
    if [ -n "${SESSION_NAME:-}" ]; then
      selector="--session $SESSION_NAME"
      (cd "$GWEBCDB" && timeout 20 python3 bridge/chrome_session.py release --session "$SESSION_NAME" >/dev/null 2>&1) || {
        echo "   WARN: could not release Chrome session ($selector) — check: cd $GWEBCDB && python3 bridge/chrome_session.py list" >&2
      }
    elif [ -n "${CDP_PORT:-}" ]; then
      selector="--port $CDP_PORT"
      (cd "$GWEBCDB" && timeout 20 python3 bridge/chrome_session.py release --port "$CDP_PORT" >/dev/null 2>&1) || {
        echo "   WARN: could not release Chrome session ($selector) — check: cd $GWEBCDB && python3 bridge/chrome_session.py list" >&2
      }
    fi
  }
  trap cleanup EXIT
  [ -n "$CDP_PORT" ] || { echo "ERROR: could not read port from acquire output:"; printf '%s\n' "$SESSION_OUT"; exit 1; }
  export CHROMEPORT_CDP_ENDPOINT="http://127.0.0.1:${CDP_PORT}"
  echo "   chrome on ${CHROMEPORT_CDP_ENDPOINT} (isolated profile; released on exit)"
fi

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
  # Prefer the plan's already-reviewed POI coordinates over a fuzzy geocoder match.
  # Route labels often use a shorter Chinese/Japanese name than the linked activity.
  local place_sql poi_coord
  place_sql="$(sql_escape "$place")"
  poi_coord="$($CHROMEPORT db query "SELECT p.lat, p.lon FROM destination_pois p \
    WHERE p.slug='${DEST}' AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
      AND p.title LIKE '%${place_sql}%' LIMIT 1" 2>/dev/null \
    | awk -F'\t' 'NR>1 && $1 ~ /^-?[0-9]+\.?[0-9]*$/ && $2 ~ /^-?[0-9]+\.?[0-9]*$/ {print $1","$2; exit}')"
  if [ -n "$poi_coord" ]; then printf '%s' "$poi_coord"; return 0; fi

  # Normalize a few common itinerary labels before Nominatim. In particular, a
  # Chinese “京都站” search can resolve to the wrong station, while the English name
  # is unambiguous. Airport queries use the airport's own city, not the trip city.
  local search_place="$place" search_context="$GEO_CONTEXT"
  case "$place" in
    "KIX"|"KIX T1"|"KIX T2") search_place="Kansai International Airport"; search_context="Osaka, Japan" ;;
    "ITM") search_place="Osaka International Airport"; search_context="Osaka, Japan" ;;
    "TPE"|"TPE T1"|"TPE T2") search_place="Taiwan Taoyuan International Airport"; search_context="Taoyuan City, Taiwan" ;;
    "NRT") search_place="Narita International Airport"; search_context="Narita, Japan" ;;
    "HND") search_place="Haneda Airport"; search_context="Tokyo, Japan" ;;
    "京都站"|"京都駅") search_place="Kyoto Station Building"; search_context="Kyoto, Japan" ;;
    "HOTEL TAVINOS KYOTO") search_place="Hotel Tavinos Kyoto"; search_context="Kyoto, Japan" ;;
    "高台寺") search_place="Kodaiji Temple"; search_context="Kyoto, Japan" ;;
    "天龍寺") search_place="Tenryu-ji Temple"; search_context="Kyoto, Japan" ;;
    "竹林之道") search_place="Arashiyama Bamboo Grove"; search_context="Kyoto, Japan" ;;
    "祇園白川") search_place="Gion Shirakawa"; search_context="Kyoto, Japan" ;;
    "大原") search_place="Ohara"; search_context="Kyoto, Japan" ;;
    "嵐山站") search_place="Saga-Arashiyama Station"; search_context="Kyoto, Japan" ;;
    "京都拉麵小路") search_place="Kyoto Ramen Koji"; search_context="Kyoto, Japan" ;;
    "山城高雄バス停") search_place="Yamashiro Takao bus stop"; search_context="Kyoto, Japan" ;;
  esac
  # normalized cache key (lowercased, trimmed, context-appended)
  local key; key="$(printf '%s' "$search_place" | tr '[:upper:]' '[:lower:]' | sed 's/^ *//;s/ *$//')|${search_context}"
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
  local q; q="${search_place}, ${search_context}"
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

# Classify itinerary route labels for the plan overview split. Keep the daily route
# maps complete; only the plan-wide overview separates these far-away endpoints.
map_place_kind() {
  local lower="${1,,}"
  case "$lower" in
    *airport*|*機場*|*空港*|tpe|tpe\ t[0-9]|kix|kix\ t[0-9]|itm|nrt|hnd) printf 'airport' ;;
    *hotel*|*hostel*|*ryokan*|*旅館*|*飯店*|*民宿*|inn|*\ inn|*\ inn\ *) printf 'hotel' ;;
    *)
      if [[ "$1" =~ ^[A-Z]{3}([[:space:]]T[0-9])?$ ]]; then printf 'airport'; else printf 'other'; fi
      ;;
  esac
}

# --- collect ORDERED stop coords for a day, from route places first, else POIs ---
# Route segments include the day's full path (hotel/airport/transit endpoints as well
# as attractions); use them first so one linked POI cannot hide the rest of the day.
day_coords() {
  local d="$1" mode="${2:-all}"
  local seg_places
  seg_places="$($CHROMEPORT db query "SELECT sort_order, from_place, to_place \
    FROM day_route_segments WHERE plan_id='${PLAN}' AND day_number=${d} ORDER BY sort_order" 2>/dev/null \
    | awk -F'\t' 'NR>1 && $1 ~ /^[0-9]+$/ {print $2"\n"$3}')"
  if [ -n "$seg_places" ]; then
    local seg_coords
    seg_coords="$(printf '%s\n' "$seg_places" | awk 'NF && $0 != prev {print; prev=$0}' \
      | while IFS= read -r place; do
          local kind; kind="$(map_place_kind "$place")"
          if [ "$mode" = "overview" ] && [ "$kind" != "other" ]; then continue; fi
          if [ "$mode" = "logistics" ] && [ "$kind" = "other" ]; then continue; fi
          coord="$(geocode_place "$place")"
          local coord_key="$coord"
          [ "$mode" = "logistics" ] && coord_key="${coord},${kind}"
          if [ -n "$coord" ] && [ "$coord_key" != "${prev_coord:-}" ]; then
            if [ "$mode" = "logistics" ]; then printf '%s,%s\n' "$coord" "$kind"; else printf '%s\n' "$coord"; fi
            prev_coord="$coord_key"
          fi
        done)"
    if [ -n "$seg_coords" ]; then printf '%s\n' "$seg_coords"; return 0; fi
  fi

  # A missing route endpoint list is not evidence for a hotel/airport location.
  [ "$mode" = "logistics" ] && return 0

  # If route places are absent or none can be geocoded, use linked activity POIs.
  # Ordered by session then activity; sort_order restarts in each session.
  local poi
  poi="$($CHROMEPORT db query "SELECT p.lat, p.lon \
    FROM activities a JOIN destination_pois p \
      ON (p.poi_id = a.poi_id OR (a.poi_id IS NULL AND p.title = a.title)) AND p.slug='${DEST}' \
    WHERE a.plan_id='${PLAN}' AND a.day_number=${d} AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
    ORDER BY CASE a.session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 WHEN 'afternoon' THEN 2 WHEN 'evening' THEN 3 ELSE 4 END, a.sort_order" 2>/dev/null \
    | awk -F'\t' 'NR>1 && $1 ~ /^-?[0-9]+\.?[0-9]*$/ && $2 ~ /^-?[0-9]+\.?[0-9]*$/ {print $1","$2}')"
  if [ -n "$poi" ]; then
    printf '%s\n' "$poi" | awk '$0 != prev { print } { prev = $0 }'
  fi
  return 0
}

# Collect plan-level accommodation and airport points from normalized booking/flight
# rows, with route endpoints as a fallback for plans whose source tables are sparse.
plan_logistics_coords() {
  local p dest rows route_rows
  p="$(sql_escape "$PLAN")"
  dest="$(sql_escape "$DEST")"
  rows="$($CHROMEPORT db query "SELECT kind, place FROM (
    SELECT 'hotel' AS kind, name AS place FROM hotels WHERE plan_id='$p' AND destination='$dest'
    UNION
    SELECT 'airport' AS kind, COALESCE(NULLIF(departure_code,''), departure_airport) AS place \
      FROM flight_legs WHERE plan_id='$p' AND destination='$dest'
    UNION
    SELECT 'airport' AS kind, COALESCE(NULLIF(arrival_code,''), arrival_airport) AS place \
      FROM flight_legs WHERE plan_id='$p' AND destination='$dest'
  ) WHERE place IS NOT NULL AND TRIM(place) <> '' ORDER BY kind, place" 2>/dev/null \
    | awk -F'\t' 'NR>1 && $1!="" && $2!="" {print $1"\t"$2}')"
  route_rows="$($CHROMEPORT db query "SELECT from_place, to_place FROM day_route_segments \
    WHERE plan_id='$p' AND destination='$dest' ORDER BY day_number, sort_order" 2>/dev/null \
    | awk -F'\t' 'NR>1 {if($1!="") print "route\t"$1; if($2!="") print "route\t"$2}')"
  {
    [ -n "$rows" ] && printf '%s\n' "$rows"
    [ -n "$route_rows" ] && printf '%s\n' "$route_rows"
  } | awk -F'\t' '!seen[$1 FS $2]++' \
    | while IFS=$'\t' read -r kind place; do
        [ "$kind" = "route" ] && kind="$(map_place_kind "$place")"
        [ "$kind" != "other" ] || continue
        coord="$(geocode_place "$place")"
        [ -n "$coord" ] && printf '%s,%s\n' "$coord" "$kind"
      done | awk -F',' '{key=$1","$2","$3; if(!seen[key]++) print}'
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
    if [ -n "$values" ]; then
      $CHROMEPORT db exec \
        "INSERT OR REPLACE INTO route_road_leg_points (leg_key, point_order, lat, lon) VALUES $values" \
        >/dev/null 2>&1 || true
    fi
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
  local connect_stops=1 legend=""
  if [ "$name" = "plan-logistics" ]; then
    connect_stops=0
    legend='<div class="legend"><span class="airport"></span>機場 / Airport &nbsp; <span class="hotel"></span>住宿 / Hotel</div>'
  fi
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
  /* Mute the OSM basemap so the route polyline and numbered markers stay dominant
     (Positron used to give this for free). Markers/route sit in their own panes. */
  .leaflet-tile-pane{filter:saturate(0.45) brightness(1.06)}
  /* Keyless tile attribution is burned in below (.credit) since the Leaflet
     attribution control is disabled — OSM tiles require credit. */
  .credit{position:absolute;right:3px;bottom:2px;z-index:1000;font:10px/13px sans-serif;
    color:#555;background:rgba(255,255,255,.7);padding:0 4px;border-radius:3px}
  .legend{position:absolute;left:6px;top:6px;z-index:1000;font:12px/18px sans-serif;
    color:#222;background:rgba(255,255,255,.9);padding:4px 7px;border-radius:4px}
  .legend span{display:inline-block;width:10px;height:10px;border-radius:50%;margin:0 3px 0 5px}
  .legend .airport{background:#ef6c00}.legend .hotel{background:#1565c0}
</style></head><body><div id="map"></div>
${legend}
<div class="credit">© OpenStreetMap contributors</div><script>
  var pts = ${arr};
  var roads = ${roads};        // [[color,kind,[[lat,lon],...]],...]  one entry per LEG
  var haveRoutes = ${have_routes};
  var connectStops = ${connect_stops};
  var ll = pts.map(function(p){return [p[0],p[1]];});
  document.title = 'MAP_LOADING';
  var map = L.map('map',{zoomControl:false,attributionControl:false});
  L.control.scale({imperial:false,position:'bottomleft'}).addTo(map);

  // --- tile-load readiness: signal MAP_READY only AFTER the visible OSM tiles have
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
          img.src.indexOf('tile.openstreetmap.org/')!==-1 &&
          r.width>0 && r.height>0 && r.right>box.left && r.left<box.right &&
          r.bottom>box.top && r.top<box.bottom; });
  }
  function markReadyAfterDecodeAndPaint(tiles){
    // decode() rejections are tolerated INDIVIDUALLY: a tile that won't decode is
    // dropped from the painted set, but as long as >=1 visible tile decoded we proceed
    // (graceful degradation — a keyless CDN drops the odd tile; that's a gray gap, not
    // a blank map). Only finishOk if at least one tile actually decoded.
    var decoded = 0;
    Promise.all(tiles.map(function(img){
      return (img.decode?img.decode():Promise.resolve())
        .then(function(){ decoded++; }, function(){});
    })).then(function(){
      requestAnimationFrame(function(){ requestAnimationFrame(function(){
        if(decoded>0) finishMap('MAP_READY','tiles='+decoded);
        // else: leave readyDone false → the poll/timeout below re-evaluates.
      }); });
    });
  }
  function waitForVisibleTiles(){
    // Tolerant gate: succeed as soon as the framed viewport has visibly-painted OSM
    // tiles — even if SOME tiles errored (one bad tile must NOT drop an otherwise-good
    // map; the old blind-sleep captured those). We only fail-loud (MAP_FAILED) when the
    // 14s safety timeout elapses with STILL nothing painted (a truly blank basemap).
    var deadline = performance.now()+1500;
    (function poll(){
      if(readyDone) return;
      var tiles = visibleLoadedCartoTiles();
      if(tiles.length>0){ markReadyAfterDecodeAndPaint(tiles); return; }
      if(performance.now()<deadline) requestAnimationFrame(poll);
      // no visible tiles yet at +1500ms: keep waiting on the 'load'/timeout cycle
      // rather than failing — slow networks paint later; the 14s cap is the real floor.
    })();
  }
  // OpenStreetMap standard tiles — genuinely keyless. CARTO Positron was dropped
  // 2026-09-04: basemaps.cartocdn.com now stamps "API KEY REQUIRED" across every
  // keyless tile, so every snapshotted map rendered with that watermark burned in.
  // Desaturated via CSS below so the route line + numbered pins still read clearly.
  var baseLayer = L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png',{maxZoom:19});
  baseLayer.on('tileerror', function(){ tileFailed++; });   // counted (for diagnostics) but NOT fatal
  baseLayer.on('load', waitForVisibleTiles);
  // Safety cap (14s). On expiry: if ANY visible tile painted, capture it (degraded but
  // present); only MAP_FAILED if the basemap is genuinely blank. This is the sole
  // fail-loud path — a partial/slow map captures, a blank map is rejected.
  setTimeout(function(){
    if(readyDone) return;
    var tiles = visibleLoadedCartoTiles();
    if(tiles.length>0) finishMap('MAP_READY','tiles='+tiles.length+';timeout;err='+tileFailed);
    else finishMap('MAP_FAILED','blank-no-tiles;err='+tileFailed);
  }, 14000);

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
    if(connectStops && !haveRoutes && run.length>1){
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

  # Navigate to the generated local HTML directly. A data: URL expands every road
  # vertex into percent-encoding (the multi-day overview can be hundreds of KB),
  # making CDP navigation flaky or timing out before Chrome writes the screenshot.
  local page_url="file://${html}"
  # --wait is a MAX: chromeport returns as soon as the page sets MAP_READY and errors on
  # MAP_FAILED. The page's 14s tile-timeout is the real budget (it captures whatever has
  # painted by then, or fails only if the basemap is blank); --wait 20s just gives
  # chromeport headroom to OBSERVE that 14s outcome. A healthy map returns in ~1-2s.
  # Drop the `navigating` line: it contains the full data URL (the whole HTML page),
  # which otherwise floods CLI output and can drown out actual screenshot diagnostics.
  $CHROMEPORT screenshot "$page_url" --out "${OUT}/${name}.png" \
    --width 640 --height 440 --wait 20000 2>&1 \
    | sed '/^navigating[[:space:]]/d' | grep -iE 'screenshot|error|ready|failed' || true
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
  local d="$1" key color
  key="day-${d}.png"
  color="${DAY_COLORS[$(( (d-1) % ${#DAY_COLORS[@]} ))]}"
  local coords; coords="$(day_coords "$d")"
  DAY_OVERVIEW_COORDS[$d]="$(day_coords "$d" overview)"
  if [ -z "$coords" ]; then
    echo "   skip day-${d}: no mappable stops (no POI link, no geocodable route place)"
    record_artifact "$key" "skipped" 0 "no mappable stops"
    return 0
  fi
  # road-follow the day's stops → one "COLOR<TAB>KIND<TAB>verts" routes line PER LEG
  # (road or straight). Fail-soft: unroutable legs come back as 'straight' lines.
  local road; road="$(printf '%s\n' "$coords" | road_geometry "$color")"
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
  local node_major
  node_major="$(node -p 'Number(process.versions.node.split(".")[0])' 2>/dev/null || echo 0)"
  if [ "$node_major" -lt 22 ]; then
    local node22
    node22="$(find "$HOME/.nvm/versions/node" -mindepth 3 -maxdepth 3 -type f -path '*/bin/node' -print 2>/dev/null | sort -V | tail -1)"
    if [ -n "$node22" ]; then
      local node_bin_dir
      node_bin_dir="$(dirname "$node22")"
      export PATH="${node_bin_dir}:$PATH"
      node_major="$(node -p 'Number(process.versions.node.split(".")[0])' 2>/dev/null || echo 0)"
    fi
  fi
  if [ "$node_major" -lt 22 ]; then
    echo "   FAIL upload ${key}: Wrangler 4 requires Node.js 22+ (found ${node_major})"
    record_artifact "$key" "failed" "$sz" "Node.js 22+ required for Wrangler"; FAILED=1
    return 0
  fi
  local wrangler_out
  if wrangler_out="$(npx wrangler r2 object put "${BUCKET}/${PLAN}/${key}" --file "$file" --content-type image/png --remote 2>&1)"; then
    printf '%s\n' "$wrangler_out" | grep -iE 'upload complete|creating object|warning' | tail -4
    echo "   uploaded ${key} (${sz}B)"
    record_artifact "$key" "uploaded" "$sz" ""
  else
    echo "   FAIL upload ${key}:"
    printf '%s\n' "$wrangler_out" | tail -20
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

echo "== render sightseeing overview (hotel/airport endpoints excluded) =="
# Build the plan overview from nearby route places only. Full routes stay available on
# per-day maps; excluding their far-away hotel/airport endpoints keeps sightseeing pins
# at a useful zoom level.
PLAN_RF="${OUT}/plan.routes"; : > "$PLAN_RF"
# Count mappable vs total days up front so the log explains WHY plan.png was
# skipped (not just "no mappable stops across any day" — which is opaque when the
# operator knows the destination has 4 POIs).
MAPPABLE_DAYS=0; TOTAL_DAYS=0
for d in $ALL_DAYS; do TOTAL_DAYS=$((TOTAL_DAYS+1)); [ -n "${DAY_OVERVIEW_COORDS[$d]:-}" ] && MAPPABLE_DAYS=$((MAPPABLE_DAYS+1)); done
echo "   overview source: ${MAPPABLE_DAYS}/${TOTAL_DAYS} day(s) have mappable stops (4 POIs for jiufen should give 2/2)"
if [ "$MAPPABLE_DAYS" -eq 0 ]; then
  # Diagnose: are there any activities at all? (poi coords may simply be missing)
  ACT_COUNT="$($CHROMEPORT db query "SELECT COUNT(*) FROM activities WHERE plan_id='${PLAN}'" 2>/dev/null | awk -F'\t' 'NR>1{print $1; exit}')"
  POI_LINKED="$($CHROMEPORT db query "SELECT COUNT(*) FROM activities a JOIN destination_pois p ON (p.poi_id=a.poi_id OR (a.poi_id IS NULL AND p.title=a.title)) AND p.slug='${DEST}' WHERE a.plan_id='${PLAN}' AND p.lat IS NOT NULL" 2>/dev/null | awk -F'\t' 'NR>1{print $1; exit}')"
  echo "   diagnostic: activities=${ACT_COUNT:-?} poi-linked=${POI_LINKED:-?}"
  if [ -n "${ACT_COUNT:-}" ] && [ "$ACT_COUNT" != "0" ] && [ "${POI_LINKED:-0}" = "0" ]; then
    echo "   hint: activities exist but none linked to a geocoded POI — run set-activity-poi or set-poi-coords to geocode them."
  fi
fi
OVERVIEW="$(for d in $ALL_DAYS; do
  [ -n "${DAY_OVERVIEW_COORDS[$d]:-}" ] || continue
  color="${DAY_COLORS[$(( (d-1) % ${#DAY_COLORS[@]} ))]}"
  printf '%s\n' "${DAY_OVERVIEW_COORDS[$d]}" | awk -F',' -v c="$color" 'NF>=2{print $1","$2","c}'
done)"
if [ -n "$OVERVIEW" ] && printf '%s\n' "$OVERVIEW" | render_map "plan" "$PLAN_RF"; then
  upload_and_record "plan.png" "${OUT}/plan.png"
else
  if [ -z "$OVERVIEW" ]; then
    echo "   skip plan.png: no mappable stops across any day (${MAPPABLE_DAYS}/${TOTAL_DAYS} days had coords; see diagnostic above)"
  else
    echo "   FAIL plan.png: screenshot failed (${#OVERVIEW} chars of overview data)"
    record_artifact "plan.png" "failed" 0 "plan screenshot failed"
    FAILED=1
    # Don't just silently skip — the failure is now in the manifest as 'failed' not
    # 'skipped' so check-maps-fresh surfaces it as EMPTY requiring attention.
    # Continue; don't exit — the per-day maps may still have uploaded.
  fi
  if [ -z "$OVERVIEW" ]; then
    record_artifact "plan.png" "skipped" 0 "no mappable stops (${MAPPABLE_DAYS}/${TOTAL_DAYS} days had coords)"
  fi
fi

echo "== render hotel/airport overview =="
LOGISTICS="$(plan_logistics_coords | awk -F',' '
  NF>=3 { color=($3=="airport" ? "#ef6c00" : "#1565c0"); print $1","$2","color }')"
if [ -n "$LOGISTICS" ] && printf '%s\n' "$LOGISTICS" | render_map "plan-logistics"; then
  upload_and_record "plan-logistics.png" "${OUT}/plan-logistics.png"
else
  echo "   skip plan-logistics.png: no geocoded hotel/airport route endpoints"
  record_artifact "plan-logistics.png" "skipped" 0 "no geocoded hotel/airport endpoints"
fi

echo "== record snapshot timestamp =="
if [ "$FAILED" -eq 0 ]; then
  "$TRAVEL" mark-maps-snapshotted "$PLAN" || echo "warning: could not record snapshot timestamp"
else
  echo "   NOT stamping freshness — one or more maps failed (see manifest / check-maps-fresh)."
fi

echo "== done: maps + manifest at ${BUCKET}/${PLAN}/ — run: ./bin/travel check-maps-fresh --plan-id ${PLAN} =="
