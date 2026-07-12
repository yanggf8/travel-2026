# 伴手禮推薦 + 購買地點 feature — 設計

**日期:** 2026-07-12
**狀態:** 設計已定（Claude brainstorm + Codex 設計探索 + Claude 逐項 corroborate vs 源碼）。待 Yang review → writing-plans（+ Codex 審 plan/test-plan + Claude corroborate）→ Grok 4.5 施工 → Claude 審+verify。
**來源:** Yang 需求「要能推薦伴手禮以及購買的地點」。

## 目標

給專案加「推薦伴手禮（品項）+ 購買地點」的能力。答兩個問題:**帶什麼回家**（品項）+ **去哪買**（賣家 POI）。

## 全域限制（照抄專案規則）

- **No hardcode / no local data / Turso-only** — 伴手禮資料是 Turso reference data,**不**寫進 Rust seed constants（`db_seed_destination_refs.rs` 是 curated hardcode,新資料不進那裡）。
- **No JSON in RDB** — 全正規化欄位,無 JSON blob。
- **Agent-first plain text** — `query-omiyage` 輸出純文字/表格,非 JSON。Fail loud。
- **No-cheat provenance（核心）** — 品項 + 「這 POI 賣這品項」都要真證據,agent-research + gwebcdb-verify,不編。Google Maps 證明店存在 **不等於** 證明它賣這品項。
- **Reference-data 命令模式** — 像 `add-transit`/`set-poi-coords`:slug-keyed GLOBAL reference,**無 audit triad**（無 plan_events/operation_runs/plans.version）,SQL 進 `travel-db` repo,CLI 只 orchestrate+render。
- **Pipeline** — Codex 設計/審;Grok 4.5 施工;Claude 逐行審 + corroborate + verify。

---

## 決策 — 模型:品項→地點,複用現有 `destination_pois` 當賣家（Codex 建議 + Claude corroborate）

**決定:** 伴手禮**品項**是 destination-scoped reference data（跨 plan 複用,像 POI）。每個品項 many-to-many 連到現有 `destination_pois`（賣家地點）。**不**新建地點模型（機場店也當成 POI）。**不**做 per-plan 購物清單（v1）。

**理由（Codex + Claude corroborate vs 源碼）:**
- `destination_pois` 已是 canonical 地點模型（area/nearest_station/address/hours/lat/lon/provenance;`schema.sql:342`,Claude 確認）—— 賣家地點複用它,不重造。
- 「地點」概念部分已存在:tokyo seed 給 `isetan_shinjuku` 一個 `omiyage` tag（`db_seed_destination_refs.rs:135`,Claude corroborate:isetan 有、daimaru 只有 department_store/food — Codex 略寬,不影響設計）+ `omiyage_premium` cluster。品項在 notes 裡是 free-text（"Premium omiyage: Yoku Moku, Henri Charpentier…"）,**不是 canonical item→seller fact**。
- `query-destination-ref` 刻意不讀 poi_tags（`destination_ref.rs:99` 註解:「intentionally not fetched here (output parity)」）—— 所以現有 tag 對 agent/user 不可見。新 feature 需要一個 canonical item→seller 事實 + 專屬 view。
- 純標地點（model B）答不了「哪個品項」;per-plan（model C）重複可複用事實 + 過早擴張 audit/itinerary/dashboard。→ **品項→地點 + 複用 POI** 是最小正解。

**三概念保持區分（不混）:**
- `destination_poi_tags.tag='omiyage'` = 廣義地點分類。
- `omiyage_premium` cluster = itinerary 用的購物 POI 群組。
- `destination_omiyage_locations` = 「某品項在某 POI 有賣」的證據。
- **絕不**推論「omiyage_premium 裡每個 POI 都賣每個品項」;品項**不**連 cluster;連賣家時**不**要求/自動建 omiyage tag（避免第二個一致性義務）。

---

## Schema — 2 張新正規化表（照 destination_transit 風格,`schema.sql` + `db_migrate.rs` 兩處建）

| Table | 欄位 | PK |
|---|---|---|
| `destination_omiyage_items` | `slug`, `item_id`, `name`, `category`, `notes`, `source_url`, `fetched_at`, `confidence` | `(slug, item_id)` |
| `destination_omiyage_locations` | `slug`, `item_id`, `poi_id`, `purchase_note`, `source_url`, `fetched_at`, `confidence` | `(slug, item_id, poi_id)` |

**邏輯參照（SQLite FK 不強制 — repo 靠 write-time validation + `validate data`,`db_migrate.rs:803` Claude 確認）:**
- `items.slug → destination_config.slug`
- `locations.(slug,item_id) → items.(slug,item_id)`
- `locations.(slug,poi_id) → destination_pois.(slug,poi_id)`

**欄位決策:**
- `category` 是**品項級**（和菓子/藥妝/名產…）,非 POI tag（百貨賣多類）。v1 非空文字,不建 category catalog 表（過早）。
- **不加 price**（因店/包裝/日期而異,item-級 scalar 會誤導 — Yang 確認採 Codex 建議）。
- `confidence` 限 `verified|reviewed`（無 `estimate`;不推論 seller link）。
- **不加** `buy_at_area/station/airport` 欄（那是 POI 屬性;機場店 = 一個 POI）。
- **不用** `destination_markets`（只有一個 ordered free-text market 值,`schema.sql:332`）。
- 兩表都有自己的 `source_url/fetched_at/confidence` — 品項真實性 vs 「這 POI 賣它」是**兩個不同 claim,各需證據**。

