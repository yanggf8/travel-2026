# 天氣抓取強化＋ now 卡時區 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**可行性檢查（Yang 規則，2026-11-16）：** 範圍已縮成 Yang 核可的 Phase 1，不做 MET 備援。兩件事都是旅途中真的碰到的問題：
- 「現在」卡片在台灣國內行程會差一小時，所以要改用目的地時區。
- Open-Meteo 卡住時要保住舊資料、說清楚錯在哪，並且不能寫入假的「0°C 晴」。

行程紀錄判斷「已經過去」也要用這個時鐘，所以這份計畫要比行程紀錄先上線。

**Goal:** now 卡改用 `destination_config.timezone`。`fetch-weather` 失敗時一列都不動，錯誤分得出 HTTP 狀態碼，缺欄位不寫假值；每個目的地一筆交易，`--all` 有失敗就非零退出。

**Architecture:**
- **Worker**：
  - `router.rs` 已經查目的地時區（[29]），改把它存進 `plan.timezone`，由 `tzfmt.rs` 透過 `Intl` 算出當地 `Clock`。
  - 沒有時區時 `clock = None`，這條路徑已經存在，會讓日卡全開、沒有 now 卡。
  - inline script 改用 `Intl` 加 `data-now-tz`。
- **CLI**：
  - `weather.rs` 拆出三個純函式：curl 結果分類、預報解析、`--all` 彙總。
  - 寫入改成每個目的地一個 `begin_immediate`／`finish_tx` 交易，`record_operation` 也包在交易內。

**Tech Stack:** Rust（travel-cli edition 2024，libsql 0.6）、workers-rs 0.8（wasm、`js_sys::Intl`）、Turso。

**Spec:** `docs/superpowers/specs/2026-11-16-weather-provider-fallback-design.md`（§1 目標、§6 設計、§7 測試、§8 部署、§10 Yang 的回答）

## Global Constraints

- 時區不准退回寫死的 JST；`+324e5`、`JST_OFFSET_MS`、`jst_from_ms` 全部刪除。
- 失敗時不准寫入部分結果；不准讀本地檔；不准用預設值填缺欄位（no default data）。
- 不加 JSON 欄位、CHECK 約束、feature flag；migrator 只加欄，不塞資料。
- 時段窗（9–12／12–14／14–18／18–22）與聚合規則不動。
- 介面文字放 `workers/trip-dashboard-rs/src/i18n.rs`（ZH／EN 成對）。
- 交易一律用 `cascade::common::begin_immediate`／`finish_tx`（`rust/crates/travel-cli/src/cascade/common.rs:298-329`）。目的地解析一律用 `cascade::common::resolve_active_destination`（`:33`）。未知旗標用 `plan_resolver::reject_unknown_flags`（`plan_resolver.rs:741`）。
- 程式只有 inline script 一段；SSR 優先，JS 關閉也能完整閱讀。
- Worker 部署：`cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`，部署前一定要 build。
- 派出去的 AI 不跑 git、不派 subagent，先讀 `~/c/agent-portal-tpm/dispatch/rules/agent-rules.md`；commit 由整合者（Opus）做。

## Review Focus

1. **午夜跨日**：`Intl` 在部分引擎回 `hour="24"`。在 00:00–00:59，日期和分鐘都不能算錯（Task 5、Task 6 有測試）。
2. **錯誤的 IANA 名稱**：時區是 `Asia/Tokio`、空字串或 NULL 時，頁面不能 500，也不能退回 JST。要不出 now 卡、日卡全開（Task 5 測試）。
3. **Open-Meteo 回 HTTP 200 但 body 是空的、或不是 JSON**：要歸類成失敗並保留舊資料，不能寫入（Task 1 測試 `classify_200_empty_body_is_fail`）。
4. **所有日子都缺欄位**：寫入天數為 0 時不 bump `plans.version`，也不寫 `operation_runs`（Task 3 測試）。
5. **`--all` 第二個目的地失敗**：第一個目的地的寫入與 audit 已經完整 commit，最後要非零退出，stderr 列出失敗的目的地（Task 3 測試 `all_summary_nonzero_when_any_failed`）。

---

## 檔案地圖

