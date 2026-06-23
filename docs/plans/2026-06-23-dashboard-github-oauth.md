# Requirement: GitHub OAuth for the trip dashboard (RS worker)

**Date:** 2026-06-23
**Worker:** `workers/trip-dashboard-rs/` (Rust / workers-rs), live at `trip-dashboard-rs.yanggf.workers.dev`
**Status:** IMPLEMENTED 2026-06-24 via a SHARED CRATE (committed; pending secrets + deploy).
The auth core is NOT hand-rolled per-worker — it is the proven, deployed implementation from
`finance-engineering/workers/plan-viewer-rs`, extracted into a shared crate
**`gwebcdb/crates/worker-github-oauth`** that BOTH workers depend on (same cross-repo path-dep
pattern as `turso-util`). Grok's first hand-rolled version (login-string-only gate, async
`crypto.subtle` HMAC) was DISCARDED in favor of the crate; only Grok's styled `render/auth.rs`
pages were kept.

The shared crate is hardened beyond the original ask: gates on the **immutable GitHub numeric id**
(`ALLOWED_GITHUB_ID=48974237`) AND login (`ALLOWED_LOGIN=yanggf8`) — a username rename can't grant
access; pure-Rust `hmac`+`sha2` (sync, fully unit-tested: 18 tests incl. tampered-login, wrong-id,
extended-expiry, valid-sig-wrong-login, garbage cookies, open-redirect); MAC-verified-before-parse;
fail-closed (`SESSION_SECRET`<32 or `ALLOWED_GITHUB_ID`==0 → deny); signed CSRF state with
`safe_path` return; 8h TTL; `__Host-` cookies. The crate is UI-free (`callback()` returns
`CallbackOutcome{Authorized|Denied{login,id}|BadState}`); travel renders styled pages on `Denied`,
finance returns plain 403.

**Build/test (all independently verified):** crate 18 passed + wasm32 clean; finance 3 passed +
wasm32 (byte-compat cookies `__Host-pv_*` → live sessions survive); travel 125 passed + wasm32.
**Code review (no CRITICAL):** share-link-logged-out works, index owner-only, `verify_session`
can't elevate a non-owner, XSS escaped, fail-closed holds. Two IMPORTANT issues FIXED:
(1) removed a hardcoded `"yanggf8"` banner fallback → uses `ALLOWED_LOGIN`; (2) reordered
`decode_state_at` to MAC-verify before any UTF-8/parse (matches `verify_session_at`). One MINOR
(unconditional share-token query) left as pre-existing/out-of-scope.

Travel files: DELETED `src/{session,oauth}.rs`; KEPT `src/render/auth.rs`; edited `router.rs`
(wired to crate `verify_session`/`CallbackOutcome`, `/auth/*` routes, `SESSION_SECRET`+`ALLOWED_*`,
two-step `OWNER_TOKEN` preserved), `lib.rs`, `Cargo.toml` (+crate dep, −`js-sys`), `i18n.rs`,
`styles.css`. NOT yet (user-performed): the secrets/vars, the GitHub OAuth App, deploy, and the
2nd-deploy `OWNER_TOKEN` removal (see below). Full plan: `~/.claude/plans/` shared-crate plan.

## Problem (observed)

Opening a dashboard URL **without a valid token** returns a bare `Response::error("Forbidden", 403)`
— plain-text "Forbidden", no HTML, no styling, no explanation, no link. Bad UX. Reproduced
2026-06-23:

| URL | Result |
|-----|--------|
| `/?plan=okinawa-2026&token=<valid>` | HTTP 200 ✅ |
| `/?plan=okinawa-2026` (no token) | `Forbidden` 403 ❌ bare text |
| `/` (no plan, no token) | `Forbidden` 403 ❌ bare text |

## Current auth model (what exists today)

Token-only. `workers/trip-dashboard-rs/src/auth.rs`:
- `AccessScope::Owner` — request `?token=` equals the `OWNER_TOKEN` secret → can view index + any plan.
- `AccessScope::Plan(slug)` — `?token=` matches a row in `plan_share_tokens` → can view that ONE plan.
- `AccessScope::Denied` — no/unknown token → 403.

Gates in `router.rs`: index `/` (owner only), `/?plan=` (owner or matching share token),
`/voucher/*` (same scope as plan), bare fallthrough. `/map/*` images are ungated (low-stakes).

**There is NO GitHub OAuth code anywhere** (verified by grep across both workers + wrangler config).
The "session" matches are itinerary time-of-day sessions, unrelated to auth. So GitHub OAuth is a
feature to BUILD, not a regression to fix.

## Target auth model (the requirement)

Three tiers. GitHub OAuth replaces the **owner token**; per-plan **share tokens stay**.

| Surface | Who | Mechanism |
|---------|-----|-----------|
Three INDEPENDENT auth mechanisms — one per surface type. They do not share machinery.

| Surface | Caller | Mechanism |
|---------|--------|-----------|
| **Dashboard pages** (index `/`, owner plan views, the overview/plan switcher) | **Human / browser** | **GitHub OAuth** (allow-list = **`yanggf8`**), handled GRACEFULLY — see UX below |
| **Write / API endpoints** (any future inbound POST/write the worker exposes) | **Machine / CLI / script** | **Secret token** (admin-token style: bearer in `Authorization` header). NOT OAuth — machines can't do the interactive login. |
| **Sharing** (handing one trip to family/friends) | **Anyone with the link** | **Flexible — Yang's choice.** Per-plan share token is the current impl; could be anything. Not a hard constraint. |

