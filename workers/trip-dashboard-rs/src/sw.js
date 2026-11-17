// Service worker for offline copies of the trip plan page. Served same-origin
// at /sw.js; the {tdMissZh}/{tdMissEn} placeholders are the offline-miss page
// text, filled from i18n.rs when the worker serves this file (src/sw.rs
// script()). No CJK literals live here — UI text arrives filled.
//
// Layout (spec 2026-11-17-offline-dashboard-design.md sections 5-6):
//   td-viewer / td-owner — HTML under the canonical page key, assets under
//     their absolute URLs, and a per-key manifest at /__td/manifest?k=...
//   td-index             — normalized request URL -> {role, key, savedAt}
//
// Interception: ONLY same-origin GET plan navigations (/?plan=), /map/ and
// /voucher/. Everything else — non-GET, cross-origin, /auth/*, /grants/*,
// /diag/*, /notes*, /n/*, bare / — is never respondWith'd. Requests with ?at=
// are skipped entirely (no read, no write). /auth/logout navigations clear the
// owner copies in waitUntil without being intercepted.
// ES2017 (async/await) is fine: every SW-capable browser has it.

var KEY_PARAMS=['plan','lang','token'];
var CACHE_NAMES=['td-viewer','td-owner','td-index'];
var NET_TIMEOUT_MS=4000;
var MISS_ZH='{tdMissZh}';
var MISS_EN='{tdMissEn}';

// Canonical key for a plan-page URL: origin + '/?' + the three key params in
// order, lang defaulted to 'zh' and always written, everything else dropped.
// dropToken produces the owner storage key (owner copies are token-less).
function normKey(url, dropToken) {
  var u = new URL(url);
  var p = u.searchParams;
  var parts = [];
  for (var i = 0; i < KEY_PARAMS.length; i++) {
    var name = KEY_PARAMS[i];
    if (name === 'token') {
      var tok = p.get('token');
      if (tok && !dropToken) parts.push('token=' + encodeURIComponent(tok));
    } else if (name === 'lang') {
      parts.push('lang=' + encodeURIComponent(p.get('lang') || 'zh'));
    } else {
      var v = p.get(name);
      if (v) parts.push(name + '=' + encodeURIComponent(v));
    }
  }
  return u.origin + '/?' + parts.join('&');
}

// Absolute on purpose: a browser resolves any relative cache key against the
// worker's base URL anyway, so this is the same storage location — but keeping
// every key the SW touches absolute means keys() values, match/delete reads
// and put writes can never disagree.
function manifestKey(key) {
  return self.location.origin + '/__td/manifest?k=' + encodeURIComponent(key);
}

function cacheFor(role) {
  return self.caches.open(role === 'owner' ? CACHE_NAMES[1] : CACHE_NAMES[0]);
}

function indexCache() {
  return self.caches.open(CACHE_NAMES[2]);
}

function jsonRes(obj) {
  return new Response(JSON.stringify(obj), { headers: { 'Content-Type': 'application/json' } });
}

async function readJson(cache, key) {
  var res = await cache.match(key);
  if (!res) return null;
  try {
    return JSON.parse(await res.text());
  } catch (e) {
    return null;
  }
}

function extractView(html) {
  var m = /data-view="([^"]*)"/.exec(html);
  return m ? m[1] : '';
}

function extractRenderedAt(html) {
  var m = /data-rendered-at="(\d+)"/.exec(html);
  return m ? Number(m[1]) : 0;
}

// Every /map/ and /voucher/ link in the page, as absolute cache keys. The HTML
// escapes '&' as '&amp;' (esc_url_attr), so the separator is restored BEFORE
// keying — otherwise the asset lands under a key nothing will ever request.
function extractAssets(html) {
  var out = [];
  var re = /(?:href|src)="([^"]*)"/g;
  var m;
  while ((m = re.exec(html)) !== null) {
    var raw = m[1].split('&amp;').join('&');
    if (raw.indexOf('/map/') === 0 || raw.indexOf('/voucher/') === 0) {
      var abs = self.location.origin + raw;
      if (out.indexOf(abs) === -1) out.push(abs);
    }
  }
  return out;
}

// Asset entries are absolute /map/ or /voucher/ URLs. An HTML key is an
// absolute URL too (origin + '/?plan=…'), so the path — not the scheme — is
// what marks a sweepable entry.
function isAssetKey(key) {
  return key.indexOf('/map/') !== -1 || key.indexOf('/voucher/') !== -1;
}

