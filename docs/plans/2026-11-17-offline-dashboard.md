# 行程網站離線支援 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**可行性檢查（Yang 規則，2026-11-17）：** 可行，也值得做。整趟行程本來就是同一頁 SSR HTML（`render/mod.rs:92` `render_plan_at`），所以存一頁就是存整趟。換票、在電車上或山區斷線，都是旅途中真的會碰到的情境。最大的坑是第一次造訪時 SW 不控制這次導航，靠被動快取存不到東西，所以 spec 改成明確的 SAVE 流程。這份計畫照做：沒看到「已存好」就代表沒存好。依 spec §11 Q5，兩週內要出發的行程先用 §12 零程式碼清單，SW 不在行程進行中部署（T7 人工關卡）。

**Goal:** 斷線時，viewer 與 owner 都能開啟最後一次「已存好」的行程頁、所有地圖和所有本機票券 PDF，並清楚標示這是幾點存的副本。

**Architecture:**
- Worker 新增同源 `/sw.js`，內容是編進 wasm 的字串常數（`include_str!`，比照 `styles.rs`）。
- 計畫頁回應帶 `X-TD-View`；body 帶 `data-rendered-at`、`data-served="net"`、`data-tz`；狀態列與橫幅是 `hidden` 元素，文案由伺服器先填好。本機票券連結一律經同一個 helper 產生，有 sha 時加上 `?v=`。
- 既有的唯一 inline script（`BLOCK_MEMORY_SCRIPT`）多三件事：註冊 SW、送 `SAVE`、顯示結果。SW 負責存檔（先資產、後 HTML）、清掉沒人引用的資產、讀取時 network-first 並在 4 秒逾時後退回快取，以及處理 403。
- `verify-site --render` 新增離線案例，在用完即丟的瀏覽器 context 裡跑。這需要先在 gwebcdb `bridge-core` 加上 CDP 能力。

**Tech Stack:** Rust（workers-rs 0.8 wasm、travel-cli、gwebcdb bridge-core／chromiumoxide 0.9）、Service Worker／Cache API（JS）、`node --test`（Node 24，無依賴）、Turso（只讀既有欄位）。

**Spec:** `docs/superpowers/specs/2026-11-17-offline-dashboard-design.md`（§1–§12；§11 Yang 2026-11-17「照建議」：五題全部照建議）

## Global Constraints

- **SSR 優先**：關掉 JS 時頁面輸出與現在相同，只多 `hidden` 元素與 `data-*`。頁面上仍然只有一段 inline script（owner 頁另有既有的 copy-share 腳本）。唯一新增的 JS 是同源 `/sw.js`，這是 Yang 依 §11 Q1 核可的例外。不准有 `<script src`、外部腳本、manifest、推播。
- **介面文字全部放 `i18n.rs`**，ZH／EN 成對。`sw.js` 裡不准出現 CJK 字面值；備援頁文案在 worker 送出 `/sw.js` 時用 `i18n::fill` 填入（同 `render/share.rs:12`），而且要先做 JS 字串跳脫。
- **不加 feature flag**：回退方式是把 `/sw.js` 路由改送 kill-switch 後重新部署（T7），不加 env、不加常數開關。
- **不改資料表、不寫 DB**：`plan_documents.sha256 TEXT NOT NULL` 已經存在（`scripts/schema.sql:925`），不需要先跑 `db migrate`。
- **快取名稱固定為 `td-viewer`、`td-owner`、`td-index`**，名稱不帶 SW 版本；SW 的 `activate` 永遠不准刪這三個 cache。
- **常數**（spec 原值）：網路逾時 `4000` ms；票券版本鍵是 `sha256` 前 `12` 碼；鍵參數只保留 `plan`、`lang`、`token`，`lang` 缺省時視為 `zh`；owner 鍵去掉 `token`。
- **SW 只攔三種請求**：同源 GET 的計畫頁導航（`/?plan=`）、同源 GET 的 `/map/` 與 `/voucher/`。其他請求一律不呼叫 `respondWith`，包括非 GET、跨來源、`/auth/*`、`/grants/*`、`/diag/*`、`/notes*`、`/n/*`、`/`。唯一例外：`/auth/logout` 導航時，在 `waitUntil` 裡清 owner 副本，但仍不攔截請求本身。
- **帶 `?at=` 的請求，SW 完全略過**：不讀快取也不寫快取；inline script 也不送 `SAVE`。
- **inline script 維持 ES5**：既有測試禁止 `=>`、`let `、`const `、反引號、`innerHTML`。`sw.js` 可以用 ES2017（async/await），因為支援 SW 的瀏覽器都支援。
- **Worker 部署**：先 `cargo test -j 2` 並 build 成功，再 `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`。
- **派出去的 AI** 先讀 `~/c/agent-portal-tpm/dispatch/rules/agent-rules.md`。它們不跑 git、不派 subagent；commit 由整合者做。改 gwebcdb 的任務還要先讀 `~/b/gwebcdb/CLAUDE.md`。

## Review Focus

1. **票券連結的 `&`**：`esc_url_attr` 保留原始 `&`（既有測試 `esc_url_attr_preserves_ampersand_neutralizes_quotes` 釘住），SSR 實際輸出 `?v=…&token=…`。SW 擷取資產時也把 `&amp;` 還原成 `&`，兩種寫法都存到頁面會請求的同一個鍵（T3 測試 `save_keys_amp_escaped_voucher_link_like_raw`）。
2. **筆記上傳、grant 表單、OAuth、跨來源圖片**：非 GET 請求和清單外的路由（含 notes 分支的 `/notes*`、`/n/*` multipart POST），以及 ArcGIS 小圖、候選照片這類跨來源請求，都必須原樣直通。SW 不可以 `respondWith`，也不可以存不透明回應（不透明回應會吃掉配額）（T3 測試 `non_plan_requests_pass_through`）。
3. **沒有版本鍵的資產**：地圖 etag 是空的時候，`cache_bust` 不會加 `?v=`（`render/map.rs:498-506`）；飯店 `voucher_url` 也可能對不到 `plan_documents`。這兩種資產每次 SAVE 都要重抓；讀取時改用 network-first，不能 cache-first 永遠回舊檔（T3 測試 `unversioned_assets_refetched_and_network_first`）。
4. **線上但伺服器回 5xx**（Turso 掛掉、Cloudflare 錯誤頁）：旅途中有訊號、網站卻壞了，有副本就要給副本並顯示橫幅，不能只給錯誤頁。這是本計畫對 spec 未明說處的決定（T3 測試 `server_error_falls_back_to_cache`）。
5. **`verify-site` 在 Yang 的共用 Chrome 留下 SW、`td-*` cache 和存了即將刪除的 token 的副本**：layer 5 的所有分頁（含既有的排版檢查）都要開在用完即丟的 browser context，結束時銷毀（T5 測試 `isolated_context_disposed`；T6 執行前後比對 context 數量）。

