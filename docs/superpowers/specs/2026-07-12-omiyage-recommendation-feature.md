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
- 「地點」概念部分已存在:tokyo seed 給 `isetan_shinjuku`（`:135`）**和 `daimaru_tokyo`（`:183`）** 都有 `omiyage` tag（Claude 第一手 corroborate 2026-07-13 — 初稿誤說 daimaru 沒有,Codex spec-review 抓到、Claude 驗證訂正;daimaru notes 就是「Tokyo Banana, all classic gifts」）+ `omiyage_premium` cluster。品項在 notes 裡是 free-text,**不是 canonical item→seller fact**。
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

**建表位置（Codex 修正）:** **`db_migrate.rs` 建**（runtime,`CREATE TABLE IF NOT EXISTS`,idempotent,比照 destination_transit `db_migrate.rs:1410`）;**`schema.sql` mirror/regenerate DDL**（reference artifact,非 runtime）。不是「both create」。
**Types/nullability（Codex precision）:** 全欄 `TEXT`;`notes`/`purchase_note` optional（可 NULL）;其餘 required（`validate data` 防守每個 required 欄）。無 JSON 欄。

**邏輯參照（SQLite FK 不強制 — repo 靠 write-time validation + `validate data`,`db_migrate.rs:803` Claude 確認）:**
- `items.slug → destination_config.slug`
- `locations.(slug,item_id) → items.(slug,item_id)`
- `locations.(slug,poi_id) → destination_pois.(slug,poi_id)`
- v1 slug query 無需額外 index（兩 PK 皆以 slug 開頭）。

**欄位決策:**
- `category` 是**品項級**（和菓子/藥妝/名產…）,非 POI tag（百貨賣多類）。v1 非空文字,不建 category catalog 表（過早）。
- **不加 price**（因店/包裝/日期而異,item-級 scalar 會誤導 — Yang 確認採 Codex 建議）。
- `confidence` 限 `verified|reviewed`（無 `estimate`;不推論 seller link）。
- **不加** `buy_at_area/station/airport` 欄（那是 POI 屬性;機場店 = 一個 POI）。
- **不用** `destination_markets`（只有一個 ordered free-text market 值,`schema.sql:332`）。
- 兩表都有自己的 `source_url/fetched_at/confidence` — 品項真實性 vs 「這 POI 賣它」是**兩個不同 claim,各需證據**。

**寫入正確性 — 精確 transaction 契約（Codex spec-review DRIFT 修正:不可 blanket INSERT OR REPLACE parent item）:**
1. Begin transaction。
2. Validate destination（slug ∈ destination_config）+ same-slug POI（`--buy-at` ∈ destination_pois for this slug）。
3. Read `(slug, item_id)`。
4. **item 不存在** → validate + insert 完整 item（需完整 item bundle）,再 insert location。
5. **item 已存在** → **不寫 item**,只 upsert 那個 location（item bundle 可省;若給則須 match 既存 item metadata,不符 fail,絕不 silently 覆蓋/更新 item）。
6. 只在預期 affected-row 數對時 commit;任何錯 → 既無 partial row 也無 partial update。
- 同賣家 re-add（同 (slug,item_id,poi_id)）→ 只更新那個 location 一列,`fetched_at` 刷新（重新查證的時間戳）。

**`validate data` 完整 invariants（Codex:初稿清單不全）,各自 error:**
- item.slug ∉ destination_config。
- location 的 parent `(slug,item_id)` 缺（orphan location）。
- location 的 same-slug POI 缺。
- item 無任何 location。
- item 的 identity/name/category 空/空白。
- 任一表的 source_url/fetched_at/confidence 空/空白。
- confidence ∉ {verified, reviewed}。
- **空的 omiyage 表全域仍 valid**（absence 是 `query-omiyage` 的 fail-loud 條件,不是 repo-wide integrity error）。

---

## CLI — 2 個新命令（Codex 建議 + Claude corroborate add-transit 模式）

### `query-omiyage --slug <destination_slug>`（read-only view）
- 分類分組（by category）渲染。每個品項印:**item_id**（Codex:canonical view 必須露出加第 2 賣家所需的 id）、name、notes、**item provenance（source_url/confidence/fetched_at）一次**;其下每個賣家印:POI title/poi_id、area、station、address/hours、purchase_note、**location provenance（source_url/confidence/fetched_at）per-location**。（item 證據 vs 賣家證據是**兩組**,分開標,不混。）
- **area/station/address/hours 來自 join `destination_pois`**（不存進 omiyage 表）;POI 的 address/hours 在 schema 是 nullable → **有值印值,無值印 `—`**（非保證有值）。
- **Deterministic ordering:** category → item name/item_id → POI title/poi_id。
- 已知 dest 但無 sourced item/location → **fail loud**（不偽裝非空輸出）。
- **corrupt/unsourced row（缺 provenance 等）→ fail,不 silently omit 或印無來源推薦**（no-cheat）。
- bare `--slug` 足夠 v1;category/item/seller filter 是 YAGNI。含 `--help`/`-h`。**reject `--plan-id`**。
- **專屬 view,不 fold 進 query-destination-ref**（v1）:直接對意圖、不膨脹 populate-itinerary 的 cluster 輸出、一個 canonical omiyage view。

