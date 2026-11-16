# 行程紀錄（Trip Record）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**可行性檢查（Yang 規則，2026-11-16）：** 範圍照 Yang 核可的縮小版（spec §0 (C)）。這份計畫直接解掉旅途中實際碰到的三個痛點：
1. 狀態原本寫在標題裡，網站分不出計畫和實際。
2. 只能「刪除再新增」，活動 id 一直換。
3. 沒去的路線照樣畫在地圖上。

不做的部分：原計畫快照、實際時間欄、`added` 狀態。

**Goal:**
- 活動、餐點、路線段可以標成 `done` 或 `skipped`，並附一句備註；時段可以寫一段中文實況。
- 已經過去的時段，網站要清楚區分「實際」「未前往」「計畫・未紀錄」。
- 既有會刪除或重建資料的指令，碰到有紀錄的列一律失敗。

**Architecture:**
- 三張表各加兩欄，`timesofday` 加一欄，都由 `db_migrate.rs` 用 `ADD COLUMN` 加上。
- 新的 `travel-db` repo 模組 `records.rs` 負責三件事：唯一比對、標記、防洗掉檢查（`ensure_unrecorded`）。
- 所有刪除或整批取代的 repo 函式，第一步先呼叫 `ensure_unrecorded`。
- CLI 新指令放在新檔 `record.rs`。
- Worker 只多讀這些欄位，SSR 依天氣計畫產生的 `Clock` 判斷「已過」。

**Tech Stack:** Rust（travel-cli、travel-db、libsql 0.6）、workers-rs、Turso。

**Spec:** `docs/superpowers/specs/2026-11-16-trip-record-design.md`（§1–§9；§11 Yang 的回答：全部照建議）

**前置計畫：** `docs/plans/2026-11-16-weather-tz-fetch-hardening.md` 的 T5、T6（worker 的 `plan.timezone`、`Clock`）必須先合併；本計畫的 worker 任務在它之後部署。

## Global Constraints

- 欄位全部可以是 NULL，不加 CHECK，也不給預設內容；合法值由 Rust enum `RecordStatus { Done, Skipped }` 驗證，DB 裡存成 `'done'` 或 `'skipped'`。
- 每個標記指令都要用 `begin_immediate`／`finish_tx` 包住 domain write 和 audit triad。
  - `plan_events` 的類型是 `record_marked` 或 `record_cleared`，最後呼叫 `cascade::common::record_operation`。
  - 同值重跑不能 bump 版本。
- 唯一比對：多筆符合就失敗，並列出候選。不改 `set_activity.rs:509-530` 既有「取第一筆」的行為。
- 不能標記替代方案列（`variant_of` 非 NULL），也不能標記目的地「今天」之後的日子。「今天」用 `destination_config.timezone` 算，不用 `plan_resolver::today_iso()`（`:304`，UTC）。
- 介面文字放 `i18n.rs`：實際／Actual、未前往／Skipped、計畫・未紀錄／Planned · not recorded。
- 不新增 JS。lint 關鍵字放程式碼，只比對 `✅`、`⏭`、「實際行程」、「實際用餐」、「未前往」，不比對英文。
- 欄位命名三張表一致：`record_status`、`record_note`；時段欄位叫 `record_summary_zh`。
- 派出去的 AI 不跑 git、不派 subagent；commit 由整合者做。

## Review Focus

1. **同名活動**：同一時段有兩個「東寺」（例如一個是替代方案列）。主日只有一筆符合時，要能標記成功；變體列不能被選中（T3 測試 `variant_rows_not_candidates`）。
2. **標題含 `%`、`_` 或引號**：比對時要當成字面文字，不能變成 LIKE 萬用字元（T2 測試 `match_is_literal_not_like`）。
3. **目的地時區是 NULL**：標記「今天以前」無法判定時要失敗並說明原因，不能退回 UTC（T3 測試 `missing_timezone_fails_loud`）。
4. **`swap-days` 只有其中一天有紀錄**：整個指令要拒絕，兩天資料完全不動（T4 測試）。
5. **已過時段的餐點全部 `skipped`**：用餐總覽與 now 卡不能顯示任何主選，也不能顯示被跳過那家的名字當主選（T6 測試 `all_meals_skipped_shows_none`）。

