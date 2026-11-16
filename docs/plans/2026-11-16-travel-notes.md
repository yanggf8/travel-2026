# 旅行筆記（Travel Notes）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**可行性檢查（Yang 規則，2026-11-16）：** 兩個真實例子在 v1 都能完整走通。
- **GODIVA 門市考察**：要給女兒看。
- **燈飾遊戲參考**：給自己用。

走通的方式有兩條：
- AI 代理把整理好的標註卡片用 CLI 直接寫入（Yang：「本來就應該用 CLI」）。
- Yang 在手機網頁上新增、編輯筆記。

分享時只送出單則筆記的連結，拿到連結的人看不到行程和兌換券。v1 砍掉的項目（段落結構、活動連結、HEIC、標註產生器）都不影響這兩個例子。

**Goal:** 筆記是獨立的實體，可以掛在某一天，也可以不掛。照片會先清掉位置等 metadata 再存進私有 R2。每則筆記可以單獨分享、可以撤回。網頁和 CLI 都能寫入。

**Architecture:**
- **共用 crate `note-core`**：純 Rust、不依賴任何套件。內容是照片白名單重寫、id／token 格式驗證，以及筆記各表的欄位清單常數。worker 和 CLI 兩邊都用它，所以只有一份清理程式碼。
- **Worker**：
  - `turso.rs` 先補上綁定參數。
  - 筆記路由放在新模組 `notes.rs`，在 `router.rs:186` 載入 `plan_share_tokens` 之前 early-return。
  - 存取判斷用純函式 `decide_note_access`。
- **CLI**：
  - 寫入走 `travel-db` 的 `repo/notes.rs`。
  - 照片上傳前用 `note-core` 清理，再用 wrangler 上傳。
  - 不寫 plan audit triad，只寫 `note_events`。

**Tech Stack:** Rust（travel-cli edition 2024、travel-db、workers-rs 0.8 wasm）、Turso Hrana `/v2/pipeline`、Cloudflare R2、wrangler。

**Spec:** `docs/superpowers/specs/2026-11-15-travel-notes-design.md`（§2 目標、§4 反目標、§7 資料模型、§8 畫面、§9 CLI、§11 實作；§12 Yang 的回答：全部照建議，Q2 = v1 就有 CLI 寫入）

## Global Constraints

- **反目標 A1–A6（spec §4）是硬性規定。**
  - 筆記 token 不能開任何其他東西。
  - 照片不公開、不可列舉、不快取。
  - metadata 清不乾淨就拒收。
  - 筆記的讀寫一律綁定參數，禁止 `sql_quote` 或字串拼接。
- **id 與 token 格式：**
  - `note_id = "note_" + 16 個小寫 hex`
  - `image_id = 16 個小寫 hex`
  - 筆記分享 token = 32 個小寫 hex（同 `router.rs:575` `is_grant_token` 的格式）
  - 一律先用 `note-core` 驗證格式，才拿去查 DB。
  - `r2_key = "<note_id>/<image_id>.<jpg|png>"`，只從 DB 讀，絕不用網址拼。
- **audit**：筆記寫入不碰 `plan_events`、`plans.version`、`operation_runs`，只寫 `note_events(event_id, note_id, action, actor, detail, at)`。網頁和 CLI 用同一組 action 名稱：`create`、`update`、`delete`、`image_add`、`image_delete`、`image_move`、`share_create`、`share_deactivate`、`share_clear`、`purge`。
- **欄位名是 `attached_plan_id`，不叫 `plan_id`**，以免被 `copy-plan`、`cleanup-deleted`、`teardown_plan` 的動態掃表掃到。
- **R2**：私有 bucket `trip-dashboard-notes`，binding `NOTES`，只加在 `wrangler.toml` 頂層，不加在 `[env.production]`。
- **回應標頭**：分享頁與照片都要帶 `Cache-Control: private, no-store`、`X-Robots-Tag: noindex`、`Referrer-Policy: no-referrer`、`X-Content-Type-Options: nosniff`。照片另外帶 `Content-Disposition: inline`，以及 DB 裡的 `content_type`。
- **上傳**：單張上限 12 MiB（12_582_912 bytes，設計者在計畫中定案）。超過時，在呼叫 `form_data()` 之前就回 413。一次 POST 只收一張。只收 JPEG（`FF D8 FF`）和 PNG（`89 50 4E 47 0D 0A 1A 0A`）。
- **Yang §12 的決定**：
  - plan 被刪除時，筆記保留。
  - 刪除單張照片時，R2 物件立刻刪掉。
  - 筆記軟刪除 30 天後才清除。
  - 分享連結不設期限、可撤回，並提供一鍵停止所有分享。
  - 分享頁的日期只顯示到「日」。
- **不加** CHECK、JSON 欄位、feature flag；介面文字放 `i18n.rs`；SSR、無 JS 也能用（不加任何新 script）。
- 派出去的 AI 不跑 git、不派 subagent；commit 由整合者做。
- **R2 寫入一律帶 `--remote`**（比照 `plan_documents.rs:214-216` 的 put、`:269` 的 delete、`map_objects.rs:37`）。沒有 `--remote` 時 wrangler 寫的是本機模擬 bucket，網站讀不到。
- **`cargo test` 的整合測試不得上傳到正式 bucket**（CLI 用 `TRAVEL_NOTES_DRY_R2=1`，比照 `plan_documents.rs:209` 的 `TRAVEL_DOCUMENTS_DRY_R2`）。唯一例外是 T8 的 `verify-site` 線上驗證：它必須打正式網站與正式 bucket 才有意義（另開 bucket 就要另一個 binding 和部署，驗到的不是使用者用的那條路徑）。規則：暫時筆記的 `created_by='verify-site'`；清理放在每一條離開路徑（成功、FAIL、錯誤都要跑）；每次開跑前先掃除上一次殘留的 `created_by='verify-site'` 筆記與其 R2 物件。
- **R2 與 DB 的刪除順序**（CLI 與 worker 相同）：先從 DB 讀出 `r2_key` → 刪 R2（物件不存在視為成功）→ 再用一筆交易刪 DB 列並寫 `note_events`。R2 刪除失敗就不動 DB，key 留在 DB 裡，下次重試；絕不先刪 DB 列。
- **每個寫入都要確認真的改到列**：CLI 端 repo 回傳 `bool`／列數，呼叫端為 0 時報錯、不寫 `note_events`。worker 端同一個 pipeline 裡，`note_events` 用 `INSERT ... SELECT ... WHERE changes() = 1`，pipeline 結束後檢查這一句的 `affected_row_count`，為 0 時回 404（筆記不存在或已刪除）。

