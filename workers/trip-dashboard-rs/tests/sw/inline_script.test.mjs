// Behavior tests for BLOCK_MEMORY_SCRIPT (src/render/mod.rs) — offline-t4
// Round 2, K3.1 Pro review item 3, extended in Round 3. The Round 1 Rust
// tests are substring checks; these EXECUTE the script against a minimal
// fake document/navigator (inline_dom.mjs) so the review findings can
// actually bite:
//
//   R2-1. stale now card: every today marker (data-today / .is-today /
//      .toc-today) moves to the computed today — exactly one [data-today]
//      remains (T6 stale-day, plan 2026-11-17-offline-dashboard.md:399-403);
//   R2-2. the SAVE wait times out to failed + retry, and a late reply from an
//      older attempt never overwrites a newer attempt's state;
//   R2-3. plus the regression contract: no SAVE on cache-served / ?at= pages,
//      exactly one {type:'SAVE',url} on net, reply → state/retry semantics,
//      no serviceWorker → unsupported, and SW/Intl failures leave the
//      existing TOC collapse features working.
//   R3-1. today-marking does not depend on #sec-now: body data-tz wins,
//      #sec-now data-now-tz is the fallback, neither → nothing; the label is
//      ADDED when the saved page carried none (trip day 1 after a pre-trip
//      save);
//   R3-3. a register() that never settles still ends in failed + retry;
//   R3-4. "today" is built from formatToParts, never format() — proven with
//      a liar Intl whose format() is non-ISO;
//   R3-5. crossing destination midnight in the per-minute remark re-runs the
//      stale/today handling.
//
// No browser behaviour is claimed here — real browsers are T6/T7.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildPage, makeEnv, expectedTime, SAVE_TIMEOUT_MS, scriptSource } from './inline_dom.mjs';

const URL_NET = 'https://td.test/?plan=p1&lang=zh&token=t0';
const ANCHOR_URL = 'https://td.test/voucher/p1/hotel.pdf?v=0123456789ab&token=t0';
const RENDERED_AT = Date.UTC(2026, 10, 17, 20, 5); // 2026-11-18 05:05 in Tokyo
const KEPT_AT = Date.UTC(2026, 10, 16, 21, 5); // 2026-11-17 06:05 in Tokyo

// The SSR's REAL date shape (Round 3 item 4): `data-now-date` is assembled
// by tzfmt.rs clock_from_parts as zero-padded YYYY-MM-DD, and a day card's
// `data-date` is the plan_days date — same strict ISO. Fixtures are built
// the same way (formatToParts + pad), NEVER with the script's own
// formatter, so a script/SSR shape mismatch cannot hide.
function isoDateIn(tz, date = new Date()) {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone: tz,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).formatToParts(date);
  const get = (t) => parts.find((p) => p.type === t)?.value ?? '';
  return `${get('year').padStart(4, '0')}-${get('month').padStart(2, '0')}-${get('day').padStart(2, '0')}`;
}
function todayIn(tz) {
  return isoDateIn(tz);
}
function dayBefore(tz) {
  return isoDateIn(tz, new Date(Date.now() - 24 * 3600 * 1000));
}
function dayAfter(tz) {
  return isoDateIn(tz, new Date(Date.now() + 24 * 3600 * 1000));
}

async function runNet(reply) {
  const page = buildPage({
    served: 'net',
    renderedAt: String(RENDERED_AT),
    anchors: [{ href: ANCHOR_URL, text: '飯店確認單' }],
  });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();
  if (reply) await env.deliver(0, reply);
  return { page, env };
}

// ---------------------------------------------------------------------------
// SAVE is sent only on a net-served, unpinned open
// ---------------------------------------------------------------------------

test('cache_served_page_sends_no_save_and_shows_banner', async () => {
  const page = buildPage({ served: 'cache', renderedAt: String(RENDERED_AT) });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();
  assert.equal(env.sw.messages.length, 0, 'no SAVE from a cache-served page');
  assert.equal(page.status.getAttribute('data-state'), '', 'status untouched');
  assert.equal(page.status.hidden, true, 'status bar stays hidden');
  assert.equal(page.banner.hidden, false, 'banner revealed');
  // {time} = data-rendered-at formatted in data-tz — never a guessed zone.
  assert.equal(
    page.banner.textContent,
    '離線副本 · ' + expectedTime('zh-Hant', 'Asia/Tokyo', RENDERED_AT) + ' 儲存，可能不是最新'
  );
});