---

## 檔案地圖

| 檔案 | 責任 | 任務 |
|---|---|---|
| `rust/crates/travel-cli/src/db_migrate.rs` | 7 個 `ADD COLUMN` | T1 |
| `rust/crates/travel-db/src/repo/records.rs`（新）、`repo/mod.rs` | `RecordStatus`、唯一比對、標記、`ensure_unrecorded` | T2 |
| `rust/crates/travel-cli/src/record.rs`（新）、`main.rs` | `record-*`、`clear-record`、目的地當地日期 | T3 |
| `travel-db/src/repo/itinerary.rs`、`route_segments.rs`、`days.rs` | 每個刪除／取代函式開頭呼叫守門 | T4 |
| `rust/crates/travel-cli/src/set_tod.rs`、`set_activity.rs`、`main.rs` | `add-meal`、`set-meal`、`set-activity-notes` | T5 |
| `rust/crates/travel-cli/src/validate.rs` | 標記 lint WARN | T5 |
| `workers/trip-dashboard-rs/src/router.rs:866-901`、`model.rs`、`render/session.rs`、`day.rs`、`now.rs`、`alerts.rs`、`i18n.rs` | 呈現 | T6 |
| `rust/crates/travel-cli/src/snapshot_maps.rs:2618,2648,3177`、`check_maps_fresh.rs` | 地圖濾掉 skipped | T7 |
| 文件、Turso 資料 | §7 清標記、部署 | T8（整合者） |

## 派工與並行

- **T1 → T2 必須先做**，因為後面所有任務都要用到 schema 與 repo 介面。
  - 實作：Codex（或 agy）。
  - 審查：後端順序，Codex → agy → K3.1 Pro，審查者不能是作者。
- **T2 合併後，T3、T4、T5 可以由不同 AI 並行**，但有兩組共用檔案要注意：
  - T3 和 T5 都會改 `main.rs` 的 dispatch：誰後合併誰 rebase，衝突只在 match arm，整合者合併時處理。
  - T4 只改 `travel-db` repo 與它的測試，跟 T3、T5 沒有共用檔案。
- **T6（worker，前端）**：在 T1 合併、而且天氣計畫的 T5 合併之後才開始，可以和 T3、T4、T5 並行。
  - 實作：agy。
  - 審查：agy → Codex → K3.1 Pro，審查者不能是作者。
- **T7** 依賴 T1，可以和 T6 並行；它碰 `snapshot_maps.rs`，不會和其他任務衝突。
- **T8** 由整合者做，要等所有任務合併；其中 §7 Q3 的「列給 Yang 確認」是人工關卡。

---

### Task 1: Schema 欄位

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs`：用既有的 `add_column`（`:50`）加 7 欄，放在既有 `ADD COLUMN` 區段的最後。
- Test: 同檔 inline `#[tokio::test]`（in-memory，比照 `:2846` 的 `mem_conn`）。

**Interfaces:**
- Produces:
  - `activities.record_status TEXT`、`activities.record_note TEXT`
  - `session_meals.record_status TEXT`、`session_meals.record_note TEXT`
  - `day_route_segments.record_status TEXT`、`day_route_segments.record_note TEXT`
  - `timesofday.record_summary_zh TEXT`

- [ ] **Step 1: 寫失敗的測試** `record_columns_added_idempotently`
  - 建立這四張表的最小 schema（取自 `scripts/schema.sql`）。
  - 呼叫新的 `add_record_columns(&conn)` 兩次。
  - 用 `PRAGMA table_info` 斷言 7 欄都存在，而且第二次呼叫不報錯。
- [ ] **Step 2: 執行**：`cd rust && cargo test -j 2 -p travel-cli db_migrate::tests::record_columns`，預期 FAIL。
- [ ] **Step 3: 實作**：新增 `async fn add_record_columns(conn: &libsql::Connection)`，由 migrate 主流程呼叫。
- [ ] **Step 4: 驗證**
  - 測試 PASS。
  - `make build && ./bin/travel db migrate` 連跑兩次都成功。
  - 重產 `scripts/schema.sql`，不可手改。
