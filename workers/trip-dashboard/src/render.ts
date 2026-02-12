import { CSS } from './styles';
import type { PlanData, BookingRow, PlanSummary } from './turso';
import {
  ZH_DAYS, ZH_TRANSIT, ZH_HOTELS, ZH_DAY_LANDMARKS,
  ZH_KYOTO_DAYS, ZH_KYOTO_TRANSIT, ZH_KYOTO_HOTELS, ZH_KYOTO_DAY_LANDMARKS,
  ZH_DAY_ROUTES, ZH_KYOTO_DAY_ROUTES, HOME_ADDRESS, KYOTO_HOME_ADDRESS,
  type RouteSegment,
} from './zh-content';

type Lang = 'en' | 'zh';

const T: Record<string, Record<Lang, string>> = {
  title: { en: 'Tokyo Trip', zh: '東京旅行' },
  bookingSummary: { en: 'Booking Summary', zh: '訂位總覽' },
  package: { en: 'Package', zh: '機+酒套裝' },
  flightOut: { en: 'Flight Out', zh: '去程航班' },
  flightBack: { en: 'Flight Back', zh: '回程航班' },
  hotel: { en: 'Hotel', zh: '飯店' },
  toHotel: { en: 'Airport \u2192 Hotel', zh: '機場\u2192飯店' },
  toAirport: { en: 'Hotel \u2192 Airport', zh: '飯店\u2192機場' },
  transitCheat: { en: 'Transit Cheat Sheet', zh: '交通速查表' },
  destination: { en: 'Destination', zh: '目的地' },
  route: { en: 'Route', zh: '路線' },
  cost: { en: 'Cost', zh: '費用' },
  time: { en: 'Time', zh: '時間' },
  morning: { en: 'Morning', zh: '上午' },
  afternoon: { en: 'Afternoon', zh: '下午' },
  evening: { en: 'Evening', zh: '晚上' },
  day: { en: 'Day', zh: '第' },
  dayUnit: { en: '', zh: '天' },
  arrival: { en: 'ARRIVAL', zh: '抵達' },
  departure: { en: 'DEPARTURE', zh: '回程' },
  fullDay: { en: 'FULL DAY', zh: '全天' },
  pending: { en: 'PENDING', zh: '待訂' },
  booked: { en: 'Booked', zh: '已訂' },
  planned: { en: 'Planned', zh: '已規劃' },
  bookBy: { en: 'Book by', zh: '預約期限' },
  actionItems: { en: 'Action Items', zh: '待辦事項' },
  packing: { en: 'Packing Notes', zh: '行李打包建議' },
  rain: { en: 'Rain', zh: '降雨' },
  langSwitch: { en: '中文', zh: 'EN' },
  langTarget: { en: 'zh', zh: 'en' },
  lastUpdated: { en: 'Last updated', zh: '最後更新' },
  perPerson: { en: '/person', zh: '/人' },
  forTwo: { en: 'for 2', zh: '2人共' },
  dailyTransit: { en: 'Daily transit: ~\u00a5600-800/person', zh: '每日交通：約\u00a5600-800/人' },
  homeBase: { en: 'Home base', zh: '住宿' },
  routeMap: { en: 'Route', zh: '路線' },
};

function t(key: string, lang: Lang): string {
  return T[key]?.[lang] ?? key;
}