---

## 檔案地圖

| 檔案 | 責任 | 任務 |
|---|---|---|
| `workers/trip-dashboard-rs/src/model.rs:331-361` | `PlanDocument.sha256` 與解碼 | T1 |
| `workers/trip-dashboard-rs/src/router.rs:1025` | plan_documents SELECT 補 `sha256` | T1 |
| `workers/trip-dashboard-rs/src/render/summary.rs:634-651, 1077-1090` | `voucher_href` helper；兩種票券連結共用 | T1 |
| `workers/trip-dashboard-rs/src/render/offline.rs`（新）、`render/mod.rs`（`pub mod`、`plan_body_attrs` `:61-64`、`render_plan_at` `:92`） | `view_of`、標頭、body 標記、狀態列／橫幅 | T2 |
| `workers/trip-dashboard-rs/src/render/day.rs:742-746` | 日卡 `data-date` | T2 |
| `workers/trip-dashboard-rs/src/router.rs:325-345, 816-820` | 計畫頁送 `X-TD-View`、改 `no-store` 註解、傳入 `rendered_at_ms` | T2 |
| `workers/trip-dashboard-rs/src/i18n.rs`（`:436` `_ => ""` 之前一整段）、`styles.css` | 離線文案、狀態列樣式 | T2 |
| `workers/trip-dashboard-rs/src/sw.js`、`sw_kill.js`（新）、`src/sw.rs`（新）、`lib.rs`、`router.rs:58` 之後 | `/sw.js` 路由、SW 本體、kill-switch | T3 |
| `workers/trip-dashboard-rs/tests/sw/harness.mjs`、`sw.test.mjs`（新） | SW 行為測試（假 Cache／fetch） | T3 |
| `workers/trip-dashboard-rs/src/render/mod.rs:299-391`（`BLOCK_MEMORY_SCRIPT` 與其文件註解）、`:1201-1260` 測試 | 註冊、SAVE、狀態、橫幅、過期 now 卡 | T4 |
| `~/b/gwebcdb/crates/bridge-core/src/cdp/mod.rs`、`tests/cdp_integration.rs` | 隔離 context、離線、延遲、攔截 | T5 |
| `rust/crates/travel-cli/src/verify_site_offline.rs`（新）、`verify_site.rs:650-662`、`verify_site_render.rs:546-637` | layer 5 離線案例、改用隔離 context | T6 |
| `docs/reference/dashboard.md`、`docs/reference/CLI.md`、`CLAUDE.md` Dashboard Rules | 例外規則、回退程序、出發前清單 | T7（整合者） |

## 派工與並行

- **第一組，可並行**：T1、T2、T3、T5，彼此沒有介面依賴。
  - 三個 worker 任務都會改 `router.rs`，但位置分開：T1 在 `:1025`，T2 在 `:325-345` 與 `:816-820`，T3 在 `:58` 之後。整合者依 T1 → T2 → T3 的順序合併；衝突只會出在 `lib.rs`／`render/mod.rs` 的 `mod` 宣告行。
  - T2 擁有 `i18n.rs` 的整段離線文案，包含 T3 要用的 `offlineMiss`。T3 不改 `i18n.rs`。
- **第二組**：
  - **T4** 在 T2 合併後開始，因為它要讀 T2 產生的 DOM。它只依賴 T3 的訊息協定（見 T3 Interfaces），程式碼可以和 T3 並行寫。
  - **T6** 在 T5 合併後開始。程式碼和單元測試可以先寫；線上執行要等 T1–T4 部署（T7 Step 3）。
- **T7** 由整合者做，要等全部合併。實機驗收是 Yang 的人工關卡。
- **派工**（依 `~/c/agent-portal-tpm/dispatch/README.md`；審查者不能是作者）：

| 任務 | 類別 | 建議實作 | 審查順序 |
|---|---|---|---|
| T1 | 前端（worker） | GLM 或 Sonnet 子代理（小單） | agy → Codex |
| T2 | 前端（worker） | agy | Codex → K3.1 Pro |
| T3 | 前端（SW，owner/viewer 隔離，重要單） | Codex | agy → K3.1 Pro（兩位都要審） |
| T4 | 前端（inline script） | Sonnet 子代理或 GLM（agy 若 T2 已完成也可以） | agy → Codex（作者是 agy 時改成 Codex → K3.1 Pro） |
| T5 | 後端（gwebcdb） | Codex | agy → K3.1 Pro（Muse 已在 11/13 到期） |
| T6 | 後端（CLI） | agy | Codex → K3.1 Pro |

- **跨計畫合併順序**（三份計畫都會改 `router.rs`／`render/mod.rs`／`i18n.rs`）：
  1. **`feat/notes-t5` 先合併**（未合併分支 `c68566c`）。它改了：
     - `router.rs:181` 一帶：加 early-return
     - `lib.rs`、`render/mod.rs`：加 `pub mod notes`
     - `i18n.rs`：加 6 行
     - `wrangler.toml`：新增 `NOTES` R2 binding
     **注意**：notes-t5 一旦進 master，從 master 部署之前就必須先建好 `trip-dashboard-notes` bucket（notes 計畫 T9），否則本計畫 T7 的 `wrangler deploy` 會失敗。
  2. **行程紀錄計畫的 worker 任務（`2026-11-16-trip-record.md` T6）第二個合併**。它改 `router.rs:866-901` 的 SELECT、`model.rs` 的 `Meal`／`Activity`／`RouteSegment`／`Session`、`render/day.rs:430-491`、`now.rs`、`session.rs`、`alerts.rs`、`i18n.rs`，和本計畫的位置分開（本計畫是 `model.rs:331`、`day.rs:742`、`router.rs:1025`）。
  3. **本計畫最後合併**：依 §11 Q5，急迫性最低。
  - 後合併者負責 rebase。
  - `i18n.rs` 三份計畫都把一整段插在 `_ => ""`（`:436`）之前，衝突時兩段都保留。
  - 筆記計畫 T8 也會改 `verify_site.rs`（在 plan 層之後呼叫 `verify_site_notes`），和本計畫 T6 的 `:650-662` 相鄰，後合併者 rebase。

---

