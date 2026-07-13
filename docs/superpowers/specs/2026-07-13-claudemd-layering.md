# CLAUDE.md 分層整理 — 設計

**日期:** 2026-07-13
**狀態:** 設計已定（Claude 探勘 + Codex 逐段分層評審 + Claude 逐項 corroborate vs 源碼/skill/gwebcdb）。待 Yang review → writing-plans。
**來源:** Yang「該整一下 claude.md 了,可以分層出去的文件放到 doc folder」。

## 問題

CLAUDE.md 每次 coding session 都載入 context,現在 **543 行 / 94,368 字元**已太肥:
- **`## Next Steps` 段（507 行→檔尾,45,054 字元 = 全檔快一半）** = 19 條 bullet,全部 `DONE/SHIPPED/FIXED` 完成記錄,沒有一條是真正待辦。單條最長 12,571 字元。
- 其他大段混了「活指令 + 已完成歷史細節」（Architecture / Trip Dashboard / Development）。
- Codex 逐段掃 + Claude 對回源碼,發現 **3 個文件內在矛盾**（不只是肥大,是操作風險）。

## 目標

CLAUDE.md 只留「每次 session 都需要、對『現在怎麼做』有指導價值的活指令」;已完成歷史/實作流水帳分層到 `docs/history/`,用**一條**全域指標連過去（考古找得到,但不占 context）。目標 **200–280 行 / 24–35KB**,設 35KB 軟上限。

## 全域限制（照抄專案規則）

- **不漏活指令為最高優先** — 這是重構核心指令檔,寧可少搬也不能搬走還在生效的規則。模稜兩可 → 標「需人工確認」,不替使用者決定搬走。
- **先解矛盾再分層** — 3 個矛盾若原封搬進 history 一樣害人;分層前先對回現行 source/skill 確認 canonical,改對 CLAUDE.md 再搬。
- **history 檔非規範性** — 每個 history 檔開頭明寫「Historical, non-normative; if this conflicts with CLAUDE.md / the owning SKILL.md / source / tests, the current sources win.」
- Pipeline — Codex 設計/審;Claude 逐項 corroborate + 執行 + verify。

---

## 第 0 步（先做）— 解 3 個矛盾（已 corroborate vs source,canonical 已確定）

分層前先把這 3 處改對,否則矛盾被原封搬走。

### 矛盾① OTA canonical writer（三套打架的說法）
- **現況**:CLAUDE.md 同時有 `travel ota write-offers`（line 232/353）、`ota_parse.py`（line 293）、**line 520「`bridge/ota_write_llm_offers.py`;regex `ota_cli parse` is the fallback」** —— 而 line 354/SKILL.md 又說「regex/parser_rules 全退役」。
- **canonical（對回 source + skill,已 corroborate）**:travel-cli 實際只有 `claim/enqueue/observations/run/write_offers`（**無 `parse.rs`**）;`ota parse` fail-loud 退役（`ota/mod.rs:23`「the coding agent is the parser」）;`scrape-ota` SKILL.md 一致說 canonical 寫入 = **`travel ota write-offers`**。gwebcdb 的 `ota_parse.py`/`ota_write_llm_offers.py` 檔案仍在,但**非現行主路徑**。
- **改法**:CLAUDE.md 修正成「canonical = `travel ota write-offers`（agent 是 parser）」單一說法;line 520 那句舊 fallback 敘述刪除/搬 history。

### 矛盾② Dashboard Rust vs legacy TS 混一節
- **現況**:line 451 明寫「the section below still describes its [legacy TS] behavior」,line 460/470/472 真的還在描述 legacy TS 的 `?edit=TOKEN`/`ADMIN_TOKEN`/舊 routes/舊 secrets。
- **canonical（對回 source,已 corroborate）**:`-rs` worker grep **無** `edit=TOKEN`/`ADMIN_TOKEN`/`api/edit`（印證 line 459「not in -rs」）;legacy TS 已 RETIRED（2026-07-02,undeployed）,source 留到 2026-08-02 review。
- **改法**:`Dashboard Operations` 只留 `-rs` current 操作（deploy/OAuth/share-token/maps/troubleshooting）;legacy TS 的 edit mode/ADMIN_TOKEN/舊 routes/舊部署指令搬 `docs/history/dashboard.md`。

### 矛盾③ no-JSON 契約 vs data/*.json / scrapes/*.json（含幽靈檔案）
- **現況**:line 67「no config JSON files」;Project Structure（line 385-388）列 `data/holidays/taiwan-2026.json`、`data/hotel-areas.json`、`data/transport-routes.json`。
- **canonical（對回 source,已 corroborate）**:`data/` 目錄**實際只有 2 個 .md 檔**（tokyo-trip-plan）——**line 385-388 列的 3 個 json 檔根本不存在（幽靈檔案）**。真正被讀的是 `data/state.json`（`db_sync_events.rs`,legacy 事件同步）。
- **改法**:修正 Project Structure,刪幽靈 json 檔;把 no-JSON 契約邊界寫清楚（reference/raw-landing 的合法例外:`shaping` handoff、`captures` envelope、gitignored `scrapes/` landing、legacy `data/state.json` sync —— 這些不是 trip data 的 source of truth）。