function esc(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function formatDate(dateStr: string, lang: Lang): string {
  const d = new Date(dateStr + 'T00:00:00Z');
  const days = lang === 'zh'
    ? ['日', '一', '二', '三', '四', '五', '六']
    : ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const months = lang === 'zh'
    ? ['1月', '2月', '3月', '4月', '5月', '6月', '7月', '8月', '9月', '10月', '11月', '12月']
    : ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  const dow = days[d.getUTCDay()];
  const mon = months[d.getUTCMonth()];
  const date = d.getUTCDate();
  return lang === 'zh'
    ? `${mon}${date}日（${dow}）`
    : `${dow}, ${mon} ${date}`;
}

function weatherIcon(description: string): string {
  const d = description.toLowerCase();
  if (d.includes('rain') || d.includes('雨')) return '\u{1F327}\uFE0F';
  if (d.includes('partly') || d.includes('局部')) return '\u26C5';
  if (d.includes('overcast') || d.includes('多雲') || d.includes('cloudy')) return '\u2601\uFE0F';
  if (d.includes('snow') || d.includes('雪')) return '\u2744\uFE0F';
  if (d.includes('clear') || d.includes('晴')) return '\u2600\uFE0F';
  return '\u26C5';
}

function dayTypeBadge(dayType: string, lang: Lang): string {
  if (dayType === 'arrival') return `<span class="day-type-badge">\u2708\uFE0F ${t('arrival', lang)}</span>`;
  if (dayType === 'departure') return `<span class="day-type-badge">\u2708\uFE0F ${t('departure', lang)}</span>`;
  return '';
}

function statusBadge(status: string, lang: Lang): string {
  if (status === 'booked') return `<span class="badge badge-booked">${t('booked', lang)}</span>`;
  if (status === 'planned') return `<span class="badge badge-planned">${t('planned', lang)}</span>`;
  if (status === 'pending') return `<span class="badge badge-pending">${t('pending', lang)}</span>`;
  return `<span class="badge badge-planned">${esc(status)}</span>`;
}

function extractActivities(activities: unknown[]): string[] {
  return activities.map((a) => {
    if (typeof a === 'string') return a;
    if (typeof a === 'object' && a !== null && 'title' in a) {
      const obj = a as Record<string, unknown>;
      let text = obj.title as string;
      if (obj.booking_status === 'pending' && obj.book_by) {
        text = `<span class="activity-booking">\u23F3 ${esc(text)}</span>`;
      }
      return text;
    }
    return String(a);
  });
}

// Convert schedule-based format to session-based format
interface ScheduleItem {
  time: string;
  activity: string;
  location?: string;
  transport?: string;
  duration?: string;
  notes?: string;
}

interface ConvertedSessions {
  morning: { focus: string; activities: string[]; meals: string[]; transit_notes: string };
  afternoon: { focus: string; activities: string[]; meals: string[]; transit_notes: string };
  evening: { focus: string; activities: string[]; meals: string[]; transit_notes: string };
}

function parseTimeToHour(timeStr: string): number {
  // Handle formats like "09:00", "13:00-14:30", "15:30-18:00"
  const match = timeStr.match(/^(\d{1,2}):/);
  return match ? parseInt(match[1], 10) : 12;
}

function convertScheduleToSessions(schedule: ScheduleItem[], title: string): ConvertedSessions {
  const sessions: ConvertedSessions = {
    morning: { focus: '', activities: [], meals: [], transit_notes: '' },
    afternoon: { focus: '', activities: [], meals: [], transit_notes: '' },
    evening: { focus: '', activities: [], meals: [], transit_notes: '' },
  };

  for (const item of schedule) {
    const hour = parseTimeToHour(item.time);
    let session: keyof ConvertedSessions;
    if (hour < 12) {
      session = 'morning';
    } else if (hour < 17) {
      session = 'afternoon';
    } else {
      session = 'evening';
    }

    // Build activity string
    let activityText = `${item.time} ${item.activity}`;
    if (item.location) activityText += ` (${item.location})`;

    // Check if it's a meal
    const actLower = item.activity.toLowerCase();
    if (actLower.includes('dinner') || actLower.includes('lunch') || actLower.includes('breakfast') ||
        actLower.includes('晚餐') || actLower.includes('午餐') || actLower.includes('早餐')) {
      sessions[session].meals.push(item.activity);
    } else {
      sessions[session].activities.push(activityText);
    }

    // Collect transport notes
    if (item.transport) {
      if (sessions[session].transit_notes) {
        sessions[session].transit_notes += '; ';
      }
      sessions[session].transit_notes += item.transport;
    }

    // Add notes if present
    if (item.notes) {
      sessions[session].activities.push(`\u2192 ${item.notes}`);
    }
  }

  // Set focus based on first activity or title
  const titleParts = title.split('+').map(s => s.trim());
  if (titleParts.length >= 3) {
    sessions.morning.focus = titleParts[0] || sessions.morning.activities[0] || '';
    sessions.afternoon.focus = titleParts[1] || sessions.afternoon.activities[0] || '';
    sessions.evening.focus = titleParts[2] || sessions.evening.activities[0] || '';
  } else {
    sessions.morning.focus = sessions.morning.activities[0] || title;
    sessions.afternoon.focus = sessions.afternoon.activities[0] || '';
    sessions.evening.focus = sessions.evening.activities[0] || '';
  }

  return sessions;
}

function inferDayType(day: Record<string, unknown>): string {
  // Check explicit day_type first
  if (day.day_type) return day.day_type as string;

  // Infer from title
  const title = ((day.title as string) || '').toLowerCase();
  if (title.includes('arrival') || title.includes('抵達')) return 'arrival';
  if (title.includes('departure') || title.includes('回程')) return 'departure';

  // Infer from schedule
  const schedule = day.schedule as ScheduleItem[] | undefined;
  if (schedule) {
    for (const item of schedule) {
      const act = item.activity.toLowerCase();
      if (act.includes('depart tpe') || act.includes('arrive k') || act.includes('arrive n')) {
        return 'arrival';
      }
      if (act.includes('depart k') || act.includes('depart n') || act.includes('arrive tpe')) {
        return 'departure';
      }
    }
  }

  return 'full';
}

interface SessionZhOverride {
  focus: string;
  activities: string[];
  meals: string[];
  transit_notes: string;
}

function renderTransitPill(transit: string, city: string): string {
  if (transit.includes('\u2192')) {
    const parts = transit.split('\u2192');
    if (parts.length >= 2) {
      const from = parts[0].trim().replace(/\d{1,2}:\d{2}/g, '').trim();
      let to = parts[1].split(/[（。，,]/)[0].trim().replace(/\d{1,2}:\d{2}/g, '').trim();
      // Remove trailing 步行 from destination name
      to = to.replace(/\s*\u6B65\u884C\s*$/, '').trim();
      if (from && to) {
        // Check travel mode only from the route description (before any period/sentence break), not trailing notes
        const routePart = transit.split(/[。]/)[0];
        const travelmode = (routePart.includes('\u6B65\u884C') && !routePart.includes('\u7DDA') && !routePart.includes('\u5DF4\u58EB')) ? 'walking' : 'transit';
        const url = `https://www.google.com/maps/dir/?api=1&origin=${encodeURIComponent(from + ' ' + city)}&destination=${encodeURIComponent(to + ' ' + city)}&travelmode=${travelmode}`;
        return `<a class="pill pill-transit" href="${esc(url)}" target="_blank" rel="noopener">\uD83D\uDE83 ${esc(transit)} \uD83D\uDDFA\uFE0F</a>`;
      }
    }
  }
  return `<span class="pill pill-transit">\uD83D\uDE83 ${esc(transit)}</span>`;
}

function renderSession(
  session: Record<string, unknown> | undefined,
  sessionKey: 'morning' | 'afternoon' | 'evening',
  lang: Lang,
  zhOverride?: SessionZhOverride,
  mapCity?: string
): string {
  if (!session) return '';

  const focus = zhOverride?.focus ?? ((session.focus as string) || '');
  const activities = zhOverride
    ? zhOverride.activities
    : extractActivities((session.activities as unknown[]) || []);
  const meals = zhOverride?.meals ?? ((session.meals as string[]) || []);
  const transit = zhOverride?.transit_notes ?? ((session.transit_notes as string) || '');

  // For ZH override, check if original session has pending booking activities
  const pendingBookings: string[] = [];
  if (zhOverride) {
    for (const a of (session.activities as unknown[]) || []) {
      if (typeof a === 'object' && a !== null && 'booking_status' in a) {
        const obj = a as Record<string, unknown>;
        if (obj.booking_status === 'pending' && obj.book_by && obj.title) {
          pendingBookings.push(obj.title as string);
        }
      }
    }
  }

  return `
    <div class="session session-${sessionKey}">
      <div class="session-label">${t(sessionKey, lang)}</div>
      <div class="session-focus">${esc(focus)}</div>
      <ul class="activity-list">
        ${activities.map((a) => {
          // Check if this maps to a pending booking from original data
          const isPending = pendingBookings.some((pb) => a.includes('teamLab') || a.includes('無界') || a.toLowerCase().includes(pb.toLowerCase()));
          if (isPending && !a.includes('<span')) {
            return `<li><span class="activity-booking">\u23F3 ${esc(a)}</span></li>`;
          }
          return `<li>${a.includes('<span') ? a : esc(a)}</li>`;
        }).join('')}
      </ul>
      <div class="info-pills">
        ${transit ? renderTransitPill(transit, mapCity || '') : ''}
        ${meals.map((m) => `<span class="pill pill-meal">\uD83C\uDF5C ${esc(m)}</span>`).join('')}
      </div>
    </div>`;
}

function clothingTip(tempLow: number, tempHigh: number, rainPct: number, lang: Lang): string {
  const tips: string[] = [];

  // Morning/evening tip based on low temp
  if (tempLow <= 0) {
    tips.push(lang === 'zh'
      ? '\uD83E\uDDE3 早晚接近0\u00B0C\u2014\u2014厚外套、圍巾、手套'
      : '\uD83E\uDDE3 Near 0\u00B0C morning/evening\u2014heavy coat, scarf, gloves');
  } else if (tempLow <= 5) {
    tips.push(lang === 'zh'
      ? `\uD83E\uDDE5 早晚${tempLow}\u00B0C\u2014\u2014冬季外套+毛衣，下午可脫`
      : `\uD83E\uDDE5 ${tempLow}\u00B0C morning/evening\u2014winter coat+sweater, lighter afternoon`);
  } else if (tempLow <= 10) {
    tips.push(lang === 'zh'
      ? `\uD83E\uDDE5 早晚${Math.round(tempLow)}\u00B0C\u2014\u2014外套+薄毛衣`
      : `\uD83E\uDDE5 ${Math.round(tempLow)}\u00B0C morning/evening\u2014jacket+light sweater`);
  }

  // Afternoon warmth note if big temp swing
  if (tempHigh - tempLow >= 10) {
    tips.push(lang === 'zh'
      ? `\u2600\uFE0F 午後回暖到${tempHigh}\u00B0C\u2014\u2014穿脫方便的洋蔥式穿搭`
      : `\u2600\uFE0F Warms to ${tempHigh}\u00B0C afternoon\u2014layer for easy removal`);
  }

  // Rain gear
  if (rainPct >= 30) {
    tips.push(lang === 'zh' ? '\u2614 帶傘（降雨機率高）' : '\u2614 Bring umbrella (likely rain)');
  } else if (rainPct >= 15) {
    tips.push(lang === 'zh' ? '\uD83C\uDF02 帶折疊傘備用' : '\uD83C\uDF02 Compact umbrella just in case');
  }

  if (tips.length === 0) return '';
  return `<div class="weather-clothing">${tips.join(' &nbsp;\u00B7&nbsp; ')}</div>`;
}

function renderWeatherStrip(day: Record<string, unknown>, lang: Lang): string {
  const weather = day.weather as Record<string, unknown> | undefined;
  if (!weather) {
    return `<div class="weather-strip"><span class="weather-icon">\u26C5</span><span style="color:var(--text-dim);font-size:12px">${lang === 'zh' ? '無天氣資料' : 'No weather data'}</span></div>`;
  }
  const desc = (weather.weather_label as string) || '';
  const tMin = weather.temp_low_c ?? '';
  const tMax = weather.temp_high_c ?? '';
  const rain = weather.precipitation_pct ?? '';
  const flMin = weather.feels_like_low_c;
  const flMax = weather.feels_like_high_c;

  const tMinNum = typeof tMin === 'number' ? tMin : parseFloat(String(tMin));
  const tMaxNum = typeof tMax === 'number' ? tMax : parseFloat(String(tMax));
  const rainNum = typeof rain === 'number' ? rain : parseFloat(String(rain));
  const flMinNum = flMin != null ? (typeof flMin === 'number' ? flMin : parseFloat(String(flMin))) : NaN;
  const flMaxNum = flMax != null ? (typeof flMax === 'number' ? flMax : parseFloat(String(flMax))) : NaN;

  // Use feels-like temps for clothing tips when available (more relevant for what to wear)
  const tipLow = !isNaN(flMinNum) ? flMinNum : tMinNum;
  const tipHigh = !isNaN(flMaxNum) ? flMaxNum : tMaxNum;
  const tip = (!isNaN(tipLow) && !isNaN(tipHigh))
    ? clothingTip(tipLow, tipHigh, isNaN(rainNum) ? 0 : rainNum, lang)
    : '';

  const feelsLikeHtml = (!isNaN(flMinNum) && !isNaN(flMaxNum))
    ? `<span class="weather-feels">${lang === 'zh' ? '體感' : 'Feels'} ${Math.round(flMinNum)}\u2013${Math.round(flMaxNum)}\u00B0C</span>`
    : '';

  return `
    <div class="weather-strip">
      <span class="weather-icon">${weatherIcon(desc)}</span>
      <span class="weather-temp">${tMin}\u2013${tMax}\u00B0C</span>
      ${feelsLikeHtml}
      <span style="color:var(--text-dim);font-size:12px">${esc(String(desc))}</span>
      ${rain !== '' ? `<span class="weather-rain">\uD83D\uDCA7 ${rain}%</span>` : ''}
    </div>
    ${tip}`;
}

function buildGoogleMapsUrl(hotel: string, landmarks: string[]): string {
  const waypoints = [hotel, ...landmarks, hotel];
  const path = waypoints.map((w) => encodeURIComponent(w)).join('/');
  return `https://www.google.com/maps/dir/${path}`;
}

function renderRouteLink(dayNum: number, hotelName: string, lang: Lang, isTokyoPlan: boolean, isKyotoPlan: boolean): string {
  if (!hotelName) return '';
  const landmarks = isTokyoPlan ? ZH_DAY_LANDMARKS[dayNum]
    : isKyotoPlan ? ZH_KYOTO_DAY_LANDMARKS[dayNum]
    : undefined;
  if (!landmarks || landmarks.length === 0) return '';
  const url = buildGoogleMapsUrl(hotelName, landmarks);
  return `<a class="route-btn" href="${esc(url)}" target="_blank" rel="noopener">\uD83D\uDDFA\uFE0F ${t('routeMap', lang)}</a>`;
}

function renderMapEmbed(dayNum: number, hotelName: string, lang: Lang, isTokyoPlan: boolean, isKyotoPlan: boolean, _mapsKey?: string): string {
  if (!hotelName) return '';

  const landmarks = isTokyoPlan ? ZH_DAY_LANDMARKS[dayNum]
    : isKyotoPlan ? ZH_KYOTO_DAY_LANDMARKS[dayNum]
    : undefined;
  const routes: RouteSegment[] | undefined = isTokyoPlan ? ZH_DAY_ROUTES[dayNum]
    : isKyotoPlan ? ZH_KYOTO_DAY_ROUTES[dayNum]
    : undefined;

  const hasLandmarks = landmarks && landmarks.length > 0;
  const hasRoutes = routes && routes.length > 0;
  if (!hasLandmarks && !hasRoutes) return '';

  const links: string[] = [];

  // Days with landmarks: day route link + individual place links
  if (hasLandmarks) {
    // Individual place links for each stop
    for (const place of landmarks) {
      const placeUrl = `https://www.google.com/maps/search/?api=1&query=${encodeURIComponent(place)}`;
      links.push(`<a class="map-place-link" href="${esc(placeUrl)}" target="_blank" rel="noopener">\uD83D\uDCCD ${esc(place)}</a>`);
    }
  }

  // Transit segment links for arrival/departure days
  if (!hasLandmarks && hasRoutes) {
    const homeAddr = isKyotoPlan ? KYOTO_HOME_ADDRESS : HOME_ADDRESS;
    const resolve = (name: string) => name === 'hotel' ? hotelName : name === 'home' ? homeAddr : name;
    const display = (name: string) => name === 'home' ? (lang === 'zh' ? '住家' : 'Home') : name === 'hotel' ? hotelName : name;
    const modeIcon = (mode: string) => mode === 'transit' ? '\uD83D\uDE87' : mode === 'driving' ? '\uD83D\uDE97' : '\uD83D\uDEB6';

    for (const seg of routes) {
      const from = resolve(seg.from);
      const to = resolve(seg.to);
      const dirUrl = `https://www.google.com/maps/dir/${encodeURIComponent(from)}/${encodeURIComponent(to)}`;
      links.push(`<a class="map-place-link" href="${esc(dirUrl)}" target="_blank" rel="noopener">${modeIcon(seg.mode)} ${esc(display(seg.from))} → ${esc(display(seg.to))}</a>`);
    }
  }

  if (links.length === 0) return '';

  return `<div class="map-links">${links.join('')}</div>`;
}

function renderDayCard(day: Record<string, unknown>, lang: Lang, hotelName: string, isTokyoPlan: boolean, isKyotoPlan: boolean, mapsKey?: string): string {
  // Support both formats: day_number (Tokyo) and day (Kyoto)
  const dayNum = (day.day_number as number) || (day.day as number);
  const date = day.date as string;
  const dayType = inferDayType(day);
  // Use appropriate ZH content based on destination
  const zhDay = lang === 'zh'
    ? (isTokyoPlan ? ZH_DAYS[dayNum] : isKyotoPlan ? ZH_KYOTO_DAYS[dayNum] : undefined)
    : undefined;
  // Support both: theme (Tokyo) and title (Kyoto)
  const theme = zhDay?.theme ?? ((day.theme as string) || (day.title as string) || '');

  // Check if this is schedule-based format (Kyoto) or session-based (Tokyo)
  const schedule = day.schedule as ScheduleItem[] | undefined;
  let morningSession: Record<string, unknown> | undefined;
  let afternoonSession: Record<string, unknown> | undefined;
  let eveningSession: Record<string, unknown> | undefined;
  let morningOverride: SessionZhOverride | undefined;
  let afternoonOverride: SessionZhOverride | undefined;
  let eveningOverride: SessionZhOverride | undefined;

  if (schedule && schedule.length > 0) {
    // Convert schedule-based format to sessions
    const converted = convertScheduleToSessions(schedule, (day.title as string) || '');
    morningSession = converted.morning as unknown as Record<string, unknown>;
    afternoonSession = converted.afternoon as unknown as Record<string, unknown>;
    eveningSession = converted.evening as unknown as Record<string, unknown>;
    // For schedule format, the converted data IS the override (no separate ZH override)
    morningOverride = converted.morning;
    afternoonOverride = converted.afternoon;
    eveningOverride = converted.evening;
  } else {
    // Session-based format (Tokyo)
    morningSession = day.morning as Record<string, unknown>;
    afternoonSession = day.afternoon as Record<string, unknown>;
    eveningSession = day.evening as Record<string, unknown>;
    morningOverride = zhDay?.morning;
    afternoonOverride = zhDay?.afternoon;
    eveningOverride = zhDay?.evening;
  }

  return `
    <div class="day-card">
      <div class="day-header">
        <div>
          <div class="day-number">${t('day', lang)}${dayNum}${t('dayUnit', lang)}</div>
          <div class="day-date">${formatDate(date, lang)}</div>
        </div>
        <div style="display:flex;align-items:center;gap:6px">
          ${dayTypeBadge(dayType, lang)}
          ${renderRouteLink(dayNum, hotelName, lang, isTokyoPlan, isKyotoPlan)}
        </div>
      </div>
      <div class="day-theme">${esc(theme)}</div>
      ${renderWeatherStrip(day, lang)}
      ${(() => {
        const city = isTokyoPlan ? 'Tokyo' : isKyotoPlan ? 'Kyoto' : '';
        return [
          renderSession(morningSession, 'morning', lang, morningOverride, city),
          renderSession(afternoonSession, 'afternoon', lang, afternoonOverride, city),
          renderSession(eveningSession, 'evening', lang, eveningOverride, city),
        ].join('');
      })()}
      ${renderMapEmbed(dayNum, hotelName, lang, isTokyoPlan, isKyotoPlan, mapsKey)}
    </div>`;
}

function renderBookingSummary(dest: Record<string, unknown>, lang: Lang, isTokyoPlan: boolean, isKyotoPlan: boolean): string {
  const transport = dest.process_3_transportation as Record<string, unknown> | undefined;
  const accommodation = dest.process_4_accommodation as Record<string, unknown> | undefined;
  const packages = dest.process_3_4_packages as Record<string, unknown> | undefined;
  const transfers = transport?.airport_transfers as Record<string, unknown> | undefined;

  const flight = transport?.flight as Record<string, unknown> | undefined;
  const outbound = flight?.outbound as Record<string, unknown> | undefined;
  const returnFl = flight?.return as Record<string, unknown> | undefined;
  const hotel = accommodation?.hotel as Record<string, unknown> | undefined;
  const arrival = transfers?.arrival as Record<string, unknown> | undefined;
  const departure = transfers?.departure as Record<string, unknown> | undefined;
  const arrSelected = arrival?.selected as Record<string, unknown> | undefined;
  const depSelected = departure?.selected as Record<string, unknown> | undefined;

  const chosenOffer = packages?.chosen_offer as Record<string, unknown> | undefined;
  const offers = (packages?.results as Record<string, unknown>)?.offers as unknown[] | undefined;
  let offerData: Record<string, unknown> | undefined;
  if (chosenOffer?.id && offers) {
    offerData = offers.find((o: unknown) => (o as Record<string, unknown>).id === chosenOffer.id) as Record<string, unknown>;
  }

  const pricePerPerson = offerData?.price_per_person as number | undefined;
  const selectedDate = chosenOffer?.selected_date as string | undefined;
  const datePricing = offerData?.date_pricing as Record<string, Record<string, unknown>> | undefined;
  let actualPrice = pricePerPerson;
  if (selectedDate && datePricing?.[selectedDate]) {
    actualPrice = datePricing[selectedDate].price as number;
  }

  const airline = (flight?.airline as string) || '';
  const airlineCode = (flight?.airline_code as string) || '';

  const items = [
    {
      icon: '\uD83D\uDCE6',
      label: t('package', lang),
      value: offerData
        ? `${esc((offerData.source_id as string) || '')} ${esc((offerData.product_code as string) || '')}`
        : '\u2014',
      sub: actualPrice
        ? `TWD ${actualPrice.toLocaleString()}${t('perPerson', lang)} (${t('forTwo', lang)} ${(actualPrice * 2).toLocaleString()})`
        : '',
      badge: packages?.status ? statusBadge(packages.status as string, lang) : '',
    },
    {
      icon: '\u2708\uFE0F',
      label: t('flightOut', lang),
      value: outbound
        ? `${esc(airline)} ${esc(airlineCode)}${esc((outbound.flight_number as string) || '')}`
        : '\u2014',
      sub: outbound
        ? `${esc((outbound.departure_airport_code as string) || '')} ${esc((outbound.departure_time as string) || '')} \u2192 ${esc((outbound.arrival_airport_code as string) || '')} ${esc((outbound.arrival_time as string) || '')}`
        : '',
      badge: '',
    },
    {
      icon: '\u2708\uFE0F',
      label: t('flightBack', lang),
      value: returnFl
        ? `${esc(airline)} ${esc(airlineCode)}${esc((returnFl.flight_number as string) || '')}`
        : '\u2014',
      sub: returnFl
        ? `${esc((returnFl.departure_airport_code as string) || '')} ${esc((returnFl.departure_time as string) || '')} \u2192 ${esc((returnFl.arrival_airport_code as string) || '')} ${esc((returnFl.arrival_time as string) || '')}`
        : '',
      badge: '',
    },
    {
      icon: '\uD83C\uDFE8',
      label: t('hotel', lang),
      value: hotel ? (() => {
        const name = (hotel.name as string) || '';
        const zhName = isTokyoPlan ? ZH_HOTELS[name]
          : isKyotoPlan ? ZH_KYOTO_HOTELS[name]
          : undefined;
        return zhName ? `${esc(name)}<span style="color:var(--text-dim);font-size:12px"> / ${esc(zhName)}</span>` : esc(name);
      })() : '\u2014',
      sub: hotel?.access ? (hotel.access as string[]).join(', ') : '',
      badge: accommodation?.status ? statusBadge(accommodation.status as string, lang) : '',
    },
    {
      icon: '\uD83D\uDE8C',
      label: t('toHotel', lang),
      value: arrSelected ? esc((arrSelected.title as string) || '') : '\u2014',
      sub: arrSelected
        ? `${esc((arrSelected.route as string) || '')} \u00B7 \u00A5${(arrSelected.price_yen as number)?.toLocaleString() || ''} \u00B7 ${esc((arrSelected.schedule as string) || '')}`
        : '',
      badge: arrival?.status ? statusBadge(arrival.status as string, lang) : '',
    },
    {
      icon: '\uD83D\uDE8C',
      label: t('toAirport', lang),
      value: depSelected ? esc((depSelected.title as string) || '') : '\u2014',
      sub: depSelected
        ? `${esc((depSelected.route as string) || '')} \u00B7 \u00A5${(depSelected.price_yen as number)?.toLocaleString() || ''} \u00B7 ${esc((depSelected.schedule as string) || '')}`
        : '',
      badge: departure?.status ? statusBadge(departure.status as string, lang) : '',
    },
  ];

  return `
    <div class="booking-summary">
      <h2>${t('bookingSummary', lang)}</h2>
      <div class="booking-grid">
        ${items
          .map(
            (item) => `
          <div class="booking-item">
            <div class="booking-icon">${item.icon}</div>
            <div class="booking-detail">
              <div class="booking-label">${item.label} ${item.badge}</div>
              <div class="booking-value">${item.value}</div>
              ${item.sub ? `<div class="booking-sub">${item.sub}</div>` : ''}
            </div>
          </div>`
          )
          .join('')}
      </div>
    </div>`;
}

function renderPendingAlerts(dest: Record<string, unknown>, lang: Lang): string {
  const itinerary = dest.process_5_daily_itinerary as Record<string, unknown> | undefined;
  const days = (itinerary?.days as Record<string, unknown>[]) || [];
  const alerts: string[] = [];

  for (const day of days) {
    for (const sessionKey of ['morning', 'afternoon', 'evening']) {
      const session = day[sessionKey] as Record<string, unknown> | undefined;
      if (!session?.activities) continue;
      for (const activity of session.activities as unknown[]) {
        if (typeof activity === 'object' && activity !== null) {
          const a = activity as Record<string, unknown>;
          if (a.booking_status === 'pending') {
            const bookBy = a.book_by as string | undefined;
            const isUrgent = bookBy ? new Date(bookBy) <= new Date() : false;
            const url = a.booking_url as string | undefined;
            const title = a.title as string;
            alerts.push(`
              <div class="alert ${isUrgent ? 'alert-urgent' : ''}">
                <span class="alert-icon">${isUrgent ? '\u26A0\uFE0F' : '\u23F3'}</span>
                <div class="alert-text">
                  <strong>${esc(title)}</strong>
                  ${bookBy ? ` \u2014 ${t('bookBy', lang)} ${esc(bookBy)}` : ''}
                  ${url ? `<br><a href="${esc(url)}" target="_blank">${lang === 'zh' ? '立即預約' : 'Book now'} \u2192</a>` : ''}
                </div>
              </div>`);
          }
        }
      }
    }
  }

  return alerts.join('');
}

function renderTransitSummary(dest: Record<string, unknown>, lang: Lang, isTokyoPlan: boolean, isKyotoPlan: boolean): string {
  const itinerary = dest.process_5_daily_itinerary as Record<string, unknown> | undefined;
  const transit = itinerary?.transit_summary as Record<string, unknown> | undefined;
  if (!transit) return '';

  const keyLines = lang === 'zh'
    ? (isTokyoPlan ? ZH_TRANSIT.key_lines : isKyotoPlan ? ZH_KYOTO_TRANSIT.key_lines : (transit.key_lines as string[]) || [])
    : (transit.key_lines as string[]) || [];
  const hotelStation = lang === 'zh'
    ? (isTokyoPlan ? ZH_TRANSIT.hotel_station : isKyotoPlan ? ZH_KYOTO_TRANSIT.hotel_station : (transit.hotel_station as string) || '')
    : (transit.hotel_station as string) || '';

  return `
    <div class="transit-summary">
      <h2>${t('transitCheat', lang)}</h2>
      <div style="font-size:13px;margin-bottom:10px">
        <strong>${t('homeBase', lang)}:</strong> ${esc(hotelStation)}
      </div>
      <div style="font-size:12px;color:var(--text-dim);margin-bottom:6px">
        ${keyLines.map((l) => `<div>\u2022 ${esc(l)}</div>`).join('')}
      </div>
      <div style="font-size:12px;color:var(--accent);font-weight:500;margin-top:8px">
        ${t('dailyTransit', lang)}
      </div>
    </div>`;
}

function renderPlanNav(plans: PlanSummary[], currentSlug: string, lang: Lang): string {
  if (plans.length <= 1) return '';
  const pills = plans.map((p) => {
    const isCurrent = p.slug === currentSlug;
    const href = `/?plan=${esc(p.slug)}&lang=${lang}&nav=1`;
    return isCurrent
      ? `<span class="plan-pill plan-pill-active">${esc(p.display_name)}</span>`
      : `<a class="plan-pill" href="${href}">${esc(p.display_name)}</a>`;
  });
  return `<div class="plan-nav">${pills.join('')}</div>`;
}

export function renderDashboard(
  planData: PlanData,
  _bookings: BookingRow[],
  lang: Lang,
  planId?: string,
  plans?: PlanSummary[],
  mapsKey?: string
): string {
  const plan = JSON.parse(planData.plan_json);
  const activeDest = plan.active_destination as string;
  const isTokyoPlan = activeDest === 'tokyo_2026';
  const isKyotoPlan = activeDest === 'kyoto_2026';
  const dest = plan.destinations?.[activeDest] as Record<string, unknown> | undefined;

  if (!dest) {
    return renderError(`Destination "${activeDest}" not found in plan`, lang);
  }

  const displayName = (dest.display_name as string) || activeDest;
  const dates = dest.process_1_date_anchor as Record<string, unknown> | undefined;
  const confirmed = dates?.confirmed_dates as Record<string, unknown> | undefined;
  const startDate = (confirmed?.start as string) || '';
  const endDate = (confirmed?.end as string) || '';
  const numDays = (dates?.days as number) || 5;

  const accom = dest.process_4_accommodation as Record<string, unknown> | undefined;
  const hotelObj = accom?.hotel as Record<string, unknown> | undefined;
  const hotelName = (hotelObj?.name as string) || '';

  const itinerary = dest.process_5_daily_itinerary as Record<string, unknown> | undefined;
  const days = (itinerary?.days as Record<string, unknown>[]) || [];

  const langParam = lang === 'zh' ? 'en' : 'zh';
  const navSuffix = plans ? '&nav=1' : '';
  const langHref = planId
    ? `?lang=${langParam}&plan=${esc(planId)}${navSuffix}`
    : `?lang=${langParam}${navSuffix}`;

  return `<!DOCTYPE html>
<html lang="${lang === 'zh' ? 'zh-Hant' : 'en'}">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${esc(displayName)} ${lang === 'zh' ? '旅行計畫' : 'Trip Plan'}</title>
  <style>${CSS}</style>
</head>
<body>
  <div class="header">
    <div>
      <h1>${lang === 'zh' ? `${esc(displayName)}旅行計畫` : `${esc(displayName)} Trip Plan`}</h1>
      <div class="header-sub">${formatDate(startDate, lang)} \u2192 ${formatDate(endDate, lang)} (${numDays} ${lang === 'zh' ? '天' : 'days'})</div>
    </div>
    <a class="lang-btn" href="${langHref}">${t('langSwitch', lang)}</a>
  </div>

  ${plans ? renderPlanNav(plans, activeDest.replace(/_/g, '-'), lang) : ''}

  ${renderPendingAlerts(dest, lang)}
  ${renderBookingSummary(dest, lang, isTokyoPlan, isKyotoPlan)}

  ${days.map((day) => renderDayCard(day, lang, hotelName, isTokyoPlan, isKyotoPlan, mapsKey)).join('')}

  ${renderTransitSummary(dest, lang, isTokyoPlan, isKyotoPlan)}

  <div class="footer">
    ${t('lastUpdated', lang)}: ${esc(planData.updated_at)}<br>
    Powered by Turso + Cloudflare Workers
  </div>
</body>
</html>`;
}

export function renderError(message: string, lang: Lang = 'en'): string {
  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Trip Dashboard</title>
  <style>${CSS}</style>
</head>
<body>
  <div class="header">
    <h1>Trip Dashboard</h1>
  </div>
  <div class="alert alert-urgent" style="margin-top:24px">
    <span class="alert-icon">\u26A0\uFE0F</span>
    <div class="alert-text">${esc(message)}</div>
  </div>
  <div class="footer">Powered by Turso + Cloudflare Workers</div>
</body>
</html>`;
}
