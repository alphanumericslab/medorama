// A patient's page (web/js/patient/*.js): the journey's columns reached by the keyboard, its
// picture said in words, critical values told from abnormal by more than a colour, its months
// right whatever order dates are written in; the detail's warning above a file sent as it is.
import test from 'node:test';
import assert from 'node:assert/strict';
import { element, install } from './dom.mjs';

install();
const core = await import('../../web/js/core.js');
core.st.cfg = { timezone: 'UTC', display: { date: 'dmy', clock: '24h' } };
const { drawJourney, journeyLabel } = await import('../../web/js/patient/journey.js');
const { renderDetail } = await import('../../web/js/patient/detail.js');

const T = Date.UTC(2024, 2, 30, 6) / 1000; // 30 March 2024
const data = {
  bin: 6 * 3600,
  lanes: [{ key: 'lab', label: 'Labs', subs: ['Lactate', 'K'] }, { key: 'note', label: 'Notes', subs: ['Notes'] }],
  // a critical lactate, an abnormal potassium, a note; then a day in April
  cells: [[0, 0, T, 1, 2], [0, 1, T, 1, 1], [1, 0, T, 1, 0], [0, 0, T + 3 * 86400, 1, 0]],
  spans: [],
};

test("the journey's column names are buttons: focusable, pressed while shown, named", () => {
  const el = element('#pp-journey');
  el.clientWidth = 400;
  drawJourney(el, data, { hidden: new Set(['note']) });
  const heads = el.innerHTML.match(/<g class="lanehead"[^>]*>/g) || [];
  assert.equal(heads.length, 2, el.innerHTML.slice(0, 400));
  for (const h of heads) assert.match(h, /role="button" tabindex="0"/);
  assert.match(heads[0], /aria-pressed="true" data-lane="0" data-lane-key="lab"/);
  assert.match(heads[1], /aria-pressed="false"/, 'a hidden column: not pressed');
  assert.match(el.innerHTML, /<title>Notes: hidden, press to show<\/title>/);
});

test('the picture is said in words: its span and its columns', () => {
  const el = element('#pp-journey');
  drawJourney(el, data, {});
  const label = el.innerHTML.match(/role="img" aria-label="([^"]*)"/)?.[1] || '';
  assert.match(label, /2 columns shown \(Labs, Notes\)/);
  assert.match(label, /also listed in the report/);
  assert.equal(journeyLabel(data.lanes, new Set(['lab']), T, T), 'The journey, 30 Mar 2024 to 30 Mar 2024, the newest on top: 1 column shown (Notes), 1 hidden (Labs). Every entry is also listed in the report.');
});

test('a critical value is outlined (a second channel beside its amber), an abnormal one is not', () => {
  const el = element('#pp-journey');
  el.clientWidth = 600;
  drawJourney(el, data, {});
  const rects = el.innerHTML.match(/<rect [^>]*opacity="1"[^>]*\/>/g) || [];
  const outlined = rects.filter((r) => r.includes('stroke-width="1"'));
  assert.equal(outlined.length, 1, rects.join('\n'));
});

test('months on the axis are right with day-first dates (31 Mar, then April)', () => {
  const el = element('#pp-journey');
  drawJourney(el, data, {});
  const months = [...el.innerHTML.matchAll(/font-weight="600">([A-Z][a-z]{2}) '(\d\d)</g)].map((m) => `${m[1]} ${m[2]}`);
  assert.deepEqual(months, ['Mar 24', 'Apr 24']);
});

const detailEl = () => element('#pp-detail');
const ctx = { id: 1, summary: { encounters: [] }, terms: [], nav: {}, context: () => '', setTimeAt: () => {} };
const detail = (over) => ({ key: 'item:1', title: 'Echo', category: 'echo', time: T, sources: ['hosp'], rows: [], fields: [], notes: [], ...over });

test('in Privacy Mode, a picture shown on the page is marked: it may show identifiers as recorded', () => {
  core.st.user = { privacy: true };
  const el = detailEl();
  renderDetail(el, detail({ attachment: { kind: 'image', url: 'files/1', name: 'mo-f0001.png' } }), ctx);
  assert.match(el.innerHTML, /<p class="idnote" data-asis>.*▲.*May show identifiers as recorded/);
  assert.ok(el.innerHTML.indexOf('data-asis') < el.innerHTML.indexOf('<img'), 'above the picture');
  renderDetail(el, detail({ attachment: { kind: 'pdf', url: 'files/2', name: 'report.pdf' } }), ctx);
  assert.match(el.innerHTML, /data-asis/);
  core.st.user = { privacy: false, phi: true };
  renderDetail(el, detail({ attachment: { kind: 'image', url: 'files/1', name: 'x.png' } }), ctx);
  assert.doesNotMatch(el.innerHTML, /data-asis/, 'out of Privacy Mode: identifiers are shown anyway');
});

test("a panel tells critical from abnormal by words, not only by colour", () => {
  core.st.user = { privacy: true };
  const el = detailEl();
  const row = (name, level, num) => ({ name, value: String(num), num, unit: 'mmol/L', ref_low: 3.5, ref_high: 5, level, flag: '', series: [], previous: '' });
  renderDetail(el, detail({ key: 'labs:1', category: 'lab', title: 'Panel', rows: [row('K', 2, 7.1), row('Na', 1, 5.5), row('Cl', 0, 4)] }), ctx);
  assert.match(el.innerHTML, /<span class="badge">1 abnormal<\/span><span class="badge crit">1 critical<\/span>/);
  assert.match(el.innerHTML, /<td class="v crit">7.1<\/td>/);
  assert.match(el.innerHTML, /<span class="badge crit">H · critical<\/span>/);
  assert.match(el.innerHTML, /<td class="v bad">5.5<\/td>/);
});

test("an ECG's detail: each measurement once, with its change and trend; statements new and no longer said", () => {
  core.st.user = { privacy: false, phi: true };
  const el = detailEl();
  const ecg = {
    previous: T - 3 * 86400,
    statements: [['Atrial fibrillation', true], ['Prolonged QT', false]],
    gone: ['Sinus rhythm'],
    measures: [
      { name: 'QTc', value: 503, unit: 'ms', change: 25, series: [[T - 3 * 86400, 478], [T, 503]] },
      { name: 'QRS', value: 104, unit: 'ms', change: 0, series: [[T - 3 * 86400, 104], [T, 104]] },
      { name: 'R axis', value: -48, unit: '°', change: null, series: [[T, -48]] },
    ],
  };
  renderDetail(el, detail({ key: 'ecg:2:ab', category: 'ecg', title: '12-lead ECG', ecg }), ctx);
  const h = el.innerHTML;
  assert.equal(h.match(/QTc/g).length, 1, 'QTc once');
  assert.match(h, /<b>503<\/b> <i>ms<\/i><span class="chg">▲ 25<\/span>/);
  assert.match(h, /<span class="chg" title="Unchanged">=<\/span>/);
  assert.match(h, /<b>-48<\/b> <i>°<\/i>\s*<\/div>/, 'no previous value: no change');
  assert.equal((h.match(/<svg/g) || []).length, 2, 'a trend where there are two ECGs or more');
  assert.match(h, /Atrial fibrillation <span class="tag">new<\/span>/);
  assert.match(h, /Prolonged QT<\/li>/);
  assert.match(h, /no longer<\/span> Sinus rhythm/);
  assert.match(h, /change since the ECG of/);
});
