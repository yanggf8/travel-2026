# 旅行筆記（Travel Notes）設計

- 狀態：Yang 已核可（2026-11-16，「都照建議」）；下一步寫實作計畫，計畫審過才派工。
- 日期：2026-11-15（kyoto-nov-2026 第 4 天，旅途中提出）

## 1. 為什麼要做

旅途中 Yang 會看到值得記下來的東西，記下來要給特定的人看，或留給自己之後用。2026-11-15 當天就有兩個實例：

1. **GODIVA dessert 門市小考察**（イオンモールKYOTO 櫻館 1F No.116）：Yang 拍了一張店面照，我們在照片上加編號標註和觀察，附上「給 DQ 夥伴的參考點」。讀者是在 DQ 打工的女兒。
2. **MOON LIGHT PROMENADE 燈飾**（同一個商場的戶外花園）：四張照片，加上觀察、遊戲點子和色票。用途是 Yang 自己要做的新遊戲的視覺參考。

Yang 的原話：

- 「這個筆記要可以能夠分享給別人比如說我女兒之類的」
- 「這個旅行筆記的話呢要能夠透過我的旅行網站來管理和分享」
- 「筆記應該當作一個新功能啊，你需要做一下設計…審之後…才能夠 AI 多工流開工」（語音輸入原文，意思是：先設計、審核，再派 AI 分工開工）

相關的構想見 memory `trip-log-feature-idea`：專案要能支援一份旅程紀錄，過了的時段清楚標示為「我們真實的行程」。筆記是這份紀錄的一部分，但兩件事分開做（見 §3）。

**成功的樣子**：

- Yang 在旅途中用手機打開旅行網站，就能新增一則筆記：上傳照片、寫文字、掛到某一天。
- 按「分享」會拿到一條連結，用 LINE 傳給女兒。
- 女兒打開連結只看得到這則筆記，看不到其他東西。


## 2. 目標

- **G1 一則一則的筆記**：每則有標題、內文（單一純文字欄）、一張以上的照片、地點名稱、拍攝日期，另有「用途/讀者」欄（擁有者自己的分類，例如「給女兒（DQ 參考）」、「遊戲參考」）。
- **G2 在網站上管理**：擁有者登入後（沿用現有 GitHub OAuth）新增、編輯、刪除筆記，上傳和刪除照片，用「上移／下移」調整照片順序（無 JS）。
- **G3 筆記可以掛到某一天**：可掛到某個 plan 的某一天，也可以不掛任何行程（Yang 決定，§10 Q1）。v1 不掛到活動（§3）。行程頁那一天顯示筆記入口，只有擁有者看得到（Yang 決定，§10 Q3）。
- **G4 單則分享**：每則筆記可產生自己的分享連結，可撤回。拿到連結的人只看得到這則筆記。分享前先顯示「對方看到的就是這頁」的預覽再確認。
- **G5 CLI 對等**：CLI 能新增、修改筆記，上傳照片，列出、讀取筆記，管理分享連結（Yang 2026-11-16：「本來就應該用 CLI」）。AI 代理整理好的筆記（含標註卡片）直接由 CLI 寫入。

## 3. 不做的事（非目標，v1）

- 不做計畫與實際的雙軌對照，也不做「已參觀/未前往」狀態。那是行程紀錄（`2026-11-16-trip-record-design.md`）的範圍。
- 不做多人協作、留言、按讚；不做公開索引頁。分享頁一律 `noindex`。
- 不做整組分享（Yang 決定，§10 Q2）。
- 不在網站上編輯行程。行程仍只用 CLI 寫。
- **審查後砍掉**（v1 不做，日後可加）：
  - 結構化段落 `note_sections`：無 JS 下增減段落要多次往返；GODIVA、燈飾兩例用單一內文加小標就夠。
  - 掛到活動 `activity_id`：worker 目前根本不讀活動 id（`router.rs:872-874`、`model.rs:36-42`），活動 id 也還不穩定。等行程紀錄完成再加。
  - `note_event_data` KV 子表：沒有讀者。只留單表 `note_events`。
  - 經緯度、照片寬高、sha256：分享頁不顯示座標；寬高要解析影像；sha256 在 Worker 上費 CPU。
  - 「分享時勾選是否顯示用途」：資料模型沒地方存，而且用途是擁有者自己的分類。v1 一律不顯示給檢視者。
  - 標註圖產生器（`travel note render`、SSR 疊圖）：標註卡片一律當成一般 PNG 上傳。
  - `/notes` 的篩選條件、HEIC、縮圖。

