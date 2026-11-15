// The paper's baseline: each strip drawn about its median, the optional zero-phase 0.5 Hz
// high-pass and 50 or 60 Hz notch.  Synthetic signals only.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const Paper = require('../paper.js');
const ECG = require('../ecg.js');

const fs = 500;
const sine = (n, f, a = 1, phase = 0) => Float64Array.from({ length: n }, (_, i) => a * Math.sin(2 * Math.PI * f * (i / fs) + phase));
const add = (...xs) => Float64Array.from(xs[0], (_, i) => xs.reduce((s, x) => s + x[i], 0));
const rms = (v, a = 0, b = v.length) => { let s = 0; for (let i = a; i < b; i++) s += v[i] ** 2; return Math.sqrt(s / (b - a)); };
const maxAbs = (v, a = 0, b = v.length) => { let m = 0; for (let i = a; i < b; i++) m = Math.max(m, Math.abs(v[i])); return m; };
/** A narrow pulse (a QRS-like Gaussian, σ 10 ms) at sample c. */
const pulseAt = (n, c, a = 1) => Float64Array.from({ length: n }, (_, i) => a * Math.exp(-0.5 * ((i - c) / (0.01 * fs)) ** 2));

test('the median: of the finite samples of a span', () => {
  assert.strictEqual(Paper.median(Float64Array.of(5, 1, 3)), 3);
  assert.strictEqual(Paper.median(Float64Array.of(4, 1, 3, 2)), 2.5);
  assert.strictEqual(Paper.median(Float64Array.of(9, 1, NaN, 2, 100), 1, 4), 1.5);
  assert.strictEqual(Paper.median(Float64Array.of(NaN, NaN)), 0);
});

test('each strip is drawn about its median: a lead 2 mV off its baseline sits on its row', () => {
  const n = fs * 10;
  const leads = ['I', 'II'].map((name, k) => ({ name, units: 'mV', fs, v: add(pulseAt(n, fs * 2, 1), Float64Array.from({ length: n }, () => 2 + k)) }));
  const rec = { name: 'x', fs, leads, meta: null };
  const d = Paper.draw(rec, { speed: 25, gain: 10, twelve: null });
  // lead I's row: its baseline (y0 = 6 + 0.62 × 32 mm) is where the trace starts, not 20 mm above it
  const y0 = 6 + 0.62 * 32;
  const start = d.snap(d.left + 0.5, y0);
  assert.ok(start && Math.abs(start.v) < 1e-9 && Math.abs(start.y - y0) < 1e-6, JSON.stringify(start));
  // the pulse is 1 mV above the median
  const peak = d.snap(d.left + 2 * 25, y0 - 10);
  assert.ok(Math.abs(peak.v - 1) < 1e-6);
  // without the median, as recorded
  const raw = Paper.draw(rec, { speed: 25, gain: 10, twelve: null, median: false });
  assert.ok(Math.abs(raw.snap(raw.left + 0.5, y0 - 20).v - 2) < 1e-9);
  assert.ok(!d.svg.includes('high-pass') && !d.svg.includes('notch'));
  // the record itself is never changed
  assert.strictEqual(rec.leads[0].v[0], 2);
});

test('the high-pass: DC and slow drift out, the ECG band in', () => {
  const n = fs * 12;
  const drift = add(Float64Array.from({ length: n }, () => 3), sine(n, 0.05, 1), sine(n, 0.1, 0.5));
  const y = Paper.highpass(drift, fs);
  const mid = [fs * 2, n - fs * 2];
  assert.ok(maxAbs(y, ...mid) < 0.15, `drift left ${maxAbs(y, ...mid)}`);
  assert.ok(Math.abs(Paper.highpass(Float64Array.from({ length: n }, () => 5), fs).reduce((m, x) => Math.max(m, Math.abs(x)), 0)) < 1e-9);
  // 5 Hz passes nearly whole (the gain² of a first-order section run twice: 1 / (1 + (0.5 / 5)²))
  const s5 = sine(n, 5);
  assert.ok(Math.abs(rms(Paper.highpass(s5, fs), ...mid) / rms(s5, ...mid) - 1 / (1 + 0.01)) < 0.005);
  // a linear ramp at the ends does not ring off the paper: the edges stay close to the middle
  const ramp = Float64Array.from({ length: n }, (_, i) => i / fs);
  assert.ok(maxAbs(Paper.highpass(ramp, fs)) < 0.5);
});

test('the notch: 50 and 60 Hz out, the rest in', () => {
  const n = fs * 4;
  const mid = [fs, n - fs];
  for (const f0 of [50, 60]) {
    const ecg = sine(n, 10, 1);
    const hum = sine(n, f0, 0.5, 0.3);
    const y = Paper.notch(add(ecg, hum), fs, f0);
    const resid = Float64Array.from(y, (x, i) => x - ecg[i]);
    assert.ok(rms(resid, ...mid) < 0.01 * rms(hum, ...mid), `${f0} Hz: ${rms(resid, ...mid)}`);
    assert.ok(rms(Paper.notch(hum, fs, f0), ...mid) < 0.01 * rms(hum, ...mid));
    // 10 Hz and the other mains frequency pass
    assert.ok(rms(Paper.notch(ecg, fs, f0), ...mid) / rms(ecg, ...mid) > 0.999);
  }
  // a notch at or above half the rate is no filter
  assert.deepStrictEqual([...Paper.notch(Float64Array.of(1, 2, 3), 100, 50)], [1, 2, 3]);
});

test('zero phase: a symmetric pulse keeps its peak in place and its symmetry', () => {
  const n = fs * 6;
  const c = fs * 3;
  const p = add(pulseAt(n, c, 1), Float64Array.from({ length: n }, () => 1.5));
  const hum = sine(n, 50, 0.2, Math.PI / 2);               // a cosine: symmetric about the pulse too
  for (const y of [Paper.highpass(p, fs), Paper.notch(pulseAt(n, c), fs, 50), Paper.notch(Paper.highpass(add(p, hum), fs), fs, 50)]) {
    let k = 0;
    for (let i = 0; i < n; i++) if (y[i] > y[k]) k = i;
    assert.strictEqual(k, c);
    for (const dt of [5, 10, 20, 50]) assert.ok(Math.abs(y[c - dt] - y[c + dt]) < 1e-3, `asymmetric at ±${dt}`);
  }
});

test('gaps stay gaps; filters on the paper, named at its foot', () => {
  const n = fs * 10;
  const v = add(pulseAt(n, fs * 1), sine(n, 60, 0.3), Float64Array.from({ length: n }, (_, i) => 1 + i / n));
  v[100] = NaN;
  const y = Paper.notch(Paper.highpass(v, fs), fs, 60);
  assert.ok(Number.isNaN(y[100]) && Number.isFinite(y[99]) && Number.isFinite(y[101]));
  const names = ECG.STANDARD;
  const rec = { name: 'x', fs, leads: names.map((name) => ({ name, units: 'mV', fs, v })), meta: null };
  const d = Paper.draw(rec, { layout: '3x4', speed: 25, gain: 10, twelve: ECG.twelveLead(rec), highpass: true, notch: 60 });
  assert.ok(d.svg.includes('0.5 Hz high-pass') && d.svg.includes('60 Hz notch'));
  // the rhythm strip's (row 4) R wave at 1 s, as drawn: 1 mV above the filtered baseline, no hum on it
  const y0 = 6 + 3 * 32 + 0.62 * 32;
  const r = d.snap(d.left + 25, y0 - 10);
  assert.ok(r && Math.abs(r.t - 1) < 0.004 && Math.abs(r.v - 1) < 0.1, JSON.stringify(r));
});
