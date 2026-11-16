# 行程紀錄（Trip Record）設計

- 狀態：Yang 已核可（2026-11-16，「都照建議」）；下一步寫實作計畫，計畫審過才派工。
- 日期：2026-11-16（kyoto-nov-2026 回程日）
- 審查：K3.1 Pro（後端，block）、Fable 5.2（concern）。agy（Gemini 4 Argon Medium／High）兩次都在無人值守模式被權限擋下、沒有產出，本版沒有 agy 意見。核對紀錄見 §12。
- 姊妹功能：[旅行筆記](2026-11-15-travel-notes-design.md)、[天氣抓取強化＋now 卡時區](2026-11-16-weather-provider-fallback-design.md)。敏感資訊遮罩已由 Yang 取消（2026-11-16），本文件與它無關。

## 0. 可行性（先看這一節）

**實際使用情境**：旅途中行程一天改好幾次（午餐換店、景點跳過、時間挪動）。Yang 在對話裡告訴 AI 代理，代理用 CLI 寫進 Turso；家人用分享連結看網站。Yang 要的是：「過了的階段留下紀錄，清楚區分是我們真實的行程」；原計畫不是重點。

**這兩天實際發生的痛點**（Turso 實查）：
1. 狀態寫進活動標題和餐點文字（`✅ 實際行程：…`、`⏭ 未前往…`），網站分不出計畫和實際，只能靠讀者看 emoji。
2. 缺「改活動備註」「追加一筆餐點」的指令，代理只好刪除再新增，活動 id 一直換。
3. 沒去的路線（Day 4 去京極スタンド的 6 段）照樣畫在地圖上。

**比較過的做法**：
- **(A) 只加時段層級的「實際紀錄」文字**（代理寫一段話，過了的時段改顯示這段話）：最省，但痛點 1、3 都沒解：標題照樣被改、地圖照樣畫沒去的地方，而且一段話無法讓地圖或筆記對應到某個地點。**不夠。**
- **(B) 原設計**：三張表加狀態、實際時間、added 狀態、路線實際耗時、回填稽核、`record-status` 指令…審查指出它還要擋掉 5 條會刪紀錄的既有指令、處理跨日搬移、時鐘兩套。範圍太大。
- **(C) 縮小版（建議）**：只在「一列就是一件事」的地方記一個狀態，其餘沿用現有欄位：
  - 活動、餐點、路線段各加 `record_status`（`done`／`skipped`）和 `record_note`。
  - **不另開實際時間欄位**：旅途中代理本來就把 `start_time`／`end_time` 改成實際時間（Day 5 上午五筆就是）；過了的列，時間就是實際時間。
  - **不分 added**：臨時加入的就是新增一筆活動再標 `done`；要知道它是不是臨時加的，看 audit 就有。
  - 時段只加一欄 `record_summary_zh`（一兩句實況），取代現在被改寫的 `focus`。

**結論**：做 (C)。它直接解掉三個痛點，網站能清楚標出「實際」，而要防的「紀錄被既有指令洗掉」縮成一個 repo 層的共用檢查。原計畫被覆寫的值（例如時間）就放棄，符合 Yang「原計畫不是重點」。

## 1. 目標（v1）

- **G1 狀態是資料**：活動、餐點、路線段可標 `done`（實際發生）或 `skipped`（未前往），附一句 `record_note`。NULL＝未紀錄。
- **G2 CLI 標記、有 audit**：一個指令標一列，走 audit triad，包在 `begin_immediate` 交易裡。
- **G3 網站清楚區分**：過了的時段，`done` 以「實際」呈現，`skipped` 收成一行灰字，未紀錄的標「計畫・未紀錄」；未到的時段照舊。
- **G4 補齊編輯指令**：改活動備註、追加一筆餐點，從此不必刪除再新增。
- **G5 清掉現有標記**：kyoto-nov-2026 Day 4–5 的文字標記轉成欄位，標題恢復成地點名稱。

## 2. 非目標

