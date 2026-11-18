// A patient's report (web/js/patient/report.js): entries escaped, reachable by the keyboard,
// the chosen one said to be chosen.
import test from 'node:test';
import assert from 'node:assert/strict';
import { st as core } from '../../web/js/core.js';
import { renderList } from '../../web/js/patient/report.js';

core.cfg = { timezone: 'UTC' };

const state = (over = {}) => ({
  entries: [
    { key: 'labs:3:1704175800', time: 1704175800, category: 'lab', title: 'Lactate <img src=x onerror=alert(1)>', names: ['Lactate'], encounter: 3 },
    { key: 'item:9', time: 1704262200, category: 'note', title: 'Progress note', snippet: 'improving', encounter: 3 },
  ],
  summary: { encounters: [{ id: 3, encounter_id: 'MO-E-TEST', admit: 1704100000, discharge: 1704500000, admit_unit: 'ICU' }] }, terms: [], counts: [], fuzzy: false, q: '', mode: 'timeline', hidden: new Set(), enc: null,
  selected: 'item:9', labels: { lab: 'Laboratory', note: 'Notes' }, trends: [],
  ...over,
});

test("an entry's text is escaped", () => {
  const el = { innerHTML: '' };
  renderList(el, state());
  assert.ok(!el.innerHTML.includes('<img'), el.innerHTML);
  assert.ok(el.innerHTML.includes('&lt;img src=x onerror=alert(1)&gt;'));
});

test('entries are buttons a keyboard reaches, the chosen one pressed', () => {
  const el = { innerHTML: '' };
  renderList(el, state());
  const rows = el.innerHTML.match(/<div class="it[^"]*"[^>]*>/g) || [];
  assert.equal(rows.length, 2, el.innerHTML);
  for (const r of rows) assert.match(r, /tabindex="0" role="button"/);
  assert.match(rows.find((r) => r.includes('item:9')), /aria-pressed="true"/);
  assert.match(rows.find((r) => r.includes('labs:3')), /aria-pressed="false"/);
});

test('a hidden category is left out', () => {
  const el = { innerHTML: '' };
  renderList(el, state({ hidden: new Set(['note']) }));
  assert.ok(!el.innerHTML.includes('item:9'));
  assert.ok(el.innerHTML.includes('labs:3'));
});

test('a measured value keeps its digits (12345, not 12350)', async () => {
  const { fmtValue } = await import('../../web/js/patient/report.js');
  assert.equal(fmtValue(12345), '12345');
  assert.equal(fmtValue(12345.678), '12345.678');
  assert.equal(fmtValue(0.1 + 0.2), '0.3');
  assert.equal(fmtValue(3.14159265), '3.14159');
  assert.equal(fmtValue(0.000123456), '0.000123456');
});

test("a stay's hospital day counts calendar days in the catalog's time zone", async () => {
  const { context } = await import('../../web/js/patient/report.js');
  core.cfg = { timezone: 'America/New_York' };
  const { resetFormats } = await import('../../web/js/core.js');
  resetFormats();
  // admitted 22:00 on Jan 2 in New York (03:00 UTC on Jan 3), the next morning is hospital day 2
  const admit = Date.UTC(2024, 0, 3, 3) / 1000;
  const sum = { encounters: [{ id: 1, admit, discharge: admit + 3 * 86400, source: 'hosp' }] };
  assert.match(context(sum, 1, admit + 10 * 3600), /^Stay · hospital day 2 · hosp$/);
  core.cfg = { timezone: 'UTC' };
  resetFormats();
});

test('the report names what was done: no values, no flags (they are the detail\'s)', () => {
  const el = { innerHTML: '' };
  renderList(el, state({ entries: [{ key: 'ecg:2:ab', time: 1704175800, category: 'ecg', title: '12-lead ECG', names: ['12-lead ECG', 'QTc'], summary: 'QTc 503 ms', abnormal: 1, encounter: null }] }));
  assert.match(el.innerHTML, /<b>12-lead ECG<\/b>/);
  assert.doesNotMatch(el.innerHTML, /503|abnormal/);
});
