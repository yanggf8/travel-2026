# Runbook: cut the `trip-dashboard.yanggf.workers.dev` URL over from the TS worker to the `-rs` worker

**Date:** 2026-07-02 · **Status:** READY (Codex-advised ACCEPT; Claude-corroborated vs source).
**Owner action required:** all `wrangler` commands run on Yang's Cloudflare account — Claude cannot deploy.

## Decision (Codex-advised, adopted)

- **Mechanism:** deploy the **Rust `-rs` worker under `name = "trip-dashboard"`** to reclaim the
  `*.workers.dev` hostname. Reusing the Worker name is the correct reclaim on `workers.dev` (both
  workers already use `*.workers.dev`, no custom routes/zones). **NOT a 301 redirect** (that only
  forwards, doesn't reclaim). A custom domain is the better *long-term* decoupling of URL from Worker
  name, but is out of scope here.
- **Auth stays ON (intended end state):** the `-rs` worker keeps its GitHub-OAuth gating on owner
  pages. After cutover, `trip-dashboard.yanggf.workers.dev` **will require owner login** for owner
  pages; logged-out viewers still open per-plan `?plan=<slug>&token=<share_token>` share links (NOT
  OAuth-gated). This is the desired security posture — the legacy TS worker was open; the reclaim
  tightens it. (If you ever want the old URL open, that's a *separate* decision and a code change to
  make OAuth conditional — not this runbook.)
- **Rollback:** redeploy the legacy TS worker from `workers/trip-dashboard/` under
  `name = "trip-dashboard"` (it still exists, unchanged). One command, full revert.

## The critical gotcha (do NOT skip) — OAuth callback / PUBLIC_ORIGIN

The `-rs` worker derives its OAuth callback from the **`PUBLIC_ORIGIN` secret**
(`workers/trip-dashboard-rs/src/router.rs:63` `env.secret("PUBLIC_ORIGIN")`). It is currently set to
`https://trip-dashboard-rs.yanggf.workers.dev`. Reclaiming the `trip-dashboard` URL means the callback
origin changes, so BOTH must be updated or **owner login breaks with a redirect_uri mismatch**:
1. Set the `PUBLIC_ORIGIN` secret (under the production env) to `https://trip-dashboard.yanggf.workers.dev`.
2. In the **GitHub OAuth App** settings, add/set the Authorization callback URL to
   `https://trip-dashboard.yanggf.workers.dev/auth/callback` (keep the `-rs` one too if you want both
   URLs to work during transition).

## What Claude prepared (in-repo, no deploy)

- **`workers/trip-dashboard-rs/wrangler.toml`** — added a `[env.production]` block that overrides only
  `name = "trip-dashboard"` (and re-declares the R2 bindings + vars, which wrangler env blocks require).
  The default (top-level) config is unchanged, so `npx wrangler deploy` still ships to `-rs` as today;
  the cutover uses `npx wrangler deploy --env production`. This keeps the reclaim opt-in and reversible.
- This runbook.

No security code changed. No behavior changed until you deploy `--env production`.

## Cutover steps (you run these)

Preconditions: `-rs` worker is at TS feature parity (it is, commit `e7c2a89`) and deploys cleanly to
`trip-dashboard-rs` today.

1. **Verify the -rs worker is healthy on its own URL** (baseline):
   - Owner login + a plan page render at `trip-dashboard-rs.yanggf.workers.dev`.
   - A `?plan=okinawa-2026&token=<share>` link renders logged-out.
2. **Point OAuth at the target URL** (the gotcha above):
   - GitHub OAuth App → add callback `https://trip-dashboard.yanggf.workers.dev/auth/callback`.
3. **Deploy the -rs worker under the production (reclaim) name:**
   ```bash
   cd workers/trip-dashboard-rs
   unset CLOUDFLARE_API_TOKEN && npx wrangler deploy --env production
   ```
   (This runs `worker-build --release` and ships as `trip-dashboard`, taking the
   `trip-dashboard.yanggf.workers.dev` hostname from the TS worker.)
4. **Set the production env's secrets** (env-scoped; the top-level `-rs` secrets do NOT carry into
   `--env production`). For each: `... npx wrangler secret put <NAME> --env production`:
   - `PUBLIC_ORIGIN` = `https://trip-dashboard.yanggf.workers.dev`  ← the changed one
   - `SESSION_SECRET`, `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` (same values as `-rs`)
   - `TURSO_URL`, `TURSO_TOKEN` (from repo `.env`)
   - (vars `ALLOWED_LOGIN`/`ALLOWED_GITHUB_ID` come from `[env.production.vars]` in wrangler.toml — no
     secret put needed; R2 `MAPS`/`VOUCHERS` bind the same existing buckets.)
5. **Redeploy** if secrets were added after the first deploy (`npx wrangler deploy --env production`).
6. **Smoke test on `trip-dashboard.yanggf.workers.dev`:**
   - Owner login → callback → owner dashboard renders (proves the PUBLIC_ORIGIN/callback fix).
   - Logout works.
   - A `?plan=<slug>&token=<share>` link renders logged-out (share path unbroken).
   - Maps/vouchers load (R2 buckets shared — should just work).
   - `?lang=en` renders EN.
7. **Retire the TS worker** (optional, after a stable day): `cd workers/trip-dashboard && npx wrangler
   delete` — OR leave it dormant as instant rollback. Recommend leaving it a few days.

## Rollback

```bash
cd workers/trip-dashboard
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy   # redeploys TS as name="trip-dashboard"
```
Then revert the GitHub OAuth callback if you removed the `-rs` one. The old URL is back to the open TS
worker within one deploy.

## After cutover
- Update CLAUDE.md "Trip Dashboard — two workers": the `-rs` worker now serves BOTH
  `trip-dashboard` and `trip-dashboard-rs`; the TS worker is retired/dormant.
- The `-rs`-serves-`trip-dashboard-rs` default deploy still works for staging.