- 不做計畫與實際的對照畫面，不保存原計畫快照。
- 不另開實際時間、實際耗時欄位；不分 `added` 狀態；不記「誰紀錄的」（CLI 沒有可信的身份來源，K3#8）。
- 不做 `record-status` 唯讀指令和日卡「行程紀錄」chip（延後）。
- 不在網站上寫紀錄；紀錄和行程一樣只用 CLI 寫。
- 不處理替代方案的「未採用」標示：替代方案列不能標記，畫面照舊（K3#7：沒有紀錄就不能推定主方案是實際）。

## 3. 反目標

- **X1 計畫冒充實際**：過了的時段裡，沒有狀態的列不能用「實際」樣式呈現。
- **X2 猜測狀態**：沒有證據不標。遷移只轉換已經寫在文字裡的標記。
- **X3 紀錄被既有指令洗掉**：任何會刪除或重建這三張表列的指令，碰到有 `record_status` 的列就失敗（§5）。
- **X4 紀錄被搬到別天**：有紀錄的列不能跨日移動或被 `swap-days` 換走。
- 既有規則全部適用：不加 constraint、不用 JSON 欄位、不做 feature flag、介面文字放 `i18n.rs`。

## 4. 資料模型

`db_migrate.rs` 用 `ALTER TABLE ADD COLUMN`，全部可 NULL、無 CHECK、無預設內容：

```
activities          +record_status TEXT   -- NULL | 'done' | 'skipped'（合法值由 CLI 的 Rust enum 驗證）
                    +record_note   TEXT
session_meals       +record_status TEXT, +record_note TEXT
day_route_segments  +record_status TEXT, +record_note TEXT
timesofday          +record_summary_zh TEXT   -- 過了的時段顯示它；worker 本來就只讀 focus_zh（router.rs:867-869），不做英文欄
```

- `record_note` 與既有規劃 `notes` 分開。worker 目前不讀 `activities.notes`（`router.rs:872-874`），v1 也不渲染它，只渲染 `record_note`。
- 餐點、路線段沒有 id（主鍵含 `sort_order`，`scripts/schema.sql:1146-1152`、`:280-286`）。v1 不加 id（設計者判斷）：用 §5 的唯一比對定位，並補 `set-meal`（原地改一筆）讓修改不必整批取代。若實作時發現 sort_order 定位太脆弱，改加穩定 id，回報後再做。

## 5. CLI

**標記**（都用 `begin_immediate` 交易包住 domain write 與 audit triad；`plan_events` 類型 `record_marked`／`record_cleared`）：

```
travel record-activity <day> <session> <id|title> done|skipped [--note "<t>"]
travel record-meal     <day> <session> <text-match> done|skipped [--note "<t>"]
travel record-leg      <day> <sort_order> done|skipped [--note "<t>"]
travel record-session  <day> <session> --zh "<t>" | --clear
travel clear-record    activity|meal|leg <day> ... <key>     # status 與 note 一起清回 NULL
```

- **唯一比對**：`<id|title>`、`<text-match>` 只接受唯一符合，多筆就失敗並列出候選。另寫 resolver，不改 `set_activity.rs:509-530` 既有「取第一筆」的行為（Fable#3、K3#4）。
- 省略 `--note` 表示不變；重新標記直接覆寫狀態（不必先 clear）。
- 不能標記替代方案列（`variant_of` 非 NULL），也不能標記目的地「今天」之後的日子。「今天」用 `destination_config.timezone` 算；CLI 現有 `today_iso()` 是 UTC（`plan_resolver.rs:304-311`），要另寫目的地當地日期 helper（K3#9）。
- 標記活動或路線段時更新 `activities.updated_at`／`touch_day`，讓 `check-maps-fresh`（`check_maps_fresh.rs:42-47`）判定地圖過期（Fable#11、K3#5）。

