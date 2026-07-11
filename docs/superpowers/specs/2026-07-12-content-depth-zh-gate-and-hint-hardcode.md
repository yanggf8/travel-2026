# content-depth ZH-completeness gate + de-hardcode place hints — 設計

**日期:** 2026-07-12
**狀態:** 設計已定（Claude brainstorm + Codex 兩輪 design review + Claude 逐項 corroborate vs 源碼）。待 Yang review → writing-plans（和 Codex corroborate plan/test-plan）→ Grok 4.5 施工 → Claude 審+verify。
**來源:** kyoto-oct-2026 真實演練。findings: `.review/2026-07-11-redrill-cli-flow-findings.md`（scratchpad）。

## 目標

修 kyoto 真實演練暴露的兩個缺陷:

- **G2 [MED · oracle metric-design]** — `compare content-depth` 把 ZH 當「越多越深」的第 4 比較軸,分母含 scaffold 造的**真空 session**(無活動)。誠實的短行程(arrival 下午到 / departure 早上走 → 有合理空 session)因此在 ZH 軸結構性吃虧,被誤判 SHORT。而填空 session 的 ZH 是造假(no-cheat)。
- **G1 [LOW-MED · no-hardcode/agent-first]** — 4 處 user-facing hint 把沖繩地名寫死當範例,編別的 dest 時提示卻教用沖繩地名。

## 全域限制（照抄專案規則）

- Agent-first 純文字 stdout(非 JSON)。Fail loud。No hardcode。No cheat（不塞假內容過 oracle）。
- 這是**唯讀比較命令**的改動 + 純字串改動;**不碰**任何 mutation / audit triad。
- 行為鎖測試:real-Turso,`tests/common/mod.rs` harness;現有 content-depth 測試用 `run_or_skip`（credless 跳過）。
- Pipeline: Codex 設計/計畫/test-plan review;Grok 4.5 施工;Claude 逐行審 + corroborate + verify。

---

## 決策 G2 — ZH 從「第 4 比較軸」改成「completeness gate」,對齊 validate.rs 既有做法

**根因（Codex 確認 + Claude corroborate）:**
- `zh_coverage_pct`（`compare_content_depth.rs:80-103`）分母 = `COUNT(all days) + COUNT(all timesofday)`,**無** `EXISTS activities`。scaffold 每天造 4 個空 timesofday(`travel-db itinerary.rs:27-30,78-99`)→ 空 session 全算進分母。
- 這個 % 餵進 `Totals`（`:105-118`）+ 當 SHORT/BETTER 的 4 軸之一（`:121-141`,strictly-greater 也算 ZH）→ oracle 逼 agent 填空 session(no-cheat 衝突)。
- **關鍵事實（Claude 第一手 corroborate）:** `validate publish` **已經**有一個 ZH-completeness gate,做法正是「有活動才要 ZH」:
  - `missing_day_zh`（`validate.rs:1484`）: `EXISTS activities(by day) AND theme_zh IS NULL` → 只抓有活動的 day 缺 theme_zh。
  - `missing_session_zh`（`validate.rs:1513`）: `EXISTS activities(by day,session) AND focus_zh/transit_notes_zh 皆空` → 只抓有活動的 session 缺 ZH。
  - 兩者 `PublishSeverity::Block`（`validate.rs:1265`）。
- 所以 content-depth 和 validate publish **對 ZH 的定義不一致**。G2 的正解是讓 content-depth **對齊 validate.rs 已在 prod 驗證的 gate 語意**,不自創。

**新設計:**
- content-depth 的 4 軸 → **3 個比較 depth 軸（activities / meals / routes-with-metadata）+ 1 個 ZH-completeness gate**。
- **ZH gate 語意（對齊 validate.rs `missing_day_zh`/`missing_session_zh` 的 eligibility）:**
  - 分子 = 有活動的 day 中 theme_zh 非空數 + 有活動的 session 中 ZH 非空數。
  - 分母 = 有活動的 day 數 + 有活動的 session 數。
  - **eligibility = `EXISTS activities`**（Codex 建議、最小、與 validate.rs 一致 — Yang 確認）。空 session/day 不列入。
  - **session-level ZH「非空」的定義對齊 validate.rs**: `focus_zh` OR `transit_notes_zh` 非空即算已翻(validate.rs:1513 用 `focus_zh`/`transit_notes_zh` 二選一)。**注意**: eligibility 用 `EXISTS activities`,**不是**用 ZH 欄本身(Codex 陷阱警告: 若用 ZH 欄決定 eligibility,缺翻譯會從分母消失 → 缺 ZH 反而變 gate PASS)。
  - **gate PASS ⟺ 分子 == 分母**（每個有活動的 day/session 都翻了）。
- **verdict 規則改為:**
  - `BETTER` = ZH gate PASS **且** 3 個 depth 軸都 ≥ 參考 **且** ≥1 depth 軸 strictly >。
  - `ALIGNED` = ZH gate PASS 且 3 depth 軸都 == 參考。
  - `SHORT` = 任一 depth 軸 < 參考 **或** ZH gate FAIL。SHORT 列出:不足的 depth 軸 + (若 gate FAIL) `ZH-gate`。
- **輸出格式:** ZH 從 `Totals` 表移到獨立區塊:
  ```
  gates:
    ZH completeness       20/20  PASS
  ```
  drill FAIL 時: `18/20  FAIL`（並在 SHORT 列出 `ZH-gate`）。
