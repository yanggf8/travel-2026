# docs/history — 歷史實作記錄索引

> Historical, non-normative records. If any of these conflict with `CLAUDE.md`,
> the owning `SKILL.md`, source code, or tests, the current sources win. These
> are an engineering journal for archaeology, NOT current instructions.

## 主題檔
- [rust-cli-dal-and-tests.md](rust-cli-dal-and-tests.md) — Rust port / DAL 遷移 / test-harness decoupling
- [ota-pipeline.md](ota-pipeline.md) — OTA execution layer / resolver / promote-bridge
- [dashboard.md](dashboard.md) — workers-rs port / OAuth / share-link / legacy TS 操作（已退役）
- [planning-flow-cli-and-drills.md](planning-flow-cli-and-drills.md) — drills / CLI hardening / content-depth / F/G findings
- [features-and-reference-data.md](features-and-reference-data.md) — omiyage / map-coverage / add-transit / POI

## Open items（非歷史 — 真正待辦,不進 DONE archive）

- **Real-scrape drill 未修的 findings** — #2 (`query-offers` can't filter by capture/job), #3 (restaurant verification asymmetry), #4 (fit↔group_tour type collapse), #5a (no `ota show-capture`), #5c (no under-extraction warn). 見 `.review/2026-07-10-real-scrape-drill-findings.md`.
- **新 destination / OTA source onboarding**（tokyo 以外）— 目前 seed 全 tokyo-scoped;新增需一次 live WSLg capture + `write-offers`（無 per-source code）。
- **D1 read-mirror pilot** — CODE-PREPARED、deploy-gated（Yang 手動 `wrangler d1 create` + 設 `D1_COMPARE_ENABLED`）。Runbook: `docs/plans/2026-07-02-dashboard-d1-mirror-pilot.md`. 細節見 [dashboard.md](dashboard.md).
- **Legacy TS worker** — 2026-08-02 archive-or-delete review（source 暫留 `workers/trip-dashboard/`）。
