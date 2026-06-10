---
name: stage4-publish-dashboard
description: Prepare, deploy, and verify the trip dashboard on explicit request. Owns Stage 4 of the adopted research-first planning flow.
version: 1.0.0
requires_skills: [travel-shared, deploy-dashboard, weather-update]
requires_processes: [process_5_daily_itinerary]
provides_processes: []
---

# /stage4-publish-dashboard

Orchestration skill for **Stage 4 — Publish to Dashboard** of the adopted
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

Stage 4 makes the plan visible through the Cloudflare Workers trip dashboard.
It is intentionally explicit: itinerary/database changes do **not**
auto-deploy. Publish only when the user asks to deploy, share, refresh, or
verify the dashboard.

Use `/deploy-dashboard` for the lower-level Cloudflare deployment steps and
`/weather-update` when forecast data should be refreshed first.

## When to use

- User says "deploy dashboard", "publish trip", "share the plan", "refresh
  the dashboard", or "verify the live dashboard".
- Stage 3 has produced a detailed enough itinerary to show publicly or the
  user explicitly wants a provisional dashboard.

Do **not** auto-run this just because itinerary data changed.

## Workflow

1. **Verify publish readiness**
   ```bash
   ./bin/travel status --full --plan-id <plan_id>
   ./bin/travel itinerary --plan-id <plan_id>
   ./bin/travel validate-itinerary --dest <destination_slug> --severity warning
   ```
   Confirm:
   - P5 itinerary exists.
   - No unresolved validation errors.
   - ZH day/session content is populated enough for the dashboard audience.
   - Bookings or placeholders are acceptable to show.

2. **Refresh optional data**

   Weather is useful only near travel dates:
   ```bash
   ./bin/travel fetch-weather --dest <destination_slug>
   ```
   Use `/weather-update` if destination or date-window pre-checks are needed.

3. **Prepare dashboard content**

   If needed, update DB-backed dashboard fields before deploy:
   ```bash
   ./bin/travel set-day-theme <day> "<theme>" --zh "<zh_title>" --dest <destination_slug>
   ./bin/travel set-tod-zh <day> <session> --zh "<focus_zh>" --transit-zh "<transit_zh>"
   ```
   Keep all display content in Turso-backed CLI paths. Do not hardcode trip
   content in Worker code.

4. **Deploy on explicit request**
   ```bash
   cd workers/trip-dashboard
   unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
   ```
   If Wrangler OAuth is not logged in, stop and ask the user to run
   `npx wrangler login` interactively.

5. **Verify live dashboard**
   ```bash
   curl "https://trip-dashboard.yanggf.workers.dev/?plan=<plan_slug>"
   curl "https://trip-dashboard.yanggf.workers.dev/api/plan/<plan_id>"
   ```
   Confirm both dashboard HTML and API JSON respond.

## Output

End with:
- Whether deployment was run or intentionally skipped.
- Dashboard URL and API URL checked.
- Validation/weather/content status.
- Any follow-up needed before sharing.

## Notes

- Stage 4 can run any time after an itinerary exists; it is not strictly
  sequential.
- The dashboard reads from Turso, so many content updates appear without
  Worker redeploy. Deploy is still needed for Worker code/config changes.
- Respect the adopted default: explicit deploy only.
