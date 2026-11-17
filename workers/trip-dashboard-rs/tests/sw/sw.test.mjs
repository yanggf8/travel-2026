// Behavior tests for the offline service worker (src/sw.js) and its kill switch
// (src/sw_kill.js), run with `node --test tests/sw/`. Contract source:
// docs/superpowers/specs/2026-11-17-offline-dashboard-design.md §5–§6 and the
// offline plan Task 3. The miss-page text is the raw {tdMiss*} placeholder here
// (this file loads raw sw.js; the i18n fill is covered by src/sw.rs tests).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ORIGIN,
  VIEWER_CACHE,
  OWNER_CACHE,
  INDEX_CACHE,
  makeCaches,
  switchableFetch,
  mapResolver,
  offline,
  htmlResponse,
  assetResponse,
  deferred,
  req,
  manifestKey,
  loadWorker,
} from './harness.mjs';

const T1 = 1731800000000;
const T2 = 1731800600000;

const viewerUrl = (token = 't1', lang = 'zh') => `${ORIGIN}/?plan=p1&lang=${lang}&token=${token}`;
const ownerUrl = `${ORIGIN}/?plan=p1&lang=zh`;
const mapUrl = (etag) => `${ORIGIN}/map/p1/day1.png${etag ? '?v=' + etag : ''}`;
const voucherUrl = (doc, sha, token) => {
  const parts = [];
  if (sha) parts.push('v=' + sha);
  if (token) parts.push('token=' + token);
  return `${ORIGIN}/voucher/p1/${doc}.pdf${parts.length ? '?' + parts.join('&') : ''}`;
};

// HTML fixtures mirroring what T1/T2 render: versioned local links with the
// raw '&' separator esc_url_attr keeps (an '&amp;' form is tested separately),
// data-view/data-rendered-at/data-served on body.
function planHtml({ view = 'viewer', renderedAt = T1, hrefs = [] } = {}) {
  return (
    `<html><body data-plan-id="p1" data-view="${view}" data-rendered-at="${renderedAt}"` +
    ` data-served="net">${hrefs.join('')}</body></html>`
  );
}
function mapImg(etag) {
  return `<img src="/map/p1/day1.png${etag ? '?v=' + etag : ''}">`;
}
function voucherLink(doc, sha, token, amp = '&') {
  let q = '';
  if (sha && token) q = `?v=${sha}${amp}token=${token}`;
  else if (sha) q = `?v=${sha}`;
  else if (token) q = `?token=${token}`;
  return `<a class="doc-link" href="/voucher/p1/${doc}.pdf${q}">${doc}</a>`;
}

async function setup({ fetchResolver, caches = makeCaches() } = {}) {
  const f = switchableFetch(fetchResolver ?? offline());
  const sw = await loadWorker({ caches, fetch: f });
  return { sw, caches, f };
}

function resolverFor(pages, assets = new Map()) {
  // pages: Map<url, {html, view?, status?}>; assets: Map<url, () => Response|Error>
  const map = new Map();
  for (const [url, p] of pages) map.set(url, () => htmlResponse(p.html, p));
  for (const [url, f] of assets) map.set(url, f);
  return mapResolver(map);
}

test('save_keys_amp_escaped_voucher_link_like_raw', async () => {
  const key = viewerUrl('t1');
  const v = voucherUrl('doc-a', '0123456789ab', 't1');
  const html = planHtml({ hrefs: [voucherLink('doc-a', '0123456789ab', 't1', '&amp;')] });
  const { sw, caches } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html }]]), new Map([
      [v, () => assetResponse('%PDF-a', 'application/pdf')],
    ])),
  });

  const reply = await sw.save(key);

  assert.equal(reply.status, 'saved');
  assert.equal(reply.pdfs, 1);
  assert.ok(caches.log.some((e) => e.op === 'put' && e.key === v), 'voucher cached under the raw-& key');
});

test('save_writes_assets_manifest_html_index_in_order', async () => {
  const key = viewerUrl('t1');
  const m = mapUrl('e1');
  const v = voucherUrl('doc-a', '0123456789ab', 't1');
  const html = planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-a', '0123456789ab', 't1')] });
  const { sw, caches } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html }]]), new Map([
      [m, () => assetResponse('M1')],
      [v, () => assetResponse('%PDF-a', 'application/pdf')],
    ])),
  });

  const reply = await sw.save(key);

  assert.equal(reply.status, 'saved');
  assert.equal(reply.maps, 1);
  assert.equal(reply.pdfs, 1);
  // Put order across ALL caches: assets → manifest → HTML → index.
  const kinds = caches.log
    .filter((e) => e.op === 'put')
    .map((e) => {
      if (e.cache === INDEX_CACHE) return 'index';
      if (e.key.includes('/__td/')) return 'manifest';
      if (e.key.startsWith(ORIGIN + '/?')) return 'html';
      return 'asset';
    });
  assert.deepEqual(kinds, ['asset', 'asset', 'manifest', 'html', 'index']);
});

