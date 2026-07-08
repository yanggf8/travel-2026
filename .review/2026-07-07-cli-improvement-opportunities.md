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

**✅ 第一批 — DONE (2026-07-07, commits ff6c4cf→d02261e, pushed):** reject-unknown-flags 全批。
helper hoist 到 plan_resolver.rs(pub(crate)+4 unit test)→ mark-booked(dispatch-arm preflight,
connect-before-parse)→ sync-bookings/fetch-weather(同)→ set-ota-catalog ×5 subcommand(每個各自
flag 清單)→ promote/import-offers(`_ => {}`→reject arm)→ tour-offers ×4。全部 TDD RED→GREEN,
15+ reject 測試綠燈,set-ota 10/10(含 valid round-trip 證明分類正確),live smoke 四類 bug 皆 fail-loud +
valid --dry-run 不被誤拒。Codex 設計 + Claude 逐項 corroborate(connect-before-parse 洞見、flag value/bool
分類、set-ota-region/url-param 無 flag)。
  ~~1. B1 mark-booked~~ ~~2. B2 set-ota-coverage~~ ~~3. B4 群~~ — 全部完成。

**✅ 第二批(A1)— DONE (2026-07-07, add-transit feature, pushed):** `travel add-transit <slug>
<from> <to> --minutes N [--line/--kind/--source/--confidence]` — slug-keyed reference-data 命令(仿
set-poi-coords,無 audit),寫 destination_transit,冪等 INSERT OR REPLACE。核心正確性:新 transit_key.rs
共用模組(norm_station/primary_pair_key/lookup_candidates),add-transit 的寫 key == derive-routes 的
lookup key(derive_routes 抽取後 behavior-lock 綠燈)。END-TO-END LOCK 證明:add-transit 一個奇怪大小寫/
空白的站對 → derive-routes → leg 拿到 duration_min=12。修掉 destination_transit hardcode 在 Rust 的
no-hardcode/Turso-only 違反 + 直接解 A3(研究的分鐘數現在可透過 add-transit 寫回參考表,對下個 plan 黏住)。
9 unit + 1 e2e + live smoke。
  ~~4. A1 add-transit~~ — 完成。A3 一併解決(add-transit 就是寫回參考表的路徑)。
**✅ A2 — DONE (2026-07-08, commit 6e48b8f):** derive-routes 現在收集所有 metadata-less 站對(跨所有天,
含 days_written=0 的 unchanged run)並印出 copy-paste worklist:`⚠ N 站對缺 destination_transit metadata`
+ 每對一行現成的 `add-transit` 命令 + 提示 re-run。閉合 A1+A2 迴圈:derive → 看缺什麼 → add-transit →
re-derive。RED→GREEN lock + live-verified(kyoto-confirm 印出全 6 個缺的站對)。Stage-3 skill 也記錄此迴圈。
  ~~5. A2~~ — 完成。

**第三批(audit 完整性 + 品質):**
- **✅ B3 — DONE (2026-07-08) — 結論修正:documented 而非強塞 event。** Codex/agent 建議「補
  plan_events」有 data-loss 陷阱:plan_events 是 process-keyed 的 current-state projection(PK=
  plan_id/scope/destination/process_id/sort_order),非 append log;insert_event 會先 DELETE 同 PK。
  set-active-destination 若寫 (timeline,"","",0) 事件會 **CLOBBER create_plan 的 plan_created 事件**。
  正確修法(仿 mark_plan_deleted):這是 metadata/lifecycle 變更、無 process 可掛 → 不發 plan_events,
  只保 record_operation(version+operation_runs)。兩個命令加了說明註解(no 行為變更)。
- **✅ B5 — DONE (2026-07-08).** resolve_active_destination 現在驗 --dest override 是該 plan 的真實
  destination(對照 list_destination_slugs / plan_destinations,fail-loud)—— `set-flight --dest bogus`
  不再寫孤兒資料 + bump version,改報「is not a destination of plan (known: ...)」。連帶把 seed_plan
  補上 plan_destinations 行(對齊 production;所有 live plan 都有),+ 修一個 display_name-assert 測試用
  OR REPLACE。RED→GREEN + broad regression sweep(11 binaries / 33 test 綠)+ live-verified。
- A4 add-activity/populate 提示 derive-routes(提示行,非自動 cascade)— 仍待做(低優先)
- B6 ✅ glyph 一致(cosmetic)— 仍待做(最低優先)
