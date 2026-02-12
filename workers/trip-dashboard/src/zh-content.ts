/**
 * Chinese (Traditional / Taiwan) content overrides for trips.
 * Keyed by day_number → session → field.
 * Tokyo: data/tokyo-trip-plan-zh.md
 * Kyoto: inline translations
 */

interface SessionZh {
  focus: string;
  activities: string[];
  meals: string[];
  transit_notes: string;
}

interface DayZh {
  theme: string;
  morning: SessionZh;
  afternoon: SessionZh;
  evening: SessionZh;
}

export const ZH_DAYS: Record<number, DayZh> = {
  1: {
    theme: '抵達 + 休息 + 濱松町探索',
    morning: {
      focus: '從桃園出發',
      activities: [
        '桃園機場',
        'TR874 桃園 13:55 → 成田 18:00',
      ],
      meals: ['出發前在機場吃午餐'],
      transit_notes: '18:00 抵達成田',
    },
    afternoon: {
      focus: '飛行 & 抵達 & 成田晚餐',
      activities: [
        '飛行時間：4小時05分',
        '18:00 抵達成田T2',
        '18:45 入境審查＋領行李',
        '18:50-19:30 成田T2 晚餐',
        '19:40 利木津巴士（SuperCabin）出發往竹芝',
      ],
      meals: ['成田T2 晚餐（拉麵、壽司、烏龍麵）'],
      transit_notes: '利木津巴士 19:40 → 竹芝 21:25',
    },
    evening: {
      focus: '飯店 Check-in & 休息',
      activities: [
        '21:25 抵達竹芝站',
        '21:30 步行到 TAVINOS 濱松町（3分）',
        '飯店 check-in',
        '休息',
      ],
      meals: ['已在成田用餐'],
      transit_notes: '竹芝站 → 飯店（步行3分）',
    },
  },
  2: {
    theme: 'teamLab 無界 + 麻布台Hills + 六本木',
    morning: {
      focus: 'teamLab 無界',
      activities: [
        '10:00 出發（前一天到飯店較晚，不用太早起）',
        '10:10 濱松町 → 神谷町（地鐵、10分）',
        'teamLab 無界（麻布台Hills、10:00開館）',
        '預留2-3小時體驗',
      ],
      meals: ['飯店輕食早餐'],
      transit_notes: '大門站 → 神谷町（日比谷線、¥180、10分）。出站步行5分到teamLab。',
    },
    afternoon: {
      focus: '麻布台Hills 探索',
      activities: [
        '探索麻布台Hills',
        '逛花園和商店',
        '麻布台Hills 午餐',
      ],
      meals: ['麻布台Hills 午餐'],
      transit_notes: '步行可達',
    },
    evening: {
      focus: '六本木之夜',
      activities: [
        '步行到六本木Hills（10分）',
        '森美術館或 Tokyo City View（可選）',
        '六本木晚餐',
        '回飯店',
      ],
      meals: ['六本木晚餐（高級餐廳或居酒屋）'],
      transit_notes: '六本木 → 大門（大江戶線、¥220、15分）。步行5分到飯店。',
    },
  },
  3: {
    theme: '澀谷 + 原宿 + 淺草',
    morning: {
      focus: '澀谷早晨',
      activities: [
        '濱松町 → 澀谷（JR山手線、20分）',
        '澀谷十字路口（早上人較少）',
        'Shibuya Sky 展望台（10:00開放）',
        '逛澀谷',
      ],
      meals: ['飯店早餐、澀谷早午餐'],
      transit_notes: 'JR濱松町 → 澀谷（山手線、¥200、20分）',
    },
    afternoon: {
      focus: '原宿 & 表參道',
      activities: [
        '步行到原宿（從澀谷10分）',
        '竹下通',
        '表參道（精品櫥窗逛街）',
        'Cat Street 探索',
      ],
      meals: ['原宿午餐/小吃'],
      transit_notes: '澀谷 → 原宿 步行（10分）',
    },
    evening: {
      focus: '淺草之夜',
      activities: [
        '原宿 → 淺草（地鐵、30分）',
        '淺草寺（傍晚氛圍、人較少）',
        '仲見世商店街',
        '雷門拍照（夜間點燈很美）',
        '淺草晚餐',
        '回飯店',
      ],
      meals: ['淺草晚餐（蕎麥麵、天婦羅、居酒屋）'],
      transit_notes: '淺草 → 大門（都營淺草線、¥280、20分直達）',
    },
  },
  4: {
    theme: 'KOMEHYO + 伴手禮（新宿）',
    morning: {
      focus: 'KOMEHYO 新宿（Chanel 包）',
      activities: [
        '濱松町 → 新宿（JR山手線、25分）',
        'KOMEHYO 新宿店（11:00開門）',
        '逛5層樓、Chanel專區',
      ],
      meals: ['飯店早餐'],
      transit_notes: 'JR濱松町 → 新宿（山手線、¥200、25分）。使用Suica/Pasmo。',
    },
    afternoon: {
      focus: '備案 + 伴手禮',
      activities: [
        '大黑屋新宿（備案、步行5分）',
        '伊勢丹新宿B2 伴手禮（Yoku Moku、Henri Charpentier）',
        '逛新宿',
      ],
      meals: ['伊勢丹餐廳樓層或附近午餐'],
      transit_notes: '步行可達',
    },
    evening: {
      focus: '新宿之夜',
      activities: [
        '思い出横丁晚餐',
        '歌舞伎町散步',
        '回飯店',
      ],
      meals: ['思い出横丁串燒'],
      transit_notes: 'JR新宿 → 濱松町（山手線、¥200、25分）',
    },
  },
  5: {
    theme: '輕鬆 + 回程',
    morning: {
      focus: '自由 / 打包',
      activities: [
        '睡晚一點',
        '打包行李',
        '退房（11:00）',
        '行李寄放飯店或東京站寄物櫃',
      ],
      meals: ['飯店早餐'],
      transit_notes: '',
    },
    afternoon: {
      focus: '飯店附近放鬆 + 午餐',
      activities: [
        '濱松町/汐留附近逛逛',
        '濱離宮恩賜庭園（可選、¥300、步行10分）',
        '汐留 City Center 或 Caretta 汐留午餐',
        '14:45 回飯店取行李',
      ],
      meals: ['濱松町/汐留午餐'],
      transit_notes: '步行可達，不需搭車。',
    },
    evening: {
      focus: '利木津巴士前往機場',
      activities: [
        '15:00 帶行李出發',
        '15:05 步行到竹芝巴士站（3分）',
        '15:30 利木津巴士（SuperCabin）出發',
        '17:30 抵達成田T2',
        '辦理登機、安檢',
        '免稅店購物 + 簡單晚餐',
        '19:55 TR875 成田出發 → 桃園 23:10',
      ],
      meals: ['成田T2 簡單晚餐'],
      transit_notes: '利木津巴士：竹芝 15:30 → 成田 17:30（¥3,200）',
    },
  },
};

