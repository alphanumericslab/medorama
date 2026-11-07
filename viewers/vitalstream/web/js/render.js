// Canvas drawing: overview strip, time axis, lanes (waveforms, trends, alarms).

import { fmtDate, fmtTime, tickLabel, ticks, zoneName } from './timefmt.js';

export const LABEL_W = 136;
export const PAD_R = 10;
const FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, Arial, sans-serif';

// The late data blocks a reader put back in order, shown only with the viewer's qc_verbose setting.
export function repairedSpans(s) {
  if (!s.ds || !s.config?.qc_verbose) return [];
  return (s.ds.late_blocks || []).filter((g) => g.moved && Number.isFinite(g.t0) && Number.isFinite(g.t1));
}

export function readTheme() {
  const cs = getComputedStyle(document.documentElement);
  const v = (n) => cs.getPropertyValue(n).trim();
  const theme = {};
  for (const n of ['bg', 'panel', 'panel-2', 'lane', 'lane-alt', 'line', 'grid', 'text', 'muted', 'accent',
    'gap', 'gap-edge', 'untimed', 'untimed-edge', 'repaired', 'repaired-edge', 'cursor', 'select', 'c-trend',
    'alarm-1', 'alarm-2', 'alarm-3']) {
    theme[n] = v(`--${n}`);
  }
  theme.modality = (m) => v(`--c-${m}`) || v('--c-other');
  return theme;
}

// Size a canvas for the device pixel ratio; returns a 2D context in CSS pixels.
export function fit(canvas, w, h) {
  const dpr = window.devicePixelRatio || 1;
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    canvas.style.height = `${h}px`;
  }
  const ctx = canvas.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

export const fmtVal = (v) => {
  if (v === null || v === undefined || Number.isNaN(v)) return '—';
  const a = Math.abs(v);
  if (a >= 1000) return v.toFixed(0);
  if (a >= 100) return v.toFixed(1);
  if (a >= 1) return v.toFixed(2);
  if (a === 0) return '0';
  return v.toPrecision(3);
};

function niceTicks(lo, hi, n = 3) {
  const span = hi - lo;
  if (!(span > 0)) return [lo];
  const raw = span / n;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => span / s <= n) || 10 * mag;
  const out = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(+v.toPrecision(12));
  return out;
}

// ---------------------------------------------------------------- overview
export function drawOverview(canvas, s, theme) {
  const W = canvas.clientWidth;
  const H = 46;
  const ctx = fit(canvas, W, H);
  ctx.clearRect(0, 0, W, H);
  const ds = s.ds;
  if (!ds || !s.range) return;
  const [t0, t1] = s.range;
  const x0 = LABEL_W;
  const pw = W - LABEL_W - PAD_R;
  const X = (t) => x0 + ((t - t0) / (t1 - t0)) * pw;
  const mods = s.modalityOrder;
  const rowH = Math.max(3, Math.min(7, Math.floor(28 / Math.max(1, mods.length + 1))));
  const top = 3;
  ctx.font = `10px ${FONT}`;
  ctx.textBaseline = 'middle';
  mods.forEach((m, i) => {
    const y = top + i * (rowH + 1);
    for (const p of s.piecesByModality[m] || []) {
      const a = X(p.t0);
      const b = X(p.t0 + p.n / p.fs);
      ctx.fillStyle = p.timed ? theme.modality(m) : theme['untimed-edge'];
      ctx.globalAlpha = 0.85;
      ctx.fillRect(a, y, Math.max(1, b - a), rowH);
    }
  });
  ctx.globalAlpha = 1;
  const ay = top + mods.length * (rowH + 1);
  for (const al of ds.alarms) {
    ctx.fillStyle = alarmColor(al, theme);
    ctx.fillRect(X(al.t0), ay, Math.max(1, X(al.t1) - X(al.t0)), 3);
  }
  for (const g of ds.gaps) {
    ctx.fillStyle = theme['gap-edge'];
    if (!g.channels) {
      ctx.fillRect(X(g.t0) - 0.5, top, Math.max(1, X(g.t1) - X(g.t0)), ay - top);
      continue;
    }
    for (const m of new Set(g.channels.map((c) => ds.channels[c].modality))) {
      const i = mods.indexOf(m);
      if (i >= 0) ctx.fillRect(X(g.t0) - 0.5, top + i * (rowH + 1), Math.max(1, X(g.t1) - X(g.t0)), rowH);
    }
  }
  // time labels
  ctx.fillStyle = theme.muted;
  const tk = ticks(t0, t1, pw, s.tz, 120);
  ctx.textBaseline = 'alphabetic';
  for (const t of tk.ticks) {
    const x = X(t);
    ctx.fillRect(x, H - 13, 1, 3);
    ctx.fillText(tickLabel(t, tk.step, s.tz), x + 3, H - 3);
  }
  ctx.textAlign = 'right';
  ctx.fillText(fmtDate(t0, s.tz), LABEL_W - 10, H - 3);
  ctx.textAlign = 'left';
  // viewport
  const va = X(s.view.t0);
  const vb = X(s.view.t1);
  ctx.fillStyle = theme.select;
  ctx.fillRect(va, 1, Math.max(2, vb - va), H - 15);
  ctx.strokeStyle = theme.accent;
  ctx.lineWidth = 1.5;
  ctx.strokeRect(va, 1, Math.max(2, vb - va), H - 15);
  ctx.fillStyle = theme.muted;
  ctx.font = `600 10px ${FONT}`;
  ctx.textBaseline = 'middle';
  ctx.fillText('OVERVIEW', 12, 16);
}

