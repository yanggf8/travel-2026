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
- **ZH gate 語意 — EXACT validator alignment（Codex 審 spec 抓到 DRIFT + Claude 第一手 corroborate 修正):** eligibility **必須逐字對齊** `validate.rs` 的 `missing_day_zh`/`missing_session_zh`,**不是** `EXISTS activities`（初稿寫錯 — validate.rs 的 eligibility 是 OR 鏈,若只用 activities,content-depth 會 PASS 但 validate publish 會 BLOCK 一個 meal-only/route-only/transit-only 的 slot → 兩 gate 不一致,正是本 spec 要避免的）。實際 eligibility（Claude corroborate `validate.rs:1493-1508` / `:1521-1536`）:
  - **day eligible ⟺** `EXISTS activities(by day) OR EXISTS session_meals(by day) OR EXISTS day_route_segments(by day)`（validate.rs:1493-1507）。
  - **session eligible ⟺** `EXISTS activities(by day,session) OR EXISTS session_meals(by day,session) OR transit_notes 非空 OR transit_notes_zh 非空`（validate.rs:1521-1536）。
  - **「已翻」判定（session）:** `focus_zh 非空 OR transit_notes_zh 非空`（validate.rs:1537-1538 只在兩者皆空時報 missing）。**day 已翻 =** `theme_zh 非空`。
  - **「非空」= `NULLIF(TRIM(COALESCE(col,'')),'') IS NOT NULL`**（逐字對齊 validator;whitespace-only 算空）。
  - **eligibility 用內容存在（activities/meals/routes/transit），不用 ZH 欄本身**（Codex 陷阱: 若用 ZH 欄決定 eligibility,缺翻譯會從分母消失 → 缺 ZH 反而 gate PASS）。
  - **gate PASS ⟺ 分子 == 分母**,即「每個 eligible 的 day/session 都已翻」;等價於「validate.rs 的 `missing_day_zh` + `missing_session_zh` 回傳空集」。分子 = eligible-day-已翻 + eligible-session-已翻;分母 = eligible-day + eligible-session。
  - **`0/0`（無任何 eligible slot）= 空洞真 PASS,印 `0/0 PASS`。**
- **verdict 規則改為（Codex: VERDICT LOGIC OK）:**
  - `SHORT` = 任一 depth 軸 < 參考 **或** drill ZH gate FAIL。SHORT 列出:不足的 depth 軸（依 activities/meals/routes 固定序）+ (若 gate FAIL) 尾綴 `ZH-gate`。
  - `BETTER` = 無 SHORT **且** ≥1 depth 軸 strictly >（drill gate PASS 已由「無 SHORT」保證）。
  - `ALIGNED` = 無 SHORT 且 3 depth 軸全 == 參考（gate 只證完整性,非 richness,不能供 strictly-greater）。
  - **wording:** 現有 ALIGNED 文案 `every axis meets the reference exactly`（`compare_content_depth.rs:139`）改為 `all 3 depth axes equal reference; ZH gate PASS`;BETTER 文案改為 `all 3 depth axes >= reference, N strictly greater; ZH gate PASS`。
- **輸出格式（Codex: OUTPUT GAP — 定死區塊順序與格式）:** 區塊順序 = `per-day` → 三列 `totals`（僅 activities/meals/routes,**移除 ZH 列**）→ `gates:` → （若參考 gate FAIL）warning 行 → `VERDICT`。
  - per-day 表**不變**（仍 `a/m/r`,本就無 ZH 欄 — `compare_content_depth.rs:195`）。
  - `gates:` 區塊**兩列**（drill + 參考),label 統一用 **`ZH slot completeness`**:
    ```
    gates:
      ZH slot completeness  drill 20/20  PASS
      ZH slot completeness  ref   19/19  PASS
    ```
    FAIL 時該列 `18/20  FAIL`。
- **參考 gate 處理（Codex: REFERENCE-GATE RISK — 收斂為 warn-and-continue，非 abort）:** 若**參考** ZH gate FAIL（`num != den`）:印一行 warning `⚠ reference ZH gate FAIL (N/M); depth comparison continues; drill must independently PASS` 到 **stdout**;**不** abort、**不** 影響 verdict、**exit 0**、**不** 降低 drill 的 gate 要求。（不用「invalid reference」措辭 — 那暗示該中止。）
- **enrichment worklist 效果:** SHORT 的 `ZH-gate` 仍是 worklist 訊號,但只叫 agent 翻 **eligible** 的 day/session(真有內容的),不會叫它填空 session。

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

**改法（Codex: exact strings 在 spec 定死,不 defer plan;保留 same-country/transit-mode 指引）。四處精確替換,只換地名範例、其餘 guidance 逐字保留:**

1. `set_tod.rs:285` — 現:`...Hint: write the pill as a clean place chain, e.g. "安里駅 → 赤嶺駅 → iias 沖縄豊崎" — no （…）notes, +步行, or clock times inside a stop.`
   → 新:`...Hint: write the pill as a clean place chain, e.g. "<站A> → <站B> → <地標>" — no （…）notes, +步行, or clock times inside a stop.`