## Review Focus

1. **只有 EXIF、沒有 JFIF 的 iPhone JPEG，而且照片是直拍（Orientation=6）**：清理後要保留 Orientation=6，GPS 和 MakerNote 不能留下任何一個位元組（T2 fixture `iphone_portrait_gps.jpg`）。
2. **APP1 是 XMP 而不是 EXIF，或同時有多個 APP1／APP13（IPTC）**：要全部丟掉（T2 測試 `drops_xmp_and_iptc`）。
3. **PNG 的 `eXIf` chunk 放在 `IDAT` 之後**：也要丟掉，而且清理後的 CRC 要保持正確（T2 測試 `png_late_exif_dropped`）。
4. **擁有者的 session 過期（8 小時）後送出表單**：已輸入的標題和內文要出現在「登入已過期」頁的唯讀框裡，可以複製後重新登入再貼上（T6 `precheck_expired_refill_*`、`expired_refill_echoes_only_submitted_text`，T8 線上驗證）。
5. **分享頁內文含 `<script>`、`https://` 網址或 emoji**：要經 `esc()` 原樣顯示，不能自動轉成連結，emoji 往返不能壞（T1、T5 測試）。

---

## 檔案地圖

| 檔案 | 責任 | 任務 |
|---|---|---|
| `rust/crates/note-core/`（新：`Cargo.toml`、`src/lib.rs`、`src/scrub.rs`、`src/ids.rs`、`tests/fixtures/*`） | 照片白名單重寫、id／token 格式、欄位常數 | T2 |
| `rust/Cargo.toml`（workspace members）、`workers/trip-dashboard-rs/Cargo.toml`（path dep `../../rust/crates/note-core`） | 依賴 | T2 |
| `workers/trip-dashboard-rs/src/turso.rs` | 綁定參數、`affected_row_count` | T1 |
| `rust/crates/travel-cli/src/db_migrate.rs` | 4 張表 DDL | T3 |
| `rust/crates/travel-db/src/repo/notes.rs`（新）、`repo/mod.rs` | 筆記 DAL | T3 |
| `rust/crates/travel-cli/tests/common/mod.rs` | `teardown_notes(&ids)` | T3 |
| `rust/crates/travel-cli/src/notes.rs`（新）、`main.rs` | `notes`、`note show/add/update/delete/add-image/delete-image/purge-deleted`、`note-share` | T4 |
| `workers/trip-dashboard-rs/src/notes.rs`（新）、`render/notes.rs`（新）、`router.rs`（只加 early-return 一處）、`i18n.rs`、`lib.rs` | 路由、存取判斷、分享頁、照片、擁有者頁面 | T5, T6, T7 |
| `workers/trip-dashboard-rs/wrangler.toml` | `NOTES` binding | T5 |
| `rust/crates/travel-cli/src/verify_site.rs`（＋新 `verify_site_notes.rs`） | 線上反目標驗證 | T8 |
| `CLAUDE.md`、`docs/reference/dashboard.md`、`docs/reference/CLI.md` | 文件 | T9 |

## 派工與並行

- **第一波（三條同時開工，沒有共用檔案）**
  - **T1**（worker `turso.rs`）：實作 agy；審查 Codex → K3.1 Pro。
  - **T2**（`note-core`，安全性最重要）：實作 Codex。審查要兩位：agy 加 K3.1 Pro。這張單在 TODO 標記「待前沿補審」，Codex 有額度時再補審一次，前提是作者不是 Codex。
  - **T3**（schema 與 repo）：實作 Codex 或 agy；審查走後端順序。
- **第二波**
  - **T4**（CLI）：依賴 T2、T3。
  - **T5**（worker 讀取端）：依賴 T1、T2、T3。
  - T4 和 T5 可以並行，因為一個是 CLI、一個是 worker，沒有共用檔案。
- **第三波**
  - **T6**（擁有者寫入頁面）依賴 T5。
  - **T7**（照片寫入、分享流程、行程頁入口）依賴 T6。
  - T6、T7 都改 worker 的 `notes.rs`、`render/notes.rs`，**必須依序做**，同一家實作比較好。
- **T8**（verify-site）：程式碼依賴 T4、T5，可以和 T6、T7 並行撰寫；**線上執行**要等 T9 建好 bucket 並部署 T5–T7 之後，在 T9 Step 4 跑。
- **T9** 由整合者在全部合併後做（部署、文件），其中建立 R2 bucket 要先經 Yang 確認（spec §11.3）。
- **跨計畫**：與天氣、行程紀錄計畫互不相依，三份計畫可以同時進行。唯一共用的檔案是 `db_migrate.rs`、`main.rs`、`i18n.rs`、`router.rs`，誰後合併誰 rebase。

---

### Task 1: Worker Turso 綁定參數

**Files:**
- Modify: `workers/trip-dashboard-rs/src/turso.rs:52-110`（新增函式；`pipeline` 不動）
- Test: 同檔 inline