| 檔案 | 責任 | 任務 |
|---|---|---|
| `rust/crates/travel-cli/src/weather.rs` | curl 分類、解析、交易寫入、`--all` 彙總、旗標 | T1, T2, T3 |
| `rust/crates/travel-cli/src/validate.rs` | 時區為空 WARN | T4 |
| `rust/crates/travel-cli/src/db_migrate.rs`、`weather_providers.rs`、`travel-db/src/repo/weather_providers.rs` | `license_url` 欄與 `--license-url` | T4 |
| `workers/trip-dashboard-rs/src/tzfmt.rs` | `clock_from_parts`、`now_clock_in_tz` | T5 |
| `workers/trip-dashboard-rs/src/render/now.rs`、`router.rs`、`model.rs` | Clock 改當地時區、`plan.timezone` | T5 |
| `workers/trip-dashboard-rs/src/render/mod.rs` | inline script | T6 |
| `workers/trip-dashboard-rs/src/render/day.rs`、`i18n.rs`、`model.rs`、`router.rs` [28] | 出處行「經彙整」與授權連結 | T7 |
| `verify_site.rs`、`verify_site_page.rs`、`docs/reference/CLI.md`、`dashboard.md` | JST 字樣、部署 | T8 |

## 派工與並行

- **A 組（CLI，後端）**：T1 → T2 → T3 必須依序做，因為都改 `weather.rs`。T4 改的是別的檔案，可以和 A 組並行。
  - 實作：Codex（額度不夠改 agy）。
  - 審查：Codex 寫的交 agy → K3.1 Pro；agy 寫的交 Codex → K3.1 Pro。
- **B 組（worker，前端）**：T5 → T6 依序做（T6 依賴 T5 的 `data-now-tz` 輸出）。T7 會碰 `router.rs`，要等 T5 合併後再做。
  - 實作：agy（或 Codex）。
  - 審查：agy → Codex → K3.1 Pro，依「審的不能是寫的」原則。
- A 組和 B 組不共用檔案，可以由兩家 AI 同時做。
- T8 由整合者在 A、B 兩組都合併後做。
- 跨計畫順序：本計畫的 T5、T6 上線之後，才能部署行程紀錄的 worker 任務。旅行筆記與本計畫互不相依。

---

### Task 1: curl 結果分類（HTTP 狀態碼、重試策略）

**Files:**
- Modify: `rust/crates/travel-cli/src/weather.rs:443-492`（`open_meteo_curl_args`、`is_transient_curl_exit`、`open_meteo_failure_message`）與 `:515-534`（重試迴圈）
- Test: 同檔 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `fn open_meteo_curl_args(url: &str) -> Vec<String>`：參數為 `["-sS", "-w", "\n%{http_code}", "--connect-timeout", "10", "--max-time", "20", url]`，不再用 `-f`。
  - `fn split_http_status(stdout: &[u8]) -> (Option<u16>, &[u8])`：取最後一行當狀態碼，回傳 `(狀態碼, body)`。
  - `enum AttemptClass { Ok, Retry, Fail }`
  - `fn classify_attempt(curl_exit: Option<i32>, http: Option<u16>, body: &[u8]) -> AttemptClass`
  - `fn open_meteo_failure_message(curl_exit: Option<i32>, http: Option<u16>, attempts: u32) -> String`

- [ ] **Step 1: 寫失敗的測試**

```rust
#[test] fn curl_args_no_fail_flag_and_write_out_status() {
    let a = open_meteo_curl_args("https://x");
    assert!(!a.iter().any(|s| s == "-sf" || s == "-f"));
    assert!(a.windows(2).any(|w| w[0] == "-w" && w[1] == "\n%{http_code}"));
    assert_eq!(a.last().unwrap(), "https://x");
}
#[test] fn split_status_parses_trailing_code() {
    assert_eq!(split_http_status(b"{\"a\":1}\n200"), (Some(200), &b"{\"a\":1}"[..]));
    assert_eq!(split_http_status(b"garbage"), (None, &b"garbage"[..]));
}
#[test] fn classify_matrix() {
    use AttemptClass::*;
    assert_eq!(classify_attempt(Some(0), Some(200), b"{}"), Ok);
    assert_eq!(classify_attempt(Some(0), Some(429), b""), Retry);
    assert_eq!(classify_attempt(Some(0), Some(503), b""), Retry);
    assert_eq!(classify_attempt(Some(28), None, b""), Retry);
    assert_eq!(classify_attempt(Some(35), None, b""), Retry);
    assert_eq!(classify_attempt(Some(7), None, b""), Retry);
    assert_eq!(classify_attempt(Some(0), Some(400), b""), Fail);
    assert_eq!(classify_attempt(Some(0), Some(403), b""), Fail);
    assert_eq!(classify_attempt(Some(0), None, b""), Fail);
}
#[test] fn classify_200_empty_body_is_fail() {
    assert_eq!(classify_attempt(Some(0), Some(200), b""), AttemptClass::Fail);
}
#[test] fn failure_message_names_http_code() {
    let m = open_meteo_failure_message(Some(0), Some(403), 1);
    assert!(m.contains("request error HTTP 403"), "{m}");
    assert!(m.contains("Nothing was written"), "{m}");
}
```