/** Ordered landmarks per day for Google Maps route links.
 *  Empty array = no route (arrival/departure days). */
export const ZH_DAY_LANDMARKS: Record<number, string[]> = {
  1: [], // arrival day
  2: ['teamLab Borderless Azabudai Hills', 'Azabudai Hills', 'Roppongi Hills Tokyo'],
  3: ['Shibuya Crossing', 'Shibuya Sky', 'Harajuku Takeshita Street', 'Senso-ji Temple Asakusa'],
  4: ['KOMEHYO Shinjuku', 'Isetan Shinjuku', 'Omoide Yokocho Shinjuku'],
  5: [], // departure day
};

export interface RouteSegment {
  from: string;   // 'hotel' = substitute actual hotel name
  to: string;
  mode: 'transit' | 'walking';
}

/** Per-segment routes for Tokyo (hotel = TAVINOS Hamamatsucho) */
export const ZH_DAY_ROUTES: Record<number, RouteSegment[]> = {
  1: [],
  2: [
    { from: 'hotel', to: 'teamLab Borderless Azabudai Hills', mode: 'transit' },
    { from: 'teamLab Borderless Azabudai Hills', to: 'Azabudai Hills', mode: 'walking' },
    { from: 'Azabudai Hills', to: 'Roppongi Hills Tokyo', mode: 'walking' },
    { from: 'Roppongi Hills Tokyo', to: 'hotel', mode: 'transit' },
  ],
  3: [
    { from: 'hotel', to: 'Shibuya Crossing', mode: 'transit' },
    { from: 'Shibuya Crossing', to: 'Shibuya Sky', mode: 'walking' },
    { from: 'Shibuya Sky', to: 'Harajuku Takeshita Street', mode: 'walking' },
    { from: 'Harajuku Takeshita Street', to: 'Senso-ji Temple Asakusa', mode: 'transit' },
    { from: 'Senso-ji Temple Asakusa', to: 'hotel', mode: 'transit' },
  ],
  4: [
    { from: 'hotel', to: 'KOMEHYO Shinjuku', mode: 'transit' },
    { from: 'KOMEHYO Shinjuku', to: 'Isetan Shinjuku', mode: 'walking' },
    { from: 'Isetan Shinjuku', to: 'Omoide Yokocho Shinjuku', mode: 'walking' },
    { from: 'Omoide Yokocho Shinjuku', to: 'hotel', mode: 'transit' },
  ],
  5: [],
};