**Interfaces:**
- Produces:

```rust
pub enum Arg { Text(String), Int(i64), Null }
pub struct Stmt { pub sql: String, pub args: Vec<Arg> }
pub struct StmtResult { pub rows: Vec<Row>, pub affected: u64 }
/// Hrana 請求 body：每個 stmt 帶 "args":[{"type":"text","value":..},{"type":"integer","value":"<i64 as string>"},{"type":"null"}]
pub fn encode_pipeline_args(stmts: &[Stmt]) -> serde_json::Value;
pub fn decode_pipeline_results(json: &Value, n: usize) -> Result<Vec<StmtResult>, String>; // 讀 affected_row_count
pub async fn pipeline_args(turso_url: &str, token: &str, stmts: &[Stmt]) -> Result<Vec<StmtResult>>;
```

- [ ] **Step 1: 寫失敗的測試**
  - `encode_int_as_string`：`Arg::Int(42)` 要編成 `{"type":"integer","value":"42"}`。
  - `encode_text_roundtrip_quotes_emoji`：`"O'Brien; DROP TABLE x; ✈️"` 原樣出現在 `value` 裡，`sql` 字串裡沒有這段文字。
  - `encode_null`。
  - `decode_reads_affected_row_count`。
  - `decode_error_entry_fails_loud`：沿用 `decode_pipeline` 的錯誤語意。
  - 最後一個 request 一定是 `{"type":"close"}`。
- [ ] **Step 2: 執行**：`cd workers/trip-dashboard-rs && cargo test -j 2 turso::`，預期 FAIL。
- [ ] **Step 3: 實作**：`decode_pipeline_results` 的列解碼重用現有的 row decoder（`:30-46`）。
- [ ] **Step 4: 執行，確認 PASS**；worker build 成功。
- [ ] **Step 5: 整合者 commit**：`feat(worker): parameterized Turso statements with affected row counts`

### Task 2: `note-core` crate（照片清理、id、欄位常數）

**Files:**
- Create: `rust/crates/note-core/Cargo.toml`（`edition = "2021"`，沒有任何 dependencies）
- Create: `rust/crates/note-core/src/lib.rs`、`src/scrub.rs`、`src/ids.rs`
- Create: `rust/crates/note-core/tests/scrub.rs`、`tests/fixtures/`。fixture 用測試程式產生的合成位元組，不放真實照片：
  - `iphone_portrait_gps.jpg`：只有 EXIF（含 GPS IFD、MakerNote、Orientation=6），沒有 JFIF
  - `jfif_icc_xmp_iptc.jpg`
  - `truncated.jpg`
  - `late_exif.png`
  - `text_chunks.png`
  - `fake.heic`
  - `svg_as_png.png`
- Modify: `rust/Cargo.toml`（members 加 `crates/note-core`）、`workers/trip-dashboard-rs/Cargo.toml`（`note-core = { path = "../../rust/crates/note-core" }`）

**Interfaces:**
- Produces:

```rust
pub enum ImageKind { Jpeg, Png }
impl ImageKind { pub fn ext(&self) -> &'static str /* "jpg"|"png" */; pub fn content_type(&self) -> &'static str; }
pub fn sniff(bytes: &[u8]) -> Option<ImageKind>;
/// 白名單重寫。JPEG 保留 SOI/DQT/DHT/DRI/SOFn/SOS+熵編碼資料/EOI、APP0 JFIF、APP2 ICC_PROFILE；
/// 丟掉其他 APPn 與 COM；若原檔 EXIF 有 Orientation(0x0112) 且 ≠1，合成只含該 tag 的最小 APP1 EXIF（big-endian、單一 IFD0）。
/// PNG 保留 IHDR/PLTE/IDAT/IEND/tRNS/gAMA/cHRM/sRGB/iCCP/bKGD/pHYs/sBIT，丟其餘 chunk（含 eXIf/tEXt/iTXt/zTXt/tIME），CRC 原樣保留。
/// 任何結構無法解析 → Err（呼叫端拒收）。
pub fn scrub(bytes: &[u8]) -> Result<(ImageKind, Vec<u8>), ScrubError>;
pub enum ScrubError { UnsupportedFormat, Malformed(&'static str) }
// ids.rs
pub fn is_note_id(s: &str) -> bool;   // "note_" + 16 lowercase hex
pub fn is_image_id(s: &str) -> bool;  // 16 lowercase hex
pub fn is_note_token(s: &str) -> bool; // 32 lowercase hex
pub fn note_id_from_bytes(b: [u8; 8]) -> String;  // 亂數由呼叫端提供（worker getrandom js / CLI getrandom）
pub fn image_id_from_bytes(b: [u8; 8]) -> String;
pub fn token_from_bytes(b: [u8; 16]) -> String;
pub fn r2_key(note_id: &str, image_id: &str, kind: ImageKind) -> String;
pub const NOTE_PUBLIC_COLUMNS: &str = "id, title, body, place_name, taken_on"; // 分享頁唯一允許的欄位
```

- [ ] **Step 1: 寫失敗的測試**（`tests/scrub.rs`）
  - `sniff_magic_only`：PNG 檔頭加上 `.jpg` 副檔名，仍判為 Png；HEIC、SVG、HTML 回 None。
  - `iphone_portrait_keeps_orientation_drops_gps`：輸出不含 `GPS` IFD tag（0x8825）、不含 `Apple iOS` MakerNote 字串、不含 `Exif\0\0` 以外的 EXIF 欄位；從輸出解析 Orientation 要等於 6。
  - `drops_xmp_and_iptc`：輸出不含 `http://ns.adobe.com/xap/1.0/` 與 `Photoshop 3.0`；保留 ICC_PROFILE。
  - `jpeg_image_data_unchanged`：SOS 之後到 EOI 的位元組和原檔完全相同。
  - `png_late_exif_dropped`、`png_text_chunks_dropped`、`png_crc_valid`。
  - `truncated_rejected`：回 `Malformed`。
  - `ids_format`：大寫、長度錯、`note_` 前綴錯都回 false。