### Task 1: 票券版本鍵（`PlanDocument.sha256`＋共用 `voucher_href`）

**Files:**
- Modify: `workers/trip-dashboard-rs/src/model.rs:331-352`（struct 加 `pub sha256: String`，`plan_documents()` 解碼 `s(row, "sha256")`）
- Modify: `workers/trip-dashboard-rs/src/router.rs:1025`（SELECT 補 `sha256`）
- Modify: `workers/trip-dashboard-rs/src/render/summary.rs`
  - `document_link`（`:634-651`）改呼叫新 helper
  - 飯店連結（`:1081-1090`）改呼叫新 helper；`booking_summary`（`:740`）已有 `plan`，傳 `&plan.documents`
- Test: `summary.rs`、`model.rs` 的 inline tests

**Interfaces:**
- Produces:

```rust
/// 本機 /voucher/ 連結的唯一產生點。path 以 "/voucher/" 開頭時：
/// 去掉前綴後等於某筆 documents[i].r2_key，且該筆 sha256 長度 ≥ 12 → 加 "v=<sha256[..12]>"；
/// 接著若 token 非空 → 加 "token=<t>"（v 在前、token 在後；第一個參數用 '?'，其後用 '&'）。
/// path 不以 "/voucher/" 開頭（外部 URL）→ 原樣回傳，不加 v 也不加 token。
pub(crate) fn voucher_href(path: &str, documents: &[crate::model::PlanDocument], token: Option<&str>) -> String;
```

- [ ] **Step 1: 寫失敗的測試**
  - `plan_documents_decodes_sha256`：row 含 `sha256`，解碼後欄位相同。
  - `voucher_href_versions_matching_document`：`r2_key="p-1/doc-flight.pdf"`、`sha256="0123456789abcdef…"`，token `t0k` → `/voucher/p-1/doc-flight.pdf?v=0123456789ab&token=t0k`。
  - `voucher_href_unmatched_hotel_has_no_v`：`/voucher/okinawa-2026/azat-voucher.pdf`，documents 是空的，token `t` → `…/azat-voucher.pdf?token=t`。
  - `voucher_href_short_or_empty_sha_has_no_v`：sha 是 `""` 或 `"abc"` → 不加 `v`。
  - `voucher_href_external_unchanged`：`https://example.com/x.pdf` 加 token → 原樣回傳。
  - `hotel_and_document_links_share_versioning`：同一個 `r2_key` 同時當 `plan_documents` 與飯店 `voucher_url`，兩個 href 完全相同，而且 HTML 裡都是 `?v=0123456789ab&token=t0k`（原始 `&`，見 Review Focus 1）。
  - 既有測試 `hotel_voucher_link_*`（`summary.rs:2353-2424`）、`day.rs:926`、`now.rs:816` 不改，照樣通過（它們的 sha 是空的）。
- [ ] **Step 2: 執行**：`cd workers/trip-dashboard-rs && cargo test -j 2 voucher_href`，預期 FAIL（找不到函式）。
- [ ] **Step 3: 實作** `voucher_href`，並讓 `model.rs`、`router.rs:1025`、兩種連結都改用它。
- [ ] **Step 4: 驗證**：`cargo test -j 2` 全部 PASS，worker build 成功。
- [ ] **Step 5: 整合者 commit**：`feat(worker): version local voucher links with the document sha`

### Task 2: 計畫頁的離線標記、狀態列與文案

**Files:**
- Create: `workers/trip-dashboard-rs/src/render/offline.rs`
- Modify: `render/mod.rs`：
  - 加 `pub mod offline;`
  - `plan_body_attrs`（`:61-64`）改用 `offline::view_of`，並加上新屬性
  - `render_plan_at`（`:92`）多一個參數 `rendered_at_ms: i64`，在 TOC 之前插入 `offline::status_bar(lang)`
  - `render_plan`（`:69-77`，僅測試用）傳 `0`
  - 既有 4 個呼叫端（`:1072`、`:1102`、`:1113`、`:1249`）同步更新
- Modify: `render/day.rs:741-746`：`<section class="day …">` 加 `data-date="<day.date>"`（日期是空字串時不輸出這個屬性）
- Modify: `router.rs`：
  - `:338-345` 傳入 `worker::Date::now().as_millis() as i64`，改用 `html_plan_page(body, view)` 送出
  - `:816-820` 一帶新增 `html_plan_page`
  - `:325-330` 註解改寫成：「`no-store` 只約束 HTTP 快取；`?at=` 頁不被存，是因為 SW 明確略過（`sw.js`）」
- Modify: `i18n.rs`：在 `:436` 之前加一整段（下列鍵）
- Modify: `styles.css`：`.offline-status`、`.offline-banner`（樣式沿用既有 summary-box 色票；`[hidden]` 不顯示）
- Test: `render/offline.rs`、`render/mod.rs`、`render/day.rs` 的 inline tests

**Interfaces:**
- Produces:

```rust
// render/offline.rs
pub fn view_of(owner_chrome: &str) -> &'static str;              // "" → "viewer"，否則 "owner"
pub fn plan_page_headers(view: &str) -> [(&'static str, String); 2]; // [("Cache-Control","private, no-store"),("X-TD-View",view)]
pub fn status_bar(lang: &str) -> String;                          // 下方 DOM 契約
// router.rs
fn html_plan_page(body: String, view: &str) -> Result<Response>;  // 套 plan_page_headers
```

- body 屬性：`data-plan-id`、`data-view`（既有），加上：
  - `data-rendered-at="<epoch ms>"`
  - `data-served="net"`
  - `data-tz="<plan.timezone>"`：timezone 是空的時候不輸出。spec §7 只有 now 卡帶 `data-now-tz`，行程期間以外也要顯示存檔時間，所以 body 也帶同一個值。
- DOM 契約（T4 讀取，T6 檢查）：

```html
<div class="offline-status" id="offline-status" hidden data-state=""
     data-l-saving="…" data-l-saved="…{maps}…{pdfs}…" data-l-partial="…{n}…"
     data-l-kept="…{time}…" data-l-failed="…" data-l-unsupported="…">
  <span class="offline-msg"></span><ul class="offline-failed" hidden></ul>
  <button type="button" class="offline-retry" hidden>重試/Retry</button></div>
<div class="offline-banner" id="offline-banner" hidden data-l-copy="…{time}…" data-l-relogin="…"></div>
```

