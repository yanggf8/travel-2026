# CLI 三項改善設計：update-activity／住宿未定／價格口徑

- 狀態：設計稿，待 Yang 核可；本文件不含程式碼，也沒有動任何資料。
- 日期：2026-11-19
- 依據：CLAUDE.md、`docs/reference/CLI.md`、`docs/reference/dashboard.md`、[行程紀錄設計](2026-11-16-trip-record-design.md)（下稱「紀錄設計」）、`docs/trips/2026-ne-coast.md`；引用的程式位置都是 2026-11-19 實際讀過的。
- 資料實況（2026-11-19 以 `/v2/pipeline` 唯讀查詢）：`ne-coast-2026` P4＝`selecting`；`domestic_accommodations` 共 20 列（ne_coast 11、jiufen 9），ranking 在 ne_coast 全為 NULL；`day_route_segments` 有 2 段端點是「頭城（住宿待定）」（D1 sort 2 的 to、D2 sort 0 的 from）；活動 D1 afternoon「抵達頭城（住宿未定，也可能當天回）」。

---

## 1. `update-activity`：原地編輯活動

### 1.1 問題與更正前提

需求寫「目前只能 delete + add」，**這與現況不符**，要先更正：

- 原地改的指令已經有三個：`set-activity-time`（start/end/fixed，`set_activity.rs:32`）、`set-activity-title`（`set_activity.rs:170`，改標題時會清 `poi_id`）、`set-activity-notes`（`set_activity.rs:1054`，紀錄設計 G4 補上，id 與 poi_id 不動）。
- **真正沒有指令能改的欄位**：`duration_min`、`area`、`nearest_station`、`priority`；以及「清掉」`start_time`／`end_time`（`set-activity-time` 只能設值，`update_activity_field` 的白名單也沒有那四欄，`repo/itinerary.rs:403-412`）。
- 這些欄位 `add-activity` 都能寫（`set_activity.rs:1695` `parse_add`），所以代理改一個 `--duration` 就得 delete + add；`delete_activity` 對有紀錄的列會被 `ensure_unrecorded` 擋下（`repo/itinerary.rs:654-665`），對有 `poi_id` 的列則會洗掉 poi 連結與 id。

### 1.2 實際使用可行性

- 誰用：Yang 旅途中對代理說「鼻頭角改 45 分鐘」「午餐不是 must 了」；代理現在只能繞路。ne-coast 這版「行程不要趕」就是反覆調時長的情境。
- 更簡單的做法比較過：(a) 只給 `set-activity-time` 加 `--duration`——能解時長，但 area／station／priority 仍缺，且指令名稱越撐越怪；(b) 一個 `update-activity` 涵蓋 add-activity 的全部欄位（建議）。舊三個指令**不動、不廢**（行為已有測試鎖，`set_activity_mutation_behavior_lock.rs`）。
- 結論：值得做，範圍小（一個指令＋一個 repo 函式）。

### 1.3 設計

**指令形狀**（旗標與 `add-activity` 同名，`set_activity.rs:1768` usage）：

```
travel update-activity <day> <session> <id|title>
    [--title <t>] [--area <t>] [--station <t>] [--duration <min>]
    [--start HH:MM] [--end HH:MM] [--fixed true|false]
    [--priority must|want|optional] [--notes <t>]
    [--clear area,station,duration,start,end,notes]
    [--dest <slug>] [--plan-id <id>|--travel-date ...]
```