// Real browsers' cache.keys() returns Request objects, not the strings put()
// stored; every keys() result is normalised to its URL string here so string
// comparisons/prefixes stay valid.
async function keyUrls(cache) {
  var keys = await cache.keys();
  var out = [];
  for (var i = 0; i < keys.length; i++) {
    out.push(typeof keys[i] === 'string' ? keys[i] : keys[i].url);
  }
  return out;
}

// Mark-and-sweep: delete asset entries that no manifest in this cache
// references. HTML and manifest keys are never asset keys, so they stay.
// Manifest detection is contains-based for the same reason isAssetKey is: a
// browser may hand the key back in resolved absolute form.
async function sweep(cache) {
  var keys = await keyUrls(cache);
  var referenced = {};
  for (var i = 0; i < keys.length; i++) {
    if (keys[i].indexOf('/__td/manifest') === -1) continue;
    var m = await readJson(cache, keys[i]);
    var list = (m && m.assets) || [];
    for (var j = 0; j < list.length; j++) referenced[list[j]] = true;
  }
  for (var k = 0; k < keys.length; k++) {
    if (isAssetKey(keys[k]) && !referenced[keys[k]]) await cache.delete(keys[k]);
  }
}

// A share token came back 403 online: it was revoked. Drop every viewer copy
// stored under it (HTML, manifest, index entries), then sweep so assets only
// that token used go too — shared maps other tokens still reference survive.
// td-viewer is scanned DIRECTLY: td-index is not authoritative for what a
// token saved — after an owner re-save of the same URL the index points at
// the owner copy, and an index-only walk would leave the revoked token's
// HTML, manifest and vouchers in place. Viewer HTML keys are absolute '/?'
// URLs; assets (which may carry ?token= themselves) and manifests are
// skipped here — assets go via the sweep once their manifest is gone.
async function purgeToken(token) {
  var viewer = await cacheFor('viewer');
  var idx = await indexCache();
  var vkeys = await keyUrls(viewer);
  for (var i = 0; i < vkeys.length; i++) {
    var k = vkeys[i];
    if (isAssetKey(k) || k.indexOf('/__td/manifest') !== -1) continue;
    if (new URL(k).searchParams.get('token') !== token) continue;
    await viewer.delete(k);
    await viewer.delete(manifestKey(k));
  }
  var keys = await keyUrls(idx);
  for (var j = 0; j < keys.length; j++) {
    var entry = await readJson(idx, keys[j]);
    if (!entry || entry.role !== 'viewer' || !entry.key) continue;
    if (new URL(entry.key).searchParams.get('token') !== token) continue;
    await idx.delete(keys[j]);
  }
  await sweep(viewer);
}

// Logout: empty td-owner entirely and drop the owner entries from td-index.
// An owner re-save of a token URL REPLACED that URL's index entry, so a plain
// delete would orphan the earlier viewer copy still stored in td-viewer
// (findCopy reads td-index only). Removing an owner entry re-points it to a
// surviving viewer copy for that same URL when one exists.
async function clearOwner() {
  var owner = await cacheFor('owner');
  var keys = await keyUrls(owner);
  for (var i = 0; i < keys.length; i++) await owner.delete(keys[i]);
  var idx = await indexCache();
  var viewer = await cacheFor('viewer');
  var ikeys = await keyUrls(idx);
  for (var j = 0; j < ikeys.length; j++) {
    var entry = await readJson(idx, ikeys[j]);
    if (!entry || entry.role !== 'owner') continue;
    await idx.delete(ikeys[j]);
    // A viewer copy is stored under exactly its own index key (handleSave:
    // key === indexKey for viewer saves), so the match below is that copy.
    if (await viewer.match(ikeys[j])) {
      await idx.put(ikeys[j], jsonRes({ role: 'viewer', key: ikeys[j], savedAt: Date.now() }));
    }
  }
}

// Look up the saved copy for a request URL via td-index ONLY (never guess from
// the URL's shape): an owner opening a token link still finds the owner copy.
async function findCopy(url) {
  var idx = await indexCache();
  var entry = await readJson(idx, normKey(url.href, false));
  if (!entry || !entry.key) return null;
  var cache = await cacheFor(entry.role);
  var res = await cache.match(entry.key);
  return res || null;
}

// Serve a cached page with data-served rewritten, so the inline script shows
// the offline banner (or the relogin banner) with the stored renderedAt.
async function rewriteServed(res, mark) {
  var html = await res.text();
  return new Response(html.split('data-served="net"').join('data-served="' + mark + '"'), {
    status: 200,
    headers: { 'Content-Type': 'text/html; charset=utf-8' },
  });
}

