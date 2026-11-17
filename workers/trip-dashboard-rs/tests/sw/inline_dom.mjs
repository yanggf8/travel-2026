// Fake DOM + runtime for executing BLOCK_MEMORY_SCRIPT (extracted from
// src/render/mod.rs) under `node --test` — no browser, no new npm deps
// (offline-t4 Round 2). It implements exactly what the ONE inline script
// touches and nothing more:
//
//   - simple selectors only: [tag][.class…][[attr]] or [attr="value"],
//     comma lists allowed, NO combinators — the script never uses one
//     (grep it; every querySelector is a single simple selector),
//   - attributes / classList / textContent / firstChild /
//     appendChild-insertBefore-removeChild / hidden / details `open`,
//   - a virtual timer queue (the SAVE reply timeout is 60 s — real waits
//     would make the suite crawl) and a MessageChannel fake whose
//     deliveries are queued on the same virtual clock, so replies and
//     timeouts race deterministically.
//
// `href`/`src` stay exactly as authored (no URL resolution): the SW's
// failed-asset list carries absolute URLs, and the page's anchors are built
// with the same absolute strings, so osName's `a.href === url` lookup works
// like on a real page. This is a contract test of the script, not a DOM.
//
// LIMITS (offline-t4 Round 3, review item 6 — do not claim more than this
// models): events do not bubble and are never captured (addEventListener
// stores handlers; fire() calls them on the target only), toggle events are
// NOT synthesized when a details `open` changes, and there is no layout,
// focus, scroll or real network. So the collapse-memory/toggle bookkeeping
// and the reveal-on-hash paths are exercised only insofar as the tests call
// them directly; interaction regressions beyond what inline_script.test.mjs
// actually asserts (button un-hiding, marker moves, state changes) are NOT
// covered here — real browsers are T6/T7.

import { readFileSync } from 'node:fs';

const REAL = {
  setTimeout: globalThis.setTimeout,
  clearTimeout: globalThis.clearTimeout,
  setInterval: globalThis.setInterval,
  clearInterval: globalThis.clearInterval,
  setImmediate: globalThis.setImmediate,
  Intl: globalThis.Intl,
  Date: globalThis.Date,
};

// Must equal TD_SAVE_TIMEOUT_MS in BLOCK_MEMORY_SCRIPT (src/render/mod.rs);
// the Rust substring tests pin the constant's name, this pins its use.
export const SAVE_TIMEOUT_MS = 60000;

export function scriptSource() {
  const src = readFileSync(new URL('../../src/render/mod.rs', import.meta.url), 'utf8');
  const begin = 'BLOCK_MEMORY_SCRIPT: &str = r#"<script>';
  const end = '</script>"#;';
  const i = src.indexOf(begin);
  const j = i < 0 ? -1 : src.indexOf(end, i);
  if (i < 0 || j < 0) throw new Error('BLOCK_MEMORY_SCRIPT not found in src/render/mod.rs');
  // The template's {tdToday} placeholder is filled by block_memory_script(lang)
  // with the i18n `today` label (ZH 「今天」) — mirror that fill here so the
  // executed source matches what a ZH page actually ships.
  return src.slice(i + begin.length, j).trim().split('{tdToday}').join('今天');
}

// ---------------------------------------------------------------------------
// Virtual timers. advance(ms) runs everything due within ms in due order,
// synchronously — time only moves when a test says so.
// ---------------------------------------------------------------------------