2. `set_route_segment.rs:60` — 現:`Hint: use a clean place name (e.g. "赤嶺駅", "iias 沖縄豊崎") — no （…）notes, +步行, or clock times inside the stop; keep both stops in the same country; use mode=transit for a rail/bus leg.`
   → 新:`Hint: use a clean place name (e.g. "<車站>", "<地標>") — no （…）notes, +步行, or clock times inside the stop; keep both stops in the same country; use mode=transit for a rail/bus leg.`
   （`keep both stops in the same country` + `use mode=transit...` **原封保留**。）
3. `validate_itinerary.rs:360` — 現:`Use a clean place name as the stop (e.g. "赤嶺駅", "iias 沖縄豊崎") — no parenthetical notes, +步行, or clock times inside the stop itself.`
   → 新:`Use a clean place name as the stop (e.g. "<車站>", "<地標>") — no parenthetical notes, +步行, or clock times inside the stop itself.`
4. `validate_itinerary.rs:411` — 現:`Add a 駅/站/Station suffix or a city (e.g. "安里駅 那覇").`
   → 新:`Add a 駅/站/Station suffix or a city (e.g. "<車站>駅 <城市>").`

原則:傳達同樣規則,不寫死任一 dest 的真地名;佔位用角括號 schematic。`checks.rs` 邏輯 + 測試/註解地名不動。

**理由:** 提示範例寫死特定 dest 的地名違反 no-hardcode 精神 + 編別的 dest 時提示與內容不符(agent/用戶困惑)。這是 cosmetic string cleanup,無邏輯風險。

**out of scope:** `checks.rs` 的地名(country 分類邏輯,非 hint)。測試/註解裡的地名(encode 真實 regression case,保留)。

---

## 測試衝擊（Codex 審 spec 補漏 + Claude corroborate）

- `zh_coverage_pct` 是 private,只有 module 內 2 個 caller（`compare_content_depth.rs:182-183`),無其他 reader（Claude grep 確認）→ 改它安全。它會被 gate 版取代(回 `(num,den)` 而非 %)。
- **必壞、要換的測試:**
  1. `zh_coverage_is_weighted_not_avg`（`compare_content_depth_behavior_lock.rs:116-160`）— 造 days+timesofday **無 activities**、鎖 `88%`。新 metric 下這些 session 非-eligible。**重寫**成 gate 語意的 case（見下「新增 behavior lock」）。
  2. `renders_header_perday_and_totals`（`compare_content_depth_behavior_lock.rs:446`,Codex 抓到、初稿漏）— assert `ZH coverage` 在 totals 表。ZH 移出 totals 後**必壞** → 改成鎖 `gates:` 區塊 + drill/ref 兩列 gate 輸出。
- **可能不用改 seed（Codex 修正初稿）:** `verdict_short`/`verdict_aligned`/`verdict_better`（`:233/:274/…`）的 helper `seed_depth_counts`（`:168`）已 insert 一個 activity + full ZH → 它們的 eligible session 已翻、gate PASS。**只需** review 確認 gate PASS 不改變其 verdict;多半無需改 seed（但要驗)。
- **新增 behavior lock（Codex 列 + exact-validator-alignment case）:**
  - gate FAIL 單獨就強制 `SHORT`（即使 3 depth 軸全 ≥ 或 ==）。
  - `SHORT` 同時列 depth 缺口（固定序）+ 尾綴 `ZH-gate`。
  - **`transit_notes_zh` 單獨**（focus_zh 空）滿足「session 已翻」→ 不算 missing。
  - **whitespace-only ZH** 仍算 missing。
  - **meal-only / route-only / transit-only 的 eligible slot**（validator alignment）: 有 meal 無 activity 的 session 缺 ZH → gate FAIL（證明 eligibility 是 OR 鏈非純 activities）。
  - **參考 gate FAIL** → 印定義的 warning、exit 0、**不**降 drill 要求、**不**改 verdict。
  - **`0/0`（零 eligible slot）→ gate PASS**、印 `0/0 PASS`。
- **歷史 88% 數字:** okinawa 舊 `88%`（4-軸 metric）出現在 design spec / CLI.md / CLAUDE.md history 等。新 metric 下 okinawa = 19/19 gate PASS。處理（Codex: 限制 doc churn）:
  - **更新** live `docs/reference/CLI.md` 的 content-depth 說明到「3 depth 軸 + ZH slot completeness gate」。
  - **標為歷史**（不改敘述): genuinely-current `CLAUDE.md` 描述處加註「(舊 4-軸 metric;新 = ZH gate)」。
  - **不改** 舊的 `2026-07-06-drill-while-comparing` design/plan（讓歷史照舊描述當時行為 — `verify-against-committed-tree` 精神:文件是歷史,測試才是活的)。

## 驗收

- G2: content-depth 印「3 depth 軸 + ZH gate」; kyoto-oct-2026（content-bearing 全填 ZH,3 depth 軸已贏）→ VERDICT BETTER,ZH gate PASS。一個有活動卻缺 ZH 的 session → gate FAIL + SHORT 列 `ZH-gate`。真空 session 缺 ZH → gate 仍 PASS。參考 <100% gate → 印無效參考警告。
- G1: 4 處 hint 無沖繩地名; 傳達同樣的「乾淨 stop」規則。`checks.rs` 地名邏輯不動。
- 測試: 重寫的 ZH 測試 + verdict 測試綠; 其餘 content-depth 測試綠。
- 對齊: content-depth 的 ZH eligibility == validate.rs `missing_day_zh`/`missing_session_zh` 的 eligibility。