test('cache_page_without_tz_strips_time_no_guess', async () => {
  const page = buildPage({ served: 'cache', renderedAt: String(RENDERED_AT), tz: null });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();
  assert.equal(env.sw.messages.length, 0);
  assert.equal(page.banner.hidden, false);
  // No data-tz → the {time}-less copy; the zone is never guessed.
  assert.equal(page.banner.textContent, '離線副本 · {time} 儲存，可能不是最新'.split('{time}').join(''));
});

test('cache_relogin_page_uses_relogin_copy', async () => {
  const page = buildPage({ served: 'cache-relogin', renderedAt: String(RENDERED_AT) });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();
  assert.equal(env.sw.messages.length, 0);
  assert.equal(page.banner.hidden, false);
  assert.equal(page.banner.textContent, '離線副本 · 需要重新登入才能更新');
});

test('at_pinned_page_sends_no_save', async () => {
  const page = buildPage({ served: 'net' });
  const env = makeEnv({ page, url: 'https://td.test/?plan=p1&at=2026-11-17T09%3A00' });
  await env.run();
  assert.deepEqual(env.sw.registerCalls, ['/sw.js'], 'registration still happens');
  assert.equal(env.sw.messages.length, 0, 'a pinned page must never SAVE (spec §6)');
  assert.equal(page.status.getAttribute('data-state'), '', 'status untouched');
  assert.equal(page.status.hidden, true);
  assert.equal(page.banner.hidden, true, 'net page shows no banner');
});

test('net_page_sends_exactly_one_save', async () => {
  const { page, env } = await runNet();
  assert.deepEqual(env.sw.registerCalls, ['/sw.js']);
  assert.deepEqual(env.sw.messages, [{ type: 'SAVE', url: URL_NET }]);
  assert.equal(env.sw.persistCalls, 1, 'navigator.storage.persist attempted');
  assert.equal(page.status.hidden, false);
  assert.equal(page.status.getAttribute('data-state'), 'saving');
  assert.equal(page.msgEl.textContent, '正在存到本機…');
  assert.equal(page.retry.hidden, true, 'retry hidden while saving');
});

// ---------------------------------------------------------------------------
// Replies → data-state and retry-button visibility
// ---------------------------------------------------------------------------

test('saved_reply_sets_saved_state_and_keeps_retry_hidden', async () => {
  const { page } = await runNet({ status: 'saved', maps: 2, pdfs: 3 });
  assert.equal(page.status.getAttribute('data-state'), 'saved');
  assert.equal(page.msgEl.textContent, '已存好，可離線查看（地圖 2、票券 3）');
  assert.equal(page.retry.hidden, true, 'retry only for partial/failed');
  assert.equal(page.ul.hidden, true, 'no failed list on saved');
});

test('partial_new_reply_lists_failures_and_unhides_retry', async () => {
  const unmatched = 'https://td.test/map/p1/zz.png?v=e';
  const { page } = await runNet({ status: 'partial-new', maps: 1, pdfs: 0, failed: [ANCHOR_URL, unmatched] });
  assert.equal(page.status.getAttribute('data-state'), 'partial');
  assert.equal(page.msgEl.textContent, '已存，但 2 項未存：');
  assert.equal(page.retry.hidden, false);
  assert.equal(page.ul.hidden, false);
  const items = page.ul.querySelectorAll('li');
  assert.equal(items.length, 2);
  // A known a[href] is named by its textContent; an unknown URL stays a URL.
  assert.equal(items[0].textContent, '飯店確認單');
  assert.equal(items[1].textContent, unmatched);
});

test('partial_kept_reply_uses_kept_time_and_unhides_retry', async () => {
  const { page } = await runNet({ status: 'partial-kept', failed: [ANCHOR_URL], keptRenderedAt: KEPT_AT });
  assert.equal(page.status.getAttribute('data-state'), 'kept');
  assert.equal(
    page.msgEl.textContent,
    '這次更新沒存完，離線時會看到 ' + expectedTime('zh-Hant', 'Asia/Tokyo', KEPT_AT) + ' 的版本'
  );
  assert.equal(page.retry.hidden, false);
  assert.equal(page.ul.querySelectorAll('li').length, 1);
});