- [ ] **Step 5: 整合者 commit**：`feat(record): record_status/record_note columns, session record_summary_zh`

### Task 2: `travel-db` repo `records.rs`

**Files:**
- Create: `rust/crates/travel-db/src/repo/records.rs`
- Modify: `rust/crates/travel-db/src/repo/mod.rs`（`pub mod records;`）
- Test: `rust/crates/travel-cli/tests/record_repo.rs`（新，真 Turso）

**Interfaces:**
- Produces:

```rust
pub enum RecordStatus { Done, Skipped }
impl RecordStatus { pub fn parse(s: &str) -> Result<Self, String>; pub fn as_str(&self) -> &'static str; }
pub enum RecordTarget { Activity, Meal, Leg }

/// 主日（variant_of IS NULL）範圍內的唯一比對；回傳 activities.id。
/// id 完全相等優先；否則 title 含 needle（大小寫不分、字面比對）。0 筆或多筆 → Err（訊息列出候選 id 與 title）。
pub async fn resolve_activity_unique(conn, plan_id, dest, day, session, needle: &str) -> Result<String, String>;
/// session_meals 的唯一比對；回傳 sort_order。
pub async fn resolve_meal_unique(conn, plan_id, dest, day, session, needle: &str) -> Result<i64, String>;
/// 寫入狀態；note=None 表示不動 note。回傳是否真的改變（同值 → false）。
pub async fn set_activity_record(conn, plan_id, dest, id: &str, st: Option<RecordStatus>, note: Option<Option<&str>>) -> Result<bool, String>;
pub async fn set_meal_record(conn, plan_id, dest, day, session, sort_order: i64, st: Option<RecordStatus>, note: Option<Option<&str>>) -> Result<bool, String>;
pub async fn set_leg_record(conn, plan_id, dest, day, sort_order: i64, st: Option<RecordStatus>, note: Option<Option<&str>>) -> Result<bool, String>;
pub async fn set_session_summary(conn, plan_id, dest, day, session, zh: Option<&str>) -> Result<bool, String>;
/// 範圍內有任何 record_status IS NOT NULL 的列，或任何 timesofday.record_summary_zh 非空的時段，就 Err；訊息列出這些列（表名、day、session、title/meal/from→to／時段摘要）。
/// 時段摘要的涵蓋：ActivitiesOfDay、LegsOfDay 以外的「整日」範圍（delete_day_content、swap、Destination）都要查該日（或全目的地）的 record_summary_zh；MealsOfSession 只查列狀態。
pub enum RecordScope<'a> {
    Activity { id: &'a str },
    ActivitiesOfDay { day: i64 },
    MealsOfSession { day: i64, session: &'a str },
    LegsOfDay { day: i64 },
    LegSlot { day: i64, sort_order: i64 },
    LegsAiRecommendedOfDay { day: i64 },
    Destination,
}
pub async fn ensure_unrecorded(conn, plan_id, dest, scope: RecordScope<'_>) -> Result<(), String>;
```

- [ ] **Step 1: 寫失敗的測試**（真 Turso，`zz_test_<nanos>`，bind plan id 後立刻 arm `Guard`，結束時 `common::teardown_plan`）
  - `unique_match_rejects_ambiguous`：同一時段兩筆標題含「寺」，比對 `寺` 要 Err，訊息含兩個 id；比對 `東寺` 要回 Ok(id)。
  - `variant_rows_not_candidates`：變體日有一筆「東寺」，主日也有一筆，要回主日那筆的 id。
  - `match_is_literal_not_like`：標題 `100%_ok` 用 needle `%` 比對只回這一筆；needle `_` 同理。
  - `set_record_same_value_returns_false`：連設兩次 `done`，第二次回 `false`。
  - `ensure_unrecorded_lists_rows`：標一筆活動 `done` 後，`ActivitiesOfDay` 要 Err 並含該標題；`MealsOfSession` 要 Ok。
