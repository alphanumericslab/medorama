// A stand-in page for the tests of the page's modules (not a test itself: `node --test` runs
// *.test.mjs).  Every element asked for by a selector is made when first asked for, keeps what
// is written to it (innerHTML, textContent, attributes, classes) and its listeners, so that a
// test can fire them and read what the module drew.

export const els = new Map();
const realTimeout = globalThis.setTimeout;

export function element(sel, classes = []) {
  const cls = new Set(classes);
  const attrs = new Map();
  const on = {};
  const e = {
    sel, id: String(sel).replace(/^#/, ''), innerHTML: '', textContent: '', value: '', placeholder: '', title: '',
    disabled: false, hidden: false, required: false, open: false, checked: false, on, dataset: {}, style: {},
    scrollTop: 0, clientWidth: 600, clientHeight: 400, scrollWidth: 600, isConnected: true,
    classList: {
      contains: (c) => cls.has(c),
      add: (...c) => c.forEach((x) => cls.add(x)),
      remove: (...c) => c.forEach((x) => cls.delete(x)),
      toggle: (c, f) => { const want = f ?? !cls.has(c); if (want) cls.add(c); else cls.delete(c); return want; },
    },
    setAttribute: (k, v) => attrs.set(k, String(v)),
    getAttribute: (k) => (attrs.has(k) ? attrs.get(k) : null),
    hasAttribute: (k) => attrs.has(k),
    removeAttribute: (k) => attrs.delete(k),
    addEventListener: (k, f) => { on[k] = f; },
    removeEventListener: () => {},
    querySelector: () => null,
    querySelectorAll: () => [],
    closest: () => null,
    matches: () => false,
    focus: () => {},
    blur: () => {},
    select: () => {},
    append: () => {},
    remove: () => {},
    scrollIntoView: () => {},
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 600, height: 400 }),
    get className() { return [...cls].join(' '); },
  };
  return e;
}

/** The element of a selector (made when first asked for). */
export const el = (sel) => {
  if (!els.has(sel)) els.set(sel, element(sel, sel === '#login' || sel === '#app' ? ['hidden'] : []));
  return els.get(sel);
};

/** A page: document, window, storage, history; `fetch` is the test's. */
export function install({ hash = '#/cohorts' } = {}) {
  const body = element('body');
  const root = element('html');
  globalThis.document = {
    body, documentElement: root, activeElement: body, title: 'Medorama',
    querySelector: el, querySelectorAll: () => [], getElementById: (id) => el(`#${id}`),
    addEventListener: () => {}, createElement: (t) => element(t),
  };
  const reloads = [];
  globalThis.location = { hash, href: `http://x/${hash}`, reload: () => reloads.push(1), assign: () => {} };
  globalThis.history = { state: null, replaceState: () => {}, pushState: () => {}, back: () => {}, forward: () => {} };
  const mem = new Map();
  const storage = { getItem: (k) => (mem.has(k) ? mem.get(k) : null), setItem: (k, v) => mem.set(k, String(v)), removeItem: (k) => mem.delete(k) };
  globalThis.sessionStorage = storage;
  globalThis.localStorage = storage;
  const mq = { matches: false, addEventListener: () => {} };
  globalThis.matchMedia = () => mq;
  globalThis.screen = { width: 1440, height: 900 };
  globalThis.window = globalThis;
  globalThis.addEventListener = () => {};
  globalThis.innerWidth = 1440;
  globalThis.innerHeight = 800;
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => '#123456' });
  globalThis.CSS = { escape: (s) => String(s) };
  if (!globalThis.Event) globalThis.Event = class { constructor(t) { this.type = t; } };
  globalThis.dispatchEvent = () => {};
  // the page's own timers (its checks every few seconds) do not keep a test running
  globalThis.setTimeout = (f, ms, ...a) => { const t = realTimeout(f, ms, ...a); t.unref?.(); return t; };
  return { reloads };
}

export const tick = () => new Promise((r) => realTimeout(r, 0));
export const wait = (ms) => new Promise((r) => realTimeout(r, ms));
export const answer = (v, status = 200) => ({ ok: status < 400, status, statusText: status === 500 ? 'Internal Server Error' : '', json: async () => v });
