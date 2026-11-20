# CLI 行程編輯改善設計（第二版）：活動備註顯示／活動釘點／追加備註／路線段部分編輯／搬移交易化／via 儲存／指定人

- 狀態：設計稿（第二版，已納入 Yang 2026-11-20 的決定與 Fable 審查修正）；本文件不含程式碼，也沒有動任何資料。
- 日期：2026-11-20
- 依據：CLAUDE.md、`docs/reference/CLI.md`、`docs/reference/dashboard.md`、[前規格](2026-11-19-cli-activity-stay-price-design.md)（其 §1 `update-activity` 已實作，本文多項延伸它）。引用的程式位置是 2026-11-20 讀過的；第二版依審查修正過的行號已更新，**實作者動手前仍應就近重讀該段**。
- 起因：2026-11-19 編輯 `ne-coast-2026` 時撞到的摩擦（見各節「問題」）。
- 現況更正（避免重做）：`update-activity` 已存在（`set_activity.rs:1380` `run_update`、`repo/itinerary.rs:688` `ActivityPatch`、`main.rs:702`）；`add-activity --after`（`repo/itinerary.rs:973`）與 `reorder-activities`（需列出整個 session 全部 id，`main.rs:761`）已存在；`set-place-geocode` 只釘「路線端點標籤」（`set_place_geocode.rs:1-9`）。

## Owner decisions（2026-11-20，Yang「按你建議做」）

| # | 決定 | 本文處理 |
|---|---|---|
| A | **新增第 0 項（最優先）**：worker 顯示 `activities.notes`；`view-itinerary` 也印活動備註 | §0 |
| B | 第 4 項**縮小**：不做 `--after/--first`；只做「`move-activity` 標題多筆符合 → 失敗並列候選」＋`run_move` 包交易；CLI.md 指向 `reorder-activities` | §4 |
| C | 第 5 項**只做 v1**：via 逐段存（子表、有序列、無 JSON）；`road-leg refetch --segment D S` 與 snapshot-maps 快取未命中時使用；**不做** `via_sig`／`leg_fetch_decision`（下一輪）。`set-route-segments-bulk` 對「from/to 標籤與舊段相同」的新段保留 via，只清有變的並印出是哪些。`--via` **只收 `lat,lon`（離線）**。`day_route_segment_via` 加進 `SESSION_TABLES`（swap-days）＋行為鎖測試 | §5 |
| D | 第 6 項**只做活動**（`requested_by`）；餐點不做 | §6 |
| Q1–Q8 | 全採建議答案（Q1 note 要印出座標；Q4 要印出確切的 `road-leg refetch --segment D S` 指令；Q8 回填順序見 §9） | §9 |

Fable 審查 H1–H4／M1–M5／L1–L2 已逐條折入：H1→§1.3（`day_logistics_legs`）、H2→§1.3（去重常數）、H3→§5、H4→§5（SESSION_TABLES）、M1→§1.3（欄位改名 `pin_only`）、M2→§5、M3→§0 共通約束與 §3/§5、M4→§3、M5→§1.3（不另加 >40 km 警告）、L1→§1.4、L2→§3（事件名 `route_segment_patched`）。

---

## 0. 共通約束

- schema 只經 `db_migrate.rs` 的 `add_column`（`db_migrate.rs:50`，冪等）與 `CREATE TABLE IF NOT EXISTS`；`scripts/schema.sql` 由既有流程重生，不手改。migrator 只建結構，不 INSERT/UPDATE 任何列。
- 無 DB constraint、無 JSON 欄、無 feature flag；驗證全在 Rust（比照 `update_accommodation.rs:369-418`）。
- **所有新的寫入指令都必須是單一交易**（M3）：`begin_immediate`（`cascade/common.rs:298`）→ repo 寫入 → `touch_day` → `record::emit_record_event`（`record.rs:128`，內含 `cascade::common::record_operation`）→ `finish_tx`（`common.rs:308`），完全比照 `run_update`（`set_activity.rs:1390` 起）。**網路／解析類工作（地名解析、OSRM）一律在 `begin_immediate` 之前做或根本不做**（§5 選擇「只收 lat,lon」）。既有的 `set-route-segment`／`set-route-segments-bulk`／`derive-routes`／`move-activity` 目前沒有包交易（`set_route_segment.rs:384` 起 `execute_single`/`execute_bulk` 直接多次寫入；`run_move` 同）；本案因為要在其中搬運 via 子表，連同包進交易（§3、§4、§5）。
- worker 明確列欄位的 SELECT（`router.rs:934-941`）遇到不存在的欄位整頁失敗，所以**部署順序固定：`db migrate` → CLI → worker deploy**。
- flag 解析走 `plan_resolver::reject_unknown_flags` 或各 parse 函式的「未知旗標即失敗」。

---

## 1. （第 0 項）顯示活動備註

### 1.0 問題
`activities.notes` 只有 CLI 讀得到：worker 的 activities SELECT 不含 notes（`router.rs:936-937`），`view-itinerary` 的 `render_activity`（`view_itinerary.rs:533-557`）只印圖示、標題、`[ref]`、`(book by …)`。於是代理寫進去的營業時間、停車、Yang 補充，旅伴與 Yang 本人在頁面上都看不到；這也是 `--append-notes`（§2）有意義的前提。

### 1.0.1 可行性與實用性
- 誰用：Yang 與同行者看行程頁；代理寫的補充資訊終於可見。每個行程都用得到。更簡單的做法（把備註塞回標題）正是現在標題過長、混入「誰指定」的根源。
- 風險（須知，不阻擋）：**分享連結檢視者也看得到備註**（同頁渲染）。依 owner 的「不做遮罩」決定可接受，但**部署前整合者必須用 CLI 讀一遍既有活動備註**（`view-itinerary` 加上備註後即可），確認沒有不想給檢視者看的內容（訂位代號應在 `booking_ref`，不是 notes；仍須人工掃過）。
- 結論：值得做，且是其他項的前置。