- [ ] **Step 2: 執行**（背景）：`cargo test -j 2 -p travel-cli --test record_repo`，預期 FAIL。
- [ ] **Step 3: 實作**
  - 全部用綁定參數。
  - 比對在 Rust 端做 `to_lowercase().contains()`，不用 SQL LIKE。
- [ ] **Step 4: 執行，確認 PASS**。
- [ ] **Step 5: 整合者 commit**：`feat(record): records repo — unique matching, marking, overwrite guard`

### Task 3: CLI `record-*`、`clear-record`

**Files:**
- Create: `rust/crates/travel-cli/src/record.rs`
- Modify: `rust/crates/travel-cli/src/main.rs`：在 `set-day-theme`（`:426`）附近加 match arm；help 文字補上新指令，並維持 `tests/cli_help_parity.rs` 綠燈。
- Test: `rust/crates/travel-cli/tests/record_cli.rs`（新，跑 binary）

**Interfaces:**
- Consumes: T2 全部介面。
- Produces（指令語法照 spec §5，一字不改）：

```
travel record-activity <day> <session> <id|title> done|skipped [--note "<t>"]
travel record-meal     <day> <session> <text-match> done|skipped [--note "<t>"]
travel record-leg      <day> <sort_order> done|skipped [--note "<t>"]
travel record-session  <day> <session> --zh "<t>" | --clear
travel clear-record    activity|meal|leg <day> ... <key>
```

  - `pub(crate) async fn destination_today(conn: &Connection, dest: &str) -> Result<String, String>`：讀 `destination_config.timezone`，呼叫系統 `date`（`TZ=<tz> date +%F`，做法比照 `plan.rs:288-311`）。
    - `TRAVEL_TODAY` 有設定時優先用它，與 `plan_resolver.rs:305` 一致。
    - 時區是 NULL 或空字串時回 Err。
  - 標記活動或路線段時，呼叫 `travel_db::repo::itinerary::touch_day`（`itinerary.rs:227`）。活動另外更新 `activities.updated_at`，讓 `check-maps-fresh` 判定地圖過期。
  - 目的地一律用 `resolve_active_destination`，未知旗標用 `reject_unknown_flags`。

- [ ] **Step 1: 寫失敗的測試**（binary，`TRAVEL_TODAY=<固定日>`）
  - `record_activity_writes_fields_and_triad`：`plans.version` +1；`plan_events` 有一列 `record_marked`；`operation_runs` 有一列。
  - `rerun_same_value_no_bump`：同值重跑，版本不變。
  - `future_day_rejected`：day 的日期大於 `TRAVEL_TODAY` 時，exit 非 0，stderr 含 `future`，資料不變。
  - `variant_row_rejected`。
  - `missing_timezone_fails_loud`：目的地 timezone 是 NULL，而且沒有設 `TRAVEL_TODAY` 時，exit 非 0，stderr 含 `destination_config.timezone`。
  - `clear_record_resets_both`：status 與 note 都清成 NULL，事件類型是 `record_cleared`。
  - `record_marks_maps_stale`：標記後執行 `check-maps-fresh`，判定為過期（斷言方式比照 `tests/maps_fresh.rs`）。
- [ ] **Step 2: 執行**（背景）：`cargo test -j 2 -p travel-cli --test record_cli`，預期 FAIL。
- [ ] **Step 3: 實作 `record.rs`**：每個指令都依這個順序執行：先做完所有讀取驗證，然後 `begin_immediate`、repo 寫入、events、`record_operation`，最後 `finish_tx`。
- [ ] **Step 4: 執行，確認 PASS**；也跑 `--test cli_help_parity`。
- [ ] **Step 5: 整合者 commit**：`feat(record): record-activity/meal/leg/session and clear-record commands`

### Task 4: 防洗掉與防跨日（repo 層守門）

**Files:**
- Modify: `rust/crates/travel-db/src/repo/route_segments.rs:32`（`delete_slot`，用 `LegSlot`）、`:50`（`delete_ai_recommended_for_day`）、`:65`（`delete_all_for_day`）
- Modify: `rust/crates/travel-db/src/repo/itinerary.rs`：
  - `:31` `replace_skeleton`（`Destination`）
  - `:287` `delete_session_meals`、`:311` `replace_session_meals`（`MealsOfSession`）
  - `:445` `delete_activity`（`Activity`）
  - `:482` `move_activity`：只在 `to_day` 和原本的 day 不同時檢查 `Activity`
  - `:1184` `swap_session_day_numbers`：對 `day_a`、`day_b` 各檢查 `ActivitiesOfDay`、`LegsOfDay`，並檢查每個時段的 `MealsOfSession`