- **也報參考的 gate 狀態（Codex 建議）:** 若參考 ZH gate < 100%（`num != den`）,視為**無效/退化的參考** — 印警告,不讓 drill 繼承較低的完整性標準。（參考本應 100%;若不是,是參考的問題。）
- **enrichment worklist 效果:** SHORT 的 `ZH-gate` 仍是 worklist 訊號,但它現在只叫 agent 翻**有活動**的 session(真該翻的),不會叫它填空 session。

**解決 kyoto 抱怨（Codex Q3 確認 + 數據）:** kyoto content-bearing 全填 → 20/20 gate PASS;okinawa 19/19 gate PASS → 無 false SHORT,且 kyoto 3 depth 軸已 BETTER（31>29 / 12>10 / 22>21）→ 整體 VERDICT BETTER。不靠灌水。

**out of scope:**
- 不改 `depth_rows`（activities/meals/routes 的算法不變 — `:51-78`）。
- 不改 validate publish 的 ZH gate（已對;content-depth 對齊它）。
- 不動 `checks.rs:166-193` 的地名 country-classification 邏輯（Codex 標:那是真邏輯不是 hint,刻意的保守啟發式,另議,別 bundle 進 G1）。

**術語校正（Codex）:** 這是「ZH slot completeness」(每個定義的 ZH slot 都填了),不是「所有內容都忠實翻譯」— 一個 session-level `focus_zh` 可涵蓋多個活動。輸出/文件用「ZH slot completeness」不要說「all content translated」。

---

## 決策 G1 — 4 處 hint 的沖繩地名範例改成通用佔位

**決定:** 4 處 user-facing hint 字串的具體沖繩地名範例,改成通用佔位(不舉特定 dest 的真地名),或只講規則不舉地名例。純字串改動。

**4 處（Claude 獨立 grep = Codex grep,一致）:**
- `set_tod.rs:285` — `e.g. "安里駅 → 赤嶺駅 → iias 沖縄豊崎"`
- `set_route_segment.rs:60` — `e.g. "赤嶺駅", "iias 沖縄豊崎"`
- `validate_itinerary.rs:360` — `e.g. "赤嶺駅", "iias 沖縄豊崎"`
- `validate_itinerary.rs:411` — `e.g. "安里駅 那覇"`

**改法:** 範例改成 schematic 佔位,例如:
- 「clean place chain」例 → `e.g. "<站A> → <站B> → <地標>"`（或英文 `"Station A → Station B → Mall"`）。
- 「clean place name」例 → `e.g. a plain station or landmark name, no （…）notes/＋步行/clock times`。
- `:411` 的「加 駅/站/city」例 → `e.g. add a 駅/站/Station suffix or a city name`。
（實際字串在 plan 定稿;原則:傳達同樣規則,不寫死任一 dest 的地名。）

**理由:** 提示範例寫死特定 dest 的地名違反 no-hardcode 精神 + 編別的 dest 時提示與內容不符(agent/用戶困惑)。這是 cosmetic string cleanup,無邏輯風險。

**out of scope:** `checks.rs` 的地名(country 分類邏輯,非 hint)。測試/註解裡的地名(encode 真實 regression case,保留)。

---

## 測試衝擊（Codex 標 + Claude corroborate）

- `zh_coverage_pct` 是 private,只有 module 內 2 個 caller（`compare_content_depth.rs:182-183`),無其他 reader（Claude grep 確認）→ 改它安全。
- **現有測試 `zh_coverage_is_weighted_not_avg`（`compare_content_depth_behavior_lock.rs:116-160`）會故意壞** — 它造 days + timesofday **無 activities**,鎖 `88%`。新 metric 下這些 session 全非-eligible → 要**重寫**成明確區分:
  1. 有活動的 session 缺 ZH → gate FAIL（降完整性）。
  2. 真空 scaffold session 缺 ZH → gate 仍 PASS（不列入）。
  3. day theme ZH 仍在 gate 分母。
- `verdict_short`/`verdict_aligned`（`:233,274`）用到 ZH — 檢查它們的 seed 是否讓 ZH gate PASS,否則 verdict 邏輯改後會變;需相應更新。
- **歷史 88% 數字:** okinawa 舊 `88%` 出現在 design spec / CLI.md / CLAUDE.md history。新 metric 下 okinawa = 19/19 gate PASS（不再是百分比軸）。處理: 舊文件的 88% **標為歷史**（「(舊 4-軸 metric;新 metric = ZH gate)」）保留歷史脈絡,不改敘述;只更新會被執行的 behavior-lock 測試到新 metric（Yang 確認: 文件是歷史記錄,測試才是活的 — 對齊 `verify-against-committed-tree` 精神）。CLI.md 的 content-depth 說明更新到新的「3 軸 + gate」。

## 驗收

- G2: content-depth 印「3 depth 軸 + ZH gate」; kyoto-oct-2026（content-bearing 全填 ZH,3 depth 軸已贏）→ VERDICT BETTER,ZH gate PASS。一個有活動卻缺 ZH 的 session → gate FAIL + SHORT 列 `ZH-gate`。真空 session 缺 ZH → gate 仍 PASS。參考 <100% gate → 印無效參考警告。
- G1: 4 處 hint 無沖繩地名; 傳達同樣的「乾淨 stop」規則。`checks.rs` 地名邏輯不動。
- 測試: 重寫的 ZH 測試 + verdict 測試綠; 其餘 content-depth 測試綠。
- 對齊: content-depth 的 ZH eligibility == validate.rs `missing_day_zh`/`missing_session_zh` 的 eligibility。