// -------------------------------------------------------------------- axis
export function drawAxis(canvas, s, theme) {
  const W = canvas.clientWidth;
  const H = 34;
  const ctx = fit(canvas, W, H);
  ctx.clearRect(0, 0, W, H);
  if (!s.ds) return;
  const { t0, t1 } = s.view;
  const pw = W - LABEL_W - PAD_R;
  const X = (t) => LABEL_W + ((t - t0) / (t1 - t0)) * pw;
  const tk = ticks(t0, t1, pw, s.tz);
  ctx.font = `11px ${FONT}`;
  ctx.fillStyle = theme.text;
  ctx.textBaseline = 'alphabetic';
  for (const t of tk.ticks) {
    const x = X(t);
    ctx.fillStyle = theme.line;
    ctx.fillRect(Math.round(x), H - 7, 1, 7);
    ctx.fillStyle = theme.text;
    ctx.fillText(tickLabel(t, tk.step, s.tz), x + 4, H - 10);
  }
  ctx.fillStyle = theme.muted;
  ctx.font = `10px ${FONT}`;
  ctx.fillText(`${fmtDate(t0, s.tz)} ${zoneName(t0, s.tz)}`, 12, 14);
  ctx.fillText(`ticks every ${tk.step < 1 ? `${Math.round(tk.step * 1000)} ms` : tk.step < 60 ? `${tk.step} s`
    : tk.step < 3600 ? `${tk.step / 60} min` : tk.step < 86400 ? `${tk.step / 3600} h` : `${tk.step / 86400} d`}`, 12, 27);
  if (s.cursor !== null) {
    const x = X(s.cursor);
    if (x >= LABEL_W && x <= W - PAD_R) {
      const label = fmtTime(s.cursor, s.tz, t1 - t0);
      ctx.font = `600 11px ${FONT}`;
      const w = ctx.measureText(label).width + 12;
      const bx = Math.min(Math.max(LABEL_W, x - w / 2), W - PAD_R - w);
      ctx.fillStyle = theme.accent;
      roundRect(ctx, bx, 4, w, 18, 4);
      ctx.fill();
      ctx.fillStyle = theme.panel;
      ctx.fillText(label, bx + 6, 17);
    }
  }
}

function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

// Alarm levels: 1 advisory, 2 warning, 3 crisis (readers and adapters map their sources' levels).
export function alarmColor(a, theme) {
  const l = a.level || 0;
  return l >= 3 ? theme['alarm-3'] : l === 2 ? theme['alarm-2'] : theme['alarm-1'];
}

