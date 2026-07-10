# OTA scrape→write→promote 橋修復 — 設計

**日期:** 2026-07-10
**狀態:** 設計已定（Codex 設計/決策 + Claude 逐項對照源碼 corroborate）。待 Yang review → writing-plans → Grok 施工 → Claude 審查+驗證。
**來源:** 真實資料 drill（osaka-aug-2026, 2026-08-05→08-09）挖出兩個 blocker。合成 drill 永遠碰不到 —— 它們只在真正跑「gwebcdb 抓真的 → ota write-offers → promote → 進 plan」這條橋時才暴露。findings: `.review/2026-07-10-real-scrape-drill-findings.md`。

## 目標

修兩個讓「真實抓來的 offer 進不了 plan」的 blocker（同橋、一起修、兩個 commit）：

- **#C** — `ota write-offers` 把 offer 的 `destination`/`region` 寫成 NULL，但 `promote-offers --dest <slug>` 用 `WHERE destination = ?1` 過濾 → 真 offer 永遠撈零筆。
- **#1** — `promote-offers` 只在去回航班都有時才寫 flight legs，真實航班來源（google_flights）只給「去程+來回總價」→ 真航班 offer promote 後零 legs → P3 永不 populated。

## 全域限制（照抄專案規則，每個 task 隱含遵守）

- **Agent-first 純文字** — stdout 純文字/表格，絕不 JSON。
- **Fail loud** — 缺 row/壞 slug 就 THROW，不 silent fallback。
- **Audit triad 留在 `cascade::common`** — domain 寫入進 `travel-db` repo；`plan_events`/`operation_runs`/`plans.version` 由 `record_operation` 統一。repo 不寫 audit。
- **Behavior-lock 測試** — real-Turso，`tests/common/mod.rs` harness（`bin`/`db_exec→Option<Rows>`/`seed_plan(plan,dest,version)`/`teardown_plan`/`Guard`/`nanos`/`is_credless(&stderr)`）；serialized `--test-threads=1` 背景跑；Guard 綁在 id 之後、不留 trailing teardown。
- **Pipeline** — Codex 設計/計畫/測試計畫；Grok 施工；Claude 逐行審 + corroborate + serialized 驗證。

---

## 決策 #C — `ota write-offers` 加 `--dest <slug>`，存 destination + region

**決定:** 給 `ota write-offers` 加一個**必填** `--dest <slug>`，寫 `offers.destination = <slug>`；`offers.region` 存 region_label（有就用，否則 region_code，否則 NULL）。**不**從 OTA region 自動推導 destination。

**理由（對照源碼 corroborate）:**
- `promote-offers` 已把 `--dest` 當全域 `offers.destination` key，只 `WHERE destination = ?1`（`promote_offers.rs:126,322`）。
- **沒有現成的 region→destination_slug 映射**：`destination_config` 只有 `slug`/`ref_id`，無 region/source/product 欄（`schema.sql:311`，Claude 已確認）。`ota_source_region_codes` 只映 `(source_id, product_type, region_label)→region_code`，不映 slug。所以憑空造映射是錯的；明確傳 `--dest` 才誠實。
- `write-offers` 已載入 claimed job，能用 `ota_jobs::get_params(conn, job_id)→Vec<(String,String)>`（`ota_jobs.rs:105`，Claude 已確認）讀 region_label/region_code；`ota enqueue` 已持久化這兩個 param（`enqueue.rs:54`）。
- `--dest` slug 用既有的存在性檢查驗證（fail loud on 壞 slug）。

**out of scope:** region→slug automap（不存在，不造）；把 promote 改成用 region 過濾（動到既有行為，風險大）。

---

## 決策 #1 — `PlanOfferWrite.flights` 改可變 Vec，`(Some,None)` 寫單 outbound leg

**決定:** `PlanOfferWrite.flights` 從 `Option<[PlanOfferFlightWrite; 2]>` 改成 `Vec<PlanOfferFlightWrite>`。build 時：去回都有→2 legs；只有去程→1 outbound leg；都無→空 vec。**不**加時刻解析（`PlanOfferFlightWrite` 維持 `direction`+`flight_number`）。