function missPage(lang) {
  var en = lang === 'en';
  return new Response(
    '<!doctype html><html lang="' + (en ? 'en' : 'zh-Hant') + '"><head><meta charset="utf-8">' +
      '<meta name="viewport" content="width=device-width,initial-scale=1"></head>' +
      '<body style="font-family:system-ui,sans-serif;padding:24px;line-height:1.6">' +
      (en ? MISS_EN : MISS_ZH) +
      '</body></html>',
    { status: 200, headers: { 'Content-Type': 'text/html; charset=utf-8' } }
  );
}

// The SAVE protocol (spec section 6). Reply statuses on port1:
//   saved {maps, pdfs} | partial-new {maps, pdfs, failed} |
//   partial-kept {failed, keptRenderedAt} | failed
// A full set of assets replaces the copy (then sweep); a partial set replaces
// nothing when a complete copy exists, and writes an incomplete copy otherwise.
async function handleSave(url, reply) {
  try {
    var u = new URL(url);
    if (u.searchParams.has('at')) return reply({ status: 'failed' });
    if (u.pathname !== '/' || !u.searchParams.get('plan')) return reply({ status: 'failed' });
    var res = await self.fetch(url);
    if (res.status === 403 && u.searchParams.get('token')) {
      await purgeToken(u.searchParams.get('token'));
      return reply({ status: 'failed' });
    }
    if (res.status !== 200) return reply({ status: 'failed' });
    var view = res.headers.get('X-TD-View');
    if (view !== 'owner' && view !== 'viewer') return reply({ status: 'failed' });
    var html = await res.text();
    if (extractView(html) !== view) return reply({ status: 'failed' });

    var role = view;
    var key = normKey(url, role === 'owner');
    var indexKey = normKey(url, false);
    var cache = await cacheFor(role);
    var oldManifest = await readJson(cache, manifestKey(key));
    var hadComplete = !!(oldManifest && oldManifest.complete);
    var renderedAt = extractRenderedAt(html);

    var assets = extractAssets(html);
    var saved = [];
    var failed = [];
    // Assets first (put order is assets -> manifest -> HTML -> index). A
    // versioned URL already in cache is skipped; an unversioned one (no sha /
    // no etag) is re-fetched every SAVE so a re-upload cannot stay stale.
    for (var i = 0; i < assets.length; i++) {
      var a = assets[i];
      var versioned = new URL(a).searchParams.has('v');
      if (versioned && (await cache.match(a))) {
        saved.push(a);
        continue;
      }
      try {
        var ares = await self.fetch(a);
        if (ares.ok) {
          await cache.put(a, ares);
          saved.push(a);
        } else {
          failed.push(a);
        }
      } catch (e) {
        failed.push(a);
      }
    }
    var maps = 0;
    var pdfs = 0;
    for (var j = 0; j < saved.length; j++) {
      if (saved[j].indexOf('/map/') !== -1) maps++;
      else pdfs++;
    }

    if (failed.length === 0) {
      await cache.put(manifestKey(key), jsonRes({ assets: saved, complete: true, renderedAt: renderedAt }));
      await cache.put(key, new Response(html, { status: 200, headers: { 'Content-Type': 'text/html; charset=utf-8' } }));
      await (await indexCache()).put(indexKey, jsonRes({ role: role, key: key, savedAt: Date.now() }));
      await sweep(cache);
      return reply({ status: 'saved', maps: maps, pdfs: pdfs });
    }
    if (hadComplete) {
      // The old complete copy stays exactly as it was (its assets too, which
      // is why there is no sweep here); newly fetched assets remain cached.
      return reply({ status: 'partial-kept', failed: failed, keptRenderedAt: oldManifest.renderedAt });
    }
    await cache.put(manifestKey(key), jsonRes({ assets: saved, complete: false, renderedAt: renderedAt }));
    await cache.put(key, new Response(html, { status: 200, headers: { 'Content-Type': 'text/html; charset=utf-8' } }));
    await (await indexCache()).put(indexKey, jsonRes({ role: role, key: key, savedAt: Date.now() }));
    return reply({ status: 'partial-new', maps: maps, pdfs: pdfs, failed: failed });
  } catch (e) {
    reply({ status: 'failed' });
  }
}