- Modify: `rust/crates/travel-db/src/repo/days.rs:450`（`delete_day_content`：檢查該日所有範圍）
- Test: `rust/crates/travel-cli/tests/record_guard.rs`（新）

**Interfaces:**
- Consumes: T2 的 `ensure_unrecorded`、`RecordScope`。
- 對應 spec §5 的 8 條路徑：
  - `set-meals`、`clear-meals`：在 `itinerary.rs` 擋。
  - `set-route-segments-bulk`、`set-route-segment`（`set_route_segment.rs:416-420`）、`derive-routes`（`derive_routes.rs:107-115`）：在 `route_segments.rs` 擋。
  - `delete-activity`：在 `itinerary.rs:445` 擋。
  - `scaffold-itinerary --force`：在 `replace_skeleton` 擋。
  - `remove-day-variant`：在 `delete_day_content` 擋。

- [ ] **Step 1: 寫失敗的測試**
  - 每條路徑一個測試，共 8 個，另加 `summary_only_session_blocks_scaffold_force_and_remove_variant_and_swap`（只寫 `record-session` 摘要、沒有標任何列，三個指令都要被擋）、`move_activity_cross_day_rejected`、`move_activity_same_day_ok`、`swap_days_rejected_when_either_day_recorded`、`copy_plan_carries_records`（`copy-plan` 之後，新 plan 的 `record_status` 相同）。
  - 每個「被拒」測試都要斷言：exit 非 0；stderr 含被擋的列；`plans.version`、目標表的列數、`plan_events` 列數都和執行前一樣。
  - 測試時直接跑 binary 執行該指令，例如 `set-meals <day> <session> ...`。
- [ ] **Step 2: 執行**（背景）：`cargo test -j 2 -p travel-cli --test record_guard`，預期 FAIL。
- [ ] **Step 3: 實作**：每個 repo 函式在第一條 DELETE 或 UPDATE 之前呼叫 `ensure_unrecorded`；錯誤直接往上傳，外層交易會 rollback。
- [ ] **Step 4: 驗證**：`record_guard` PASS。另外在背景跑既有測試：`--test clear_meals --test route_segment_guard --test swap_days --test derive_routes_behavior_lock --test scaffold_itinerary_behavior_lock --test day_variants --test move_and_after --test copy_plan`，全部要綠燈。
- [ ] **Step 5: 整合者 commit**：`feat(record): refuse to delete, rebuild or move recorded rows`

### Task 5: 補齊編輯指令與標記 lint

**Files:**
- Modify: `rust/crates/travel-cli/src/set_tod.rs`：新增 `run_add_meal`、`run_set_meal`，放在 `:418` `run_meals` 旁邊。
- Modify: `rust/crates/travel-cli/src/set_activity.rs`：新增 `run_set_notes`。
- Modify: `rust/crates/travel-db/src/repo/itinerary.rs`：新增 `append_session_meal`、`update_session_meal_text`。`set-activity-notes` 沿用 `update_activity_field`（`:368`），但先在函式開頭加欄位白名單 `match`（仿 `:166` timesofday 的寫法：`start_time`、`end_time`、`is_fixed_time`、`booking_status`、`booking_ref`、`book_by`、`booking_required`、`notes`），其他欄名回 Err；目前它直接把呼叫端給的欄名插進 SQL。
- Modify: `rust/crates/travel-cli/src/main.rs`：3 個 arm。
- Modify: `rust/crates/travel-cli/src/validate.rs`：新增 `fn record_marker_hits(text: &str) -> bool` 與一個 WARN check。
- Test: `rust/crates/travel-cli/tests/record_edit_cmds.rs`（新）、`validate.rs` inline

**Interfaces:**
- Produces（語法照 spec §5）：