**防洗掉（X3），放在 repo 層**：凡是刪除或整批取代 `activities`、`session_meals`、`day_route_segments` 列的 repo 函式，先查範圍內有沒有 `record_status IS NOT NULL` 的列，有就失敗並列出。已核對的路徑：
- `set-meals`、`clear-meals`、`set-route-segments-bulk`、`delete-activity`（原設計已列）
- `set-route-segment`：實際是先刪再插（`set_route_segment.rs:416-420`）
- `derive-routes`：刪掉當天所有 `ai_recommended` 段再重建（`derive_routes.rs:107-115`）
- `scaffold-itinerary --force` → `replace_skeleton`：刪整個目的地（`repo/itinerary.rs:56-72`）
- `remove-day-variant`：刪整天內容（`repo/days.rs:457-474`）

**防跨日（X4）**：`move-activity --to-day` 與 `swap-days` 碰到有紀錄的列就拒絕；同日內換時段可以，欄位跟著走。`copy-plan` 是改名用途，紀錄照搬（schema 驅動，`copy_plan.rs:5-12`，不需改碼，只加測試）。

**補齊的編輯指令（G4）**：
```
travel set-activity-notes <day> <session> <id|title> --notes "<t>" | --clear   # 不動 id、poi_id
travel add-meal <day> <session> --meal "<t>" [--recommended]                   # 追加一筆，不取代
travel set-meal <day> <session> <text-match> --meal "<t>"                      # 原地改一筆
```
（沒有 `--zh`：餐點只有一欄文字，`set_tod.rs:351-354`。）

**lint**：`validate data` 加一項 WARN：標題、餐點文字、路線 notes、時段 focus、活動 notes 裡出現 `✅`、`⏭`、「實際行程」、「實際用餐」、「未前往」。只比對 emoji 和中文標記，不比對英文 `visited`／`skipped`，避免誤報（Fable#15）。關鍵字放程式碼，屬 CLAUDE.md 允許的「拿來比對資料的關鍵字」。

## 6. 網站呈現

SSR、無新 JS；介面文字放 `i18n.rs`（實際／Actual、未前往／Skipped、計畫・未紀錄／Planned · not recorded）。

- **「已過」**：時段結束早於現在。時鐘依賴天氣文件的 now 卡時區修正（`destination_config.timezone`），不再寫死日本時間；本功能在它之後上線。去回程日的台北時間列，以時段為單位判斷，接受頂多一小時的誤差（設計者判斷，§11 不另問）。
- **活動**：`done` 實心加「實際」；`skipped` 不混排，時段最後一行灰字「未前往：東寺」；過了但 NULL 標「計畫・未紀錄」。
- **時段標頭**：過了且有 `record_summary_zh` 就顯示它，取代 `focus_zh`。
- **餐點**：用餐總覽與 now 卡的「主選」改成第一筆非 `skipped` 的列（`render/day.rs:434-460`、`render/now.rs:193-217` 目前固定取第一列，K3#5、Fable#18）。
- **待訂位警示**：`skipped` 的活動不再警示（`render/alerts.rs:197-198`）。
- **地圖**：`snapshot-maps` 查詢濾掉 `skipped` 的活動與路線段；畫的點線變了，input hash 自然跟著變（`snapshot_maps.rs:1383`）。
- **分享**：plan 分享連結的檢視者看到跟擁有者一樣的紀錄。
- 行內腳本每分鐘只重標 now 卡；日卡的「計畫・未紀錄」只在 SSR 當下判斷，可接受。

## 7. 清掉現有標記（kyoto-nov-2026 Day 4–5）