- [ ] **Step 2: 執行**：`cd rust && cargo test -j 2 -p note-core`，預期 FAIL。
- [ ] **Step 3: 實作**：只做位元組層級的 segment／chunk 走訪，不解碼影像。逐 segment 檢查長度欄位，任何越界都是 `Malformed`。
- [ ] **Step 4: 驗證**
  - `cargo test -j 2 -p note-core` PASS。
  - `cargo build -p note-core --target wasm32-unknown-unknown` 成功（worker 會用到）。
- [ ] **Step 5: 整合者 commit**：`feat(notes): note-core — fail-closed image metadata whitelist, id formats`

### Task 3: Schema 與 `travel-db` repo

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs`：4 張表用 `CREATE TABLE IF NOT EXISTS`，欄位完全照 spec §7，不加 constraint。
  - `notes`
  - `note_images`：`PRIMARY KEY (note_id, image_id)`
  - `note_share_tokens`：`status TEXT DEFAULT 'active'`
  - `note_events`
- Create: `rust/crates/travel-db/src/repo/notes.rs`；Modify: `repo/mod.rs`
- Modify: `rust/crates/travel-cli/tests/common/mod.rs`：新增 `pub fn teardown_notes(ids: &[&str])`，依序刪 `note_events`、`note_share_tokens`、`note_images`、`notes`。
- Test: `rust/crates/travel-cli/tests/notes_repo.rs`（新）

**Interfaces:**
- Produces（全部用綁定參數；`actor` 由呼叫端傳入）：

```rust
pub struct NoteInput<'a> { pub title: &'a str, pub body: &'a str, pub purpose: Option<&'a str>, pub place_name: Option<&'a str>,
    pub taken_on: Option<&'a str>, pub attached_plan_id: Option<&'a str>, pub attached_destination: Option<&'a str>, pub attached_day_number: Option<i64> }
pub struct NoteRow { pub id: String, pub title: String, pub body: String, pub purpose: Option<String>, pub place_name: Option<String>,
    pub taken_on: Option<String>, pub attached_plan_id: Option<String>, pub attached_destination: Option<String>, pub attached_day_number: Option<i64>,
    pub created_by: String, pub deleted_at: Option<String> }