---

## 第 1 步 — 抽出「仍生效的活契約」放回 CLAUDE.md 正確活章節

Codex 從散在 Next Steps 各 DONE 記錄裡挖出的、**仍在生效、不能跟 DONE 敘事一起消失**的規則（Claude 已抽樣 corroborate,分 8 類）。這些要先確保在 CLAUDE.md 有落點（多數已在別的活章節,補齊缺的）：

**Rust/DAL/mutation** — domain reads/writes 經 `travel-db` repos;audit triad 留 `cascade::common`（repo 不寫 audit）;新 mutation 先寫 events 再 `record_operation` 一次;`db_migrate`/`db exec` 是 inline SQL 例外;新 offer filter 加入 `OfferFilter`（不重建字串 SQL）;`repo::process_statuses::upsert` 與 scaffold 的 INSERT-OR-REPLACE **語意不同不可合併**;archived TS read-only 不得再 refactor/port。

**測試安全（高風險,務必留全）** — shared live Turso 測試用 canonical `common::teardown_plan`;Guard 在 plan id 綁定後**立刻** arm;non-plan-keyed rows 測試自行清理（有 FK/subquery 順序要求）;需 `teardown_offers` 的資料不可漏;**共享 Turso 測試背景執行**（避免 timeout SIGTERM 使 Guard 無法 Drop）;驗證通過 ≠ teardown 正確,必須實際 review cleanup。

**OTA** — agent 是 parser（capture text → TSV → normalized offers）;`write-offers` 要求 `--dest`,slug 必須驗證（不發明 region→slug mapping）;outbound-only flight 是合法 offer（不強制 return leg）;新 source onboarding 是 DB workflow/config（不加 per-source parser code）;legacy Python scraper 與 chromeport 禁止作 fallback;登入/2FA 由人處理;`parser_rules`/in-CLI regex parser 已退役。

**Workflow/品質門檻** — known flights/hotel 是預設 fast path,shaping 是條件式工具;`shaping-purchase-matrix` 僅 shaping run 有 offers 時用;drill 找流程/工具缺陷（不只修 synthetic plan）;synthetic `[DRILL]` plan 不得 publish;`validate publish` 是 Stage 4 gate;`compare content-depth` 現行 = **3 depth axes + ZH slot-completeness gate**（舊「4 axes / ZH %」已取代）;Dashboard web render 是最終視覺 gate;不得填造假空 session/ZH 內容。

**Plan/destination** — view 的 `--dest` 目前 validation-only;view 只渲染 active destination（錯的 fail loud）;非 active destination 支援等首個 real multi-dest plan（不造 speculative path）;destination override 必須屬於該 plan;測試 seed 必須建對應 `plan_destinations`。

**Map/POI/路線** — `set-activity-poi --auto` 只連「唯一且已 geocode」的確定匹配（0/複數/ungeocoded 不猜）;POI matching rule 共用（不各寫一份）;transit pair normalization 單一 shared 實作;derive routes 缺 metadata 的正確 loop = `derive → add-transit → re-derive`;有 activities 但無 geocoded stop/route 的日子要 map warning。

**Omiyage** — item 與 seller 都要官方證據（Google Maps 只證店存在,不證店賣該商品）;unverifiable candidate 不寫入;「auto-generate」= flow 讓 agent 研究驗證（非 DB/cascade 憑空生 row）;global 表只表達商品+銷售地點,購買時間屬 plan itinerary activity;不把 last-day policy 寫回 global 表,不引入 `perishable/buy_timing` enum。CLAUDE.md 留 2-3 行總契約,細節由 Stage 3 skill/spec 承接。

**delegation（提升為正式規則）** — delegate 不得執行**任何** git 操作（不只 commit）—— 從 omiyage 事故故事提升為 CLAUDE.md 正式規則。

---

## 第 2 步 — Next Steps 分層策略（Codex 建議 b:依主題拆多檔）

`Next Steps` 19 條 DONE 記錄搬到 `docs/history/`,依粗粒度主題拆檔（比單一 45KB CHANGELOG 好:查單一主題時只載入相關檔;比留摘要好:DONE 摘要留熱 context 仍會過期,且已出現「舊結論撤回」「同條 IN-PROGRESS→DONE」問題）：