- [ ] **Step 2: 執行測試，確認失敗**
  - 指令：`cd rust && cargo test -j 2 -p travel-cli weather::tests -- --nocapture`
  - 預期：FAIL（`split_http_status`／`classify_attempt` 未定義）。
- [ ] **Step 3: 實作**
  - 重試迴圈改成：每次 attempt 取 `(code, http, body)`，依 `classify_attempt` 決定是 break、`sleep(OPEN_METEO_RETRY_DELAY_SECS)` 再試，還是回錯誤。
  - `OPEN_METEO_MAX_ATTEMPTS` 維持 3。
  - 非 429 的 4xx 不重試。
  - 刪除 `is_transient_curl_exit` 和它的 exit 22 分支，原本的 exit 22 測試改寫成 HTTP 狀態碼的版本。
- [ ] **Step 4: 執行測試，確認通過**
  - 指令同 Step 2。
  - 預期：PASS。原本的 `curl_args_have_timeouts_no_curl_retry_and_url_last`（`:789`）要改名或合併，不可直接刪掉它斷言的 timeout 參數。
- [ ] **Step 5: 整合者 commit**：`fix(fetch-weather): tell HTTP status codes apart, no retry on 4xx`

### Task 2: 缺欄位的日子不寫入

**Files:**
- Modify: `rust/crates/travel-cli/src/weather.rs:536-575`：解析迴圈抽成純函式，刪掉 `:565-569` 的 `unwrap_or(0)`／`unwrap_or(0.0)`。
- Test: 同檔 tests

**Interfaces:**
- Produces:
  - `fn parse_forecasts(data: &serde_json::Value) -> Result<(Vec<Forecast>, Vec<String>), String>`：第二個值是要印到 stderr 的訊息，格式為 `"<YYYY-MM-DD>: Open-Meteo returned no <field> — kept previous data"`。
  - `<field>` 只會是 `temperature_2m_max`、`temperature_2m_min`、`weather_code`。缺 `daily`／`time` 時，照舊回 `Err`。

- [ ] **Step 1: 寫失敗的測試**

```rust
#[test] fn day_missing_tmax_is_skipped_with_message() {
    let data = serde_json::json!({"daily":{"time":["2026-11-20","2026-11-21"],
        "temperature_2m_max":[null,15.0],"temperature_2m_min":[8.0,9.0],"weather_code":[3,61],
        "apparent_temperature_max":[1.0,1.0],"apparent_temperature_min":[1.0,1.0],
        "precipitation_probability_max":[10,80]},"hourly":{"time":[]}});
    let (f, msgs) = parse_forecasts(&data).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].date, "2026-11-21");
    assert_eq!(msgs, vec!["2026-11-20: Open-Meteo returned no temperature_2m_max — kept previous data".to_string()]);
}
#[test] fn day_missing_weather_code_is_skipped() { /* weather_code: [null,61] → 只剩 11-21，訊息含 weather_code */ }
#[test] fn optional_fields_stay_none() { /* apparent_* 為 null → feels_like_*_c == None，日子照樣保留 */ }
```

- [ ] **Step 2: 執行測試，確認失敗**
  - 指令：`cargo test -j 2 -p travel-cli weather::tests::day_missing`
  - 預期：FAIL。
- [ ] **Step 3: 實作**
  - `fetch_weather` 改成呼叫 `parse_forecasts`，訊息逐行 `eprintln!`。
  - 既有的「前一小時」降雨測試（`weather.rs:889` 一帶）不改。
- [ ] **Step 4: 執行 `cargo test -j 2 -p travel-cli weather::`**，預期全部 PASS。
- [ ] **Step 5: 整合者 commit**：`fix(fetch-weather): skip days with missing temperature/code instead of writing 0`

### Task 3: 每個目的地一筆交易；零寫入不 bump；`--all` 非零；resolver 與旗標

**Files:**
- Modify: `rust/crates/travel-cli/src/weather.rs`
  - `:200-303` `run`
  - `:305-326` `update_destination_weather`
  - `:347-368` 刪除 `read_active_destination`
  - `:582-656` `write_day_weather`／`finalize`
