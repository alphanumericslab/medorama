// Small charts of the patient page, drawn as SVG with presentation attributes only (the page's
// security policy allows no inline styles): sparklines, and the overview of an ICU recording.

import { esc, fmtHM, fmtShort } from '../core.js';

/** A colour of the current theme (style.css custom properties). */
export function color(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim() || '#999';
}

const finite = (x) => x !== null && x !== undefined && isFinite(x);

/** A sparkline of (t, v) points: the reference range as a band, the last point marked. */
export function spark(points, lo, hi, { w = 120, h = 22, stroke = color('c-lab'), mark = null } = {}) {
  const pts = points.filter((p) => finite(p[0]) && finite(p[1]));
  if (!pts.length) return '';
  const vs = pts.map((p) => p[1]);
  let mn = Math.min(...vs, finite(lo) ? lo : Infinity);
  let mx = Math.max(...vs, finite(hi) ? hi : -Infinity);
  if (mx === mn) { mx += 1; mn -= 1; }
  const t0 = pts[0][0];
  const t1 = pts[pts.length - 1][0] === t0 ? t0 + 1 : pts[pts.length - 1][0];
  const x = (t) => 2 + ((t - t0) / (t1 - t0)) * (w - 4);
  const y = (v) => 2 + (1 - (v - mn) / (mx - mn)) * (h - 4);
  let s = `<svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" aria-hidden="true">`;
  if (finite(lo) || finite(hi)) {
    const a = y(finite(hi) ? hi : mx);
    const b = y(finite(lo) ? lo : mn);
    s += `<rect x="0" y="${a.toFixed(1)}" width="${w}" height="${Math.max(1, b - a).toFixed(1)}" fill="${color('soft2')}"/>`;
  }
  s += `<polyline points="${pts.map((p) => `${x(p[0]).toFixed(1)},${y(p[1]).toFixed(1)}`).join(' ')}" fill="none" stroke="${stroke}" stroke-width="1.5" stroke-linejoin="round"/>`;
  const last = mark ?? pts[pts.length - 1];
  const out = (finite(hi) && last[1] > hi) || (finite(lo) && last[1] < lo);
  s += `<circle cx="${x(last[0]).toFixed(1)}" cy="${y(last[1]).toFixed(1)}" r="2.6" fill="${out ? color('warn') : stroke}"/>`;
  return `${s}</svg>`;
}

/**
 * The whole of a recording at a glance: its main vital signs over time (with their range, for a
 * series found from the waveforms: a Holter's heart rate), its gaps.  Returns the SVG and a
 * function from a click's x to a time.
 */
/** 1, 2 or 5 times a power of ten, at least `x`. */
function niceStep(x) {
  const p = 10 ** Math.floor(Math.log10(Math.max(x, 1e-9)));
  return [1, 2, 5, 10].map((m) => m * p).find((m) => m >= x);
}

/** A monitored parameter's colour, as VitalStream draws it: heart rate as the ECG, pressures,
 * saturation, respiration and CO2, temperature; others blue. */
export function paramColor(name) {
  const n = String(name).toLowerCase();
  if (/\b(hr|heart|pulse|pvc|st[- ]?)/.test(n)) return color('p-ecg');
  if (/\b(n?i?bp|abp|art|map|cvp|pap|icp|pressure|sys|dia)/.test(n)) return color('p-pressure');
  if (/spo2|sat|pleth|svo2/.test(n)) return color('p-spo2');
  if (/\b(resp|rr|co2|etco2|awrr)/.test(n)) return color('p-resp');
  if (/temp|\bt[0-9]?\b/.test(n)) return color('p-temp');
  return color('c-icu');
}