- i18n 鍵（ZH 照 spec §7 原文；EN 由實作者翻譯，審查者核對）：
  - `offlineSaving`、`offlineSaved`、`offlinePartial`、`offlineRetry`、`offlineFailed`、`offlineCopy`、`offlineRelogin`、`offlineUnsupported`、`offlineMiss`
  - `offlineKeptOld`：「這次更新沒存完，離線時會看到 {time} 的版本」。這句取自 spec §6 步驟 5，§7 漏列，本計畫補上。

- [ ] **Step 1: 寫失敗的測試**
  - `plan_view_header_matches_body_data_view`：對 `""` 與 `"<div class=\"owner-chrome\"></div>"` 兩種 owner_chrome，`plan_page_headers(view_of(x))[1].1` 都等於整頁 HTML 裡 `data-view="…"` 的值。
  - `no_store_page_has_no_view_header`：`plan_page_headers` 以外的頁面（`html_no_store` 用的標頭清單）不含 `X-TD-View`。做法：把 `html_no_store` 的標頭抽成 `fn no_store_headers() -> [(&'static str, &'static str); 1]` 再測。
  - `plan_body_carries_offline_markers`：`render_plan_at(…, 1731800000000)` 輸出含 ` data-rendered-at="1731800000000" data-served="net"`；`plan.timezone="Asia/Tokyo"` 時含 ` data-tz="Asia/Tokyo"`，timezone 是空的時候不含 `data-tz`。
  - `day_card_carries_data_date`：`data-day="2" data-date="2026-11-14"`；日期是空的時候不輸出 `data-date`。注意 `day.rs:874` 的整段快照字串要一起更新。
  - `offline_status_served_hidden_with_templates`：zh 與 en 頁都含 `id="offline-status" hidden`、`id="offline-banner" hidden`，`data-l-saved` 含 `{maps}` 與 `{pdfs}`；ZH 頁含「已存好，可離線查看」，EN 頁不含任何 CJK。
  - `offline_i18n_keys_have_zh_and_en`：上列 10 個鍵，`t(k,"zh")` 和 `t(k,"en")` 都不是空字串。
  - 既有的 `block_memory_script_is_emitted_once_on_viewer_and_owner_pages` 仍然通過（狀態列不含 `<script`）。
- [ ] **Step 2: 執行**：`cargo test -j 2 offline`，預期 FAIL。
- [ ] **Step 3: 實作**（依 Interfaces；`plan_body_attrs` 的簽名改成 `(plan_id, owner_chrome, rendered_at_ms, tz)`）。
- [ ] **Step 4: 驗證**：`cargo test -j 2` 全部 PASS，build 成功；JS 關閉時頁面上看不到新元素（全部是 `hidden`）。
- [ ] **Step 5: 整合者 commit**：`feat(worker): offline markers, view header and hidden save status on the plan page`

### Task 3: `/sw.js`（存檔、讀取、隔離、清除、kill-switch）

**Files:**
- Create: `workers/trip-dashboard-rs/src/sw.js`、`src/sw_kill.js`、`src/sw.rs`
- Modify: `src/lib.rs`（`mod sw;`）
- Modify: `router.rs`：在 `/map/` 區塊（`:58-75`）之後加 `if path == "/sw.js"`，不需要 auth，也不讀 Turso
- Create: `workers/trip-dashboard-rs/tests/sw/harness.mjs`（假的 `caches`／`fetch`／事件；全域 `Request`／`Response`／`Headers` 用 Node 內建的）、`tests/sw/sw.test.mjs`
- Test: `sw.rs` inline tests；`node --test tests/sw/`

**Interfaces:**
- Consumes: T2 的 `X-TD-View`、`data-view`、`data-served="net"`、`offlineMiss`；T1 的 `?v=` 連結。
- Produces（Rust）：

```rust
pub const KEY_PARAMS: [&str; 3] = ["plan", "lang", "token"];
pub const CACHE_NAMES: [&str; 3] = ["td-viewer", "td-owner", "td-index"];
pub const NET_TIMEOUT_MS: u32 = 4000;
pub fn script() -> String;            // sw.js 填入 {tdMissZh}/{tdMissEn}（先做 JS 單引號字串跳脫：\ ' < 換行）
pub const KILL_SWITCH: &str;          // include_str!("sw_kill.js")
pub fn sw_headers() -> [(&'static str, &'static str); 2]; // Content-Type: application/javascript; charset=utf-8；Cache-Control: no-cache
```

- Produces（給 T4 的訊息協定）：
  - 頁面送出：`reg.active.postMessage({type:'SAVE', url: location.href}, [port2])`。
  - SW 在 `port1` 回傳以下其中一種：
    - `{status:'saved', maps:n, pdfs:n}`
    - `{status:'partial-new', maps:n, pdfs:n, failed:[url…]}`：原本沒有副本，或原本的副本不完整，就寫入這份不完整的新副本。
    - `{status:'partial-kept', failed:[url…], keptRenderedAt:ms}`：原本已有完整副本，舊 HTML 與舊 manifest 都不動。
    - `{status:'failed'}`：HTML 不是 200、缺 `X-TD-View`、`data-view` 不符，或網路錯誤；既有副本不動。若是帶 token 的 403，則照 403 規則清除。
- 儲存格式（SW 內部）：
  - `td-viewer`／`td-owner`：
    - HTML 存在鍵 URL 下：viewer 用 `/?plan=&lang=&token=`，owner 用 `/?plan=&lang=`（`lang` 一定寫出來）。
    - 資產以還原 `&amp;` 後的完整絕對 URL 為鍵。
    - manifest 存在 `/__td/manifest?k=<encodeURIComponent(鍵)>`，body 是 `{assets:[url…], complete:bool, renderedAt:ms}`。
  - `td-index`：以「正規化後的請求 URL」（保留三個參數）為鍵，body 是 `{role, key, savedAt}`。
  - 全部是 SW 端的 Cache API，不是 RDB。
- 讀取規則：
  - 計畫頁導航：
    - 網路回 200 → 原樣回傳。
    - 網路錯誤或超過 4000 ms → 查 `td-index`。有副本 → 把 `data-served="net"` 改寫成 `"cache"` 後回傳（橫幅用存檔時間）；沒有副本 → 回傳 `offlineMiss` 備援頁，語言依 URL 的 `lang`。
    - 線上但 5xx（網路有回應）：有副本 → 回傳副本（同樣改寫成 `"cache"`，以存檔時間標示）；沒有副本 → 把原本的 5xx 原樣透傳，不回 `offlineMiss`。
    - 帶 token 的 403 → 清除該 token（即使已逾時，也在 `waitUntil` 裡等原請求完成再清），回傳 403。
    - 不帶 token 的 403，且有 owner 副本 → 回傳副本，改寫成 `"cache-relogin"`；沒有副本 → 回傳 403。
  - 資產：
    - 有 `v=` → cache-first。
    - 沒有 `v=` → network-first，失敗再用快取。
    - 兩邊都沒有 → 原網路錯誤。
