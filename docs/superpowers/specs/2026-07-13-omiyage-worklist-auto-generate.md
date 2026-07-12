# 伴手禮流程中自動產生 (omiyage-worklist) — 設計

**日期:** 2026-07-13
**狀態:** 設計已定（Claude brainstorm + Codex 設計探索 + Claude 逐項 corroborate vs 源碼）。待 Yang review → writing-plans（+ Codex 審 plan/test-plan + Claude corroborate）→ Grok 4.5 施工 → Claude 審+verify。
**來源:** Yang 需求「伴手禮應在流程中自動產生,而不是加入」+「應該要有 cli」。延續已上線的手動 omiyage feature（`omiyage-feature` 記憶 / `2026-07-12-omiyage-recommendation-feature.md`）。

## 目標

讓伴手禮在**規劃流程中自動產生**,而非只靠手動 `add-omiyage`。核心洞察（Codex + Claude corroborate）:「自動」= **流程編排 agent 去查證**,不是資料庫自動生成 row —— 因為 no-cheat 要求品項+賣家都要真證據,agent 不能憑印象生成(就像餐廳不能編評分)。

## 全域限制（照抄專案規則）

- **No-cheat（核心）** — omiyage 兩表只放**查證過的事實**。候選(POI notes 提示)**絕不**當事實寫入。
- **No hardcode / Turso-only / agent-first plain-text / fail-loud**。
- **Read-only 發現命令,無 audit,無 --plan-id**（像 query-omiyage / query-destination-ref）。
- **不改既有 schema / 不加 pending confidence**（見「決策」）。
- **Pipeline** — Codex 設計/審;Grok 4.5 施工;Claude 逐行審 + corroborate + verify。

---

## 決策 — 兩層:read-only 發現 (`omiyage-worklist`) + agent 查證後走既有 `add-omiyage`（Codex 建議 + Claude corroborate）

**決定:** 加一個 **read-only** CLI `travel omiyage-worklist --slug <dest>` —— 從既有 omiyage-tag POI 發現候選、原樣印 notes 當**未查證提示**、產出 verify worklist + add-omiyage 模板,**寫 nothing**。Stage 3 skill **自動跑它** → agent 對每項 gwebcdb 查證兩半 → 用**既有 add-omiyage** 寫入 → query-omiyage + validate data 收尾。

**「自動」的正確意思(Codex):** Stage 3 自動從 Turso 發現 omiyage 研究 worklist,agent 用真來源查證每項的兩半,才透過 add-omiyage 持久化。**不**是把 POI notes 自動晉升為推薦。

**為什麼不寫 pending row / 不加 pending confidence（Codex 6 理由 + Claude corroborate）:**
- 既有兩表是乾淨的 canonical 斷言:此品項存在 / 此 POI 賣它 / 兩半都有 provenance（`schema.sql:382`;`add_omiyage.rs:30` 原子要求兩半）。
- 加 pending → canonical query 混入假設;每個 reader 要 filter;query-omiyage/validate/dashboard 可能把提示當推薦呈現;**pending location row 結構上就在斷言 `item_id @ poi_id` —— 那正是還沒證明的賣家事實**（no-cheat 違規）;需 confirm/reject/expire/dedup 生命週期機制(無需求);把 plan-level 政策帶進全域 slug-keyed reference data。
- 既有 validator 刻意把空 omiyage 資料當 valid、只驗已存事實（`validate.rs:989`）—— 保持此 invariant。
- **canonical 表 fact-only,候選只印不寫。**

**out of scope:** pending/candidate 表;pending confidence;POI-note → 品項的自動萃取(notes 是提示不是事實);ranking;dashboard 渲染。

---

## CLI — `travel omiyage-worklist --slug <destination_slug>`（read-only）

**命名（Codex）:** 用 `omiyage-worklist` —— **不叫** `scaffold-omiyage`（既有 scaffold-* 寫 row,`scaffold_itinerary.rs` 10 處 INSERT,Claude 確認 —— read-only 用此名會誤導）,**不叫** `generate-omiyage`（CLI 無法自造有證據的推薦）。

