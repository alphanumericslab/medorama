// The journey: a patient's data as one picture.  Time runs up (newest on top), one column per
// category (laboratory, vital signs and medications split into a thin column per test, sign or
// drug); busy periods get room and quiet stretches fold.  A long history scrolls under fixed
// column names.  Drawn as SVG; hover and click are found by position.  Each column's name is a
// button (Tab, then Enter or Space, shows or hides the column); the drawing itself is a picture
// whose text says its span and its columns: every mark is also an entry of the report.

import { dayKey, esc, fmtDate, fmtDayOfMonth, fmtHM, fmtMonth, fmtShort, fmtYear, zonedMidnight } from '../core.js';
import { color } from './charts.js';

const TOP = 104;
const BOTTOM = 12;
const AXIS = 62;
const SINGLE = 12;
/** The least height of a busy six-hour cell (12 px a day); below it the journey scrolls. */
const MIN_BIN = 3;
/** The least width of a mark that shows a critical value's outline (narrower: colour only). */
const OUTLINE_W = 4;

/** What the journey shows, in words (its picture's text): the span, the columns shown and hidden. */
export function journeyLabel(lanes, hidden, t0, t1) {
  const shown = lanes.filter((l) => !hidden.has(l.key)).map((l) => l.label);
  const off = lanes.filter((l) => hidden.has(l.key)).map((l) => l.label);
  return `The journey, ${fmtDate(t0)} to ${fmtDate(t1)}, the newest on top: ${shown.length} column${shown.length === 1 ? '' : 's'} shown (${shown.join(', ') || 'none'})${off.length ? `, ${off.length} hidden (${off.join(', ')})` : ''}. Every entry is also listed in the report.`;
}

/**
 * Draw `data` (GET patients/{id}/journey) into `el`.  opts: hidden (Set of category keys),
 * window ([t0, t1]: only that span, not folded), zoom (≥ 1), selected ({ t0, t1, lane }).
 * Returns { hits, yOf, height, top } for hover, click and scrolling: the column names, `top`
 * px high, stay in place while the rest scrolls, so a point `y` px below the top of `el` is at
 * `y < top ? y : y + el.scrollTop` in the hits' coordinates.
 */