- `install` 呼叫 `skipWaiting()`，`activate` 呼叫 `clients.claim()`，兩者都不刪任何 cache。
- `sw_kill.js`：`install` 呼叫 `skipWaiting`；`activate` 刪除所有以 `td-` 開頭的 cache，再呼叫 `self.registration.unregister()`；不攔截任何 fetch。

- [ ] **Step 1: 寫失敗的測試**
  - Rust：
    - `script_fills_miss_text_both_langs_escaped`：輸出含 `offlineMiss` 的 ZH 與 EN 原文，不含 `{tdMiss`；用含 `'` 的假字串測 `js_str_escape`。
    - `script_constants_match_rust`：`sw.js` 含 `var KEY_PARAMS=['plan','lang','token'];`、`var NET_TIMEOUT_MS=4000;`，以及 `CACHE_NAMES` 的每個名稱（字面值由 Rust 常數組出）。
    - `sw_js_has_no_cjk`。
    - `kill_switch_unregisters_and_deletes_td_caches`：含 `unregister()`，以及 `indexOf('td-')===0` 或等價的寫法。
    - `sw_headers_no_cache_js`。
  - Node（`node --test tests/sw/`，假的 `fetch` 依 URL 回傳設定好的 Response、錯誤或延遲；計時器用 `node:test` 的 `mock.timers`）：
    - `save_writes_assets_manifest_html_index_in_order`：寫入順序要依序是資產 → manifest → HTML → index。
    - `save_rejects_missing_header_or_mismatched_data_view`：兩種情況都回 `failed`，既有副本不變。
    - `save_partial_keeps_complete_copy`（spec §8 1b 的邏輯）：
      - 第一次全部成功。
      - 第二次其中一張票券回 500 → 回 `partial-kept`，舊 HTML、manifest、舊票券都還在；新抓到的地圖也留在 cache。
      - 不跑 sweep。
    - `save_partial_writes_when_no_copy`：回 `partial-new`，manifest 的 `complete:false`。
    - `save_skips_cached_versioned_asset`：已經在 cache 的 `?v=` 資產不會再呼叫 fetch。
    - `unversioned_assets_refetched_and_network_first`（Review Focus 3）：沒有 `v=` 的地圖或票券，每次 SAVE 都重抓；線上讀取時拿到網路的新內容，離線時拿到快取。
    - `extracts_assets_unescaping_amp`（Review Focus 1）：HTML 裡是 `href="/voucher/p/doc-a.pdf?v=0123456789ab&amp;token=t"`，cache 鍵要是 `…?v=0123456789ab&token=t`。
    - `sweep_keeps_asset_referenced_by_other_manifest`：ZH、EN 與兩個 token 共用同一張地圖；其中一份重存後不再引用某張票券 → 只刪那張票券，共用地圖保留。
    - `owner_key_drops_token_and_index_finds_owner_copy`：owner 帶 token 的 URL 回 `X-TD-View: owner` 時，存在 owner 鍵下；離線用同一個 URL 找得到這份副本。
    - `timeout_serves_cache_marked_cache`：網路延遲 5000 ms 時，4000 ms 就回傳快取，內容含 `data-served="cache"`。
    - `late_403_with_token_purges`：網路 6000 ms 後才回 403，那時已經先回了快取；等原請求完成後，該 token 的 HTML、index、manifest、票券全都刪除，另一個 token 的副本不動。
    - `owner_403_without_token_serves_relogin_and_keeps_copy`。
    - `logout_navigation_clears_owner_only`：`/auth/logout` 不呼叫 `respondWith`；`td-owner` 清空，`td-index` 的 owner 項目也刪除，viewer 不動。
    - `at_param_bypassed`：帶 `&at=` 的導航不呼叫 `respondWith`；送出帶 `at` 的 SAVE 時回 `failed`，而且不寫入任何東西。
    - `non_plan_requests_pass_through`（Review Focus 2）：POST `/notes/x`、GET `/n/abc`、`/grants/create`、`/auth/callback`、`/`、`https://server.arcgisonline.com/…` 都不呼叫 `respondWith`，cache 也沒有新增任何項目。
    - `server_error_falls_back_to_cache`（Review Focus 4）：網路回 502 而且有副本時，回傳副本並標 `data-served="cache"`；沒有副本時原樣透傳 502（不回 `offlineMiss`，與讀取規則一致）。
    - `offline_miss_page_in_url_lang`：`lang=en` 時回 EN 文案，沒有 `lang` 時回 ZH。
    - `activate_never_deletes_td_caches`：`install` 呼叫了 `skipWaiting`，`activate` 呼叫了 `claim`，三個 cache 都還在。
    - `kill_switch_clears_and_unregisters`：載入 `sw_kill.js` 並觸發 `activate` 後，`td-*` 都不見了，`unregister` 被呼叫。
- [ ] **Step 2: 執行**：`cd workers/trip-dashboard-rs && cargo test -j 2 sw:: && node --test tests/sw/`，預期 FAIL。
- [ ] **Step 3: 實作** `sw.js`、`sw_kill.js`、`sw.rs` 與路由（依 Interfaces；`sw.js` 的鍵正規化只讀 `KEY_PARAMS`）。
- [ ] **Step 4: 驗證**：`cargo test -j 2` 與 `node --test tests/sw/` 都 PASS，build 成功。
- [ ] **Step 5: 整合者 commit**：`feat(worker): same-origin service worker with explicit save, owner/viewer isolation and kill switch`

### Task 4: inline script：註冊、SAVE、狀態、橫幅、過期 now 卡

**Files:**
- Modify: `workers/trip-dashboard-rs/src/render/mod.rs`：
  - `BLOCK_MEMORY_SCRIPT`（`:330-391`）在結尾 `})();` 前加第 4、5 項功能
  - 文件註解（`:299-329`）補第 4、5 點
  - `:1201-1260` 的測試補上斷言，並在註解寫明「頁面仍只有一段 inline script；`/sw.js` 是 Yang 2026-11-17 核可的同源例外」
- Test: `render/mod.rs` inline tests

**Interfaces:**
- Consumes：
  - T2 的 DOM 契約（`#offline-status`、`data-state`、`data-l-*`、`#offline-banner`、body 的 `data-served`／`data-rendered-at`／`data-tz`、日卡的 `data-date`）
  - T3 的訊息協定