**行為（模仿 query-omiyage 的 read-only 結構 — `query_omiyage.rs:13-22`:connect_read → config_slug_exists → render;`--plan-id` 明確 reject）:**
- dest 不在 `destination_config` → fail loud。
- 查 `destination_poi_tags` 的 **exact `tag='omiyage'`** → JOIN `destination_pois`（Claude corroborate:tokyo 有 isetan/daimaru,osaka_kyoto 有 kuromon）。
- 也讀該 slug/POI **既有 canonical omiyage locations**（rerun 顯示每個 POI 已 sourced 幾個,友善)。
- **POI notes 原樣印**（**不** split comma / 不推品牌 / 不建 item_id / 不分類 category —— notes 是 narrative 提示;poi_tags 是分開的 row 所以 POI 選擇可靠,但品項萃取不可靠）。
- **寫 nothing、無 --plan-id、reject 未知 flag、`--help`**。
- **無 omiyage-tag POI → 非零退出 + fail-loud 訊息**。

**純文字輸出(Codex 建議形狀):**
```
Omiyage research worklist: tokyo_2026
WARNING: POI notes are hints, not item or seller evidence.

POI isetan_shinjuku — Isetan Shinjuku B2
  area: shinjuku
  note hint: Premium omiyage: Yoku Moku, Henri Charpentier, Pierre Herme. B1-B2 food halls.
  already sourced here: 0

  VERIFY BEFORE ADDING:
    1. official item/product page
    2. official branch/floor-guide page proving sale at isetan_shinjuku

  CONFIRM WITH:
    travel add-omiyage tokyo_2026 <item_id> --buy-at isetan_shinjuku \
      --location-source-url <url> --location-confidence reviewed \
      --name <name> --category <category> \
      --item-source-url <url> --item-confidence reviewed
```
模板可填 slug + poi_id（Turso 事實）;**item identity / category / URLs / confidence 留 placeholder**（那些要 agent 查證）。

**DAL:** SQL 進既有 `travel-db::repo::omiyage`（加 `omiyage_worklist_pois(conn, slug) -> Vec<WorklistPoi>` 讀 poi_tags⨝pois + already-sourced count）;CLI 只 orchestrate+render。

---

## Stage 3 hook（agent-first）

`src/skills/stage3-expand-itinerary/SKILL.md` 尾段（agent-first + labeled provenance,Claude 確認）加:Stage 3 **自動跑 `omiyage-worklist --slug <dest>`** → 對每個候選 agent gwebcdb 查證品項官方頁 + 賣家分店頁 → 用 `add-omiyage` 寫入（provenance 齊全）→ `query-omiyage` 檢視 + `validate data` 確認。守 no-cheat:查不到證據的候選**不寫**（誠實留缺,像餐廳查不到就不編）。

---

## 驗收

- `omiyage-worklist --slug <dest>`: read-only(寫 0 row),查 omiyage-tag POI + 原樣印 notes + WARNING + verify 步驟 + add-omiyage 模板(slug/poi 填,其餘 placeholder) + already-sourced count。dest 不存在 / 無 omiyage-tag POI → fail loud。reject --plan-id / 未知 flag。--help。
- **無 pending row / 無 schema 改 / 無 confidence enum 改** —— canonical 表仍 fact-only,validate 空表仍 valid。
- Stage 3 skill 記錄 auto-run + agent gwebcdb 查證 + add-omiyage 的流程。
- Behavior-lock 測試(real-Turso,common:: harness,全域列 teardown 依序 locations→items→pois→config): worklist 印出 seeded omiyage-tag POI 的 notes + 模板 + already-sourced count;無 tag POI → fail;unknown dest → fail;--plan-id/未知 flag reject;**寫 0 row**(跑後查 4 表無新增)。
- Live smoke: `omiyage-worklist --slug tokyo_2026` 印出 isetan/daimaru worklist(它們已有 omiyage tag + notes),不寫任何 row。
