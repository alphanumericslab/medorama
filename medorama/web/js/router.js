// Every screen has its own address (#/patients, #/patient/12?item=…, #/viewer?path=…), so that
// back and forward work, and a page can be reloaded, bookmarked or shared.  The ‹ › buttons of
// the top bar are the browser's history; the router remembers where in it the page is.

const views = new Map();
let current = null;
let pos = 0;
// one number per screen drawn: what an older drawing's answers would show is dropped
let renders = 0;
let report = (e) => console.error(e);

/** Who tells the user that a screen failed (main.js: a toast). */
export function onError(f) { report = f; }

// a malformed address (#/patient/%E0) is read as it is, not refused
const decode = (s) => { try { return decodeURIComponent(s); } catch { return s; } };

/** Register a screen: `name` is the first part of the address (#/name/...). */
export function view(name, v) { views.set(name, v); }

/** The address now: { name, parts, params }. */
export function route() {
  const h = location.hash.replace(/^#\/?/, '');
  const [path, query = ''] = h.split('?');
  const parts = path.split('/').filter(Boolean).map(decode);
  return { name: parts[0] || 'cohorts', parts: parts.slice(1), params: new URLSearchParams(query) };
}

export function href(name, parts = [], params = {}) {
  const p = new URLSearchParams(Object.entries(params).filter(([, v]) => v !== undefined && v !== null && v !== ''));
  const q = p.toString();
  return `#/${[name, ...parts].map(encodeURIComponent).join('/')}${q ? `?${q}` : ''}`;
}

function arrows() {
  const max = +(sessionStorage.getItem('catalog.histmax') || 0);
  const back = document.getElementById('hist-back');
  const fwd = document.getElementById('hist-fwd');
  if (back) back.disabled = pos <= 0;
  if (fwd) fwd.disabled = pos >= max;
}

/** Go somewhere: a new history entry (or, with `replace`, change this one). */
export function go(to, { replace = false } = {}) {
  if (to === location.hash) return render();
  if (replace) {
    history.replaceState({ pos }, '', to);
  } else {
    pos += 1;
    sessionStorage.setItem('catalog.histmax', String(pos));
    history.pushState({ pos }, '', to);
  }
  return render();
}

/** Change the current address's parameters without a new history entry or a redraw. */
export function setParams(params) {
  const r = route();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '') r.params.delete(k); else r.params.set(k, v);
  }
  const q = r.params.toString();
  history.replaceState({ pos }, '', `#/${[r.name, ...r.parts].map(encodeURIComponent).join('/')}${q ? `?${q}` : ''}`);
}

/** The drawing now: a view keeps it before an await and compares after (`superseded`). */
export function navigation() { return renders; }
/** Whether a newer screen was drawn since `n` (an answer for `n` arrived late: dropped). */
export function superseded(n) { return n !== renders; }

export async function render() {
  const n = ++renders;
  const r = route();
  const v = views.get(r.name) ? r.name : 'cohorts';
  if (current && current !== v) views.get(current).hide?.();
  current = v;
  document.querySelectorAll('.view').forEach((s) => s.classList.toggle('hidden', s.id !== `view-${views.get(v).section || v}`));
  document.querySelectorAll('header.top [data-nav]').forEach((b) => b.classList.toggle('on', b.dataset.nav === (views.get(v).nav || v)));
  arrows();
  try {
    await views.get(v).show(r);
  } catch (e) {
    // the failure of a drawing a newer one replaced: nothing to tell
    if (superseded(n)) return;
    views.get(v).error?.(e);
    throw e;
  } finally {
    if (!superseded(n)) keepFocus(document.getElementById(`view-${views.get(v).section || v}`));
  }
}

/** Whether the focus is on nothing a keyboard user can see: the page itself, a removed or
 * hidden element. */
export function focusLost() {
  const a = document.activeElement;
  return !a || a === document.body || a === document.documentElement || !a.isConnected || !!a.closest('.hidden, [hidden]');
}

/** A screen just drawn: the focus moves to it when it was lost (left on the screen before, or
 * on the sign-in form), so that Tab goes on from there; a control that has it keeps it. */
export function keepFocus(screen) {
  if (!screen || !focusLost()) return;
  if (!screen.hasAttribute('tabindex')) screen.setAttribute('tabindex', '-1');
  screen.focus({ preventScroll: true });
}

export function start() {
  if (history.state && typeof history.state.pos === 'number') pos = history.state.pos;
  else history.replaceState({ pos }, '', location.hash || '#/cohorts');
  window.addEventListener('popstate', (e) => {
    pos = e.state?.pos ?? 0;
    render().catch(report);
  });
}

export const back = () => history.back();
export const forward = () => history.forward();