- Produces（T6 依這些屬性判定）：
  - `#offline-status` 的 `data-state` 依序設成 `saving`、`saved`、`partial`、`kept`、`failed`、`unsupported` 其中之一，並且移除 `hidden`。
  - 從快取送出時，`#offline-banner` 移除 `hidden`。
- 行為：
  1. 不支援 SW：`!('serviceWorker' in navigator)`，或 `register('/sw.js')` reject → 顯示 `unsupported`。不做 UA 判斷。
  2. `data-served="net"`、而且 `location.search` 沒有 `at=` 時：
     - 先顯示 `saving`。
     - 在 `navigator.serviceWorker.ready` 之後，用 `MessageChannel` 送 `SAVE`。
     - 收到回覆後，依 status 填入對應文案。
     - 失敗清單的每一項：用 URL 找頁面上對應的 `a[href]` 取 textContent，或 `img[src]` 取 alt，用 `textContent` 寫入 `<li>`。
     - `partial-*` 與 `failed` 時，把重試鈕的 `hidden` 移除，按下就重送 `SAVE`。
     - 若 `navigator.storage && navigator.storage.persist` 存在就呼叫（不理會結果）。
  3. `data-served` 是 `cache` 或 `cache-relogin` 時：不送 SAVE，顯示橫幅。
     - `{time}` 用 body 的 `data-tz` 搭配 `Intl.DateTimeFormat` 格式化 `data-rendered-at`。
     - 沒有 `data-tz` 時，用去掉 `{time}` 的那句文案。不准猜時區。
  4. 過期 now 卡：`#sec-now` 存在、沒有 `data-now-pinned`，而且用它自己的 `data-now-tz` 算出的「今天」晚於 `data-now-date` 時：
     - 把 `#sec-now` 設成 `hidden`。
     - 打開 `section[data-date="今天"] > details[data-block]`。
     - 不再執行 `remark`。
     - 找不到今天的日卡時只隱藏 now 卡。
- `kept` 文案的 `{time}` 使用回覆裡的 `keptRenderedAt`。

- [ ] **Step 1: 寫失敗的測試**（擴充 `block_memory_script_is_emitted_once_on_viewer_and_owner_pages`，或新增同檔測試）
  - `script_registers_sw_feature_detected`：含 `'serviceWorker' in navigator`、`register('/sw.js')`。
  - `script_sends_save_only_when_net_and_unpinned`：含 `getAttribute('data-served')`、`type:'SAVE'`、`MessageChannel`、`/[?&]at=/`。
  - `script_sets_status_states`：`saving`、`saved`、`partial`、`kept`、`failed`、`unsupported` 六個字面值都出現；含 `storage.persist`。
  - `script_banner_uses_rendered_at_and_tz_no_guess`：含 `data-rendered-at`、`data-tz`、`Intl.DateTimeFormat`，不含 `324e5`、`+08`、`+09`。
  - `script_hides_stale_now_card_and_opens_today`：含 `data-date`、`data-now-date`、`.hidden=true`。
  - 既有斷言維持：ES5 禁字、不含 `innerHTML`／`insertAdjacentHTML`、viewer 頁 1 段 `<script>`、owner 頁 2 段、不含 `<script src`、`localStorage` 只出現 1 次。
- [ ] **Step 2: 執行**：`cargo test -j 2 block_memory_script`，預期 FAIL。
- [ ] **Step 3: 實作**：用 ES5，每個 SW、`Intl`、`storage` 呼叫都包在 try/catch 或 `.then(…, fn)` 裡，失敗時顯示 `failed` 或 `unsupported`，不能讓頁面其他功能中斷。
- [ ] **Step 4: 驗證**：`cargo test -j 2` PASS，build 成功。實際行為由 T6 layer 5 與 T7 實機驗收證明；這一步不宣稱瀏覽器行為已驗證。
- [ ] **Step 5: 整合者 commit**：`feat(worker): register the service worker, save on every online open, show offline status`

### Task 5: gwebcdb `bridge-core`：隔離 context、離線、延遲、攔截

**Files:**
- Modify: `~/b/gwebcdb/crates/bridge-core/src/cdp/mod.rs`
- Test: `~/b/gwebcdb/crates/bridge-core/tests/cdp_integration.rs`（`--features integration`，用測試自己啟動的 headless Chromium；本機 `127.0.0.1` 是 secure context，可以註冊 SW）

**Interfaces:**
- Produces（仍然 attach-only：不准 `Browser.close`，也不准把 `chromiumoxide::Browser` 交出去）：

```rust
impl BridgeBrowser {
    /// Target.createBrowserContext → createTarget(about:blank, browserContextId)。
    pub async fn new_isolated_page(&self) -> Result<IsolatedPage, CdpError>;
    pub async fn browser_context_count(&self) -> Result<usize, CdpError>; // Target.getBrowserContexts
}
pub struct IsolatedPage { /* page: BridgePage, context_id */ }
impl IsolatedPage {
    pub fn page(&self) -> &BridgePage;
    /// 對本頁 target 與同一 context 的每個 service_worker target 設定 Network.emulateNetworkConditions。
    pub async fn set_offline(&self, offline: bool) -> Result<(), CdpError>;
    pub async fn set_latency_ms(&self, ms: u64) -> Result<(), CdpError>;   // 同上，只設 latency
    /// 對同一 context 的 service_worker target 啟用 Fetch，URL 含 `needle` 的請求以 `status` 回應（Fetch.fulfillRequest）。
    pub async fn fail_sw_requests(&self, needle: &str, status: u16) -> Result<(), CdpError>;
    /// 在本頁每個新 document 之前注入時鐘覆寫（Page.addScriptToEvaluateOnNewDocument 覆寫 Date 與 Intl 預設時間，固定為 `epoch_ms`）；
    /// 只作用於本隔離 context 的頁面，不碰 SW。用來模擬「昨天存好、今天離線重開」。
    pub async fn set_page_clock_ms(&self, epoch_ms: i64) -> Result<(), CdpError>;
    pub async fn clear_overrides(&self) -> Result<(), CdpError>;
    /// 關閉分頁並 Target.disposeBrowserContext（清掉該 context 的 SW 與 Cache）。
    pub async fn dispose(self) -> Result<(), CdpError>;
}
```