test('save_rejects_missing_header_or_mismatched_data_view', async () => {
  const key = viewerUrl('t1');
  const seeded = planHtml({ renderedAt: T1 });
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html: seeded }]])),
  });
  await sw.save(key); // complete copy exists

  // Missing X-TD-View header.
  f.set(resolverFor(new Map([[key, { html: planHtml({ renderedAt: T2 }), view: null }]])));
  assert.equal((await sw.save(key)).status, 'failed');
  // Header says viewer but the body says owner.
  f.set(resolverFor(new Map([[key, { html: planHtml({ view: 'owner', renderedAt: T2 }) }]])));
  assert.equal((await sw.save(key)).status, 'failed');

  const viewer = await caches.open(VIEWER_CACHE);
  const html = await (await viewer.match(key)).text();
  assert.ok(html.includes(String(T1)), 'old copy kept');
  assert.ok(!html.includes(String(T2)), 'failed save wrote nothing');
  const manifest = JSON.parse(await (await viewer.match(manifestKey(key))).text());
  assert.equal(manifest.complete, true);
});

test('save_partial_keeps_complete_copy', async () => {
  const key = viewerUrl('t1');
  const m1 = mapUrl('e1');
  const m2 = mapUrl('e2');
  const v1 = voucherUrl('doc-a', null, 't1'); // unversioned: re-fetched (and can fail) on every SAVE
  const v2 = voucherUrl('hotel', null, 't1'); // unversioned voucher
  const orphan = `${ORIGIN}/map/p1/orphan.png?v=zz`;

  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(
      new Map([[key, { html: planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-a', null, 't1'), voucherLink('hotel', null, 't1')] }) }]]),
      new Map([
        [m1, () => assetResponse('M1')],
        [v1, () => assetResponse('%PDF-a', 'application/pdf')],
        [v2, () => assetResponse('%PDF-h', 'application/pdf')],
      ]),
    ),
  });
  await sw.save(key); // all assets OK
  const viewer = await caches.open(VIEWER_CACHE);
  await viewer.put(orphan, assetResponse('ORPHAN')); // unreferenced: sweep would drop it

  // Second save: new map OK, one voucher 500 → partial.
  f.set(resolverFor(
    new Map([[key, { html: planHtml({ renderedAt: T2, hrefs: [mapImg('e2'), voucherLink('doc-a', null, 't1')] }) }]]),
    new Map([
      [m2, () => assetResponse('M2')],
      [v1, () => new Response('boom', { status: 500 })],
    ]),
  ));
  const reply = await sw.save(key);

  assert.equal(reply.status, 'partial-kept');
  assert.deepEqual(reply.failed, [v1]);
  assert.equal(reply.keptRenderedAt, T1);
  // Old HTML and old manifest untouched.
  const html = await (await viewer.match(key)).text();
  assert.ok(html.includes(String(T1)));
  assert.ok(!html.includes(String(T2)));
  const manifest = JSON.parse(await (await viewer.match(manifestKey(key))).text());
  assert.equal(manifest.complete, true);
  assert.equal(manifest.renderedAt, T1);
  // Old assets kept, newly fetched map kept, no sweep (orphan survives).
  for (const k of [m1, v1, v2, m2, orphan]) assert.ok(await viewer.match(k), `kept: ${k}`);
});

test('save_partial_writes_when_no_copy', async () => {
  const key = viewerUrl('t1');
  const m = mapUrl('e1');
  const v = voucherUrl('doc-a', 's1', 't1');
  const { sw, caches } = await setup({
    fetchResolver: resolverFor(
      new Map([[key, { html: planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-a', 's1', 't1')] }) }]]),
      new Map([
        [m, () => assetResponse('M1')],
        [v, () => new TypeError('Failed to fetch: ' + v)],
      ]),
    ),
  });

  const reply = await sw.save(key);

  assert.equal(reply.status, 'partial-new');
  assert.deepEqual(reply.failed, [v]);
  assert.equal(reply.maps, 1);
  assert.equal(reply.pdfs, 0);
  const viewer = await caches.open(VIEWER_CACHE);
  const manifest = JSON.parse(await (await viewer.match(manifestKey(key))).text());
  assert.equal(manifest.complete, false);
  assert.ok(await viewer.match(key), 'partial HTML written');
  assert.ok(await viewer.match(m), 'fetched asset written');
  assert.equal(await viewer.match(v), undefined);
  const idx = await caches.open(INDEX_CACHE);
  assert.ok(await idx.match(key), 'index written');
});

test('save_skips_cached_versioned_asset', async () => {
  const key = viewerUrl('t1');
  const m = mapUrl('e1');
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html: planHtml({ hrefs: [mapImg('e1')] }) }]])),
  });
  const viewer = await caches.open(VIEWER_CACHE);
  await viewer.put(m, assetResponse('CACHED'));

  const reply = await sw.save(key);

  assert.equal(reply.status, 'saved');
  assert.equal(reply.maps, 1);
  assert.equal(f.log.includes(m), false, 'cached ?v= asset not re-fetched');
});