## 4. 絕對不能發生的事（反目標）

- **A1**：筆記分享連結不能打開行程頁、voucher PDF、其他筆記、其他筆記的照片，或任何別的東西。
  - 現況風險：單筆 plan 檢視只看 `?plan=`、不看路徑（`router.rs:282`），而 plan token 從 `?token=` 讀（`router.rs:197-203`）。所以筆記路由必須在載入 `plan_share_tokens`（`router.rs:186`）之前 early-return，全程不建立 `AccessScope`。
- **A2**：筆記頁不能帶出任何個人資料：訂位代號、護照、姓名、電話、卡號、LINE 連結、司機資訊、`created_by`（GitHub 帳號）、經緯度。
  - 分享頁只渲染筆記本身的欄位，不 join 行程的 booking、voucher、flight。
  - 內文與照片裡的個資無法自動擋；v1 的防線是「分享前預覽確認」（G4）。
- **A3**：照片不能變成公開或可列舉的網址，也不能被快取。照片一律經 worker 驗權才讀；R2 bucket 不設公開網域。
- **A4**：不能出現長期有效、整個 plan 範圍的 token。筆記 token 只對單一則筆記有效，可撤回。
- **A5**：分享出去的照片不能帶拍攝位置或其他 metadata。清不乾淨就拒收，不能放行再提示。
- **A6**：不能有 SQL 注入。筆記是第一條把自由文字寫進 DB 的網頁路徑；worker 的 Turso client 目前只送 SQL 字串、沒有綁定參數（`turso.rs:55-58`），現有寫入靠輸入只含安全字元（`router.rs:487-494` 的 SAFETY 註解）。
- **A7**：現有規則全部適用：DB 不加 CHECK 等 constraint；不用 JSON 欄位；不做 feature flag；內容放 Turso，介面文字放 `i18n.rs`。

## 5. 限制與現況（讀 repo 得到，審查已核對）

- **Worker**：`workers/trip-dashboard-rs`，Rust workers-rs，SSR，直接讀 Turso。沒有 HTTP 測試框架（無 `tests/` 目錄），只有純函式單元測試和打線上站的 `verify-site`。
- **擁有者登入**：GitHub OAuth，`__Host-td_session` cookie（SameSite=Lax，TTL 8 小時，`worker-github-oauth/src/session.rs:21`）。允許帳號 `ALLOWED_LOGIN = "yanggf8,lisyanggf"`。
- **現有網頁寫入**：`/grants/create|deactivate|deactivate-batch` 是擁有者專用 POST，用 `TURSO_WRITE_TOKEN`（整個 DB 的寫入權）和 `GrantCsrf`（HMAC，綁 session）。`req.form_data()` 會先把整份表單讀進記憶體。筆記是第一條寫**內容**、第一條收**檔案**的網頁路徑。
- **R2**：`MAPS`（地圖 PNG，`public, max-age=86400`，不驗權，`router.rs:68`）、`VOUCHERS`（PDF，驗 plan scope 後直接用路徑當 key 取物件，`router.rs:212-222`）。
- **分享 token 生命週期**：網頁「停止分享」是 UPDATE 成 inactive（`router.rs:505-513`）；CLI `share-token clear` 是 DELETE active＋inactive（`share_token.rs:57-67`，Yang 2026-11-02：deactivated row is worth nothing）。token 由 `getrandom` 產生 128-bit（`router.rs:566`），`is_grant_token` 只檢查格式（`router.rs:575`）。
- **前端腳本**：SSR 優先，頁面不開 JS 也要能用。
- **活動 id**：改標題只清 `poi_id`、不換 id（`travel-db/src/repo/itinerary.rs:417`）；id 會變是因為代理「刪除再新增」，以及 `replace_skeleton` 重建（`itinerary.rs:31`）。
- **以 `plan_id` 欄位動態掃表的三個機制**：`copy-plan`（單一 TEXT 主鍵又不在 generator 清單的表會 fail loud，`copy_plan.rs:466-472`）、`db cleanup-deleted`（硬刪該 plan 所有含 `plan_id` 的表）、測試 `teardown_plan`（`tests/common/mod.rs:16-23`）。
- **`days` 主鍵**：`(plan_id, destination, day_number)`（`scripts/schema.sql:318`），另有 `variant_of`。
- **回應標頭**：目前沒有任何 `Referrer-Policy`、`X-Content-Type-Options` 回應標頭。