export function overview(trends, gaps, t0, t1, { w = 560, cursor = null, rowH = 46 } = {}) {
  const LH = rowH;
  const left = trends.some((tr) => tr.name.length > 9) ? 100 : 74;
  const W = w - left;
  const n = Math.max(1, trends.length);
  const H = n * LH;
  const X = (t) => left + ((t - t0) / Math.max(1, t1 - t0)) * W;
  let s = `<svg width="${w}" height="${H + 22}" viewBox="0 0 ${w} ${H + 22}">`;
  // time ticks: hours for a short recording, days for a long one
  const span = t1 - t0;
  const step = span > 2 * 86400 ? 86400 : span > 6 * 3600 ? 3 * 3600 : span > 3600 ? 1800 : 300;
  for (let t = Math.ceil(t0 / step) * step; t <= t1; t += step) {
    s += `<line x1="${X(t).toFixed(1)}" x2="${X(t).toFixed(1)}" y1="0" y2="${H}" stroke="${color('grid')}"/>`;
    s += `<text x="${(X(t) + 3).toFixed(1)}" y="${H + 15}" fill="${color('muted')}">${esc(step >= 86400 ? fmtShort(t) : fmtHM(t))}</text>`;
  }
  for (const [a, b] of gaps || []) {
    if (b < t0 || a > t1) continue;
    s += `<rect x="${X(Math.max(a, t0)).toFixed(1)}" y="0" width="${Math.max(2, X(Math.min(b, t1)) - X(Math.max(a, t0))).toFixed(1)}" height="${H}" fill="${color('hatch')}"/>`;
  }
  trends.forEach((tr, k) => {
    const y0 = k * LH;
    const band = tr.lo && tr.hi && tr.lo.length === tr.t.length;
    const vs = [...tr.v, ...(band ? [...tr.lo, ...tr.hi] : [])].filter(finite);
    let mn = Math.min(...vs);
    let mx = Math.max(...vs);
    if (!isFinite(mn)) return;
    if (mx - mn < 1e-6) { mx += 1; mn -= 1; }
    const pad = (mx - mn) * 0.1;
    mn -= pad; mx += pad;
    const Y = (v) => y0 + 6 + (1 - (v - mn) / (mx - mn)) * (LH - 12);
    // runs of points without a break (none where the recording has none for a while)
    const runs = [];
    for (let i = 0; i < tr.t.length; i++) {
      if (!finite(tr.v[i])) continue;
      const brk = !runs.length || tr.t[i] - tr.t[runs.at(-1).at(-1)] > Math.max(120, (t1 - t0) / 150) || runs.at(-1).at(-1) !== i - 1;
      if (brk) runs.push([i]); else runs.at(-1).push(i);
    }
    const pt = (i, v) => `${X(tr.t[i]).toFixed(1)},${Y(v).toFixed(1)}`;
    const d = runs.map((r) => `M${r.map((i) => pt(i, tr.v[i])).join('L')}`).join('');
    if (band) {
      const area = runs.filter((r) => r.length > 1).map((r) => `M${r.map((i) => pt(i, tr.hi[i])).join('L')}L${[...r].reverse().map((i) => pt(i, tr.lo[i])).join('L')}Z`).join('');
      s += `<path d="${area}" fill="${paramColor(tr.name)}" opacity=".18"/>`;
    }
    // a tall row gets a value scale
    if (LH >= 100) {
      const st = niceStep((mx - mn) / 4);
      for (let v = Math.ceil(mn / st) * st; v <= mx; v += st) {
        s += `<line x1="${left}" x2="${w}" y1="${Y(v).toFixed(1)}" y2="${Y(v).toFixed(1)}" stroke="${color('grid')}" stroke-dasharray="2 3"/>`;
        s += `<text x="${left - 6}" y="${(Y(v) + 3.5).toFixed(1)}" text-anchor="end" fill="${color('muted')}">${esc(String(+v.toFixed(6)))}</text>`;
      }
    }
    if (k) s += `<line x1="${left}" x2="${w}" y1="${y0}" y2="${y0}" stroke="${color('grid')}"/>`;
    s += `<text x="0" y="${y0 + 18}" fill="${color('text2')}" font-weight="600">${esc(tr.name)}</text>`;
    s += `<text x="0" y="${y0 + 31}" fill="${color('muted')}">${esc(tr.unit || '')}</text>`;
    s += `<path d="${d}" fill="none" stroke="${paramColor(tr.name)}" stroke-width="1.4" stroke-linejoin="round"/>`;
  });
  if (finite(cursor) && cursor >= t0 && cursor <= t1) s += `<line x1="${X(cursor).toFixed(1)}" x2="${X(cursor).toFixed(1)}" y1="0" y2="${H}" stroke="${color('accent')}" stroke-width="1.4"/>`;
  s += '</svg>';
  return { svg: s, timeAt: (x) => t0 + ((x - left) / W) * (t1 - t0), left };
}