test('save_message_promise_registered_via_waitUntil', async () => {
  const key = viewerUrl('t1');
  const { sw } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html: planHtml({}) }]])),
  });

  const reply = await sw.save(key);
  assert.equal(reply.status, 'saved');

  // The SAVE promise must be tied to the message event's lifetime: without
  // waitUntil the UA may terminate the worker mid-save (phones do).
  const ev = sw.state.messageEvents.at(-1);
  assert.equal(ev._waiters.length, 1, 'SAVE promise passed to event.waitUntil');
  assert.equal(typeof ev._waiters[0].then, 'function');
  await ev._waiters[0]; // it is the real save promise: it settles
});

// R3 item 2: two concurrent SAVEs for the same key run ONE AFTER ANOTHER —
// save 1's whole write set (assets → manifest → HTML → index) is complete
// before save 2 writes anything. Unserialized, the two interleave at every
// await (asset put of save 2 lands before save 1's manifest), which is how a
// retry racing its own timed-out first attempt corrupts a copy.
test('concurrent_saves_to_same_key_run_one_after_another', async () => {
  const key = viewerUrl('t1');
  const m1 = mapUrl('e1');
  const m2 = mapUrl('e2');
  let pageCalls = 0;
  const { sw, caches } = await setup({
    // Each SAVE re-fetches the page: the first save sees the older HTML, the
    // second the newer one (distinguishable assets + renderedAt).
    fetchResolver: (url) => {
      if (url === key) {
        pageCalls += 1;
        return htmlResponse(planHtml({
          renderedAt: pageCalls === 1 ? T1 : T2,
          hrefs: [pageCalls === 1 ? mapImg('e1') : mapImg('e2')],
        }));
      }
      if (url === m1 || url === m2) return assetResponse('M-' + new URL(url).searchParams.get('v'));
      return new TypeError('Failed to fetch: ' + url);
    },
  });

  const [r1, r2] = await Promise.all([sw.save(key), sw.save(key)]);
  assert.equal(r1.status, 'saved');
  assert.equal(r2.status, 'saved');

  const puts = caches.log.filter((e) => e.op === 'put');
  const idxOf = (pred, desc) => {
    const i = puts.findIndex(pred);
    assert.ok(i >= 0, 'missing put: ' + desc);
    return i;
  };
  const firstIndexPut = idxOf((e) => e.cache === INDEX_CACHE, 'index');
  const m2Put = idxOf((e) => e.key === m2, 'save-2 asset');
  // Save 2's first write is AFTER save 1's last write: nothing interleaved.
  assert.ok(m2Put > firstIndexPut, `save 2 interleaved with save 1: asset@${m2Put} vs index@${firstIndexPut}`);
});

// R3 item 2: an older SAVE whose HTML fetch resolves late (the page already
// timed it out and retried) must NOT replace the newer copy the retry wrote
// — not even partially: no HTML, no manifest, no index write, and none of the
// older save's assets are fetched after the guard refuses.
test('late_older_save_does_not_replace_newer_copy', async () => {
  const key = viewerUrl('t1');
  const mNew = mapUrl('e2');
  const mOld = mapUrl('e1');
  const late = deferred(); // save A's page fetch stalls until B is done
  let pageCalls = 0;
  const { sw, caches, f } = await setup({
    fetchResolver: (url) => {
      if (url === key) {
        pageCalls += 1;
        if (pageCalls === 1) return late.promise; // the older save (T1)
        return htmlResponse(planHtml({ renderedAt: T2, hrefs: [mapImg('e2')] }));
      }
      if (url === mNew) return assetResponse('M-NEW');
      if (url === mOld) return assetResponse('M-OLD');
      return new TypeError('Failed to fetch: ' + url);
    },
  });

  const older = sw.save(key); // starts, stalls on its page fetch
  await new Promise((r) => setImmediate(r));
  const newer = await sw.save(key); // retry: completes with the T2 copy
  assert.equal(newer.status, 'saved');
  const viewer = await caches.open(VIEWER_CACHE);
  assert.ok((await (await viewer.match(key)).text()).includes(String(T2)), 'newer copy written');

  late.resolve(htmlResponse(planHtml({ renderedAt: T1, hrefs: [mapImg('e1')] })));
  const lateReply = await older;
  assert.equal(lateReply.status, 'failed', 'a stale save reports failed (the page ignores it by attempt id)');

  // The newer copy is exactly what it was, and the older save wrote nothing.
  const html = await (await viewer.match(key)).text();
  assert.ok(html.includes(String(T2)), 'newer HTML kept');
  assert.ok(!html.includes(String(T1)), 'older HTML not written');
  const manifest = JSON.parse(await (await viewer.match(manifestKey(key))).text());
  assert.equal(manifest.renderedAt, T2);
  const idx = await caches.open(INDEX_CACHE);
  const entry = JSON.parse(await (await idx.match(key)).text());
  assert.equal(entry.key, key);
  // The refused save fetched none of its own assets either.
  assert.equal(f.log.includes(mOld), false, 'older asset never fetched after the guard');
  assert.equal(await viewer.match(mOld), undefined);
});

