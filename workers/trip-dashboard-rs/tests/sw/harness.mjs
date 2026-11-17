// Test harness for src/sw.js and src/sw_kill.js under `node --test` — no browser.
// Each loadWorker() call evaluates the script once against a fresh Service-Worker-
// shaped `self` (see loadWorker for why that must be a Function scope, not an
// import). Caches and fetch are fakes; Request/Response/Headers are Node's
// built-ins, so URL/method/headers/body behave like the real platform.
//
// Wire contracts consumed here come from the other offline tasks: `?v=` versioned
// asset links (T1) and X-TD-View / data-view / data-rendered-at / data-served="net"
// on the plan page (T2). This file loads the RAW sw.js, so the i18n miss-page
// placeholders {tdMissZh}/{tdMissEn} stay literal; the fill + JS escaping is
// locked by the Rust tests in src/sw.rs.

import { readFileSync } from 'node:fs';

const srcCache = new Map();

function swSource(file) {
  if (!srcCache.has(file)) {
    srcCache.set(file, readFileSync(new URL('../../src/' + file, import.meta.url), 'utf8'));
  }
  return srcCache.get(file);
}

export const ORIGIN = 'https://td.test';
export const VIEWER_CACHE = 'td-viewer';
export const OWNER_CACHE = 'td-owner';
export const INDEX_CACHE = 'td-index';

function keyOf(key) {
  return typeof key === 'string' ? key : key.url;
}

async function snapshot(resp) {
  const body = await resp.clone().text();
  const headers = {};
  for (const [k, v] of resp.headers.entries()) headers[k] = v;
  return { status: resp.status, headers, body };
}

export class FakeCache {
  constructor(name, log) {
    this.name = name;
    this.map = new Map();
    this.log = log;
  }
  async match(key) {
    const snap = this.map.get(keyOf(key));
    return snap ? new Response(snap.body, { status: snap.status, headers: snap.headers }) : undefined;
  }
  async put(key, resp) {
    const k = keyOf(key);
    this.map.set(k, await snapshot(resp));
    this.log.push({ op: 'put', cache: this.name, key: k });
  }
  async delete(key) {
    const k = keyOf(key);
    const had = this.map.delete(k);
    this.log.push({ op: 'delete', cache: this.name, key: k });
    return had;
  }
  async keys() {
    // Real browsers return Request objects here, never the strings put()
    // stored — and a relative key a put() accepted was resolved against the
    // worker's base URL, so .url is ALWAYS absolute. Resolve every stored
    // key against the harness origin (ORIGIN is this worker's base, the
    // same role self.location plays in the real script) so callers are
    // exercised with absolute keys exactly as in a browser.
    return [...this.map.keys()].map((k) => new Request(new URL(k, ORIGIN).href));
  }
}

export class FakeCaches {
  constructor() {
    this.map = new Map();
    this.log = [];
    this.deleted = [];
  }
  async open(name) {
    if (!this.map.has(name)) this.map.set(name, new FakeCache(name, this.log));
    return this.map.get(name);
  }
  async keys() {
    return [...this.map.keys()];
  }
  async has(name) {
    return this.map.has(name);
  }
  async delete(name) {
    this.deleted.push(name);
    return this.map.delete(name);
  }
  names() {
    return [...this.map.keys()];
  }
}

export function makeCaches() {
  return new FakeCaches();
}

// A fetch whose resolver can be swapped between test phases (seed online, then
// go offline / return 500 / stall). Every URL requested is logged.
export function switchableFetch(initial) {
  let resolver = initial;
  const log = [];
  const f = async (input) => {
    const url = typeof input === 'string' ? input : input.url;
    log.push(url);
    const v = resolver(url, input);
    if (v instanceof Error) throw v;
    return v;
  };
  f.set = (r) => {
    resolver = r;
  };
  f.log = log;
  return f;
}

// Resolve from a Map<url, () => Response|Error>; anything unmapped is offline.
export function mapResolver(map) {
  return (url) => {
    const f = map.get(url);
    if (f) return f();
    return new TypeError('Failed to fetch: ' + url);
  };
}

export function offline() {
  return () => new TypeError('Failed to fetch');
}

export function htmlResponse(html, { view = 'viewer', status = 200 } = {}) {
  const headers = { 'Content-Type': 'text/html; charset=utf-8' };
  if (view) headers['X-TD-View'] = view;
  return new Response(html, { status, headers });
}

export function assetResponse(body = 'ASSET', type = 'image/png') {
  return new Response(body, { status: 200, headers: { 'Content-Type': type } });
}

export function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

export function req(url, init) {
  return new Request(url, init);
}

// Mirrors sw.js's manifestKey: absolute, keyed under the harness origin.
export function manifestKey(key) {
  return ORIGIN + '/__td/manifest?k=' + encodeURIComponent(key);
}

// Install `self` and evaluate the SW script with `self` as a function
// parameter — each load gets its own private scope, the way a real service
// worker instance has its own global. (A dynamic import cannot do this: the
// module cache returns the first evaluation for every load, and a bare `self`
// would resolve against globalThis, cross-linking every test's instances.)
export async function loadWorker({ file = 'sw.js', caches, fetch } = {}) {
  const listeners = new Map();
  const state = { skipWaiting: 0, claims: 0, unregistered: 0, waitUntil: [], messageEvents: [] };
  const swSelf = {
    addEventListener(type, fn) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(fn);
    },
    caches,
    fetch,
    location: new URL(ORIGIN + '/sw.js'),
    skipWaiting() {
      state.skipWaiting += 1;
    },
    clients: {
      claim() {
        state.claims += 1;
        return Promise.resolve();
      },
    },
    registration: {
      unregister() {
        state.unregistered += 1;
        return Promise.resolve();
      },
    },
  };
  new Function('self', swSource(file))(swSelf);

  function mkEvent() {
    const ev = {};
    ev.waitUntil = (p) => {
      state.waitUntil.push(p);
      ev._waiters = ev._waiters || [];
      ev._waiters.push(p);
    };
    return ev;
  }
  async function fire(type, event) {
    for (const fn of listeners.get(type) || []) {
      const r = fn(event);
      if (r && typeof r.then === 'function') await r;
    }
  }
  return {
    state,
    install: () => fire('install', mkEvent()),
    activate: () => fire('activate', mkEvent()),
    fetchEvent(request) {
      const ev = mkEvent();
      ev.request = request;
      ev.responded = false;
      ev.respondWith = (p) => {
        ev.responded = true;
        ev.response = p;
      };
      for (const fn of listeners.get('fetch') || []) fn(ev);
      return ev;
    },
    save(url) {
      return new Promise((resolve) => {
        const port = { postMessage: (m) => resolve(m) };
        const ev = mkEvent();
        ev.data = { type: 'SAVE', url };
        ev.ports = [port];
        state.messageEvents.push(ev);
        for (const fn of listeners.get('message') || []) fn(ev);
      });
    },
    async waitIdle() {
      await Promise.all([...state.waitUntil]);
    },
  };
}