## 6. Yang 已經決定的事

- **D1**：筆記是獨立的新功能，在網站上管理和分享（「筆記應該當作一個新功能」、「透過我的旅行網站來管理和分享」）。網站的寫入只限筆記相關資料（筆記、照片、筆記的分享連結）；行程維持 CLI 唯一寫入。
- **D2**：分享對象是特定的人，例如女兒，透過連結分享。
- **D3**：流程（2026-11-16）：設計 → 設計者自審 → 第三方審查 → 核對審查意見 → Yang 核可 → 實作計畫 → 派多個 AI 分工開工。
- **D4**：§10 的四個決定。
- **D5**：旅途中不動程式，回國後開工；旅行筆記是這批功能的優先項目（「特別是旅途筆記本功能」）。

## 7. 資料模型

所有表正規化、沒有 JSON 欄位、沒有 constraint。`db_migrate.rs` 只建 schema。

```
notes
  id TEXT PRIMARY KEY            -- note_<16 hex 隨機>，不含日期（設計者判斷，避免洩漏建立時間）
  attached_plan_id TEXT          -- 可為 NULL；故意不叫 plan_id，見下（設計者判斷）
  attached_destination TEXT      -- 可為 NULL；days 主鍵含 destination
  attached_day_number INTEGER    -- 可為 NULL
  title TEXT, body TEXT          -- 純文字；v1 不收 HTML、不自動連結
  purpose TEXT                   -- 用途/讀者，只給擁有者看
  place_name TEXT                -- 選填
  taken_on TEXT                  -- 拍攝日期 YYYY-MM-DD（目的地日期）
  created_at, created_by, updated_at, updated_by, deleted_at, deleted_by   -- 軟刪除

note_images
  note_id TEXT, image_id TEXT    -- PRIMARY KEY (note_id, image_id)；image_id 為隨機 hex
  sort_order INTEGER             -- 應用層重排，不進主鍵
  r2_key TEXT                    -- <note_id>/<image_id>.<jpg|png>，只從 DB 讀，不由網址組
  content_type TEXT              -- 由 magic bytes 決定
  size_bytes INTEGER, caption TEXT, created_at, created_by

note_share_tokens               -- 比照 plan_share_tokens
  token TEXT PRIMARY KEY, note_id TEXT, status TEXT DEFAULT 'active',
  created_at, created_by, deactivated_at, deactivated_by

note_events                     -- 單表 append-only 紀錄，網頁和 CLI 共用
  event_id TEXT PRIMARY KEY, note_id TEXT, action TEXT, actor TEXT, detail TEXT, at TEXT
```

- **為什麼用 `attached_plan_id`**（設計者判斷）：叫 `plan_id` 會被 `copy-plan`、`cleanup-deleted`、`teardown_plan` 三個動態掃表機制掃到。`copy-plan` 碰到有筆記的 plan 會直接失敗；`cleanup-deleted` 會硬刪筆記列、留下子表和 R2 孤兒。改名後筆記成為獨立實體，plan 被刪時筆記保留、只是掛載失效。這是資料壽命問題，列入 §12 Q1 請 Yang 確認。
- **audit**：筆記不改行程狀態，**不寫** `plan_events`、**不 bump** `plans.version`、不寫 `operation_runs`（它的 `plan_id`、`version_before` 是 NOT NULL，`db_migrate.rs:489,494`）。寫 `note_events`。這和現有 `share-token`、`domestic_accommodations`、`add-transit` 等非 plan 狀態的寫入不寫 triad 是同一類（審查指出不必當成「例外」）。
- **R2**：新 bucket `trip-dashboard-notes`，binding `NOTES`，只加在頂層（`[env.production]` 區塊已作廢，不加）。私有，不設公開網域。