- Test: 同檔 tests（in-memory libsql，比照 `db_migrate.rs:2846` 的 `mem_conn`）——驗交易邊界的快速單元測試
- Test: `rust/crates/travel-cli/tests/fetch_weather_all.rs`（新，真 Turso，跑 binary）——驗實際指令行為

**Interfaces:**
- Consumes: T2 的 `parse_forecasts`。
- Produces:
  - `async fn write_destination_weather(conn: &Connection, plan_id: &str, dest: &str, assigned: &[(i64, &Forecast)], sourced_at: &str) -> Result<usize, String>`
    - 內容：`assigned` 為空時直接回 `Ok(0)`，不開交易。否則 `begin_immediate(conn, "fetch-weather")` → 逐日呼叫 `write_day_weather` → `finalize`（`record_operation`，summary 用 `dest`）→ `finish_tx`。
  - `fn all_destinations_exit(results: &[(String, Result<usize, String>)]) -> Result<(), String>`：有任何 `Err` 就回 `Err`，內容是彙總訊息；呼叫端用 `std::process::exit(1)`。
  - `fn kept_data_note(last_sourced_at: Option<&str>) -> String`：產生 `"Previous forecast kept (updated <ts>); retry in a few minutes."`；沒有舊資料時是 `"No previous forecast for this destination; retry in a few minutes."`。

- [ ] **Step 1: 寫失敗的測試**
  - 測試 DB：`test_db()` 用 `conn.execute_batch(include_str!("../../../../scripts/schema.sql"))` 建表，再 seed 這些資料：
    - `plans`：`version=5`
    - `days`：day 1、2 有舊值 `weather_label='old'`
    - `day_weather_sessions`：day 1 有舊列

```rust
#[tokio::test] async fn write_is_atomic_per_destination() {
    let conn = test_db().await;          // seed plan p / dest d, version 5, old weather rows
    let good = forecast("2026-11-20", "morning");
    let bad  = forecast("2026-11-21", "bogus");   // replace_day rejects unknown session key
    let err = write_destination_weather(&conn, "p", "d", &[(1, &good), (2, &bad)], "2026-11-16T00:00:00Z").await;
    assert!(err.is_err());
    assert_eq!(scalar(&conn, "SELECT weather_label FROM days WHERE day_number=1").await, "old");
    assert_eq!(scalar(&conn, "SELECT count(*) FROM day_weather_sessions WHERE day_number=1").await, "1");
    assert_eq!(scalar(&conn, "SELECT version FROM plans WHERE plan_id='p'").await, "5");
    assert_eq!(scalar(&conn, "SELECT count(*) FROM plan_events WHERE plan_id='p'").await, "0");
    assert_eq!(scalar(&conn, "SELECT count(*) FROM operation_runs WHERE plan_id='p'").await, "0");
}
#[tokio::test] async fn zero_days_does_not_bump() {
    let conn = test_db().await;
    assert_eq!(write_destination_weather(&conn, "p", "d", &[], "t").await.unwrap(), 0);
    assert_eq!(scalar(&conn, "SELECT version FROM plans WHERE plan_id='p'").await, "5");
}
#[tokio::test] async fn success_writes_one_operation_run() { /* 兩天正常 → Ok(2)、version 6、operation_runs 1 列 */ }
#[test] fn all_summary_nonzero_when_any_failed() {
    let r = vec![("a".into(), Ok(3)), ("b".into(), Err("HTTP 503".into()))];
    let e = all_destinations_exit(&r).unwrap_err();
    assert!(e.contains("b") && e.contains("HTTP 503"), "{e}");
    assert!(all_destinations_exit(&[("a".into(), Ok(0))]).is_ok());
}
#[test] fn kept_note_mentions_previous_stamp() {
    assert!(kept_data_note(Some("2026-11-15T09:00:00Z")).contains("2026-11-15T09:00:00Z"));
}
```

- [ ] **Step 1b: 寫失敗的真 Turso 整合測試** `fetch_weather_all.rs::all_partial_failure_commits_good_dest_and_exits_nonzero`
  - seed：`zz_test_<nanos>` plan，兩個 `zz_test_<nanos>_a`／`_b` 目的地（`destination_config` 全域列，在 `teardown_plan` 之前本地刪除）；綁定 plan-id 後立刻 arm `Guard`。
  - `_a` 用真實座標（京都）；`_b` 用 Open-Meteo 會回 HTTP 400 的座標（緯度 `95`）——走真正的失敗路徑，不加任何測試專用旗標或環境變數。如果 seed 或抓取前的驗證就擋下這個座標，回報整合者另找失敗方式，不准自行加旗標。
  - 跑 `./bin/travel fetch-weather --plan-id <p> --all`。
  - 斷言：exit 非 0；stderr 含 `_b` 與 `HTTP 400`；`_a` 的 `days.weather_*` 已寫入；`_b` 的天氣列和執行前相同；`plans.version` 恰好 +1；`operation_runs` 恰好 1 列、`summary` 是 `_a`。
  - 如果 `_a` 本身因網路類錯誤（逾時、TLS）失敗，測試印出原因後 skip，不算 FAIL（Open-Meteo 卡住正是這份計畫要處理的狀況，不能讓它把整套測試弄紅）。
