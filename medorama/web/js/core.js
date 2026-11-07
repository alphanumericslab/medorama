// What every screen of the catalog page uses: the API, the signed-in user, formatting, small
// DOM helpers.  Nothing here decides what a user may see: the server does (gate.rs).  In
// Privacy Mode it sends no identifier, and dates moved back; the page shows what it is given.

export const API = 'api/v1/';
export const PAGE = 100;
export const $ = (s, root = document) => root.querySelector(s);
export const $$ = (s, root = document) => Array.from(root.querySelectorAll(s));

/** Shared state: the configuration and user, set at sign-in; `redrawing` while the page
 * redraws on its own (new data), its requests then background ones. */
export const st = { cfg: null, user: null, redrawing: false };

export function esc(t) {
  return String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

/** Text with the given words marked (case ignored); the text is escaped. */
export function mark(text, words) {
  const ws = (words || []).filter((w) => w && w.length > 1).map((w) => String(w).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  if (!ws.length) return esc(text);
  // found in the text as it is, then each part escaped (a word such as "amp" never meets an
  // entity the escaping wrote)
  return String(text ?? '').split(new RegExp(`(${ws.join('|')})`, 'gi')).map((p, i) => (i % 2 ? `<mark>${esc(p)}</mark>` : esc(p))).join('');
}

export const store = {
  get(k, d) { try { const v = localStorage.getItem(`catalog.${k}`); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(`catalog.${k}`, JSON.stringify(v)); } catch { /* private mode */ } },
};

let onUnauthorized = () => {};
export function setUnauthorized(f) { onUnauthorized = f; }

export async function api(path, opts = {}) {
  const init = { method: opts.method || 'GET', headers: {}, credentials: 'same-origin' };
  // a request the page gave up on (a newer search typed): aborted
  if (opts.signal) init.signal = opts.signal;
  if (init.method !== 'GET') init.headers['X-Medorama'] = '1';
  // the page's own refresh, not the user's: it does not keep an idle session alive
  if (opts.background || st.redrawing) init.headers['X-Background'] = '1';
  if (opts.json !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(opts.json);
  } else if (opts.form !== undefined) {
    init.body = opts.form; // FormData: the browser writes its type and boundary
  } else if (opts.body !== undefined) {
    init.headers['Content-Type'] = 'application/octet-stream';
    init.body = opts.body;
  }
  const r = await fetch(API + path, init);
  let data = null;
  try { data = await r.json(); } catch { /* empty */ }
  if (r.status === 401 && path !== 'login') {
    onUnauthorized();
    throw new Error('Please sign in.');
  }
  if (!r.ok) throw new Error((data && data.detail) || `${r.status} ${r.statusText}`);
  return data;
}

// ------------------------------------------------------------ Privacy Mode
// The server decides and does it all (gate.rs): a session in Privacy Mode is answered with no
// identifier (placeholders and pseudonyms in their place), its dates moved back, ages capped.
// The page asks the server to switch (`POST privacy`: on for anyone, off only with the PHI
// permission) and reloads, so that nothing identified stays on screen.  Dates typed in the view
// are sent as typed: the server reads them as dates as shown, for each patient (it alone knows
// how far their dates move).
export const privacy = {
  get on() { return !!st.user?.privacy; },
  /** May the user leave Privacy Mode (the PHI permission). */
  get allowed() { return !!st.user?.phi_allowed; },
  async set(on) {
    await api('privacy', { method: 'POST', json: { on } });
    location.reload();
  },
  hides(f) { return this.on && (st.cfg?.deid?.fields || []).includes(f); },
};
export function qs(obj) {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(obj)) if (v !== undefined && v !== null && v !== '') p.set(k, v);
  return p.toString();
}

export function toast(msg, isErr = false) {
  const t = $('#toast');
  t.textContent = msg;
  t.classList.toggle('err', isErr);
  t.classList.remove('hidden');
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add('hidden'), isErr ? 7000 : 3500);
}
export const fail = (e) => toast(e.message || String(e), true);