```
docs/history/
├── README.md                          # 主題+時間索引 + 非規範性聲明 + 全域指標目標
├── rust-cli-dal-and-tests.md          # Rust port / DAL 遷移 / test-harness decoupling
├── ota-pipeline.md                    # OTA execution layer / resolver / promote-bridge
├── dashboard.md                       # workers-rs port / OAuth / share-link / legacy TS 操作
├── planning-flow-cli-and-drills.md    # drills / CLI hardening / content-depth / F/G findings
└── features-and-reference-data.md     # omiyage / map-coverage / add-transit / POI
docs/trips/
├── 2026-okinawa.md                    # Okinawa 旅程狀態（從 Trip Details/Current Status 移出）
└── drills.md                          # drill 旅程記錄（kyoto-jul / masterdrill 等 synthetic）
```

**遷移注意**：
- 第 521 行那個 12,571 字元 bullet **不可當一個單位搬** —— 它混了 OTA / DAL / resolver / Dashboard D1 四主題,要拆到對應檔。
- **Next Steps 並非全部已完成** —— 這些是真的 open,先**人工分流**（進 backlog 而非 history archive，不能混進 DONE 後消失）：real-drill 的 #2/#3/#4/#5a/#5c「NOT yet fixed」;新 destination/source onboarding;D1 pilot（deploy-gated）;legacy TS Worker 2026-08-02 archive review。
- 每 history 檔保留日期/commit/spec/review 連結。

---

## 第 3 步 — CLAUDE.md 新結構（Codex 建議,Claude 採納）

目標 top-level（約 200–280 行 / 24–35KB;禁單行數千字 journal、禁 commit 清單/測試通過數字/live-smoke 數量/某代理實作過程）：

```
# Japan Travel Project
## Session Routing            — Skill Decision Tree / CLI / history / trip records 短路由
## Non-Negotiable Contracts   — Turso source-of-truth / plain-text CLI 無 JSON boundary / travel-db DAL boundary / audit triad / no local fallback / plan+destination resolution
## Agent-First Workflow       — 預設 known-flights path / decision tree / skill 位置 / OTA URL routing
## Development and Verification — build/test/pre-commit / shared-live-DB 測試安全 / token+sandbox / 精簡 repo map / raw SQL decision
## CLI Quick Reference        — 只留最高頻 15-25 命令 + plan resolution;完整旗標指 docs/reference/CLI.md
## Dashboard Operations       — 只 current Rust Worker（auth/share/deploy/maps/troubleshooting）
## Documentation Map          — active specs / CLI reference / trip records
## Historical Records         — 一條非規範性指標
```

整併/刪除：`Trip Details`+`Current Status`→`docs/trips/`（即時狀態改 CLI 查）;`Architecture`+`Turso DB`+`DB Operation Decision`→`Non-Negotiable Contracts`;`Available Skills`+`OTA Sources`→`Agent-First Workflow`;`Project Structure`+`Development`+`Build Gate`→`Development and Verification`;`Next Steps`→完全移除;`Trip Dashboard`→重寫成 current-only `Dashboard Operations`。

---

## 第 4 步 — 指標策略（Codex 建議）

CLAUDE.md 尾端**只留一條全域歷史指標**（不在每主題各放「歷史見…」,那會讓熱檔重長索引噪音、誘導 agent 讀過時內容）：

> Historical implementation records are non-normative and may be superseded: see `docs/history/README.md`.

「現行操作文件」在使用點就近指向（已有慣例,保留）：CLI flags → `docs/reference/CLI.md`;OTA → `src/skills/scrape-ota/SKILL.md`;Stage 3 → `src/skills/stage3-expand-itinerary/SKILL.md`。

---

## 驗收

- CLAUDE.md 降到 ~200–280 行 / 24–35KB,`Next Steps` 段移除,無單行數千字 journal。
- 3 矛盾解決:OTA 單一 canonical（write-offers）、Dashboard 只 current `-rs`、Project Structure 無幽靈 json 檔 + no-JSON 邊界寫清。
- 第 1 步的 8 類活契約**每條**都能在 CLAUDE.md 找到落點（grep 驗證關鍵規則字串仍在）。
- `docs/history/` 6 檔 + README（非規範性聲明）建立;每 DONE 記錄找得到對應 history 位置;open 項目進 backlog 不進 archive。
- 12,571 字元 bullet 拆到對應主題檔（不整塊搬）。
- CLAUDE.md 尾端一條全域歷史指標。
- **人工確認清單**（分層執行前逐項確認,不替使用者決定）：`make setup` 是否仍建 chromeport（「仍建但不得執行」是否刻意）;`/p5-itinerary` 等 skill 是否仍現行入口;HTTP pipeline 直接 mutation 是否仍允許;`db seed plans` 是否仍是正確修復;Rust Worker 實際支援哪些舊 TS routes。