pub struct ImageRow { pub image_id: String, pub sort_order: i64, pub r2_key: String, pub content_type: String, pub size_bytes: i64, pub caption: Option<String> }
pub async fn insert_note(conn, id: &str, n: &NoteInput<'_>, actor: &str, now: &str) -> Result<(), String>;
pub async fn update_note(conn, id: &str, n: &NoteInput<'_>, actor: &str, now: &str) -> Result<bool, String>;
pub async fn soft_delete_note(conn, id: &str, actor: &str, now: &str) -> Result<bool, String>;
pub async fn get_note(conn, id: &str) -> Result<Option<NoteRow>, String>;
pub async fn list_notes(conn, plan: Option<&str>, day: Option<i64>) -> Result<Vec<NoteRow>, String>; // 不含已刪除
pub async fn insert_image(conn, note_id: &str, img: &ImageRow, actor: &str, now: &str) -> Result<(), String>; // sort_order = MAX+1
pub async fn get_image_key(conn, note_id: &str, image_id: &str) -> Result<Option<String /*r2_key*/>, String>;
pub async fn delete_image_row(conn, note_id: &str, image_id: &str) -> Result<bool, String>; // 呼叫端先刪 R2 成功後才呼叫
pub async fn list_images(conn, note_id: &str) -> Result<Vec<ImageRow>, String>;
pub async fn insert_share_token(conn, note_id: &str, token: &str, actor: &str, now: &str) -> Result<(), String>;
pub async fn deactivate_share_tokens(conn, prefixes: &[&str], actor: &str, now: &str) -> Result<u64, String>;
pub async fn clear_share_tokens(conn, note_id: Option<&str>) -> Result<u64, String>; // None = 全部筆記
pub async fn list_share_tokens(conn, note_id: &str) -> Result<Vec<(String, String /*status*/, String /*created_at*/)>, String>;
pub async fn deleted_before(conn, cutoff: &str) -> Result<Vec<(String, Vec<String> /*r2_keys*/)>, String>;
pub async fn purge_note(conn, id: &str) -> Result<(), String>; // 硬刪 notes、note_images、note_share_tokens；note_events 不刪，呼叫端另寫一列 action='purge'
pub async fn insert_event(conn, note_id: &str, action: &str, actor: &str, detail: &str, now: &str) -> Result<(), String>;
```

- [ ] **Step 1: 寫失敗的測試**（真 Turso；note id 用 `note_` 加上由 nanos 推導出的 16 hex；建立 id 後立刻 arm `Guard` 呼叫 `teardown_notes`）
  - `insert_get_roundtrip_unicode`：標題 `GODIVA ✈️ 'quote'; --`，取回後完全相同。
  - `list_filters_plan_day_and_excludes_deleted`。
  - `image_sort_order_appends`。
  - `get_image_key_reads_db`、`delete_image_row_false_when_missing`。
  - `update_note_missing_or_deleted_returns_false`：不存在、已軟刪除的筆記，`update_note`／`soft_delete_note` 回 `false`。
  - `clear_share_tokens_all_vs_one`。
  - `deleted_before_cutoff`。
  - `migrate_idempotent`：連跑兩次 `db migrate`。
  - `teardown_plan_does_not_touch_notes`：用 `attached_plan_id` 指向測試 plan，跑完 `teardown_plan` 後筆記列仍然存在。
- [ ] **Step 2: 執行**（背景）：`cargo test -j 2 -p travel-cli --test notes_repo`，預期 FAIL。
- [ ] **Step 3: 實作**：每個多步驟寫入（例如 `purge_note`）由呼叫端包在 `begin_immediate`／`finish_tx` 裡。repo 函式本身不開交易，與現有 repo 的慣例一致。
- [ ] **Step 4: 驗證**
  - 測試 PASS。
  - `make build && ./bin/travel db migrate` 連跑兩次。
  - 重產 `scripts/schema.sql`。
  - `./bin/travel copy-plan`（在 `tests/copy_plan.rs`）綠燈。
- [ ] **Step 5: 整合者 commit**：`feat(notes): schema and travel-db notes repo`

### Task 4: CLI 指令

**Files:**
- Create: `rust/crates/travel-cli/src/notes.rs`；Modify: `main.rs`（加 arm，help 文字與 `cli_help_parity` 同步）
- Modify: `rust/crates/travel-cli/Cargo.toml`：加 `note-core = { path = "../note-core" }`
- Test: `rust/crates/travel-cli/tests/notes_cli.rs`（新；設 `TRAVEL_NOTES_DRY_R2=1`）

**Interfaces:**
- Consumes: T2 的 `scrub`、ids；T3 的 repo。
- Produces（輸出一律是純文字，不印 JSON；actor 一律寫 `'cli'`，比照 `share_token.rs:460-467` 的 `created_by`）：

```
travel notes [--plan-id <id>] [--day <n>]          # 不套用 resolve_plan_id；--day 無 --plan-id → 錯誤
travel note show <id>
travel note add --title "<t>" [--body "<t>" | --body-file -] [--purpose "<t>"] [--place "<t>"] [--taken-on YYYY-MM-DD]
                [--plan-id <id> --day <n> [--dest <slug>]]
travel note update <id> [same flags]               # 只改有給的欄位
travel note delete <id>                            # 軟刪除
travel note add-image <id> <path> [--caption "<t>"]
travel note delete-image <id> <image_id>           # 立刻刪 R2
travel note purge-deleted --older-than-days <n>    # Yang §12 Q4：30；需明確給值
travel note-share <id> [--show | --show-full] | deactivate <prefix...> | clear [<id> | --all]
```

  - `--body-file -` 從 stdin 讀內文。這是指令輸入，不算本地資料來源，因為內容會寫進 Turso。
  - `add-image` 的流程：
    1. 讀檔，經 `note_core::scrub` 清理；失敗就拒收，exit 1。
    2. 把清理後的位元組寫到 `std::env::temp_dir()` 下的暫存檔。
    3. 執行 `wrangler r2 object put trip-dashboard-notes/<r2_key> --file <tmp> --content-type <ct> --remote`，方式比照 `plan_documents.rs:208-220` 的 `wrangler_command()`。
    4. DB 寫入失敗時，再執行 `wrangler r2 object delete trip-dashboard-notes/<r2_key> --remote` 補償。
    5. 刪除暫存檔。
    - `TRAVEL_NOTES_DRY_R2=1` 時，只印 `dry-run: would upload <key>`。
  - `delete-image`：`get_image_key` → `wrangler r2 object delete ... --remote`（not found 視為成功）→ 交易內 `delete_image_row` + `note_events`。R2 失敗就 exit 1、DB 不動。
  - `purge-deleted`：逐則筆記先刪完它所有 R2 物件，全部成功才在交易內 `purge_note` + 事件；任一張失敗就跳過這則（列保留，下次重試），最後非零退出並列出跳過的筆記。
  - `update`／`delete`：repo 回 `false` 時 exit 1（「筆記不存在或已刪除」），不寫事件。
  - `--dest` 沒給時，用 `resolve_active_destination` 推出目的地。

- [ ] **Step 1: 寫失敗的測試**
  - `add_show_update_delete_flow`：每一步都斷言對應的 `note_events` 有一列，而且 `plans.version` 沒有變。
  - `add_image_rejects_heic`：exit 非 0，DB 沒有 image 列。
  - `add_image_dry_run_writes_row_with_db_key`：`r2_key` 的格式符合 `note-core::r2_key`。
  - `notes_day_without_plan_errors`。
  - `note_share_clear_all`：清除所有筆記的分享 token，plan 的分享 token 不受影響。
  - `purge_requires_days_flag`。
  - `update_missing_note_fails_no_event`：不存在的 id → exit 非 0，`note_events` 沒有新列。
  - `wrangler_args_include_remote`：把組 wrangler 參數抽成純函式 `r2_put_args`／`r2_delete_args`，斷言都含 `--remote`。
- [ ] **Step 2: 執行**（背景）：`cargo test -j 2 -p travel-cli --test notes_cli`，預期 FAIL。
- [ ] **Step 3: 實作**。
- [ ] **Step 4: 執行，確認 PASS**；`--test cli_help_parity` 綠燈。
- [ ] **Step 5: 整合者 commit**：`feat(notes): CLI — notes, note add/update/delete/add-image, note-share`

### Task 5: Worker 讀取端（隔離、分享頁、照片路由）

**Files:**
- Create: `workers/trip-dashboard-rs/src/notes.rs`、`src/render/notes.rs`；Modify: `lib.rs`／`render/mod.rs` 宣告模組
- Modify: `workers/trip-dashboard-rs/src/router.rs`：在 `:182`（`/grants/*` 區塊之後、`:184` 載入 `plan_share_tokens` 之前）只加一段：

  ```rust
  if notes::is_note_path(&path) {
      return notes::handle(req, env, &path, &query, is_owner_session, session_login, lang).await;
  }
  ```

  確切參數依 `router.rs` 當時的區域變數決定。
- Modify: `wrangler.toml`：頂層加 `[[r2_buckets]] binding = "NOTES" bucket_name = "trip-dashboard-notes"`
- Modify: `i18n.rs`：分享頁的介面文字
- Test: `notes.rs`、`render/notes.rs` 的 inline 測試

**Interfaces:**
- Consumes: T1 的 `pipeline_args`；T2 的 ids、`NOTE_PUBLIC_COLUMNS`。
- Produces:

```rust
pub fn is_note_path(path: &str) -> bool; // "/notes" 或 "/notes/..." 或 "/n/..."
pub enum NoteRoute { OwnerList, OwnerNew, OwnerEdit(String), OwnerPost(String /*action*/), SharePage(String), ShareImage(String, String) }
pub fn parse_note_route(path: &str) -> Option<NoteRoute>;  // id 格式不符 → None
pub enum NoteAccess { Allow, NotFound }
/// 純函式。row = 單次 SQL 的結果：token 狀態、token 所屬 note_id、筆記 deleted_at、(照片路由) note_images 是否存在。
pub fn decide_note_access(route: &NoteRoute, is_owner: bool, token: Option<&str>, row: Option<&AccessRow>) -> NoteAccess;
pub struct AccessRow { pub token_note_id: Option<String>, pub token_status: Option<String>, pub note_deleted: bool, pub image_exists: bool }
pub fn note_response_headers(h: &Headers, is_image: bool) -> Result<()>; // 依 Global Constraints
pub fn render_share_page(note: &PublicNote, images: &[(String /*image_id*/)], token: &str, lang: &str) -> String;
```

  - 擁有者以外的人打 `/notes*`，一律回 404。
  - 分享頁參數是 `?t=`，不讀 `?token=`。
  - 照片 SQL 一次查詢就要確認 token、筆記、`note_images` 三個條件，取得 `r2_key` 後才去讀 R2 的 `NOTES`。
  - 分享頁 SELECT 只用 `NOTE_PUBLIC_COLUMNS`；`taken_on` 只顯示到日。

- [ ] **Step 1: 寫失敗的測試**
  - `uniform_404`：下列情況都要回 `NotFound`：token 錯、token 已撤回、token 屬於別的筆記、筆記已刪除、筆記不存在、照片列不存在、id 格式錯。
  - `owner_without_token_allowed`。
  - `plan_query_token_ignored`：只帶 `?token=<有效的 plan token>`，沒有 `?t=`，要回 NotFound。
  - `parse_route_rejects_traversal`：`/n/../x`、`/n/note_xxx/img/../../a` 回 None。
  - `share_page_no_purpose_no_created_by`：輸出不含 purpose 字串和 GitHub login。
  - `share_page_escapes_and_no_autolink`：`<script>` 被跳脫；`https://x` 不包成 `<a`。
  - `share_page_noindex_meta`。
  - `headers_set`：用 `Headers::new()` 呼叫 `note_response_headers`，斷言四個標頭都在。
