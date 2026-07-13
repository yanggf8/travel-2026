# CLAUDE.md 分層整理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 CLAUDE.md（543 行 / 94KB）瘦到 ~200–280 行 / 24–35KB —— 先解 3 個文件內在矛盾,抽出仍生效的活契約,把 45KB 的 Next Steps DONE 流水帳搬到 `docs/history/` 主題檔,重組成 8-section 結構,尾端留一條全域歷史指標。

**Architecture:** 這是**文件重構**,不是程式碼。「測試」= (1) `grep` 驗證關鍵活契約字串在重構後仍在 CLAUDE.md;(2) `wc` 驗證行數/字元數達標;(3) `git grep` 驗證搬走的 DONE 內容在 `docs/history/` 找得到。順序**必須**是:先解矛盾（第 0 步,否則矛盾被原封搬走）→ 抽活契約回落點 → 搬歷史 → 重組結構 → 全域指標。每步一個 commit。

**Tech Stack:** Markdown 檔編輯（Edit/Write）;`grep`/`wc`/`git grep` 當驗證 oracle;`./bin/travel` 當 source-of-truth 驗證（不是改它）。

## Global Constraints

（逐字抄自 spec,每個 task 隱含包含）
- **不漏活指令為最高優先** — 寧可少搬也不能搬走還在生效的規則;模稜兩可標「需人工確認」不替使用者決定搬走。
- **先解矛盾再分層** — 3 矛盾若原封搬進 history 一樣害人。
- **history 檔非規範性** — 每個 history 檔開頭明寫:「Historical, non-normative record. If this conflicts with `CLAUDE.md`, the owning `SKILL.md`, source code, or tests, the current sources win.」
- **canonical 對回 source,不對回 CLAUDE.md**（它就是矛盾源）。
- Pre-commit hook 會跑 `cargo build -p travel-cli` + `validate data`;純 doc 改動不影響,但 commit 前 export 好 Turso env（見 CLAUDE.md「Token resolution / sandbox gotcha」）。
- **每個 commit 只 stage 自己的 pathspec**（平行 session 會污染 index）。
- 目標 CLAUDE.md 禁單行數千字 journal、禁 commit 清單/測試通過數字/live-smoke 數量/某代理實作過程。

## 精確錨點（source-corroborated 2026-07-13）

CLAUDE.md 段落行號:`## Trip Details`=11, `## Architecture`=17（`### Data Model`=19, `### Cascade Rules`=22）, `## Development`=77（`### CLI Execution`=79）, `## Agent-First Workflow`=140（`### Skill Decision Tree`=157, `### URL Routing`=209）, `## Available Skills`=260, `## OTA Sources`=283, `## Current Status`=300, `## CLI Quick Reference`=308, `## Project Structure`=389, `## Turso DB`=427（`### DB Operation Decision`=442）, `## Trip Dashboard`=447（`### Dashboard Troubleshooting`=477）, `## Build Gate`=504, `## Next Steps`=507→543(檔尾)。

3 矛盾行號:① OTA fallback 舊敘述在 **line 520**;② Dashboard legacy TS 混入在 **line 459/460/470/472**（line 451 是 legacy worker 說明,保留但精簡）;③ 幽靈 json 在 **line 393/394/395**。

**矛盾③ 修正（re-corroborate 2026-07-13,比 spec 更精確）**:`data/` 實際只有 2 個 .md;line 393-395 的 3 個 json 檔不存在。**且** `compare_true_cost.rs:101/118` 現在從 **Turso 表 `hotel_areas`/`transport_routes` 讀**（`db_migrate.rs:1210/1222` 建表）,不讀 json。所以正確結論:這些 reference data **已從 json 遷進 Turso 表,line 393-395 是遷移前殘留**（印證 no-JSON 契約已落實,不是「邊界不清」）。

---

## Task 1: 建 docs/history/ 骨架 + 非規範性聲明