- 方案解析走 `plan_resolver::resolve_plan_id`（`plan_resolver.rs:722`），分派寫在 `main.rs`，比照 `set-activity-notes`（`main.rs:691-700`）與 `add-activity`（`main.rs:738`）：先 `wants_help`，再 resolve，再 `set_activity::run_update`。目的地走 `read_destination`（`set_activity.rs:451` → `cascade::common::resolve_active_destination`，`common.rs:33`）。
- **比對**：用 `records::match_activity_unique`（`repo/records.rs:214`）——id 精確或標題子字串，**0 或多筆都失敗並列候選**；不用 `find_activity`（`set_activity.rs:459`，多筆時取第一筆）。理由同紀錄設計 §5「唯一比對」。變體日可編輯（同 `set-activity-notes` 註解）。
- **部分更新語意**：只改有給的旗標；沒給＝不動。`--clear` 接逗號清單，只收可為 NULL 的欄位：`area, station, duration, start, end, notes`。`title`、`priority`、`fixed` 是 NOT NULL，不能清。同一欄同時給值與 `--clear` → 失敗。至少要一個旗標，否則失敗。
- **驗證（寫入前、全部在 Rust 側）**：`--start/--end` 用 `checks::validate_time_flag`（`checks.rs:371`）；起訖順序要用「合併後的值」檢查（只給 `--end` 時拿 DB 裡現有 start 比，`validate_start_le_end` `checks.rs:386`）；`--duration` 為正整數；`--priority` 限三值；標題過 `check_title_map_url`（`checks.rs:295`）、不得為空。不加 DB constraint。
- **標題與 poi**：沿用 `run_title` 的規則（`set_activity.rs:170-299`）：標題真的變了且列上有 `poi_id` → 清 `poi_id` 並印 `note:` 與重新連結指令。**但已有紀錄的列不允許改標題**（見 1.4）。
- **冪等**：合併後與現值完全相同 → 不寫、不 bump、不留 audit，輸出「無變更」（比照 `set_activity_record` 的 `Ok(false)`，`repo/records.rs:387`）。
- **永遠不碰**：`id`、`sort_order`、`day/session`、`poi_id`（除標題變更規則）、`booking_*`、`record_status`、`record_note`、`source`、`cost_estimate`。換時段用 `move-activity`、訂位用 `set-activity-booking`、連結用 `set-activity-poi`、紀錄用 `record-activity`。

### 1.4 已有紀錄（record_status／record_note）的列

依紀錄設計：X3 只擋「刪除或重建」（§3、§5），X4 擋跨日；§0 明說旅途中代理本來就把時間改成實際時間。原地編輯不洗紀錄，所以：

| 欄位 | 有紀錄的列 | 理由 |
|---|---|---|
| start／end／fixed／duration／area／station／priority／notes | **可改** | 都是事實更正（實際時間、實際停留），列與紀錄仍指同一件事 |
| title | **拒絕**（兩個入口共用同一檢查，見下），訊息指向「用 `record-activity` 改 note，或先 `clear-record`」 | `done 東寺` 後把標題改成別處，紀錄就說謊（紀錄設計 X1/X2 精神）；也避免標題清 poi |
| record_status／record_note | **不屬於本指令** | 只能用 `record-activity`／`clear-record` |

- 判定：寫入前呼叫 `repo::records` 讀該列 `record_status`（`read_activity_record`，`repo/records.rs:387` 內部用的同一個讀取），非 NULL 且有 `--title` 才拒絕。不新增 `RecordScope`（`repo/records.rs:730`），因為 `ensure_unrecorded` 是「範圍內有紀錄就整個失敗」，語意不同。
- **共用同一個檢查（已決定）**：`set-activity-title`（`set_activity.rs:170-299`）目前沒有紀錄檢查，會繞過新規則。因此把「列有 `record_status` 就拒絕改標題」做成 repo 層一個函式（`repo/records.rs` 新增 `ensure_title_editable(conn, plan, dest, activity_id)`，內部讀 `record_status`，錯誤訊息指向 `record-activity`／`clear-record`），`run_update --title` 與 `run_title` 在寫入前都呼叫它，不各寫一份。標題與紀錄相同值（未變）時不拒絕。測試加：`set-activity-title` 對有紀錄的列失敗且資料不變、對無紀錄列行為不變（既有 `set_activity_title_poi.rs` 不退步）。此項會改變 `set-activity-title` 既有行為，CLI.md 需註明。