- [ ] **Step 2: 執行測試，確認失敗**
  - 整合測試在背景跑：`cargo test -j 2 -p travel-cli --test fetch_weather_all`
  - 指令：`cargo test -j 2 -p travel-cli weather::tests::write_is_atomic`
  - 預期：FAIL（函式未定義）。如果 `schema.sql` 在 in-memory SQLite 載入失敗，`test_db` 改成只執行 `schema.sql` 裡這六張表的 `CREATE TABLE` 段：`plans`、`days`、`day_weather_sessions`、`plan_events`、`plan_event_data`、`operation_runs`。不可手寫欄位。
- [ ] **Step 3: 實作**
  - **單一目的地**：`run` 改為 `reject_unknown_flags(args, &["--dest"], &["--all"])`，目的地用 `resolve_active_destination(&conn, &plan_id, dest_opt.as_deref())`。
  - **`--all`**：每個目的地各自呼叫 `write_destination_weather`，結果收進 `Vec`，最後交給 `all_destinations_exit`。
  - **抓取失敗**：印出 `kept_data_note`（讀 `MAX(days.weather_sourced_at)`，限定該目的地）。
  - 刪除 `read_active_destination`。
- [ ] **Step 4: 執行 `cargo test -j 2 -p travel-cli weather::`**，預期 PASS。
  - 接著在背景跑 `cargo test -j 2 -p travel-cli --test fetch_weather_all --test reject_flags_bookings_weather --test dest_override_validated`，預期 PASS；如果斷言的是舊訊息，就依新行為更新。
- [ ] **Step 5: 整合者 commit**：`fix(fetch-weather): one transaction per destination, non-zero exit when --all has a failure`

### Task 4: `validate data` 時區 WARN；`weather_providers.license_url`

**Files:**
- Modify: `rust/crates/travel-cli/src/validate.rs`：新增一個 check，結構仿照 `:1538` 的 clothing-tip WARN。
- Modify: `rust/crates/travel-cli/src/db_migrate.rs`：在 `weather_providers` 建表（`:1606`）之後呼叫 `add_column(conn, "ALTER TABLE weather_providers ADD COLUMN license_url TEXT")`。
- Modify: `rust/crates/travel-cli/src/weather_providers.rs:10`：`VALUE_FLAGS` 加 `--license-url`（選填，`http(s)://` 驗證比照 `--url`），`print_all` 多印一欄。
- Modify: `rust/crates/travel-db/src/repo/weather_providers.rs`：struct 加 `license_url: Option<String>`，upsert 與 list 跟著改。
- Test: `rust/crates/travel-cli/tests/set_weather_provider.rs`（既有，真 Turso）；`weather_providers.rs` inline 的 `parses_full_args`。

**Interfaces:**
- Produces:
  - `weather_providers.license_url TEXT`，可為 NULL。
  - `travel set-weather-provider ... [--license-url <https://..>]`。

- [ ] **Step 1: 寫失敗的測試**
  - inline：`parses_license_url_optional`，帶 `--license-url https://creativecommons.org/licenses/by/4.0/` 時，`license_url == Some(..)`；不帶時是 `None`；`--license-url ftp://x` 要回 Err。
  - 整合測試 `set_weather_provider.rs` 新增 `license_url_roundtrip`：
    - 用 `zz_test_<nanos>` provider_id，設好 URL 後 SELECT 確認。
    - 用 Guard 刪掉這一列（這是全域列，不屬 `teardown_plan` 範圍）。
  - validate：inline 純函式 `fn empty_timezone_warnings(rows: &[(String, Option<String>)]) -> Vec<String>`：輸入 `("kyoto_2026", None)` 與 `("x", Some(""))`，要產生兩行，每行都含 slug 與 `destination_config.timezone`。