## 8. 畫面與流程

### 8.1 路由與隔離

- 所有 `/notes*`、`/n/*` 路由在 `router.rs:186`（載入 `plan_share_tokens`）**之前** early-return，由獨立純函式 `decide_note_access` 判斷，不建立 `AccessScope`。
- 擁有者以外打 `/notes*`：回 404（不落到登入頁）。
- **統一 404**：token 錯、已撤回、筆記不存在、已刪除、token 屬於別的筆記，一律回同一個 404，不洩漏筆記是否存在。
- 行程頁的筆記清單：只在 `is_owner_session` 時另外查，不放進 `load_plan` 的 viewer pipeline。

### 8.2 擁有者（OAuth 登入）

- `/notes`：列出筆記。`/notes/new`、`/notes/<id>/edit`：一般 HTML form。
- **照片上傳一次一張**：`multipart/form-data`，`<input type=file accept="image/jpeg,image/png">`，不加 `capture`（`capture` 會強制開相機、不能從相簿選，兩個實例都是已拍好的照片）。要多張就重複上傳。
- **寫入前置檢查順序**：owner session → POST → `Content-Length` 上限（約 12 MB，超過回 413）→ 才呼叫 `form_data()` → CSRF → magic bytes／metadata 清理 → R2 put → DB 寫入。
- **CSRF**：比照 `GrantCsrf`。新建綁 `note:create:<session>`；其他動作綁 `note:<action>:<note_id>:<session>`，照片動作再綁 `image_id`。
- **寫入一致性**：同一次 Turso 交易寫內容與 `note_events`。R2 不在交易內：先 R2 put，DB 失敗就補償刪除 R2 物件；表單帶一次性 id 防重複送出。
- **session 8 小時過期**：表單送出被拒時，回填使用者已輸入的內文，不讓內容丟失（設計者判斷，實作細節留給建造者）。
- **分享流程**：按「分享」→ 先顯示分享頁預覽（就是對方會看到的內容）→ 確認才產生 token，顯示可複製的連結。「停止分享」按鈕撤回。

### 8.3 分享頁 `/n/<note_id>?t=<token>`

- 只顯示：標題、照片（依序）、內文、地點名稱、拍攝日期。不顯示 `purpose`、`created_by`、時間到分鐘。
- 參數名用 `?t=`，不用 `?token=`（與 plan token 分開）。驗證 SQL：`note_share_tokens JOIN notes`，帶 `status='active' AND deleted_at IS NULL AND note_id=?`。
- 回應標頭：`Cache-Control: private, no-store`、`X-Robots-Tag: noindex`、`Referrer-Policy: no-referrer`、`X-Content-Type-Options: nosniff`。
- 內文不自動連結（避免 `?t=` 經 Referer 外流）。
- 預設中文，`?lang=en` 只切換介面文字。
- LINE 會抓連結預覽，標題會出現在預覽裡（§12 Q5）。

### 8.4 照片路由 `/n/<note_id>/img/<image_id>?t=<token>`

- 一次查詢確認：token 有效且屬於這則筆記、筆記未刪除、`note_images` 有 `(note_id, image_id)` 這一列。通過後**只用 DB 裡的 `r2_key`** 讀 R2，不把網址拼成 key（不能照抄 `/voucher` 的做法）。
- 回應：`Cache-Control: private, no-store`、DB 裡的 `content_type`、`X-Content-Type-Options: nosniff`、`Content-Disposition: inline`、`Referrer-Policy: no-referrer`。
- 擁有者有 session 時不需要 token。

### 8.5 照片處理（fail-closed）

- 只收 JPEG、PNG，以 magic bytes 判斷（`FF D8 FF`、`89 50 4E 47`），不信瀏覽器 MIME。拒收 HEIC、SVG、HTML 等。
- **metadata 白名單重寫**，不是「刪掉 GPS」：
  - JPEG：只保留 SOI、DQT、DHT、SOF、SOS、影像資料、EOI，以及 JFIF（APP0）與 ICC（APP2 ICC_PROFILE）；丟掉其他 APPn（EXIF、XMP、MPF、IPTC、MakerNote、內嵌縮圖）。另外合成一個只含 Orientation 的最小 EXIF，避免直拍照片橫躺。
  - PNG：丟 `eXIf`、`tEXt`、`iTXt`、`zTXt`、`tIME`。
  - 解析失敗、格式不認得：**拒收**，回錯誤訊息。不保存未清理的原檔。
