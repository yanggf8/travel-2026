# 真實抓取 drill 的 CLI/流程改善發現（2026-07-10）

背景：跑了一次**真實資料** drill（osaka-aug-2026，日期刻意選近 2026-08-05→08-09）。用 gwebcdb 抓
真 google_flights 航班 + 真 agoda 飯店 → agent 萃取 → `ota write-offers` → 真 POI + gwebcdb-Google-Maps
查證的真餐廳。跟過去合成 `[DRILL]` drill 不同，這次撞到只有真實資料才暴露的摩擦。

Explore agent 系統掃過（帶 file:line），Claude 第一手 corroborate 了 #1 與 #3。以下待 Codex 核查
+ Claude 再 corroborate Codex，才進修復。**這是發現清單，不是修復計畫。**

---

## #1 [HIGH · 真資料專屬] 真實航班進不了 plan — 資料模型假設「完整去回」，真實來源只給「去程+總價」

- google_flights（以及多數 OTA 航班頁）顯示的是「去程航班 + 來回總價」，**不配對特定回程時刻**（選了去程才展開回程）。所以真實抓來的 flight offer **天生只有 `flight_outbound`，`flight_return` 為空**。
- `promote_offers.rs:407` — `match (&o.flight_outbound, &o.flight_return)` **只有 `(Some, Some)` 才寫 legs**，`(Some, None)`（只有去程）落入 `_ => None`。→ 真航班 offer promote 後**零 flight legs**。
- `cascade/select_offer.rs:94-96,197` — `has_flight()` = `!legs.is_empty()`；零 legs → P3 永遠不 populated，甚至印「Offer has no flight — nothing to populate」，把真航班當成無航班。
- `plan_offers.rs:40-42` — `PlanOfferFlightWrite` 只有 `{ direction, flight_number }`，**無時刻/機場代碼**（對比 `ImportFlightLeg` 有完整 leg data）。就算湊齊去回，抓到的真時刻（TPE 18:20→KIX 22:00）也在 promote 時全丟。
- 逃生口：`set-flight outbound`（set_flight.rs:60-68,260-261）**能**單獨處理去程 —— 但它跟 scraped offer 脫節，operator 得手抄真航班進 set-flight，等於航班的 scrape-verify 白做。
- **Claude 已第一手證實**（promote_offers.rs:407 `_ => None` + plan_offers.rs:40 只有 flight_number）。
- 影響：對航班而言，「抓真的 → 驗證 → 進 plan」這條鏈在 promote 這步斷掉。**最該修。**

## #3 [MED · 真資料專屬] 餐廳是整條管線唯一無查證的一環

- 航班/飯店/POI 都有 capture→extract→write 溯源；餐廳純 free-text `set-meals`（`set_tod.rs:418` run_meals + parse_meals 491-552），只收 `--meal "<text>"` + 選配 `｜map:<query>` pin + `--recommended` flag。
- **零查證**：無 rating 欄、無 address/place-id、無「這是不是真地方」檢查；連 `｜map:` query 是否解析到真地點都不查。
- 唯一「驗證」是 advisory lint（純字串形狀）：validate_itinerary.rs:437-450「無 map pin」Info、463-480「可能需訂位」Info。都只檢查 pin 標記存在 / 訂位登錄，從不查 pin 是否真地方或評分。
- OTA 管線（ota mod.rs:12-31）product_types 只有 flight/hotel/fit/group_tour，**無 restaurant/meal**。
- **Claude 已第一手證實**（set_tod.rs run_meals 只做 parse+replace，零查證）。
- 影響：restaurant-pick 規則（Google 評分/main+backup/pin）只靠作者自律 + 「有加 pin 嗎」的 nudge。

## #2 [MED · 真資料專屬] query-offers 無法依 capture/job 過濾 → 全域 offers 混雜

