# Dashboard (trip-dashboard-rs / legacy TS) — 歷史實作記錄

> Historical, non-normative record. If this conflicts with `CLAUDE.md`, the owning `SKILL.md`, source code, or tests, the current sources win.

（Next Steps 對應主題的 DONE 記錄搬入,保留日期/commit/spec/review 連結。）

## Legacy TS worker (RETIRED 2026-07-02) — operational reference

Undeployed via `wrangler delete`; source kept in-repo pending a 2026-08-02
archive-or-delete review. **NOT current** — the live worker is
`trip-dashboard-rs`, which has **NO edit mode** (grep-confirmed: no
`edit=TOKEN` / `ADMIN_TOKEN` / `api/edit` in `-rs`). The old
`trip-dashboard.yanggf.workers.dev` now 301-redirects → `-rs` via
`workers/trip-dashboard-redirect/` (preserves path+query so old
`?plan=…&token=…` share links still land).

- **Edit mode** — `?edit=TOKEN` activated inline editing when TOKEN matched the
  `ADMIN_TOKEN` secret. Pencil icons next to editable fields (theme, focus,
  activities, meals, transit notes). POSTed to `/api/edit` with token in JSON
  body. Set token: `wrangler secret put ADMIN_TOKEN`.
- **Routes** incl. `/?plan=<slug>&edit=TOKEN` (edit mode), `POST /api/edit`
  (write field) — neither exists in `-rs`.
- **Secrets** incl. `ADMIN_TOKEN` (edit mode) — not used by `-rs`.


- **Worker `workers-rs` port — DONE & DEPLOYED; legacy TS RETIRED (2026-07-02).** The Rust dashboard lives at `workers/trip-dashboard-rs/`, canonical at `trip-dashboard-rs.yanggf.workers.dev` (keyless maps, token auth, meal-pin links). **The old-URL cutover was ABANDONED as pointless** — `-rs` already served its own URL fully, so reclaiming the original URL (and putting OAuth in front of a previously-open URL) bought nothing. Instead: the legacy TS worker was **undeployed** (`npx wrangler delete`; source ARCHIVED 2026-08-26 → `archive/ts-dashboard-retired/` — the overdue archive-or-delete review resolved as ARCHIVE, -rs being at full TS parity), and the old `trip-dashboard.yanggf.workers.dev` now **301-redirects → `-rs`** via a 0.31 KiB redirect worker (`workers/trip-dashboard-redirect/`, `feat` commit `23d2e7d`; reclaims the `trip-dashboard` name, preserves path+query so old bookmarks + `?plan=…&token=…` share links land on `-rs`). Verified live (301 → HTTP 200 on follow). The `[env.production]` cutover block + `scripts/deploy-cutover.sh` remain in-repo as an unused record only. Redeploy the redirect if ever needed: `cd workers/trip-dashboard-redirect && npx wrangler deploy`. **D1 read-mirror pilot** (`scripts/deploy-d1-pilot.sh`) is independent + still optional.


**D1 read-mirror pilot (split from the OTA/DAL sweep bullet):**
- **CODE-PREPARED (2026-07-02, deploy-gated)** — a compare-only, owner+flag-gated `/diag/d1-compare` route in the `-rs` dashboard worker (`src/d1_compare.rs`, `worker` `d1` feature; reads `plans`+`date_anchors` from BOTH Turso and a D1 mirror and reports the dialect delta; **D1 never serves**; inert until Yang runs `wrangler d1 create` + sets `D1_COMPARE_ENABLED`). Runbook: `docs/plans/2026-07-02-dashboard-d1-mirror-pilot.md`.