test('failed_reply_sets_failed_and_unhides_retry', async () => {
  const { page } = await runNet({ status: 'failed' });
  assert.equal(page.status.getAttribute('data-state'), 'failed');
  assert.equal(page.msgEl.textContent, '沒存好，離線時看不到這一頁');
  assert.equal(page.retry.hidden, false);
  assert.equal(page.ul.hidden, true, 'send-time empty list stays hidden');
});

// ---------------------------------------------------------------------------
// R2 item 2: bounded SAVE wait + attempt-id guard
// ---------------------------------------------------------------------------

test('save_reply_timeout_shows_failed_and_retry', async () => {
  const { page, env } = await runNet(); // SW never replies
  assert.equal(page.status.getAttribute('data-state'), 'saving');
  await env.advance(SAVE_TIMEOUT_MS);
  assert.equal(page.status.getAttribute('data-state'), 'failed', 'timeout must not park at saving');
  assert.equal(page.msgEl.textContent, '沒存好，離線時看不到這一頁');
  assert.equal(page.retry.hidden, false, 'retry available after timeout');
});

test('late_reply_from_older_attempt_is_ignored', async () => {
  const { page, env } = await runNet();
  await env.advance(SAVE_TIMEOUT_MS); // attempt 1 times out → failed
  assert.equal(page.status.getAttribute('data-state'), 'failed');
  await env.clickRetry(); // attempt 2 → saving again
  assert.equal(page.status.getAttribute('data-state'), 'saving');
  assert.equal(env.sw.messages.length, 2, 'retry re-sends SAVE');
  await env.deliver(0, { status: 'saved', maps: 9, pdfs: 9 }); // attempt 1's late reply
  assert.equal(page.status.getAttribute('data-state'), 'saving', 'old attempt must not overwrite');
  assert.equal(page.msgEl.textContent, '正在存到本機…');
  await env.deliver(1, { status: 'failed' }); // attempt 2's reply
  assert.equal(page.status.getAttribute('data-state'), 'failed');
});

// ---------------------------------------------------------------------------
// Feature detection
// ---------------------------------------------------------------------------

test('no_service_worker_shows_unsupported', async () => {
  const page = buildPage({ served: 'net' });
  const env = makeEnv({ page, url: URL_NET, swKind: 'none' });
  await env.run();
  assert.equal(page.status.getAttribute('data-state'), 'unsupported');
  assert.equal(page.status.hidden, false);
  assert.equal(page.msgEl.textContent, '這個瀏覽器不能離線保存，請用 Safari／Chrome 開啟');
  assert.equal(page.retry.hidden, true);
  assert.equal(env.sw.messages.length, 0);
});

// ---------------------------------------------------------------------------
// R2 item 1: stale now card — today markers move to the computed today
// ---------------------------------------------------------------------------

test('stale_now_card_moves_every_today_marker', async () => {
  const tdy = todayIn('Asia/Tokyo');
  const yda = dayBefore('Asia/Tokyo');
  const page = buildPage({
    served: 'cache', // the real stale scenario: an offline re-open
    now: { date: yda, tz: 'Asia/Tokyo' },
    days: [
      { n: 1, date: yda, today: true }, // SSR's "today" — yesterday now
      { n: 2, date: tdy, today: false, open: false }, // remembered collapsed
    ],
    lsMap: new Map([['trip-blocks:p1', 'day-2=0']]),
  });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();

  assert.equal(page.nowCard.hidden, true, 'stale now card hidden');
  // Exactly one [data-today] — on the computed today's day card (T6 (c)+(e)).
  const marked = page.doc.querySelectorAll('[data-today]');
  assert.equal(marked.length, 1);
  assert.equal(marked[0], page.dayDetails[1]);
  assert.equal(page.dayDetails[1].hasAttribute('data-today'), true);
  assert.equal(page.dayDetails[0].hasAttribute('data-today'), false, 'yesterday unmarked');
  // Today's card is opened — over the reader's remembered collapsed state.
  assert.equal(page.dayDetails[1].open, true);
  // TOC: is-today class moved…
  assert.equal(page.tocLinks[1].classList.contains('is-today'), true);
  assert.equal(page.tocLinks[0].classList.contains('is-today'), false);
  // …and the 今天 label span moved with it, first child of the new .toc-day-n.
  const spans = page.doc.querySelectorAll('.toc-today');
  assert.equal(spans.length, 1);
  const n = page.tocLinks[1].querySelector('.toc-day-n');
  assert.equal(spans[0].parentNode, n);
  assert.equal(n.firstChild, spans[0]);
  // The per-minute remark is never registered on a stale card.
  assert.equal(env.timers.pendingIntervals(), 0);
  assert.equal(env.sw.messages.length, 0, 'cache page never SAVEs');
});

