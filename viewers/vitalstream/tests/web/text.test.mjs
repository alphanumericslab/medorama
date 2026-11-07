// The page's text (web/js/text.js), without a browser: the Recording panel shows names and
// reasons from the files as text, and channels are grouped by their labels.  `node --test`.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { displayModality, escapeHtml, infoContent } from '../../web/js/text.js';

const ds = {
  format: 'WFDB records', description: '', channels: [], pieces: [], vitals: [], alarms: [], gaps: [],
  markers: { count: 0, names: [] }, time_resolution_s: null,
  notes: ['<b>note</b>'],
  skipped: { '<img src=x onerror=alert(document.domain)>': '/data/<img src=x onerror=alert(1)>.hea: cannot read' },
};

test('a skipped record and its reason are text, never HTML', () => {
  const { rows, notes } = infoContent(ds, [1709287200, 1709290800], 'UTC');
  assert.equal(rows[0][0], 'Format');
  assert.deepEqual(notes, [
    '<b>note</b>',
    'Not shown: <img src=x onerror=alert(document.domain)> (/data/<img src=x onerror=alert(1)>.hea: cannot read)',
  ]);
  // the page puts the panel's text into the document with textContent only
  const app = fs.readFileSync(new URL('../../web/js/app.js', import.meta.url), 'utf8');
  const render = app.slice(app.indexOf('function renderInfo()'), app.indexOf('let vitalsTimer'));
  assert.ok(render.includes('textContent') && !render.includes('innerHTML'), 'renderInfo builds DOM nodes with textContent');
});

test('no style attributes in the page markup (the Content-Security-Policy allows none)', () => {
  for (const f of ['../../web/js/app.js', '../../web/js/render.js', '../../web/js/host.js', '../../web/index.html']) {
    const text = fs.readFileSync(new URL(f, import.meta.url), 'utf8');
    assert.ok(!/style="/.test(text), `${f}: a style attribute`);
    if (f.endsWith('.html')) assert.ok(!/<script(?![^>]*\bsrc=)[^>]*>/.test(text) && !/<style/.test(text), 'no inline script or style');
  }
});

test('escaping', () => {
  assert.equal(escapeHtml('<a href="x">O\'B & c</a>'), '&lt;a href=&quot;x&quot;&gt;O&#39;B &amp; c&lt;/a&gt;');
  assert.equal(escapeHtml(null), '');
});

test('channel groups from common monitor and sleep-study labels', () => {
  const table = [
    // label, unit, group (undefined: the reader's own, 'edf')
    ['ECG1', '', 'ecg'], ['ECG II', 'mV', 'ecg'], ['EKG2', '', 'ecg'], ['ECG_II', '', 'ecg'], ['II', 'mV', 'ecg'],
    ['V5', 'mV', 'ecg'], ['MCL1', '', 'ecg'],
    ['P1', '', 'pressure'], ['P2', 'mmHg', 'pressure'], ['ART1', 'mmHg', 'pressure'], ['ABP', 'mmHg', 'pressure'],
    ['PAP', 'mmHg', 'pressure'], ['CVP', 'mmHg', 'pressure'], ['ICP', 'mmHg', 'pressure'], ['CPP', 'mmHg', 'pressure'],
    ['AWP', 'cmH2O', 'vent'], ['Paw', 'cmH2O', 'vent'], ['PEEP', 'cmH2O', 'vent'],
    ['SpO2', '%', 'spo2'], ['Pleth', '', 'spo2'], ['SaO2', '%', 'spo2'],
    ['CO2', 'mmHg', 'co2'], ['etCO2', 'kPa', 'co2'],
    ['O2', '%', 'gas'], ['FiO2', '%', 'gas'],
    ['Resp', '', 'resp'], ['Flow', 'L/min', 'resp'], ['Thor', '', 'resp'], ['Abdomen', '', 'resp'], ['Imp', 'Ohm', 'resp'],
    ['C3-M2', 'uV', 'eeg'], ['E1-M2', 'uV', 'eeg'], ['E2', 'uV', 'eeg'], ['M1', 'uV', 'eeg'], ['Fp1', '', 'eeg'],
    ['O2', 'uV', 'eeg'], ['O1-M2', 'uV', 'eeg'], ['P3', 'µV', 'eeg'], ['Pz', 'uV', 'eeg'], ['T3', 'uV', 'eeg'],
    ['EEG Fpz-Cz', 'uV', 'eeg'], ['EOG horizontal', 'uV', 'eeg'], ['LOC-M2', 'uV', 'eeg'],
    ['EMG1', 'uV', 'eeg'], ['Chin1', 'uV', 'eeg'], ['Leg', 'uV', 'eeg'],
    ['T1', '°C', 'edf'], ['Temp', '°C', 'edf'], ['HR', 'bpm', 'edf'],
  ];
  for (const [label, unit, want] of table) {
    assert.equal(displayModality({ modality: 'edf', label, unit }), want, `${label} (${unit})`);
  }
  // a reader's own modality is kept
  assert.equal(displayModality({ modality: 'pressure', label: 'C3' }), 'pressure');
});