export const isAdmin = () => st.user?.role === 'admin';
export const canRun = () => st.user && (st.user.role === 'admin' || st.user.role === 'analyst');
export const phi = () => !!st.user?.phi;
/** Whether ages are shown (from the date of birth, on the server: PHI permission or Privacy Mode). */
export const ages = () => !!(st.user?.phi || st.user?.privacy);

// ------------------------------------------------------------ time, in the catalog's time zone
const fmts = new Map();
function parts(t, opts) {
  const key = JSON.stringify(opts);
  if (!fmts.has(key)) fmts.set(key, new Intl.DateTimeFormat('en-US', { timeZone: st.cfg?.timezone || 'UTC', ...opts }));
  return Object.fromEntries(fmts.get(key).formatToParts(new Date(t * 1000)).map((x) => [x.type, x.value]));
}
export function resetFormats() { fmts.clear(); }
const ok = (t) => t !== null && t !== undefined && isFinite(t);
// How dates and times are written (Admin › Dates and times): the order of a date's parts and
// the clock.  Grouping keys (dayKey) and dates sent to the server stay ISO.
const shown = () => st.cfg?.display || { date: 'ymd', clock: '24h' };
/** Written-out dates month first (Jan 2, 2024): unless a day-first order is chosen. */
const us = () => !shown().date.startsWith('dmy');
/** A date's numeric parts in the chosen order. */
function dateText(p) {
  switch (shown().date) {
    case 'mdy': return `${p.month}/${p.day}/${p.year}`;
    case 'dmy': return `${p.day}/${p.month}/${p.year}`;
    case 'dmy_dot': return `${p.day}.${p.month}.${p.year}`;
    default: return `${p.year}-${p.month}-${p.day}`;
  }
}
/** A time of day on the chosen clock: 14:05, or 2:05 PM. */
function clockText(p, seconds) {
  const s = seconds ? `:${p.second}` : '';
  if (shown().clock !== '12h') return `${p.hour}:${p.minute}${s}`;
  const h = +p.hour;
  return `${h % 12 || 12}:${p.minute}${s} ${h < 12 ? 'AM' : 'PM'}`;
}
/** 2024-01-02 06:10, or as chosen (tables, logs) */
export function fmtTime(t, seconds = false) {
  if (!ok(t)) return '—';
  const p = parts(t, { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: seconds ? '2-digit' : undefined, hourCycle: 'h23' });
  return `${dateText(p)} ${clockText(p, seconds)}`;
}
/** A date alone, as chosen: 2024-01-02 */
export function fmtNumDate(t) {
  if (!ok(t)) return '—';
  return dateText(parts(t, { year: 'numeric', month: '2-digit', day: '2-digit' }));
}
/** Jan 2, 2024 (or 2 Jan 2024 with a day-first order) */
export function fmtDate(t) {
  if (!ok(t)) return '—';
  const p = parts(t, { year: 'numeric', month: 'short', day: 'numeric' });
  return us() ? `${p.month} ${p.day}, ${p.year}` : `${p.day} ${p.month} ${p.year}`;
}
/** 2 (the day of the month) */
export function fmtDayOfMonth(t) {
  return ok(t) ? parts(t, { day: 'numeric' }).day : '';
}
/** Jan (a month's short name) */
export function fmtMonth(t) {
  return ok(t) ? parts(t, { month: 'short' }).month : '';
}
/** 2024 (in the catalog's time zone, as the view shows it) */
export function fmtYear(t) {
  return ok(t) ? parts(t, { year: 'numeric' }).year : '';
}
/** Jan 2 (or 2 Jan) */
export function fmtShort(t) {
  if (!ok(t)) return '—';
  const p = parts(t, { month: 'short', day: 'numeric' });
  return us() ? `${p.month} ${p.day}` : `${p.day} ${p.month}`;
}
/** Tuesday, Jan 2, 2024 (or Tuesday, 2 Jan 2024) */
export function fmtDay(t) {
  if (!ok(t)) return 'Undated';
  const p = parts(t, { weekday: 'long', year: 'numeric', month: 'short', day: 'numeric' });
  return us() ? `${p.weekday}, ${p.month} ${p.day}, ${p.year}` : `${p.weekday}, ${p.day} ${p.month} ${p.year}`;
}
/** 06:10, or 6:10 AM */
export function fmtHM(t) {
  if (!ok(t)) return '';
  return clockText(parts(t, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }), false);
}
/** YYYY-MM-DD of a time (grouping by day) */
export function dayKey(t) {
  if (!ok(t)) return '';
  const p = parts(t, { year: 'numeric', month: '2-digit', day: '2-digit' });
  return `${p.year}-${p.month}-${p.day}`;
}
export function fmtDuration(s) {
  if (!ok(s)) return '—';
  // rounded once, then split (never "60 min" or "1 d 24 h")
  const min = Math.round(s / 60);
  if (min < 60) return `${min} min`;
  if (s < 2 * 86400) return `${(s / 3600).toFixed(1)} h`;
  const h = Math.round(s / 3600);
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}
/** A date (YYYY-MM-DD) at midnight in the catalog's time zone, as Unix seconds. */
export function zonedMidnight(d) {
  if (!d) return undefined;
  const [y, m, day] = d.split('-').map(Number);
  const guess = Date.UTC(y, m - 1, day) / 1000;
  // the zone's wall clock at that instant, read from its parts (never from shown text)
  const p = parts(guess, { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23' });
  return guess - (Date.UTC(+p.year, +p.month - 1, +p.day, +p.hour, +p.minute, +p.second) / 1000 - guess);
}

/** The end of a date (YYYY-MM-DD) in the catalog's time zone: the next day's midnight (a day
 * of a change of clocks lasts 23 or 25 hours). */
export function zonedDayEnd(d) {
  if (!d) return undefined;
  const [y, m, day] = d.split('-').map(Number);
  const next = new Date(Date.UTC(y, m - 1, day + 1));
  const pad = (n, w = 2) => String(n).padStart(w, '0');
  return zonedMidnight(`${pad(next.getUTCFullYear(), 4)}-${pad(next.getUTCMonth() + 1)}-${pad(next.getUTCDate())}`);
}

// ------------------------------------------------------------ recordings and the viewer
export const STATUS_TEXT = {
  ready: 'ready', new: 'not indexed', queued: 'queued', indexing: 'indexing', stale: 'changed',
  failed: 'failed', missing: 'missing', none: 'no recordings',
};
export function dot(status, progress) {
  const s = status || 'none';
  const pct = s === 'indexing' && progress ? `<small>${Math.round(progress * 100)}%</small>` : '';
  return `<span class="status" title="${esc(STATUS_TEXT[s] || s)}"><span class="dot ${esc(s)}"></span>${pct}</span>`;
}
export function statusCounts(c) {
  return Object.entries(c || {}).map(([k, n]) => `${n} ${STATUS_TEXT[k] || k}`).join(', ');
}
/** `embedded` (the catalog runs it), `link` (elsewhere: viewer_url) or `none`. */
export const viewerMode = () => st.cfg?.viewer?.mode || 'none';
/** The hash of the viewer's own page for a recording, optionally at a time span. */
export function viewerHash(path, t0, t1) {
  const p = { path };
  if (ok(t0) && ok(t1)) { p.t0 = (+t0).toFixed(3); p.t1 = (+t1).toFixed(3); }
  return new URLSearchParams(p).toString();
}
export function externalViewerLink(path, t0, t1) {
  const base = (st.cfg?.viewer?.url || '').replace(/#.*$/, '');
  return `${base}#${viewerHash(path, t0, t1)}`;
}

/** A removal's counts, in words. */
export function removalText(r) {
  const parts2 = [['recording', r.recordings], ['patient', r.patients], ['encounter', r.encounters], ['bed stay', r.bed_stays], ['item', r.ehr_items]]
    .filter(([, n]) => n).map(([w, n]) => `${n} ${w}${n === 1 ? '' : 's'}`);
  let t = parts2.length ? parts2.join(', ') : 'nothing';
  if (r.kept_recordings || r.kept_encounters) t += `\n(${r.kept_recordings || 0} recording(s) and ${r.kept_encounters || 0} encounter(s) stay: another database also has them)`;
  return t;
}

/** The colour class of a category (style.css: .c-lab …). */
export const catClass = (k) => `c-${k || 'other'}`;