// ------------------------------------------------------------------- lanes
// Visible y-range of a waveform lane from the loaded blocks.
function waveRange(blocks, t0, t1) {
  let lo = Infinity;
  let hi = -Infinity;
  for (const b of blocks) {
    const n = b.kind === 'raw' ? b.y.length : b.min.length;
    const i0 = Math.max(0, Math.floor((t0 - b.t0) / b.dt));
    const i1 = Math.min(n - 1, Math.ceil((t1 - b.t0) / b.dt));
    if (b.kind === 'raw') {
      for (let i = i0; i <= i1; i++) {
        const v = b.y[i];
        if (v !== null) { if (v < lo) lo = v; if (v > hi) hi = v; }
      }
    } else {
      for (let i = i0; i <= i1; i++) {
        const a = b.min[i];
        const c = b.max[i];
        if (a !== null && a < lo) lo = a;
        if (c !== null && c > hi) hi = c;
      }
    }
  }
  return [lo, hi];
}

function trendRange(tr, t0, t1) {
  let lo = Infinity;
  let hi = -Infinity;
  if (!tr) return [lo, hi];
  const vals = tr.kind === 'env' ? [tr.min, tr.max] : [tr.v, tr.v];
  for (let i = 0; i < tr.t.length; i++) {
    if (tr.t[i] < t0 - 600 || tr.t[i] > t1 + 600) continue;
    if (vals[0][i] < lo) lo = vals[0][i];
    if (vals[1][i] > hi) hi = vals[1][i];
  }
  return [lo, hi];
}

function padRange([lo, hi]) {
  if (!Number.isFinite(lo)) return [0, 1];
  if (hi - lo < 1e-9) {
    const d = Math.abs(lo) * 0.05 || 0.5;
    return [lo - d, hi + d];
  }
  const d = (hi - lo) * 0.08;
  return [lo - d, hi + d];
}