export function makeTimers() {
  let now = 0;
  let seq = 0;
  const items = new Map(); // id -> {id, due, fn, interval, cleared}

  function nextDue(target) {
    let best = null;
    for (const t of items.values()) {
      if (t.cleared || t.due > target) continue;
      if (!best || t.due < best.due || (t.due === best.due && t.id < best.id)) best = t;
    }
    return best;
  }

  return {
    // Virtual "now" (ms since the epoch start of this timer world) — the fake
    // Date class below reads it, so advancing timers also moves Date.now().
    time() {
      return now;
    },
    setTimeout(fn, ms) {
      const id = ++seq;
      items.set(id, { id, due: now + (Number(ms) || 0), fn, interval: 0, cleared: false });
      return id;
    },
    clearTimeout(id) {
      const t = items.get(id);
      if (t) t.cleared = true;
    },
    setInterval(fn, ms) {
      const id = ++seq;
      items.set(id, { id, due: now + (Number(ms) || 0), fn, interval: Number(ms) || 1, cleared: false });
      return id;
    },
    clearInterval(id) {
      const t = items.get(id);
      if (t) t.cleared = true;
    },
    pendingIntervals() {
      let n = 0;
      for (const t of items.values()) if (t.interval && !t.cleared) n += 1;
      return n;
    },
    pendingTimeouts() {
      let n = 0;
      for (const t of items.values()) if (!t.interval && !t.cleared) n += 1;
      return n;
    },
    advance(ms) {
      const target = now + ms;
      for (;;) {
        const t = nextDue(target);
        if (!t) break;
        now = Math.max(now, t.due);
        if (t.interval) t.due = now + t.interval;
        else t.cleared = true;
        t.fn();
      }
      now = target;
    },
  };
}

// MessageChannel fake routed through the virtual clock: a postMessage on
// either port delivers to the other port's onmessage at virtual t+0.
export function makeMessageChannelClass(timers) {
  return function MessageChannel() {
    const port1 = { onmessage: null };
    const port2 = { onmessage: null };
    port1.postMessage = (m) => {
      timers.setTimeout(() => {
        if (port2.onmessage) port2.onmessage({ data: m });
      }, 0);
    };
    port2.postMessage = (m) => {
      timers.setTimeout(() => {
        if (port1.onmessage) port1.onmessage({ data: m });
      }, 0);
    };
    port1.close = port2.close = () => {};
    this.port1 = port1;
    this.port2 = port2;
  };
}

// ---------------------------------------------------------------------------
// Selector engine (simple selectors, comma lists).
// ---------------------------------------------------------------------------

