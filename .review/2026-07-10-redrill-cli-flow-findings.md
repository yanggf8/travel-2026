# tokyo-sep 真實演練找到的 CLI/流程改善點(2026-07-10)

背景:`tokyo-sep-2026` 真實資料演練(gwebcdb 抓 google_flights+agoda → write-offers --dest →
promote → select → scaffold/populate/derive-routes/set-meals/set-tod-zh → oracle → validate publish
→ web render)一路做到 VERDICT BETTER。過程中撞到的摩擦 = 這份清單。都是 Claude 第一手核對源碼**並實跑重現**
確認過的(file:line)。**這是發現清單,不是修復計畫** —— 待 Codex 審 + Claude corroborate Codex 後才進修。

規則:fix the LOGIC not the plan;agent-first(每個輸出給下一步最佳行動);fail loud;no synthetic。

**已自我 corroborate:抓到並排除 1 個誤報(見文末 F1-RETRACTED),不讓它進計畫。**

---

## F2 [HIGH · agent-first] 4 個 query/檢查命令 reject `--help`

- `query-destination-ref` / `query-offers` / `query-bookings` / `check-freshness` 對 `--help` 回
  **`unknown flag for <cmd>: --help`**(而 `status`/`itinerary`/`ota-status`/`leave`/`query-recommendations`
  /`query-tour-group-offers`/`shaping-compare` 正常印 Usage)。
- 後果:agent 探索一個命令的第一動作就是 `--help`;這 4 個直接回錯誤,agent 以為命令壞了/不是要找的工具。
  這次我就是因此**沒發現 `query-destination-ref` 能列 clusters/POIs**,改去猜欄位名 raw db exec(見 F3)。
- Claude 已第一手實跑證實(逐一跑過 8 個命令:4 拒 4 過;`query-offers --help` → `unknown flag`)。
- 修法方向:這 4 個命令的 arg-parse loop 遇 `--help`/`-h` 印 Usage 並 `Ok(())`(比照 set-tod 家族在
  main.rs 已有的 `if rest.iter().any(|a| a=="--help"||a=="-h")` pattern),而非落入 unknown-flag 分支。
  抽一個共用 `is_help_flag` 或在各 loop 首檢查,四處一致。**注意**:這 4 個是 query/read 命令,plan 解析走
  view-resolver;修時別動到解析,只在 unknown-flag 判斷前先攔 `--help`。

## F3 [MED · agent-first] populate-itinerary 缺 `--goals` 的錯誤不指路;查 clusters 只能猜欄位名 raw SQL

- `populate-itinerary` 沒帶 `--goals` → `Err("populate-itinerary requires --goals \"<c1,c2,...>\"")`
  (`populate_itinerary.rs:365`)—— **不指去哪找可用 clusters**。而「0 activities added」那條(`:306-308`)**有**
  指 `query-destination-ref --slug <dest>`。同命令兩條錯誤指路不一致。
- 疊加 F2:就算它指了 `query-destination-ref`,那命令又 reject `--help`,所以「怎麼用它列 cluster」也不明。
  這次我全程 raw `db exec` 撈 clusters,還把欄位名猜錯兩次(`destination_clusters` 用 `name`/`best_area` 非
  `title`;`destination_pois` 用 `slug`/`title` 非 `destination`/`name`)。
- Claude 已第一手實跑證實(populate 缺 goals → `requires --goals`;`query-destination-ref --slug tokyo_2026`
  實跑能列 9 areas + clusters + POIs)。
- 修法方向:`requires --goals` 的錯誤訊息尾巴補一句
  `— list available clusters with: travel query-destination-ref --slug <dest>`。（小、純字串,最高性價比。）

## F4 [LOW · agent-first] content-depth SHORT 只給軸名,不給 per-axis 逐日缺口摘要

- oracle 印 `VERDICT: SHORT: routes`(`compare_content_depth.rs:135`),要自己讀 per-day 表(`:195`)算「哪天缺幾條」。
  per-day 資料已印,只是沒把「Day 1 routes 3<5」這種 diff 摘要出來當 worklist。