### `add-omiyage <slug> <item_id> --buy-at <poi_id> --location-source-url <url> --location-confidence verified|reviewed [--name <n>] [--category <c>] [--item-source-url <url>] [--item-confidence verified|reviewed] [--notes <t>] [--purchase-note <t>]`
- 一次一個「品項在某 POI 賣」的斷言。重複帶另一個 `--buy-at`（再跑一次）加另一個賣家。
- **existing-item 契約（Codex CLI-GAP）:**
  - **新 item**:`--name`/`--category`/`--item-source-url`/`--item-confidence` **全required**（完整 item bundle）。
  - **已存在 item**:item bundle **可省**;若給,須與既存 item metadata **match**,不符 **fail**（絕不 silently 覆蓋 item）。
  - `--buy-at`/`--location-source-url`/`--location-confidence` **永遠 required**（每個賣家斷言的 location provenance）。
- **像 add-transit**:slug-keyed GLOBAL reference data,**無 --plan-id、無 audit triad**（`add_transit.rs:1-15` Claude 確認）。SQL 進**新 `travel-db::repo::omiyage`**;CLI 只 orchestrate+render（DAL 邊界,比照 `repo::destination_ref`）。transactional（見上「寫入正確性」6 步）。
- **Fail-loud reject（Codex 補全）:** 未知 dest;未知或 wrong-slug POI;空/空白的 identity/name/category/required 字串;缺/空白 provenance;confidence ∉ {verified,reviewed};非 HTTP(S) 或畸形 source URL;缺 flag 值;多餘 positional;**`--plan-id`**;未知 flag（`reject_unknown_flags`）。
- main.rs dispatch arm 照 add-transit（`"add-transit" => add_transit::run(rest)` 風格,Claude 確認 `main.rs`）。含 `--help`/`-h`（`wants_help`）。
- **賣家 POI 必須已在 `destination_pois`（same slug）** — 缺就 fail,不 fallback free-text。（v1 用既有 POI;未來常需註冊新賣家 POI → 另立 generic destination-POI authoring 命令,不在本範圍。）
- **兩個命令都 reject `--plan-id`**(query-omiyage 也是)。

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

## confidence 語意（Codex UNDER-SPEC 修正）

- `verified` = 有分店特定/官方直接證據（分店樓層導覽、店家清單、官方商品頁）。`reviewed` = 有可信但非分店特定證據（近期 review/彙整）。二者皆非「保證現貨」。
- **機器強制 vs 人工:** schema/命令**強制** source_url 存在 + HTTP(S) 格式 + confidence ∈ {verified,reviewed};但「這 URL 真的證明這品項在這店賣」是**人工 live-smoke 證據審查**(agent 查證),不能只靠 schema 證。兩者分開:命令擋缺失/畸形,人審擋內容不實。

## 驗收

- Schema: 2 表由 `db_migrate.rs` 建（idempotent）+ `schema.sql` mirror,PK 正確（`(slug,item_id)` / `(slug,item_id,poi_id)`）,全 TEXT,無 JSON 欄。
- `add-omiyage`: slug-keyed,**無 audit triad**（即使設 `TRAVEL_PLAN_ID` 也不寫 plan mutation）,item+location 原子（見 6 步契約）,existing-item 只 upsert location（item bytes 不變）,同賣家 re-add 一列 + refresh fetched_at,fail-loud 全矩陣。
- `query-omiyage`: 分類分組純文字,item_id + dual provenance 分開標,nullable POI 欄印 `—`,deterministic ordering,unknown/known-empty dest fail loud,corrupt row fail（不 silently omit）。
- `validate data`: 各自抓 8 類 invariant（slug/orphan-location×2/item-無-location/空 identity/空 provenance×2/bad confidence）;空表全域仍 valid。
- Provenance: 兩表各 source_url（HTTP(S)）+ confidence ∈ {verified,reviewed} + fetched_at 自動 stamp;不進 Rust seed。

**Behavior-lock 測試（real-Turso,common:: harness — Codex 列全）:**
- schema 欄/PK + idempotent migration。
- add→query round-trip（證 POI join、nullable 印 `—`、雙 provenance、分類分組）。
- 既有 item 加第 2 賣家:2 locations,原 item **byte-for-byte 不變**。
- 同賣家 re-add:1 location 列 + fetched_at refresh。
- 失敗 create:0 item + 0 location 列（原子 rollback）。
- fail-loud 矩陣:未知 dest、未知/wrong-slug POI、空 required、缺/空 provenance、bad confidence/URL、缺 flag 值、多餘 positional、未知 flag。
- **兩命令 connect/write 前 reject `--plan-id`**。
- query 未知 dest + 已知但空 dest（皆 fail loud,訊息不同）。
- validate 各 orphan 分別抓 + item-無-location + 各表缺 provenance + bad confidence + 畸形 required。
- 無 audit/plan mutation（即使 `TRAVEL_PLAN_ID` set）。
- `--help` parity + 真 typo flag reject。

**測試 teardown（CRITICAL — Codex:非 plan-keyed 全域列,`common::teardown_plan` 清不掉）:** 測試用 **unique slug** + **panic-safe RAII Guard**,依相依序清:**locations → items → POIs/destination_config**（parent 後於 child）,加防禦性 pre-clean（比照 `CLAUDE.md:119-120` 的 Guard 慣例 + `db_exec_teardown` for non-plan-keyed rows）。

- Live smoke: 用真 gwebcdb-verified 品項（如 osaka/tokyo 真伴手禮 + 真賣家 POI）走 add-omiyage → query-omiyage。
