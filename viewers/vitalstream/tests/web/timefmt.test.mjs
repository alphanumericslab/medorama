// Times on the page's axis and in Go to (web/js/timefmt.js) around daylight-saving changes, and
// times beyond what a JavaScript Date holds.  `node --test`.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fields, fmtFull, tickLabel, ticks, toUnix } from '../../web/js/timefmt.js';

const tz = 'America/New_York';
const iso = (t) => new Date(t * 1000).toISOString();

test('a wall-clock time that does not exist, or occurs twice, as Python reads it (fold=0)', () => {
  // the clocks go forward at 02:00 on 2024-03-10: 02:30 does not exist, read as 03:30 EDT
  assert.equal(iso(toUnix(2024, 3, 10, 2, 30, 0, tz)), '2024-03-10T07:30:00.000Z');
  assert.equal(iso(toUnix(2024, 3, 10, 3, 30, 0, tz)), '2024-03-10T07:30:00.000Z');
  assert.equal(iso(toUnix(2024, 3, 10, 1, 30, 0, tz)), '2024-03-10T06:30:00.000Z');
  // they go back at 02:00 on 2024-11-03: 01:30 occurs twice, its first occurrence (EDT)
  assert.equal(iso(toUnix(2024, 11, 3, 1, 30, 0, tz)), '2024-11-03T05:30:00.000Z');
  assert.equal(iso(toUnix(2024, 11, 3, 2, 30, 0, tz)), '2024-11-03T07:30:00.000Z');
  assert.equal(iso(toUnix(2024, 7, 1, 12, 0, 0, tz)), '2024-07-01T16:00:00.000Z');
  assert.equal(iso(toUnix(2024, 7, 1, 12, 0, 0, 'UTC')), '2024-07-01T12:00:00.000Z');
});

test('daily ticks stay on midnight across a change', () => {
  const tk = ticks(toUnix(2024, 11, 1, 12, 0, 0, tz), toUnix(2024, 11, 7, 12, 0, 0, tz), 800, tz);
  assert.equal(tk.step, 86400);
  const labels = tk.ticks.map((t) => tickLabel(t, tk.step, tz));
  assert.deepEqual(labels, ['Sat 11-02', 'Sun 11-03', 'Mon 11-04', 'Tue 11-05', 'Wed 11-06', 'Thu 11-07']);
  for (const t of tk.ticks) assert.match(fmtFull(t, tz), / 00:00:00\.000 /);
});

test('hourly ticks stay on the hour, each hour once', () => {
  const back = ticks(toUnix(2024, 11, 2, 22, 0, 0, tz), toUnix(2024, 11, 3, 6, 0, 0, tz), 1000, tz);
  assert.equal(back.step, 3600);
  for (const t of back.ticks) assert.match(fmtFull(t, tz), /:00:00\.000 /);
  const fwd = ticks(toUnix(2024, 3, 9, 22, 0, 0, tz), toUnix(2024, 3, 10, 6, 0, 0, tz), 1000, tz);
  const hours = fwd.ticks.map((t) => tickLabel(t, fwd.step, tz));
  assert.deepEqual(hours, ['22:00', '23:00', '00:00', '01:00', '03:00', '04:00', '05:00', '06:00']);
  assert.equal(new Set(fwd.ticks).size, fwd.ticks.length);
});

test('short steps and other zones are unchanged', () => {
  const tk = ticks(1709287200, 1709287260, 800, 'UTC');
  assert.equal(tk.step, 10);
  assert.equal(tk.ticks[0], 1709287200);
  const kol = ticks(1709287200, 1709287200 + 6 * 3600, 600, 'Asia/Kolkata');   // UTC+05:30
  for (const t of kol.ticks) assert.match(fmtFull(t, 'Asia/Kolkata'), /:00:00\.000 /);
});

test('times beyond the range of a Date do not throw', () => {
  assert.doesNotThrow(() => fields(1e20, tz));
  assert.doesNotThrow(() => fmtFull(1e20, tz));
  assert.doesNotThrow(() => fmtFull(-1e20, tz));
  assert.doesNotThrow(() => fmtFull(NaN, tz));
  const tk = ticks(1e20, 1e20 + 1e7, 800, tz);
  assert.ok(tk.ticks.length <= 400);
});