```
travel set-activity-notes <day> <session> <id|title> --notes "<t>" | --clear
travel add-meal <day> <session> --meal "<t>" [--recommended]
travel set-meal <day> <session> <text-match> --meal "<t>"
```

  - `set-activity-notes`、`set-meal` 使用 T2 的唯一比對。
  - `add-meal` 的 `sort_order = MAX+1`，必須在交易內讀取。`--recommended` 的 `source` 值沿用 `set-meals` 既有的對應方式（見 `set_tod.rs:591` `parse_meals`）。
  - 三個指令都寫 audit triad。

- [ ] **Step 1: 寫失敗的測試**
  - `add_meal_appends_not_replaces`：原有 2 筆，加 1 筆後共 3 筆，前 2 筆內容不變。
  - `set_activity_notes_keeps_id_and_poi`：`id`、`poi_id` 不變，`notes` 改成新值。
  - inline：`update_activity_field_rejects_unknown_column`。
  - `set_meal_ambiguous_fails`。
  - lint 單元測試：`record_marker_hits("✅ 實際行程：東福寺")`、`("⏭ 未前往")`、`("實際用餐")` 都是 true；`("visited Toji")`、`("skipped")` 都是 false。
- [ ] **Step 2: 執行**：binary 測試在背景跑 `--test record_edit_cmds`；lint 跑 `cargo test -j 2 -p travel-cli validate::`。預期 FAIL。
- [ ] **Step 3: 實作**：lint 掃描範圍包括 `activities.title`、`activities.notes`、`session_meals.meal`、`day_route_segments.notes`、`timesofday.focus`、`timesofday.focus_zh`。
- [ ] **Step 4: 執行，確認 PASS**；`--test cli_help_parity` 也要綠燈。
- [ ] **Step 5: 整合者 commit**：`feat(record): add-meal, set-meal, set-activity-notes; validate warns on in-text markers`

### Task 6: Worker 呈現

**Files:**
- Modify: `workers/trip-dashboard-rs/src/router.rs`：
  - `:866-869` 的 timesofday SELECT 加 `record_summary_zh`
  - `:871-874` 的 activities SELECT 加 `record_status, record_note`
  - `:876-878` 的 session_meals SELECT 加 `record_status, record_note`
  - `:901` 一帶的 day_route_segments SELECT 加 `record_status, record_note`
- Modify: `workers/trip-dashboard-rs/src/model.rs`：
  - `Activity`（`:36`）、`Meal`（`:27`）、`RouteSegment`（`:16`）各加 `record_status: String`、`record_note: String`
  - `Session`（`:45`）加 `record_summary_zh: String`
- Modify: `render/session.rs`（`render_unit`，`:188`）、`render/day.rs:430-460`（用餐總覽）、`render/now.rs:190-217`（`meal_line`）、`render/alerts.rs:197`
- Modify: `i18n.rs`：新增 key `recDone`（實際／Actual）、`recSkippedLine`（未前往：{list}／Skipped: {list}）、`recUnrecorded`（計畫・未紀錄／Planned · not recorded）
- Test: 各檔 inline tests

**Interfaces:**
- Consumes: 天氣計畫 T5 的 `Option<&now::Clock>`、`Session.time_end`。
- Produces:
  - `pub fn session_is_past(day_date: &str, sess: &Session, clock: Option<&now::Clock>) -> bool`，放在 `render/session.rs`。
    - `clock` 是 None 時回 false。
    - day 的日期早於 clock 的日期時回 true。
    - 同一天時比較 `time_end`，空的話以 `DAY_END` 計，`<= clock.minutes` 就是已過。
  - `pub fn split_meals(meals: &[Meal]) -> (Option<&Meal> /*主選*/, Vec<&Meal> /*備選*/, Vec<&Meal> /*skipped*/)`：主選是第一筆非 `skipped`；備選是其餘非 `skipped`（不含主選）；`skipped` 另列。用餐總覽（`render/day.rs:435` 目前用 `meals[1..]`）、now 卡（`render/now.rs:210` 目前用 `meals.get(1)`）、時段內的餐點列表三處都改用它，不再用索引。已過時段的 skipped 餐點和活動一樣收進時段最後的 `recSkippedLine`。
  - 路線段（spec §6 的地圖規則延伸到網頁，避免地圖和網頁說法不一）：`render_route_block`（`render/day.rs:491`）與 now 卡的「接下來」（`render/now.rs` `leg_slot` 一帶）都濾掉 `skipped` 的段；已過時段的 `done` 段附 `recDone`，`record_note` 用 `esc()` 附在後面。
  - `render_unit` 多一個參數 `past: bool`，所有呼叫端都要更新。
    - 已過時，`done` 的活動加上 `recDone` 標章。
    - `skipped` 的活動不在列表中出現，收到時段最後一行 `recSkippedLine`。
    - 狀態是空的活動加上 `recUnrecorded` 標章。
    - `record_note` 用 `esc()` 後附在後面。
    - 時段標頭在已過、而且 `record_summary_zh` 非空時，改用它取代 `focus_zh`。
    - 未到的時段輸出和現在完全相同。