- [ ] **Step 1: 寫失敗的測試**（integration；用 `std::net::TcpListener` 執行緒提供 `/`、`/sw.js`、`/probe`。測試用的 SW 一律走 `fetch(event.request)` 直通網路）
  - `isolated_context_offline_reaches_service_worker`：等 SW 開始控制頁面後 `set_offline(true)`，頁面內 `fetch('/probe')` 失敗，伺服器端沒有收到這個請求；`set_offline(false)` 後成功。
  - `latency_applies_to_service_worker`：`set_latency_ms(1500)` 之後，`/probe` 經 SW 的耗時 ≥ 1500 ms。
  - `fail_sw_requests_fulfills_status`：`fail_sw_requests("/probe", 500)` 之後，頁面拿到 500。
  - `page_clock_override_applies`：`set_page_clock_ms(T)` 後重新載入，頁面內 `Date.now()` 與 `new Date().getTime()` 皆在 `T` 起算 1000 ms 內，且 `new Date().toISOString()` 的日期等於 `T` 的日期；`clear_overrides` 後回到真實時間。
  - `isolated_context_disposed`（Review Focus 5）：`browser_context_count()` 在 `new_isolated_page` 前後各取一次，`dispose()` 之後要回到原值；另開一個 isolated page 時，`navigator.serviceWorker.getRegistrations()` 是空的。
- [ ] **Step 2: 執行**：`cd ~/b/gwebcdb && cargo test -j 2 -p bridge-core --features integration --test cdp_integration`（完整執行，含延遲與攔截測試，不加 `isolated` 過濾），預期 FAIL。
- [ ] **Step 3: 實作**：SW target 用 `Target.getTargets` 依 `browserContextId` 與 `type=="service_worker"` 篩選，再 `attachToTarget(flatten)`；方法要能處理「SW 在設定之後才啟動」的情況。做法是每次呼叫時重新列舉 SW target；T6 在 SAVE 完成後才會呼叫這些方法。
- [ ] **Step 4: 驗證**：上列 4 個測試與同檔其他既有測試全部 PASS（完整 `--test cdp_integration`）；`cargo test -j 2 -p bridge-core`（預設 features）仍然全綠。
- [ ] **Step 5: 整合者在 gwebcdb repo commit**：`feat(bridge-core): isolated browser context with offline, latency and SW request faults`

### Task 6: `verify-site --render` 離線案例

**Files:**
- Create: `rust/crates/travel-cli/src/verify_site_offline.rs`
- Modify: `verify_site_render.rs:546-637`：`render_plan` 的每個分頁改用 `new_isolated_page()`，結束時 `dispose()`；比對執行前後的 `browser_context_count()`，不一致就 WARN
- Modify: `verify_site.rs:650-662`：`render_dir` 有值時，在既有 layer 5 之後呼叫 `offline::run(conn, plan, host, token, &documents, tally)`，這時 token 仍有效；`USAGE`（`:68`）與模組註解（`:37-48`）補上離線案例說明
- Modify: `main.rs` 的模組宣告；`tests/cli_help_parity.rs` 維持綠燈
- Test: `verify_site_offline.rs` inline 純函式測試；線上執行在 T7 Step 3

**Interfaces:**
- Consumes: T5 全部；T2、T4 的 DOM 契約；`share_token::mint_for_plan`、`remove_verify_tokens`（`verify_site.rs:385`）、`redact`。
- Produces:

```rust
pub(crate) const LAYER: &str = "offline";
pub(crate) struct OfflineProbe { view: String, served: String, banner: bool, state: String,
    broken_maps: Vec<String>, vouchers: Vec<(String, bool)>, miss_page: bool }
pub(crate) const OFFLINE_PROBE_JS: &str; // 讀 body data-view/data-served、#offline-status[data-state]、#offline-banner、img[src^="/map/"] naturalWidth、逐一 fetch 每個 a.doc-link/a.voucher-link（本機）檢查 200 + %PDF 開頭；結果存 window.__tdProbe，再以 wait_for_function 取回（不依賴 evaluate 會 await Promise）
pub(crate) fn judge_first_visit(p: &OfflineProbe) -> Check;     // view=viewer、served=cache、banner、無破圖、每張票券 ok
pub(crate) fn judge_kept_after_partial(p: &OfflineProbe, saved_state: &str) -> Check; // saved_state=="kept" 且同上
pub(crate) fn judge_miss(p: &OfflineProbe) -> Check;            // miss_page 且無票券可開
pub(crate) async fn run(conn: &Connection, plan: &str, host: &str, token: &str,
    documents: &[PlanDocument], tally: &mut Tally);
```

- 案例（不帶 `?at=` 的分享網址，對應 spec §8）：
  1. `first-visit`：開新的隔離分頁 → 等到 `data-state` 是 `saved` → `set_offline(true)` → 重新載入 → `judge_first_visit`。
  2. `partial-keep`（1b）：在同一個 context 線上重開 → `fail_sw_requests("/voucher/", 500)` → 重新整理並等到 `data-state` 是 `kept` → 清除覆寫 → 離線重新載入 → `judge_kept_after_partial`。
  3. `multi-token`：`mint_for_plan` 第二個 token（`VERIFY_CREATED_BY`），ZH 與 EN 各自存好。
  4. `revoke`：用 `remove_verify_tokens(conn, Some(&token2))` 刪掉 token2 → 線上重開 token2 頁，應得到 403 → 離線開 token2 頁，用 `judge_miss` 判定；離線開 token1 的 ZH 與 EN，用 `judge_first_visit` 判定，共用地圖要還在。
  5. `late-403`：在新的 context 裡，用 token3 存好 → 刪掉 token3 → `set_latency_ms(6000)` → 重新載入：先看到 `served=cache`，等 ≥ 7 秒 → 離線重新載入，用 `judge_miss` 判定。
  6. `stale-day`（主要情境：昨天存好、今天離線重開）：
     - 先確認方案有一個日期 D（從 `plan_days` 取一天，且該日卡存在）。開新隔離分頁，用 `set_page_clock_ms(D 前一天 12:00，目的地時區)` 載入，等到 `data-state` 是 `saved`。
     - `set_offline(true)`，再 `set_page_clock_ms(D 當天 12:00，目的地時區)`，重新載入。
     - 斷言：(a) `data-served` 是 `cache`；(b) `section[data-date="D"]` 存在，且其 `details[data-block]` 有 `open`；(c) 日期 D 在頁面上有「今天」標記（既有 `data-today` 或 TOC／時間軸的今天標示）；(d) 若頁面有 `#sec-now` 且無 `data-now-pinned`，它必須是 `hidden`；(e) 其他日卡不得誤標今天（`[data-today]` 恰一個）。
     - 判定函式 `judge_stale_day(p)` 加入 `OfflineProbe`（新欄位 `today_marked`、`today_open`、`now_hidden`）；`OFFLINE_PROBE_JS` 同步讀取。
  - token2、token3 不論成功、FAIL 或出錯，都要在 `run` 的每一條離開路徑上刪除；刪除結果是 0 列或 1 列都算正常。token1 照現行規則，由 `verify_viewer_page` 刪除。
  - 每次輸出前都 `redact` 所有 token。每個 context 都在 `run` 結束前 `dispose`。

