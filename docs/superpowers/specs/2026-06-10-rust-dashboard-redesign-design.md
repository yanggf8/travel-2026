# Rust Dashboard Redesign — Design Spec

**Date:** 2026-06-10
**Status:** Design approved (pending written-spec review)
**Supersedes:** the TS `workers/trip-dashboard/` (kept live until cutover)
**Related:** `docs/plans/2026-06-10-roadmap-v2-rust.md` §6 (parked Worker port — now unparked as a redesign), `docs/handoff-worker-noon-meals-transfers.md` (interim TS bug-fix)

## 1. Goal

Replace the TypeScript Cloudflare `trip-dashboard` Worker with a greenfield **Rust/WASM
(`workers-rs`)** worker, driven primarily by a **UX/RWD redesign** that learns from best-in-class
day-by-day itinerary products (Wanderlog, TripIt). Rust is the chosen build stack; design quality
is the success metric.

Non-goals: eliminating npm (wrangler stays for deploy — accepted), porting the Rust CLI's DB
crates into WASM (explicitly rejected — see §9), carrying over web-based content editing.

## 2. What we learned from research (drives the design)

Captured via chromeport from a web search + Wanderlog/TripIt directly:
- **Vertical chronological timeline**, not a dense list; sessions separated by distinct visual cues
  and color-coded icons.