- [ ] **Step 1: 寫失敗的測試**
  - `past_done_renders_actual`、`past_null_renders_unrecorded`、`skipped_collapsed_to_one_grey_line`。
  - `future_session_output_unchanged`：同一個 Session，`past=false` 時的輸出要和改動前的 snapshot 字串相同。
  - `summary_replaces_focus_when_past`。
  - `split_meals_first_skipped`：第一家 skipped → 第二家成主選，備選不含第二家也不含第一家。
  - `split_meals_middle_skipped`：中間一家 skipped → 主選不變，備選不含它。
  - `all_meals_skipped_shows_none`：用餐總覽和 now 卡都不出現主選或備選。
  - `skipped_leg_hidden_in_route_block_and_now_next`、`past_done_leg_has_badge_and_note`。
  - `skipped_activity_no_pending_alert`。
  - `session_is_past_taipei_clock`：`Clock{date:"2026-11-16",minutes:600}`，時段 09:00–09:30 是 true，10:30–12:00 是 false。
- [ ] **Step 2: 執行**：`cd workers/trip-dashboard-rs && cargo test -j 2`，預期 FAIL。
- [ ] **Step 3: 實作**：依 Interfaces；`plan` 分享連結的檢視者和擁有者走同一條渲染路徑，不要另外分支。
- [ ] **Step 4: 驗證**：`cargo test -j 2` PASS，worker build 成功。
- [ ] **Step 5: 整合者 commit**：`feat(worker): show actual / skipped / not-recorded for past sessions`

### Task 7: 地圖濾掉 skipped

**Files:**
- Modify: `rust/crates/travel-cli/src/snapshot_maps.rs:2618`（路線段查詢）、`:2648`（活動 POI 查詢）、`:3177`，在 WHERE 加 `AND (record_status IS NULL OR record_status <> 'skipped')`。`:2648` 用 `a.record_status`。
- Test: `rust/crates/travel-cli/tests/record_maps.rs`（新）或 `snapshot_maps.rs` 既有 inline 查詢測試。

- [ ] **Step 1: 寫失敗的測試** `skipped_leg_and_activity_not_in_map_inputs`
  - seed 兩段路線和兩個有座標的活動，各標一筆 `skipped`。
  - 呼叫 snapshot_maps 讀取輸入的函式（`:2618`／`:2648` 所在的 fn），斷言結果只剩沒被標 `skipped` 的那一筆。
  - 斷言 input hash（`:1383`）和標記之前不同。
- [ ] **Step 2: 執行**（背景），預期 FAIL。
- [ ] **Step 3: 實作**：只改 WHERE。
- [ ] **Step 4: 執行，確認 PASS**；在背景跑 `--test maps_fresh --test validate_publish_map_coverage`，要綠燈。
- [ ] **Step 5: 整合者 commit**：`feat(maps): do not draw skipped stops or legs`

### Task 8: 清掉現有標記、部署、文件（整合者）

依 spec §7、§9 的順序做。