- [ ] **Step 1: 寫失敗的測試**（純函式，不需要瀏覽器）
  - `judge_first_visit_passes_clean_probe`、`judge_first_visit_fails_on_net_served`（`served=net` 表示根本沒離線，FAIL 訊息要說明是離線模擬沒生效）、`judge_first_visit_lists_broken_maps_and_bad_vouchers`。
  - `judge_kept_requires_kept_state`。
  - `judge_miss_fails_when_page_still_served`。
  - `judge_stale_day_requires_today_marked_open_and_now_hidden`：三項缺任一項都 FAIL，訊息指出缺哪一項。
  - `probe_output_redacts_tokens`：票券 href 含 token 時，Check 的 detail 只出現 fingerprint。
  - `offline_probe_js_reads_contract`：`OFFLINE_PROBE_JS` 含 `data-served`、`#offline-status`、`data-state`、`naturalWidth`、`%PDF`。
- [ ] **Step 2: 執行**：`cd rust && cargo test -j 2 -p travel-cli verify_site_offline`，預期 FAIL。
- [ ] **Step 3: 實作**。
- [ ] **Step 4: 驗證**：單元測試 PASS；`make build` 成功；`./bin/travel verify-site --help` 列出離線案例。
- [ ] **Step 5: 整合者 commit**：`feat(verify-site): offline cases in a disposable browser context`

### Task 7: 部署、回退演練、文件、實機驗收（整合者）

- [ ] **Step 1：人工關卡（Yang）**：確認現在沒有兩週內要出發、也沒有進行中的行程，或 Yang 明說可以在行程中部署（spec §11 Q5）。沒有確認就停在這裡。
- [ ] **Step 2**：
  - 確認合併順序已經完成（見派工與並行）。master 若已含 `NOTES` binding，先確認 `trip-dashboard-notes` bucket 存在。
  - 執行 `cd workers/trip-dashboard-rs && cargo test -j 2 && node --test tests/sw/`。
  - 執行 `unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`（wrangler 會先 build）。
  - 用 `curl -sI https://travel.ahexagram.com/sw.js` 確認 `Cache-Control: no-cache` 與 JS content-type。
  - 用分享連結執行 `curl -sI`，確認回應有 `X-TD-View: viewer`。
- [ ] **Step 3**：`make build && ./bin/travel verify-site --render <dir>`，`offline:*` 六個案例和既有的 layer 1–5 都要 PASS。分享連結所用的兩個網域都要跑；如果 verify-site 只打一個 host，另一個網域就在 Step 5 實機補驗。
- [ ] **Step 4：回退演練**：
  - 把 `router.rs` 的 `/sw.js` 改成送 `sw::KILL_SWITCH`，build 並部署。
  - 實機線上開一次，確認 SW 已註銷、`td-*` 已清（Chrome DevTools → Application；iPhone 則確認離線時已經打不開）。
  - 把 router 改回原樣，重新部署。這個改動不 commit。
- [ ] **Step 5：人工關卡（Yang 與一位同行者）實機入口驗收**（spec §8）：
  - 測試步驟：線上開啟 → 看到「已存好，可離線查看」而且沒有失敗項 → 開飛航模式 → 重開頁面，並逐張開票券。
  - 入口：iPhone Safari、iPhone 主畫面 app、Android Chrome、LINE →「用 Safari 開啟」。
  - iOS LINE 內建瀏覽器：只確認會出現 `offlineUnsupported` 提示。
  - owner 手測：
    - 開頁存好後離線開啟
    - 帶 token 的網址離線開啟
    - owner session 過期（8 小時後）時線上開啟，應仍看到副本與「需要重新登入」
    - 登出後 owner 副本應被清除
  - 每個入口記錄是否通過；沒通過的入口不對同行者宣稱支援。iOS web app 的 `target="_blank"` 票券若失敗，該入口改走 §12「票券存檔案」。
- [ ] **Step 6：文件**
  - `CLAUDE.md` Dashboard Rules 的 SSR-first 那一條，補上「同源 `/sw.js`（Yang 2026-11-17 核可）為唯一例外；頁面仍只有一段 inline script」。
  - `docs/reference/dashboard.md`：
    - `:28` 的 inline script 說明，補上第 4、5 項功能
    - 新增「離線」一節：快取規則、owner/viewer 隔離、撤銷的取捨、回退程序
    - 新增 spec §12 的出發前清單
  - `docs/reference/CLI.md`：補上 `verify-site --render` 的離線案例。
- [ ] **Step 7: 整合者 commit**：`docs(offline): service worker exception, rollback and pre-departure checklist`

## 審查核對紀錄

| # | 發現 | 審查者 | 處置 | 理由 |
|---|---|---|---|---|
| P1 | 主要情境（昨天存好、今天離線重開，今天被標記、日卡展開）沒有行為測試 | Codex 計畫審查 | 採用 | T5 加 `set_page_clock_ms`，T6 加 `stale-day` 案例與明確斷言。 |
| P2 | T3 規則（5xx 無副本回 `offlineMiss`）與測試（回原 502）矛盾 | Codex 計畫審查 | 採用 | 統一為：線上 5xx 有副本回副本、無副本透傳原 5xx。 |
| P3 | T5 驗證指令用 `isolated` 過濾，跳過延遲與攔截測試 | Codex 計畫審查 | 採用 | 改為完整 `--test cdp_integration`。 |
| P4 | `day.rs` 行號參照不準 | Codex 計畫審查 | 採用 | 改為 `:741-746`（已對照原始碼）。 |

### Yang 已確認（2026-11-17「同意」；原為計畫自行做的決定）

1. 跨來源圖片（ArcGIS 候選住宿小圖、候選照片）不快取，不在「已存好」的保證範圍；「已存好」訊息不得暗示這些圖片離線可看。
2. 線上但伺服器回 5xx 且有副本時，改回傳副本（標示存檔時間）；沒有副本則透傳原 5xx。這是本計畫對 spec 未明說處的決定。