- 純位元組處理，不解碼影像，不做縮圖。顯示用 `<img loading=lazy>` 加 CSS。
- 清理邏輯放在 CLI 與 worker 共用的 crate（計畫中稱 `note-core`）；網頁上傳與 CLI `note add-image` 兩條路徑都必須先經過它，沒有繞過的上傳路徑（§9，Yang 2026-11-16 決定 v1 含 CLI 寫入）。
- 刪除照片時同步刪 R2 物件；筆記軟刪除時照片保留到最終清理（§12 Q4）。

### 8.6 與其他功能的關係

- **行程紀錄**：活動 id 穩定後，再加 `attached_activity_id`。
- **敏感資訊遮罩**：Yang 已於 2026-11-16 取消這個功能（同行夥伴需要訂位資訊，分享連結管理就夠）。筆記分享給非同行者時，靠分享前預覽確認。

## 9. CLI 對等

- `travel notes [--plan-id] [--day]`：純文字表格列出筆記。沒有 selector 時列出全部，包含不掛行程的；`--day` 沒有 plan 時報錯。不套用 `resolve_plan_id` 的預設推論。
- `travel note show <id>`：顯示一則筆記。
- `travel note-share <id> [--show | --show-full] | deactivate <prefix...> | clear [--all]`：語意比照 `share_token.rs`：`deactivate` 改狀態，`clear` 硬刪 active＋inactive，`clear --all` 一步停止所有筆記分享。
- `travel note add` / `note update <id>` / `note delete <id>`、`travel note add-image <id> <path>`（Yang 決定，§12 Q2）：
  - 照片在 CLI 端先經共用 crate 的 metadata 白名單重寫（與 worker 同一份程式碼，同一組測試），清不乾淨就拒收，之後才上傳 R2；不得沿用 `plan_documents.rs:208-214` 直接把原檔交給 wrangler 的做法。
  - 寫入走 `travel-db` repo（DAL boundary），與 worker 的 SQL 共用同一份欄位定義；文字一律綁定參數。
  - 每次寫入記一列 `note_events`，與網頁路徑相同。
- 所有 CLI 輸出是純文字，不輸出 JSON。

## 10. Yang 的決定（2026-11-15，「先按你的建議」）

- **Q1 筆記可以不掛行程**：可以。例如回國後補寫的遊戲參考。
- **Q2 整組分享**：v1 不做，只做單則分享。
- **Q3 行程分享連結不連帶顯示筆記**：拿行程分享連結的人看不到筆記；筆記一律另外分享。
- **Q4 兩個帳號都是擁有者**：`yanggf8`、`lisyanggf` 都能新增和編輯，`created_by` 記錄作者。

## 11. 實作、測試、部署（給建造者）

### 11.1 前置工作（開放任何筆記寫入之前）

- `turso::pipeline` 加綁定參數（Hrana `stmt.args`，整數以字串傳），並檢查 `affected_row_count`。筆記的所有讀寫一律綁定參數，禁止 `sql_quote` 拼接；id、token 另做格式白名單。

### 11.2 測試

- **純函式單元測試**：`decide_note_access`（每種 token 狀態都回同一個 404）、CSRF、metadata 白名單重寫（帶 GPS、XMP、Orientation、截斷檔、偽造 MIME 的 fixture，斷言輸出位元組不含 GPS IFD 與 XMP）、綁定參數對 `'`、`;`、emoji 的往返。
- **Turso 整合測試**（CLI 端，背景跑）：fixture 用 `zz_test_<nanos>_*`，RAII Guard。新增 `common::teardown_notes(&ids)`，在 `teardown_plan` 之前刪子表與事件。測試不得上傳到正式 bucket。
- **線上驗證**：擴充 `verify-site`，沿用它「鑄造 token → 檢查 → 刪除」的模式：
  - 反目標：無 session 時，以 `?token=<note-token>` 打 `/?plan=`、`/voucher/...`，以 `?t=` 打別的筆記與別的筆記的照片，全部不得成功。
  - plan token 打筆記頁不得成功；撤回、刪除後照片立即失效。
  - 回應標頭符合 §8.3、§8.4。