/** Hotel name EN → ZH mapping */
export const ZH_HOTELS: Record<string, string> = {
  'TAVINOS HAMAMATSUCHO': 'TAVINOS 濱松町',
};

/** ZH transit cheat sheet */
export const ZH_TRANSIT = {
  hotel_station: 'JR濱松町（步行8分）或 大門站（步行5分）',
  key_lines: [
    'JR山手線（綠色環線）— 新宿、澀谷、原宿、東京站',
    '都營淺草線 — 大門直達淺草',
    '地鐵日比谷線 — 神谷町（teamLab）',
    '都營大江戶線 — 大門直達六本木',
  ],
};

// ============================================================================
// KYOTO ZH CONTENT
// ============================================================================

export const ZH_KYOTO_DAYS: Record<number, DayZh> = {
  1: {
    theme: '抵達 + 伏見稻荷',
    morning: {
      focus: '從桃園出發',
      activities: [
        '泰獅航 TPE 09:00 → KIX 12:30',
      ],
      meals: ['機上或出發前用餐'],
      transit_notes: '泰獅航 Thai Lion Air',
    },
    afternoon: {
      focus: '入境 + 前往京都',
      activities: [
        '關西機場入境、領行李',
        'JR Haruka特急 → 京都車站（約75分）',
        'APA Hotel 京都站前 check-in',
      ],
      meals: [],
      transit_notes: 'Haruka特急 關西機場 → 京都車站',
    },
    evening: {
      focus: '京都車站周邊探索',
      activities: [
        '京都車站大樓探索',
        '京都車站 Porta 地下街 或 拉麵小路（10F）晚餐',
      ],
      meals: ['京都車站晚餐'],
      transit_notes: '從飯店步行約3分',
    },
  },
  2: {
    theme: '伏見稻荷 + 錦市場',
    morning: {
      focus: '伏見稻荷大社',
      activities: [
        '伏見稻荷大社 — 早起人較少',
        '千本鳥居隧道',
        '登頂或半山腰展望台（約1.5-2小時）',
      ],
      meals: ['飯店或車站輕食早餐'],
      transit_notes: 'JR奈良線 京都 → 稻荷（5分，1站）',
    },
    afternoon: {
      focus: '錦市場 & 四條購物',
      activities: [
        '錦市場（Nishiki Market）— 京都的廚房，小吃伴手禮',
        '四條河原町周邊購物',
        '寺町通 / 新京極商店街',
      ],
      meals: ['錦市場街邊小吃午餐'],
      transit_notes: 'JR稻荷 → 京都車站，地鐵到四條',
    },
    evening: {
      focus: '先斗町晚餐',
      activities: [
        '先斗町（Pontocho）小路散步',
        '先斗町或四條周邊晚餐',
      ],
      meals: ['先斗町晚餐（居酒屋或河畔餐廳）'],
      transit_notes: '從四條步行，地鐵回京都車站',
    },
  },
  3: {
    theme: '嵐山一日遊',
    morning: {
      focus: '保津川遊船',
      activities: [
        'JR嵯峨野線 京都 → 龜岡（約25分）',
        '保津川遊船（Hozugawa River Boat，約2小時）',
      ],
      meals: ['飯店或車站輕食早餐'],
      transit_notes: 'JR嵯峨野線 京都 → 龜岡',
    },
    afternoon: {
      focus: '嵐山觀光',
      activities: [
        '遊船抵達嵐山',
        '竹林小徑（Bamboo Grove）',
        '天龍寺（Tenryuji，世界遺產）',
      ],
      meals: ['嵐山午餐（湯豆腐或豆腐料理）'],
      transit_notes: '嵐山區域內步行',
    },
    evening: {
      focus: '渡月橋夕陽 + 返程',
      activities: [
        '渡月橋（Togetsukyo Bridge）夕陽',
        '嵐山花燈路（キモノフォレスト，晚間點燈）',
        '返回京都車站',
      ],
      meals: ['京都車站周邊晚餐'],
      transit_notes: 'JR嵯峨野線 嵯峨嵐山 → 京都',
    },
  },
  4: {
    theme: '和服午後 — 東山散策',
    morning: {
      focus: '自由活動 / 放鬆',
      activities: [
        '睡晚一點（前一天嵐山整天行程）',
        '可選：東寺（從飯店步行15分）',
        '11:00-12:00 京都車站周邊午餐',
      ],
      meals: ['京都車站午餐（拉麵小路10F 或 Porta地下街）'],
      transit_notes: '從飯店步行即可',
    },
    afternoon: {
      focus: '和服體驗 + 東山拍照',
      activities: [
        '13:00 抵達夢館（Yumeyakata）五條店',
        '和服換裝 + 髮型設計（約1小時）',
        '~14:00 計程車到二年坂（約5分，¥800）',
        '三年坂（Sannenzaka）— 石疊老街，和服拍照重點',
        '二年坂（Ninenzaka）— スターバックス京都二寧坂ヤサカ茶屋店',
        '八坂の塔（法觀寺）— 東山地標',
        '~15:30 計程車回夢館，還和服',
      ],
      meals: [],
      transit_notes: '步行或地鐵到五條（夢館），計程車來回東山',
    },
    evening: {
      focus: '祇園散步 + 伴手禮',
      activities: [
        '聖護院八ッ橋 — 買生八橋伴手禮（出發前一天最新鮮）',
        '祇園花見小路（Hanamikoji）漫步',
        '祇園晚餐',
      ],
      meals: ['祇園晚餐（懷石料理或居酒屋）'],
      transit_notes: '祇園搭巴士或計程車回京都車站',
    },
  },
  5: {
    theme: '購物 + 回程',
    morning: {
      focus: '最後購物 + 退房',
      activities: [
        '京都車站周邊最後購物（伊勢丹百貨、Porta地下街）',
        '打包行李',
        'APA Hotel 退房',
      ],
      meals: ['飯店早餐 或 京都車站'],
      transit_notes: '從飯店步行約3分',
    },
    afternoon: {
      focus: '前往關西機場',
      activities: [
        'JR Haruka特急 京都 → 關西機場（約75分）',
        '抵達關西機場辦理登機',
      ],
      meals: [],
      transit_notes: 'Haruka特急 京都 → 關西機場',
    },
    evening: {
      focus: '返回台灣',
      activities: [
        '泰獅航 KIX 13:30 → TPE 15:40',
      ],
      meals: [],
      transit_notes: '泰獅航 Thai Lion Air',
    },
  },
};