### 1.0.2 設計
- **worker**：`router.rs:936` 的 SELECT 加 `notes`；`model.rs` 的 `Activity`（`:46`）加 `notes: String`，`build_sessions`（`:214`）填入；`render/session.rs`（活動行 `:279-300` 一帶）在活動標題與 `ai_rec_badge` 之後、紀錄徽章之前，於**同一個 `div.activity` 內**加一個子區塊，內容用 `render_activity_text(&a.notes)`（已負責跳脫、`\n`→`<br>`、內嵌 URL 連結化，`render/activity_text.rs:41`）；notes 為空時不輸出任何標記。樣式沿用既有的淡色小字樣式（重用 `record-note`／`now-transit` 一類既有 class；至多新增一條 CSS 規則做區塊顯示，不引入新視覺語彙）。`render/now.rs:274` 的 `now-act`（現在・接下來展開內容）同樣顯示。已標 skipped 的過去活動仍只進「未前往」那行（不帶備註），維持現狀（`session.rs:279`、`now.rs:274`）。
- **i18n**：不需要新字串（備註本身是資料，沒有標籤）。
- **搜尋**：備註在 `div.activity` 內，屬於該 session 的 `data-search-unit` 文字，TOC 搜尋自然找得到（以測試確認，不另寫程式）。
- **CLI**：`plan::ActivityView`（`plan.rs:233`）加 `notes`，對應的 activities 查詢加欄；`render_activity` 在標題行之後，若 notes 非空，另起縮排行印 `      ↳ <notes 第一行>`，多行則每行縮排續印（純文字，與既有 `(<notes>)` 路線段後綴風格一致）。這會改變 `view-itinerary` 輸出，須更新 view 的行為鎖測試（`view_itinerary.rs` 檔頭註解提到的 TS 對齊規則）。
- **verify-site**：渲染結果新增內容，需確認不退步，並加一條輕量檢查——record 層既然已逐列讀 `activities`（以 `data-activity-id` 對列），就順便驗「notes 非空 → 該 `div.activity` 內含其文字」（第一行的 HTML 跳脫後文字即可）。檔案為 `verify_site_record.rs`（歸 L4）。`--render` 的版面量測因頁面變長可能改變截圖，不改判準。

### 1.0.3 測試
- worker 單元：有／無 notes × zh／en 的活動行 HTML；notes 含 `<b>`、`\n`、URL 的跳脫與換行（跳脫咬合：`<b>` 不得原樣輸出）；past＋skipped 不帶備註；`now-act` 同步。
- CLI 單元：`render_activity` 單行／多行／無備註。
- 整合：`tests/` 既有 view-itinerary 整合測試補一列帶備註的活動。
- 咬合：把 SELECT 的 `notes` 拿掉，worker 測試必須變紅（用含 `notes` 欄的 row fixture 驗證 `build_sessions` 有填值）。

---

## 2. 活動自己的地圖釘點（不靠路線段）

### 2.1 問題
snapshot-maps 的釘點**只來自路線段端點**：`assemble_maps` 逐日迭代 `src.segments`，端點經 `resolve_segment_label`（`snapshot_maps.rs:2918`）→ `HubGate::point`（`:558`）變成 `Point`。活動只有一條後備路：`points.is_empty()` 才用 `src.pois`（`:801` 一帶 `// POI fallback is for a day that has no route at all`），而 `query_pois`（`:2757`）要求 `poi_id`／同名 JOIN `destination_pois` 且有 lat/lon。國內行程的地點不在 `destination_pois`，所以 ne-coast 只好把 關渡→川岩→滿山望海→頭城 硬塞成路線才能給兩間咖啡廳釘點；三民大飯店不是任何路線端點，無釘。worker 側 `attach_stops`（`model.rs:879-925`）座標也只來自 POI 列，沒有就走 `maps/search/<標題>` 搜尋連結。

### 2.2 可行性與實用性
- 誰用：Yang／代理在國內自駕行程加零散的點，要在地圖上看到「這天有哪些點」，不一定要有路線。高頻。
- 比較：(1) 為每個點建 `destination_pois`（全域參考資料，PK=(slug,poi_id)，`schema.sql:427-444`；要想 poi_id、寫一堆欄；會汙染 `set-activity-poi --auto` 的比對池）——不採用；(2) `set-place-geocode "<活動標題>"`（快取鍵是標籤正規化字串，`set_place_geocode.rs:26-29`，標題改了就失配，且只被路線端點讀）——不採用；(3) **活動列自帶座標**——同一列、隨 `move-activity` 走（id 不變）、形狀與 `update_accommodation --lat/--lon` 相同。採用 (3)。
- 結論：值得做。

### 2.3 設計

**資料**：`activities` 加 `lat REAL`、`lon REAL`、`geo_source TEXT`（自由文字，僅供追溯，無 CHECK、無預設、無回填）。

**CLI**
```
travel set-activity-geocode <day> <session> <id|title> (--lat <f> --lon <f> [--source <t>] | --clear)
    [--dest <slug>] [--plan-id <id>|--travel-date ...]
```
- 比對用 `records::match_activity_unique`（`repo/records.rs:214`，0/多筆失敗並列候選）。`--lat/--lon` 成對；`--clear` 與之互斥。
- 驗證：有限數值、lat∈[-90,90]、lon∈[-180,180]、(0,0) 拒絕（訊息同 `update_accommodation.rs:418`）。共用純函式 `checks::validate_coords`（現在 `update_accommodation.rs` 有私有 `coord_value`，改呼叫共用版，不複製第三份；§5 的 via 也用它）。
- 冪等：完全相同 → 「No change」，零寫入、零 audit。
- `add-activity` 增加選用 `--lat/--lon/--source`（成對）。不新增 `update-activity` 旗標（避免 `ActivityPatch` 與 `activity_row_diff` 固定欄位序膨脹，`itinerary.rs:729`）。
- repo：`repo/itinerary.rs` 新增 `set_activity_geocode(...)`，固定欄位名、值全 bind。
- **改標題不清座標**（Q1 已決定）：改標題且該活動有座標時，只印 note，且**要印出座標**：`note: 活動已有座標 (<lat>,<lon>)，標題改變不會清除；地點若換了請 set-activity-geocode`。
- 事件 `activity_geocode_set`／`activity_geocode_cleared`；KV：`day_number`、`session`、`activity_id`、`title`、`from.lat/lon/geo_source`、`to.lat/lon/geo_source`（空值寫 `undefined`）。`command_type`＝`set-activity-geocode`。