### 11.3 部署順序

1. `./bin/travel db migrate` → 重新產生 `scripts/schema.sql`（不可手改）。
2. 建私有 R2 bucket `trip-dashboard-notes`（要 Yang 確認後才建），頂層加 `NOTES` binding。
3. `make build`，worker build。
4. `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`。
5. `verify-site` 上線驗證。
- 順序反了，`decode_pipeline` 遇到 no such table 會讓擁有者頁面 500（`turso.rs:95-99`）。

### 11.4 要同步更新的文件

- CLAUDE.md：「Rust binary is the sole write path」加上「worker 只可寫 `plan_share_tokens` 與 `notes*`」；DAL boundary 說明 worker 內聯 SQL 與 `travel-db` repo 對筆記表的分工。
- `docs/reference/dashboard.md`（路由、分享段落）、`docs/reference/CLI.md`（`notes`、`note show`、`note-share`）。

### 11.5 留給建造者

- Workers 方案（Free/Paid）決定的 CPU 上限細節；`Content-Length` 上限的精確值。
- 無 JS 的「上移／下移」排序 UI。
- iOS Safari 選 HEIC 照片時是否自動轉 JPEG，要實機確認。

## 12. Yang 的回答（2026-11-16）

**Yang 決定（2026-11-16）：全部照建議（「都照建議」）。**


1. **plan 被刪除時，筆記怎麼辦？** 建議：筆記保留、只解除掛載（所以欄位叫 `attached_plan_id`）。
2. ~~v1 要不要讓 AI 代理用 CLI 直接新增筆記和上傳照片？~~ **Yang 決定（2026-11-16）：要，「本來就應該用 CLI」。** v1 的 CLI 包含 `note add/update/add-image`；照片的 metadata 清除放在 CLI 與 worker 共用的 crate，兩條上傳路徑都必須經過它（對應 Fable#2 的繞過風險）。
3. **照片清不乾淨位置資訊時直接拒收，可以嗎？只收 JPEG/PNG、不收 HEIC 可以嗎？** 建議：兩個都可以。
4. **刪除筆記或照片時，雲端原檔要立刻刪嗎？** 建議：刪單張照片立刻刪；刪整則筆記保留 30 天後清除。
5. **分享頁的標題會出現在 LINE 預覽，可以接受嗎？日期只顯示到「日」可以嗎？** 建議：都可以。
6. **筆記分享連結要不要有期限？** 建議：不設期限、隨時可撤回，並提供「一鍵停止所有筆記分享」。

## 13. 審查核對紀錄（2026-11-16）

審查者：agy（Gemini 4 Argon Medium，前端面）、K3.1 Pro（後端面）、Fable 5.2（加強審查，與設計稿同屬 Anthropic，不當唯一第三方）。每條都已對照原始碼核實。

