/**
 * Chinese (Traditional / Taiwan) content overrides for Tokyo trip.
 * Keyed by day_number → session → field.
 * Source: data/tokyo-trip-plan-zh.md
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