### 1.5 Audit 與輸出

- 單一交易：`begin_immediate`（`cascade/common.rs:298`）→ 讀 `plans.version` → repo 寫入 → `touch_day`（`set_activity.rs:706`，讓 `check-maps-fresh` 判定地圖過期）→ `emit_record_event`（`record.rs:128`，同時寫 `dest_process` 與 `timeline` 兩組 `plan_events`＋`plan_event_data`，並呼叫 `cascade::common::record_operation`，`common.rs:398`）→ `finish_tx`（`common.rs:308`）。完全比照 `run_set_notes`（`set_activity.rs:1054-1170`）。
- 事件名 `activity_updated`；KV：`day_number`、`session`、`activity_id`、`title`、`changed`（逗號欄位清單）、每個變動欄位一對 `from.<欄>`／`to.<欄>`（空值寫 `undefined`，沿用既有慣例，`set_activity.rs:424-445`）。`command_type`＝`update-activity`。
- 輸出純文字：
  ```
  📝 Updating activity: D2/morning 鼻頭角
     duration_min: 60 → 45
     area: (none) → 鼻頭
  ✅ Activity updated (id preserved: <id>)
  ```
  無變更：`ℹ️ No change (id=<id>)`。有清 poi 時照 `run_title` 印 `note:`。

### 1.6 要改的檔案

- 另：`repo/records.rs` 新增 `ensure_title_editable`；`set_activity.rs` 的 `run_title` 呼叫它。

- `rust/crates/travel-db/src/repo/itinerary.rs`：新增 `ActivityPatch`（每欄 `Option<Option<..>>`：外層＝有沒有給、內層＝設值或清成 NULL）與 `update_activity_fields(...) -> Result<bool>`，欄位名固定字面值、值全部 bind（比照 `update_activity_field` 白名單，`:389-412`）；新增 `read_activity_row` 供比對現值。
- `rust/crates/travel-cli/src/set_activity.rs`：`run_update`、`parse_update`（重用 `arg_value` `:419`、`parse_bool` `:425`、resolver flag 吞掉 `is_resolver_flag` `plan_resolver.rs:328`）。
- `rust/crates/travel-cli/src/main.rs`：分派＋`--help`＋第 1208 行的指令總表。
- `docs/reference/CLI.md`（`:237-252` 一帶）、`docs/reference/routing.md`「旅途中改變行程」那段加入 `update-activity`。
- 不動 `db_migrate.rs`、`scripts/schema.sql`（無 schema 變更）。

### 1.7 遷移

無。純新增指令。

### 1.8 測試

- 單元（`set_activity.rs` inline）：`parse_update` 的旗標、`--clear` 欄位白名單與衝突、缺旗標、非法 priority／時間／duration、`--end` 早於既有 start 的合併檢查（純函式化）。
- 整合（新檔 `rust/crates/travel-cli/tests/update_activity.rs`，比照 `set_activity_mutation_behavior_lock.rs` 與 `record_edit_cmds.rs`；`zz_test_<nanos>_*`、bind 後立刻 arm `Guard`、`common::teardown_plan`、背景執行）：
  1. 改 duration／area／station／priority：欄位變、`id`／`sort_order`／`poi_id` 不變、`updated_at` 更新。
  2. `--clear start,notes` 設回 NULL。
  3. audit triad 完整：`plan_events`（兩組）＋`plan_event_data`（含 from/to）＋`plans.version` +1＋`operation_runs.command_type='update-activity'`。
  4. 同值重跑：version 不變、無新 audit。
  5. 多筆符合時失敗且資料與 audit 完全不變。
  6. 有紀錄的列：改 duration／start 成功且 `record_status`／`record_note` 不變；`--title` 被拒且資料不變。
  7. 改標題＋有 poi_id 的未紀錄列：poi 被清並印 note。
  8. 變體日可編輯。
  9. 任何一欄驗證失敗 → 零寫入（rollback）。