Rationale for the split (decided 2026-06-23):
- **Dashboard = OAuth** because it's a human in a browser; OAuth proves identity and the worker can
  set a session cookie. The whole point of this requirement is that OAuth is handled WELL — a
  protected page must offer a way IN (a "Sign in with GitHub" page), never a bare "Forbidden".
- **API = secret token**, NOT OAuth: OAuth needs a human to click "Approve", so it cannot protect a
  CLI/script/curl call. A bearer secret (like the legacy TS worker's `ADMIN_TOKEN`) is the right
  tool for machine-to-machine. (The RS worker exposes NO write API today — all writes go through
  `./bin/travel` straight to Turso — so this tier is "ready when a write endpoint is added", not now.)
- **Sharing = anything** — left open; share tokens work today and need no change for this requirement.

Key invariant: a **shared plan link must keep working for a logged-out stranger** — sharing one trip
must NOT force a GitHub login. So OAuth gates owner/dashboard surfaces, never the share path.

## UX requirements (THE core of this requirement — "handle OAuth well, not just Forbidden")

The complaint: a protected page returns a bare `Response::error("Forbidden", 403)` — dead end, no
way forward. **Every denial must offer a path forward.**

- **Owner dashboard page (e.g. `/`) while logged out** → a **friendly styled page with a
  "Sign in with GitHub" button** (links to `/login` → GitHub OAuth). NOT bare "Forbidden", NOT a
  silent redirect — a visible, clickable login affordance. (Optionally auto-redirect to `/login`,
  but the page-with-button is the baseline so the user always understands what's happening.)
- **After login**, the page shows "signed in as `yanggf8`" + a **logout** link.
- **Logged-in NON-owner GitHub user** on an owner surface → styled "this account isn't authorized;
  sign in as the owner / log out" page — still a path forward, not a dead end.
- **Shared plan link with a valid token** → renders, **no login** prompt at all.
- **Plan link with a bad/missing token** AND not logged in → friendly styled page ("this trip link
  needs a valid share link"), with the GitHub login option for the owner. Not bare "Forbidden".
- **API/machine call** with a bad/missing secret token → **401 JSON** (`{"error":"unauthorized"}`),
  NOT an HTML login page — machines don't read login pages.
- All HTML denial pages use the existing `render::page(title, body, lang)` shell (CSS, viewport,
  anti-translate, ZH default) — consistent with the rest of the dashboard.

## AS-BUILT deploy steps (code done; these are user-performed)

1. **GitHub OAuth App** (github.com/settings/developers → New OAuth App):
   - Homepage: `https://trip-dashboard-rs.yanggf.workers.dev`
   - **Authorization callback URL: `https://trip-dashboard-rs.yanggf.workers.dev/auth/callback`** (exact).
   - Copy Client ID → `GITHUB_CLIENT_ID`; generate a client secret → `GITHUB_CLIENT_SECRET`.
2. **Secrets** (`cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put <NAME>`):
   - `SESSION_SECRET` — `openssl rand -base64 48` (must be ≥32 bytes; crate fails closed otherwise).
   - `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`.
   - `PUBLIC_ORIGIN` — `https://trip-dashboard-rs.yanggf.workers.dev` (the worker builds
     `redirect_uri = {PUBLIC_ORIGIN}/auth/callback` per request).
3. **Vars** (plaintext config, in `wrangler.toml` `[vars]` or `wrangler` vars):
   - `ALLOWED_LOGIN = "yanggf8"`
   - `ALLOWED_GITHUB_ID = "48974237"` (the immutable numeric id — primary gate).
4. **Routes** (already in code): `/auth/login` (302 → GitHub), `/auth/callback` (exchange + gate +
   set `__Host-td_session`), `/auth/logout` (clear). Owner pages gated via the crate's
   `verify_session`; share links `/?plan=X&token=…` stay open to logged-out viewers.
5. `npx wrangler deploy`, then smoke-test (see Verification in the plan).
6. **Two-step OWNER_TOKEN cutover**: this deploy keeps the `OWNER_TOKEN` `?token=` owner fallback;
   after confirming GitHub login works, a 2nd deploy removes that branch + `wrangler secret delete
   OWNER_TOKEN`.

**Finance** (`plan-viewer-rs`) just needs a redeploy — its config (env vars, `SESSION_SECRET`,
`__Host-pv_*` cookies) is unchanged, so live sessions survive.

## Decisions captured (2026-06-23, all confirmed by Yang)

- **Dashboard → GitHub OAuth**, handled GRACEFULLY: a protected page must offer a "Sign in with
  GitHub" affordance, never a bare "Forbidden". Allow-list = **only `yanggf8`**.
- **API / write endpoints → secret token** (admin-token style), NOT OAuth. (No write API exists in
  the RS worker today; this tier applies if/when one is added.)
- **Sharing → keep the current mechanism unchanged.** Per-plan share tokens (`plan_share_tokens` +
  `?token=`) work fine; this requirement does NOT touch the share path. A shared link must keep
  rendering for a logged-out viewer.

## Resolved (no longer open)
Both prior open questions are settled: the styled-denial page + the OAuth flow shipped together
(one pass, via the shared crate); the GitHub App steps are the checklist above. The auth core lives
in the shared `gwebcdb/crates/worker-github-oauth` crate, reused by finance + travel.
