# kyoto-oct 演練的流程/CLI 改進點 — 最終盤點（2026-07-12）

演練主線產出（**已修上線**）:G1（4 處 hint 去 hardcode 沖繩地名）+ G2（content-depth ZH 改 completeness gate）。
本檔記主線以外、過程中撞到的流程摩擦,第一手核對過真實性,區分 真缺陷 / 排除。

## 真缺陷（待 pipeline 判修法,不現在衝）

### P1 [MED · derive-vs-manual 設計張力] 手動 route leg 與 derive 的 leg 無法在同一天共存
- **現況**（第一手 corroborate `derive_routes.rs:75-78,107,344,403`）:derive-routes 對每天:若該天有**任何** `source='confirmed'` segment → **整天 skip**;否則清掉所有 `ai_recommended` segment 重建。
- `set-route-segment`（不帶 `--recommended`）寫 `confirmed`;`--recommended` 寫 `ai_recommended`（`set_route_segment.rs:103-106`）。
- **張力**:一天若想「derive 自動產生活動間 legs」**加上**「手動加的 airport→hotel leg」→ 做不到:
  - 手動 leg 標 confirmed → 整天被 skip → derive 不產生活動 legs。
  - 手動 leg 標 ai_recommended → 下次 derive 把它當 stale 清掉。
- 這次演練 Day1/5 就卡這個:反覆手動補 legs → re-derive → 被清 → 再補（3-4 次）。逃生口是「derive 先、手動最後、且手動別再 derive」,但很脆。
- **可能修法（待 Codex 判）:** (a) derive 只清「它自己上次產生的」（用不同 source tag 如 `derived` 區分 derive-產生 vs 手動-ai_recommended）→ 手動 ai_recommended leg 能 survive;(b) 或 per-segment「pin」旗標;(c) 或改成 per-segment merge（有 station pair 的活動 leg 更新,手動 leg 保留）。YAGNI 評估:這只在「同一天既要 derive 又要手動 leg」時痛,arrival/departure 天最常見。

### P2 [LOW · agent-first 文件缺口] confirmed 逃生口 + derive 會清 ai_recommended 完全沒提示
- derive-routes 印 `replaced N stale ai_recommended` / `cleared N stale`（`derive_routes.rs:133,138`）+ skip-confirmed（:77）,但**沒任何地方**告訴使用者:
  - 手動 `set-route-segment`（不帶 --recommended）標 confirmed → survive derive;
  - `set-route-segment --recommended` 的 leg **會被下次 derive 清掉**。
- agent（我）踩了 3-4 次才自己推出來。純文件/hint 改進:derive 完成訊息 或 set-route-segment help 提一句。與 P1 相關但獨立（P2 純文件,P1 是行為）。
- 第一手 corroborate:`set_route_segment.rs` 的 help/hint 無此說明;`derive_routes.rs` 完成訊息無此警告。

## 已排除（像 F1/F5,不是缺陷,是我操作）

### O1-EXCLUDED derive-routes 沒 --plan-id 印 plan 清單
- 原記:「易漏 --plan-id → no-op」。**排除**:derive-routes 有 `wants_help` + `resolve_plan_id`（多 upcoming plan 時 fail-loud 印清單,`main.rs` derive arm）。這是正確的 plan-ambiguity fail-loud,不是缺陷。我當時漏帶 --plan-id 是操作問題。（同 F1 教訓:別把自己的調用漏當 CLI bug。）

## 已修上線（主線）
- G1（`56cff35`）— 4 hint 去 hardcode。
- G2（`d3f0116`）— content-depth ZH gate。

---

## 最終判定（Codex 判 + Claude corroborate 完成 2026-07-12）

**P1 → 不修（YAGNI）。降級 MED→LOW。** Codex 修正了 finding 的過度陳述,Claude corroborate:
- 「無法共存」不完全真 —— 正確流程「derive 先 → 手動加 confirmed（預設,別 --recommended）→ 兩者都存都渲染」可行,只是那天之後凍結不能再 auto-re-derive。演練丟失**主因是我標了 --recommended**（我操作）,不是存不下。
- 所有修法選項都有實質代價（Codex 列 + Claude corroborate `schema.sql:48` CHECK 約束確認）:(a) 新 source='derived' 撞 `CHECK(source IN ('confirmed','ai_recommended'))` → 需 table-rebuild migration + 不解 PK collision（day+sort_order）+ 漣漪 4 處（validate.rs:1368 pending-AI count、route_segments.rs:134/167、dashboard render/mod.rs:60 AI badge）;(b) pin flag 太重;(c) station-pair merge 無法區分 manual vs obsolete。最小正解是獨立 `generator` 欄,但仍需 ordering policy + legacy migration,非小 patch。
- **結論:逃生口（confirmed）+「derive 先、手動 confirmed 最後」夠用,等有真證據（頻繁的 post-manual 活動編輯）再說。**

**P2 → 修（低成本、值得,commit `<pending>`）。** Codex 修正:非「documented nowhere」（`derive-routes --help` 已提 skip confirmed,main.rs:751,Claude corroborate ✓）。真正缺的是**破壞性關係**:derive 擁有並取代每個 ai_recommended leg。修法:`set-route-segment` 單+bulk 成功訊息各加條件 hint —— 帶 --recommended → 「下次 derive 取代」;不帶（confirmed）→ 「未來 derive skip 這天」。放在 provenance 被選的地方最精準;**不**在 derive 每次完成加行（已有 per-day skip/replace 輸出）。

**O1 → 排除**（我漏帶 --plan-id,非缺陷）。

**pipeline 收穫:** Codex 又幫我看清一個 finding 的過度陳述（P1「無法共存」→ 其實只是「加 --recommended 才丟」+「加了 confirmed 就凍結」),避免了為一個 YAGNI 的設計張力動 schema。P2 修一個真的文件缺口。