- [ ] **Step 2: 執行，確認失敗**
  - inline：`cargo test -j 2 -p travel-cli weather_providers::`，再跑一次 `cargo test -j 2 -p travel-cli validate::`（cargo 的 TESTNAME 只收一個 filter）
  - 整合測試在背景跑：`cargo test -j 2 -p travel-cli --test set_weather_provider`
- [ ] **Step 3: 實作**：`validate data` 只查 `SELECT slug, timezone FROM destination_config`，交給 `empty_timezone_warnings`，結果以 WARN 印出。
- [ ] **Step 4: 執行，確認通過**
  - 同 Step 2 的指令。
  - 另外 `make build && ./bin/travel db migrate` 連跑兩次，第二次不能報錯。
  - 重產 `scripts/schema.sql`，方法與 `db migrate` 文件所述相同，不可手改。
- [ ] **Step 5: 整合者 commit**：`feat(weather): license_url on weather_providers; validate warns on empty destination timezone`

### Task 5: Worker 時鐘改目的地時區

**Files:**
- Modify: `workers/trip-dashboard-rs/src/tzfmt.rs`：新增兩個函式，wasm 與 host 各一版，比照 `format_in_tz` 在 `:58`／`:95` 的雙版本寫法。
- Modify: `workers/trip-dashboard-rs/src/render/now.rs:29-50`：刪掉 `JST_OFFSET_MS`、`jst_from_ms`，改寫 `Clock` 的文件註解。
- Modify: `workers/trip-dashboard-rs/src/render/now.rs:64`：`parse_at` 的註解改成「目的地牆鐘時間」，邏輯不變。
- Modify: `workers/trip-dashboard-rs/src/render/now.rs:323-325`：now 卡加 `data-now-tz`。
- Modify: `workers/trip-dashboard-rs/src/model.rs:358`：`Plan` 加 `pub timezone: String`；`:48` 的 JST 註解也改掉。
- Modify: `workers/trip-dashboard-rs/src/router.rs:324-330`：決定 clock 的順序；`:1056-1068` 把 tz 存進 `plan.timezone`。
- Test: `tzfmt.rs`、`now.rs`、`render/mod.rs` 的 inline tests。

**Interfaces:**
- Produces:
  - `pub fn clock_from_parts(parts: &[(String, String)]) -> Option<(String, i64)>`：回傳 `(YYYY-MM-DD, 從午夜起算的分鐘數)`，`hour "24"` 視為 0。
  - `pub fn now_clock_in_tz(ms: f64, tz: &str) -> Option<now::Clock>`：wasm 版用 `Intl.DateTimeFormat('en-CA', {timeZone, hourCycle:'h23', year:'numeric', month:'2-digit', day:'2-digit', hour:'2-digit', minute:'2-digit'})`，與 `format_in_tz` 共用建立 formatter 的 helper。host 版回 `None`。tz 為空或被 Intl 拒絕時回 `None`。
  - `pub fn decide_clock(at: Option<&str>, live: Option<now::Clock>) -> Option<now::Clock>`：放在 `now.rs`，純函式。`at` 能 parse 就用它，否則用 `live`。
  - router 呼叫 `decide_clock(query.get("at"), tzfmt::now_clock_in_tz(Date::now(), &plan.timezone))`。結果是 `None` 時，`console_error!("no clock: destination timezone '{tz}' for {slug}")`。
  - now 卡輸出 `data-now-tz="<IANA>"`。

- [ ] **Step 1: 寫失敗的測試**

```rust
// tzfmt.rs
#[test] fn clock_parts_taipei_and_tokyo() {
    let p = |y,m,d,h,mi| vec![("year".into(),y),("month".into(),m),("day".into(),d),("hour".into(),h),("minute".into(),mi)]
        .into_iter().map(|(a,b):(String,&str)|(a,b.to_string())).collect::<Vec<_>>();
    assert_eq!(clock_from_parts(&p("2026","11","16","14","05")), Some(("2026-11-16".into(), 845)));
    assert_eq!(clock_from_parts(&p("2026","11","17","24","10")), Some(("2026-11-17".into(), 10)));
    assert_eq!(clock_from_parts(&p("2026","11","17","xx","10")), None);
}
#[test] fn host_now_clock_is_none() { assert!(now_clock_in_tz(0.0, "Asia/Taipei").is_none()); }
// now.rs
#[test] fn pinned_at_wins_without_timezone() {
    let c = decide_clock(Some("2026-11-16T07:00"), None).unwrap();
    assert!(c.pinned); assert_eq!(c.minutes, 420);
}
#[test] fn no_tz_no_pin_means_no_clock() { assert!(decide_clock(None, None).is_none()); }
#[test] fn bad_at_falls_to_live() { assert!(decide_clock(Some("nope"), None).is_none()); }
#[test] fn now_card_carries_tz_attr() { /* Plan{timezone:"Asia/Taipei",..} 加 pinned clock → 輸出含 data-now-tz="Asia/Taipei" */ }
// render/mod.rs
#[test] fn no_clock_page_has_no_now_card_and_all_days_open() { /* render_plan_at(.., None) → 不含 id="sec-now"，每個日卡 <details ... open> */ }
```