- [ ] **Step 2: 執行**：`cargo test -j 2 notes::`（這個 filter 同時涵蓋 `notes::` 與 `render::notes::`），預期 FAIL。
- [ ] **Step 3: 實作**：`router.rs` 只加 early-return 那一段，其他邏輯全部放在 `notes.rs`。
- [ ] **Step 4: 驗證**：`cargo test -j 2` PASS，build 成功；`git diff --stat` 顯示 `router.rs` 只增加少量行數。
- [ ] **Step 5: 整合者 commit**：`feat(notes): isolated note routes, share page and authenticated photo route`

### Task 6: Worker 擁有者頁面（清單、新增、編輯、刪除）

**Files:**
- Modify: `workers/trip-dashboard-rs/src/notes.rs`、`render/notes.rs`、`i18n.rs`
- Test: inline

**Interfaces:**
- Consumes: T5 的路由。`GrantCsrf` 的 HMAC 做法（`router.rs:602-640`）。
- Produces:

```rust
pub struct NoteCsrf { /* secret, session_cookie */ }
impl NoteCsrf {
    pub fn new(secret: &str, session_cookie: &str) -> Self;
    pub fn create(&self) -> String;                                   // 綁 "note:create:<session>"
    pub fn action(&self, action: &str, note_id: &str) -> String;      // 綁 "note:<action>:<note_id>:<session>"
    pub fn image(&self, action: &str, note_id: &str, image_id: &str) -> String;
    pub fn verify(&self, expected: &str, got: &str) -> bool;          // 常數時間比較
}
pub enum PostOutcome { Redirect(String), Refill { form: NoteForm, error_key: &'static str }, Status(u16) }
pub enum Precheck { Owner, ExpiredRefill, Reject(u16) }
/// 純函式。順序：owner → method → Content-Length → CSRF。
/// 非 owner 時：只有「POST + 帶著 session cookie（存在但驗證失敗，多半是 8 小時過期）+ 文字表單 action（create/update，不含照片上傳）+ Content-Length ≤ TEXT_FORM_MAX（64 KiB）」回 ExpiredRefill；其他一律 Reject(404)。
pub fn precheck(is_owner: bool, has_session_cookie: bool, action: &str, method_is_post: bool, content_length: Option<u64>, max: u64) -> Precheck;
pub fn decide_note_post(action: &str, form: &NoteForm, csrf_ok: bool) -> PostOutcome;
/// ExpiredRefill 頁：只把「這次 POST 送來的」標題與內文用 esc() 放進唯讀 textarea，附重新登入連結；不讀 DB、不寫 DB；帶 no-store、noindex 標頭。
pub fn render_expired_refill(form: &NoteForm, lang: &str) -> String;
```

  - 表單欄位：title、body、purpose、place_name、taken_on、`attached_plan_id`／`attached_destination`／`attached_day_number`，以及一次性的 `form_nonce`，用來防止重複送出。nonce 在 `note_events.detail` 裡記錄並查重。
  - 寫入時用 `pipeline_args`，在同一個 pipeline 裡依序送 `BEGIN`、內容寫入、`note_events`、`COMMIT`。
  - 刪除是軟刪除。