- offers 表有 provenance 欄：capture_id/produced_by_job_id/produced_by_attempt_id/scraped_at（schema.sql:521, write_offers.rs:283）。
- `db_query_offers.rs:20-33,49-81` — QueryOffersArgs 只有 dest/region/start/end/sources/type/max-price/fresh-hours/max/include-undated/sql，**無 --capture-id/--job-id/--attempt-id**（unknown flag 被拒）。DAL OfferFilter（offers.rs:59-199）也無此 predicate；SELECT 甚至不 project 那些欄 → 輸出也看不到。
- 部分緩解：`--fresh-hours N` 依 recency，但 recency ≠「這次 capture」；promote 依 MAX(scraped_at) per id dedup，但不同 job 的不同 id 仍累積、query 無法區分。

## #4 [MED] enqueue product_type vs TSV type 碰撞 — 危險的 flight↔hotel 有擋，fit↔group_tour 靜默混

- 有 guard：`write_offers.rs:242-251` 比 `offer_row_kind(job.product_type)` vs 每列 TSV type，不符報錯。測試 ota_write_offers.rs:399-444 證 package-TSV-under-flight-job 被拒。**最怕的 flight↔hotel 混寫被擋住。**
- 洞：`offer_row_kind`（common.rs:83-89）只 map flight→flight、hotel→hotel，**其餘全→package**。product_types 有 fit/group_tour，TSV type 只收 package|flight|hotel。→ fit job 與 group_tour job 都塌成 package，可互相混寫無錯，且都存成 type='package'（schema CHECK 只允許 package/flight/hotel → 區分在儲存層被抹掉）。
- 且兩個 type 概念從沒在 help 一起解釋（enqueue.rs:20-25 只講 <product_type>；write-offers.rs:192-195 沒提相容性）。

## #5a [MED · 真資料專屬] 無 CLI 看 capture raw_text — agent 是 parser 卻沒指令讀它要 parse 的東西

- `captures.rs:13` 只有 `get(capture_id)`（無 list）；ota::dispatch（mod.rs:13-31）enqueue/claim/heartbeat/finish/reap-stale/write-offers/observations/run 沒一個印 raw_text。`ota run --capture-only` 印 capture_id 但不印 raw_text。
- 「agent IS the parser」（mod.rs:8-11）必須讀 raw_text 才能產 TSV，唯一路徑是 `db exec "SELECT raw_text"`（我這次就這樣）。ota parse 退休訊息還叫你「read the capture's raw_text」卻不給指令。

## #5b [LOW] scrape 流程無 next-step hint 鏈

- itinerary 流程有鏈式提示（select_offer.rs:142 印「Next: scaffold-itinerary」）；scrape 流程沒有：
  - ota run --capture-only（run.rs:426-434）印 ids 但無「Next: 讀 raw_text 然後 write-offers」。
  - write_offers.rs:309-317 印 inserted/deduped 但無「Next: promote-offers」。
  - promote_offers.rs:305 印「Saved」但無「Next: select-offer」。

## #5c [LOW · 真資料專屬] write-offers 無 under-extraction 警告

- write_offers.rs:309-317 報 candidates/inserted/deduped/status；但 candidate_count 只是 parsed.len()（agent 選擇 emit 的列數），CLI 對頁面真實 offer 數無感。頁面 40 筆只萃 3 筆、甚至 header-only 0 筆，都印 succeeded（parse_tsv 只對 malformed 報錯，不對 0 列；common.rs:263-274 對 0 列 finish succeeded inserted=0）。「萃取成功」與「萃取幾乎沒抓到/被當完成」看起來一樣。

---

## 建議排序（待核查後決定）
1. **#1** — 讓真實航班 offer（只有去程）能進 P3 + 存時刻。最高價值，航班 scrape-verify 否則白做。
2. **#3** — 餐廳查證不對稱；至少讓 set-meals 記 rating/place 或加 verify affordance。
3. **#5a + #5c** — 小但實用：`ota show-capture` 讀 raw_text + write-offers 對 0-candidate/全-dedup 出警告。
4. #2/#4/#5b — 一致性/可用性，次要。

（待 Codex 核查 → Claude corroborate Codex → 再進多-AI pipeline 修。）