test('stale_now_card_without_today_card_only_hides', async () => {
  const page = buildPage({
    served: 'cache',
    now: { date: '2020-01-01', tz: 'Asia/Tokyo' }, // long stale
    days: [{ n: 1, date: '2020-01-01', today: true }], // no card for today
  });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();
  assert.equal(page.nowCard.hidden, true, 'now card still hidden');
  // No today card → remove no markers; the old marking stays as served.
  assert.equal(page.dayDetails[0].hasAttribute('data-today'), true);
  assert.equal(page.doc.querySelectorAll('[data-today]').length, 1);
  assert.equal(page.tocLinks[0].classList.contains('is-today'), true);
  const spans = page.doc.querySelectorAll('.toc-today');
  assert.equal(spans.length, 1);
  assert.equal(spans[0].parentNode, page.tocLinks[0].querySelector('.toc-day-n'));
  assert.equal(env.timers.pendingIntervals(), 0);
});

// ---------------------------------------------------------------------------
// R3 item 1: today-marking must not depend on #sec-now
// ---------------------------------------------------------------------------

// Trip day 1 opened offline when yesterday was OUTSIDE the trip: the saved
// HTML has no now card and no today markers at all (SSR only marks during
// the trip). Today is still marked — data-today, is-today and a CREATED
// .toc-today label (its text comes from the page's filled-in i18n label,
// never a CJK literal in the script).
test('today_marked_without_now_card_and_without_existing_markers', async () => {
  const tdy = todayIn('Asia/Tokyo');
  const page = buildPage({
    served: 'cache',
    days: [
      { n: 1, date: dayBefore('Asia/Tokyo'), today: false }, // yesterday: outside trip, unmarked
      { n: 2, date: tdy, today: false, open: false }, // today: also unmarked
    ],
    lsMap: new Map([['trip-blocks:p1', 'day-2=0']]),
  });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();

  // Exactly one [data-today] — on today's day card, which is opened over the
  // remembered collapsed state (T6 stale-day (c)+(e)).
  const marked = page.doc.querySelectorAll('[data-today]');
  assert.equal(marked.length, 1);
  assert.equal(marked[0], page.dayDetails[1]);
  assert.equal(page.dayDetails[1].open, true);
  assert.equal(page.dayDetails[0].hasAttribute('data-today'), false);
  assert.equal(page.tocLinks[1].classList.contains('is-today'), true);
  assert.equal(page.tocLinks[0].classList.contains('is-today'), false);
  // The label was ADDED (nothing to move): first child of the new .toc-day-n,
  // with the page language's today text.
  const spans = page.doc.querySelectorAll('.toc-today');
  assert.equal(spans.length, 1);
  const n = page.tocLinks[1].querySelector('.toc-day-n');
  assert.equal(spans[0].parentNode, n);
  assert.equal(n.firstChild, spans[0]);
  assert.equal(spans[0].textContent, '今天');
  assert.equal(page.nowCard, null, 'fixture really has no now card');
});