順序與工具（Fable#14、K3#1：不用一次大 `db exec` 寫稽核）：
1. 盤點（唯讀 SELECT）：列出含標記的標題、餐點、路線 notes、focus、活動 notes，給 Yang 看。
2. **狀態**：用新做好的 `record-*` 指令逐列標記，每筆自帶 audit，也等於實測指令。對照：`✅`／已參觀／實際行程／實際用餐 → `done`；`⏭`／未前往 → `skipped`；票價、改去哪裡等說明 → `--note`。
3. **文字清理**：一次性 `db exec`，一句 UPDATE 只改一列的標題或文字，每句檢查 affected rows。直接 UPDATE 標題不會觸發 `set-activity-title` 的清 poi 防呆；東福寺、東寺的 `poi_id` 目前仍是 `tofukuji`、`toji`（Turso 實查），會保留。這一步是冪等的，失敗可重跑，lint 歸 0 即完成。
4. focus 裡的實況句子移到 `record_summary_zh`；focus 保留一句中性描述，`validate publish` 不會因此變紅（Fable#17）。
5. 驗證：lint WARN＝0、`validate publish`、`snapshot-maps` 重畫 Day 4–5、`verify-site --at 2026-11-16T20:00`。
6. 沒標記的列（Day 4 路線段 0–4、Day 5 中午與晚上）保持 NULL，等 Yang 確認（§11 Q3）。

## 8. 測試

- **Worker 單元測試**：done／skipped／NULL 在已過、未到時段的輸出；餐點主選略過 skipped；skipped 不出警示；台北時區 destination 的已過判斷。
- **CLI Turso 整合測試**（背景執行、`zz_test_<nanos>_*`、bind 後立刻 arm `Guard`、`common::teardown_plan`）：
  - `record-*` 寫欄位與完整 audit triad；同值重跑不 bump。
  - 多筆符合時失敗；未來日期、替代方案列被拒。
  - §5 列出的每一條刪除路徑碰到有紀錄的列都失敗，資料與 audit 完全不變。
  - `move-activity --to-day`、`swap-days` 碰到有紀錄的列被拒。
  - `record-activity` 後 `check-maps-fresh` 判定過期。
  - `add-meal` 追加不取代；`set-activity-notes` 不改 id 與 `poi_id`。
- **lint 單元測試**：emoji 與中文標記。

## 9. 部署順序

`make build` → `./bin/travel db migrate`（只加欄位，重跑兩次確認冪等）→ 再生 `scripts/schema.sql`（不手改）→ §7 清標記 → 部署 worker（先 `unset CLOUDFLARE_API_TOKEN`；worker 用明確欄位清單查詢，欄位不存在整頁會失敗，所以一定在 migrate 之後）→ `snapshot-maps` → `verify-site`。天氣文件的 now 卡時區修正要先上線或同批上線。

## 10. 留給實作者的

- 欄位命名維持三張表一致即可。
- 餐點、路線段若用 sort_order 定位太脆弱，改加穩定 id（回報後做）。
- 文件更新：CLI.md、dashboard.md、routing.md 加「旅途中改變行程 → 改時間用既有指令、標結果用 `record-*`，不改標題、不刪除重建」。

## 11. Yang 的回答（2026-11-16）

**Yang 決定（2026-11-16）：全部照建議（「都照建議」）。**


1. **範圍照 §0 (C) 縮小版做，可以嗎？**（只記 done／skipped＋一句備註；過了的列時間就是實際時間；不分臨時加入。）建議：可以。
2. **旅途中被改成實際值的計畫欄位（時間、時段範圍、focus），直接當成實際紀錄、原計畫值就此放棄，可以嗎？** 建議：可以，符合「原計畫不是重點」。
3. **Day 4 的路線段 0–4、Day 5 中午與晚上這些沒標記的項目，要我列給你一次確認後標成實際，還是維持「計畫・未紀錄」？** 建議：列給你一次確認。
4. **過了但沒紀錄的項目，標「計畫・未紀錄」照樣顯示，可以嗎？還是隱藏？** 建議：照樣顯示。
5. **未前往的項目，在每個時段最後一行灰字列出，可以嗎？還是完全不顯示？** 建議：一行灰字。

## 12. 審查核對紀錄（2026-11-16）