- 嚴重度:LOW —— 表在,能算,只是多一步。這次我手算 Day1/Day5 是 route 缺口來源。
- Claude 已第一手證實(:135 SHORT 只 join 軸名;:195 per-day 表印 a/m/r 但無 Δ 欄)。
- 修法方向(YAGNI 評估後再定):SHORT 行後補一句最短板日,如 `SHORT: routes (worst: day 1 −2, day 5 −2)`。
  或維持現狀(表已足夠),列為 nice-to-have。

## F5-RETRACTED [誤報 · Codex 抓、Claude corroborate] add-transit 已有 --confidence 旗標

- 原假設:add-transit 沒有旗標標「已查證」vs「估的」,minutes 一律寫 estimate。
- **Codex 審查推翻 + Claude 第一手 corroborate**:`add-transit` **已有** `--confidence verified|reviewed|estimate`
  (`add_transit.rs:87`,預設 estimate;usage 也印了)。這次演練我 backfill 沒帶 `--confidence` 才全落 estimate ——
  是我沒用旗標,不是缺旗標。
- 教訓:又一個「以為缺、其實有」的誤報,這次是 Codex 抓的,我 corroborate 確認。演練時該用 `--confidence verified`
  標查證過的分鐘數。**排除,不修。**

---

## F1-RETRACTED [誤報 · 已排除] "set-meals/set-tod-* 對 --plan-id 靜默 no-op" — 不是真的

- 原假設:`set-meals ... --plan-id X`(未設 `$TRAVEL_PLAN_ID`)靜默 no-op,只印 plan 清單。
- **實跑重現推翻**:`set-meals 3 afternoon --plan-id tokyo-sep-2026 --meal ...`(無 shell 變數、未設
  `$TRAVEL_PLAN_ID`)→ `✅ Meals updated`,實際寫入 1 筆。`resolve_plan_id` 確實認 `--plan-id`
  (`plan_resolver.rs:319` 白名單 + `:621` `"--plan-id" =>` arm)。
- 真正原因:當初那次失敗是我把 `--plan-id X` 塞進 shell 變數 `$P` 展開的**調用問題**,不是 CLI 缺陷。
- 教訓:把自己的調用錯誤誤判成工具 bug。**先自我 corroborate(實跑重現)再交 Codex,才抓得到這種誤報** ——
  同 `cli-audit-corroborate-main-rs`(那次也是沒讀全就誤判)。

---

## 最終判定(Codex 審 + Claude corroborate 完成 2026-07-11)

**真缺陷(進 writing-plans → Grok 施工):**
1. **F2**（4 命令 reject `--help`：`query-offers`/`query-bookings`/`query-destination-ref`/`check-freshness`）——
   CONFIRMED。修 4 個獨立 arg-parse loop(`db_query_offers.rs:81`、`bookings.rs`、`destination_ref.rs:36`、
   `freshness.rs:52`），每個 loop 首加 `"--help"|"-h"` arm 印 Usage + `return`，範本 = `db_schema.rs:16`
   已有的 pattern。TRAP:別動到各命令的 plan/positional 解析,只在 unknown-flag 分支前先攔 `--help`。**最高。**
2. **F3**（populate 缺 `--goals` 不指路）—— CONFIRMED。`populate_itinerary.rs:365` 的 `requires --goals` 錯誤
   尾巴補 `— list clusters with: travel query-destination-ref --slug <dest>`(比照 :306-308 的 0-added 錯誤已有指路)。
   純字串一句,最高性價比。F2 修好後 query-destination-ref 也能 `--help` 探索了,兩者互補。

**誤報(已排除,不修):**
- **F1**（set-meals `--plan-id` no-op）—— Claude 自己抓:實跑 `✅ Meals updated`,是當初 shell 變數調用問題。
- **F5**（add-transit 無 confidence 旗標）—— Codex 抓、Claude corroborate:`add-transit --confidence` 已存在(add_transit.rs:87)。

**Nice-to-have（YAGNI 邊緣,暫不做）:**
- **F4**（content-depth SHORT 只給軸名）—— per-day 表已足夠算缺口。除非之後常做 drill loop 覺得煩,否則不值一個改動。列為觀察。

**成果:5 findings → 2 真缺陷(F2/F3),2 誤報(F1/F5)在施工前攔下,1 nice-to-have 擱置。**
下一步:writing-plans(F2+F3)→ Grok 4.5 施工 → Claude 逐行審+corroborate+serialized 驗證。