// Attribute values accept either quote style (the script builds its TOC
// selector with single-quoted CSS values to keep `"#` out of the Rust raw
// string).
const SIMPLE_RE = /^([a-zA-Z][\w-]*|)((?:\.[\w-]+)*)(?:\[([\w-]+)(?:=(["'])([^"']*)\4)?\])?$/;

function compileSimple(sel) {
  const m = SIMPLE_RE.exec(sel.trim());
  if (!m) throw new Error('fake DOM: unsupported selector: ' + sel);
  return {
    tag: m[1] ? m[1].toUpperCase() : null,
    classes: m[2] ? m[2].slice(1).split('.') : [],
    attr: m[3] || null,
    value: m[5],
  };
}

function matches(el, s) {
  if (s.tag && el.tagName !== s.tag) return false;
  for (const c of s.classes) if (!el.classList.contains(c)) return false;
  if (s.attr) {
    if (!el.hasAttribute(s.attr)) return false;
    if (s.value !== undefined && el.getAttribute(s.attr) !== s.value) return false;
  }
  return true;
}

function queryAll(root, selector) {
  const sels = selector.split(',').map(compileSimple);
  const out = [];
  const seen = new Set();
  (function walk(n) {
    for (const c of n.children) {
      if (typeof c === 'string') continue;
      for (const s of sels) {
        if (matches(c, s) && !seen.has(c)) {
          seen.add(c);
          out.push(c);
        }
      }
      walk(c);
    }
  })(root);
  return out;
}

// ---------------------------------------------------------------------------
// Element / document.
// ---------------------------------------------------------------------------

function tokens(el) {
  return el.className ? el.className.split(/\s+/).filter(Boolean) : [];
}

function setClasses(el, arr) {
  el.className = arr.join(' ');
}

export class El {
  constructor(tag, attrs = {}) {
    this.tagName = tag.toUpperCase();
    this.attrs = new Map();
    this.children = [];
    this.parentNode = null;
    this.listeners = {};
    this.className = '';
    this.hidden = !!attrs.hidden;
    this.open = !!attrs.open;
    this.value = '';
    this.alt = '';
    this.classList = {
      add: (...cs) => setClasses(this, [...tokens(this), ...cs].filter((v, i, a) => a.indexOf(v) === i)),
      remove: (...cs) => setClasses(this, tokens(this).filter((c) => !cs.includes(c))),
      toggle: (c, force) => {
        const has = tokens(this).includes(c);
        const want = force === undefined ? !has : !!force;
        if (want && !has) this.classList.add(c);
        if (!want && has) this.classList.remove(c);
        return want;
      },
      contains: (c) => tokens(this).includes(c),
    };
    for (const [k, v] of Object.entries(attrs)) {
      if (k === 'class') this.className = v;
      else if (k === 'hidden' || k === 'open') continue;
      else this.setAttribute(k, v);
    }
    if (attrs.lang !== undefined) this.lang = attrs.lang;
    // Plain authored values (see file header): absolute in tests, absolute
    // in the SW's lists, so the equality lookups behave like a real page.
    this.href = attrs.href;
    this.src = attrs.src;
  }

  setAttribute(k, v) {
    this.attrs.set(k, String(v));
    if (k === 'id') this.id = String(v);
  }
  getAttribute(k) {
    return this.attrs.has(k) ? this.attrs.get(k) : null;
  }
  hasAttribute(k) {
    return this.attrs.has(k);
  }
  removeAttribute(k) {
    this.attrs.delete(k);
  }

  get textContent() {
    return this.children.map((c) => (typeof c === 'string' ? c : c.textContent)).join('');
  }
  set textContent(v) {
    this.children = [String(v)];
  }

  get firstChild() {
    return this.children[0] !== undefined ? this.children[0] : null;
  }

  detach(n) {
    if (n.parentNode) {
      const i = n.parentNode.children.indexOf(n);
      if (i >= 0) n.parentNode.children.splice(i, 1);
      n.parentNode = null;
    }
  }
  appendChild(n) {
    if (typeof n === 'string') {
      this.children.push(n); // builder convenience: appendChild(text)
      return n;
    }
    this.detach(n);
    n.parentNode = this;
    this.children.push(n);
    return n;
  }
  insertBefore(n, ref) {
    this.detach(n);
    let i = ref ? this.children.indexOf(ref) : this.children.length;
    if (i < 0) i = this.children.length;
    this.children.splice(i, 0, n);
    n.parentNode = this;
    return n;
  }
  removeChild(n) {
    const i = this.children.indexOf(n);
    if (i < 0) throw new Error('fake DOM: removeChild of a non-child');
    this.children.splice(i, 1);
    n.parentNode = null;
    return n;
  }

  querySelector(sel) {
    return queryAll(this, sel)[0] || null;
  }
  querySelectorAll(sel) {
    return queryAll(this, sel);
  }

  addEventListener(type, fn) {
    (this.listeners[type] = this.listeners[type] || []).push(fn);
  }
  fire(type, ev = {}) {
    for (const fn of [...(this.listeners[type] || [])]) fn({ target: this, ...ev });
  }
}

export class FakeDoc {
  constructor(lang = 'zh-Hant') {
    this.listeners = {};
    this.documentElement = new El('html', { lang });
    this.body = new El('body');
    this.documentElement.appendChild(this.body);
  }
  getElementById(id) {
    let found = null;
    (function walk(n) {
      if (found) return;
      for (const c of n.children) {
        if (typeof c === 'string') continue;
        if (c.getAttribute('id') === id) {
          found = c;
          return;
        }
        walk(c);
        if (found) return;
      }
    })(this.documentElement);
    return found;
  }
  createElement(tag) {
    return new El(tag);
  }
  createTextNode(t) {
    return String(t);
  }
  querySelector(sel) {
    return queryAll(this.documentElement, sel)[0] || null;
  }
  querySelectorAll(sel) {
    return queryAll(this.documentElement, sel);
  }
  addEventListener(type, fn) {
    (this.listeners[type] = this.listeners[type] || []).push(fn);
  }
  fire(type, ev = {}) {
    for (const fn of [...(this.listeners[type] || [])]) fn(ev);
  }
  byId(id) {
    return this.getElementById(id);
  }
}

// ---------------------------------------------------------------------------
// Page builder — the plan-page DOM shapes the script and T6's probe read.
// ---------------------------------------------------------------------------

const ZH = {
  saving: '正在存到本機…',
  saved: '已存好，可離線查看（地圖 {maps}、票券 {pdfs}）',
  partial: '已存，但 {n} 項未存：',
  kept: '這次更新沒存完，離線時會看到 {time} 的版本',
  failed: '沒存好，離線時看不到這一頁',
  unsupported: '這個瀏覽器不能離線保存，請用 Safari／Chrome 開啟',
  copy: '離線副本 · {time} 儲存，可能不是最新',
  relogin: '離線副本 · 需要重新登入才能更新',
  retry: '重試',
  today: '今天',
};

export function buildPage({
  planId = 'p1',
  served = 'net',
  renderedAt = '1763395200000',
  tz = 'Asia/Tokyo',
  lang = 'zh-Hant',
  now = null, // { date, tz, pinned } — omit for no #sec-now card
  days = [], // [{ n, date, today=false, open=false }]
  anchors = [], // [{ href, text }] — a[href] entries for failed-asset naming
  lsMap = null, // Map<string,string> pre-seeded localStorage
} = {}) {
  const doc = new FakeDoc(lang);
  const b = doc.body;
  b.setAttribute('data-plan-id', planId);
  b.setAttribute('data-view', 'viewer');
  b.setAttribute('data-rendered-at', String(renderedAt));
  b.setAttribute('data-served', served);
  if (tz) b.setAttribute('data-tz', tz);

  const status = new El('div', {
    class: 'offline-status',
    id: 'offline-status',
    hidden: true,
    'data-state': '',
    'data-l-saving': ZH.saving,
    'data-l-saved': ZH.saved,
    'data-l-partial': ZH.partial,
    'data-l-kept': ZH.kept,
    'data-l-failed': ZH.failed,
    'data-l-unsupported': ZH.unsupported,
  });
  const msgEl = new El('span', { class: 'offline-msg' });
  const ul = new El('ul', { class: 'offline-failed', hidden: true });
  const retry = new El('button', { class: 'offline-retry', type: 'button', hidden: true }, []);
  retry.textContent = ZH.retry;
  status.appendChild(msgEl);
  status.appendChild(ul);
  status.appendChild(retry);
  const banner = new El('div', {
    class: 'offline-banner',
    id: 'offline-banner',
    hidden: true,
    'data-l-copy': ZH.copy,
    'data-l-relogin': ZH.relogin,
  });
  b.appendChild(status);
  b.appendChild(banner);

  // TOC with the two collapse buttons (served hidden; the script un-hides).
  const nav = new El('nav', { class: 'page-toc', id: 'toc' });
  const actions = new El('div', { class: 'toc-actions' });
  const tocBtns = [];
  for (const mode of ['close', 'open']) {
    const btn = new El('button', { class: 'toc-btn', type: 'button', 'data-blocks': mode, hidden: true });
    btn.textContent = mode === 'close' ? '全部收合' : '全部展開';
    actions.appendChild(btn);
    tocBtns.push(btn);
  }
  const head = new El('div', { class: 'toc-head' });
  const h2 = new El('h2');
  h2.textContent = '目錄';
  head.appendChild(h2);
  head.appendChild(actions);
  nav.appendChild(head);
  const list = new El('ol', { class: 'toc-list' });
  const tocLinks = [];
  for (const d of days) {
    const li = new El('li');
    const a = new El('a', {
      class: 'toc-day' + (d.today ? ' is-today' : ''),
      href: '#day-' + d.n,
    });
    const n = new El('span', { class: 'toc-day-n' });
    if (d.today) {
      const sp = new El('span', { class: 'toc-today' });
      sp.textContent = ZH.today;
      n.appendChild(sp);
    }
    n.appendChild('第 ' + d.n + ' 天');
    a.appendChild(n);
    li.appendChild(a);
    list.appendChild(li);
    tocLinks.push(a);
  }
  nav.appendChild(list);
  b.appendChild(nav);

  // Now card.
  let nowCard = null;
  if (now) {
    const attrs = {
      class: 'now-card',
      id: 'sec-now',
      'data-now-date': now.date,
      'data-l-now': '現在',
      'data-l-next': '接下來',
    };
    if (now.tz) attrs['data-now-tz'] = now.tz;
    if (now.pinned) attrs['data-now-pinned'] = '';
    nowCard = new El('section', attrs);
    b.appendChild(nowCard);
  }

  // Day cards.
  const daySections = [];
  const dayDetails = [];
  for (const d of days) {
    const sec = new El('section', {
      class: 'day',
      id: 'day-' + d.n,
      'data-day': String(d.n),
    });
    if (d.date) sec.setAttribute('data-date', d.date);
    const detAttrs = { 'data-block': 'day-' + d.n };
    const det = new El('details', detAttrs);
    det.open = d.open || d.today; // SSR serves today's card open
    if (d.today) det.setAttribute('data-today', '');
    const sum = new El('summary');
    const h = new El('h2');
    h.textContent = '第 ' + d.n + ' 天';
    sum.appendChild(h);
    det.appendChild(sum);
    const bodyDiv = new El('div', { class: 'day-body' });
    det.appendChild(bodyDiv);
    sec.appendChild(det);
    b.appendChild(sec);
    daySections.push(sec);
    dayDetails.push(det);
  }

  for (const a of anchors) {
    const el = new El('a', { href: a.href, class: 'doc-link' });
    el.textContent = a.text;
    b.appendChild(el);
  }

  const ls = lsMap
    ? {
        getItem: (k) => (lsMap.has(k) ? lsMap.get(k) : null),
        setItem: (k, v) => lsMap.set(k, String(v)),
      }
    : { getItem: () => null, setItem() {} };

  return { doc, body: b, status, msgEl, ul, retry, banner, nowCard, daySections, dayDetails, tocLinks, tocBtns, ls, ZH };
}

// ---------------------------------------------------------------------------
// Environment: installs the fake globals ONLY while the script (or a reply
// delivery) is actually running, so node:test's own timers stay real.
// ---------------------------------------------------------------------------

// A Date whose "now" rides the virtual timer clock (clockMs = its epoch
// base): advancing timers also advances new Date()/Date.now(), so a test can
// hold the page at 23:30 destination time and then cross midnight.
function makeDateClass(timers, base) {
  return class FakeDate extends REAL.Date {
    constructor(...args) {
      if (args.length) super(...args);
      else super(timers.time() + base);
    }
    static now() {
      return timers.time() + base;
    }
  };
}

// Round 3 item 4: Intl.DateTimeFormat.format() has NO specified output shape
// (only formatToParts is specified). This liar keeps formatToParts real but
// scrambles format() into M/D/YYYY — a script that compares format() output
// to the SSR's ISO data-now-date silently never matches; one built from
// formatToParts keeps working.
function makeLiarIntl() {
  return {
    DateTimeFormat: class {
      constructor(locale, opts) {
        this._f = new REAL.Intl.DateTimeFormat(locale, opts);
      }
      formatToParts(x) {
        return this._f.formatToParts(x);
      }
      format(x) {
        return this._f
          .formatToParts(x)
          .filter((p) => ['month', 'day', 'year'].includes(p.type))
          .map((p) => p.value)
          .join('/');
      }
    },
  };
}

export function makeEnv({
  page,
  url = 'https://td.test/?plan=p1&lang=zh&token=t0',
  swKind = 'active', // 'active' | 'none' | 'registerThrows' | 'readyNever' | 'registerNever'
  persist = true,
  noIntl = false,
  intlLiar = false, // format() returns a non-ISO shape; formatToParts stays real
  clockMs = null, // virtual Date epoch base (fake clock), or null for real time
} = {}) {
  const timers = makeTimers();
  const MessageChannel = makeMessageChannelClass(timers);
  const sw = { registerCalls: [], messages: [], ports: [], persistCalls: 0 };
  const activeReg = {
    active: {
      postMessage(msg, ports) {
        sw.messages.push({ ...msg });
        sw.ports.push(ports && ports[0]);
      },
    },
  };
  const navigator =
    swKind === 'none'
      ? {}
      : {
          serviceWorker: {
            register(src) {
              sw.registerCalls.push(src);
              if (swKind === 'registerThrows') throw new Error('register exploded');
              if (swKind === 'registerNever') return new Promise(() => {});
              return Promise.resolve({});
            },
            ready: swKind === 'readyNever' ? new Promise(() => {}) : Promise.resolve(activeReg),
          },
        };
  if (persist) navigator.storage = { persist: () => (sw.persistCalls += 1) };

  const u = new URL(url);
  const vals = {
    document: page.doc,
    navigator,
    window: { localStorage: page.ls, addEventListener() {} },
    location: { href: url, search: u.search, hash: u.hash || '' },
    MessageChannel,
    setTimeout: (fn, ms) => timers.setTimeout(fn, ms),
    clearTimeout: (id) => timers.clearTimeout(id),
    setInterval: (fn, ms) => timers.setInterval(fn, ms),
    clearInterval: (id) => timers.clearInterval(id),
  };
  if (clockMs !== null) vals.Date = makeDateClass(timers, clockMs);

  const G = globalThis;
  const prev = {};
  // Node ≥21 exposes `navigator` as a getter-only global, so overrides go
  // through property descriptors (saved whole and restored verbatim).
  function setGlobal(k, v) {
    const d = Object.getOwnPropertyDescriptor(G, k);
    if (!d || d.configurable) {
      Object.defineProperty(G, k, { value: v, writable: true, configurable: true });
      return d || null;
    }
    if (d.writable) {
      G[k] = v;
      return d;
    }
    throw new Error('fake DOM: cannot override global ' + k);
  }
  function restoreGlobal(k, d) {
    if (d) Object.defineProperty(G, k, d);
    else delete G[k];
  }
  function install() {
    for (const k of Object.keys(vals)) prev[k] = setGlobal(k, vals[k]);
    if (noIntl) prev.Intl = setGlobal('Intl', undefined);
    else if (intlLiar) prev.Intl = setGlobal('Intl', makeLiarIntl());
  }
  function restore() {
    for (const k of Object.keys(vals)) restoreGlobal(k, prev[k]);
    if (noIntl || intlLiar) restoreGlobal('Intl', prev.Intl);
  }

  async function withGlobals(fn) {
    install();
    try {
      return await fn();
    } finally {
      restore();
    }
  }
  const flush = () => new Promise((r) => REAL.setImmediate(r));

  return {
    page,
    sw,
    timers,
    async run() {
      await withGlobals(async () => {
        new Function(scriptSource())();
        await flush(); // register().then → osSend → ready.then → postMessage
        await flush();
      });
    },
    // Play the SW: deliver a reply for attempt i (0-based, in send order).
    deliver(i, msg) {
      return withGlobals(() => {
        if (!sw.ports[i]) throw new Error('no SAVE attempt ' + i);
        sw.ports[i].postMessage(msg);
        timers.advance(0);
      });
    },
    clickRetry() {
      return withGlobals(async () => {
        page.retry.fire('click');
        await flush();
      });
    },
    advance(ms) {
      return withGlobals(() => timers.advance(ms));
    },
  };
}

// The {time} the script should render for a given ms/tz — computed with the
// same Intl call so the assertion pins "uses data-tz via Intl", never a
// hardcoded offset.
export function expectedTime(lang, tz, ms) {
  return new Intl.DateTimeFormat(lang || undefined, {
    timeZone: tz,
    hourCycle: 'h23',
    month: 'numeric',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(ms));
}