### 1.9 風險

- 與 `set-activity-time/-title/-notes` 重疊：行為不一致時代理會困惑。對策：CLI.md 標明「新欄位用 update-activity；舊三個維持」，且共用同一個 repo 寫入與同一個紀錄規則。
- `Option<Option<>>` 語意易寫錯：用 parse 單元測試鎖死。
- 兩個 session 並發改同一列：`begin_immediate` 已序列化。

### 1.10 工時

約 0.5～1 天（repo 0.2、CLI＋解析 0.3、整合測試 0.3、文件 0.1）。

---

## 2. 「住宿未定／可能不過夜」

### 2.1 目前系統怎麼表示「一晚的住宿」

沒有「晚」這個概念，住宿散在五個地方：

| 地方 | 現況 | 位置 |
|---|---|---|
| 行程／日 | `days` 沒有任何住宿欄位 | `scripts/schema.sql:302-318` |
| 路線段 | `from_place`／`to_place` 是自由文字，「過夜點」只是一個普通端點 | `scripts/schema.sql:280-286` |
| 地圖圖例／plan.png | snapshot-maps 把端點文字拿去地理編碼成一般景點；只有「已訂」的住宿才會合併成飯店釘（`query_domestic_stay` 只讀 `bookings_current` `status='booked'`，`snapshot_maps.rs:2829-2853`；合併邏輯 `:1033-1047`）。`hotels` 表只有 Japan 路徑（`query_hotel_names`，`:2807`） | 同左 |
| 預訂總表 | 國內已訂住宿才出現；候選區獨立（`summary.rs:1149-1158`、`:1310-1321`） | `render/summary.rs` |
| P4 狀態 | `process_statuses` 一列（`schema.sql:1128`）；階梯含 `skipped`（`cascade/status_machine.rs:110-114`）；worker 只看 `booked/confirmed`（`summary.rs:1151`） | |

ne-coast 的佔位「頭城（住宿待定）」因此是：路線段端點（`day_route_segments`）→ 被當一般景點地理編碼（座標＝鎮中心，由 `set-place-geocode` 釘，`set_place_geocode.rs`）→ 出現在 plan.png 圖例 → 候選小圖的 🚩 hub（`hub_stop`，`summary.rs:606`，測試 `:2993` 就是這個形狀）。「可能當天回」其實已寫在活動標題裡（D1 afternoon），只是不是結構化資料。

### 2.2 實際使用可行性（先看這一節）

- **誰用、什麼情境**：Yang 的國內自駕短程（ne-coast 這類，2 天 1 夜），住宿常到當天才決定、甚至不住。目前只有 ne-coast 一次；jiufen 已訂；Japan 住宿都先訂好。低頻、單晚。
- **不需要逐晚狀態表**（booked／undecided／maybe-day-trip）。2 天 1 夜只有一個狀態；為它建 `plan_nights`＋CLI＋worker＋Japan 相容是過度設計。
- **現有機制已涵蓋**：
  1. 地圖一個點＝佔位地名＋`set-place-geocode`，已在運作，標籤「（住宿待定）」本身誠實。
  2. 「不列候選」＝候選排名全 NULL。commit `ac5047b`（已部署）已移除「全未排名就全部顯示」的回退：`summary.rs:1305-1317` 零排名 → 整個候選區不渲染；ne-coast 現在 0 張卡。Yang 要的「都不列」已達成。
  3. P4 狀態維持 `selecting`：階梯本來就代表「還在選、可能訂」。國內 `defer` 也明定不跳 P4（`CLI.md:234`）。`skipped` 只留給「已確定不過夜」（朋友家、當天來回已定案）——它不能代表「未定」。
- **所以 A2（P4 skipped）撤銷**：skipped 與「可能過夜」語意相反。

### 2.3 設計