// R4 item 1: serialization and the rendered-at guard used to be per STORAGE
// key, but owner and viewer saves of the same URL write different storage keys
// while sharing the indexKey — an older viewer SAVE whose page fetch resolved
// after a newer owner SAVE repointed td-index at the older viewer copy, and
// the offline read served that older page. The write section must serialize
// per indexKey, and the index must never be repointed to a copy whose
// renderedAt is older than the one it routes to now.
test('late_older_viewer_save_cannot_repoint_index_off_newer_owner_copy', async () => {
  const url = viewerUrl('t1');
  const mOld = mapUrl('e1');
  const mNew = mapUrl('e2');
  const late = deferred(); // the viewer save's page fetch stalls until the owner save is done
  let pageCalls = 0;
  const { sw, caches, f } = await setup({
    fetchResolver: (u) => {
      if (u === url) {
        pageCalls += 1;
        if (pageCalls === 1) return late.promise; // the older save (viewer, T1)
        return htmlResponse(planHtml({ view: 'owner', renderedAt: T2, hrefs: [mapImg('e2')] }), { view: 'owner' });
      }
      if (u === mNew) return assetResponse('M-NEW');
      if (u === mOld) return assetResponse('M-OLD');
      return new TypeError('Failed to fetch: ' + u);
    },
  });

  const older = sw.save(url); // viewer attempt: stalls on its page fetch
  await new Promise((r) => setImmediate(r));
  const newer = await sw.save(url); // owner attempt: completes with the T2 copy
  assert.equal(newer.status, 'saved');
  const idx = await caches.open(INDEX_CACHE);
  const ownerEntry = JSON.parse(await (await idx.match(url)).text());
  assert.equal(ownerEntry.role, 'owner');

  late.resolve(htmlResponse(planHtml({ renderedAt: T1, hrefs: [mapImg('e1')] })));
  const lateReply = await older;
  assert.equal(lateReply.status, 'failed', 'the older save reports failed');

  const entry = JSON.parse(await (await idx.match(url)).text());
  assert.equal(entry.role, 'owner', 'index still routes this URL to the owner copy');
  assert.equal(entry.key, ownerUrl);
  assert.equal(entry.renderedAt, T2);
  // The refused save wrote nothing of its own and fetched none of its assets.
  assert.equal(f.log.includes(mOld), false, 'older asset never fetched after the guard');
  const viewer = await caches.open(VIEWER_CACHE);
  assert.equal(await viewer.match(url), undefined, 'older viewer copy not written');

  // Offline, the same URL serves the newer OWNER copy.
  f.set(offline());
  const body = await (await sw.fetchEvent(req(url)).response).text();
  assert.match(body, /data-served="cache"/);
  assert.ok(body.includes(String(T2)), 'the owner copy is served');
  assert.ok(!body.includes(String(T1)), 'not the older viewer render');
});

// R4 item 1 compat: index entries written before renderedAt was recorded (an
// older SW build) have no renderedAt — they must behave as 0 and never block a
// newer save from repointing.
test('index_entry_without_rendered_at_is_treated_as_zero', async () => {
  const key = viewerUrl('t1');
  const m = mapUrl('e1');
  const { sw, caches } = await setup({
    fetchResolver: resolverFor(
      new Map([[key, { html: planHtml({ renderedAt: T2, hrefs: [mapImg('e1')] }) }]]),
      new Map([[m, () => assetResponse('M1')]]),
    ),
  });
  const idx = await caches.open(INDEX_CACHE);
  await idx.put(
    key,
    new Response(JSON.stringify({ role: 'viewer', key: key, savedAt: 1 }), {
      headers: { 'Content-Type': 'application/json' },
    }),
  );

  const reply = await sw.save(key);

  assert.equal(reply.status, 'saved', 'a missing renderedAt must not block the save');
  const entry = JSON.parse(await (await idx.match(key)).text());
  assert.equal(entry.renderedAt, T2, "entry refreshed with the saved copy's renderedAt");
  const viewer = await caches.open(VIEWER_CACHE);
  assert.ok((await (await viewer.match(key)).text()).includes(String(T2)), 'new copy written');
});

