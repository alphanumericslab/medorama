// An ECG on standard paper, as SVG in millimetres: 1 mm and 5 mm grid, 25 mm/s, 10 mm/mV,
// a 1 mV calibration pulse at the start of each row.  Layouts: the standard 3 × 4 and 6 × 2, each
// with a lead II rhythm strip, and 12 × 1; records that are not 12-lead ECGs show every lead in full.
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

  /** The path of samples v[a..b) at `speed` mm/s and `gain` mm/mV, from (x0, y0) mm. */
  function trace(v, fs, a, b, x0, y0, speed, gain) {
    let d = '';
    let pen = false;
    for (let n = Math.max(0, a); n < Math.min(v.length, b); n++) {
      const x = x0 + ((n - a) / fs) * speed;
      const y = v[n];
      if (!Number.isFinite(y)) { pen = false; continue; }
      d += `${pen ? 'L' : 'M'}${f2(x)} ${f2(y0 - y * gain)}`;
      pen = true;
    }
    return d;
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
   * ECG.twelveLead, or null).  Returns { svg, width, height (mm), seconds, toTime(x mm),
   * snap(x, y) }: `snap` finds the point of a trace nearest a point of the paper (for calipers).
   */
  function draw(rec, opts) {
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
    if (L) {
      const colS = seconds / L.cols;
      L.rows.forEach((names) => rows.push(names.map((nm, c) => ({ lead: twelve[nm], name: nm, t0: c * colS, t1: (c + 1) * colS }))));
      if (L.rhythm) rows.push([{ lead: twelve[L.rhythm], name: L.rhythm, t0: 0, t1: seconds, rhythm: true }]);
    } else {
      rec.leads.forEach((lead) => rows.push([{ lead, name: lead.name, t0: 0, t1: seconds }]));
    }
    const width = Math.ceil((left + stripW + 6) / 5) * 5;
    const height = Math.ceil((top + rows.length * rowH + 8) / 5) * 5;
    let s = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" class="paper">`;
    s += '<defs><pattern id="g5" width="5" height="5" patternUnits="userSpaceOnUse">'
      + '<path class="minor" d="M1 0V5M2 0V5M3 0V5M4 0V5M0 1H5M0 2H5M0 3H5M0 4H5"/>'
      + '<path class="major" d="M0 0V5M0 0H5"/></pattern></defs>';
    s += `<rect class="sheet" width="${width}" height="${height}"/><rect width="${width}" height="${height}" fill="url(#g5)"/>`;
    s += `<path class="major" d="M${width} 0V${height}M0 ${height}H${width}"/>`;
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
        paths += trace(lead.v, lead.fs, a, b, x0, y0, speed, gain);
        placed.push({ lead, name: cell.name, a, b: Math.min(b, lead.v.length), x0, y0 });
        // the lead's name, and a mark where the lead changes
        labels += `<text x="${f2(x0 + 1.2)}" y="${f2(y0 - rowH * 0.42)}">${esc(cell.name)}</text>`;
        if (c > 0) paths += `M${f2(x0)} ${f2(y0 - 2.5)}V${f2(y0 + 2.5)}`;
      });
    });
    s += `<path class="trace" d="${paths}"/>`;
    s += `<g class="labels">${labels}</g>`;
    s += `<text class="foot" x="${left}" y="${height - 2.2}">${speed} mm/s   ${gain} mm/mV   ${esc(rec.fs)} Hz</text>`;
    s += '<g class="calipers"></g></svg>';
    /** The sample nearest (x, y) mm, within ±40 ms along the paper: { x, y, v (mV), t (s), lead }. */
    const snap = (x, y) => {
      let best = null;
      for (const c of placed) {
        const fs = c.lead.fs;
        const xEnd = c.x0 + ((c.b - c.a) / fs) * speed;
        if (x < c.x0 - 0.5 || x > xEnd + 0.5) continue;
        const n0 = c.a + Math.round(((x - c.x0) / speed) * fs);
        const w = Math.max(1, Math.round(0.04 * fs));
        for (let n = Math.max(c.a, n0 - w); n < Math.min(c.b, n0 + w + 1); n++) {
          const v = c.lead.v[n];
          if (!Number.isFinite(v)) continue;
          const px = c.x0 + ((n - c.a) / fs) * speed;
          const py = c.y0 - v * gain;
          const d2 = (px - x) ** 2 + (py - y) ** 2;
          if (!best || d2 < best.d2) best = { d2, x: px, y: py, v, t: (px - left) / speed, lead: c.name };
        }
      }
      return best;
    };
    return { svg: s, width, height, seconds, left, speed, gain, toTime: (x) => (x - left) / speed, snap };
  }

  const api = { draw, LAYOUTS };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Paper = api;
})(typeof self !== 'undefined' ? self : this);
