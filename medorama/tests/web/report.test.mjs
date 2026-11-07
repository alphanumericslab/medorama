// A patient's report (web/js/patient/report.js): entries escaped, reachable by the keyboard,
// the chosen one said to be chosen.
import test from 'node:test';
import assert from 'node:assert/strict';
import { st as core } from '../../web/js/core.js';
import { renderList } from '../../web/js/patient/report.js';

core.cfg = { timezone: 'UTC' };

const state = (over = {}) => ({
  entries: [
    { key: 'labs:3:1704175800', time: 1704175800, category: 'lab', title: 'Lactate <img src=x onerror=alert(1)>', summary: '4.6 mmol/L', encounter: 3 },
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
