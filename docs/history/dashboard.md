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
