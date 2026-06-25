# Dashboard one-click share-link copy

**Date:** 2026-06-25  
**Worker:** `workers/trip-dashboard-rs/`  
**Status:** DEPLOYED 2026-06-25 — commit `747fa40`, worker version `3b182c83-bfb6-4281-acea-97ed1eacf271` at `trip-dashboard-rs.yanggf.workers.dev`

## Goal

When the owner views a trip at `/?plan=<slug>`, add a button that copies a **viewer share URL** to the clipboard:

```
https://<PUBLIC_ORIGIN>/?plan=okinawa-2026&token=<share_token>
```

Recipients open that link logged-out and see exactly one plan (`AccessScope::Plan`). The copied URL must **never** contain `OWNER_TOKEN`, a GitHub session cookie, or any owner credential.

**Scope:** single plan page only (not the plans index).

## Current state

| Piece | Location | Notes |
|-------|----------|-------|
| Auth tiers | `workers/trip-dashboard-rs/src/auth.rs` | `Owner` / `Plan(slug)` / `Denied` |
| Share tokens loaded | `workers/trip-dashboard-rs/src/router.rs` L99–116 | `SELECT token, plan_id FROM plan_share_tokens` → `HashMap<token, plan_id>` |
| Share URL format (CLI canonical) | `rust/crates/travel-cli/src/share_token.rs` | `https://{host}/?plan={hyphen-slug}&token={token}` |
| Plan render entry | `workers/trip-dashboard-rs/src/render/mod.rs` `render_plan()` | SSR HTML, no client JS today |
| Public origin | `router.rs` | `env.secret("PUBLIC_ORIGIN")` already read for OAuth |

## Design decisions

### 1. Copy button: logged-in owner only

You use the dashboard after **GitHub login**. On a plan page, you see **Copy share link**.

| Who | What they see |
|-----|----------------|
| **You** (logged into dashboard) | Plan + copy button |
| **Others** (your shared link) | Plan only — no copy button, **no login** |

Gate: `is_owner_session` in `router.rs` (you have a valid owner session cookie). Not related to how recipients access the trip — they use the copied `?token=<share_token>` URL.

### 2. Always copy the view-scope token, not the current URL

The button copies the **per-plan share token** from Turso — never the request URL or any bearer the page was loaded with.

- Build URL from `PUBLIC_ORIGIN` + `?plan=` + hyphenated slug + `&token=` + **share token row**
- **Never** use `query.get("token")` for the copy URL (can be owner secret)
- **Never** echo `req.url()`

### 3. Two maps from one query

```sql
SELECT token, plan_id FROM plan_share_tokens ORDER BY created_at DESC
```

From the same rows, build:

1. `token → plan_id` — existing auth (`auth::resolve()`); unchanged semantics
2. `plan_slug → newest token` — owner copy chrome only

For DESC order, use `entry(plan_id).or_insert(token)` (first-write-wins = newest). Plain `insert()` would overwrite and pick the **oldest**.

Normalize plan slug: `plan_id.replace('_', "-")` for URL parity with CLI `share_url()`.

### 4. URL parity with CLI

Match `rust/crates/travel-cli/src/share_token.rs`:

- Trim trailing slash from `PUBLIC_ORIGIN`
- `https://{origin}/?plan={hyphen-slug}&token={token}`

### 5. No token minting in the Worker

If no share token exists, show owner-only muted hint (bilingual):

- EN: "No share link yet — run `./bin/travel share-token`"
- ZH: equivalent

Minting stays CLI-only per project architecture.

### 6. Minimal inline JS (first read-mode script)

- `navigator.clipboard.writeText()` primary; `textarea` + `execCommand('copy')` fallback
- Read URL from `data-copy-url` via `button.dataset.copyUrl` — **never** interpolate URL inside `<script>`
- `data-copy-url` escaped with `esc()` (double-quoted attribute)
- Flash via `textContent` only (no `innerHTML`)
- Include script once per plan page when owner chrome is rendered

### 7. UI placement

Owner chrome bar at top of plan body (above alerts / booking summary):

```
[Signed in as yanggf8]  [Copy share link]  [Log out]
```

- Logged-in owner (signed-in label + copy + logout)
- CSS: flex/wrap for 640px mobile; `.owner-chrome`, `.copy-share-btn`, `.copy-share-ok`, `.copy-share-missing`

### 8. Voucher links unchanged

Keep passing request `token` to `summary::render()` for voucher PDF links — separate from share copy URL.

## Files to change

| File | Change |
|------|--------|
| **NEW** `workers/trip-dashboard-rs/src/render/share.rs` | `share_url()`, `copy_button()`, `owner_plan_chrome()`, `COPY_SCRIPT`, unit tests |
| `workers/trip-dashboard-rs/src/render/mod.rs` | `pub mod share`; `render_plan(..., owner_chrome: &str)` |
| `workers/trip-dashboard-rs/src/router.rs` | Dual maps; owner chrome on `/?plan=` when logged in (`is_owner_session`) |
| `workers/trip-dashboard-rs/src/i18n.rs` | `copyShareLink`, `noShareLink` |
| `workers/trip-dashboard-rs/src/styles.css` | Owner chrome + copy button styles |

**Not in scope:** index page, Worker token mint/revoke, TS legacy worker.

## Security checklist

- Share token only in copied URL — never `OWNER_TOKEN`, never session cookie
- Copy UI only when owner is logged into dashboard (`is_owner_session`)
- `esc()` on `data-copy-url`; safe inline script
- `PUBLIC_ORIGIN` for host (not hardcoded)
- No new Worker write endpoints

## Tests

Unit tests in `render/share.rs`; update `render/mod.rs` test signature.

Manual verify post-deploy: owner sees button; clipboard has share token; incognito viewer has no button; missing token shows hint.

## Codex review (2026-06-25)

**Verdict:** Conditionally ready — no CRITICAL blockers. See `.review/codex-review.md`.

Key refinements incorporated above: dual maps, `or_insert` for newest token, never use request `?token=` for copy URL, slug normalization, trim `PUBLIC_ORIGIN`.

## Claude review (2026-06-25)

**Verdict:** GO with plan amendments. See `.review/claude-review.md`.

Amendments adopted: soften “newest token” to second-granularity; dual-map insert split (auth `insert` vs copy `or_insert`); copy button for logged-in owner only.

## Implementation review (2026-06-25)

Codex + Claude implementation review: **GO**. See `.review/impl-review-summary.md`, `.review/claude-impl-review.md`.

Post-review hardening: auth map stores hyphenated slug (underscore `plan_id` rows safe). Root `.gitignore` excludes `.wrangler/`.