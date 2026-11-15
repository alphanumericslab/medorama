// A DICOM ECG waveform (made by the PACS viewer's tests) read into the ECG viewer.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const Dicom = require('../dicom.js');
const ECG = require('../ecg.js');
const { dicomJson, leadOfCode } = require('../dicom-ecg.js');
const { el, item, sq, part10, ecgFile } = require('../../pacs/tests/make.js');

test('a DICOM ECG on paper: leads in mV, the report from its annotations', () => {
  const f = ecgFile(true);
  const rec = ECG.readJson(dicomJson(Dicom.waveforms(f)));
  assert.strictEqual(rec.fs, 500);
  assert.deepStrictEqual(rec.leads.map((l) => l.name), ['I', 'II']);
  assert.deepStrictEqual([...rec.leads[0].v], [0, 0.25, 0.5, 0.75, 1]);
  assert.deepStrictEqual(rec.meta.measurements, [{ name: 'QT', value: 412, unit: 'ms', lead: '' }]);
  assert.deepStrictEqual(rec.meta.statements[0].lines, ['Sinus rhythm']);
  assert.ok(!JSON.stringify(rec.meta).includes('Doe'));
});

// twelve channels named by their codes' meanings (SCPECG, DICOM CID 3001, older and newer wordings)
const MEANINGS = [['1', 'I (Einthoven)'], ['2', 'Lead II'], ['61', 'III'], ['62', 'aVR, augmented voltage, right'],
  ['63', 'aVL, augmented voltage, left'], ['64', 'aVF, augmented voltage, foot'], ['3', 'V1'], ['4', 'Lead V2'],
  ['5', 'V3 (chest)'], ['6', 'Lead V4'], ['7', 'V5'], ['8', 'V6']];
function twelveLeadFile() {
  const x = true;
  const n = 4;
  const data = Buffer.alloc(n * 12 * 2);
  for (let i = 0; i < n; i++) for (let c = 0; c < 12; c++) data.writeInt16LE(c * 10 + i, 2 * (i * 12 + c));
  const channel = ([v, meaning]) => item(sq(0x003a, 0x0208, [item(el(0x0008, 0x0100, 'SH', `5.6.3-9-${v}`, x),
    el(0x0008, 0x0102, 'SH', 'SCPECG', x), el(0x0008, 0x0104, 'LO', meaning, x))], x), el(0x003a, 0x0210, 'DS', '1', x));
  const group = item(el(0x003a, 0x0005, 'US', 12, x), el(0x003a, 0x0010, 'UL', n, x), el(0x003a, 0x001a, 'DS', '500', x),
    sq(0x003a, 0x0200, MEANINGS.map(channel), x), el(0x5400, 0x1004, 'US', 16, x), el(0x5400, 0x1006, 'CS', 'SS', x), el(0x5400, 0x1010, 'OW', data, x));
  return part10('1.2.840.10008.1.2.1', Buffer.concat([el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.9.1.1', x),
    el(0x0008, 0x0060, 'CS', 'ECG', x), sq(0x5400, 0x0100, [group], x)]));
}