- [ ] **Step 1**：`make build`；`./bin/travel db migrate` 連跑兩次；確認 `scripts/schema.sql` 已重產。
- [ ] **Step 2**：用唯讀 SELECT 盤點 kyoto-nov-2026 Day 4–5 所有含標記的標題、餐點、路線 notes、focus、活動 notes，整理成清單。
- [ ] **Step 3（人工關卡，Yang §11 Q3「列給你確認」）**：把盤點結果，以及沒有標記的列（Day 4 路線段 0–4、Day 5 中午與晚上）一起列給 Yang。Yang 確認後才往下做。
- [ ] **Step 4**：照 Yang 在 Step 3 的回答處理沒有標記的列：
  - Yang 說「標 done」的列 → `record-activity`／`record-meal`／`record-leg ... done`。
  - Yang 說「沒去」的列 → `... skipped`（可附 `--note`）。
  - Yang 說「維持原樣」或沒提到的列 → 不動（`record_status` 保持 NULL，網站顯示「計畫・未紀錄」）。
  - 完成後用唯讀 SELECT 列出 Day 4–5 每一列的 `record_status`，與 Yang 的回答逐列對照，結果附在回報裡。
  接著照 spec §7 的對照表，把含文字標記的列用 `record-activity`、`record-meal`、`record-leg`、`record-session` 逐列標記：
  - `✅`、已參觀、實際行程、實際用餐 → `done`
  - `⏭`、未前往 → `skipped`
  - 說明文字放進 `--note`
  - `TRAVEL_TODAY` 不需要設，因為 Day 4–5 都是過去的日子。
- [ ] **Step 5**：文字清理用一次性的 `db exec`，一句 UPDATE 只改一列，並檢查 affected rows。
  - 標題恢復成地點名稱，`poi_id`（`tofukuji`、`toji`）保留。
  - focus 裡描述實況的句子移到 `record_summary_zh`，focus 只留一句中性描述。
- [ ] **Step 6**：`./bin/travel validate data` 的標記 lint WARN 要是 0；`./bin/travel validate publish` 要綠燈。
- [ ] **Step 7**：部署 worker（先 `unset CLOUDFLARE_API_TOKEN`，在 migrate 之後）→ `./bin/travel snapshot-maps --plan-id kyoto-nov-2026` → `./bin/travel verify-site --plan-id kyoto-nov-2026 --at 2026-11-16T20:00`，要全部 PASS。
- [ ] **Step 8**：更新文件：
  - `docs/reference/CLI.md`：新增 8 個指令。
  - `docs/reference/dashboard.md`：紀錄的呈現方式。
  - `docs/reference/routing.md`：「旅途中改變行程：改時間用既有指令，標結果用 `record-*`，不改標題、不刪除重建」。
- [ ] **Step 9: 整合者 commit**：`docs(record): trip-record commands and in-trip routing`


## 審查核對紀錄（2026-11-16 計畫審查）

審查者：Codex（block，整體；本計畫只有 1 項 minor）、K3.1 Pro（block，整體）。

| 發現（審查者） | 處理 | 理由 |
|---|---|---|
| `ensure_unrecorded` 不看時段摘要，只寫摘要的時段仍會被 scaffold／remove-day-variant／swap 洗掉（K3） | 採納 | 整日與全目的地範圍也查 `record_summary_zh`；加 summary-only 測試 |
| 只處理主選，備選仍用 `meals[1..]`／`get(1)`，skipped 店會留在備選（K3） | 採納 | 已核 `render/day.rs:435`、`render/now.rs:210`。改用 `split_meals` 三處共用，補第一家／中間一家／全部 skipped 測試 |
| 路線的 `record_status/note` 有讀進 model 卻沒人呈現，skipped 段仍在網頁（K3） | 採納 | 網頁濾掉 skipped 段、done 加標章，和地圖規則一致；否則 `record-leg` 在網站上沒有作用 |
| `update_activity_field` 沒有欄位白名單、欄名由呼叫端插入（Codex minor） | 採納 | 已核 `itinerary.rs:368-383`；加 `match` 白名單（含 `notes`）與測試 |
| 人工關卡後「標 done／維持 NULL」各自怎麼做沒寫（K3 minor） | 採納 | T8 Step 4 補三個分支與完成後逐列對照 |