| 議題 | 審查者 | 處理 | 理由 |
|---|---|---|---|
| X4 漏掉會刪紀錄的路徑（set-route-segment、derive-routes、replace_skeleton、remove-day-variant） | Fable#1、K3#2 | 採納（§5，repo 層共用檢查） | 已核對 `set_route_segment.rs:416-420`、`derive_routes.rs:107-115`、`itinerary.rs:56-72` |
| 回填用多語句 `db exec` 非原子、還寫完成 audit | K3#1、Fable#14 | 採納（§7 改用 record-* 指令；文字清理逐句、冪等） | 已核對 `db_exec.rs:119-141` 逐句執行 |
| 標題子字串取第一筆會標錯列 | Fable#3、K3#4 | 採納（§5 唯一比對） | 已核對 `set_activity.rs:509-530` |
| 「已過」時鐘寫死 JST | Fable#4、K3#9 | 採納（依賴天氣文件的時區修正；CLI 另寫當地日期 helper） | `today_iso()` 是 UTC，已核對 |
| 跨日 move／swap 搬走紀錄 | Fable#5、K3#3 | 採納（§5 拒絕） | — |
| 餐點、路線段用 sort_order 當 key | Fable#2 | 部分採納（唯一比對＋`set-meal`；穩定 id 留給實作者判斷） | 縮小範圍後重排需求變少 |
| clear-record／省略參數語意不清 | K3#4 | 採納（§5） | — |
| `add-meal --zh` 沒地方存 | Fable#12、K3#4 | 採納（拿掉） | `set_tod.rs:351-354` |
| 餐點主選、now 卡、警示對 skipped 的規則 | K3#5、Fable#18 | 採納（§6） | — |
| 地圖新鮮度要 touch updated_at | Fable#11、K3#5 | 採納（§5） | `check_maps_fresh.rs:42-47` 已核對 |
| 「過了的日子主方案＝實際」違反 X2 | K3#7 | 採納（v1 不做未採用標示） | — |
| 「紀錄者由 audit 保存」不實 | K3#8 | 採納（刪除承諾） | `operation_runs`、`plan_events` 無 actor 欄 |
| worker 根本不讀活動 id／notes／時間 | Fable#8 | 採納（§4、§6 明寫為新增渲染、不渲染 notes） | `router.rs:872-874` 已核對 |
| 新自由文字要經遮罩 | K3#6、Fable#8 部分 | 不採納 | 遮罩功能已由 Yang 取消 |
| 「改標題清 poi」因果錯誤 | Fable#9 | 採納（更正 §7） | Turso 實查：東福寺、東寺 poi 仍在；GODIVA、AEON 從未連過 |
| §1／§10 對 Turso 現況描述不符 | Fable#10 | 採納（§7 盤點改成實查清單） | — |
| 時段範圍已被改寫、重疊 | Fable#6 | 採納為問題（§11 Q2） | 屬原計畫值去留 |
| input hash 要含 record_status | Fable#11 | 不採納 | hash 對實際畫出的點線計算，濾掉 skipped 自然改變 |
| `validate publish` 會被回填弄紅 | Fable#17 | 採納（§7 第 4 步保留中性 focus） | — |
| cascade 不會抹紀錄要寫明 | Fable#7 | 採納（併入 §5 的路徑清單說明） | `date_change.rs` 只標 dirty |
| 部署順序、schema 再生 | Fable#16、K3#10 | 採納（§9） | — |
| 英文關鍵字誤報；X3 只有 WARN | Fable#15 | 採納（只比 emoji 與中文；X3 改成資料欄位後 lint 是輔助） | — |
| 砍 `record_summary` 英文、`added`、路線實際時間、`record-status`、chip | Fable#20、K3#11 | 採納 | 縮小範圍 |
| 實際時間另開欄位 | 原設計 | 不採納（改沿用 start/end） | 旅途中時間本來就被改成實際值 |

統計：約 25 項；採納 21、部分採納 1、不採納 3（遮罩已取消、hash 不需改、實際時間欄位）。
**收斂情形**：「既有指令會洗掉紀錄」「回填非原子」「標題取第一筆」「時鐘寫死 JST」「跨日搬移」兩位審查者各自指出，可信度高。只有 Fable 用 Turso 實查指出現況描述錯誤；只有 K3 指出 `today_iso()` 是 UTC 與「紀錄者」不實。agy 兩次未產出。