**snapshot-maps：活動釘點（只畫點，不畫線）**
- `MapSource` 新增 `act_pins`，由 `query_activity_pins`（與 `query_pois` 同型）讀：`lat/lon IS NOT NULL` 且非 skipped（skipped 者其座標也併入 `skipped_here`，沿用 `query_skipped_pois` 做法，`:2774`），排序 `day_number, 時段序, sort_order`。
- **`Point` 新增欄位 `pin_only: bool`**（M1：命名用 `pin_only`，預設 false＝既有路線停靠點；不叫 `routed`，因 `RouteLine.routed`（`:2679` 一帶）已有不同含義）。構造統一經 `HubGate::point`（`:558`）加參數。
- **畫線入口只取非 `pin_only` 的點**：`day_routes`（`:608`）、`net.routes(&points)`、`cached_routes` 的 `windows(2)`（`:2624`）。釘點照畫、照編號、照進圖例。
- **H1（審查必修）**：`day_logistics_legs`（`:2019-2023`）以 `Kind::Sightseeing` 找 `first`／`last` 以及 `sight_idx`；活動釘也是 `Kind::Sightseeing`，會被當成「當天第一個／最後一個景點」，把旅館往返線接到一個沒有路線的釘上。**`first`、`last`、`sight_idx` 一律只考慮 `!pin_only`**。實作者必須 grep **所有** `Kind::Sightseeing` 使用處逐一判斷（編譯器不會報：型別沒變）；已知需檢查：`day_logistics_legs`（`:2019`）、`distant_excursion_stop`（`:2303-2318`）、`overview_day_points`、airport inset 的 `on_airport_leg`／`on_other_leg`（這兩個集合由段端點建立，釘不在內，行為應自然正確）。咬合測試見 §2.5。
- **M5**：不另加「活動座標離路線 >40 km」警告。既有的 `distant_excursion_stop` 建議（`:2303-2318`，>40 km 提示 `set-day-excursion`）因釘也是 Sightseeing 而可能把遠處活動釘納入——這是既有提示（只建議、不改資料），維持原樣，不為它新增程式。
- **H2（審查必修）去重**：新常數 `ACTIVITY_PIN_DEDUPE_M = 120.0`（量級 100–150 m），以 `haversine_m`（`:2291`）判斷；**不得重用 `leg_match_radius_m`**（`:2341`，它依兩端距離縮放、用途是比對快取路線，量級與語意都不同）。規則：活動釘與同日任一已存在的點（路線停靠點或較早的活動釘）距離 ≤ 常數，或 `coord_key` 相同 → 不另開釘，沿用先到者的標籤。此規則同時處理「活動同時也是路線端點」。
- **編號與標籤**：同日內，路線停靠點在前（原順序），活動釘依行程順序（時段、`sort_order`）接後；`pin_numbers`（`:1464`）per-day 編號自然延續，不改該函式。標籤＝活動標題第一行（取乾淨短名，避開內嵌 URL）。圖例不區分釘與路線點（Q7）。
- `map_input_sha256`（`:1496`）把 `pin_only` 納入雜湊；`RENDER_REV`（`:1896`）bump。
- 缺座標的活動不產生釘（不猜、不呼叫 Nominatim）。
- verify-site 走同一個 `assemble_maps`（`CacheOnly`，`:296-302`）：活動釘不需網路，雜湊可重現。

**worker**：`router.rs:936` 的 SELECT 另加 `lat, lon`；`model.rs` `attach_stops`（`:879`）座標優先序＝活動座標 > POI 座標 > 無；有座標時 `maps_link` 用乾淨座標連結（`:903-905`），否則維持原搜尋連結行為。無新 i18n。

**validate data**（單一 SELECT，緊鄰 `validate_stay_placeholders`，`validate.rs:2454`；純函式 `activity_geo_issues`）：ERROR——lat/lon 只給一個；ERROR——超界或 (0,0)；WARN——離 hub 超過 500 km（可能 lat/lon 對調；與 `MAX_HUB_DISTANCE_KM` `:526` 同門檻）；INFO——同日兩活動同座標（5 位小數）。

### 2.4 稽核、文件、遷移
- audit 見 §2.3；`touch_day` 讓 `check-maps-fresh` 判過期。
- 文件：`CLI.md`（新指令＋`add-activity` 旗標，`:254` 一帶）、`routing.md`（國內：「要在地圖上標一個點 → set-activity-geocode，不要為了釘點硬造路線段」）、`dashboard.md:16` 地圖段落（活動釘不連線；**L1：活動釘的編號接在路線停靠點之後，不代表遊覽順序**）、`docs/trips/2026-ne-coast.md`。
- 遷移：僅 `add_column` 三行；`plan_copy.rs`（`:486-546`）以欄位資訊建複製清單，新欄位應隨之複製——由 copy-plan 測試證實，不預設。