**Files:**
- Create: `docs/history/README.md`
- Create: `docs/history/rust-cli-dal-and-tests.md`（空骨架 + 聲明）
- Create: `docs/history/ota-pipeline.md`
- Create: `docs/history/dashboard.md`
- Create: `docs/history/planning-flow-cli-and-drills.md`
- Create: `docs/history/features-and-reference-data.md`

**Interfaces:**
- Produces: 6 個 history 檔 + README 索引,供 Task 4 把 Next Steps 內容搬入。每檔開頭有非規範性聲明。

- [ ] **Step 1: 建 6 個 history 檔,每檔開頭一致的非規範性聲明**

每個主題檔（`rust-cli-dal-and-tests.md` 等 5 個）開頭寫:
```markdown
# <主題> — 歷史實作記錄

> Historical, non-normative record. If this conflicts with `CLAUDE.md`, the owning `SKILL.md`, source code, or tests, the current sources win.

（Next Steps 對應主題的 DONE 記錄搬入,保留日期/commit/spec/review 連結。）
```

`README.md` 內容:
```markdown
# docs/history — 歷史實作記錄索引

> Historical, non-normative records. If any of these conflict with `CLAUDE.md`,
> the owning `SKILL.md`, source code, or tests, the current sources win. These
> are an engineering journal for archaeology, NOT current instructions.

## 主題檔
- [rust-cli-dal-and-tests.md](rust-cli-dal-and-tests.md) — Rust port / DAL 遷移 / test-harness decoupling
- [ota-pipeline.md](ota-pipeline.md) — OTA execution layer / resolver / promote-bridge
- [dashboard.md](dashboard.md) — workers-rs port / OAuth / share-link / legacy TS 操作（已退役）
- [planning-flow-cli-and-drills.md](planning-flow-cli-and-drills.md) — drills / CLI hardening / content-depth / F/G findings
- [features-and-reference-data.md](features-and-reference-data.md) — omiyage / map-coverage / add-transit / POI

## Open items（非歷史 — 見對應 backlog / spec）
（Task 4 把 Next Steps 裡真正 open 的項目分流到這裡當指標,不進 archive。）
```

- [ ] **Step 2: 驗證骨架建立**

Run: `ls docs/history/ && head -3 docs/history/ota-pipeline.md`
Expected: 6 檔 + README 列出;每檔第 3 行是非規範性聲明。

- [ ] **Step 3: Commit**

```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
git add docs/history/
git commit -m "docs(history): scaffold docs/history/ with non-normative headers

6 topic files + README index for the CLAUDE.md Next Steps journal migration.
Each file declares itself non-normative (current sources win on conflict).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: 解矛盾①（OTA canonical writer）+ ③（幽靈 json）

這兩個是「就地修正 CLAUDE.md 錯句」,不搬移,合成一個 task（都是小幅事實修正）。

**Files:**
- Modify: `CLAUDE.md:520`（OTA fallback 舊敘述）
- Modify: `CLAUDE.md:393-395`（幽靈 json 檔）

**Interfaces:**
- Consumes: 無。
- Produces: CLAUDE.md 的 OTA canonical 說法唯一化 + Project Structure 反映 Turso-表現實。Task 4 搬 line 520 整條到 history 時,搬的是已修正版。

- [ ] **Step 1: 修正矛盾① — line 520 的 OTA fallback 舊敘述**

line 520 現在的開頭:`Agent-parse reads capture \`raw_text\` → TSV → \`bridge/ota_write_llm_offers.py\`; regex \`ota_cli parse\` is the fallback.` —— 這和 line 354/SKILL.md「regex 全退役」矛盾。canonical（`ota/mod.rs:23` + `scrape-ota` SKILL.md）= `travel ota write-offers`。

改成:
```
- **OTA scraping pipeline (gwebcdb / WSLg).** Agent-parse reads capture `raw_text` → TSV → `travel ota write-offers` (the single canonical writer; the coding agent IS the parser — the in-CLI regex path and `ota parse` are RETIRED, and gwebcdb's `ota_parse.py`/`ota_write_llm_offers.py` are legacy Python, not the current path).
```
（其餘 line 520 的 chrome_session/promote-offers/spec 連結**這步不動**,Task 4 整條搬 history 時處理。）

