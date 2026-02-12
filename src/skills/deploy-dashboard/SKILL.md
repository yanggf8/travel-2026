---
name: deploy-dashboard
description: Deploy trip dashboard to Cloudflare Workers with pre-deployment checks
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: [process_5_daily_itinerary]
provides_processes: []
---

# /deploy-dashboard

## Purpose

Deploy trip dashboard to Cloudflare Workers with pre-checks to prevent common deployment failures.

## When to Use

After itinerary finalized and/or weather fetched.

## Workflow

### 1. Pre-check .env configuration

```bash
cat .env | grep -E "TURSO_URL|TURSO_TOKEN"
```

Required variables: `TURSO_URL`, `TURSO_TOKEN`

### 2. Pre-check wrangler secrets

```bash
cd workers/trip-dashboard
npx wrangler secret list
```

Required secrets: `TURSO_URL`, `TURSO_TOKEN`, `GOOGLE_MAPS_KEY` (optional)

If not set:
```bash
TURSO_URL=$(grep '^TURSO_URL=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_URL <<< "$TURSO_URL"
TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_TOKEN <<< "$TURSO_TOKEN"
```

### 3. Deploy

```bash
cd workers/trip-dashboard
unset CLOUDFLARE_API_TOKEN
npx wrangler deploy
```

`unset CLOUDFLARE_API_TOKEN` is critical — wrangler uses OAuth by default; the env var causes auth conflicts.

### 4. Verify deployment

```bash
# Test API endpoint
curl "https://trip-dashboard.<subdomain>.workers.dev/api/plan/<plan-id>"

# Test dashboard (requires ?plan= param)
curl "https://trip-dashboard.<subdomain>.workers.dev/?plan=<slug>"
```

Actual routes:
- `/?plan=<slug>` — Dashboard HTML
- `/api/plan/<id>` — Raw JSON API

## Error Handling

| Error | Cause | Fix |
|-------|-------|-----|
| Authentication failed | `CLOUDFLARE_API_TOKEN` env var set | `unset CLOUDFLARE_API_TOKEN` |
| Missing secrets | Wrangler secrets not configured | See step 2 above |
| Build failed | Missing deps or TS errors | `cd workers/trip-dashboard && npm install` |
| 403 on dashboard | Missing `?plan=` param | Use `/?plan=<slug>` |

## See Also

- `workers/trip-dashboard/` — Dashboard source code
- `workers/trip-dashboard/wrangler.toml` — Deployment config
- CLAUDE.md "Dashboard" section for full deployment commands
