/**
 * Update Osaka-Kyoto plan P5 itinerary in Turso DB.
 *
 * Replaces the existing draft P5 itinerary with the full 5-day Kyoto-based plan:
 *   Day 1 (Feb 24): Arrival & settle in
 *   Day 2 (Feb 25): Kimono day -- Higashiyama
 *   Day 3 (Feb 26): Arashiyama full day
 *   Day 4 (Feb 27): Fushimi Inari & temples
 *   Day 5 (Feb 28): Shopping & departure
 *
 * Usage: npx ts-node scripts/update-osaka-p5-itinerary.ts
 */
import path from 'node:path';

const tursoService = require(path.join(process.cwd(), 'src/services/turso-service'));
const { readPlanFromDb, writePlanToDb } = tursoService;

const PLAN_ID = 'osaka-kyoto-2026';
const DEST_KEY = 'osaka_kyoto_2026';
const NOW = new Date().toISOString();

// ---------------------------------------------------------------------------
// Full 5-day P5 itinerary
// ---------------------------------------------------------------------------

function buildItinerary() {
  return {
    status: 'researched',
    updated_at: NOW,
    days: [
      // -----------------------------------------------------------------------
      // Day 1 (Tue Feb 24) -- ARRIVAL
      // -----------------------------------------------------------------------
      {
        day_number: 1,
        date: '2026-02-24',
        day_type: 'arrival',
        theme: 'Arrival & settle in',
        status: 'planned',
        weather: {
          weather_label: 'Partly cloudy',
          temp_low_c: 1,
          temp_high_c: 9,
          precipitation_pct: 10,
        },
        morning: {
          focus: 'Flight TPE to KIX',
          activities: [
            'Flight TPE 09:00 -> KIX 12:30 (Thai Lion Air)',
          ],
          meals: [],
          transit_notes: 'Thai Lion Air',
        },
        afternoon: {
          focus: 'Immigration + transit to Kyoto',
          activities: [
            'Immigration + baggage claim at KIX',
            'JR Haruka Express to Kyoto Station (~75min)',
            'Check in APA Hotel Kyoto Ekimae',
          ],
          meals: [],
          transit_notes: 'Haruka Express KIX -> Kyoto',
        },
        evening: {
          focus: 'Explore Kyoto Station area',
          activities: [
            'Explore Kyoto Station building',
            'Dinner at Kyoto Station Porta underground or Ramen Koji (10F)',
          ],
          meals: ['Kyoto Station dinner'],
          transit_notes: 'Walk from hotel (~3min)',
        },
      },
      // -----------------------------------------------------------------------
      // Day 2 (Wed Feb 25) -- FULL DAY: Kimono day
      // -----------------------------------------------------------------------
      {
        day_number: 2,
        date: '2026-02-25',
        day_type: 'full',
        theme: 'Kimono day -- Higashiyama',
        status: 'planned',
        weather: {
          weather_label: 'Cloudy',
          temp_low_c: 0,
          temp_high_c: 8,
          precipitation_pct: 15,
        },
        morning: {
          focus: 'Kimono fitting at Yumeyakata',
          activities: [
            {
              title: '京都夢館和服體驗',
              booking_status: 'booked',
              book_by: '2026-02-25',
              booking_url: 'https://www.yumeyakata.com/',
              location: '夢館 五條店, 京都市下京区塩竈町353',
            },
            '女性含髮型設計 (10:00, ~1.5hr fitting + hairstyling)',
          ],
          meals: [],
          transit_notes: 'Walk from hotel ~10min or subway 1 stop to Gojo',
        },
        afternoon: {
          focus: 'Higashiyama walk in kimono',
          activities: [
            '二年坂 (Ninenzaka)',
            '三年坂 (Sannenzaka)',
            'Yasaka Pagoda (八坂の塔)',
            '祇園 (Gion) stroll',
          ],
          meals: ['Lunch at Higashiyama area (おばんざい or matcha cafe)'],
          transit_notes: 'Bus 206/100 from Gojo to Kiyomizu-michi',
        },
        evening: {
          focus: 'Return kimono, Gion evening',
          activities: [
            'Return kimono to 夢館 (check return deadline)',
            'Gion evening stroll — Hanamikoji (花見小路)',
            'Dinner in Gion',
          ],
          meals: ['Gion dinner (kaiseki or izakaya)'],
          transit_notes: 'Bus back to Kyoto Station or taxi',
        },
      },
      // -----------------------------------------------------------------------
      // Day 3 (Thu Feb 26) -- FULL DAY: Arashiyama
      // -----------------------------------------------------------------------
      {
        day_number: 3,
        date: '2026-02-26',
        day_type: 'full',
        theme: 'Arashiyama full day',
        status: 'planned',
        weather: {
          weather_label: 'Sunny',
          temp_low_c: -1,
          temp_high_c: 10,
          precipitation_pct: 5,
        },
        morning: {
          focus: 'Hozugawa River Boat Ride',
          activities: [
            'JR Sagano Line to Kameoka (~25min)',
            '保津川下り (Hozugawa River Boat Ride, ~2hr)',
          ],
          meals: ['Light breakfast at hotel or station'],
          transit_notes: 'JR Sagano Line Kyoto -> Kameoka',
        },
        afternoon: {
          focus: 'Arashiyama sightseeing',
          activities: [
            'Arrive Arashiyama by boat',
            '竹林の小径 (Bamboo Grove)',
            '天龍寺 (Tenryuji Temple, UNESCO)',
          ],
          meals: ['Lunch at Arashiyama (tofu cuisine or yudofu)'],
          transit_notes: 'Walk within Arashiyama area',
        },
        evening: {
          focus: 'Togetsukyo sunset + return',
          activities: [
            '渡月橋 (Togetsukyo Bridge) sunset view',
            'Kimono Forest light pillars',
            'Return to Kyoto Station',
          ],
          meals: ['Dinner at Kyoto Station area'],
          transit_notes: 'JR Sagano Line Saga-Arashiyama -> Kyoto',
        },
      },
      // -----------------------------------------------------------------------
      // Day 4 (Fri Feb 27) -- FULL DAY: Fushimi Inari & temples
      // -----------------------------------------------------------------------
      {
        day_number: 4,
        date: '2026-02-27',
        day_type: 'full',
        theme: 'Fushimi Inari & temples',
        status: 'planned',
        weather: {
          weather_label: 'Partly cloudy',
          temp_low_c: 2,
          temp_high_c: 11,
          precipitation_pct: 10,
        },
        morning: {
          focus: 'Fushimi Inari Taisha',
          activities: [
            '伏見稲荷大社 (Fushimi Inari Taisha) — early start for fewer crowds',
            'Hike to summit or midway viewpoint (~1.5-2hr)',
          ],
          meals: [],
          transit_notes: 'JR Nara Line Kyoto -> Inari (5min, 1 stop)',
        },
        afternoon: {
          focus: 'Kiyomizudera & Higashiyama',
          activities: [
            '清水寺 (Kiyomizudera) — temple, Otowa waterfall, scenic views',
            'Kiyomizu-zaka street food + shopping',
          ],
          meals: ['Lunch at Kiyomizu-zaka street food'],
          transit_notes: 'JR Inari -> Tofukuji, walk or bus to Kiyomizu',
        },
        evening: {
          focus: 'Nishiki Market + Shijo dinner',
          activities: [
            '錦市場 (Nishiki Market) for souvenirs and snacks (closes ~17:00-18:00)',
            'Dinner in Shijo area',
          ],
          meals: ['Nishiki Market snacks + Shijo dinner'],
          transit_notes: 'Bus to Shijo or walk from Kiyomizu',
        },
      },
      // -----------------------------------------------------------------------
      // Day 5 (Sat Feb 28) -- DEPARTURE
      // -----------------------------------------------------------------------
      {
        day_number: 5,
        date: '2026-02-28',
        day_type: 'departure',
        theme: 'Shopping & departure',
        status: 'planned',
        weather: {
          weather_label: 'Cloudy',
          temp_low_c: 3,
          temp_high_c: 13,
          precipitation_pct: 20,
        },
        morning: {
          focus: 'Last shopping + checkout',
          activities: [
            'Last-minute shopping at Kyoto Station area (Isetan dept store, Porta underground)',
            'Pack and checkout from APA Hotel',
          ],
          meals: ['Hotel breakfast or Kyoto Station'],
          transit_notes: 'Walk from hotel (~3min)',
        },
        afternoon: {
          focus: 'Transit to KIX',
          activities: [
            'JR Haruka Express Kyoto -> KIX (~75min)',
            'Arrive KIX for flight',
          ],
          meals: [],
          transit_notes: 'Haruka Express Kyoto -> KIX',
        },
        evening: {
          focus: 'Flight home',
          activities: [
            'Flight KIX 13:30 -> TPE 15:40 (Thai Lion Air)',
          ],
          meals: [],
          transit_notes: 'Thai Lion Air',
        },
      },
    ],
    transit_summary: {
      hotel_station: 'JR Kyoto Station (3min walk from APA Hotel Kyoto Ekimae)',
      key_lines: [
        'JR Haruka Express — KIX <-> Kyoto Station (~75min)',
        'JR Sagano Line — Kyoto -> Kameoka / Saga-Arashiyama',
        'JR Nara Line — Kyoto -> Inari (1 stop, 5min)',
        'Kyoto City Bus — 206/100 for Higashiyama, Kiyomizu area',
        'Subway Karasuma Line — Kyoto Station <-> Gojo (1 stop)',
      ],
    },
    notes: '5-day Kyoto-based trip using LionTravel APA package. Kimono experience included in package (Day 2). Hotel: APA Hotel Kyoto Ekimae, JR Kyoto Station 3min walk. Airport: KIX via Haruka Express. Late Feb weather: highs 5-13C, lows -2 to 5C, mostly dry.',
  };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  console.log('=== Update Osaka-Kyoto P5 Itinerary in DB ===\n');

  // 1. Read from DB
  console.log('1. Reading plan from DB...');
  const row = await readPlanFromDb(PLAN_ID);
  if (!row) {
    console.error(`Plan "${PLAN_ID}" not found in DB.`);
    process.exit(1);
  }
  console.log(`   Found. Last updated: ${row.updated_at}`);

  const plan = JSON.parse(row.plan_json);
  const state = row.state_json ? JSON.parse(row.state_json) : null;
  const dest = plan.destinations?.[DEST_KEY];
  if (!dest) {
    console.error(`Destination "${DEST_KEY}" not found in plan.`);
    process.exit(1);
  }

  // Show current P5 status
  const oldP5 = dest.process_5_daily_itinerary;
  const oldDayCount = oldP5?.days?.length || 0;
  const oldStatus = oldP5?.status || 'unknown';
  console.log(`   Current P5: status=${oldStatus}, days=${oldDayCount}`);

  // 2. Replace P5 itinerary
  console.log('\n2. Replacing P5 itinerary with full 5-day plan...');
  const newP5 = buildItinerary();
  dest.process_5_daily_itinerary = newP5;
  console.log(`   New P5: status=${newP5.status}, days=${newP5.days.length}`);
  for (const day of newP5.days) {
    console.log(`   Day ${day.day_number} (${day.date}): ${day.theme} [${day.day_type}]`);
  }

  // 3. Update state if present
  if (state) {
    console.log('\n3. Updating state...');
    state.current_focus = `${DEST_KEY}.process_5_daily_itinerary`;
    state.next_actions = [
      'book_hozugawa_river_boat',
      'add_restaurant_reservations',
      'confirm_weather_closer_to_date',
    ];

    if (state.destinations?.[DEST_KEY]?.processes?.process_5_daily_itinerary) {
      state.destinations[DEST_KEY].processes.process_5_daily_itinerary.state = 'researched';
    }

    state.event_log = state.event_log || [];
    state.event_log.push({
      event: 'p5_itinerary_updated',
      at: NOW,
      destination: DEST_KEY,
      process: 'process_5_daily_itinerary',
      data: {
        days: 5,
        themes: newP5.days.map((d: { theme: string }) => d.theme),
        hotel: 'APA Hotel Kyoto Ekimae',
        airport: 'KIX',
      },
    });
    console.log('   State updated with itinerary event.');
  } else {
    console.log('\n3. No state_json found — skipping state update.');
  }

  // 4. Write back to DB
  console.log('\n4. Writing updated plan to DB...');
  const planJson = JSON.stringify(plan);
  const stateJson = state ? JSON.stringify(state) : null;
  await writePlanToDb(PLAN_ID, planJson, stateJson, plan.schema_version || '4.2.0');
  console.log('   Written successfully.');

  // 5. Verify
  console.log('\n5. Verifying...');
  const verifyRow = await readPlanFromDb(PLAN_ID);
  if (!verifyRow) {
    console.error('   VERIFY FAILED: Plan not found after write!');
    process.exit(1);
  }
  const verifyPlan = JSON.parse(verifyRow.plan_json);
  const vDest = verifyPlan.destinations?.[DEST_KEY];
  const vP5 = vDest?.process_5_daily_itinerary;

  const checks: Array<{ label: string; ok: boolean; detail: string }> = [];

  // P5 status
  checks.push({
    label: 'P5 status is researched',
    ok: vP5?.status === 'researched',
    detail: `status=${vP5?.status}`,
  });

  // Day count
  checks.push({
    label: 'P5 has 5 days',
    ok: vP5?.days?.length === 5,
    detail: `days=${vP5?.days?.length}`,
  });

  // Day structure checks
  const dayTypes = ['arrival', 'full', 'full', 'full', 'departure'];
  const dayDates = ['2026-02-24', '2026-02-25', '2026-02-26', '2026-02-27', '2026-02-28'];

  for (let i = 0; i < 5; i++) {
    const d = vP5?.days?.[i];
    checks.push({
      label: `Day ${i + 1} day_number=${i + 1}`,
      ok: d?.day_number === i + 1,
      detail: `day_number=${d?.day_number}`,
    });
    checks.push({
      label: `Day ${i + 1} date=${dayDates[i]}`,
      ok: d?.date === dayDates[i],
      detail: `date=${d?.date}`,
    });
    checks.push({
      label: `Day ${i + 1} day_type=${dayTypes[i]}`,
      ok: d?.day_type === dayTypes[i],
      detail: `day_type=${d?.day_type}`,
    });
    checks.push({
      label: `Day ${i + 1} has morning session`,
      ok: typeof d?.morning === 'object' && d?.morning !== null,
      detail: `morning=${typeof d?.morning}`,
    });
    checks.push({
      label: `Day ${i + 1} has afternoon session`,
      ok: typeof d?.afternoon === 'object' && d?.afternoon !== null,
      detail: `afternoon=${typeof d?.afternoon}`,
    });
    checks.push({
      label: `Day ${i + 1} has evening session`,
      ok: typeof d?.evening === 'object' && d?.evening !== null,
      detail: `evening=${typeof d?.evening}`,
    });
  }

  // Kimono activity object on Day 2
  const day2Morning = vP5?.days?.[1]?.morning;
  const kimonoActivity = day2Morning?.activities?.find(
    (a: unknown) => typeof a === 'object' && a !== null && (a as Record<string, unknown>).title === '京都夢館和服體驗'
  );
  checks.push({
    label: 'Day 2 has kimono activity object',
    ok: kimonoActivity != null,
    detail: `found=${kimonoActivity != null}`,
  });
  checks.push({
    label: 'Kimono booking_status is booked',
    ok: kimonoActivity?.booking_status === 'booked',
    detail: `booking_status=${kimonoActivity?.booking_status}`,
  });
  checks.push({
    label: 'Kimono has location',
    ok: typeof kimonoActivity?.location === 'string' && kimonoActivity.location.includes('五條店'),
    detail: `location=${kimonoActivity?.location}`,
  });

  // Transit summary
  checks.push({
    label: 'Has transit_summary',
    ok: typeof vP5?.transit_summary === 'object' && vP5?.transit_summary !== null,
    detail: `transit_summary=${typeof vP5?.transit_summary}`,
  });
  checks.push({
    label: 'transit_summary has hotel_station',
    ok: typeof vP5?.transit_summary?.hotel_station === 'string',
    detail: `hotel_station=${vP5?.transit_summary?.hotel_station}`,
  });
  checks.push({
    label: 'transit_summary has key_lines',
    ok: Array.isArray(vP5?.transit_summary?.key_lines) && vP5.transit_summary.key_lines.length > 0,
    detail: `key_lines count=${vP5?.transit_summary?.key_lines?.length}`,
  });

  // Print results
  console.log('');
  let passed = 0;
  let failed = 0;
  for (const c of checks) {
    const icon = c.ok ? 'PASS' : 'FAIL';
    console.log(`   [${icon}] ${c.label} (${c.detail})`);
    if (c.ok) passed++;
    else failed++;
  }

  console.log(`\n=== Results: ${passed}/${checks.length} passed, ${failed} failed ===`);

  if (failed > 0) {
    process.exit(1);
  }

  console.log('\nDone! Osaka-Kyoto P5 itinerary updated and verified in DB.');
}

main().catch((err) => {
  console.error('Script failed:', err);
  process.exit(1);
});