export function drawLanes(canvas, s, theme) {
  const W = canvas.clientWidth;
  const H = s.lanes.reduce((a, l) => a + l.h, 0);
  const ctx = fit(canvas, W, Math.max(H, 1));
  ctx.clearRect(0, 0, W, H);
  const { t0, t1 } = s.view;
  const pw = W - LABEL_W - PAD_R;
  const X = (t) => LABEL_W + ((t - t0) / (t1 - t0)) * pw;
  const span = t1 - t0;
  const tk = ticks(t0, t1, pw, s.tz);
  let y = 0;
  s.lanes.forEach((lane, li) => {
    lane.y = y;
    ctx.fillStyle = li % 2 ? theme['lane-alt'] : theme.lane;
    ctx.fillRect(0, y, W, lane.h);
    // vertical grid
    ctx.fillStyle = theme.grid;
    for (const t of tk.ticks) ctx.fillRect(Math.round(X(t)), y, 1, lane.h);
    ctx.fillStyle = theme.line;
    ctx.fillRect(0, y + lane.h - 1, W, 1);
    ctx.save();
    ctx.beginPath();
    ctx.rect(LABEL_W, y, pw, lane.h);
    ctx.clip();
    if (lane.type === 'wave') drawWave(ctx, lane, s, theme, X, y, span);
    else if (lane.type === 'trend') drawTrend(ctx, lane, s, theme, X, y);
    else if (lane.type === 'alarm') drawAlarms(ctx, lane, s, theme, X, y);
    ctx.restore();
    drawLabel(ctx, lane, s, theme, y);
    y += lane.h;
  });
  // breaks and untimed data, across all lanes
  ctx.save();
  ctx.beginPath();
  ctx.rect(LABEL_W, 0, pw, H);
  ctx.clip();
  for (const u of s.ds ? s.ds.untimed : []) {
    const a = X(u.t0);
    const b = X(u.t1);
    if (b < LABEL_W || a > W) continue;
    ctx.fillStyle = theme.untimed;
    ctx.fillRect(a, 0, Math.max(2, b - a), H);
    ctx.fillStyle = theme['untimed-edge'];
    ctx.fillRect(a, 0, 1, H);
    ctx.fillRect(Math.max(a, b - 1), 0, 1, H);
    if (b - a > 110) {
      ctx.font = `600 10px ${FONT}`;
      ctx.fillText('TIME NOT STATED', a + 6, 12);
    }
  }
  for (const g of s.ds ? s.ds.gaps : []) {
    const a = X(g.t0);
    const b = X(g.t1);
    if (b < LABEL_W - 2 || a > W) continue;
    // a break of the whole stream shades every lane; a channel's break only its lanes
    const spans = g.channels
      ? s.lanes.filter((l) => l.type === 'wave' && g.channels.includes(l.ch)).map((l) => [l.y, l.h])
      : [[0, H]];
    for (const [y0, h] of spans) {
      ctx.fillStyle = theme.gap;
      ctx.fillRect(a, y0, Math.max(2, b - a), h);
      ctx.fillStyle = theme['gap-edge'];
      ctx.fillRect(a - 0.5, y0, 1, h);
      if (b - a > 2) ctx.fillRect(b - 0.5, y0, 1, h);
    }
  }
  // verbose QC: data blocks the reader put back in order (a dashed outline; not a break)
  for (const g of repairedSpans(s)) {
    const a = X(g.t0);
    const b = X(g.t1);
    if (b < LABEL_W - 2 || a > W) continue;
    ctx.fillStyle = theme.repaired;
    ctx.fillRect(a, 0, Math.max(2, b - a), H);
    ctx.strokeStyle = theme['repaired-edge'];
    ctx.lineWidth = 1;
    ctx.setLineDash([3, 3]);
    ctx.beginPath();
    ctx.moveTo(a + 0.5, 0);
    ctx.lineTo(a + 0.5, H);
    if (b - a > 2) {
      ctx.moveTo(b - 0.5, 0);
      ctx.lineTo(b - 0.5, H);
    }
    ctx.stroke();
    ctx.setLineDash([]);
  }
  // selection and cursor
  if (s.selection) {
    const a = X(Math.min(s.selection[0], s.selection[1]));
    const b = X(Math.max(s.selection[0], s.selection[1]));
    ctx.fillStyle = theme.select;
    ctx.fillRect(a, 0, b - a, H);
  }
  if (s.cursor !== null) {
    ctx.fillStyle = theme.cursor;
    ctx.fillRect(Math.round(X(s.cursor)), 0, 1, H);
  }
  ctx.restore();
  // cursor readouts
  if (s.cursor !== null) {
    ctx.font = `600 11px ${FONT}`;
    ctx.textBaseline = 'top';
    for (const lane of s.lanes) {
      if (lane.type !== 'wave' && lane.type !== 'trend') continue;
      const text = lane.readout ? lane.readout(s.cursor) : null;
      if (!text) continue;
      const w = ctx.measureText(text).width + 10;
      const x = W - PAD_R - w - 2;
      ctx.fillStyle = theme.panel;
      ctx.globalAlpha = 0.85;
      ctx.fillRect(x, lane.y + 4, w, 16);
      ctx.globalAlpha = 1;
      ctx.fillStyle = lane.color;
      ctx.fillText(text, x + 5, lane.y + 6);
    }
  }
}

function drawLabel(ctx, lane, s, theme, y) {
  ctx.fillStyle = theme.panel;
  ctx.fillRect(0, y, LABEL_W - 1, lane.h - 1);
  ctx.fillStyle = theme.line;
  ctx.fillRect(LABEL_W - 1, y, 1, lane.h);
  ctx.fillStyle = lane.color;
  ctx.fillRect(0, y, 3, lane.h - 1);
  ctx.textBaseline = 'top';
  ctx.font = `600 12px ${FONT}`;
  ctx.fillStyle = theme.text;
  const title = lane.title.length > 17 ? `${lane.title.slice(0, 16)}…` : lane.title;
  ctx.fillText(title, 12, y + 6);
  ctx.font = `11px ${FONT}`;
  ctx.fillStyle = theme.muted;
  if (lane.sub && lane.h >= 40) ctx.fillText(lane.sub, 12, y + 22);
  if (lane.range && lane.h >= 54) {
    ctx.textAlign = 'right';
    ctx.fillText(fmtVal(lane.range[1]), LABEL_W - 8, y + 6);
    ctx.textBaseline = 'bottom';
    ctx.fillText(fmtVal(lane.range[0]), LABEL_W - 8, y + lane.h - 5);
    ctx.textAlign = 'left';
  }
  ctx.textBaseline = 'alphabetic';
}