**A1. 慣例（只改文件）**
- 過夜端點標籤統一 `<地名>（住宿待定）`，座標用 `set-place-geocode` 釘（OSM/Nominatim，不猜）。寫進 `routing.md`「國內行程」；CLAUDE.md 只放規則，不放。

**A2'. 中性提示：現在不需要**
- 候選區消失後，頁面上的行程本身已寫「住宿未定，也可能當天回」（D1 afternoon 活動標題）與路線端點「（住宿待定）」，讀者看得到。「selecting＋零排名」顯示「住宿尚未決定」小字會與這兩處重複，且 selecting＋零排名也可能只是「候選還沒研究完」，語意不乾淨。
- 結論：**不做**。若日後家人反映「不知道住哪」，再加：條件 `p4_status=="selecting"` 且無排名候選 → `i18n.rs` 一組 ZH/EN「住宿尚未決定／Lodging not decided yet」，單元測試 selecting＋零排名有小字、booked 與有排名時沒有。列為 Q2 讓 Yang 決定要不要現在做。

**A3. lint：過期佔位**
- `validate data` 新增 WARN：P4 為 `booked/confirmed` 的 plan，只要 `day_route_segments.from_place/to_place` 或 `activities.title` 含「住宿待定／住宿未定」就警告，列出位置與修正指令（`set-route-segment`、`update-activity`〔§1〕、`derive-routes`）。關鍵字放程式碼（CLAUDE.md 允許）。位置：`validate.rs` 緊鄰 `validate_domestic_accommodations`（`:1928`），單獨一次 SELECT。
- 缺口 B 的由來：`set-accommodation` 之後 plan.png 會多一個飯店釘（`snapshot_maps.rs:1033-1047` 只有座標相同才取代，鎮中心座標不同），佔位仍留在路線段與標題。

**Japan／國內相容**：不改任何渲染，只多一條只讀 lint。

### 2.4 層級 B（不做，只記觸發條件）

`plan_nights(plan_id, destination, night_date, state, area_label, area_lat, area_lon)`，`state` 由 Rust enum 驗證，`set-stay`／`clear-stay` 走 audit triad，snapshot-maps 讀 `area_*`。觸發條件：出現多晚且各晚狀態不同、或 Japan 多飯店需逐晚標示的行程。目前沒有。

### 2.5 遷移與既有佔位列

- 資料不遷移：「頭城（住宿待定）」路線段／活動／地理編碼原樣保留（正確的一般資料，也是候選小圖 hub 的錨點，`summary.rs:606`、測試 `:2993`）。
- **不還原排名**：四個候選維持 NULL 排名＝不列；恢復時才依 `2026-ne-coast.md` 記載順序用 `update-accommodation --rank` 排回。
- `db_migrate.rs` 不動。

### 2.6 要改的檔案

- `rust/crates/travel-cli/src/validate.rs`（A3）。
- 文件：`docs/reference/routing.md`（A1）、`dashboard.md:51`（候選渲染規則改為「零排名 → 無候選區」，移除「全未排序仍顯示」字句）、`docs/trips/2026-ne-coast.md`（住宿段與 `ac5047b` 對齊，註明目前 P4 `selecting`、全未排名＝不顯示）。
- worker 與 i18n 本項不改。

### 2.7 測試

- `validate` A3 抽成純函式（輸入：P4 狀態＋文字列）的單元測試；Turso 整合測試（`zz_test_*`、Guard、`teardown_plan`、背景）種一條含「住宿待定」路線段＋P4 booked → WARN；P4 selecting → 無 WARN；P4 booked 但無佔位 → 無 WARN。
- worker：`ac5047b` 已有零排名無卡的測試（`summary.rs:3285` 一帶）；本項不新增 worker 測試，也**不新增**任何鎖舊回退行為的測試。
- 對 ne-coast-2026 實跑 `verify-site`，確認不退步。

### 2.8 風險

- A3 只認兩個中文關鍵詞，換說法會漏；可接受（WARN 非 ERROR）。
- 「不做提示」的取捨：讀者只能從行程文字得知住宿未定；以 Q2 讓 Yang 決定。