- [ ] **Step 2: 修正矛盾③ — line 393-395 幽靈 json**

line 393-395 現在列 3 個不存在的 json 檔並說「used by compare-true-cost」。實際 `data/` 只有 2 md,且 `compare_true_cost.rs:101/118` 從 Turso 表 `hotel_areas`/`transport_routes` 讀。

把 line 393-395 三行改成反映現實:
```
│   ├── tokyo-trip-plan.md          # legacy trip note (reference; not read by the CLI)
│   └── tokyo-trip-plan-zh.md       # legacy trip note (ZH)
```
（holiday/hotel-areas/transport-routes 的 reference data 現在在 Turso 表 `hotel_areas`/`transport_routes`,由 `compare true-cost` 讀 —— 不是 json 檔。這句歸屬 Task 5 的 Non-Negotiable Contracts / Project Structure 精簡時併入。）

- [ ] **Step 3: 驗證兩處修正**

Run: `grep -n "single canonical writer" CLAUDE.md && grep -c "transport-routes.json" CLAUDE.md`
Expected: line 520 出現「single canonical writer」;`transport-routes.json` 計數 = 0（幽靈檔已移除）。

- [ ] **Step 4: Commit**

```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
git add CLAUDE.md
git commit -m "docs(claude): fix OTA-canonical + ghost-json contradictions (step 0)

(1) OTA had 3 conflicting canonical-writer claims; unify to \`travel ota
write-offers\` (agent-is-parser) per ota/mod.rs + scrape-ota SKILL.
(2) Project Structure listed 3 data/*.json files that do not exist; the
reference data moved into Turso tables hotel_areas/transport_routes (read by
compare_true_cost.rs), so the json listing was pre-migration residue.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: 解矛盾②（Dashboard legacy TS 混入）— 拆 current vs legacy

**Files:**
- Modify: `CLAUDE.md:447-503`（`## Trip Dashboard` 整段）
- Append: `docs/history/dashboard.md`（legacy TS 操作搬入）

**Interfaces:**
- Consumes: Task 1 的 `docs/history/dashboard.md` 骨架。
- Produces: CLAUDE.md 的 Dashboard 段只剩 `-rs` current 操作;legacy TS 的 edit-mode/ADMIN_TOKEN/舊 routes/舊 secrets 移入 history。Task 5 重組時把此段改名 `## Dashboard Operations`。

- [ ] **Step 1: 把 legacy TS 專屬行搬到 docs/history/dashboard.md**

從 CLAUDE.md `## Trip Dashboard` 段抽出**只屬於 legacy TS**（已 RETIRED,`-rs` 無此功能,grep 已證 `-rs` 無 `edit=TOKEN`/`ADMIN_TOKEN`/`api/edit`）的行,append 到 `docs/history/dashboard.md`:
- line 460 `**Edit mode** — ?edit=TOKEN ... ADMIN_TOKEN ...` 整條
- line 470 `**Routes**` 裡的 `?edit=TOKEN` / `POST /api/edit` 部分（保留 `-rs` 有的 routes）
- line 472 `**Secrets**` 裡的 `ADMIN_TOKEN (edit mode)` 部分
- line 459 尾巴 `legacy TS worker edit mode used ?edit=TOKEN (not in -rs)` 的 legacy 註解

append 格式:
```markdown

## Legacy TS worker (RETIRED 2026-07-02) — operational reference

（undeployed via wrangler delete; source kept pending 2026-08-02 archive review.
NOT current — the live worker is trip-dashboard-rs, which has NO edit mode.）

- Edit mode: `?edit=TOKEN` (TOKEN == `ADMIN_TOKEN` secret); pencil icons; POST `/api/edit`.
- Routes incl. `?plan=<slug>&edit=TOKEN`, `POST /api/edit`.
- Secrets incl. `ADMIN_TOKEN`.
```

