# Session summary: dashboard share-link copy

**Date:** 2026-06-25  
**Feature:** One-click **Copy share link** for logged-in owner on single-plan dashboard pages  
**Worker:** `workers/trip-dashboard-rs/` → https://trip-dashboard-rs.yanggf.workers.dev  
**Commits:** `747fa40` (implementation), `848b0fc` (docs + reviews)

---

## What was requested

1. Add a one-click control on the trip dashboard so the **owner** can copy a shareable URL for family/friends.
2. Recipients open that URL **without GitHub login** — same per-plan share-token model as before.
3. Implement, review, deploy, and verify on the live RS worker.

**Clarified auth model:**

| Role | Mechanism | Sees copy button? |
|------|-----------|-------------------|
| Owner (you) | GitHub OAuth session on dashboard | **Yes** |
| Recipient | `?plan=<slug>&token=<share_token>` | **No** (plan only) |

"OAuth only" means the **owner must be logged in** to see the button — not that recipients need OAuth.

---

## What was built

### Behavior

- On `/?plan=<slug>` when `is_owner_session` is true, the page prepends an **owner chrome** bar:
  - Signed-in label (`signedInAs` + GitHub login)
  - **Copy share link** button (ZH: 複製分享連結)
  - Logout link
- Clipboard URL format (never owner secret, never session cookie):

  ```
  https://<PUBLIC_ORIGIN>/?plan=<hyphen-slug>&token=<share_token>
  ```

- Share token comes from `plan_share_tokens` (newest per plan via `ORDER BY created_at DESC`).
- Inline clipboard script with `navigator.clipboard` + `execCommand` fallback.
- If no share token exists for the plan: shows `noShareLink` hint instead of a button.

### Files changed

| File | Change |
|------|--------|
| `workers/trip-dashboard-rs/src/render/share.rs` | **NEW** — `build_share_maps`, `share_url`, `owner_plan_chrome`, `COPY_SCRIPT`, unit tests |
| `workers/trip-dashboard-rs/src/router.rs` | Dual share maps; `owner_chrome` when `is_owner_session` |
| `workers/trip-dashboard-rs/src/render/mod.rs` | `render_plan(..., owner_chrome)` |
| `workers/trip-dashboard-rs/src/i18n.rs` | `copyShareLink`, `noShareLink` |
| `workers/trip-dashboard-rs/src/styles.css` | `.owner-chrome`, `.copy-share-btn`, etc. |
| `.gitignore` | Exclude `.wrangler/` |
| `docs/plans/2026-06-25-dashboard-share-link-copy.md` | Design plan + Codex review notes |
| `CLAUDE.md`, `docs/reference/CLI.md` | Document deployed feature |

### Post-review fix

Codex flagged that `token_to_plan` auth map must store **hyphenated** slugs (`okinawa-2026`), not underscore `plan_id` (`okinawa_2026`). Fixed before deploy.

---

## Reviews

| Stage | Verdict | Notes |
|-------|---------|-------|
| Design (Codex + Claude) | Conditionally GO | Security: never copy owner token or session |
| Implementation (Claude) | GO | 7/7 checklist, 136 tests pass |
| Implementation (Codex) | GO | Underscore `plan_id` caveat → fixed |

Review artifacts: `.review/` (codex, claude design + impl reviews, final-decision).

---

## Deployment

### Initial issue

Early deploys skipped the documented pre-check workflow (`/deploy-dashboard`, OAuth plan AS-BUILT steps). User correctly flagged this.

### Corrected workflow (followed in session)