function yGrid(ctx, lane, theme, Y, y) {
  ctx.fillStyle = theme.grid;
  for (const v of niceTicks(lane.range[0], lane.range[1], Math.max(2, Math.floor(lane.h / 28)))) {
    const yy = Math.round(Y(v));
    if (yy > y + 2 && yy < y + lane.h - 2) ctx.fillRect(LABEL_W, yy, 10000, 1);
  }
}

function drawWave(ctx, lane, s, theme, X, y, span) {
  const blocks = (s.win && s.win.channels[lane.ch]) || [];
  lane.range = padRange(waveRange(blocks, s.view.t0, s.view.t1));
  const [lo, hi] = lane.range;
  const top = y + 5;
  const h = lane.h - 10;
  const Y = (v) => top + h - ((v - lo) / (hi - lo)) * h;
  yGrid(ctx, lane, theme, Y, y);
  ctx.strokeStyle = lane.color;
  ctx.fillStyle = lane.color;
  ctx.lineWidth = 1.25;
  ctx.lineJoin = 'round';
  for (const b of blocks) {
    const n = b.kind === 'raw' ? b.y.length : b.min.length;
    const i0 = Math.max(0, Math.floor((s.view.t0 - b.t0) / b.dt) - 1);
    const i1 = Math.min(n - 1, Math.ceil((s.view.t1 - b.t0) / b.dt) + 1);
    if (b.kind === 'raw') {
      ctx.beginPath();
      let pen = false;
      for (let i = i0; i <= i1; i++) {
        const v = b.y[i];
        if (v === null) { pen = false; continue; }
        const x = X(b.t0 + i * b.dt);
        if (pen) ctx.lineTo(x, Y(v)); else ctx.moveTo(x, Y(v));
        pen = true;
      }
      ctx.stroke();
      const pxPerSample = (b.dt / span) * (ctx.canvas.clientWidth - LABEL_W - PAD_R);
      if (pxPerSample >= 6) {
        const r = pxPerSample >= 14 ? 2.6 : 1.9;
        for (let i = i0; i <= i1; i++) {
          const v = b.y[i];
          if (v === null) continue;
          ctx.beginPath();
          ctx.arc(X(b.t0 + i * b.dt), Y(v), r, 0, 2 * Math.PI);
          ctx.fill();
        }
      }
    } else {
      // min/max envelope as a filled band (step per bucket)
      let i = i0;
      while (i <= i1) {
        while (i <= i1 && b.min[i] === null) i++;
        if (i > i1) break;
        let j = i;
        while (j + 1 <= i1 && b.min[j + 1] !== null) j++;
        ctx.beginPath();
        for (let k = i; k <= j; k++) {
          const xa = X(b.t0 + k * b.dt);
          const xb = X(b.t0 + (k + 1) * b.dt);
          const yy = Y(b.max[k]);
          if (k === i) ctx.moveTo(xa, yy); else ctx.lineTo(xa, yy);
          ctx.lineTo(xb, yy);
        }
        for (let k = j; k >= i; k--) {
          const xa = X(b.t0 + k * b.dt);
          const xb = X(b.t0 + (k + 1) * b.dt);
          const yy = Y(b.min[k]);
          ctx.lineTo(xb, yy);
          ctx.lineTo(xa, yy);
        }
        ctx.closePath();
        ctx.globalAlpha = 0.9;
        ctx.fill();
        ctx.globalAlpha = 1;
        ctx.lineWidth = 0.8;
        ctx.stroke();
        ctx.lineWidth = 1.25;
        i = j + 1;
      }
    }
  }
  // markers (ticks at the top of the lane)
  if (s.markers && s.markers.byCh[lane.ch]) {
    ctx.fillStyle = theme.muted;
    ctx.font = `9px -apple-system, Arial, sans-serif`;
    const pts = s.markers.byCh[lane.ch];
    const sparse = pts.length < 80;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    for (const [t, code] of pts) {
      const x = X(t);
      ctx.beginPath();
      ctx.moveTo(x - 3, y + 1);
      ctx.lineTo(x + 3, y + 1);
      ctx.lineTo(x, y + 5);
      ctx.fill();
      if (sparse && s.markers.short[code]) ctx.fillText(s.markers.short[code], x, y + 6);
    }
    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';
  }
  if (!blocks.length) {
    ctx.fillStyle = theme.muted;
    ctx.font = `11px ${FONT}`;
    ctx.fillText(s.loading || !s.win ? 'loading…' : 'no data in view', LABEL_W + 10, y + lane.h / 2);
  }
}