- [ ] **Step 2: 執行測試，確認失敗**
  - 指令：`cd workers/trip-dashboard-rs && cargo test -j 2 -- tzfmt:: render::now:: render::tests::no_clock`（多個 filter 要放在 `--` 之後交給 libtest）
  - 預期：FAIL。
- [ ] **Step 3: 實作**
  - 依 Interfaces 實作。
  - 既有用 `Plan::default()` 又需要時鐘的測試（`now.rs:649-675`、`render/mod.rs:775`、`:1218` 一帶）補上 `timezone: "Asia/Tokyo".into()`。
  - 找不到任何 `jst_from_ms` 的呼叫後再刪除：`grep -rn jst_from_ms src` 要沒有結果。
- [ ] **Step 4: 驗證**
  - `cargo test -j 2` 全部 PASS。
  - `grep -rn "JST_OFFSET_MS\|jst_from_ms" src` 沒有結果。
  - `worker-build --release`（或專案既有的 build 指令）成功。
- [ ] **Step 5: 整合者 commit**：`fix(worker): now card uses the destination timezone, no JST fallback`

### Task 6: inline script 改用 `Intl`

**Files:**
- Modify: `workers/trip-dashboard-rs/src/render/mod.rs:368-371`（`remark` 函式）。
- Test: `render/mod.rs` 既有的 script 字串斷言（`:1183` `block_memory_script_is_emitted_once_on_viewer_and_owner_pages` 一帶）。

**Interfaces:**
- Consumes: T5 的 `data-now-tz` 屬性。
- Produces: script 從 `nc.getAttribute('data-now-tz')` 讀時區，算出當地日期 `ymd` 與分鐘 `m`，其他邏輯不動。

- [ ] **Step 1: 寫失敗的測試**：把 script 斷言改成下面這樣。

```rust
let s = render_plan_at(&plan, "zh", None, &status, "", Some(&clock)); // 同 render/mod.rs:1183 測試的取法
assert!(!s.contains("324e5"));
assert!(s.contains("data-now-tz"));
assert!(s.contains("hourCycle:'h23'"));
assert!(s.contains("%24"));          // hour 正規化（minified，無空白）
assert!(s.contains("try{") && s.contains("catch(e){return;}"));
assert_eq!(s.matches("<script").count(), 1);   // 仍只有一段 inline script
```

- [ ] **Step 2: 執行測試**：`cargo test -j 2 render::tests`，預期 FAIL。
- [ ] **Step 3: 實作 `remark`**
  - 用 `new Intl.DateTimeFormat('en-CA',{timeZone:tz,hourCycle:'h23',year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}).formatToParts(new Date())`，從 parts 組出 `ymd`；`m=(+hour%24)*60+(+minute)`。
  - `ymd!==data-now-date` 時 `m=1e9`。
  - 整段包在 `try{…}catch(e){return;}` 裡。
- [ ] **Step 4: 驗證**：`cargo test -j 2` PASS，build 成功。
- [ ] **Step 5: 整合者 commit**：`fix(worker): per-minute re-mark reads the destination timezone`

### Task 7: 出處行「經彙整」與授權連結

**Files:**
- Modify: `workers/trip-dashboard-rs/src/router.rs:1042`（[28] SELECT 加 `license_url`）。
- Modify: `workers/trip-dashboard-rs/src/model.rs:76,102`（`license_url: String`）。
- Modify: `workers/trip-dashboard-rs/src/render/day.rs:225-250`（出處行，`wxLicense` 在 `:243`）。
- Modify: `workers/trip-dashboard-rs/src/i18n.rs`：新增 key `wxAggregated`，ZH「天氣資料（經時段彙整）：」、EN「Weather data (aggregated):」。
- Test: `render/day.rs:1344` 一帶的既有測試。

**Interfaces:**
- Consumes: T4 的 `weather_providers.license_url`。

