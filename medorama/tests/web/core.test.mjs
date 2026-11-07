// The page's helpers (web/js/core.js), without a browser: escaping, highlighting, addresses'
// queries, dates in the catalog's time zone, Privacy Mode's typed dates.  `node --test`.
import test from 'node:test';
import assert from 'node:assert/strict';
import * as core from '../../web/js/core.js';

const { esc, mark, qs, fmtTime, fmtDate, dayKey, fmtDuration, zonedMidnight, removalText, viewerHash, st } = core;

test('text is escaped before it becomes HTML', () => {
  assert.equal(esc('<img src=x onerror="alert(1)">'), '&lt;img src=x onerror=&quot;alert(1)&quot;&gt;');
  assert.equal(esc("O'Brien & Co"), 'O&#39;Brien &amp; Co');
  assert.equal(esc(null), '');
  assert.equal(esc(0), '0');
});

test('words are marked in escaped text, never as HTML', () => {
  assert.equal(mark('Lactate <b>high</b>', ['lactate']), '<mark>Lactate</mark> &lt;b&gt;high&lt;/b&gt;');
  assert.equal(mark('a.b a+b', ['a.b']), '<mark>a.b</mark> a+b', 'a word is not a regular expression');
  assert.equal(mark('x', ['x']), 'x', 'one letter: not marked');
  assert.equal(mark('<script>', ['script']), '&lt;<mark>script</mark>&gt;');
});

test('queries leave empty values out', () => {
  assert.equal(qs({ q: 'a b', empty: '', none: null, gone: undefined, n: 0, f: false }), 'q=a+b&n=0&f=false');
});

test('times show in the catalog time zone', () => {
  st.cfg = { timezone: 'America/New_York' };
  core.resetFormats?.();
  const t = Date.UTC(2024, 0, 2, 13, 5) / 1000; // 08:05 in New York
  assert.equal(fmtTime(t), '2024-01-02 08:05');
  assert.equal(fmtTime(t, true), '2024-01-02 08:05:00');
  assert.equal(fmtDate(t), 'Jan 2, 2024');
  assert.equal(dayKey(Date.UTC(2024, 0, 2, 3) / 1000), '2024-01-01', 'still the 1st in New York');
  assert.equal(fmtTime(null), '—');
  assert.equal(zonedMidnight('2024-07-01'), Date.UTC(2024, 6, 1, 4) / 1000, 'midnight in summer time: 04:00 UTC');
  st.cfg = { timezone: 'UTC' };
  core.resetFormats?.();
});

test('durations read naturally', () => {
  assert.equal(fmtDuration(600), '10 min');
  assert.equal(fmtDuration(5400), '1.5 h');
  assert.equal(fmtDuration(3 * 86400 + 7200), '3 d 2 h');
  assert.equal(fmtDuration(undefined), '—');
});

test('the page holds no date offset: dates typed in Privacy Mode go to the server as typed', () => {
  assert.equal(core.realDate, undefined);
  assert.equal('offset' in core.privacy, false);
});

test('what a removal takes, in words', () => {
  assert.equal(removalText({ patients: 1, ehr_items: 3 }), '1 patient, 3 items');
  assert.equal(removalText({}), 'nothing');
  assert.match(removalText({ recordings: 2, kept_recordings: 1 }), /1 recording\(s\) and 0 encounter\(s\) stay/);
});

test("the viewer's address of a recording and a time span", () => {
  assert.equal(viewerHash('mo-rec:5', 1.5, 9), 'path=mo-rec%3A5&t0=1.500&t1=9.000');
  assert.equal(viewerHash('mo-rec:5'), 'path=mo-rec%3A5');
});

test('dates and times as the catalog chose (Admin › Dates and times)', () => {
  const t = Date.UTC(2024, 0, 31, 14, 5, 9) / 1000;
  const as = (date, clock) => {
    st.cfg = { timezone: 'UTC', display: { date, clock } };
    core.resetFormats();
    return [fmtTime(t), fmtDate(t), core.fmtHM(t), core.fmtShort(t), core.fmtDay(t), core.fmtNumDate(t)];
  };
  assert.deepEqual(as('ymd', '24h'), ['2024-01-31 14:05', 'Jan 31, 2024', '14:05', 'Jan 31', 'Wednesday, Jan 31, 2024', '2024-01-31']);
  assert.deepEqual(as('mdy', '12h'), ['01/31/2024 2:05 PM', 'Jan 31, 2024', '2:05 PM', 'Jan 31', 'Wednesday, Jan 31, 2024', '01/31/2024']);
  assert.deepEqual(as('dmy', '24h').slice(0, 2), ['31/01/2024 14:05', '31 Jan 2024']);
  assert.deepEqual(as('dmy_dot', '12h').slice(0, 1), ['31.01.2024 2:05 PM']);
  st.cfg = { timezone: 'UTC', display: { date: 'ymd', clock: '12h' } };
  core.resetFormats();
  assert.equal(fmtTime(Date.UTC(2024, 0, 31, 0, 30) / 1000), '2024-01-31 12:30 AM', 'midnight is 12 AM');
  assert.equal(fmtTime(t, true), '2024-01-31 2:05:09 PM');
  // grouping and midnight never depend on the choice
  st.cfg = { timezone: 'America/New_York', display: { date: 'mdy', clock: '12h' } };
  core.resetFormats();
  assert.equal(dayKey(Date.UTC(2024, 0, 2, 3) / 1000), '2024-01-01');
  assert.equal(zonedMidnight('2024-07-01'), Date.UTC(2024, 6, 1, 4) / 1000);
  st.cfg = { timezone: 'UTC' };
  core.resetFormats();
});

test('marking words never touches what escaping wrote', () => {
  assert.equal(core.mark('a & b', ['amp']), 'a &amp; b');
  assert.equal(core.mark('<b> lt', ['lt']), '&lt;b&gt; <mark>lt</mark>');
  assert.equal(core.mark('Fish & Chips', ['chips']), 'Fish &amp; <mark>Chips</mark>');
});

test('durations never read 60 min or 24 h', () => {
  assert.equal(fmtDuration(3599), '1.0 h');
  assert.equal(fmtDuration(3 * 86400 - 60), '3 d 0 h');
});

test('a day ends at the next midnight of the zone', () => {
  st.cfg = { timezone: 'Europe/Paris' };
  assert.equal(core.zonedDayEnd('2024-03-31') - zonedMidnight('2024-03-31'), 23 * 3600);
  assert.equal(core.zonedDayEnd('2024-12-31') - zonedMidnight('2024-12-31'), 86400);
  st.cfg = null;
});

test("the page's own redraws ask in the background", async () => {
  const seen = [];
  const was = globalThis.fetch;
  globalThis.fetch = async (url, init) => { seen.push(init.headers['X-Background']); return { ok: true, status: 200, json: async () => ({}) }; };
  try {
    await core.api('patients');
    st.redrawing = true;
    await core.api('patients');
  } finally {
    st.redrawing = false;
    globalThis.fetch = was;
  }
  assert.deepEqual(seen, [undefined, '1']);
});