test('a 12-lead DICOM ECG whose leads are named by their code meanings: the twelve leads, the 12-lead layout', () => {
  const rec = ECG.readJson(dicomJson(Dicom.waveforms(twelveLeadFile())));
  assert.deepStrictEqual(rec.leads.map((l) => l.name), ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6']);
  const twelve = ECG.twelveLead(rec);
  assert.ok(twelve);
  assert.strictEqual(twelve.aVR.v[1], (3 * 10 + 1) / 1000);       // the fourth channel, in µV → mV
  for (const [label, name] of [['I (Einthoven)', 'I'], ['aVR, augmented voltage, right', 'aVR'], ['Lead II', 'II'], ['Lead III', 'III'],
    ['V1', 'V1'], [' lead aVF ', 'aVF'], ['ECG V6 (mV)', 'V6'], ['MLII', 'II'], ['Pleth', null], ['', null], [undefined, null]]) {
    assert.strictEqual(ECG.standardLead(label), name, label);
  }
});

test('leads by their codes: MDC (ISO/IEEE 11073-10101) and SCP-ECG', () => {
  const mdc = [[1, 'I'], [2, 'II'], [61, 'III'], [62, 'aVR'], [63, 'aVL'], [64, 'aVF'], [3, 'V1'], [4, 'V2'], [5, 'V3'], [6, 'V4'], [7, 'V5'], [8, 'V6']];
  for (const [k, name] of mdc) {
    assert.strictEqual(leadOfCode({ value: `2:${k}`, scheme: 'MDC' }), name);
    assert.strictEqual(leadOfCode({ value: String(2 * 65536 + k), scheme: 'MDC' }), name);   // context-free (131329 is I)
    assert.strictEqual(leadOfCode({ value: `5.6.3-9-${k}`, scheme: 'SCPECG' }), name);
  }
  for (const c of [null, {}, { value: '2:9', scheme: 'MDC' }, { value: '2:1', scheme: 'SCPECG' }, { value: '5.6.3-9-1', scheme: 'MDC' }, { value: '1:1', scheme: 'MDC' }]) {
    assert.strictEqual(leadOfCode(c), null, JSON.stringify(c));
  }
  // a channel with a code and a label that names no lead (its meaning missing): named by the code
  const ch = (code, name) => ({ name, units: 'uV', code, samples: Float64Array.of(0, 1) });
  const w = { groups: [{ label: '', fs: 500, channels: mdc.map(([k]) => ch({ value: `2:${k}`, scheme: 'MDC', meaning: '' }, `Channel ${k}`)) }],
    annotations: { measurements: [], text: [] }, acquired: '' };
  const rec = ECG.readJson(dicomJson(w));
  assert.deepStrictEqual(rec.leads.map((l) => l.name), mdc.map((x) => x[1]));
  assert.ok(ECG.twelveLead(rec));
  // no code, a label that names no lead: as it is
  assert.strictEqual(dicomJson({ ...w, groups: [{ label: '', fs: 500, channels: [ch(null, 'Pleth')] }] }).leads[0].name, 'Pleth');
});

test('a DICOM ECG whose channel sources have codes only (no meanings): dicom.js keeps the code, the leads are named by it', () => {
  const x = true;
  const codes = [1, 2, 61, 62, 63, 64, 3, 4, 5, 6, 7, 8];
  const data = Buffer.alloc(2 * 12 * 2);
  const channel = (k) => item(sq(0x003a, 0x0208, [item(el(0x0008, 0x0100, 'SH', `2:${k}`, x), el(0x0008, 0x0102, 'SH', 'MDC', x))], x),
    el(0x003a, 0x0210, 'DS', '1', x));
  const group = item(el(0x003a, 0x0005, 'US', 12, x), el(0x003a, 0x0010, 'UL', 2, x), el(0x003a, 0x001a, 'DS', '500', x),
    sq(0x003a, 0x0200, codes.map(channel), x), el(0x5400, 0x1004, 'US', 16, x), el(0x5400, 0x1006, 'CS', 'SS', x), el(0x5400, 0x1010, 'OW', data, x));
  const f = part10('1.2.840.10008.1.2.1', Buffer.concat([el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.9.1.1', x),
    el(0x0008, 0x0060, 'CS', 'ECG', x), sq(0x5400, 0x0100, [group], x)]));
  const w = Dicom.waveforms(f);
  assert.deepStrictEqual(w.groups[0].channels[0].code, { value: '2:1', scheme: 'MDC', meaning: '' });
  const rec = ECG.readJson(dicomJson(w));
  assert.deepStrictEqual(rec.leads.map((l) => l.name), ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6']);
  assert.ok(ECG.twelveLead(rec));
});