export function drawJourney(el, data, opts = {}) {
  const hidden = opts.hidden || new Set();
  const BIN = data.bin;
  const lanes = data.lanes;
  const w = el.clientWidth;
  const h0 = el.clientHeight;
  // the time extent
  let lo = Infinity;
  let hi = -Infinity;
  for (const c of data.cells) { lo = Math.min(lo, c[2]); hi = Math.max(hi, c[2] + BIN); }
  for (const s of data.spans) { lo = Math.min(lo, s.t0); hi = Math.max(hi, s.t1); }
  if (!isFinite(lo)) {
    el.innerHTML = '<p class="placeholder">No dated data for this patient.</p>';
    return { hits: [], yOf: () => 0, height: 0, top: TOP };
  }
  let t0;
  let t1;
  if (opts.window) {
    [t0, t1] = opts.window;
  } else {
    t0 = lo - 86400;
    t1 = hi + 86400;
  }
  const b0 = Math.floor(t0 / BIN);
  const b1 = Math.ceil(t1 / BIN);
  const nb = b1 - b0;
  // busy bins: around every item; along spans of up to a month
  const busy = new Uint8Array(nb);
  const mark = (a, b) => {
    for (let i = Math.max(0, Math.floor(a / BIN) - b0); i <= Math.min(nb - 1, Math.floor(b / BIN) - b0); i++) busy[i] = 1;
  };
  if (opts.window) busy.fill(1);
  else {
    for (const c of data.cells) mark(c[2] - 30 * 3600, c[2] + 30 * 3600);
    for (const s of data.spans) {
      if (s.t1 - s.t0 <= 30 * 86400) mark(s.t0 - 30 * 3600, s.t1 + 30 * 3600);
      else { mark(s.t0 - 30 * 3600, s.t0 + 30 * 3600); mark(s.t1 - 30 * 3600, s.t1 + 30 * 3600); }
    }
  }
  const weight = (i) => (busy[i] ? 1 : 0.01);
  const cum = new Float64Array(nb + 1);
  for (let i = 0; i < nb; i++) cum[i + 1] = cum[i] + weight(i);
  let busyN = 0;
  for (let i = 0; i < nb; i++) busyN += busy[i];
  const h = Math.max(h0, Math.round(h0 * (opts.zoom || 1)), Math.ceil(TOP + BOTTOM + busyN * MIN_BIN + (cum[nb] - busyN) * MIN_BIN));
  const px = (h - TOP - BOTTOM) / cum[nb];
  const yOf = (t) => {
    const f = Math.max(0, Math.min(nb, t / BIN - b0));
    const k = Math.min(nb - 1, Math.floor(f));
    return h - BOTTOM - (cum[k] + (f - k) * weight(k)) * px;
  };
  // the columns
  const splitW = lanes.filter((l) => l.subs.length > 1).reduce((a, l) => a + l.subs.length, 0);
  const singles = lanes.filter((l) => l.subs.length === 1).length;
  const room = Math.max(0, w - AXIS - 8 - lanes.length * 2 - singles * SINGLE);
  const subW = splitW ? Math.max(2, room / splitW) : 0;
  const x0 = [];
  const sw = [];
  let x = AXIS + 4;
  for (const l of lanes) {
    const width = l.subs.length === 1 ? (splitW ? SINGLE : Math.max(SINGLE, room / singles + SINGLE)) : subW;
    sw.push(width);
    x0.push(x);
    x += l.subs.length * width + 2;
  }
  const right = x;
  const C = {
    text: color('text2'), muted: color('muted'), faint: color('faint'), grid: color('grid'), band: color('band'),
    hatch: color('hatch'), accent: color('accent'), bg: color('bg'), abn: color('abn'), crit: color('crit'),
  };
  const laneColor = lanes.map((l) => color(`c-${l.key}`));
  const hits = [];
  let head = `<svg class="jhead" width="${w}" height="${TOP}" viewBox="0 0 ${w} ${TOP}" role="group" aria-label="The journey's columns: each name shows or hides its column">`;
  let s = `<svg width="${w}" height="${h - TOP}" viewBox="0 ${TOP} ${w} ${h - TOP}" role="img" aria-label="${esc(journeyLabel(lanes, hidden, Math.max(t0, lo), Math.min(t1, hi)))}"><defs><pattern id="jhatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line x1="0" y1="0" x2="0" y2="6" stroke="${C.hatch}" stroke-width="2"/></pattern></defs>`;
  // headers: the legend, and the switch to hide a category (a button: pressed while shown)
  lanes.forEach((l, i) => {
    const off = hidden.has(l.key);
    const cw = l.subs.length * sw[i];
    head += `<g class="lanehead" role="button" tabindex="0" aria-label="${esc(l.label)}" aria-pressed="${!off}" data-lane="${i}" data-lane-key="${esc(l.key)}"><title>${esc(l.label)}: ${off ? 'hidden, press to show' : 'shown, press to hide'}</title>`;
    head += `<rect class="lhit" x="${(x0[i] - 1).toFixed(1)}" y="1" width="${(cw + 2).toFixed(1)}" height="${TOP - 2}" rx="3"/>`;
    head += `<rect x="${x0[i].toFixed(1)}" y="${TOP - 9}" width="${cw.toFixed(1)}" height="4" rx="2" fill="${off ? C.grid : laneColor[i]}"/>`;
    head += `<text transform="translate(${(x0[i] + cw / 2 + 3.5).toFixed(1)} ${TOP - 14}) rotate(-90)" fill="${off ? C.faint : C.text}"${off ? ' text-decoration="line-through"' : ''}>${esc(l.label)}</text></g>`;
    hits.push({ kind: 'head', lane: i, x0: x0[i] - 1, x1: x0[i] + cw + 1, y0: 0, y1: TOP });
  });
  // folded stretches
  let run = -1;
  for (let i = 0; i <= nb; i++) {
    const quiet = i < nb && !busy[i];
    if (quiet && run < 0) run = i;
    if (!quiet && run >= 0) {
      if ((i - run) * BIN > 20 * 86400) {
        const ya = h - BOTTOM - cum[i] * px;
        const yb = h - BOTTOM - cum[run] * px;
        s += `<rect x="${AXIS}" y="${ya.toFixed(1)}" width="${right - AXIS}" height="${Math.max(1, yb - ya).toFixed(1)}" fill="url(#jhatch)"/>`;
      }
      run = -1;
    }
  }
  // encounters: light bands behind everything
  const encLane = lanes.findIndex((l) => l.key === 'enc');
  for (const sp of data.spans) {
    if (sp.lane !== encLane) continue;
    const ya = yOf(sp.t1);
    const yb = yOf(sp.t0);
    s += `<rect x="${AXIS}" y="${(ya - 1).toFixed(1)}" width="${right - AXIS}" height="${Math.max(2, yb - ya + 2).toFixed(1)}" fill="${C.band}"/>`;
  }
  // days and months on the axis: at midnight in Medorama's time zone, a month told by its
  // date's key (YYYY-MM), whatever order dates are written in
  let lastMonth = '';
  let lastY = Infinity;
  const day0 = zonedMidnight(dayKey(t0)) ?? Math.floor(t0 / 86400) * 86400;
  for (let step = day0; step < t1; step += 86400) {
    const i0 = Math.floor(step / BIN) - b0;
    if (i0 < 0 || i0 >= nb || (!busy[i0] && !busy[Math.min(nb - 1, i0 + 2)])) continue;
    // the day's own midnight (a step drifts by an hour across a change of clocks)
    const t = zonedMidnight(dayKey(step + 6 * 3600));
    const y = yOf(t);
    s += `<line x1="${AXIS}" x2="${right}" y1="${y.toFixed(1)}" y2="${y.toFixed(1)}" stroke="${C.grid}" stroke-width=".6"/>`;
    const month = dayKey(t).slice(0, 7);
    if (month !== lastMonth) {
      s += `<text x="${AXIS - 8}" y="${(y + 3).toFixed(1)}" text-anchor="end" fill="${C.text}" font-weight="600">${esc(fmtMonth(t))} '${esc(fmtYear(t).slice(2))}</text>`;
      lastMonth = month;
      lastY = y;
    } else if (lastY - y >= 22) {
      s += `<text x="${AXIS - 8}" y="${(y + 3).toFixed(1)}" text-anchor="end" fill="${C.muted}">${esc(fmtDayOfMonth(t))}</text>`;
      lastY = y;
    }
  }
  // spans (stays, drugs, recordings), then cells; light spans (ICU stays) under the others
  const fill = (lane, level) => (level === 2 ? C.crit : level === 1 ? C.abn : laneColor[lane]);
  // a critical value: also outlined, when its mark is wide enough to show it (a second channel
  // beside the colour: abnormal and critical are two ambers)
  const outline = (level, width) => (level === 2 && width >= OUTLINE_W ? ` stroke="${C.text}" stroke-width="1"` : '');
  const spans = [...data.spans.filter((sp) => sp.light), ...data.spans.filter((sp) => !sp.light)];
  for (const sp of spans) {
    if (hidden.has(lanes[sp.lane].key)) continue;
    const ya = yOf(sp.t1);
    const yb = yOf(sp.t0);
    const xx = x0[sp.lane] + sp.sub * sw[sp.lane];
    const ww = sw[sp.lane];
    const op = sp.light ? 0.4 : lanes[sp.lane].key === 'med' ? 0.55 : lanes[sp.lane].key === 'enc' ? 0.9 : 1;
    // a recording whose end is not known yet (not indexed): faint, outlined dashed
    const est = sp.estimated ? ` stroke="${fill(sp.lane, sp.level)}" stroke-dasharray="2 2" fill-opacity="0.25"` : outline(sp.level, ww);
    s += `<rect x="${(xx + 0.4).toFixed(1)}" y="${ya.toFixed(1)}" width="${Math.max(1, ww - 0.8).toFixed(1)}" height="${Math.max(2.5, yb - ya).toFixed(1)}" rx="${ww > 6 ? 2 : 0.6}" fill="${fill(sp.lane, sp.level)}" opacity="${op}"${est}/>`;
    // at least 8 px to click, however short the span
    const mid = (ya + yb) / 2;
    hits.push({ kind: 'span', lane: sp.lane, sub: sp.sub, t0: sp.t0, t1: sp.t1, key: sp.key, level: sp.level, light: sp.light, x0: xx, x1: xx + ww, y0: Math.min(ya - 1, mid - 4), y1: Math.max(yb + 1, mid + 4) });
  }
  for (const [lane, sub, t, n, level] of data.cells) {
    if (hidden.has(lanes[lane].key)) continue;
    const ya = yOf(t + BIN);
    const yb = yOf(t);
    const xx = x0[lane] + sub * sw[lane];
    const ww = sw[lane];
    const split = lanes[lane].subs.length > 1;
    const op = level ? 1 : lanes[lane].key === 'vit' ? 0.62 : split ? 0.85 : 1;
    s += `<rect x="${(xx + 0.4).toFixed(1)}" y="${ya.toFixed(1)}" width="${Math.max(1, ww - 0.8).toFixed(1)}" height="${Math.max(2.2, yb - ya - 0.6).toFixed(1)}" rx="${ww > 6 ? 1.5 : 0.5}" fill="${fill(lane, level)}" opacity="${op}"${outline(level, ww)}/>`;
    hits.push({ kind: 'cell', lane, sub, t0: t, t1: t + BIN, n, level, x0: xx, x1: xx + ww, y0: Math.min(ya, yb - 2.2) - 0.5, y1: yb + 0.5 });
  }
  // the selected entry: a band across, its column outlined, its time on the axis
  const sel = opts.selected;
  if (sel && isFinite(sel.t0)) {
    const ya = yOf(Math.max(sel.t1 ?? sel.t0, sel.t0 + BIN * 0.5));
    const yb = yOf(sel.t0);
    s += `<rect x="${AXIS}" y="${(ya - 2).toFixed(1)}" width="${right - AXIS}" height="${Math.max(5, yb - ya + 4).toFixed(1)}" fill="${C.accent}" opacity=".10"/>`;
    const li = lanes.findIndex((l) => l.key === sel.lane);
    if (li >= 0 && !hidden.has(sel.lane)) {
      s += `<rect x="${(x0[li] - 1.5).toFixed(1)}" y="${(ya - 3).toFixed(1)}" width="${(lanes[li].subs.length * sw[li] + 3).toFixed(1)}" height="${Math.max(7, yb - ya + 6).toFixed(1)}" rx="3" fill="none" stroke="${C.accent}" stroke-width="1.6"/>`;
    }
    const ym = Math.max(TOP + 8, (ya + yb) / 2);
    s += `<rect x="2" y="${(ym - 8).toFixed(1)}" width="${AXIS - 8}" height="16" rx="4" fill="${C.accent}"/><text x="${(AXIS - 6) / 2 + 2}" y="${(ym + 3.5).toFixed(1)}" text-anchor="middle" fill="${C.bg}" font-weight="600">${esc(fmtShort(sel.t0))} ${esc(fmtHM(sel.t0))}</text>`;
  }
  el.innerHTML = `${head}</svg>${s}</svg>`;
  return { hits, yOf, height: h, top: TOP };
}

/** The hit under a point (the last drawn wins: cells over spans). */
export function hitAt(hits, x, y) {
  for (let i = hits.length - 1; i >= 0; i--) {
    const k = hits[i];
    if (x >= k.x0 && x <= k.x1 && y >= k.y0 && y <= k.y1) return k;
  }
  return null;
}
