# CLI + 流程改進機會 — 2026-07-07 掃描

**方法:** 2 個唯讀 explore agent(流程摩擦 / 一致性缺口)並行掃 ~60 個命令模組,
**每一項發現都由 Claude 對源碼親自驗證**(cite file:line),不照單全收。緣起:enrich
tokyo-sep-2026 時親手撞到路線/交通的反覆手動摩擦。全部 CONFIRMED against source。

已排除的三個先前修過的類別(不重報):resolver-flag 統一、set-route-segment 未知 flag 拒絕、7 命令 `✅`。

---

## A 類 — 流程摩擦(交通/路線 cascade;源自這次真實痛點)

### A1 [最高] 交通參考資料 hardcode 在 Rust,且無 CLI 可寫 — 踩兩條核心原則
- **源碼:** `db_seed_destination_refs.rs:70` `transit: &'static [Transit]` + `:225` 實際資料 = 編譯進 binary。
  唯一寫 `destination_transit` 的路徑是這個 seed;main.rs 無 `set-transit`/`add-transit` dispatch(已驗)。
- **踩到:** CLAUDE.md「no hardcode」+「Turso sole source of truth, no local data」。交通參考資料本應在
  Turso 可 CLI 增修,卻硬編在 Rust,只能重編譯/重 seed 才能改。
- **這次痛點:** enrich 時被迫 `db exec INSERT OR REPLACE INTO destination_transit` 5 次(~15 站對),
  untracked、無 pair_key 正規化(易 key 錯導致下輪 derive 仍 miss)。
- **修法:** `travel add-transit --dest <slug> --from <st> --to <st> --minutes N [--line ..] [--kind ..]`,
  用 derive_routes 同款 `norm_station` + `{a}_to_{b}` 正規化(保證下輪 lookup 命中),INSERT OR REPLACE,
  `--dest` 從 active_destination 預設,slug-keyed 無 audit(同 set-poi-coords)。

### A2 [高] derive-routes 對「寫了 leg 但 metadata=NULL」半沉默 — 無 backfill worklist
- **源碼:** `derive_routes.rs:446-449` lookup 回 (None,None) 時仍寫 leg(duration_min=NULL),不印任何提示;
  下游 `compare_content_depth.rs:59` 只數 `duration_min>0` → 靜默排除 → 幾步後才以 SHORT 現形,無指回哪些站對。
- **修法:** derive-routes 結束時對每個 NULL leg 印 `⚠ Day N: <from>→<to> 缺交通資料(補:travel add-transit ...)`,
  並把 missing 計數加進 totals。純文字非阻斷。與 A1 接成順手迴圈。

### A3 [高] 研究出的分鐘數不寫回參考表 → 知識對下個 plan 丟失(Agent A 挖出,已驗)
- **源碼:** `set_route_segment.rs:10` 走 `repo::route_segments` 寫 `day_route_segments`(單一 plan/day),
  **不寫** `destination_transit`(已驗)。step-6 workaround 研究的交通時間對下個 plan / `--force` 重 scaffold 丟失。
- **修法:** 由 A1 解決 —— 研究時用 add-transit 寫回參考表,知識黏住。或給 set-route-segment 一個
  `--also-transit` 旁寫 destination_transit。

### A4 [中] add-activity/populate 後不自動 cascade routes(涉設計權衡)
- **源碼:** `populate_itinerary.rs`/`add_activity.rs` 不呼叫 derive-routes(已驗)。agent 每次改活動得手動記得跑。
- **權衡:** derive-routes 刻意獨立+冪等(Stage-3 skill 設計)。傾向不自動 cascade,而是成功後印一行
  「run derive-routes --day N to cascade transit」提示,保留顯式性。

---

## B 類 — 一致性 / 正確性缺口(Agent B,全部已驗)

### B1 [REAL BUG] mark-booked 的 --dry-run typo → 真的寫入(最高影響)
- **源碼:** `mark_booked.rs:44` `args.iter().any(|a| a == "--dry-run")`;`:69` `if !dry_run` 才寫。
  打成 `--dry-runn` → dry_run=false → **執行真實 booking transition**,agent 以為是預覽。無 reject。
- **修法:** mark-booked/sync-bookings(option_value 掃描型)加 reject_unknown_flags 前置。