### 2.5 測試
- 單元：`validate_coords`（邊界、(0,0)、NaN、單給 lat）；`--clear` 互斥；`add-activity` 成對旗標；假 `MapResolver`（`snapshot_maps.rs:3703` 的 `fake_net`）下：釘不增線（咬合：`RouteLine` 數量不變）、編號延續（路線 3＋釘 2＝1..5）、去重（同座標／≤120 m／>150 m 不去重——**邊界咬合**：121 m 與 119 m 兩側各一個）、兩個活動釘同點只留一個、hub 過濾（遠處釘進 away）、skipped 不出釘、`map_input_sha256` 對 `pin_only` 敏感。
- **H1 咬合測試**：一天有旅館＋兩個路線景點，另加一個**排在最前面**的活動釘與一個**排在最後面**的活動釘；斷言 `day_logistics_legs` 回傳的 legs 與「沒有活動釘」時完全相同（拿掉 `!pin_only` 過濾必紅）。
- 整合（新檔 `tests/set_activity_geocode.rs`；`zz_test_<nanos>_*`、綁定後立刻 arm `Guard`、`common::teardown_plan`、背景跑）：設／清、audit triad 完整、同值零 audit、多筆比對失敗零寫入、`move-activity` 後座標不變、`update-activity --title` 後座標不變且 note 含座標、驗證失敗零寫入。
- snapshot 層：只有活動座標、無任何路線段的一天 → 圖例列 `pin_no` 連號；有路線段＋釘 → 路線數不變。

### 2.6 風險
`Point` 加欄牽動面廣：以 `HubGate::point` 簽名一次改完，編譯器列出漏改的構造處；但 `Kind::Sightseeing` 的語意使用處**編譯器抓不到**，靠 grep 清單與 H1 咬合測試。

---

## 3. `update-activity --append-notes`（以及路線段備註）

### 3.1 問題
`update-activity --notes` 整段取代（`ActivityPatch.notes`，`repo/itinerary.rs:688-700`）；一條長的坪林茶博備註被覆蓋，靠 `plan_event_data` 的 `from.notes`（`set_activity.rs:1446-1450`）救回。

### 3.2 可行性與實用性
代理陸續補資訊，高頻；覆蓋是無聲資料損失。更簡單的「先讀再整段重寫」正是出事的流程。有了 §1（備註顯示），備註的重要性更高。值得做，小。

### 3.3 設計
- `update-activity --append-notes <t>`，與 `--notes`、`--clear notes` 互斥（`parse_update` 既有衝突檢查，`set_activity.rs:1205-1320`）。
- 純函式 `append_note(existing: Option<&str>, add: &str, sep: &str) -> Option<String>`（`None`＝無變更），放新檔 `repo/text_merge.rs`（由 L0 建立，供活動與路線段共用）：
  1. `add` trim；空 → 失敗。
  2. 現值 NULL／空 → 結果＝`add`。
  3. **冪等**：把現值與 `add` 各按行切、各行 trim；`add` 的各行若已**連續區塊**出現在現值中 → `None`（「No change」，零寫入、零 audit；用行區塊而非子字串，避免「好」這類短字誤判）。
  4. 否則＝現值（去尾端空白）＋`sep`＋`add`。活動 `sep="\n"`。
- 合併發生在交易內讀到現值之後（`begin_immediate`，`set_activity.rs:1390`），並發安全。`ActivityPatch` 加 `append_notes: Option<String>`，`apply` 內呼叫 `append_note`，`activity_row_diff` 自然產生 `notes` 的 from/to。
- audit：沿用 `activity_updated`，額外 KV `notes_mode=append`。
- 安全網：`--notes` 取代時若現值非空且新值**不包含**舊值，印一行 `warn: 取代了 N 字備註；舊內容保留在 plan_event_data from.notes（或改用 --append-notes）`。
- 路線段備註：`update-route-segment --append-notes`（§4），`sep="；"`（段備註渲染為單行 ` · {notes}`，`render/day.rs:558`；Q2）。

### 3.4 文件、測試
- 文件：`CLI.md:240`。
- 單元：`append_note` 全表（NULL、空、已含單行、已含多行連續區塊、已含但不連續→追加、短字誤判、兩種分隔符、尾端空白）；`parse_update` 互斥。
- 整合：追加→舊＋`\n`＋新；同文字重跑→version 不變、無新 audit（咬合）；audit KV 含 `notes_mode=append`；取代警告出現時機。

---

## 4. `update-route-segment` 與 `delete-route-segment`

### 4.1 問題
路線段只能整天重寫（`set-route-segments-bulk`，`set_route_segment.rs:140`）或重給全部欄位（`set-route-segment` 五個位置參數必填，`:22-26`；內部 `delete_slot`＋`insert_segment`，`:411-428`）。改一個 duration 要重打整條，且 `set-route-segment` 不帶 `--recommended` 會把 `ai_recommended` 段默默變 `confirmed`（`source` 判定在 `:100-104` 一帶，輸出提示在其後）。

### 4.2 可行性與實用性
Yang 說「這段改 30 分鐘」「出發改 10:30」「這段不要了」，高頻。整天 bulk 對有紀錄的日子根本不能用（`ensure_unrecorded` 擋，`repo/route_segments.rs:81-92`）。值得做；`update` 還順帶避開 `source` 被翻轉。

### 4.3 設計

**`update-route-segment <day> <sort_order>`**
```
[--from <t>] [--to <t>] [--mode <m>] [--duration <min>] [--start-time HH:MM]
[--notes <t> | --append-notes <t>]
[--clear duration,start_time,notes]
[--via "<lat,lon;lat,lon>" | --clear via]        # §5
[--dest <slug>] [--plan-id <id>]
```
- 目標＝`(plan, dest, day, sort_order)` 一列；不存在 → 失敗並列該日現有 `sort_order`。
- 部分更新語意同 `ActivityPatch`（`SegmentPatch` 放 `repo/route_segments.rs`）；`from/to/mode` NOT NULL 不可 clear；至少一旗標；值＋`--clear` 衝突失敗。
- 驗證：`checks::normalize_mode`；**合併後**的 from/to/mode 重跑 `checks::guard_segment`（與 `set-route-segment` 同一道門，`:58`）；`--start-time` 走 `validate_time_flag`；`--duration` 正整數。
- 不動 `source`、`record_status`、`record_note`。原地 `UPDATE`，不 delete＋insert；有紀錄的段可改事實欄位，但**拒絕改 `--from/--to`**（紀錄會指向另一條路，同 `ensure_title_editable` 精神）。
- 冪等：合併後相同 → 「No change」。
- **交易（M3）**：全程 `begin_immediate`…`finish_tx`。
- **audit（L2）**：事件名 `route_segment_patched`（與既有 `route_segment_updated` 區分）；KV：`day_number`、`sort_order`、`changed`、每個變動欄位 `from.<欄>`／`to.<欄>`，via 變動時 `via_count`。`command_type`＝`update-route-segment`。注意既有 `set-route-segment` 的 KV 不含 notes／start_time（`set_route_segment.rs:442-444` 註解），覆蓋無法從 audit 復原；新指令補齊，不回改舊指令。