| 議題 | 誰提出 | 處理 | 理由 |
|---|---|---|---|
| Turso 無綁定參數，自由文字寫入＝注入 | agy#1、K3#2、Fable#1 | 採納（§4 A6、§11.1） | 三方一致；`turso.rs:55-58` 確認 |
| metadata 刪不乾淨的退路會洩漏 GPS | K3#1、Fable#2、agy#3 | 採納（§8.5） | 三方一致；改白名單重寫＋fail-closed＋保留 Orientation |
| 照片路由要查 `note_images` 列、key 只從 DB 讀 | agy#4、K3#4、Fable#3 | 採納（§8.4） | 三方一致；`/voucher` 直接用路徑當 key（`router.rs:218`） |
| 照片快取、nosniff、Referrer-Policy | agy#4/#5、K3#7、Fable#3/#4 | 採納（§8.3、§8.4） | 目前無這些回應標頭，已 grep 確認 |
| 筆記路由在載入 plan token 前 early-return | agy#5、Fable#6 | 採納（§8.1） | `router.rs:186`、`:282` 確認 |
| 統一 404，不洩漏存在 | Fable#5 | 採納（§8.1） | 原稿 403/404 混用 |
| 擁有者才查筆記，不進 viewer pipeline | agy#14、Fable#7 | 採納（§8.1） | 同時避開部署順序風險 |
| 一次一張＋`form_data()` 前檢查 `Content-Length` | agy#6/#13、K3#6、Fable#17 | 採納（§8.2） | 三方一致；`form_data()` 全讀進記憶體 |
| CSRF 綁 note id 對新建不可行 | agy#6、K3#8、Fable#9 | 採納（§8.2） | 改綁動作＋session |
| `note_images` 主鍵改穩定 image_id | agy#15、K3#9、Fable#19 | 採納（§7） | 三方一致 |
| `plan_id` 被 copy-plan／cleanup／teardown 掃到 | agy#9、K3#12、Fable#16 | 採納（§7，選改名；§12 Q1） | `copy_plan.rs:466-472` 確認 |
| 砍 note_sections、activity_id、event KV、lat/lon、寬高、標註產生器、篩選 | agy#16-19、K3#15、Fable#20 | 採納（§3） | 三方一致 |
| 用途勾選無欄位可存 | agy#11、K3#9、Fable#20 | 採納（v1 一律不顯示） | — |
| 拿掉 `capture` | agy Q3、Fable#18 | 採納（§8.2） | 兩個實例都是已拍好的照片 |
| HEIC v1 不收 | agy#13、K3#1、Fable#18 | 採納（§8.5，§12 Q3 確認） | — |
| 遮罩套不到不掛行程的筆記 | agy#2、K3#5、Fable#14 | 採納（§8.6，v1 改預覽確認） | 遮罩功能已取消（Yang 2026-11-16） |
| 撤回語意：網頁 UPDATE、CLI clear 是 DELETE | agy#8、K3#11、Fable#10 | 採納（§5、§9） | `share_token.rs:57-67` 確認 |
| 活動 id 會變的原因敘述不精確 | agy#7、Fable#11 | 採納（§5） | `itinerary.rs:417` 確認 |
| audit 不必當「例外」 | agy#10 | 採納（§7） | 與 share-token 等同類 |
| 測試：沒有 worker HTTP 框架 | Fable#13、K3#12 | 採納（§11.2 改 verify-site） | 無 `tests/` 目錄確認；K3 要求的 preview worker HTTP 測試**不採納**，理由：專案沒有這套框架，verify-site 已能打線上路由 |
| 部署順序、binding 只加頂層 | agy#12/#14、K3#13、Fable#19 | 採納（§11.3） | — |
| 文件同步清單（DAL、dashboard.md、CLI.md） | agy#10、Fable#15 | 採納（§11.4） | — |
| `days` 主鍵含 destination | Fable#19 | 採納（§7 加 `attached_destination`） | `schema.sql:318` 確認 |
| CLI 寫入繞過 metadata 清理 | Fable#2 | 採納（§9 CLI 與 worker 共用清理 crate；Yang 決定保留 CLI 寫入） | `plan_documents.rs:208-214` 用 wrangler 直傳 |
| 寫入交易一致性、R2 補償刪除、防重複送出 | K3#2 | 採納（§8.2） | — |
| session 8 小時過期丟內容 | Fable#9 | 採納（§8.2，細節留建造者） | `session.rs:21` 確認 |
| A1 對地圖的例外不清楚 | K3#14 | 採納（A1 改為不含地圖） | `/map/*` 是公開 passthrough |
| token 128-bit CSPRNG | K3#3 | 已滿足 | `router.rs:566` 用 getrandom 16 bytes |
| K3：筆記 token 預設到期／定期換鏈 | K3 Q2 | 延後，列 §12 Q6 | 屬產品決定 |
| agy：note_id 用日期格式 | agy#15 | 不採納 | Fable#5 指出含日期會洩漏建立時間；改純隨機 hex |
| agy：遮罩改成全域套用所有登記值 | agy#2 | 不採納 | 遮罩功能已取消（Yang 2026-11-16） |

**收斂情形**：注入、metadata、照片路由驗權、上傳大小、主鍵、`plan_id` 掃表、v1 刪減，三位審查者各自獨立指出，可信度高。只有 Fable 指出的：統一 404、CLI 寫入繞過清理、`days` 主鍵含 destination、session 過期。只有 K3 指出的：寫入交易與 R2 補償。
