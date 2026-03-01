#!/usr/bin/env ts-node
/**
 * Populate zh session content for kyoto-2026 using parameterized queries.
 * Using parameterized args instead of inline SQL strings to avoid encoding issues.
 */
import * as fs from 'fs';
import * as path from 'path';

const envFile = path.join(__dirname, '..', '.env');
for (const line of fs.readFileSync(envFile, 'utf8').split('\n')) {
  const m = line.match(/^([^#=]+)=(.*)$/);
  if (m) process.env[m[1].trim()] = m[2].trim();
}

const TURSO_URL = process.env.TURSO_URL!.replace('libsql://', 'https://');
const TURSO_TOKEN = process.env.TURSO_TOKEN!;
const PID = 'kyoto-2026';
const DEST = 'kyoto_2026';

interface SessionContent {
  day: number;
  session: string;
  focus_zh: string;
  transit_notes_zh: string;
  activities_zh: string[];
}

const sessions: SessionContent[] = [
  // ── DAY 1 ──────────────────────────────────────────────────
  {
    day: 1, session: 'morning',
    focus_zh: '✈️ TPE 09:00 → KIX 12:30',
    transit_notes_zh: '泰國獅子航空 SL396',
    activities_zh: ['✈️ SL396 TPE T1 09:00 起飛 → KIX T1 12:30 抵達（泰國獅子航空）'],
  },
  {
    day: 1, session: 'afternoon',
    focus_zh: '入境 KIX・轉乘至京都',
    transit_notes_zh: 'JR Haruka 特急 KIX→京都駅（~75分，含套裝）',
    activities_zh: ['KIX 入境・領行李', 'JR Haruka 特急至京都駅（~75分）', '入住 APA Hotel 京都站前'],
  },
  {
    day: 1, session: 'evening',
    focus_zh: '探索京都駅周邊',
    transit_notes_zh: '從飯店步行（~3分）',
    activities_zh: ['逛逛京都駅大樓', '晚餐：京都拉麵小路（10F）或 Porta 地下美食街'],
  },
  // ── DAY 2 ──────────────────────────────────────────────────
  {
    day: 2, session: 'morning',
    focus_zh: '金閣寺・可選龍安寺',
    transit_notes_zh: '地鐵烏丸線 京都→北大路，轉101/102公車至金閣寺道（約45分）',
    activities_zh: ['金閣寺（鹿苑寺）— 黃金閣，早晨光線最美', '可選：龍安寺 — 禪意枯山水，距金閣寺步行15分'],
  },
  {
    day: 2, session: 'afternoon',
    focus_zh: '錦市場・四條購物',
    transit_notes_zh: '從金閣寺道搭公車至四條河原町，或地鐵北大路→四條',
    activities_zh: ['錦市場 — 京都廚房，街頭小食與伴手禮', '四條河原町商圈逛街', '寺町・新京極室內商店街'],
  },
  {
    day: 2, session: 'evening',
    focus_zh: '先斗町晚餐',
    transit_notes_zh: '從四條步行先斗町，地鐵回京都駅',
    activities_zh: ['先斗町小巷漫步', '晚餐：先斗町或四條一帶'],
  },
  // ── DAY 3 ──────────────────────────────────────────────────
  {
    day: 3, session: 'morning',
    focus_zh: '保津川遊船・從亀岡出發',
    transit_notes_zh: 'JR 山陰線 京都→亀岡（~25分）',
    activities_zh: ['JR 山陰線至亀岡（~25分）', '保津川下り（保津川遊船，~2小時）11:30 開船'],
  },
  {
    day: 3, session: 'afternoon',
    focus_zh: '嵐山觀光',
    transit_notes_zh: '遊船抵達嵐山',
    activities_zh: ['遊船抵達嵐山公園', '竹林の小径 — 嵐山竹林步道', '天龍寺（UNESCO 世界遺產）— 日式名勝庭園'],
  },
  {
    day: 3, session: 'evening',
    focus_zh: '渡月橋夕陽・返回京都',
    transit_notes_zh: 'JR 嵯峨野線 嵯峨嵐山→京都駅',
    activities_zh: ['渡月橋夕陽美景', '嵐山キモノフォレスト（和服森林彩燈）', '返回京都駅'],
  },
  // ── DAY 4 ──────────────────────────────────────────────────
  {
    day: 4, session: 'morning',
    focus_zh: '睡個懶覺・輕鬆早晨',
    transit_notes_zh: '飯店步行範圍',
    activities_zh: ['睡個懶覺（昨天嵐山是充實的一天）', '可選：東寺（距飯店步行15分）', '11:00 京都駅周邊午餐'],
  },
  {
    day: 4, session: 'afternoon',
    focus_zh: '和服體驗・東山散步',
    transit_notes_zh: '地鐵或計程車至夢館（五條），步行東山區',
    activities_zh: [
      '京都夢館和服體驗',
      '13:00 抵達夢館 — 試穿＋髮型（~1小時）',
      '~14:00 計程車至二年坂（~5分，¥800）',
      '三年坂 — 石板老街，和服拍照最佳地點',
      '二年坂（スターバックス京都二寧坂）',
      '八坂之塔 — 東山地標',
      '~15:30 計程車回夢館，換回衣服',
    ],
  },
  {
    day: 4, session: 'evening',
    focus_zh: '祇園夜遊・採購伴手禮',
    transit_notes_zh: '搭公車或計程車從祇園回京都駅',
    activities_zh: ['聖護院八ッ橋 — 買生八橋（出發前一天最新鮮）', '祇園漫步 — 花見小路', '晚餐：祇園一帶'],
  },
  // ── DAY 5 ──────────────────────────────────────────────────
  {
    day: 5, session: 'morning',
    focus_zh: '退房・最後採購',
    transit_notes_zh: '從飯店步行（~3分）至京都駅',
    activities_zh: ['APA Hotel 打包退房（09:00 前）', '京都駅伊勢丹＆Porta 地下街最後採購'],
  },
  {
    day: 5, session: 'afternoon',
    focus_zh: '搭 Haruka 至關西機場',
    transit_notes_zh: 'JR Haruka 特急 京都→關西國際機場（~75分）',
    activities_zh: ['JR Haruka 特急 京都 09:30 → KIX（~75分）', '抵達 KIX T1，辦理登機手續（10:30 開放）'],
  },
  {
    day: 5, session: 'evening',
    focus_zh: '✈️ 返回台灣',
    transit_notes_zh: '泰國獅子航空 SL397',
    activities_zh: ['✈️ KIX T1 13:30 起飛 → TPE T1 15:40 抵達（泰國獅子航空 SL397）'],
  },
];

async function run() {
  const pipelineUrl = `${TURSO_URL}/v2/pipeline`;

  // Build one pipeline request for all 15 sessions
  const requests: Array<{type: string; stmt?: {sql: string; args: Array<{type: string; value: string}>}}> = [];

  for (const s of sessions) {
    requests.push({
      type: 'execute',
      stmt: {
        sql: `UPDATE timesofday SET focus_zh=?, transit_notes_zh=?, activities_zh_json=? WHERE plan_id=? AND destination=? AND day_number=? AND session_type=?`,
        args: [
          { type: 'text', value: s.focus_zh },
          { type: 'text', value: s.transit_notes_zh },
          { type: 'text', value: JSON.stringify(s.activities_zh) },
          { type: 'text', value: PID },
          { type: 'text', value: DEST },
          { type: 'integer', value: String(s.day) },
          { type: 'text', value: s.session },
        ],
      },
    });
  }
  requests.push({ type: 'close' });

  const res = await fetch(pipelineUrl, {
    method: 'POST',
    headers: { Authorization: `Bearer ${TURSO_TOKEN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ requests }),
  });

  if (!res.ok) throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  const json = await res.json() as { results: Array<{ type: string; response?: { type: string; result?: { affected_row_count?: number }; error?: unknown } }> };

  let ok = 0; let failed = 0;
  for (let i = 0; i < sessions.length; i++) {
    const entry = json.results?.[i];
    const result = entry?.response?.result;
    const affected = result?.affected_row_count ?? 0;
    if (entry?.response?.error || affected === 0) {
      const s = sessions[i];
      console.error(`❌ D${s.day} ${s.session} — ${entry?.response?.error ? JSON.stringify(entry.response.error) : 'rows_affected=0'}`);
      failed++;
    } else {
      ok++;
    }
  }
  console.log(`\n✅ ${ok}/${sessions.length} session ZH content updates applied (${failed} failed)`);
}

run().catch(e => { console.error(e); process.exit(1); });