1. `npx wrangler whoami` — logged in as `yanggf@yahoo.com`
2. Verify `.env` has `TURSO_URL` + `TURSO_TOKEN`
3. `cd workers/trip-dashboard-rs && wrangler secret list` — all 7 secrets present:
   - `TURSO_URL`, `TURSO_TOKEN`
   - `SESSION_SECRET`, `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `PUBLIC_ORIGIN`
   - `OWNER_TOKEN` (transitional owner fallback)
4. `unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`

### Deploy history (this session)

| Version ID | Notes |
|------------|-------|
| `e4150e2f-fe5c-4a35-9992-b81728ebbfaf` | Redeploy after doc workflow correction |
| `a5f54738-136f-4706-9487-8b025016cd08` | Latest deploy on user request |

**Live URL:** https://trip-dashboard-rs.yanggf.workers.dev

### Post-deploy curl smoke tests

| URL | HTTP | Copy button in HTML? |
|-----|------|----------------------|
| `/?plan=okinawa-2026&token=<share>` | 200 | No (correct — viewer) |
| `/?plan=okinawa-2026` (no token) | 403 | No — styled denial + login path |
| `/` (no auth) | 200 | No — GitHub sign-in page |
| `/auth/login` | 302 | Redirects to GitHub OAuth |

Share token for okinawa-2026: `dd90508f2efd063ee760197d127fffa4`  
CLI: `./bin/travel share-token --show --plan-id okinawa-2026`

---

## gwebcdb browser verification

Used gwebcdb CDP bridge (`/home/yanggf/b/gwebcdb/bridge/`) against WSLg Chrome on `:9222`.

### Tools run

- `list_pages.py` — tab inventory
- `navigate.py` — dashboard + `/auth/login`
- `save_page_text.py`, `snapshot.py` — page capture
- `form_state.py` — GitHub login form inspection
- Custom DOM poll — `document.querySelector('.copy-share-btn')`

### Results

| State | Page title | `hasCopyBtn` | `hasOwnerChrome` |
|-------|------------|--------------|------------------|
| Not logged in, no token | 分享連結無效 | false | false |
| Share-token viewer URL | 200 itinerary | false | false (CSS classes only in stylesheet) |
| GitHub OAuth not completed | Sign in to GitHub | false | false |
| 90s poll after `/auth/login` | 分享連結無效 | false | false (timeout — no session) |

**Conclusion:** Deploy is correct. Copy button is **gated on GitHub owner session** (`__Host-td_session`). gwebcdb cannot see the button without human GitHub sign-in in the WSLg Chrome profile.

Screenshot: `/home/yanggf/b/gwebcdb/outputs/screenshot.png` (denial page: 擁有者？登入)

### Owner verification steps (human-required)

1. Open WSLg Chrome (Windows desktop): https://trip-dashboard-rs.yanggf.workers.dev/auth/login?next=%2F%3Fplan%3Dokinawa-2026
2. Sign in as **yanggf8**
3. Land on `/?plan=okinawa-2026` — expect owner chrome + **複製分享連結**
4. Re-run gwebcdb DOM check or `./bin/travel share-token --show` to confirm copied URL

---

## Why the button may be missing (troubleshooting)

| Mistake | Symptom |
|---------|---------|
| Using legacy URL `trip-dashboard.yanggf.workers.dev` | No copy feature (TS worker) |
| Not GitHub-logged-in on RS worker | 403 / 分享連結無效 |
| Opening share-token URL as owner test | Plan renders, no owner chrome (by design) |
| Expecting `OWNER_TOKEN` `?token=` to show button | Owner scope yes, chrome no — session-only |

**Correct owner flow:**

```
/auth/login  →  GitHub OAuth  →  /?plan=okinawa-2026  (no ?token=)
```

---

## Outstanding / optional follow-ups

1. **Human GitHub login + gwebcdb re-verify** — blocked on interactive OAuth in WSLg Chrome.
2. **Update `/deploy-dashboard` skill** — still points at legacy `workers/trip-dashboard/`; should reference `trip-dashboard-rs` + OAuth secret checklist from `docs/plans/2026-06-23-dashboard-github-oauth.md`.
3. **URL cutover (Task 11 handoff)** — rename worker to `trip-dashboard` so bookmarks on the original URL get the feature.
4. **Remove `OWNER_TOKEN` fallback** — second deploy after confirming GitHub login works everywhere.

---

## Reference docs

- Design plan: `docs/plans/2026-06-25-dashboard-share-link-copy.md`
- OAuth deploy: `docs/plans/2026-06-23-dashboard-github-oauth.md`
- RS handoff Task 11: `docs/handoff-dashboard-rs-finish.md`
- Deploy skill (stale path): `src/skills/deploy-dashboard/SKILL.md`