**理由（對照源碼 corroborate）:**
- `None` 在這裡沒資訊：空 vec = 無 legs、1 元素 = 只有去程、2 元素 = 去回。
- 唯一 constructor 是 `promote_offers::build_plan_offer_write`（`promote_offers.rs:378,432`）；唯一 consumer 是 `plan_offers::insert_offer`（`plan_offers.rs:78,147`）。
- 下游**不假設剛好 2 legs**：`select_offer` 把 `plan_offer_flights` 讀進 Vec，`has_flight()`=`!legs.is_empty()`（`select_offer.rs:94`，Claude 已確認）任意非空即當 flight offer 並 populate P3；`flight_legs::replace_from_offer` iterate 任意 slice（`flight_legs.rs:93,110`）。
- 只新增 `(Some(outbound), None) → vec![outbound]` 一個 arm。
- `plan_offer_flights` 表已有 time/code 欄（`schema.sql:711`），但**本次不填**（時刻解析另議）—— 時刻缺在 `offers`/TSV/promote 萃取那層，不是這裡。

**out of scope:** 時刻/機場代碼解析（parse "TPE 18:20 - KIX 22:00"）；動 `ImportPlanOfferWrite`/`ImportFlightPair`（不同的 import 路徑，Claude 已確認是別的 struct）。

---

## Task 1（commit 1）— #C：write-offers 存 destination/region，餵得動 promotion

**檔案:**
- 改 `rust/crates/travel-cli/src/ota/write_offers.rs`（加 `--dest` parse + 驗 slug + 讀 job params + 算 region + 傳入 parsed_to_offer_row）
- 改 `rust/crates/travel-cli/src/ota/common.rs:375`（`parsed_to_offer_row` 簽名 + 取代 `region:None, destination:None`）
- 改 usage 字串：`shaping.rs:312`、`search_compare.rs:278`、`CLAUDE.md:232`
- 測試 `rust/crates/travel-cli/tests/ota_write_offers.rs`（既有檔擴充）

**測試優先（RED）:** seed `destination_config`+`ota_sources`+`captures`+claimed `ota_jobs`+`ota_job_params(region_label,region_code)`+minimal plan（測試先 `db migrate`，因 `ota_job_params` CHECK runtime widen）。跑 `ota write-offers <job> --capture <cap> --claim-token <tok> --tsv <fixture> --dest <dest>` → assert `offers.destination=dest` AND `offers.region=region_label`。再跑 `promote-offers --from-offers --dest <dest> --plan-id <plan>` → assert 1 筆 `plan_offers`。teardown 刪 `ota_job_params`/global `offers`/capture/job/source/`destination_config` + `teardown_plan`；Guard 綁 id 之後。
**GREEN:** `cd rust && cargo test -p travel-cli --test ota_write_offers -- --test-threads=1`（背景 serialized）。

## Task 2（commit 2）— #1：outbound-only offer 寫單 leg + populate P3

**檔案:**
- 改 `rust/crates/travel-db/src/repo/plan_offers.rs:27`（`flights: Vec<PlanOfferFlightWrite>`）+ `:147`（`for leg in &write.flights`）
- 改 `rust/crates/travel-cli/src/promote_offers.rs:407`（build vec：both→2、outbound-only→1、else 空）+ 更新過時註解 `:21`
- 測試 `rust/crates/travel-cli/tests/promote_offers.rs`（既有檔加 focused test）

**測試優先（RED）:** seed 1 筆全域 offer `flight_outbound=CI120, flight_return=NULL`,價/日期齊。跑 `promote-offers` → assert `plan_offer_flights` 恰 `outbound|CI120`。再跑 `select-offer` → assert `flight_legs` 1 筆 outbound + `process_3_transportation='populated'`（`select_offer.rs:195`）。
**GREEN:** `cd rust && cargo test -p travel-cli --test promote_offers -- --test-threads=1`（背景 serialized）。

## Trickier-than-it-looks（Codex 標注）
- `scripts/schema.sql` 的 `ota_job_params` CHECK 是舊的；runtime migration widen 加了 destination/origin/currency/rooms/hotel（`schema.sql:568` vs `db_migrate.rs:143`）。測試要依賴 widened keys 前先 `db migrate`（照 `ota_write_offers.rs:49` 既有 pattern）。

## 驗收
- #C：真 offer 帶 destination 落地，`promote-offers --dest` 找得到並 promote 成功。
- #1：outbound-only 航班 offer promote 出 1 leg，select-offer 後 P3=populated。
- 兩個 behavior-lock 測試 serialized live 綠；既有 `promote_offers`/`ota_write_offers` regression 綠。
- 用真的 osaka-aug-2026（destination NULL 的真 offer）live smoke：backfill destination 後能 promote。