### 2.9 工時

約 0.25 天（lint＋測試＋文件）。層級 B 若做約 2～3 天，不在本次範圍。

---

## 3. 國內住宿價格口徑（含稅／未稅）

### 3.1 問題與資料實況

Booking 顯示含稅總價，Agoda 顯示未稅房價（結帳另加約 15%）（`2026-ne-coast.md`「價格比較注意口徑」）。目前口徑只能靠人記，或塞在自由文字裡：ne_coast 的 Agoda 那列 `price_source` 就是「Agoda（含稅及服務費）」（Turso 實查）。把 Booking 的含稅價和 Agoda 的未稅價並排排名，會讓便宜的 Agoda 看起來更划算。

實況：20 列，`price_source` 分佈＝ne_coast：Booking.com 10、Agoda（含稅及服務費）1；jiufen：Booking.com 5、`booking` 4（舊寫法）。ranking：jiufen 4 列有排序；ne_coast 全無。

### 3.2 實際使用可行性

- 誰用：Yang 比價時看卡片；代理排序前自查。每次國內行程都會同時看 Booking 與 Agoda，高頻。
- 更簡單做法：繼續把口徑塞進 `price_source` 文字——不可驗證、`validate` 無法偵測混用、卡片文字不統一。不夠。
- 結論：加一欄結構化口徑值得做，且改動面小（沿用 `price_source`／`price_checked_at` 那條路）。

### 3.3 設計

**資料**：`domestic_accommodations.price_basis TEXT` 可為 NULL、無 CHECK、無預設。合法值 `incl`（含稅及服務費的總價）、`excl`（未稅，結帳另加）由 Rust enum 驗證（比照 `RecordStatus::parse`，`repo/records.rs:122-146`）。**不從 `price_source` 推導、不填預設**（CLAUDE.md「No default data」）；NULL＝口徑未確認。不存「約 15%」——那是內容且會變，不寫死；也因此不做稅額換算。

**CLI**（`domestic_accommodations` 是 slug 鍵全域參考資料，無 `--plan-id`、無 audit triad，`update_accommodation.rs:5-8`）：
- `add-accommodation … [--price-basis incl|excl]`（`add_accommodation.rs:181` usage 與 `:270` 解析一帶）。
- `update-accommodation --id <id> [--price-basis incl|excl|clear]`（`update_accommodation.rs:144` usage、`:273` 解析、`sets()` `:57-80`）；`clear` 設回 NULL（與 `--notes clear`、`--lat clear` 一致）。
- **單獨改口徑不蓋 `price_checked_at`**：口徑是價格的屬性，但不是「重新查價」；`:351` 的「`--price-source` 沒給日期就蓋今天」規則不套用在 `--price-basis`。
- **改價格來源時口徑一併失效（已決定：清回 NULL，不要求必填）**：`update-accommodation` 只要有 `--price-source` 而沒同時給 `--price-basis`，就把 `price_basis` 設回 NULL，並印一行 `note: price_basis cleared (price source changed) — set --price-basis incl|excl`。理由：舊口徑屬於舊來源，留著會把 Booking 的 `incl` 掛在 Agoda 旁發布錯誤口徑；而強制必填會讓只想修正來源拼字的更新變得很重，且 `--price-source` 現在本來就能單獨使用（`update_accommodation.rs:351-353`）。清成 NULL 是「未知」，由 validate INFO 追蹤，符合「不猜」。同值重設（來源沒變）不清。需讀現有 `price_source` 比對，所以 update 多一次 SELECT。
- `add-accommodation` 給了 `--price-source` 卻沒給 `--price-basis` → 輸出一行提示（stdout，不失敗）：`tip: --price-basis incl|excl（Booking 多為含稅、Agoda 多為未稅，請以頁面結帳為準）`。提示放程式碼（UI 文字，非資料）。
- `list-accommodations`（`list_accommodations.rs:149` 的明細行）與 `query-accommodation` 顯示 `price_basis=`。