**`delete-route-segment <day> <sort_order>`**（同樣單一交易）
- 先 `ensure_unrecorded`（`RecordScope::LegSlot`，`repo/records.rs:743`）；有紀錄就拒絕。
- 刪列（連同其 via 子表列）後**重編號、不留洞**。**M4：兩段式負值位移**（比照 `swap_session_day_numbers` 的 TMP 手法，`repo/itinerary.rs:1664`，不是靠逐列遞增的偶然順序）：階段一 `UPDATE … SET sort_order = -1 - sort_order WHERE … AND sort_order > ?removed`（全落在負數區、互不衝突）；階段二 `UPDATE … SET sort_order = -sort_order - 2 WHERE … AND sort_order < 0`（還原成 old−1）。**同一個 repo 函式**對 `day_route_segments` 與 `day_route_segment_via` 各做同樣兩段，不得分散成兩個函式。測試要求：5 段刪第 2 段，主鍵無衝突、結果連號。
- 不直接動 `map_legend_stops`（snapshot-maps 的衍生輸出，每次整批重寫，`snapshot_maps.rs:1366`）；`touch_day` 讓 `check-maps-fresh` 判過期，輸出提示 `run snapshot-maps`。
- 不刪 road-leg 快取（`route_road_legs` 全域、以端點座標為鍵，`schema.sql:1173`，可能被別的計畫重用）。
- `derive-routes` 語意：刪除不改其他段 `source`；若被刪的是 `ai_recommended` 段，輸出提示「`derive-routes` 重跑可能重生此段（把其餘段 confirm 才會凍結）」；刪 `confirmed` 段不受影響（`derive_routes.rs:107` 只清 ai_recommended）。
- audit：事件 `route_segment_deleted`；KV：`day_number`、`sort_order`、被刪段 from/to/mode/duration/start_time/notes 全文、`renumbered_count`。

### 4.4 文件、測試
- 文件：`CLI.md:247-248`、`main.rs` 分派＋`--help`＋指令總表（`:1235`）、`routing.md`「旅途中改變行程」。
- 單元：`SegmentPatch` 解析與 `--clear` 白名單；合併後 guard（鐵路段改 `--mode walking` 被擋）；重編號兩段式純函式（含輸入有洞）。
- 整合（新檔 `tests/update_route_segment.rs`）：只改 duration，其他欄位與 `source` 不變（咬合：先建 `ai_recommended` 段，改完仍是；對比 `set-route-segment` 會翻成 confirmed）；`--clear start_time`；有紀錄段改 duration 成功、改 `--to` 被拒；同值零 audit；刪 5 段中的第 2 段 → 連號且 audit KV 含被刪內容；有紀錄段刪除被拒零寫入；刪 `ai_recommended` 段印提示；中途失敗（例如 guard 不過）零寫入；交易回滾（以刻意失敗的第二步驗證）。

---

## 5. `move-activity`：標題多筆即失敗＋交易化（縮小範圍）

### 5.1 決定
Yang 選擇**不做** `--after/--first`。留下兩件事，都是修正而非新功能：
1. 標題子字串比對多筆時**失敗並列候選**（Q3）。現況 `run_move` 用 `find_activity`（`set_activity.rs:466`，多筆取第一筆），可能搬錯。
2. `run_move`（`set_activity.rs:1567` 起）目前沒有 `begin_immediate`，多個寫入（`move_activity`、兩次 `touch_day`、事件）不在同一交易；包進 `begin_immediate`/`finish_tx`。

### 5.2 設計
- 比對改用 `records::match_activity_unique`（與 `update-activity` 同）；其餘行為不變：追加到目標 session 尾端；來源與目標 (day, session) 相同仍報「nothing to move」；跨日搬有紀錄者被擋（X4，`repo/itinerary.rs:907-933`）；`move_activity` 不碰 `updated_at`。
- 要改位置（插到第 N 個）→ 用 `reorder-activities`（`main.rs:761`，需列出該 session 全部活動）。**`CLI.md` 與 `main.rs:756` 的 usage 文字都明確寫**：「`move-activity` 只追加到尾端；要指定位置請用 `reorder-activities`」。
- audit：既有事件 `activity_moved`（`set_activity.rs:1621-1635`）不變。

### 5.3 測試
- 先讀 `tests/move_and_after.rs` 與 `set_activity_mutation_behavior_lock.rs`，若鎖定了「多筆取第一筆」就更新並在 CLI.md 註明這個行為修正。
- 新增：多筆符合 → 失敗、列候選、資料與 audit 零變更（咬合）；單筆行為與搬移後 id／poi_id／座標／`requested_by` 不變；交易回滾（事件步驟失敗時 `day_number/session/sort_order` 不變）。

---

## 6. road-leg 的 via 隨路線段儲存（v1）

### 6.1 問題
OSRM 永遠回最快路徑，要走台2濱海線必須給 via 點（`road_leg.rs:1-18`）。但 via 只存在於 `road-leg refetch --via` 那一次呼叫；snapshot-maps 遇快取未命中就用空 via 抓最快路徑（`snapshot_maps.rs:2702` `fetch_osrm_leg(w, f, t, vec![])`），每次重抓都得再貼一次長清單。