- [ ] **Step 2: 從 CLAUDE.md 刪掉那些 legacy 行 + 精簡 line 451**

- 刪 line 460（Edit mode 整條）
- line 470 Routes 移除 `?edit=TOKEN` / `POST /api/edit`（`-rs` 無）
- line 472 Secrets 移除 `ADMIN_TOKEN (edit mode)`
- line 459 移除 `legacy TS worker edit mode used ?edit=TOKEN (not in -rs)` 尾註
- line 451（legacy worker 說明）精簡成一行:`workers/trip-dashboard/ — legacy TS, RETIRED 2026-07-02 (undeployed; source pending 2026-08-02 review). Old URL 301-redirects → -rs via workers/trip-dashboard-redirect/. Details: docs/history/dashboard.md.`

- [ ] **Step 3: 驗證 legacy 已搬走、-rs 操作保留**

Run: `grep -c "ADMIN_TOKEN\|?edit=TOKEN" CLAUDE.md && grep -c "trip-dashboard-rs\|share-token\|snapshot-maps" CLAUDE.md && grep -c "ADMIN_TOKEN" docs/history/dashboard.md`
Expected: CLAUDE.md 的 `ADMIN_TOKEN`/`?edit=TOKEN` = 0;`-rs`/share-token/snapshot-maps 仍 >0（current 操作保留）;history 的 ADMIN_TOKEN >0（搬到了）。

- [ ] **Step 4: Commit**

```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
git add CLAUDE.md docs/history/dashboard.md
git commit -m "docs(claude): split retired legacy-TS dashboard ops out of live section

The live worker is trip-dashboard-rs (no edit mode, grep-confirmed). The
retired legacy-TS edit-mode/ADMIN_TOKEN/routes were mixed into the live
Dashboard section — moved to docs/history/dashboard.md; CLAUDE.md keeps
current -rs operations only.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: 搬 Next Steps 19 條到 docs/history/ 主題檔 + open 項目分流

**Files:**
- Modify: `CLAUDE.md:507-543`（刪整個 `## Next Steps` 段）
- Append: `docs/history/{rust-cli-dal-and-tests,ota-pipeline,dashboard,planning-flow-cli-and-drills,features-and-reference-data}.md`
- Modify: `docs/history/README.md`（Open items 區填入真正 open 的項目當指標）

**Interfaces:**
- Consumes: Task 1 的 6 個 history 骨架;Task 2 修正過的 line 520。
- Produces: CLAUDE.md 無 `## Next Steps` 段;每條 DONE 記錄在對應主題檔;open 項目在 README 分流。

- [ ] **Step 1: 依主題把 19 條 bullet 分派到 5 個主題檔**