function drawTrend(ctx, lane, s, theme, X, y) {
  const tr = s.trends ? s.trends[lane.name] : null;
  lane.range = padRange(trendRange(tr, s.view.t0, s.view.t1));
  if (!tr) return;
  const [lo, hi] = lane.range;
  const top = y + 6;
  const h = lane.h - 12;
  const Y = (v) => top + h - ((v - lo) / (hi - lo)) * h;
  yGrid(ctx, lane, theme, Y, y);
  ctx.strokeStyle = lane.color;
  ctx.fillStyle = lane.color;
  ctx.lineWidth = 1.5;
  const maxGap = 900;
  if (tr.kind === 'env') {
    for (let i = 0; i < tr.t.length; i++) {
      const x = X(tr.t[i]);
      const a = Y(tr.max[i]);
      const b = Y(tr.min[i]);
      ctx.fillRect(x - 0.5, a, 1.5, Math.max(1.5, b - a));
    }
    return;
  }
  ctx.beginPath();
  for (let i = 0; i < tr.t.length; i++) {
    const x = X(tr.t[i]);
    const v = Y(tr.v[i]);
    if (i > 0 && tr.t[i] - tr.t[i - 1] <= maxGap) ctx.lineTo(x, v); else ctx.moveTo(x, v);
  }
  ctx.stroke();
  const pw = ctx.canvas.clientWidth - LABEL_W - PAD_R;
  if (tr.t.length < pw / 5) {
    for (let i = 0; i < tr.t.length; i++) {
      ctx.beginPath();
      ctx.arc(X(tr.t[i]), Y(tr.v[i]), 2.2, 0, 2 * Math.PI);
      ctx.fill();
    }
  }
}

function drawAlarms(ctx, lane, s, theme, X, y) {
  const rows = { 'Monitor alarm state': 0 };
  const rowH = (lane.h - 8) / 2;
  ctx.font = `600 10px ${FONT}`;
  ctx.textBaseline = 'middle';
  lane.hits = [];
  const textEnd = [-Infinity, -Infinity];     // per row: where the last label ends
  for (const a of s.ds.alarms) {
    const xa = X(a.t0);
    const xb = Math.max(xa + 3, X(a.t1));
    if (xb < LABEL_W || xa > ctx.canvas.clientWidth) continue;
    const row = rows[a.kind] ?? 1;
    const yy = y + 4 + row * rowH;
    ctx.fillStyle = alarmColor(a, theme);
    ctx.globalAlpha = 0.85;
    ctx.fillRect(xa, yy + 1, xb - xa, rowH - 2);
    ctx.globalAlpha = 1;
    lane.hits.push([xa, xb, yy, yy + rowH, a]);
    const w = ctx.measureText(a.text).width;
    const tx = Math.max(xa, LABEL_W) + 4;
    if (xb - tx > w + 4 && tx > textEnd[row] + 6) {
      ctx.fillStyle = theme.panel;
      ctx.fillText(a.text, tx, yy + rowH / 2);
      textEnd[row] = tx + w;
    }
  }
  ctx.textBaseline = 'alphabetic';
}