/** Ordered landmarks per day for Google Maps route links (Kyoto) */
export const ZH_KYOTO_DAY_LANDMARKS: Record<number, string[]> = {
  1: [], // arrival day
  2: ['Fushimi Inari Taisha', 'Nishiki Market Kyoto', 'Pontocho Kyoto'],
  3: ['Kameoka Station', 'Arashiyama Bamboo Grove', 'Tenryuji Temple', 'Togetsukyo Bridge'],
  4: ['Kyoto Yumeyakata Kimono', 'Ninenzaka Kyoto', 'Sannenzaka Kyoto', 'Yasaka Pagoda', 'Gion Kyoto'],
  5: [], // departure day
};

/** Per-segment routes for Kyoto (hotel = APA Hotel Kyoto Ekimae) */
export const ZH_KYOTO_DAY_ROUTES: Record<number, RouteSegment[]> = {
  1: [],
  2: [
    { from: 'hotel', to: 'Fushimi Inari Taisha', mode: 'transit' },
    { from: 'Fushimi Inari Taisha', to: 'Nishiki Market Kyoto', mode: 'transit' },
    { from: 'Nishiki Market Kyoto', to: 'Pontocho Kyoto', mode: 'walking' },
    { from: 'Pontocho Kyoto', to: 'hotel', mode: 'transit' },
  ],
  3: [
    { from: 'hotel', to: 'Kameoka Station', mode: 'transit' },
    { from: 'Kameoka Station', to: 'Arashiyama Bamboo Grove', mode: 'transit' },
    { from: 'Arashiyama Bamboo Grove', to: 'Tenryuji Temple', mode: 'walking' },
    { from: 'Tenryuji Temple', to: 'Togetsukyo Bridge', mode: 'walking' },
    { from: 'Togetsukyo Bridge', to: 'hotel', mode: 'transit' },
  ],
  4: [
    { from: 'hotel', to: 'Kyoto Yumeyakata Kimono', mode: 'transit' },
    { from: 'Kyoto Yumeyakata Kimono', to: 'Ninenzaka Kyoto', mode: 'walking' },
    { from: 'Ninenzaka Kyoto', to: 'Sannenzaka Kyoto', mode: 'walking' },
    { from: 'Sannenzaka Kyoto', to: 'Yasaka Pagoda', mode: 'walking' },
    { from: 'Yasaka Pagoda', to: 'Gion Kyoto', mode: 'walking' },
    { from: 'Gion Kyoto', to: 'hotel', mode: 'transit' },
  ],
  5: [],
};

/** Hotel name EN → ZH mapping (Kyoto) */
export const ZH_KYOTO_HOTELS: Record<string, string> = {
  'APA Hotel Kyoto Ekimae': 'APA京都站前',
  'APA Hotel Kyoto Ekimae (APA京都站前)': 'APA京都站前',
};

/** ZH transit cheat sheet (Kyoto) */
export const ZH_KYOTO_TRANSIT = {
  hotel_station: 'JR京都車站（從APA Hotel步行3分）',
  key_lines: [
    'JR Haruka特急 — 關西機場 ↔ 京都車站（約75分）',
    'JR嵯峨野線 — 京都 → 龜岡/嵯峨嵐山',
    'JR奈良線 — 京都 → 稻荷（1站，5分）',
    '京都市巴士 — 206/100 東山、清水寺方向',
    '地鐵烏丸線 — 京都車站 ↔ 五條（1站）',
  ],
};