主題歸屬（19 條,依 spec）:
- **rust-cli-dal-and-tests.md**: Test-harness decoupling / set-process-status / DAL 相關段（含 12,571 字元 bullet 裡的 DAL/resolver 部分）
- **ota-pipeline.md**: OTA scraping pipeline(已修正) / Rust-first OTA execution layer(拆:OTA + resolver 部分) / promote-bridge fix(#C+#1) / real-scrape drill / redrill real-data
- **dashboard.md**: Worker workers-rs port / D1 pilot（append 到 Task 3 已建的檔）
- **planning-flow-cli-and-drills.md**: Drill=DIAGNOSTIC / kyoto-jul drill / --dest view / CLI hardening sweep / Second CLI audit / Master drill / F2/F3 / kyoto-oct ZH-gate / content-depth
- **features-and-reference-data.md**: Map-coverage+auto-poi / Omiyage feature / shaping-purchase-matrix / Okinawa trip（Okinawa 旅程細節其實該去 docs/trips/，但本 plan 範圍先進 features-and-reference-data，Task 6 人工確認時再決定是否移 trips/）

**關鍵**:第 521 行那條 12,571 字元 bullet **拆開** —— OTA execution/resolver → `ota-pipeline.md`;DAL 遷移 → `rust-cli-dal-and-tests.md`;D1 pilot → `dashboard.md`。不整塊搬。

每條保留日期/commit/spec/review 連結,原樣貼進對應檔（它們本就是完整敘述）。

- [ ] **Step 2: open 項目分流到 README（不進 archive）**

這些是真 open,寫進 `docs/history/README.md` 的「Open items」區當指標（不混進 DONE archive）:
- real-drill 未修:#2（query-offers 無法 filter by capture/job）、#3（restaurant verification asymmetry）、#4（fit↔group_tour type collapse）、#5a（無 `ota show-capture`）、#5c（無 under-extraction warn）→ 指到 `.review/2026-07-10-real-scrape-drill-findings.md`
- 新 destination/source onboarding（tokyo 以外）— open
- D1 read-mirror pilot — deploy-gated（Yang 手動 `wrangler d1 create`）
- legacy TS Worker — 2026-08-02 archive-or-delete review

- [ ] **Step 3: 從 CLAUDE.md 刪掉整個 `## Next Steps` 段（507→543）**

刪除 `## Next Steps` 標題到檔尾的全部內容（Task 5 會在此位置附近加 `## Historical Records` 指標）。

- [ ] **Step 4: 驗證搬移無遺漏**

Run: `grep -c "^## Next Steps" CLAUDE.md; git grep -c "promote-bridge\|Test-harness decoupling\|content-depth ZH-gate" docs/history/`
Expected: CLAUDE.md 的 `## Next Steps` = 0;history 檔裡找得到這些 DONE 標題（搬到了）。

Run: `grep -c "query-offers can't filter\|show-capture\|2026-08-02" docs/history/README.md`
Expected: >0（open 項目分流到了 README）。

- [ ] **Step 5: Commit**

```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
git add CLAUDE.md docs/history/
git commit -m "docs(history): migrate Next Steps journal to topic files; split 12KB bullet

The 45KB Next Steps DONE/SHIPPED journal moved to docs/history/ topic files
(the 12,571-char OTA bullet split across ota-pipeline/rust-cli-dal/dashboard).
Genuinely-open items (real-drill #2-5c, D1 pilot, TS archive review) routed to
README Open-items — NOT buried in the DONE archive. CLAUDE.md loses the section.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: 重組 CLAUDE.md 成 8-section 結構 + 抽活契約回落點 + 全域指標

這是最重的一步:整併 top-level、確保 8 類活契約每條有落點、尾端加全域指標。

**Files:**
- Modify: `CLAUDE.md`（全檔重組）

**Interfaces:**
- Consumes: Task 2/3/4 後已無矛盾、無 Next Steps 的 CLAUDE.md。
- Produces: ~200–280 行 / 24–35KB 的 8-section CLAUDE.md;所有活契約 grep 得到;尾端一條歷史指標。

- [ ] **Step 1: 整併成 8 個 top-level section**

依 spec 第 3 步整併（保留各段內的活指令原文,只是搬到新 section 下 + 刪已完成歷史細節）:
- `## Session Routing` ← 開頭導覽 + Skill Decision Tree 路由 + CLI/history/trip 短指標
- `## Non-Negotiable Contracts` ← `Architecture`（Data Model/Cascade Rules/規則 bullets）+ `Turso DB` + `DB Operation Decision` 的**契約句**（Turso source-of-truth / plain-text CLI 無 JSON boundary / travel-db DAL boundary / audit triad / no local fallback / plan+destination resolution / no-JSON-in-RDB）。刪「28+」等易過期數字、遷移故事。
- `## Agent-First Workflow` ← 幾乎整段保留（主動下一步/CLI 優先/輸出格式/known-flight fast path/decision tree）+ `Available Skills` + `OTA Sources` + `URL Routing`。刪「每趟旅程都如此」案例證據 + port history + 六來源驗證日期。
- `## Development and Verification` ← `Development`（CLI Execution/Setup/Tests）+ `Build Gate` + `Project Structure`（壓成 10–15 行）。**測試安全活契約全留**（canonical teardown_plan / Guard arm 時機 / non-plan cleanup / 背景執行 / 驗證≠teardown）。刪 12 batches/減幾行/洩漏哪些 row 的事故敘事。
- `## CLI Quick Reference` ← 只留最高頻 15–25 命令 + plan resolution;完整旗標指 `docs/reference/CLI.md`。刪 OTA recipe/tour-group 長清單/完整 mutation 列表。
- `## Dashboard Operations` ← Task 3 後的 current-only `-rs` 段。
- `## Documentation Map` ← `Docs` 精簡 + active specs/CLI ref/trip records 路由。
- `## Historical Records` ← 一條全域指標（Step 3 加）。

`Trip Details` + `Current Status` 的旅程狀態 → 這步先移除段落主體,即時狀態改「跑 `travel plans/status`」一句;旅程細節歸 `docs/trips/`（Task 6 人工確認 Okinawa 是否移）。

- [ ] **Step 2: 驗證 8 類活契約每條都有落點（grep oracle）**

這步是**最關鍵的安全檢查** —— spec 第 1 步的 8 類活契約,每類挑 1–2 個關鍵字串 grep,確保重組後仍在 CLAUDE.md:

Run:
```bash
for s in "teardown_plan" "Guard" "audit triad" "record_operation" "OfferFilter" \
         "write-offers" "known-flights" "validate publish" "content-depth" \
         "set-activity-poi --auto" "transit_key\|pair.*normali" "add-transit" \
         "validation-only" "plan_destinations" "official.*evidence\|Google Maps.*store" \
         "delegate.*git\|git.*delegate" "no local fallback\|fail loud" "No JSON"; do
  printf "%-40s" "$s:"; grep -Eic "$s" CLAUDE.md
done
```
Expected: **每一行計數 ≥ 1**。任何一行 = 0 → 那條活契約漏了,必須補回 CLAUDE.md 才能繼續。

- [ ] **Step 3: 加 delegate-no-git 正式規則 + 尾端全域歷史指標**

在 `## Non-Negotiable Contracts` 或 `## Development and Verification` 加一行正式規則（從 omiyage 事故故事提升）:
```
- **Delegates (Grok/Codex subagents) run NO git operations** — not reset/add/commit/checkout/push. The agent gates every commit itself.
```

在檔尾 `## Historical Records` 加全域指標:
```markdown
## Historical Records

Historical implementation records are non-normative and may be superseded:
see [`docs/history/README.md`](docs/history/README.md). Current sources
(CLAUDE.md / the owning SKILL.md / source code / tests) always win on conflict.
```

- [ ] **Step 4: 驗證行數/字元數達標 + 結構完整**

Run: `wc -l CLAUDE.md && wc -c CLAUDE.md && grep -c "^## " CLAUDE.md`
Expected: 行數 200–280;字元 24,000–36,000（35KB 軟上限,略超可接受但要記錄）;top-level `## ` section 數 = 8（+ 標題 `# Japan Travel Project`）。

Run: `grep -n "^## " CLAUDE.md`
Expected: 正好 Session Routing / Non-Negotiable Contracts / Agent-First Workflow / Development and Verification / CLI Quick Reference / Dashboard Operations / Documentation Map / Historical Records。

- [ ] **Step 5: Commit**

```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
git add CLAUDE.md
git commit -m "docs(claude): restructure into 8 sections; ~200-280 lines / <35KB

Consolidate top-level sections (Architecture+Turso+DB-op → Non-Negotiable
Contracts; Dev+Build+Structure → Development and Verification; Skills+OTA →
Agent-First Workflow). All 8 categories of still-live contracts grep-verified
present. Add delegate-no-git as a formal rule (from the omiyage incident) and
a single global non-normative pointer to docs/history/.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: 人工確認清單 — 逐項確認後補正（不替使用者決定）

**Files:**
- Modify: `CLAUDE.md`（依確認結果微調)
- Possibly Modify: `docs/trips/`（若確認 Okinawa 移出）

**Interfaces:**
- Consumes: Task 5 後的 CLAUDE.md。
- Produces: 5 個「需人工確認」項的確定答案 + 對應補正。

- [ ] **Step 1: 逐項對回 source 確認（讀,不猜）**

逐項驗證（這些 spec 標「需人工確認」,不替使用者決定搬走）:
1. `make setup` 是否仍建 chromeport? → `grep -n chromeport Makefile`;若仍建但 CLAUDE.md 說 chromeport RETIRED,確認「仍建但不得執行」是否刻意,CLAUDE.md 註明。
2. `/p5-itinerary` 等 skill 是否仍現行入口? → `ls src/skills/ && grep -rn "p5-itinerary" src/skills/*/SKILL.md`;過時的從 Skill Decision Tree 移除。
3. HTTP pipeline 直接 mutation 是否仍允許? → 對回 CLAUDE.md「Token resolution」段 + 實際用法;保留或註記。
4. `db seed plans` 是否仍是正確修復方式? → `grep -rn "seed plans\|seed_plans" rust/crates/travel-cli/src/`;確認 Dashboard Troubleshooting 的修復步驟仍對。
5. Rust Worker 實際支援哪些舊 TS routes? → `grep -rn "plan\|token\|nav\|lang\|api/plan" workers/trip-dashboard-rs/src/`;Dashboard Operations 的 routes 清單對回實際。

- [ ] **Step 2: 依確認結果補正 CLAUDE.md**

每項確認後,若 CLAUDE.md 有過時/不精確,就地修正（一句一句改,附確認來源）。若某項確認「現況正確」,無需改。

- [ ] **Step 3: 最終驗證 — 全檔一致性**

Run: `./bin/travel doctor 2>&1 | tail -5`（確認 CLI 健康,間接證明沒動壞 source）
Run: `wc -l CLAUDE.md && grep -c "^## " CLAUDE.md`（確認仍達標）
Run: 重跑 Task 5 Step 2 的 18 條活契約 grep（確認人工補正沒誤刪契約）
Expected: doctor 綠;行數仍 200–280;18 條契約全 ≥1。

- [ ] **Step 4: Commit**

```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
git add CLAUDE.md docs/
git commit -m "docs(claude): resolve manual-confirmation items after source check

Confirmed chromeport build intent / current skill entry points / HTTP-pipeline
mutation / db-seed-plans fix / -rs worker routes against source, corrected
CLAUDE.md where stale. Layering complete: 543→~250 lines, journal in
docs/history/, 3 contradictions resolved, all live contracts grep-verified.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- 第 0 步解 3 矛盾 → Task 2（①③）+ Task 3（②）✓
- 第 1 步抽 8 類活契約 → Task 5 Step 2 的 18 條 grep oracle + delegate-no-git（Task 5 Step 3）✓
- 第 2 步 Next Steps 主題拆檔 + open 分流 + 12KB bullet 拆 → Task 4 ✓
- 第 3 步 8-section 結構 → Task 5 Step 1 + Step 4 驗證 ✓
- 第 4 步全域指標 → Task 5 Step 3 ✓
- 人工確認清單 5 項 → Task 6 ✓
- history 檔非規範性聲明 → Task 1 ✓

**2. Placeholder 掃描:** 無 TBD/TODO/「類似 Task N」;每個 grep/wc 命令都有明確 Expected。Task 4 Step 1 的主題歸屬是清單不是佔位（19 條的實際分派）。✓

**3. Type/命名一致性:** history 檔名 6 個在 Task 1/4/5 一致;section 名 8 個在 Task 5 Step 1/Step 4 一致;活契約 grep 字串在 Task 5 Step 2 和 Task 6 Step 3 一致。✓

**一個 spec 未明、plan 補上的修正:** 矛盾③ 的 canonical 比 spec 更精確（reference data 已遷 Turso 表 `hotel_areas`/`transport_routes`,非「邊界不清」而是「遷移殘留」)—— 已寫入「精確錨點」段 + Task 2 Step 2。