**repo**：`NewDomesticAccommodation`（`repo/domestic_accommodations.rs:157`）、`DomesticAccommodationRow`（`:9`）、`insert`（`:193`，欄位與 `?N` 都要加）、`query` 的 SELECT 清單（`:111` 一帶，**加在尾端**避免位移）；`update_fields`／`build_update`（`:230-257`）已是通用的，不必改。

**遷移**：`db_migrate.rs` 在 `:1754-1764` 那個 `for ddl in [...]` 清單加一行 `ALTER TABLE domestic_accommodations ADD COLUMN price_basis TEXT;`（走 `add_column`，`:50`，重跑冪等）。之後重生 `scripts/schema.sql`，不手改。

**validate（`validate.rs:1928` `validate_domestic_accommodations`）**：
- SELECT 尾端加 `price_basis`（現有欄位索引 0–9，新欄位用索引 10，不動既有 `row.get(n)`）。
- **WARN**：同一 `destination` 內，`ranking IS NOT NULL` 的列同時出現 `incl` 與 `excl` → 「<dest> 排序中的候選混用價格口徑：含稅 N 列（…）、未稅 M 列（…）；排序比較不可信，請換算或補查同口徑價格」，列出 id。
- **INFO**：`price_source` 非空但 `price_basis` 為 NULL → 提示 `update-accommodation --id X --price-basis incl|excl`（與既有 `price_source` 缺 `price_checked_at` 的 INFO 同型，`:2018-2051`）。只對有排序的列發 INFO 以免噪音（未排序＝淘汰列不比價）。
- **WARN**：`price_basis` 非 NULL 且不是 `incl`／`excl`（欄位無 CHECK，直接 `db exec` 或舊程式可能寫入髒值）→ 列出 id 與值。
- 判斷抽成純函式 `basis_mix_issue(rows) -> Option<..>` 方便單元測試。

**worker**：
- `model.rs`：`DomesticCandidate`（`:286`）加 `price_basis: String`；`:773` 一帶 `price_basis: s(r, "price_basis")`。
- `router.rs:1044` 的 SELECT 加 `price_basis`（worker 用明確欄位清單，欄位不存在整頁會失敗，所以**部署順序固定：`db migrate` → 部署 worker**，同紀錄設計 §9）。
- `render/summary.rs:318` `price_note`：緊接 `priceSourceRate` 之後加一段 `priceBasisIncl`／`priceBasisExcl`，成為「Booking.com 房價 · 含稅 · 2026-09-04 查 · …」；只渲染 `incl`／`excl` 兩個合法值；NULL 與任何其他值都視同未知、不渲染（不顯示「口徑未知」，避免版面雜訊；未知由 validate INFO 處理）。
- `i18n.rs:500-505` 旁加兩組 ZH/EN：ZH「含稅」「未稅（結帳另加稅費）」、EN「incl. tax」「excl. tax (taxes added at checkout)」。
- `docs/reference/dashboard.md:51` 與 `CLI.md:199-200` 更新。

### 3.4 回填（整合者執行，不是 migrator）

`db_migrate.rs` 只加欄位，**不 UPDATE 任何列**（CLAUDE.md「No trip content in the migrator」）。回填用 `update-accommodation --id <id> --price-basis …` 逐列（20 列，一個 shell 迴圈即可；每列一次 SELECT 驗證＋一次 UPDATE，且不蓋 `price_checked_at`）。依證據，不全部設預設：

| 範圍 | 建議值 | 證據 |
|---|---|---|
| ne_coast＋`Booking.com`（10 列） | `incl` | `2026-ne-coast.md`：Booking 顯示含稅總價 |
| ne_coast＋`Agoda（含稅及服務費）`（1 列，晏京） | `incl` | 該列 `price_source` 已明寫含稅及服務費 |
| jiufen＋`Booking.com`（5）、`booking`（4，舊寫法） | 待 Yang 確認（Q4）；jiufen 已訂，價值低，可留 NULL | 舊資料未留口徑證據 |