// The zone comes from the BODY's data-tz; #sec-now's data-now-tz is only a
// fallback. 02:00 UTC: Tokyo has already crossed to its date, New York has
// not — so the two zones disagree on "today" and the winner is observable.
test('body_data_tz_wins_over_now_card_tz', async () => {
  const base = new Date();
  const utc2am = Date.UTC(base.getUTCFullYear(), base.getUTCMonth(), base.getUTCDate(), 2);
  const tdyTokyo = isoDateIn('Asia/Tokyo', new Date(utc2am));
  const tdyNy = isoDateIn('America/New_York', new Date(utc2am));
  assert.notEqual(tdyTokyo, tdyNy, 'fixture must pick an instant where the zones disagree');

  const page = buildPage({
    served: 'cache',
    tz: 'Asia/Tokyo', // body wins
    now: { date: tdyNy, tz: 'America/New_York' }, // "today" in NY — stale in Tokyo
    days: [
      { n: 1, date: tdyNy, today: true }, // marked for the NY date
      { n: 2, date: tdyTokyo, today: false, open: false },
    ],
  });
  const env = makeEnv({ page, url: URL_NET, clockMs: utc2am });
  await env.run();

  // Tokyo's today: now card stale (its NY date < Tokyo today) → hidden, no
  // remark; markers moved to the Tokyo-today card.
  assert.equal(page.nowCard.hidden, true, 'stale per the BODY zone');
  assert.equal(env.timers.pendingIntervals(), 0);
  const marked = page.doc.querySelectorAll('[data-today]');
  assert.equal(marked.length, 1);
  assert.equal(marked[0], page.dayDetails[1]);
  assert.equal(page.dayDetails[1].open, true);
});

// No body data-tz but the now card has one → the fallback still drives the
// stale handling (the R2 behavior, kept).
test('now_card_tz_used_when_body_has_none', async () => {
  const tdy = todayIn('Asia/Tokyo');
  const page = buildPage({
    served: 'cache',
    tz: null,
    now: { date: dayBefore('Asia/Tokyo'), tz: 'Asia/Tokyo' },
    days: [
      { n: 1, date: dayBefore('Asia/Tokyo'), today: true },
      { n: 2, date: tdy, today: false, open: false },
    ],
  });
  const env = makeEnv({ page, url: URL_NET });
  await env.run();
  assert.equal(page.nowCard.hidden, true, 'fallback zone still detects stale');
  const marked = page.doc.querySelectorAll('[data-today]');
  assert.equal(marked.length, 1);
  assert.equal(marked[0], page.dayDetails[1]);
});

// Neither zone → do nothing, never guess (no crash, no marker changes).
test('no_timezone_anywhere_does_nothing', async () => {
  const page = buildPage({
    served: 'cache',
    tz: null,
    days: [{ n: 1, date: todayIn('Asia/Tokyo'), today: false, open: false }],
  });
  const env = makeEnv({ page, url: URL_NET });
  await env.run(); // must not throw
  assert.equal(page.doc.querySelectorAll('[data-today]').length, 0, 'nothing marked');
  assert.equal(page.dayDetails[0].open, false, 'nothing opened');
  assert.equal(page.doc.querySelectorAll('.toc-today').length, 0, 'no label invented');
});

// ---------------------------------------------------------------------------
// R3 item 3: a register() that never settles still ends in failed + retry
// ---------------------------------------------------------------------------

test('register_never_settles_times_out_to_failed_and_retry', async () => {
  const page = buildPage({ served: 'net' });
  const env = makeEnv({ page, url: URL_NET, swKind: 'registerNever' });
  await env.run();
  // The wait starts BEFORE register(): saving is already visible while the
  // registration is pending…
  assert.equal(page.status.hidden, false);
  assert.equal(page.status.getAttribute('data-state'), 'saving');
  assert.equal(env.sw.messages.length, 0, 'no SAVE before registration resolves');
  // …and a registration that never settles surfaces failed + retry.
  await env.advance(SAVE_TIMEOUT_MS);
  assert.equal(page.status.getAttribute('data-state'), 'failed');
  assert.equal(page.msgEl.textContent, '沒存好，離線時看不到這一頁');
  assert.equal(page.retry.hidden, false, 'retry available');
});

// ---------------------------------------------------------------------------
// R3 item 4: "today" comes from formatToParts, never format()
// ---------------------------------------------------------------------------

