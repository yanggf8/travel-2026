---
name: deploy-dashboard
description: Deploy the trip dashboard (Rust workers-rs worker) to Cloudflare Workers with pre-deployment checks
version: 1.1.0
requires_skills: [travel-shared]
requires_processes: [process_5_daily_itinerary]
provides_processes: []
---

# /deploy-dashboard

## Purpose

Deploy the trip dashboard to Cloudflare Workers with pre-checks to prevent common
deployment failures. The live dashboard is the **Rust** worker
`workers/trip-dashboard-rs/` (SSR, GitHub-OAuth-gated owner pages + share-token
viewer links). The legacy TS `workers/trip-dashboard/` was **retired and
undeployed 2026-07-02** — do not deploy it; the old
`trip-dashboard.yanggf.workers.dev` URL now 301-redirects to `-rs`.

## When to Use

After the itinerary is finalized and/or weather fetched, on the user's explicit
request. A production deploy is **Yang-gated** — confirm before deploying.

## Workflow

### 0. Verify wrangler auth

```bash
npx wrangler whoami
```

If "Not logged in": **stop and ask the user to run `npx wrangler login` in their
terminal** — OAuth needs an interactive browser session Claude Code cannot
complete. Wait for confirmation before proceeding.

### 1. Pre-check secrets (`-rs` worker)

```bash
cd workers/trip-dashboard-rs
unset CLOUDFLARE_API_TOKEN && npx wrangler secret list
```

Required secrets: `TURSO_URL`, `TURSO_TOKEN`; for OAuth-gated owner pages also
`SESSION_SECRET`, `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `PUBLIC_ORIGIN`
(vars `ALLOWED_LOGIN`/`ALLOWED_GITHUB_ID` live in `wrangler.toml`). Set a missing
one from `.env`, e.g.:

```bash
TURSO_URL=$(grep '^TURSO_URL=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_URL <<< "$TURSO_URL"
TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_TOKEN <<< "$TURSO_TOKEN"
```

### 2. Deploy

```bash
cd workers/trip-dashboard-rs
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
```

`unset CLOUDFLARE_API_TOKEN` is critical — wrangler uses OAuth by default and the
env var causes auth conflicts. Deploy runs `worker-build --release` (Rust→WASM);
first run installs `worker-build` via cargo.

### 3. Verify deployment

Owner pages are OAuth-gated, so verify login-free via a share-token viewer link
(mint with `./bin/travel share-token --show-full --plan-id <id>`):

```bash
curl -sL -o /dev/null -w "%{http_code}\n" \
  "https://trip-dashboard-rs.yanggf.workers.dev/?plan=<slug>&token=<share_token>"
```

Expect `200`. The `-rs` worker is **SSR — there is no `/api/plan` JSON route**;
render the page HTML to verify content. Use `-L` so the old-URL 301 is followed.

## Error Handling

| Error | Cause | Fix |
|-------|-------|-----|
| `Not logged in` / non-interactive error | OAuth session expired or `CLOUDFLARE_API_TOKEN` set | User runs `npx wrangler login` in terminal; then retry |
| Authentication failed | `CLOUDFLARE_API_TOKEN` env var set | `unset CLOUDFLARE_API_TOKEN` |
| Missing secrets | Wrangler secrets not configured | See step 1 above |
| Build failed | `worker-build`/cargo/wasm toolchain issue | Ensure Rust + `wasm32` target; `cargo install worker-build` |
| 403 / sign-in page on owner URL | Page is OAuth-gated | Use a `?plan=<slug>&token=<share_token>` viewer link, or sign in as owner |

## See Also

- `workers/trip-dashboard-rs/` — the live Rust dashboard source
- `workers/trip-dashboard-rs/wrangler.toml` — deployment config
- CLAUDE.md "Trip Dashboard (Cloudflare Worker)" section for the full picture
  (auth, share tokens, keyless maps, the retired TS worker + redirect worker)