### 6.2 可行性與實用性
Yang 要求「走濱海線」的國內自駕行程，中頻但必然多次重抓。現有替代是把 via 點塞成路線中途停靠（污染圖例與行程文字）。值得做。**v1 範圍（決定 C）**：存、讀、用於「快取未命中」與 `road-leg refetch --segment`；**不做** `via_sig` 自動失效偵測。已知限制：設了 via 之後，已快取的舊幾何要靠 `road-leg refetch --segment D S` 手動換掉（指令會**印出確切指令**，Q4）。

### 6.3 設計

**資料（正規化，無 JSON）**：新表
```
CREATE TABLE IF NOT EXISTS day_route_segment_via (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  via_order INTEGER NOT NULL,
  lat REAL NOT NULL, lon REAL NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order, via_order)
);
```
只存座標（snapshot-maps 與 verify-site 要可重現，不能依賴地名解析）。無 `label` 欄、無 `via_sig`。

**`--via` 解析方式（已選定）：只收 `lat,lon`，完全離線**。`update-route-segment --via "25.289700,121.569000;25.231700,121.501800"`（分號分隔、順序＝行進順序），每個 token 經 `checks::validate_coords`（含 (0,0) 拒絕）。理由：寫入指令在交易內不得打網路（§0）；via 本就該是「道路上的點」，用地名會落到行政區重心而繞進內陸（`road_leg.rs:232` 的警告就是為此）。要由地名取得座標，用既有 `road-leg refetch --via <地名>` 看它印的解析結果，或 `set-place-geocode`／地圖查座標再貼進來。

**CLI**
- `update-route-segment … --via "<lat,lon;…>"`（整組取代）／`--clear via`。不連網，只存；輸出印確切指令：`next: travel road-leg refetch --segment <D> <S> [--dest <slug>]  then  travel snapshot-maps`（Q4）。`--clear via` 同樣印此指令（用以丟掉舊 via 幾何；無 via 的 `--segment` refetch 即最快路徑）。
- `road-leg refetch --segment <day> <sort_order> [--plan-id ..] [--dest ..]`：端點取自該段 from/to 標籤（以既有 `resolve_token` 解析，`road_leg.rs:146`；這是 `road-leg` 既有的快取維護指令，非 audit 指令，可連網）；via 取自子表；與 `--from/--to/--via` 互斥；舊的手動 `--via` 用法保留不變。`road-leg list` 不變。

**snapshot-maps 使用 via（僅快取未命中）**
- `MapSource`/`DbResolver`（`:305`）載入 `via_hints: HashMap<(coord_key_from, coord_key_to), Vec<(f64,f64)>>`，鍵以該段兩端點解析後座標（`assemble_maps` 已逐段 `resolve_segment_label`，順便記下）。
- `cached_routes`（`:2624`）：快取命中 → 照舊（不看 via）；**未命中** → `fetch_osrm_leg(w, f, t, hints.get(pair).cloned().unwrap_or_default())`（取代 `:2702` 的 `vec![]`）。`CacheOnly`（verify-site）不抓，行為不變。快取鍵（`:2662` `exact`＝`lat,lon>lat,lon|osrm-demo|driving`）全域以端點為鍵，不含 via——沿用。
- 因快取命中不看 via，**v1 的使用流程**＝設 via → `road-leg refetch --segment` → `snapshot-maps`。下一輪若要自動失效再加 `via_sig`（本案不做）。

**路線段身分（sort_order 位置鍵）與替換規則（決定 C）**
- via 鍵是 `(plan,dest,day,sort_order)`，跟著位置。
- `delete-route-segment`：via 列連動刪除並與主表同函式重編號（§4.3 M4）。
- **整條改寫時的保留規則**（`set-route-segments-bulk`、`set-route-segment` 同槽重寫、`derive-routes` 重生 ai_recommended）：先在交易內快照被取代的舊段（`from_place,to_place`＋其 via 列）；寫入新段後，對每個新段找「`from_place` 與 `to_place` 文字完全相同且尚未被取用」的舊段，**搬回其 via**（重編到新 `sort_order`）；其餘新段無 via、舊段的 via 隨之消失。**輸出明列**：`via kept: D<d> S<n> 關渡→川岩 (3 點)` 與 `via cleared: D<d> S<n> 舊 A→B（端點已變）`。此邏輯放 `repo/route_segments.rs` 的單一函式 `carry_via_over(old_snapshot, new_segments)`，三條路徑共用，不各寫一份。因此 `set-route-segment`／`bulk`／`derive-routes` 本案一併包進 `begin_immediate`（否則 delete 成功、insert 失敗會同時丟 via）。
- `update-route-segment` 改 `--from/--to` 而未同時給 `--via`／`--clear via` → 清 via 並印同樣 `via cleared`；只改 duration／notes／mode／start-time → 保留。
- `delete_slot`／`delete_all_for_day`／`delete_ai_recommended_for_day`（`repo/route_segments.rs:35-119`）各自連動清 via 子表，不留孤兒。
- **H4（審查必修）swap-days**：`swap_session_day_numbers` 的 `SESSION_TABLES`（`repo/itinerary.rs:1654-1661`，現有 `timesofday`、`activities`、`session_meals`、`session_activities_zh`、`day_route_segments`、`day_landmarks`）**加入 `day_route_segment_via`**（它有 `day_number` 欄，且若漏掉，換日後 via 會留在舊天、卻對到別天的 sort_order，錯掛到不相干的路線段）。行為鎖測試：擴充 `tests/swap_days_repoint_behavior_lock.rs`——兩天各有帶 via 的路線段，`swap-days` 後 via 列跟著各自的路線段（以 `(day,sort_order,via_order,lat,lon)` 全比對，咬合：把表從清單移除必紅）；另一個「行為鎖」斷言是既有六張表的行為不變。