### B2 [REAL BUG] set-ota-coverage --proven typo → 靜默寫 proven=0(provenance)
- **源碼:** `set_ota_catalog.rs:63` 同款 any(=="--proven");`:24` filter(!starts_with("--")) 丟未知 flag,無 reject。
  typo `--provven` → proven=0,catalog reader 誤判「已驗 OTA」為未驗。是 audited mutation,壞行durably committed。
- **影響命令:** set-ota-source/coverage/region/workflow/url-param(整個 set_ota_catalog.rs)。
- **修法:** hoist set_route_segment.rs:298 的 reject_unknown_flags 到共用模組,每個 run_set_* 頂部呼叫。

### B3 [REAL BUG] set-plan-name / set-active-destination 缺 plan_events(audit 三元組不完整)
- **源碼:** `set_active_destination.rs` 只 record_operation(version+operation_runs),無 insert_event(已驗)。
  set_plan_name 同。切換 active_destination 不留 timeline 事件 → 重新詮釋後續所有 dest-scoped 事件,replay 看不到。
- **佐證:** `create_plan.rs:87` 已寫 timeline-scope `plan_created` 事件,證明 lifecycle 事件可掛 timeline。
- **修法:** 補 timeline-scope `active_destination_changed`/`plan_renamed` 事件(仿 create_plan),或加 mark_plan_deleted 式的免除註解。前者較正確。

### B4 [REAL BUG 群] 多個 mutation parser 有 `_ => {}` catch-all 吞未知 flag
- **源碼(已驗樣本):** promote_offers.rs:113、import_offers.rs:115、weather.rs:89、add_offer.rs:95、
  add_besttour/lifetour_offer、import_tour_group_offers.rs:46、mark_booked(B1)、sync_bookings。
- **排序:** mark_booked(--dry-run 繞過)> promote/import_offers(plan provenance)> add-*/catalog(attribution)。
- **修法:** 加 `other if other.starts_with("--") => return Err(...)` arm(flow_decision.rs:150/set_flight.rs:190/
  derive_routes.rs:257 已有此 pattern)。

### B5 [系統性 caveat] resolve_active_destination 盲信 --dest override,不驗 slug 存在
- **源碼:** `cascade/common.rs:38-40` `if let Some(d)=dest_override { return Ok(d.to_string()); }`(已驗)。
  `set-flight --dest bogus_slug` → 寫孤兒資料 + 版本 bump。是寫入側的 assert_dest_matches 缺口。
- **修法:** override 時對照 `plan_lifecycle::list_destination_slugs`(plan_lifecycle.rs:24,已存在且 set_plan_name 已用)驗證存在。因統一套用,排在 per-command 之後。

### B6 [COSMETIC] 成功行 glyph 不一致
- flow_decision:94、import_offers:351、promote_offers:298/198、sync_bookings:100、weather:150、
  add_besttour_offer:166(其 sibling add_lifetour_offer:170 有 ✅ — 直接 sibling 不一致)。都有確認行,只缺 ✅。低優先。

### B 排除的(採信+快驗,不列 finding)
- --help guard:每個 mutation 都有(inline 或 main.rs wants_help)。
- --dest 驗證模式:view 用 assert_dest_matches,mutation 用 resolve_active_destination 當 selector,一致(除 B5 caveat)。
- 衍生表跳過 plan triad:sync_bookings/set_poi_coords/snapshot_maps/mark_plan_deleted/set_ota_catalog/db_sync_events 皆有各自 audit 或有註解,正確。
- ota/ 子系統:獨立 queue pipeline,dispatcher 拒絕未知 subcommand,非 plan-audit 範疇。

---

## 建議修復順序

**第一批(REAL BUG,靜默資料/provenance 風險):**
1. B1 mark-booked --dry-run 繞過(最高:安全預覽變不可逆寫入)
2. B2 set-ota-coverage --proven 靜默 proven=0
3. B4 群(promote/import_offers 等 catch-all)+ B1/B2 一起用共用 reject_unknown_flags 修掉

**第二批(原則違反 + 這次痛點,最有槓桿):**
4. A1 add-transit 命令(修 hardcode + no-CLI-write 原則違反)→ 直接解 A3
5. A2 derive-routes 報告缺 metadata(接 A1 成迴圈)

**第三批(audit 完整性 + 品質):**
6. B3 set-active-destination/set-plan-name 補 plan_events
7. B5 resolve_active_destination 驗 slug 存在
8. A4 add-activity/populate 提示 derive-routes(提示行,非自動 cascade)
9. B6 ✅ glyph 一致(cosmetic)
