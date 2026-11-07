// A DICOM ECG waveform (made by the PACS viewer's tests) read into the ECG viewer.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const Dicom = require('../dicom.js');
const ECG = require('../ecg.js');
const { dicomJson } = require('../dicom-ecg.js');
const { ecgFile } = require('../../pacs/tests/make.js');

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