- **Itinerary + map in one view** — Wanderlog's core pitch; the single biggest UX upgrade.
- **Progressive disclosure** — clean cards; booking minutiae (PNR, confirmation #) tucked behind a
  tap.
- **At-a-glance summary** — current day's primary event / trip overview up top.
- Patterns explicitly **not** adopted (wrong for a read-only, solo, already-booked trip view):
  drag-and-drop reorder, collaborative editing, email-forward auto-import, flight-status alerts.

## 3. Hard constraints

- **Pure SSR, zero client-side JavaScript.** (Preserves the current worker's core principle and
  fits WASM cleanly.)
- **Read-only.** All writes happen via the `./bin/travel` CLI (the audited source-of-truth write
  path). No `/api/edit`, no `?edit` mode.
- **Turso is the sole data source.** Port the *TS HTTP pipeline shape*, not the Rust CLI DB layer.
- **Keyless maps.** No GCP / Google Static Maps **API** key. (See §6 and memory `no-gcp-maps-key`.)
- **workers-rs/WASM realities** (per Codex review): entry via `#[event(fetch)]`; bindings/secrets
  via `Env`, not `std::env`; no filesystem/process/threads/native-TLS at runtime; CSS/small assets
  via `include_str!`; binary assets via R2 / Static Assets binding; run `wasm-opt`; keep deps lean
  and `wasm32-unknown-unknown`-compatible.
- Both itinerary formats must keep working: **session-based** (Tokyo, Okinawa) and
  **schedule-based** (Kyoto).
- Bilingual: Traditional Chinese default, `?lang=en` toggle; `notranslate` + `lang` to stop browser
  auto-translation (as today).

## 4. Architecture & module boundaries

Greenfield crate at `workers/trip-dashboard-rs/` (name TBD at impl), built as focused,
independently-testable modules. The 1,207-line `render.ts` becomes six render modules.

```
workers/trip-dashboard-rs/src/
├── lib.rs          # #[event(fetch)] entry → router
├── router.rs       # route match; calls auth BEFORE any Turso read
├── auth.rs         # token → AccessScope { Owner, Plan(slug), Denied }
├── turso.rs        # Turso HTTP pipeline client (port the known-good TS pipeline shape)
├── model.rs        # typed structs: Plan, Day, Session, Activity, Meal, Transfer, Stop{lat,lon}
├── render/
│   ├── mod.rs      # page shell: <head>, css include, lang, notranslate
│   ├── index.rs    # plans index — OWNER scope only
│   ├── summary.rs  # trip at-a-glance + booking summary (flights/hotel/transfers)
│   ├── day.rs      # one day card: theme, weather strip, 4 session blocks, day map
│   ├── session.rs  # one session block: activities, meals, transit pill, session map
│   └── map.rs      # map-image <img> (R2 URL) + stop list + per-stop Google Maps links
├── styles.rs       # inline CSS, mobile-first RWD (include_str! or const)
└── i18n.rs         # zh/en strings + weather-label translation
```

**Data flow:** `request → router → auth(AccessScope) → turso(pipeline) → model(Plan) →
render/*(HTML String) → Response`. Each `render/*` takes a typed slice of the model and returns an
HTML `String` — unit-testable in isolation (render one day card, assert HTML).

**Boundary rules (per Codex):** `render` never sees raw Turso rows; `turso` returns typed row
structs (not ad-hoc `HashMap<String,String>`); `model` owns assembly. No "shared dashboard model"
god-object that becomes a second state manager.

## 5. Access & routing model

| Route | Owner token | Per-plan token (matching slug) | No/invalid token |
|---|---|---|---|
| `GET /` (plans index) | render index + all plans | ❌ Denied | ❌ Denied |
| `GET /?plan=<slug>` | render that plan | render iff token's slug == `<slug>` | ❌ Denied |
| `GET /api/plan/<id>` | JSON | JSON iff scope matches | ❌ Denied |

- `auth.rs` resolves a request to `AccessScope::{Owner, Plan(slug), Denied}` from the URL token,
  **before** any Turso read.
- **Owner token** (Cloudflare secret `OWNER_TOKEN`): full access — index + every plan + switch.
- **Per-plan token:** an opaque, unguessable string that scopes the viewer to exactly one plan.
  No index, no other plans. A different plan needs a different share link. **No signing** — signing
  vs. not is no real difference for a bearer token in a shareable URL; a long random token already
  prevents guessing, and the data is low-stakes. The token *is* the share link, so it living in the
  URL is intended.
- Per-plan tokens are stored in a small Turso table `plan_share_tokens(plan_id TEXT, token TEXT,
  created_at)` — generated/managed via a CLI command (e.g. `./bin/travel share-token <plan>`),
  consistent with "CLI is the write path".
- URL tokens are **view-scope only — never DB credentials.** One server-side Turso **read** token
  lives in a Cloudflare secret and is used for all reads.

## 6. Maps — keyless, three levels

The biggest UX upgrade and the most-discussed constraint. Resolved to a **keyless** design.

- **Distinction:** the Google Static Maps **API** URL needs a GCP key + billing (rejected). A
  static map **image** (a PNG) needs no key — it is just an `<img>`.
- **Map images** (plan / day / session levels): captured with **chromeport** (`browser snapshot` /
  screenshot of a map view with the day's pins) at plan-build time → stored as PNG in **Cloudflare
  R2** (an R2 bucket bound to the worker; *not* the Static Assets binding, since per-plan map images
  are generated/updated independently of deploys) → served as `<img src>` from the worker. No Maps
  API call in the
  render path, no key, CDN-cached by construction.
- **Per-stop links:** `https://www.google.com/maps?q=<lat>,<lon>` — keyless deep-link; the rich
  destination page/directions live in Google, not in our page.
- **Pins/labels:** pins are positioned from **stored coordinates**, not render-time geocoding. Each
  stop shows **title + address as text** beside the map (human-readable), plus its Maps link. We do
  not rely on Google to interpret place names.

Three map extents:
1. **Plan map** — all stops across all days (trip footprint).
2. **Day map** — that day's stops + route.
3. **Session map** — the stops within one session block.

(Watch the practical pin count per image; many pins are fine for a captured PNG — there is no
Static-Maps-API URL-length limit because we are not calling that API.)

## 7. Data additions

- **`destination_pois.lat REAL`, `destination_pois.lon REAL`** (+ existing provenance columns).
  Seed sourced coordinates per Naha POI (via chromeport research), like the rest of the reference
  data. Required for map pins and per-stop links.
- **`plan_share_tokens(plan_id, token, created_at)`** — per-plan view-scope tokens (§5).
- **Map PNGs in Cloudflare R2** — produced by a chromeport snapshot step; not in Turso (binary
  assets do not belong in the RDB), not in git.

Everything else the page needs already exists in Turso (days/timesofday/activities/session_meals/
flight_legs/hotels/airport_transfers/weather).

## 8. The page (layout)

```
MOBILE (≤640px, single column)            DESKTOP (≥900px, two column)
┌────────────────────────────┐            ┌──────────────────────────────────┐
│ Header: trip · dates · EN  │            │ Header                           │
│ ── Trip summary ──         │            │ ┌ summary ┐ ┌ PLAN MAP (all) ──┐ │
│  ✈ flights ⏷ 🏨 hotel ⏷    │            │ │ flights │ │ all days' pins   │ │
│  🚌 transfers              │            │ └─────────┘ └──────────────────┘ │
│ ── [ PLAN MAP image ] ──   │            │ Day card: timeline │ day map     │
│ Day 1 ▸ theme · 🌧 weather │            │  上午/中午/下午/晚上 │ (image)    │
│  [ day map image ]         │            └──────────────────────────────────┘
│   上午 ▸ stop·🍜·🚃 [s.map] │
│   中午 ▸ …                 │   Progressive disclosure: PNR/CFM behind <details> (no JS).
│   下午 ▸ …                 │   Day-type left-border accents (arrival/full/departure).
│   晚上 ▸ …                 │   Weather strip with feels-like + rain-gear tip.
│ Day 2 ▸ …                  │
└────────────────────────────┘
```

The **4-session block** model (上午/中午/下午/晚上) is rendered by construction, which structurally
fixes the `noon`-dropped bug that exists in the current TS worker.

## 9. Why not reuse the Rust CLI DB layer (Codex finding)

`turso-util` and the CLI DB layer are native-process oriented: filesystem/env/process token
minting, `libsql::Builder::new_remote`, Tokio multithread. None compile/run on
`wasm32-unknown-unknown` in a Worker. The worker must use the **Turso HTTP pipeline** (port the TS
`turso.ts` shape) with a read token from a Cloudflare secret. The worker and the CLI share *the
database*, not *the data-access code*.

## 10. Build order (Approach B — isolated modules)

0. *(Parallel, optional)* Run `docs/handoff-worker-noon-meals-transfers.md` so the **current** TS
   dashboard is correct while the Rust worker is built.
1. `turso.rs` + `model.rs` — HTTP pipeline + typed structs (port known-good shape). Unit tests on
   assembly.
2. `auth.rs` + `router.rs` — AccessScope gate; `plan_share_tokens` table + CLI token command. No
   render yet.
3. `render/day.rs` + `render/session.rs` — the timeline (fixes noon by construction).
4. `map.rs` — R2 `<img>` URLs + per-stop links; seed `destination_pois.lat/lon`; chromeport
   snapshot pipeline producing the plan/day/session PNGs into R2.
5. `render/summary.rs` + `render/index.rs` + `styles.rs` — RWD polish, owner index.
6. `wasm-opt`, deploy to a **staging** URL, compare against the bug-fixed TS worker, then cut over
   DNS/route and retire the TS worker.

## 11. Testing

- Unit-test each `render/*` module against a fixture `Plan` model (assert HTML fragments —
  e.g. a noon session with a meal renders the meal; a transfer renders route/time/price).
- `auth.rs`: owner/plan/denied scope resolution table.
- `turso.rs`: assembly from canned pipeline responses.
- Integration: against the live `okinawa-2026` / `tokyo-2026` (session-based) and `kyoto-2026`
  (schedule-based) plans on the staging URL before cutover — confirm no regression on either
  itinerary format.

## 12. Token-expiry countdown banner

The worker authenticates to Turso with a single static `TURSO_TOKEN` secret (the HTTP-pipeline
path; there is no D1 and no in-worker token minting — minting is CLI-only via `turso-util`). The
one silent failure mode is **token expiry**: when it lapses, every request starts throwing a
pipeline error with no prior warning. Surface that as a visible countdown so it can be refreshed
*before* it breaks.

- **Banner** in the header/top-of-body: "🔑 Token refresh in N days", N computed at request time
  (`Date.now()` is available in the worker runtime) from the token's expiry date.
- **Escalation by styling:** >30d → muted/hidden, ≤30d → amber warning, ≤7d → red alert, past
  expiry → "TOKEN EXPIRED — refresh now" (render this state even on the pipeline-error path if
  feasible, since reads are already failing by then).
- **Visibility:** operational info — gate behind owner/edit scope (§5), not on public share links.
- **Source of the expiry date — prefer auto:** Turso DB tokens are JWTs with an `exp` claim. Decode
  the token payload (base64 of the middle segment; no signature check needed just to read `exp`)
  and derive expiry automatically — no second secret to keep in sync. Fallback: if the issued
  token's `exp` is effectively non-expiring, treat the countdown as a self-imposed rotation
  reminder against a configured date (annual) stored alongside the token. See Open Items.

## 13. Open items to resolve during planning

- Final crate/worker name and R2 bucket/binding name.
- Exact `plan_share_tokens` lifecycle (generate, list, revoke) and the CLI command surface.
- chromeport map-capture mechanics: which map UI to drive, zoom/extent per level, image naming
  convention in R2 keyed by `(plan, day?, session?)`.
- Whether the JSON API (`/api/plan/<id>`) stays (handy) or is dropped (smaller surface).
- Token-countdown source (§12): decode the live `TURSO_TOKEN`'s JWT `exp` once during planning —
  if it's a real near-term date, build the auto-from-JWT path; if effectively non-expiring, add a
  `TURSO_TOKEN_EXPIRES` (or `app_config` row) self-rotation date instead.