- [ ] **Step 1: 寫失敗的測試**
  - `precheck_order`：非擁有者回 404，不是 413；GET 回 405；`Content-Length` 超過上限回 413。
  - `precheck_expired_refill_only_for_text_post_with_cookie`：沒有 cookie → 404；有 cookie 但是照片上傳 → 404；有 cookie、文字表單、超過 64 KiB → 404；有 cookie、文字表單、大小合格 → ExpiredRefill。
  - `csrf_bound_to_action_note_session`：建立時的 token 不能用在編輯；另一則筆記的 token 不能用；另一個 session 不能用。
  - `expired_refill_echoes_only_submitted_text`：輸出含送出的標題與內文（`<script>` 被跳脫），不含任何 DB 內容（函式簽名就拿不到 DB）。
  - 實際路由由 T8 的 `verify-site` 線上驗證（專案沒有 worker HTTP 測試框架，spec §13 已決定不建）。
  - `create_redirects_to_edit`。
- [ ] **Step 2: 執行，確認 FAIL**。
- [ ] **Step 3: 實作**。
- [ ] **Step 4: 執行，確認 PASS**；build 成功。
- [ ] **Step 5: 整合者 commit**：`feat(notes): owner note list and editor (no-JS forms, CSRF, refill on expiry)`

### Task 7: Worker 照片上傳／刪除／排序、分享流程、行程頁入口

**Files:**
- Modify: `workers/trip-dashboard-rs/src/notes.rs`、`render/notes.rs`、`i18n.rs`
- Modify: `workers/trip-dashboard-rs/src/router.rs`：只在 `is_owner_session` 時另外查當天的筆記清單，渲染到行程頁那一天，不放進 `load_plan` 的 viewer pipeline（spec §8.1、Yang §10 Q3）。
- Test: inline

**Interfaces:**
- Consumes: T2 的 `scrub`；T6 的 `NoteCsrf`、`precheck`。
- Produces:
  - 上傳流程：`precheck` → `form_data()` → 取第一個 file → `note_core::scrub` → 失敗回 Refill 和錯誤訊息 → R2 `NOTES.put(r2_key, bytes)` → `pipeline_args` 寫 `note_images` 與 `note_events` → DB 失敗時 `NOTES.delete(r2_key)` 補償。
  - 刪除照片：從 DB 取 `r2_key` → `NOTES.delete` → 交易內刪列並寫事件（順序見 Global Constraints）；R2 失敗就回錯誤頁、列保留。
  - 上傳的 `<input type="file">` 必須帶 `accept="image/jpeg,image/png"`：iPhone Safari 從相簿選 HEIC 照片時，會自動轉成 JPEG 再送出；沒有這個屬性就會送 HEIC，被 scrub 拒收，手機上等於不能用。
  - 上移／下移：交換相鄰兩列的 `sort_order`，在同一個交易內完成。
  - 分享：
    - `POST share-preview` 回傳預覽頁，內容等同 `render_share_page`，再加一個確認按鈕。
    - `POST share-create` 鑄造 token，顯示可複製的連結（純文字 input，不加 JS）。
    - `POST share-deactivate` 撤回。
    - `POST share-stop-all` 撤回所有筆記的所有 token。
  - `pub fn owner_day_notes_block(notes: &[(String, String)], lang: &str) -> String`。

- [ ] **Step 1: 寫失敗的測試**
  - `upload_rejects_unscrubbable`：scrub 失敗時不會走到 R2 put。用純函式 `plan_upload(bytes) -> Result<UploadPlan, &'static str>` 測試。
  - `upload_plan_key_from_ids`。
  - `move_up_first_is_noop`。
  - `share_preview_equals_share_page`：預覽頁中的筆記主體，和 `render_share_page` 的輸出相同。
  - `viewer_plan_page_has_no_notes_block`：非擁有者的 render 不含筆記區塊的標記。
  - `upload_input_accepts_jpeg_png_only`：上傳表單的 file input 含 `accept="image/jpeg,image/png"`。
- [ ] **Step 2: 執行，確認 FAIL**。
- [ ] **Step 3: 實作**。
- [ ] **Step 4: 執行，確認 PASS**；build 成功。
- [ ] **Step 5: 整合者 commit**：`feat(notes): photo upload/delete/reorder, share preview and revoke, owner day entry`

### Task 8: `verify-site` 擴充（線上反目標驗證）

**Files:**
- Create: `rust/crates/travel-cli/src/verify_site_notes.rs`
- Modify: `verify_site.rs`：在 plan 層檢查之後呼叫。沿用「鑄造 → 檢查 → 刪除」的模式（`:169-191`、`:387`），`created_by='verify-site'`。
- Test: `rust/crates/travel-cli/tests/verify_site_live.rs`（既有；新增一個 case）

