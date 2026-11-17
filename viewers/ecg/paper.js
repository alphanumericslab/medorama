// An ECG on standard paper, as SVG in millimetres: 1 mm and 5 mm grid, 25 mm/s, 10 mm/mV,
// a 1 mV calibration pulse at the start of each row.  Layouts: the standard 3 × 4 and 6 × 2, each
// with a lead II rhythm strip, and 12 × 1; records that are not 12-lead ECGs show every lead in full.
// A record whose sampling rate is unknown is drawn by sample number (500 samples per 25 mm at
// 25 mm/s), with the sample numbers under the paper.  The device's median beats (drawMedian) are
// drawn on paper of their own, 3 × 4, with the P, QRS and T onsets and offsets marked.
// The baseline: each strip (a lead's segment on the paper) is drawn about its median, and an
// optional 0.5 Hz high-pass and 50 or 60 Hz notch, both zero-phase (run forward then backward),
// clean the signal first.  The samples of the record are never changed.
// A classic script (it works from file://), and a CommonJS module for tests.

(function (root) {
  'use strict';

  const LAYOUTS = {
    '3x4': { cols: 4, rows: [['I', 'aVR', 'V1', 'V4'], ['II', 'aVL', 'V2', 'V5'], ['III', 'aVF', 'V3', 'V6']], rhythm: 'II' },
    '6x2': { cols: 2, rows: [['I', 'V1'], ['II', 'V2'], ['III', 'V3'], ['aVR', 'V4'], ['aVL', 'V5'], ['aVF', 'V6']], rhythm: 'II' },
    '12x1': { cols: 1, rows: [['I'], ['II'], ['III'], ['aVR'], ['aVL'], ['aVF'], ['V1'], ['V2'], ['V3'], ['V4'], ['V5'], ['V6']], rhythm: null },
  };

  const esc = (t) => String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const f2 = (x) => (Math.round(x * 100) / 100).toString();

  // ---------------------------------------------------------------- filters
  /**
   * A biquad { b: [b0, b1, b2], a: [a1, a2] } (a0 = 1) run over x forward then backward, so with
   * no phase shift: each end extended by its odd reflection (up to 1 s) and the filter started in
   * its steady state for the first value, so that the ends do not ring.  Gaps (NaN) are held at
   * the last value while filtering and are gaps again in the result.
   */
  function filtfilt(x, fs, f) {
    const n = x.length;
    const out = new Float64Array(n);
    let first = -1;
    for (let i = 0; i < n; i++) if (Number.isFinite(x[i])) { first = i; break; }
    if (first < 0) return out.fill(NaN);
    // gaps held
    const y = new Float64Array(n);
    let last = x[first];
    for (let i = 0; i < n; i++) { if (Number.isFinite(x[i])) last = x[i]; y[i] = last; }
    if (n < 2) return Float64Array.from(x);
    const pad = Math.min(n - 1, Math.max(1, Math.round(fs)));
    const ext = new Float64Array(n + 2 * pad);
    for (let i = 0; i < pad; i++) ext[i] = 2 * y[0] - y[pad - i];
    ext.set(y, pad);
    for (let i = 0; i < pad; i++) ext[pad + n + i] = 2 * y[n - 1] - y[n - 2 - i];
    const [b0, b1, b2] = f.b;
    const [a1, a2] = f.a;
    const gain = (b0 + b1 + b2) / (1 + a1 + a2);
    // transposed direct form II, from the steady state of a constant input x0
    const run = (v) => {
      let z1 = (gain - b0) * v[0];
      let z2 = (b2 - a2 * gain) * v[0];
      for (let i = 0; i < v.length; i++) {
        const xi = v[i];
        const yi = b0 * xi + z1;
        z1 = b1 * xi - a1 * yi + z2;
        z2 = b2 * xi - a2 * yi;
        v[i] = yi;
      }
    };
    run(ext);
    ext.reverse();
    run(ext);
    ext.reverse();
    for (let i = 0; i < n; i++) out[i] = Number.isFinite(x[i]) ? ext[pad + i] : NaN;
    return out;
  }

  /** A first-order Butterworth high-pass at fc Hz (bilinear transform); run forward and backward, −6 dB at fc. */
  function highpassFilter(fs, fc) {
    const k = Math.tan((Math.PI * fc) / fs);
    return { b: [1 / (1 + k), -1 / (1 + k), 0], a: [(k - 1) / (k + 1), 0] };
  }

  /** A notch at f0 Hz of quality q (the bandwidth f0 / q), as in the Audio EQ Cookbook. */
  function notchFilter(fs, f0, q = 30) {
    const w = (2 * Math.PI * f0) / fs;
    const alpha = Math.sin(w) / (2 * q);
    const a0 = 1 + alpha;
    const c = -2 * Math.cos(w) / a0;
    return { b: [1 / a0, c, 1 / a0], a: [c, (1 - alpha) / a0] };
  }

  /** v high-passed at fc Hz (0.5 by default), with no phase shift. */
  const highpass = (v, fs, fc = 0.5) => filtfilt(v, fs, highpassFilter(fs, fc));
  /** v without f0 Hz (50 or 60, mains), with no phase shift; unchanged when f0 is not below fs / 2. */
  const notch = (v, fs, f0) => (f0 > 0 && f0 < fs / 2 ? filtfilt(v, fs, notchFilter(fs, f0)) : Float64Array.from(v));

  /** The median of the finite samples v[a..b), or 0 when there are none. */
  function median(v, a = 0, b = v.length) {
    const xs = [];
    for (let i = Math.max(0, a); i < Math.min(v.length, b); i++) if (Number.isFinite(v[i])) xs.push(v[i]);
    if (!xs.length) return 0;
    xs.sort((p, q) => p - q);
    const m = xs.length >> 1;
    return xs.length % 2 ? xs[m] : (xs[m - 1] + xs[m]) / 2;
  }

  /**
   * The samples the paper draws of a lead: its first `seconds` (and 2 s more, so the filters'
   * ends fall off the paper), high-passed at 0.5 Hz when `highpass`, without mains when `notch`
   * (50 or 60 Hz).  The lead's own samples when neither is on.
   */
  function conditioned(lead, seconds, opts) {
    const hp = opts.highpass === true ? 0.5 : +opts.highpass || 0;
    const f0 = +opts.notch || 0;
    if (!hp && !f0) return lead.v;
    let v = lead.v.subarray ? lead.v.subarray(0, Math.min(lead.v.length, Math.ceil((seconds + 2) * lead.fs))) : lead.v;
    if (hp > 0 && hp < lead.fs / 2) v = highpass(v, lead.fs, hp);
    if (f0) v = notch(v, lead.fs, f0);
    return v;
  }

  /** The path of samples v[a..b), less `off` mV, at `speed` mm/s and `gain` mm/mV, from (x0, y0) mm. */
  function trace(v, fs, a, b, x0, y0, speed, gain, off = 0) {
    let d = '';
    let pen = false;
    for (let n = Math.max(0, a); n < Math.min(v.length, b); n++) {
      const x = x0 + ((n - a) / fs) * speed;
      const y = v[n] - off;
      if (!Number.isFinite(y)) { pen = false; continue; }
      d += `${pen ? 'L' : 'M'}${f2(x)} ${f2(y0 - y * gain)}`;
      pen = true;
    }
    return d;
  }

  // the rate a record of unknown rate is drawn at: its samples by number, 500 per 25 mm at 25 mm/s
  const NOMINAL = 500;

  /**
   * snap(x, y) of drawn strips (for calipers): the sample nearest (x, y) mm, within ±40 ms along
   * the paper: { x, y, v (mV, as drawn: about the strip's median, filtered), t (s from the
   * record's start), n (the sample's number), lead }.
   */
  function snapper(placed, speed, gain) {
    return (x, y) => {
      let best = null;
      for (const c of placed) {
        const { fs } = c;
        const xEnd = c.x0 + ((c.b - c.a) / fs) * speed;
        if (x < c.x0 - 0.5 || x > xEnd + 0.5) continue;
        const n0 = c.a + Math.round(((x - c.x0) / speed) * fs);
        const w = Math.max(1, Math.round(0.04 * fs));
        for (let n = Math.max(c.a, n0 - w); n < Math.min(c.b, n0 + w + 1); n++) {
          const v = c.v[n] - c.off;
          if (!Number.isFinite(v)) continue;
          const px = c.x0 + ((n - c.a) / fs) * speed;
          const py = c.y0 - v * gain;
          const d2 = (px - x) ** 2 + (py - y) ** 2;
          if (!best || d2 < best.d2) best = { d2, x: px, y: py, v, t: c.t0 + (n - c.a) / fs, n, lead: c.name };
        }
      }
      return best;
    };
  }

  /** The paper: the grid of 1 mm and 5 mm, width × height mm. */
  function sheet(width, height) {
    let s = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" class="paper">`;
    s += '<defs><pattern id="g5" width="5" height="5" patternUnits="userSpaceOnUse">'
      + '<path class="minor" d="M1 0V5M2 0V5M3 0V5M4 0V5M0 1H5M0 2H5M0 3H5M0 4H5"/>'
      + '<path class="major" d="M0 0V5M0 0H5"/></pattern></defs>';
    s += `<rect class="sheet" width="${width}" height="${height}"/><rect width="${width}" height="${height}" fill="url(#g5)"/>`;
    s += `<path class="major" d="M${width} 0V${height}M0 ${height}H${width}"/>`;
    return s;
  }

  /** A calibration pulse (1 mV, 200 ms) ending at x0, on the baseline y0. */
  function pulse(x0, y0, speed, gain) {
    const w = 0.2 * speed;
    const a = x0 - w - 2;
    return `M${f2(a - 1.5)} ${f2(y0)}L${f2(a)} ${f2(y0)}L${f2(a)} ${f2(y0 - gain)}L${f2(a + w)} ${f2(y0 - gain)}L${f2(a + w)} ${f2(y0)}L${f2(x0)} ${f2(y0)}`;
  }

  /**
   * Draw `rec` (see ecg.js) on paper.  opts: layout ('3x4', '6x2', '12x1'), speed (mm/s),
   * gain (mm/mV), seconds (the strip's length, default 10), twelve (leads by name, from
   * ECG.twelveLead, or null), median (each strip drawn about its median; true unless false),
   * highpass (true for 0.5 Hz, or a cut-off in Hz; off by default), notch (50 or 60 Hz; off by
   * default).  Returns { svg, width, height (mm), seconds, samples (true: drawn by sample
   * number), toTime(x mm), snap(x, y) }: `snap` finds the point of a trace nearest a point of the
   * paper (for calipers).  A record of unknown rate (rec.rateUnknown) is drawn by sample number,
   * without filters (they need the rate).
   */
  function draw(rec, opts) {
    const bySample = !!rec.rateUnknown;
    if (bySample) {
      const as = new Map(rec.leads.map((l) => [l, { ...l, fs: NOMINAL }]));
      rec = { ...rec, fs: NOMINAL, leads: rec.leads.map((l) => as.get(l)) };
      const twelve = opts.twelve && Object.fromEntries(Object.entries(opts.twelve).map(([k, l]) => [k, as.get(l) || { ...l, fs: NOMINAL }]));
      opts = { ...opts, twelve, highpass: false, notch: 0 };
    }
    const speed = opts.speed || 25;
    const gain = opts.gain || 10;
    const seconds = opts.seconds || Math.min(10, Math.max(...rec.leads.map((l) => l.v.length / l.fs)));
    const left = 14;          // room for the calibration pulse
    const top = 6;
    const stripW = seconds * speed;
    const rowH = Math.max(3.2 * gain, 22);
    const twelve = opts.twelve;
    const L = twelve ? LAYOUTS[opts.layout] || LAYOUTS['3x4'] : null;
    const rows = [];
    const signal = new Map();       // lead -> the samples drawn (conditioned once, also for the rhythm strip)
    const samples = (lead) => {
      if (!signal.has(lead)) signal.set(lead, conditioned(lead, seconds, opts));
      return signal.get(lead);
    };
    if (L) {
      const colS = seconds / L.cols;
      L.rows.forEach((names) => rows.push(names.map((nm, c) => ({ lead: twelve[nm], name: nm, t0: c * colS, t1: (c + 1) * colS }))));
      if (L.rhythm) rows.push([{ lead: twelve[L.rhythm], name: L.rhythm, t0: 0, t1: seconds, rhythm: true }]);
    } else {
      rec.leads.forEach((lead) => rows.push([{ lead, name: lead.name, t0: 0, t1: seconds }]));
    }
    const width = Math.ceil((left + stripW + 6) / 5) * 5;
    const axisH = bySample ? 5 : 0;      // the sample numbers under the paper
    const height = Math.ceil((top + rows.length * rowH + 8 + axisH) / 5) * 5;
    let s = sheet(width, height);
    let paths = '';
    let labels = '';
    const placed = [];
    rows.forEach((cells, r) => {
      const y0 = top + r * rowH + rowH * 0.62;
      paths += pulse(left, y0, speed, gain);
      cells.forEach((cell, c) => {
        const x0 = left + cell.t0 * speed;
        const lead = cell.lead;
        const a = Math.round(cell.t0 * lead.fs);
        const b = Math.round(cell.t1 * lead.fs);
        const v = samples(lead);
        const off = opts.median === false ? 0 : median(v, a, b);
        paths += trace(v, lead.fs, a, b, x0, y0, speed, gain, off);
        placed.push({ lead, fs: lead.fs, v, off, name: cell.name, a, b: Math.min(b, v.length), x0, y0, t0: cell.t0 });
        // the lead's name, and a mark where the lead changes
        labels += `<text x="${f2(x0 + 1.2)}" y="${f2(y0 - rowH * 0.42)}">${esc(cell.name)}</text>`;
        if (c > 0) paths += `M${f2(x0)} ${f2(y0 - 2.5)}V${f2(y0 + 2.5)}`;
      });
    });
    s += `<path class="trace" d="${paths}"/>`;
    if (bySample) {
      // the sample numbers, every 25 mm, under the last row
      const y = top + rows.length * rowH + 3.5;
      const per = (NOMINAL * 25) / speed;
      for (let x = 0; x <= stripW + 0.01; x += 25) {
        labels += `<text class="axis" x="${f2(left + x)}" y="${f2(y)}" text-anchor="middle">${Math.round((x / 25) * per)}</text>`;
      }
    }
    s += `<g class="labels">${labels}</g>`;
    const hp = opts.highpass === true ? 0.5 : +opts.highpass || 0;
    const filt = [hp ? `${hp} Hz high-pass` : '', +opts.notch ? `${+opts.notch} Hz notch` : ''].filter(Boolean).map((t) => `   ${t}`).join('');
    const xs = bySample ? `${(NOMINAL * 5) / speed} samples per 5 mm (rate unknown)` : `${speed} mm/s`;
    s += `<text class="foot" x="${left}" y="${height - 2.2}">${esc(xs)}   ${gain} mm/mV${bySample ? '' : `   ${esc(rec.fs)} Hz`}${esc(filt)}</text>`;
    s += '<g class="calipers"></g></svg>';
    const snap = snapper(placed, speed, gain);
    return { svg: s, width, height, seconds, left, speed, gain, samples: bySample, toTime: (x) => (x - left) / speed, snap };
  }

  const MEDIAN_ROWS = [['I', 'aVR', 'V1', 'V4'], ['II', 'aVL', 'V2', 'V5'], ['III', 'aVF', 'V3', 'V6']];
  // the marks named on the paper (the offsets of P and QRS are drawn, not named)
  const NAMED = new Set(['P', 'QRS', 'T end']);

  /**
   * Draw the median beats of `rec` (rec.median, see ecg.js) on paper: the twelve leads in 3 × 4
   * (else every lead, four to a row), each beat whole, at `speed` mm/s and `gain` mm/mV, about
   * its median, with a calibration pulse on each row and the P, QRS and T onsets and offsets
   * (rec.median.marks) as dotted lines across each beat.  Returns what draw does.
   */
  function drawMedian(rec, opts) {
    const m = rec.median;
    const bySample = !m.fs;
    const fs = m.fs || NOMINAL;
    const speed = opts.speed || 25;
    const gain = opts.gain || 10;
    const byName = new Map(m.leads.map((l) => [l.name, l]));
    const rows = MEDIAN_ROWS.every((r) => r.every((nm) => byName.has(nm)))
      ? MEDIAN_ROWS.map((r) => r.map((nm) => byName.get(nm)))
      : Array.from({ length: Math.ceil(m.leads.length / 4) }, (_, i) => m.leads.slice(4 * i, 4 * i + 4));
    const n = Math.max(...m.leads.map((l) => l.v.length));
    const seconds = n / fs;
    const beatW = seconds * speed;
    const gap = 5;
    const left = 14;
    const top = 6;
    const rowH = Math.max(3.2 * gain, 22);
    const cols = Math.max(...rows.map((r) => r.length));
    const width = Math.ceil((left + cols * (beatW + gap) + 1) / 5) * 5;
    const height = Math.ceil((top + rows.length * rowH + 13) / 5) * 5;
    let s = sheet(width, height);
    let paths = '';
    let marks = '';
    let labels = '';
    const placed = [];
    rows.forEach((leads, r) => {
      const y0 = top + r * rowH + rowH * 0.62;
      paths += pulse(left, y0, speed, gain);
      leads.forEach((lead, c) => {
        const x0 = left + c * (beatW + gap);
        const off = median(lead.v);
        paths += trace(lead.v, fs, 0, lead.v.length, x0, y0, speed, gain, off);
        placed.push({ fs, v: lead.v, off, name: lead.name, a: 0, b: lead.v.length, x0, y0, t0: 0 });
        labels += `<text x="${f2(x0 + 1.2)}" y="${f2(y0 - rowH * 0.42)}">${esc(lead.name)}</text>`;
        for (const k of m.marks) {
          const x = x0 + (k.sample / fs) * speed;
          if (x < x0 || x > x0 + beatW) continue;
          marks += `M${f2(x)} ${f2(y0 - rowH * 0.34)}V${f2(y0 + rowH * 0.32)}`;
          if (r === rows.length - 1 && NAMED.has(k.name)) {
            labels += `<text class="markname" x="${f2(x)}" y="${f2(y0 + rowH * 0.32 + 3.2)}" text-anchor="middle">${esc(k.name)}</text>`;
          }
        }
      });
    });
    s += `<path class="mark" d="${marks}"/><path class="trace" d="${paths}"/><g class="labels">${labels}</g>`;
    const xs = bySample ? `${(NOMINAL * 5) / speed} samples per 5 mm (rate unknown)` : `${speed} mm/s`;
    const what = m.marks.length ? '   P, QRS and T from the device\'s measurements' : '';
    s += `<text class="foot" x="${left}" y="${height - 2.2}">Median beats   ${esc(xs)}   ${gain} mm/mV${bySample ? '' : `   ${esc(fs)} Hz`}${esc(what)}</text>`;
    s += '<g class="calipers"></g></svg>';
    return { svg: s, width, height, seconds, left, speed, gain, samples: bySample, toTime: (x) => (x - left) / speed, snap: snapper(placed, speed, gain) };
  }

  const api = { draw, drawMedian, LAYOUTS, NOMINAL, filtfilt, highpass, notch, median, highpassFilter, notchFilter };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Paper = api;
})(typeof self !== 'undefined' ? self : this);