**寫入正確性（無 FK 強制,靠這三點）:**
1. 寫前 validate slug + item + POI 存在。
2. 品項 + 地點原子寫入（item 不存在時,add-omiyage 建 item + 第一個 location;後續加賣家只 insert location）。
3. `validate data` 加錯誤:orphan location（品項/POI 不存在）、item 無 location、缺 provenance。

---

## CLI — 2 個新命令（Codex 建議 + Claude corroborate add-transit 模式）

### `query-omiyage --slug <destination_slug>`（read-only view）
- 分類分組（by category）渲染:品項名、推薦 note、賣家 POI title/id、area、station、address/hours、purchase_note、confidence、fetched_at、source_url。
- 已知 dest 但無 sourced item/location → fail loud（非空輸出偽裝）。
- **專屬 view,不 fold 進 query-destination-ref**（v1）:直接對 agent/user 意圖、不膨脹 populate-itinerary 用的 cluster 輸出、一個 canonical omiyage view。

### `add-omiyage <slug> <item_id> --name <n> --category <c> --buy-at <poi_id> --item-source-url <url> --item-confidence verified|reviewed --location-source-url <url> --location-confidence verified|reviewed [--notes <t>] [--purchase-note <t>]`
- 一次一個「品項在某 POI 賣」的斷言。重複帶另一個 `--buy-at` 加另一個賣家。
- **像 add-transit**:slug-keyed GLOBAL reference data,**無 --plan-id、無 audit triad**（`add_transit.rs:1-15` Claude 確認模式）。SQL 進**新的 `travel-db::repo::omiyage`**;CLI 模組只 orchestrate+render（DAL 邊界,比照 `repo::destination_ref`）。
- 冪等（INSERT OR REPLACE,像 upsert_transit）+ transactional（item+location 原子）。
- Fail-loud reject:未知 dest/POI、`--plan-id`、空 name/category、缺 provenance、未知 flag（`reject_unknown_flags`）、rows_affected 非預期。
- main.rs dispatch arm 照 add-transit（`main.rs` 的 `"add-transit" => add_transit::run(rest)` 風格）。
- **賣家 POI 必須已在 `destination_pois`** — 缺就 fail,不 fallback free-text。（v1 用既有 POI;未來若常需註冊新賣家 POI → 另立一個 generic destination-POI authoring 命令,不在本 feature 範圍。）

---

## Provenance / no-cheat 契約

資料流:**agent research / gwebcdb verification → add-omiyage → 正規化 Turso rows → query-omiyage**。**不**寫進 Rust seed（那是 hardcode）。

兩個 claim 各需證據:
1. **品項真實** → 官方製造商/商品頁（`items.source_url`）。
2. **這 POI 賣它** → 分店頁/樓層導覽/店家清單/近期分店證據（`locations.source_url`）。**Google Maps 證明店存在 ≠ 證明它賣這品項。**
- `fetched_at` 自動 stamp + query 印出。confidence = 證據品質,非保證現貨。
- **不**複製 set-meals（它只存 authored text + 選配 map query,無 seller/product provenance,`set_tod.rs:408`）。

---

## v1 範圍 / YAGNI（Yang 確認採 Codex 的克制範圍）

**v1 = 2 張正規化表 + 1 個 sourced 寫命令（add-omiyage）+ 1 個純文字 query（query-omiyage）+ validate。**

**Dashboard/itinerary:v1 不渲染**（Yang 確認）。現有 cluster/POI 機制已讓 agent 把購物店排進 activity;伴手禮 catalog 留 reference data。dashboard 要另加 Turso pipeline query + model + renderer（`workers/trip-dashboard-rs` 現組 16-query pipeline）— 過早。

**v1 延後（YAGNI）:** per-plan 購物清單 + audit;dashboard 卡;price/stock/包裝/變體/賞味期/免稅;ranking/personalization;多語 item 欄;category registry 表;area/station/airport 多型 link;多證據表/capture FK;JSON/TSV bulk import;POI-tag/cluster 自動同步。

## 驗收

- Schema: 2 表在 `schema.sql` + `db_migrate.rs` 建,PK 正確,無 JSON 欄。
- `add-omiyage`: slug-keyed,無 audit triad,冪等,item+location 原子,fail-loud（未知 dest/POI、--plan-id、空欄、缺 provenance、未知 flag）。
- `query-omiyage`: 分類分組純文字,無 sourced 資料時 fail loud,印 confidence/fetched_at/source_url。
- `validate data`: 抓 orphan location / item 無 location / 缺 provenance。
- Provenance: 品項 + 賣家各有 source_url;confidence ∈ {verified, reviewed};不進 Rust seed。
- Behavior-lock 測試（real-Turso,common:: harness）: add→query round-trip、fail-loud cases、validate 抓 orphan。
- Live smoke: 用真 gwebcdb-verified 品項（如 osaka/tokyo 真伴手禮）走 add-omiyage → query-omiyage。