**Interfaces:**
- Consumes: T3 的 repo（建立暫時筆記與 token）、T4 的上傳與清理流程。測試照片用 `note-core` 測試 fixture 的 PNG 經 scrub 後的結果，以 CLI 的上傳路徑寫入正式 bucket（Global Constraints 的唯一例外，含開跑前掃除與每條離開路徑的清理）。
- Produces（每一項都是一行 PASS/FAIL）：
  - 無 session 用 `?token=<note-token>` 打 `/?plan=<plan>`：不能是 200 的 plan 頁。
  - 用 `?token=<note-token>` 打 `/voucher/<plan>/<file>`：不能是 200。
  - `/n/<別的筆記>?t=<token>`：404。
  - `/n/<note>/img/<別的筆記的 image>?t=<token>`：404。
  - plan 分享 token 用 `?t=` 打筆記：404。
  - 筆記與照片的四個回應標頭都正確。
  - 撤回 token 後，照片立即回 404；軟刪除筆記後，照片立即回 404。
  - 帶一個偽造（驗證不過）的 session cookie，POST 文字表單到 `/notes/new`：回應含送出的標題（已跳脫），DB 沒有新筆記列；不帶 cookie 的同一個 POST 回 404。
  - CLI 上傳後，擁有者路由讀得到這張照片（證明 `--remote` 寫到網站讀的 bucket）。
  - 結束時刪除暫時筆記的 R2 物件、所有列、token；中途失敗也要跑。

- [ ] **Step 1: 寫失敗的 case**：在 `verify_site_live.rs` 新增 `notes_anti_goals_pass`，斷言輸出含 `notes:` 開頭的各行，而且沒有 FAIL。
- [ ] **Step 2: 執行**（背景），預期 FAIL。
- [ ] **Step 3: 實作**。
- [ ] **Step 4**：線上執行移到 T9 Step 4（要等 bucket 建好、T5–T7 部署之後）。這個任務只交付程式碼與 `cargo build`。
- [ ] **Step 5: 整合者 commit**：`test(verify-site): live anti-goal checks for note sharing`

### Task 9: 部署與文件（整合者）

- [ ] **Step 1**：`make build && ./bin/travel db migrate`，連跑兩次；重產 `scripts/schema.sql`。
- [ ] **Step 2（人工關卡）**：先問 Yang 確認，再執行 `unset CLOUDFLARE_API_TOKEN && npx wrangler r2 bucket create trip-dashboard-notes`；不設公開網域。
- [ ] **Step 3**：build worker，然後 `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`。
- [ ] **Step 4**：`verify-site`，要全部 PASS。
- [ ] **Step 5a（手機網頁驗收，整合者）**：用 Playwright 的 iPhone 機型模擬（viewport、touch、UA），以擁有者身分走一次：新增筆記 → 上傳 JPEG 與 PNG 各一張 → 直拍照片方向正確 → 上下移 → 分享預覽 → 建立連結 → 無痕開啟 → 撤回 → 照片 404。實機（iPhone 從相簿選 HEIC 照片）由 Yang 回國後順手試一次即可，不擋上線。
- [ ] **Step 5**：實際測試：
  - 用 CLI 把 GODIVA 標註卡（`/tmp/claude-1000/-home-yanggf-b-travel-2026/443f38dc-308b-4e9a-88fd-e0b939c7c4fd/scratchpad/godiva/godiva-dessert-notes.png`）寫成一則筆記，掛在 kyoto-nov-2026 Day 4。
  - 用 `note-share` 產生連結。
  - 開無痕視窗確認只看得到這則筆記。
  - 連結先不送出，等 Yang 決定。
- [ ] **Step 6**：更新文件：
  - `CLAUDE.md`：「Rust binary is the sole write path」加上「worker 只可寫 `plan_share_tokens` 與 `notes*`」；DAL boundary 說明 worker 內聯 SQL 與 `travel-db` repo 對筆記表的分工，兩邊共用 `note-core` 的欄位常數。
  - `docs/reference/dashboard.md`：路由、分享。
  - `docs/reference/CLI.md`：所有 `note*` 指令。
- [ ] **Step 7: 整合者 commit**：`docs(notes): routes, sharing and CLI reference`


## 審查核對紀錄（2026-11-16 計畫審查）

審查者：Codex（block）、K3.1 Pro（block）。

| 發現（審查者） | 處理 | 理由 |
|---|---|---|
| CLI 的 `wrangler r2 object put/delete` 少 `--remote`（Codex blocker） | 採納 | `plan_documents.rs:214-216,269` 都帶；沒帶會寫到本機模擬 bucket。加入 Global Constraints、T4 指令、`wrangler_args_include_remote` 測試、T8 線上確認網站讀得到 |
| 過期 session 的回填被 owner 404 擋住，純函式測不到路由（Codex、K3 blocker） | 採納 | 改成：非 owner 但帶 session cookie 的文字表單 POST → 「登入已過期」頁，只回顯本次送出的文字（唯讀、不讀不寫 DB）；其他仍 404。路由由 T8 線上驗證；不建 worker HTTP 測試框架（spec §13 已決定） |
| T8 寫正式 bucket 與「測試不上傳正式 bucket」衝突；並行與部署順序錯（Codex、K3 blocker） | 採納順序；不採納另開 bucket | 另開 bucket 要另一個 binding 與部署，驗到的不是真正路徑。改成明文例外：開跑前掃除殘留、每條離開路徑都清理；線上執行移到 T9 部署後 |
| CLI 刪圖／purge 的 R2 與 DB 失敗順序未定，可能留下孤兒物件（K3） | 採納 | 統一為「讀 key → 刪 R2（不存在視為成功）→ 交易刪列＋事件」；purge 逐則處理、失敗跳過並非零退出 |
| 寫入沒有檢查真的改到列（K3） | 採納 | CLI 看 repo 回傳值；worker 用 `WHERE changes() = 1` 的事件 INSERT 判斷，0 列回 404 |
| 手機網頁沒有端到端驗收（K3） | 部分採納 | T9 加 Playwright iPhone 模擬全流程；實機 HEIC 由 Yang 回國後順手試，不擋上線。另加 file input 的 `accept`，讓 iPhone 自動把 HEIC 轉 JPEG（可行性關鍵） |
| `cargo test` 多個 filter 會報錯（K3） | 採納 | 改成單一共同 filter |