**OSRM 端點吸附（須知，不是缺陷）**：OSRM 會把每個座標（含起訖點與 via）吸附到最近可行駛道路；海邊咖啡廳端點可能被吸到鄰近道路，`anchor_to_stops`（`snapshot_maps.rs:2495`）把幾何頭尾接回原釘點，所以圖上線頭到店門而實際路徑起點在隔壁路。via 若落在非濱海的內陸道路旁，會被吸附到錯的路。文件註明「via 要放在目標道路上」；不新增吸附偵測。

**同端點、不同 via 的限制**：快取以端點為鍵、全域共享，同一對端點出現在兩段而 via 不同，後抓者覆蓋。`update-route-segment --via` 偵測到「同計畫另一段端點相同但 via 不同」時印警告，不阻擋。

**validate data**（純函式 `via_issues`）：WARN——`day_route_segment_via` 孤兒列（無對應 `day_route_segments`）；WARN——via 離 hub 超過 500 km。

### 6.4 文件、遷移、測試
- 文件：`CLI.md:159-160`（road-leg）、`update-route-segment` 條目、`routing.md`（國內濱海線做法：存 via → `road-leg refetch --segment`）、`dashboard.md:16`。
- 遷移：僅 `CREATE TABLE IF NOT EXISTS`；不回填。
- 單元：`--via` 解析（空字串、非 `lat,lon`、(0,0)、超界）；`carry_via_over`（相同端點保留並重編號、端點改變清除、重複端點只取用一次、輸出明細）；`osrm_url` 既有測試（`snapshot_maps.rs:4637`）不動；`cached_routes` 的 hint 取用（假 resolver：未命中且有 hint → via 傳入；命中 → 不看 hint）。
- 整合：`--via` 寫入有序列、KV `via_count`；`--clear via`；改 `--to` 無 `--via` → 清除；`set-route-segment` 同槽相同端點 → 保留、改端點 → 清除並印；**bulk 重寫** 兩段保留一段清除、輸出列出；`delete-route-segment` 第 2 段 → 後面段的 via `sort_order` 同步 −1；swap-days 鎖測試；`road-leg refetch --segment` 只測到「組出的請求 URL 含 via、端點取自該段」（不打外網）。

---

## 7. `requested_by`（誰指定的，僅活動）

### 7.1 問題與可行性
「誰要的」被塞在標題（「Ping 指定」「Yang 指定優先」），標題變長、無法查詢。旅伴看行程時一眼看到誰想去的；代理排程時避免動別人指定的點（記憶規則「貼的地點是優先」）。名字只會是同行夥伴；owner 已決定不做遮罩，檢視者看得到。**餐點不做**（決定 D）。值得做，小。

### 7.2 設計
- **資料**：`activities.requested_by TEXT`（`add_column`，nullable、無預設、無 CHECK）。
- **CLI**：`add-activity … --requested-by <name>`、`update-activity … --requested-by <name>`、`update-activity --clear requested_by`（`--clear` 白名單加一項，`set_activity.rs:1311` 一帶）。`ActivityRow`／`ActivityPatch` 加此欄；`activity_row_diff` 在**固定欄位序尾端**追加，既有 diff 測試順序不變。
- **驗證（Rust）**：trim 後非空；**1–16 個字元**（Unicode 字元計）；不含換行與 `http`／`<`／`>`；**超長失敗，不截斷**。同值＝「No change」。
- audit：沿用 `activity_updated`（`changed` 含 `requested_by`、`from./to.requested_by`）。
- `view-itinerary`：活動行尾加 `〔指定：<name>〕`（CLI 人讀輸出，標籤字樣在 view 的格式化常數）。與 §1 的備註行同一個 view 變更，一次更新行為鎖測試。
- **worker**：`router.rs:936` SELECT 加 `requested_by`；`model.rs` `Activity` 加欄；`render/session.rs` 活動行在 `ai_rec_badge` 之後加 `<span class="req-chip">`（名字轉義；它是資料）；`render/now.rs:274` 的 `now-act` 同步。i18n（chrome）新增一組 ZH/EN，緊鄰 `recDone`：`reqBy`＝ZH「{name} 指定」、EN「Requested by {name}」，以 `tf`（`i18n.rs:633`）填入。CSS 一條（沿用 `record-badge` 小膠囊尺寸），並加 `max-width`＋`overflow-wrap:anywhere` 防舊資料／手改資料撐版。分享檢視者看得到。
- 搬移：`move-activity` 只 re-key，不動此欄。`copy-plan` 依欄位資訊複製（同 §2.4 待證實點）。

### 7.3 validate data
WARN——`requested_by` 長度 >16 或含換行（直接 `db exec` 的髒資料）；INFO——標題仍含「指定」但 `requested_by` 為 NULL（提示搬到欄位；既有資料由整合者以 `update-activity --requested-by` 逐筆做）。關鍵字放程式碼。

### 7.4 測試
- 單元：名字驗證（空、17 字、剛好 16 字、含換行、CJK 與 emoji 字元計數、前後空白）；`parse_update`／`parse_add` 的新旗標與 `--clear requested_by`；worker 有／無 chip × zh／en 的 HTML（名字 `<b>` 跳脫咬合）；`reqBy` 兩組字串。
- 整合（擴充 `tests/update_activity.rs`）：設／改／清；同值零 audit；超長失敗零寫入；`move-activity` 後保留。
- 部署後 `verify-site`（ne-coast、jiufen）不退步。

---

## 8. 實作分線（lanes）與檔案所有權

**共用檔案規則**：`main.rs` 與 `docs/reference/CLI.md` 是共用點——各線**不直接合併進**，而是各自在自己的 PR／工作區提供「分派區塊＋usage＋指令總表一行」與「CLI.md 條目文字」，由**整合者**最後合併（衝突只會是相鄰行）。`checks.rs` 與 `repo/mod.rs`：**只有 L0 修改**；其他線唯讀引用。