// The liar Intl keeps formatToParts real but makes format() return M/D/YYYY:
// a script comparing format() output to the SSR's ISO dates never matches,
// while the formatToParts-built date keeps working.
test('today_computed_from_formatTo_parts_not_format', async () => {
  const tdy = todayIn('Asia/Tokyo');
  const page = buildPage({
    served: 'cache',
    now: { date: dayBefore('Asia/Tokyo'), tz: 'Asia/Tokyo' },
    days: [
      { n: 1, date: dayBefore('Asia/Tokyo'), today: true },
      { n: 2, date: tdy, today: false, open: false },
    ],
  });
  const env = makeEnv({ page, url: URL_NET, intlLiar: true });
  await env.run();
  assert.equal(page.nowCard.hidden, true, 'stale detected via formatToParts');
  const marked = page.doc.querySelectorAll('[data-today]');
  assert.equal(marked.length, 1);
  assert.equal(marked[0], page.dayDetails[1]);
  assert.equal(page.dayDetails[1].open, true);
});

// ---------------------------------------------------------------------------
// R3 item 5: crossing destination midnight re-runs the stale/today handling
// ---------------------------------------------------------------------------

test('midnight_crossing_reruns_stale_and_today_handling', async () => {
  const tdy = todayIn('Asia/Tokyo');
  // Hold the page at 23:30 Tokyo (a current, non-stale now card)…
  const [Y, M, D] = tdy.split('-').map(Number);
  const at2330 = Date.UTC(Y, M - 1, D, 23, 30) - 9 * 3600 * 1000;
  const page = buildPage({
    served: 'net',
    now: { date: tdy, tz: 'Asia/Tokyo' },
    days: [
      { n: 1, date: tdy, today: true }, // today, marked, remark active
      { n: 2, date: dayAfter('Asia/Tokyo'), today: false, open: false }, // tomorrow's trip day
    ],
  });
  const env = makeEnv({ page, url: URL_NET, clockMs: at2330 });
  await env.run();
  assert.equal(page.nowCard.hidden, false, 'current at 23:30');
  assert.equal(page.dayDetails[0].hasAttribute('data-today'), true);

  // …then let the per-minute remark cross midnight in the destination zone.
  await env.advance(31 * 60 * 1000);
  assert.equal(page.nowCard.hidden, true, 'now card hidden after midnight');
  const marked = page.doc.querySelectorAll('[data-today]');
  assert.equal(marked.length, 1);
  assert.equal(marked[0], page.dayDetails[1], "markers moved to the new day's card");
  assert.equal(page.dayDetails[1].open, true);
  assert.equal(page.tocLinks[1].classList.contains('is-today'), true);
  assert.equal(page.tocLinks[0].classList.contains('is-today'), false);
});

// ---------------------------------------------------------------------------
// Resilience: SW/Intl failures never break the pre-existing features
// ---------------------------------------------------------------------------

test('sw_and_intl_failures_leave_toc_features_working', async () => {
  const page = buildPage({
    served: 'net',
    now: { date: dayBefore('Asia/Tokyo'), tz: 'Asia/Tokyo' }, // would be stale
    days: [
      { n: 1, date: dayBefore('Asia/Tokyo'), today: true },
      { n: 2, date: todayIn('Asia/Tokyo'), today: false },
    ],
  });
  const env = makeEnv({ page, url: URL_NET, swKind: 'registerThrows', noIntl: true });
  await env.run(); // must not throw
  for (const [i, btn] of page.tocBtns.entries()) {
    assert.equal(btn.hidden, false, `toc collapse button ${i} still un-hidden`);
  }
  // register() threw synchronously → unsupported, not a broken page.
  assert.equal(page.status.getAttribute('data-state'), 'unsupported');
  // Intl gone → the stale check failed gracefully: card stays, remark runs.
  assert.equal(page.nowCard.hidden, false);
  assert.equal(env.timers.pendingIntervals(), 1, 'remark interval registered (non-stale path)');
});

// Self-check: the extraction really pulled the script out of mod.rs (an
// empty or truncated body would make every test above vacuously pass).
test('script_source_extracted_is_the_real_iife', () => {
  const src = scriptSource();
  assert.ok(src.startsWith('(function(){'), 'extracted body starts with the IIFE');
  assert.ok(src.includes('trip-blocks:'));
  assert.ok(src.includes('serviceWorker'));
  assert.ok(src.length > 3000);
});
