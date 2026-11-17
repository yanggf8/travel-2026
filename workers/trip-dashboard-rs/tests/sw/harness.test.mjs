// Self-test for the test harness (proof lock, same role as the Rust
// teardown_plan self-test): FakeCache must emulate the real Cache API's
// key contract — keys() returns Request objects whose .url is ALWAYS
// absolute (a browser resolves a relative put() key against the worker's
// base URL), never the raw string that was stored.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ORIGIN, VIEWER_CACHE, makeCaches, assetResponse, htmlResponse } from './harness.mjs';

test('cache_keys_returns_absolute_url_requests_for_relative_stored_keys', async () => {
  const caches = makeCaches();
  const c = await caches.open(VIEWER_CACHE);
  await c.put('/__td/manifest?k=' + encodeURIComponent(ORIGIN + '/?plan=p1'), htmlResponse('{}'));
  await c.put(ORIGIN + '/map/p1/day1.png?v=e1', assetResponse('M'));

  const keys = await c.keys();
  assert.equal(keys.length, 2);
  for (const k of keys) {
    assert.ok(k instanceof Request, `keys() yields a Request, got: ${JSON.stringify(k)}`);
    assert.ok(k.url.startsWith(ORIGIN), `absolute .url, got: ${k.url}`);
  }
  const urls = keys.map((k) => k.url);
  assert.ok(urls.includes(ORIGIN + '/__td/manifest?k=' + encodeURIComponent(ORIGIN + '/?plan=p1')));
  assert.ok(urls.includes(ORIGIN + '/map/p1/day1.png?v=e1'));
});