回填後 `validate data` 預期：ne_coast 排序恢復後全為 `incl`，無 WARN。未來若加入 Agoda 未稅列且被排序，WARN 立刻出現——這就是本功能要擋的情境。

### 3.5 測試

- 單元：`add_accommodation.rs`／`update_accommodation.rs` 的 `parse_args`：`incl|excl` 合法、`clear`（update）、非法值失敗、單給 `--price-basis` 不蓋日期、`add` 有 source 無 basis 的提示；`build_update` 含新欄位（`repo/domestic_accommodations.rs:314` 一帶既有測試型式）。
- 整合（擴充 `tests/domestic_accommodations_crud.rs`；`zz_test_<nanos>_*`、Guard、背景執行）：add 帶 basis 寫入；update 改／清；`price_checked_at` 不變；舊列 basis 為 NULL。
- 更新來源：`--price-source` 無 `--price-basis` → basis 清 NULL 並印 note；兩者同給 → 保留新值；來源同值 → 不清；`price_basis` 非法值 validate WARN；worker 對非法值不渲染。
- validate：`basis_mix_issue` 純函式測試（同 dest 混用→WARN、單一口徑→無、未排序列不計、跨 dest 不互相影響、NULL 不計入混用）。
- worker：`price_note` 單元測試（incl／excl／NULL × zh／en）；i18n 四個字串。
- `verify-site` 對 jiufen、ne-coast 實跑不退步。

### 3.6 風險

- 回填證據不足的列（jiufen 舊列）：因此不設預設，留 NULL。
- 「含稅」只代表該 OTA 頁面顯示的口徑，不保證與店家現場一致；卡片已有 `price_checked_at`，文字不做額外承諾。
- 不做稅額換算，所以混用時系統只警告、不能自動比價；若 Yang 要「未稅價換成含稅估計」，需要新增稅率資料（內容），另案（Q5）。
- 新 worker 欄位與 migrate 順序顛倒會整頁失敗：寫進部署步驟，`verify-site` 作為把關。

### 3.7 工時

約 0.5～0.75 天（migrate＋repo＋兩個 CLI 0.25、validate 0.15、worker＋i18n 0.15、測試與回填 0.2）。

---

## 4. 建議順序與總工時

1. 項目 3（高頻、風險最低，且 ne-coast 排序一恢復就用得上）。
2. 項目 1（修補 CLI 缺口，且是項目 2 的 A3 修正指令之一）。
3. 項目 2（只剩 lint 與文件）。

合計約 1.25～2 天。三項彼此獨立，可各自派工、各自提交；都沒有 feature flag，都不改 `scripts/schema.sql`（只有項目 3 透過 `db_migrate.rs` 加一欄並重生）。

## 5. 給 Yang 的未決問題

已決定、不再詢問：`set-activity-title` 共用標題防護；P4 維持 `selecting`、`skipped` 只用於確定不過夜；不還原排名；`--price-source` 改動而無口徑時清 NULL；非法口徑值 WARN。

- **Q1**：候選區消失後，頁面是否還要一行中性的「住宿尚未決定」（`selecting`＋零排名才顯示）？我的建議是現在不加（行程文字已寫明），日後有人看不懂再加。
- **Q2**：jiufen 的 9 列（`Booking.com` 5、`booking` 4）要回填口徑，還是已訂行程就不管？
- **Q3**：未稅價日後要不要提供「估算含稅」？那需要把稅費比例當資料存起來，會擴大範圍。
- **Q4**：`update-activity` 要不要收 `cost_estimate`？建議不收，與 `add-activity` 對稱。
- **Q5**：層級 B（逐晚住宿表）是否明確延後，直到出現多晚且各晚狀態不同的行程？