- [ ] **Step 1: 寫失敗的測試**
  - `provider_line_links_license_when_url_present`：`license_url="https://creativecommons.org/licenses/by/4.0/"` 時，輸出含 `<a href="https://creativecommons.org/licenses/by/4.0/"`、`CC BY 4.0`、`經時段彙整`。
  - `provider_line_plain_license_when_url_empty`：沒有 `license_url` 時，`CC BY 4.0` 不是連結。
- [ ] **Step 2: 執行測試**：`cargo test -j 2 render::day::`，預期 FAIL。
- [ ] **Step 3: 實作**：前綴改用 `t("wxAggregated", lang)`；`href` 經 `esc()`，而且只接受 `http(s)://` 開頭。
- [ ] **Step 4: 驗證**：`cargo test -j 2` PASS，build 成功。
  - 部署必須在 T4 的 migrate 之後：worker 用明確欄位清單查詢，欄位不存在時整頁會失敗。
- [ ] **Step 5: 整合者 commit**：`feat(weather): credit says aggregated, license links to its terms`

### Task 8: JST 字樣、部署、線上驗證（整合者）

**Files:**
- Modify: `rust/crates/travel-cli/src/verify_site.rs:69,756,813`、`verify_site_page.rs:565`：「JST」改成「destination local time」。
- Modify: `docs/reference/CLI.md:257`、`docs/reference/dashboard.md:30,31,47`。

- [ ] **Step 1**：`grep -rn "JST" rust/crates/travel-cli/src/verify_site*.rs docs/reference/CLI.md docs/reference/dashboard.md`，逐處改寫。只留下指 Kyoto 實際行程時間的字樣。
- [ ] **Step 2**：`make build && ./bin/travel db migrate`（已在 T4 跑過的話，這次應該冪等）。
- [ ] **Step 3**：`./bin/travel set-weather-provider open_meteo --name-zh <現值> --name-en <現值> --url <現值> --license <現值> --license-url https://creativecommons.org/licenses/by/4.0/`。現值先用 `./bin/travel weather-providers` 讀出來。
- [ ] **Step 4**：build worker 後部署：`cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`。
- [ ] **Step 5**：`./bin/travel verify-site --plan-id kyoto-nov-2026 --at "$(TZ=Asia/Tokyo date '+%Y-%m-%dT%H:%M')"`，預期全部 PASS（基準 195）。
  - **時區線上驗證**（`now_clock_in_tz` 的 host 版回 `None` 是因為 `Intl` 只在 wasm 裡有；換算邏輯由 `clock_from_parts` 純函式測試涵蓋，線上分兩段補）：
    - 釘選：找一個台北時區的 plan，用它行程日期內的 `?at=` 抓頁面，確認有 now 卡、`data-now-tz="Asia/Taipei"`、標示的時段正確。
    - 未釘選：同一個 plan 不帶 `?at=`；在行程期間外時，確認頁面沒有 now 卡、日卡全開（無時鐘路徑不出錯）。
    - 實際時鐘：下一趟行程第一天，整合者不帶 `?at=` 打開網站，核對 now 卡的「現在」與目的地當地時間一致，且每分鐘重標正常；結果記在該趟的 `docs/trips/` 紀錄。這一步不擋部署。
- [ ] **Step 6**：`./bin/travel fetch-weather --plan-id kyoto-nov-2026` 實跑一次，確認 stdout 和 audit 只有一筆 `operation_runs`。
- [ ] **Step 7: 整合者 commit**：`docs: destination-local time wording for the now card and verify-site`


## 審查核對紀錄（2026-11-16 計畫審查）

審查者：Codex（block，整體）、K3.1 Pro（block，整體）。

| 發現（審查者） | 處理 | 理由 |
|---|---|---|
| T3 只用 in-memory DB 與純函式，沒有真 Turso 驗 `--all` 部分失敗（Codex、K3） | 採納 | 新增 `fetch_weather_all.rs`：真 Turso、跑 binary、唯一 fixture、立刻 arm Guard；用 Open-Meteo 會回 400 的座標走真實失敗路徑，不加測試旗標；`_a` 遇網路類錯誤 skip，避免外部服務卡住弄紅測試 |
| host 測試時鐘回 `None`、線上台北 plan 可能不在行程期間，證明不了時區修正（Codex） | 部分採納 | `Intl` 只在 wasm，host 回 `None` 是設計；換算由 `clock_from_parts` 測。線上補「釘選 `?at=` 驗 SSR 與 `data-now-tz`」與「下一趟行程第一天實測未釘選時鐘」，後者不擋部署 |
| `cargo test` 多個 filter 會報錯（K3） | 採納 | T4 拆成兩次；T5 改用 `--` 後多 filter |