// R4 item 2: the serialized write section includes the asset fetches — one
// never-settling fetch used to block every later SAVE of the key (and the
// page's retry) forever. Each asset fetch is deadline-bound via
// AbortController; a timed-out asset counts as a failed asset (partial path)
// and the chain moves on. 45001 must stay ASSET_TIMEOUT_MS + 1.
test('hung_asset_fetch_times_out_and_next_save_of_key_completes', async (t) => {
  const key = viewerUrl('t1');
  const m = mapUrl(''); // unversioned: fetched on every SAVE
  const hang = deferred();
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(
      new Map([[key, { html: planHtml({ hrefs: [mapImg('')] }) }]]),
      new Map([[m, () => hang.promise]]),
    ),
  });

  t.mock.timers.enable({ apis: ['setTimeout'] });
  const first = sw.save(key);
  await new Promise((r) => setImmediate(r)); // let the save reach its asset fetch
  t.mock.timers.tick(45001); // ASSET_TIMEOUT_MS elapses → the hung fetch aborts

  const reply1 = await first;
  assert.equal(reply1.status, 'partial-new');
  assert.deepEqual(reply1.failed, [m], 'the timed-out asset counts as failed');
  const viewer = await caches.open(VIEWER_CACHE);
  assert.equal(await viewer.match(m), undefined, 'nothing cached for the hung asset');

  // The chain is free: a second SAVE of the same key completes.
  f.set(resolverFor(
    new Map([[key, { html: planHtml({ renderedAt: T2, hrefs: [mapImg('')] }) }]]),
    new Map([[m, () => assetResponse('M2')]]),
  ));
  const reply2 = await sw.save(key);
  assert.equal(reply2.status, 'saved');
  assert.equal(await (await viewer.match(m)).text(), 'M2');
});

test('unversioned_assets_refetched_and_network_first', async () => {
  const key = viewerUrl('t1');
  const u = mapUrl(''); // no ?v= (empty etag — render/map.rs cache_bust skips it)
  const html = planHtml({ hrefs: [mapImg('')] });
  const { sw, f } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html }]]), new Map([[u, () => assetResponse('MAP-1')]])),
  });
  await sw.save(key);
  f.set(resolverFor(new Map([[key, { html }]]), new Map([[u, () => assetResponse('MAP-2')]])));
  await sw.save(key);
  assert.equal(f.log.filter((x) => x === u).length, 2, 'unversioned asset refetched every SAVE');

  // Online read: network-first gives the network's fresh content.
  f.set(mapResolver(new Map([[u, () => assetResponse('MAP-NET')]])));
  const online = await (await sw.fetchEvent(req(u)).response).text();
  assert.equal(online, 'MAP-NET');

  // Offline read: network fails, cache answers with the last saved copy.
  f.set(offline());
  const offlineRead = await (await sw.fetchEvent(req(u)).response).text();
  assert.equal(offlineRead, 'MAP-2');
});

test('extracts_assets_unescaping_amp', async () => {
  const key = viewerUrl('t1');
  const v = voucherUrl('doc-a', '0123456789ab', 't1');
  const { sw, caches } = await setup({
    fetchResolver: resolverFor(
      new Map([[key, { html: planHtml({ hrefs: [voucherLink('doc-a', '0123456789ab', 't1')] }) }]]),
      new Map([[v, () => assetResponse('%PDF-a', 'application/pdf')]]),
    ),
  });

  await sw.save(key);

  const viewer = await caches.open(VIEWER_CACHE);
  assert.ok(await viewer.match(v), 'cache key is the &-unescaped absolute URL');
});

