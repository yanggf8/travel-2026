# kyoto-oct 真實演練新發現 (2026-07-11)

## G1 [LOW-MED · agent-first/no-hardcode nit] route/transit hint 把沖繩地名寫死當範例
獨立 grep 後確認是 **4 處**(findings 初稿只記 2 處,漏 2):
- set_tod.rs:285 — `Hint: ... e.g. "安里駅 → 赤嶺駅 → iias 沖縄豊崎" — no （…）notes...`
- set_route_segment.rs:60 — `Hint: use a clean place name (e.g. "赤嶺駅", "iias 沖縄豊崎")...`
- validate_itinerary.rs:360 — `Use a clean place name as the stop (e.g. "赤嶺駅", "iias 沖縄豊崎")...`
- validate_itinerary.rs:411 — `Add a 駅/站/Station suffix or a city (e.g. "安里駅 那覇").`
- 我在編 KYOTO plan,提示卻教我用 OKINAWA 地名 → agent/用戶困惑「為何舉沖繩例子」。
- 不是嚴重 bug（提示照樣傳達「stop 要乾淨」意思，是說明文字非資料/邏輯），但範例寫死特定 dest 違反 no-hardcode 精神 + 提示與當前 dest 不符。
- 修法方向:範例改成通用/佔位（e.g. "A駅 → B駅 → C商場"）或不舉具體地名，只講規則。純字串。
- Claude 第一手撞到（set-tod-zh --transit-zh 觸發 285 那句）。
- 待 Codex 核 + Claude corroborate。

## G2 [MED · oracle 設計張力] content-depth ZH 分母含真空 session,懲罰誠實的短行程
- ZH coverage = filled_zh / (all days + all sessions)。分母是「所有 day+session」,不管該 session 該不該有內容。
- kyoto-oct 樂桃下午到、Day5 早上走 → Day1 上午/中午/下午 + Day5 下午/晚上 = 5 個真空 session(無活動)。
  填了會是造假(no-cheat)。所以 ZH 上限 = 20/25 = 80%,結構性到不了 okinawa 的 88%。
- okinawa 88% 是它 arrival/departure 剛好較滿(session 都有內容)。
- 這不是 plan 做得差,是 oracle 把「合理空 session」也算進分母 → 誠實的短行程在 ZH 軸吃虧。
- 修法方向(待議): (a) ZH 分母只算「有活動的 session + 所有 day」(空 session 不列入)。
  (b) 或 ZH 改看「有內容的 session 裡 ZH 覆蓋率」。(c) 或維持,但 verdict 對 ZH 用「有內容處的覆蓋」而非全域。
- Claude 第一手撞到(kyoto activities 31>29 / meals 12>10 / routes 22>21 全贏,只 ZH 因真空 session 卡 80%)。
- 待 Codex 核 + Claude corroborate。與 G1 一起進 findings pipeline。

## 演練流程觀察 (非缺陷,但值得記)
- O1: derive-routes 沒帶 --plan-id → 印 plan 清單並 no-op(多 upcoming plan)。正確 fail-loud,但易漏 → 以為 re-derived 其實沒。oracle 前確認 re-derive 真的帶了 --plan-id。
- O2: derive-routes 會清掉「沒有 geocoded 活動支撐」的手動 set-route-segment ai_recommended legs(Day5 全 add-activity 非 geocoded → 手動 legs 被 deleted)。所以流程是「derive 先、手動 legs 最後補」,否則反覆被清。這是真實的 derive-vs-manual 互動摩擦。