| 線 | 範圍 | 獨佔檔案 | 相依 |
|---|---|---|---|
| **L0 Schema＋共用小件** | `activities.lat/lon/geo_source/requested_by`、新表 `day_route_segment_via`；重生 `scripts/schema.sql`；建立 `repo/text_merge.rs`（`append_note` 純函式＋單元測試）並在 `repo/mod.rs` 註冊；`checks::validate_coords`（純函式＋單元測試） | `db_migrate.rs`、`scripts/schema.sql`、`repo/mod.rs`、`repo/text_merge.rs`、`checks.rs` | 最先；其餘線都依賴它 |
| **L1 活動 CLI** | `set-activity-geocode`、`--append-notes`、`move-activity` 唯一比對＋交易、`--requested-by`、`add-activity` 座標旗標、`update_accommodation.rs` 改呼叫共用座標驗證、`view-itinerary` 的備註與指定人輸出、`SESSION_TABLES` 加 `day_route_segment_via`＋swap-days 行為鎖測試 | `set_activity.rs`、新檔 `set_activity_geocode.rs`、`repo/itinerary.rs`（含 `SESSION_TABLES`）、`update_accommodation.rs`、`view_itinerary.rs`、`plan.rs`、`tests/`：`update_activity.rs`、`set_activity_geocode.rs`、`move_and_after.rs`、`set_activity_mutation_behavior_lock.rs`、`swap_days_repoint_behavior_lock.rs`、view-itinerary 測試 | L0 |
| **L2＋L3 路線段／via／地圖（合併）** | `update-route-segment`、`delete-route-segment`、via 子表讀寫與 `carry_via_over`、`set-route-segment`／bulk／`derive-routes` 包交易、`road-leg refetch --segment`、snapshot-maps 的活動釘（`pin_only`、H1／H2、編號、雜湊、`RENDER_REV`）與 via 快取未命中使用 | 新檔 `update_route_segment.rs`、`set_route_segment.rs`、`repo/route_segments.rs`、`road_leg.rs`、`derive_routes.rs`、`snapshot_maps.rs`、`check_maps_fresh.rs`（若需）、`tests/`：`update_route_segment.rs`、`route_segment_guard.rs`、`road_leg.rs`、`derive_routes_*`、maps 相關 | L0 |
| **L4 worker** | 活動備註顯示、活動座標優先的 `maps_link`、`req-chip`、`reqBy` i18n、一條 CSS、verify-site record 層的 notes 檢查 | `workers/trip-dashboard-rs/src/*`（`router.rs`、`model.rs`、`render/session.rs`、`render/now.rs`、`i18n.rs`）、`verify_site_record.rs` | L0（SELECT 欄位）；**最後部署** |
| **L5 validate＋文件** | `validate.rs` 三組規則（活動座標、via 孤兒／離 hub、`requested_by`／標題「指定」）；`dashboard.md`（含 L1 備註：活動釘編號不代表遊覽順序）、`routing.md`、`docs/trips/2026-ne-coast.md`；最後與整合者合併 `CLI.md` | `validate.rs`、`docs/**`（`CLI.md` 由整合者最終合併） | 規則細節依 L1／L2+L3 定案；可並行起草 |

L2+L3 合併的理由：兩者都碰 `snapshot_maps.rs` 與 route 段落的 repo，拆開會互踩。L1 與 L2+L3 唯一交集是 `repo/itinerary.rs`（`SESSION_TABLES`）與 `repo/route_segments.rs` 分屬不同檔，無衝突。

順序：L0 → （L1、L2+L3、L4 並行）→ L5 → 整合（合併 `main.rs`、`CLI.md`；`make test`〔背景〕；`validate data`）→ `db migrate` → 部署 worker → 整合者對 ne-coast 回填（§9 Q8）。

估工（人天）：L0 0.3、L1 1.0、L2+L3 1.75、L4 0.75、L5 0.5、整合 0.5；合計約 4.8，三線並行約 3。

---

## 9. 已決定事項（原 Q1–Q8，皆採建議答案）

1. **Q1 改標題不清活動座標**；改標題且有座標時印 note，**note 內含座標**（§2.3）。
2. **Q2 路線段備註追加用「；」**，活動備註用換行（§3.3）。
3. **Q3 `move-activity` 多筆符合改為失敗並列候選**（§5）。
4. **Q4 `--clear via` 不自動丟舊幾何**；設／清 via 後**印出確切的 `road-leg refetch --segment D S` 指令**（§6.3）。
5. **Q5 餐點指定人不做**（決定 D，原問題作廢）。
6. **Q6 `requested_by` 上限 16 字元、超長失敗不截斷**（§7.2）。
7. **Q7 圖例不區分活動釘與路線點**（§2.3）。
8. **Q8 既有資料回填由整合者部署後以新指令逐筆做，順序固定**：(1) 用 `set-activity-geocode` 釘三個地點（川岩、滿山望海、三民大飯店）→ `snapshot-maps` 確認釘出現；(2) **刪除為了釘點而加的繞路路線段**（`delete-route-segment`，注意重編號與 `ai_recommended` 提示）→ `snapshot-maps`；(3) 為濱海線設 via（`update-route-segment --via` → `road-leg refetch --segment` → `snapshot-maps`）；(4) 標題裡的「指定」改成 `--requested-by`；每步之後都跑一次 `snapshot-maps`，最後 `verify-site`。部署前另需人工掃過既有活動備註（§1.0.1）。

---

## 10. 共通測試規則

- 真 Turso 整合測試：`zz_test_<nanos>_*` 的 plan／destination／活動 id；plan-id 綁定後**立刻** arm RAII `Guard`；一律 `common::teardown_plan`（不手寫表單）；新表 `day_route_segment_via` 是 plan 鍵表，測試明確斷言「teardown 後該表無此 plan 列」以證明沒洩漏；全域列（測試用 `route_road_legs`）須在 `teardown_plan` 之前本地刪除。
- 序列化的 Turso 測試在**背景**跑；失敗先單獨重跑再判斷（共享 Turso 競爭）。
- 每項至少一個「咬合」斷言：拿掉被測行為，測試必須變紅（已在各節列出）。
- 不對正式資料斷言；不在測試裡打外網（OSRM／Nominatim）。