test('sweep_keeps_asset_referenced_by_other_manifest', async () => {
  const zhT1 = viewerUrl('t1', 'zh');
  const enT1 = viewerUrl('t1', 'en');
  const zhT2 = viewerUrl('t2', 'zh');
  const m = mapUrl('e1');
  const v1 = voucherUrl('doc-a', 's1', 't1');
  const v2 = voucherUrl('doc-b', 's2', 't2');
  const assets = new Map([
    [m, () => assetResponse('M')],
    [v1, () => assetResponse('%PDF-a', 'application/pdf')],
    [v2, () => assetResponse('%PDF-b', 'application/pdf')],
  ]);
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(
      new Map([
        [zhT1, { html: planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-a', 's1', 't1')] }) }],
        [enT1, { html: planHtml({ hrefs: [mapImg('e1')] }) }],
        [zhT2, { html: planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-b', 's2', 't2')] }) }],
      ]),
      assets,
    ),
  });
  await sw.save(zhT1);
  await sw.save(enT1);
  await sw.save(zhT2);

  // zhT1 re-saved with the voucher dropped from the page.
  f.set(resolverFor(
    new Map([[zhT1, { html: planHtml({ hrefs: [mapImg('e1')] }) }]]),
    assets,
  ));
  const reply = await sw.save(zhT1);
  assert.equal(reply.status, 'saved');

  const viewer = await caches.open(VIEWER_CACHE);
  assert.equal(await viewer.match(v1), undefined, 'unreferenced voucher swept');
  assert.ok(await viewer.match(m), 'map shared by other manifests kept');
  assert.ok(await viewer.match(v2), "other token's voucher kept");
});

test('owner_key_drops_token_and_index_finds_owner_copy', async () => {
  const key = viewerUrl('t1'); // owner opens a token URL — role comes from X-TD-View
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html: planHtml({ view: 'owner' }), view: 'owner' }]])),
  });
  await sw.save(key);

  const owner = await caches.open(OWNER_CACHE);
  assert.ok(await owner.match(ownerUrl), 'stored under the token-less owner key');
  assert.equal(await owner.match(key), undefined);
  const idx = await caches.open(INDEX_CACHE);
  const entry = JSON.parse(await (await idx.match(key)).text());
  assert.equal(entry.role, 'owner');
  assert.equal(entry.key, ownerUrl);

  // Offline with the SAME URL finds the owner copy.
  f.set(offline());
  const res = await sw.fetchEvent(req(key)).response;
  const body = await res.text();
  assert.match(body, /data-served="cache"/);
});

test('timeout_serves_cache_marked_cache', async (t) => {
  const key = viewerUrl('t1');
  const { sw, f } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html: planHtml({}) }]])),
  });
  await sw.save(key);

  const d = deferred();
  f.set(() => d.promise); // network slower than NET_TIMEOUT_MS
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const ev = sw.fetchEvent(req(key));
  t.mock.timers.tick(4000);
  const res = await ev.response;
  assert.equal(ev.responded, true);
  assert.match(await res.text(), /data-served="cache"/);
});

test('late_403_with_token_purges', async (t) => {
  const zhT1 = viewerUrl('t1', 'zh');
  const zhT2 = viewerUrl('t2', 'zh');
  const m = mapUrl('e1');
  const va = voucherUrl('doc-a', 's1', 't1');
  const vb = voucherUrl('doc-b', 's2', 't2');
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(
      new Map([
        [zhT1, { html: planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-a', 's1', 't1')] }) }],
        [zhT2, { html: planHtml({ hrefs: [mapImg('e1'), voucherLink('doc-b', 's2', 't2')] }) }],
      ]),
      new Map([
        [m, () => assetResponse('M')],
        [va, () => assetResponse('%PDF-a', 'application/pdf')],
        [vb, () => assetResponse('%PDF-b', 'application/pdf')],
      ]),
    ),
  });
  await sw.save(zhT1);
  await sw.save(zhT2);

  const d = deferred();
  f.set((url) => (url === zhT1 ? d.promise : new TypeError('Failed to fetch: ' + url)));
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const ev = sw.fetchEvent(req(zhT1));
  t.mock.timers.tick(4000);
  const served = await ev.response;
  assert.match(await served.text(), /data-served="cache"/); // cache shown first

  d.resolve(new Response('revoked', { status: 403 })); // 403 arrives late
  await sw.waitIdle();

  const viewer = await caches.open(VIEWER_CACHE);
  const idx = await caches.open(INDEX_CACHE);
  for (const [desc, k] of [['HTML', zhT1], ['manifest', manifestKey(zhT1)], ['voucher', va]]) {
    assert.equal(await viewer.match(k), undefined, `t1 ${desc} purged`);
  }
  assert.equal(await idx.match(zhT1), undefined, 't1 index entry purged');
  // The other token's copy and the shared map survive the sweep.
  assert.ok(await viewer.match(zhT2));
  assert.ok(await viewer.match(vb));
  assert.ok(await viewer.match(m));
  assert.ok(await idx.match(zhT2));
});