// Race helper: never lets a network rejection escape the race — it becomes a
// {net:false} result the caller turns into a cache fallback.
function guarded(p) {
  return p.then(
    function (res) {
      return { net: true, res: res };
    },
    function (err) {
      return { net: false, err: err };
    }
  );
}

// Plan-page navigation. Network-first: 200 passes through; error or NET_TIMEOUT
// falls back to the td-index copy (marked "cache"); a server 5xx with a copy
// also serves the copy, without one the 5xx itself is passed through (NOT the
// miss page). 403 with a token purges that token (even after the timeout, via
// waitUntil); 403 without a token serves the owner copy marked "cache-relogin".
async function servePlanPage(event, req, url) {
  var net = guarded(self.fetch(req));
  var timer = null;
  var timeout = new Promise(function (resolve) {
    timer = setTimeout(function () {
      resolve('td-timeout');
    }, NET_TIMEOUT_MS);
  });
  var winner = await Promise.race([net, timeout]);
  var mark = 'cache';
  var passthrough = null;

  if (winner === 'td-timeout') {
    event.waitUntil(
      net.then(function (g) {
        if (g.net && g.res.status === 403 && url.searchParams.get('token')) {
          return purgeToken(url.searchParams.get('token'));
        }
      })
    );
  } else {
    clearTimeout(timer);
    if (!winner.net) {
      // offline: fall through to the copy / miss page below
    } else if (winner.res.status === 200) {
      return winner.res;
    } else if (winner.res.status === 403) {
      var tok = url.searchParams.get('token');
      if (tok) {
        await purgeToken(tok);
        return winner.res;
      }
      mark = 'cache-relogin';
      passthrough = winner.res;
    } else if (winner.res.status >= 500) {
      passthrough = winner.res;
    } else {
      return winner.res;
    }
  }

  var found = await findCopy(url);
  if (found) return rewriteServed(found, mark);
  if (passthrough) return passthrough;
  return missPage(url.searchParams.get('lang'));
}

async function assetFromCache(absUrl) {
  var viewer = await cacheFor('viewer');
  var hit = await viewer.match(absUrl);
  if (hit) return hit;
  var owner = await cacheFor('owner');
  return await owner.match(absUrl);
}

// Assets: versioned (?v=) is cache-first; unversioned is network-first so a
// re-uploaded file never serves stale. Neither is stored on read — SAVE is the
// only writer.
async function serveAsset(req, url) {
  var abs = url.href;
  if (url.searchParams.has('v')) {
    var hit = await assetFromCache(abs);
    if (hit) return hit;
    return self.fetch(req);
  }
  var net = guarded(self.fetch(req));
  var timer = null;
  var timeout = new Promise(function (resolve) {
    timer = setTimeout(function () {
      resolve('td-timeout');
    }, NET_TIMEOUT_MS);
  });
  var winner = await Promise.race([net, timeout]);
  if (winner !== 'td-timeout') {
    clearTimeout(timer);
    if (winner.net) return winner.res;
    var fallback = await assetFromCache(abs);
    if (fallback) return fallback;
    throw winner.err;
  }
  var late = await assetFromCache(abs);
  if (late) return late;
  return net.then(function (g) {
    if (g.net) return g.res;
    throw g.err;
  });
}

self.addEventListener('install', function () {
  self.skipWaiting();
});

self.addEventListener('activate', function () {
  self.clients.claim();
});

self.addEventListener('message', function (event) {
  var d = event.data;
  if (!d || d.type !== 'SAVE') return;
  var port = event.ports && event.ports[0];
  if (!port) return;
  // The save outlives this handler's turn: without waitUntil the UA may kill
  // the worker mid-save (phones terminate idle SWs aggressively).
  event.waitUntil(
    handleSave(d.url, function (msg) {
      port.postMessage(msg);
    })
  );
});

self.addEventListener('fetch', function (event) {
  var req = event.request;
  if (req.method !== 'GET') return;
  var url;
  try {
    url = new URL(req.url);
  } catch (e) {
    return;
  }
  if (url.origin !== self.location.origin) return;
  if (url.searchParams.has('at')) return;
  if (url.pathname === '/auth/logout') {
    event.waitUntil(clearOwner());
    return;
  }
  if (url.pathname === '/' && url.searchParams.get('plan')) {
    event.respondWith(servePlanPage(event, req, url));
    return;
  }
  if (url.pathname.indexOf('/map/') === 0 || url.pathname.indexOf('/voucher/') === 0) {
    event.respondWith(serveAsset(req, url));
  }
});