test('purge_removes_viewer_copies_even_when_index_points_at_owner', async () => {
  const url = viewerUrl('t1'); // viewer saves, then the owner re-saves the SAME URL
  const m = mapUrl('e1');
  const v = voucherUrl('doc-a', 's1', 't1');
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(
      new Map([[url, { html: planHtml({ view: 'viewer', hrefs: [mapImg('e1'), voucherLink('doc-a', 's1', 't1')] }) }]]),
      new Map([
        [m, () => assetResponse('M')],
        [v, () => assetResponse('%PDF-a', 'application/pdf')],
      ]),
    ),
  });
  await sw.save(url); // viewer copy under the token-ful key
  f.set(resolverFor(
    new Map([[url, { html: planHtml({ view: 'owner', renderedAt: T2, hrefs: [mapImg('e1')] }), view: 'owner' }]]),
    new Map([[m, () => assetResponse('M')]]), // the owner save fetches the map into td-owner
  ));
  await sw.save(url); // owner re-save: td-index now points at the owner copy

  // Token revoked → 403 on the same URL must still purge the old viewer copy.
  f.set(mapResolver(new Map([[url, () => new Response('revoked', { status: 403 })]])));
  const res = await sw.fetchEvent(req(url)).response;
  assert.equal(res.status, 403);

  const viewer = await caches.open(VIEWER_CACHE);
  for (const [desc, k] of [['HTML', url], ['manifest', manifestKey(url)], ['voucher', v], ['map', m]]) {
    assert.equal(await viewer.match(k), undefined, `old viewer ${desc} purged/swept`);
  }
  // The owner copy is untouched: its own cache kept HTML, manifest and map,
  // and the index entry still routes this URL to it.
  const owner = await caches.open(OWNER_CACHE);
  assert.ok(await owner.match(ownerUrl), 'owner HTML kept');
  assert.ok(await owner.match(manifestKey(ownerUrl)), 'owner manifest kept');
  assert.ok(await owner.match(m), 'owner map kept');
  const idx = await caches.open(INDEX_CACHE);
  const entry = JSON.parse(await (await idx.match(url)).text());
  assert.equal(entry.role, 'owner');
  assert.equal(entry.key, ownerUrl);
});

test('owner_403_without_token_serves_relogin_and_keeps_copy', async () => {
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(new Map([[ownerUrl, { html: planHtml({ view: 'owner' }), view: 'owner' }]])),
  });
  await sw.save(ownerUrl);

  f.set(mapResolver(new Map([[ownerUrl, () => new Response('expired', { status: 403 })]])));
  const res = await sw.fetchEvent(req(ownerUrl)).response;
  assert.match(await res.text(), /data-served="cache-relogin"/);
  assert.ok(await (await caches.open(OWNER_CACHE)).match(ownerUrl), 'owner copy kept');
});

test('logout_navigation_clears_owner_only', async () => {
  const { sw, caches } = await setup({
    fetchResolver: resolverFor(
      new Map([
        [ownerUrl, { html: planHtml({ view: 'owner' }), view: 'owner' }],
        [viewerUrl('t1'), { html: planHtml({}) }],
      ]),
    ),
  });
  await sw.save(ownerUrl);
  await sw.save(viewerUrl('t1'));

  const ev = sw.fetchEvent(req(`${ORIGIN}/auth/logout`));
  assert.equal(ev.responded, false, 'logout is not intercepted');
  await sw.waitIdle();

  const owner = await caches.open(OWNER_CACHE);
  assert.equal((await owner.keys()).length, 0, 'td-owner emptied');
  const idx = await caches.open(INDEX_CACHE);
  for (const k of await idx.keys()) {
    const e = JSON.parse(await (await idx.match(k)).text());
    assert.notEqual(e.role, 'owner', 'owner index entries removed');
  }
  const viewer = await caches.open(VIEWER_CACHE);
  assert.ok(await viewer.match(viewerUrl('t1')), 'viewer copy untouched');
  assert.ok((await viewer.keys()).length > 0);
});

test('logout_repoints_index_to_surviving_viewer_copy', async () => {
  const url = viewerUrl('t1');
  const { sw, caches, f } = await setup({
    fetchResolver: resolverFor(new Map([[url, { html: planHtml({ view: 'viewer', renderedAt: T1 }) }]])),
  });
  await sw.save(url); // 1. viewer copy under the token-ful key

  f.set(resolverFor(new Map([[url, { html: planHtml({ view: 'owner', renderedAt: T2 }), view: 'owner' }]])));
  await sw.save(url); // 2. owner re-save of the SAME URL: td-index now points at the owner copy

  sw.fetchEvent(req(`${ORIGIN}/auth/logout`)); // 3. owner logs out
  await sw.waitIdle();

  f.set(offline()); // 4. offline read: the viewer copy must still be reachable
  const body = await (await sw.fetchEvent(req(url)).response).text();
  assert.match(body, /data-served="cache"/);
  assert.ok(body.includes(`data-rendered-at="${T1}"`), 'the viewer copy is served');
  assert.ok(!body.includes(String(T2)), 'not the deleted owner copy');
  const idx = await caches.open(INDEX_CACHE);
  const entry = JSON.parse(await (await idx.match(url)).text());
  assert.equal(entry.role, 'viewer');
  assert.equal(entry.key, url);
  // R4 item 1: the re-pointed entry carries the surviving copy's renderedAt,
  // so the never-repoint-to-older guard keeps working across a logout.
  assert.equal(entry.renderedAt, T1, "the re-pointed entry carries the viewer copy's renderedAt");
});

test('at_param_bypassed', async () => {
  const atUrl = `${ORIGIN}/?plan=p1&lang=zh&at=2026-11-17T10:00`;
  const { sw, caches, f } = await setup();

  const ev = sw.fetchEvent(req(atUrl));
  assert.equal(ev.responded, false, 'navigation with ?at= is not intercepted');

  const reply = await sw.save(atUrl);
  assert.equal(reply.status, 'failed');
  assert.equal(f.log.length, 0, 'SAVE with ?at= fetches nothing');
  for (const name of [VIEWER_CACHE, OWNER_CACHE, INDEX_CACHE]) {
    assert.equal((await (await caches.open(name)).keys()).length, 0, `${name} empty`);
  }
});

test('non_plan_requests_pass_through', async () => {
  const { sw, caches } = await setup();
  const targets = [
    new Request(`${ORIGIN}/notes/x`, { method: 'POST', body: 'hi' }),
    req(`${ORIGIN}/n/abc`),
    req(`${ORIGIN}/grants/create`),
    req(`${ORIGIN}/auth/callback`),
    req(`${ORIGIN}/`),
    req('https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/1/2/3'),
  ];
  for (const r of targets) {
    assert.equal(sw.fetchEvent(r).responded, false, `not intercepted: ${r.url} ${r.method}`);
  }
  for (const name of [VIEWER_CACHE, OWNER_CACHE, INDEX_CACHE]) {
    assert.equal((await (await caches.open(name)).keys()).length, 0, `${name} empty`);
  }
});

test('server_error_falls_back_to_cache', async () => {
  const key = viewerUrl('t1');
  const { sw, f } = await setup({
    fetchResolver: resolverFor(new Map([[key, { html: planHtml({}) }]])),
  });
  await sw.save(key);

  f.set(mapResolver(new Map([[key, () => new Response('bad gateway', { status: 502 })]])));
  const withCopy = await sw.fetchEvent(req(key)).response;
  assert.match(await withCopy.text(), /data-served="cache"/);

  const other = viewerUrl('t9');
  f.set(mapResolver(new Map([[other, () => new Response('bad gateway', { status: 502 })]])));
  const noCopy = await sw.fetchEvent(req(other)).response;
  assert.equal(noCopy.status, 502, 'no copy → the 5xx itself, not the miss page');
  assert.doesNotMatch(await noCopy.text(), /tdMiss/);
});

test('offline_miss_page_in_url_lang', async () => {
  const { sw } = await setup();

  const en = await (await sw.fetchEvent(req(`${ORIGIN}/?plan=p1&lang=en&token=t1`)).response).text();
  assert.match(en, /\{tdMissEn\}/);
  assert.doesNotMatch(en, /tdMissZh/);

  const zh = await (await sw.fetchEvent(req(`${ORIGIN}/?plan=p1&token=t1`)).response).text();
  assert.match(zh, /\{tdMissZh\}/);
  assert.doesNotMatch(zh, /tdMissEn/);
});

test('activate_never_deletes_td_caches', async () => {
  const { sw, caches } = await setup();
  await caches.open(VIEWER_CACHE);
  await caches.open(OWNER_CACHE);
  await caches.open(INDEX_CACHE);

  await sw.install();
  await sw.activate();

  assert.equal(sw.state.skipWaiting, 1);
  assert.equal(sw.state.claims, 1);
  assert.deepEqual(caches.deleted, []);
  for (const name of [VIEWER_CACHE, OWNER_CACHE, INDEX_CACHE]) {
    assert.ok(caches.names().includes(name), `${name} still exists`);
  }
});

test('kill_switch_clears_and_unregisters', async () => {
  const caches = makeCaches();
  for (const name of [VIEWER_CACHE, OWNER_CACHE, INDEX_CACHE, 'site-other']) {
    await caches.open(name);
  }
  const sw = await loadWorker({ file: 'sw_kill.js', caches, fetch: switchableFetch(offline()) });

  await sw.install();
  assert.equal(sw.state.skipWaiting, 1);
  await sw.activate();
  await sw.waitIdle();

  assert.deepEqual([...caches.deleted].sort(), [INDEX_CACHE, OWNER_CACHE, VIEWER_CACHE]);
  assert.ok(caches.names().includes('site-other'), 'non-td cache untouched');
  assert.equal(sw.state.unregistered, 1);
  assert.equal(sw.fetchEvent(req(`${ORIGIN}/?plan=p1`)).responded, false, 'intercepts nothing');
});
