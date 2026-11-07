// node --test viewers/ecg/tests/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const ECG = require('../ecg.js');
const WFDB = require('../wfdb.js');
const Paper = require('../paper.js');

const LEADS = ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'];

/** Format 16 samples, interleaved by frame. */
function fmt16(frames) {
  const b = Buffer.alloc(frames.length * frames[0].length * 2);
  let o = 0;
  for (const f of frames) for (const v of f) { b.writeInt16LE(v, o); o += 2; }
  return b;
}

/** Format 212: two 12-bit samples in three bytes. */
function fmt212(samples) {
  const b = Buffer.alloc(Math.ceil(samples.length / 2) * 3);
  for (let k = 0; k < samples.length; k += 2) {
    const a = samples[k] & 0xfff;
    const c = (samples[k + 1] ?? 0) & 0xfff;
    const o = (k / 2) * 3;
    b[o] = a & 0xff;
    b[o + 1] = ((a >> 8) & 0x0f) | ((c >> 4) & 0xf0);
    b[o + 2] = c & 0xff;
  }
  return b;
}

test('a 12-lead WFDB record, format 16', () => {
  const fs = 500;
  const n = fs * 10;
  const frames = Array.from({ length: n }, (_, i) => LEADS.map((_, j) => (j + 1) * 10 + (i % 7)));
  frames[3][2] = -32768;   // a missing sample
  const hea = `ecg1 12 ${fs} ${n} 09:30:00 02/01/2024\n`
    + LEADS.map((l) => `ecg1.dat 16 1000(0)/mV 16 0 0 0 0 ${l}`).join('\n') + '\n# a comment\n';
  assert.deepStrictEqual(WFDB.wfdbFiles(hea), ['ecg1.dat']);
  const rec = WFDB.readWfdb(hea, { 'ecg1.dat': fmt16(frames) });
  assert.strictEqual(rec.fs, 500);
  assert.strictEqual(rec.start, Date.UTC(2024, 0, 2, 9, 30) / 1000);
  assert.strictEqual(rec.leads.length, 12);
  assert.strictEqual(rec.leads[1].name, 'II');
  assert.strictEqual(rec.leads[1].v.length, n);
  assert.strictEqual(rec.leads[1].v[1], (20 + 1) / 1000);
  assert.ok(Number.isNaN(rec.leads[2].v[3]));
  const twelve = ECG.twelveLead(rec);
  assert.ok(twelve && twelve.aVF === rec.leads[5]);
  const d = Paper.draw(rec, { layout: '3x4', speed: 25, gain: 10, twelve });
  assert.ok(d.svg.startsWith('<svg') && d.svg.includes('>aVR<'));
  assert.ok(d.width >= 250 && d.height > 100);
});

test('format 212, gain in µV, a baseline, the first 10 s only', () => {
  const fs = 250;
  const n = fs * 12;
  const v = Array.from({ length: n * 2 }, (_, i) => ((i * 37) % 2000) - 1000);
  const hea = `r2 2 ${fs} ${n}\nr2.dat 212 2(-4)/uV 12 0 0 0 0 MLII\nr2.dat 212 2(-4)/uV 12 0 0 0 0 V5\n`;
  const rec = WFDB.readWfdb(hea, { 'r2.dat': fmt212(v) });
  assert.strictEqual(rec.leads[0].v.length, fs * 10);
  // (sample − baseline) / gain, in µV, to mV
  assert.strictEqual(rec.leads[0].v[0], ((v[0] + 4) / 2) / 1000);
  assert.strictEqual(rec.leads[1].v[5], ((v[11] + 4) / 2) / 1000);
  assert.strictEqual(ECG.standardLead('MLII'), 'II');
  assert.strictEqual(ECG.standardLead('ECG aVL'), 'aVL');
  assert.strictEqual(ECG.twelveLead(rec), null);
  const d = Paper.draw(rec, { speed: 25, gain: 10, twelve: null });
  assert.ok(d.svg.includes('>MLII<') && d.svg.includes('>V5<'));
});

test('the JSON form and errors', () => {
  const rec = ECG.readJson({ name: 'x', start: '2024-01-02T09:30:00Z', fs: 500, leads: [{ name: 'II', units: 'uV', samples: [1000, null, -500] }] });
  assert.deepStrictEqual(Array.from(rec.leads[0].v.slice(0, 1)), [1]);
  assert.ok(Number.isNaN(rec.leads[0].v[1]));
  assert.strictEqual(rec.start, Date.UTC(2024, 0, 2, 9, 30) / 1000);
  assert.throws(() => ECG.readJson({}), /leads/);
  assert.throws(() => WFDB.parseHeader(''), /empty/);
  assert.throws(() => WFDB.readWfdb('r 1 250 10\nr.dat 16 200 16 0 0 0 0 I\n', {}), /missing/);
});

test('metadata from WFDB comments, never identity; calipers snap to the trace', () => {
  const fs = 500;
  const n = fs * 10;
  const frames = Array.from({ length: n }, (_, i) => LEADS.map(() => (i % 500 === 100 ? 1000 : 0)));
  const hea = `e 12 ${fs} ${n}\n` + LEADS.map((l) => `e.dat 16 1000/mV 16 0 0 0 0 ${l}`).join('\n')
    + '\n# Name: Doe^Jane\n# Patient ID: 12345\n# Age: 67\n# QTc: 455 ms\n# Vent rate: 60\n# Dx: Sinus rhythm; Normal ECG\n';
  const rec = WFDB.readWfdb(hea, { 'e.dat': fmt16(frames) });
  assert.deepStrictEqual(rec.meta.fields, [['Age', '67']]);
  assert.ok(!JSON.stringify(rec.meta).includes('Doe') && !JSON.stringify(rec.meta).includes('12345'));
  assert.deepStrictEqual(rec.meta.measurements.map((m) => [m.name, m.value, m.unit]), [['QTc', 455, 'ms'], ['Vent. rate', 60, 'bpm']]);
  assert.deepStrictEqual(rec.meta.statements[0].lines, ['Sinus rhythm', 'Normal ECG']);
  const d = Paper.draw(rec, { layout: '3x4', speed: 25, gain: 10, twelve: ECG.twelveLead(rec) });
  // a click 3 mm beside the R wave of lead II's rhythm strip, at its peak's height, snaps onto it
  const t = 100 / fs + 1;                                   // the second beat's R wave (1.2 s)
  const strip = d.snap(d.left + t * 25 + 0.3, 200);         // far below: the baseline nearby
  assert.ok(strip && Math.abs(strip.t - t) < 0.05);
  // lead I's first R wave (0.2 s, 1 mV) peaks 10 mm above its baseline (row 1: 6 + 0.62 × 32 mm)
  const near = d.snap(d.left + (100 / fs) * 25 + 0.6, 6 + 0.62 * 32 - 10 + 0.5);
  assert.ok(near && Math.abs(near.v - 1) < 1e-9 && Math.abs(near.t - 100 / fs) < 1e-9, JSON.stringify(near));
  assert.strictEqual(d.snap(2, 2), null, 'the margin is not on a trace');
});

test('the JSON form carries its metadata, cleaned', () => {
  const rec = ECG.readJson({
    fs: 500, leads: [{ name: 'II', samples: [0, 1] }],
    meta: {
      fields: [['Acquired', '2024-01-02 09:30'], ['Patient name', 'Doe']],
      measurements: [{ name: 'QT', value: 400, unit: 'ms' }, { name: 'ST', value: 0.05, unit: 'mV', lead: 'V2' }],
      statements: [{ source: 'computer', program: 'Marquette 12SL', lines: ['Sinus rhythm'] }, { source: 'confirmed', by: 'Dr. A', lines: ['Normal ECG'] }],
    },
  });
  assert.deepStrictEqual(rec.meta.fields, [['Acquired', '2024-01-02 09:30']]);
  assert.strictEqual(rec.meta.measurements[1].lead, 'V2');
  assert.deepStrictEqual(rec.meta.statements.map((s) => s.source), ['computer', 'confirmed']);
});

test('WFDB comments: confirmed and edited statements, a lead\'s measurements, the program', () => {
  const m = ECG.metaFromLines([
    'Program: Marquette 12SL v243', 'Dx: Sinus rhythm; ST depression, consider subendocardial injury', 'Dx confirmed: Sinus rhythm | Abnormal ECG',
    'Confirmed by: A. Rivera', 'Confirmed at: 2024-01-02 15:20', 'ST V5: -0.16 mV', 'R amp (V5): 2.1 mV', 'QTc: 471 ms', 'Patient: nobody',
  ]);
  assert.deepStrictEqual(m.statements, [
    { source: 'computer', program: 'Marquette 12SL v243', lines: ['Sinus rhythm', 'ST depression, consider subendocardial injury'] },
    { source: 'confirmed', by: 'A. Rivera', time: '2024-01-02 15:20', lines: ['Sinus rhythm', 'Abnormal ECG'] },
  ]);
  assert.deepStrictEqual(m.measurements, [
    { name: 'ST', value: -0.16, unit: 'mV', lead: 'V5' }, { name: 'R amp', value: 2.1, unit: 'mV', lead: 'V5' }, { name: 'QTc', value: 471, unit: 'ms' },
  ]);
  assert.deepStrictEqual(m.fields, []);
});

test('identity never shows, whatever the key that names it', () => {
  const keys = ['Name', 'Full name', 'Surname', 'Firstname', 'Pt name', 'Pt_ID', 'PatientID', 'Patient name', 'Subject', 'Subject ID', 'MRN',
    'Medical record number', 'Hospital number', 'Hospital no', 'Accession', 'Accession number', 'NHS number', 'SSN', 'Email', 'E-mail', 'Zip',
    'Postal code', 'Postcode', 'Address', 'Phone', 'Tel', 'DOB', 'Born', 'Date of birth', 'Birth date', 'ID', 'UID', 'Visit number', 'Order no', 'City'];
  const m = ECG.metaFromLines(keys.map((k) => `${k}: SYN-${k}`).concat(['Age: 67', 'Sex: F', 'Chart speed: 25 mm/s', 'Rapid: yes', 'Width: 3']));
  for (const k of keys) assert.ok(!JSON.stringify(m).includes(`SYN-${k}`), k);
  assert.deepStrictEqual(m.fields.map((f) => f[0]), ['Age', 'Sex', 'Chart speed', 'Rapid', 'Width']);
  // the JSON form too
  const j = ECG.cleanMeta({ fields: keys.map((k) => [k, `SYN-${k}`]).concat([['Site', 'Clinic']]) });
  assert.deepStrictEqual(j.fields, [['Site', 'Clinic']]);
});

test('a de-identified view: ages above 89 years as 90+, in their units; the start moved back', () => {
  const ages = (k, v) => ECG.deidMeta({ fields: [[k, v]], measurements: [], statements: [], notes: [] }, 0, true).fields[0][1];
  assert.strictEqual(ages('Age', '93'), '90+');
  assert.strictEqual(ages('<age>', '95'), '90+');                 // PhysioNet's "# <age>: 95"
  assert.strictEqual(ages('Age (years)', '91 years'), '90+');
  assert.strictEqual(ages('Age', '095Y'), '90+');
  assert.strictEqual(ages('Age', '95 days'), '95 days');
  assert.strictEqual(ages('Age', '1100 months'), '90+');
  assert.strictEqual(ages('Age', '95 months'), '95 months');
  assert.strictEqual(ages('Age', '60 weeks'), '60 weeks');
  assert.strictEqual(ages('Age', '67'), '67');
  assert.strictEqual(ages('Age', 'unknown'), 'unknown');
  const start = Date.UTC(2024, 2, 12, 14, 0, 5) / 1000;
  assert.strictEqual(ECG.startText(start), '2024-03-12 14:00:05');
  assert.strictEqual(ECG.startText(start, 86400 * 400), '2023-02-06 14:00:05');
  assert.strictEqual(ECG.startText(null, 5), '');
  assert.strictEqual(ECG.startText(1e20), '');
  const app = require('node:fs').readFileSync(require('node:path').join(__dirname, '..', 'app.js'), 'utf8');
  assert.ok(/ECG\.startText\(rec\.start, \+params\.get\('shift'\) \|\| 0\)/.test(app), 'the page shows the start moved back');
});

test('a skewed WFDB signal is read `skew` frames later, as the WFDB library reads it', () => {
  const frames = Array.from({ length: 10 }, (_, i) => [i, 100 + i]);
  const hea = 'sk 2 100 10\nsk.dat 16 1/mV 16 0 0 0 0 I\nsk.dat 16:3 1/mV 16 0 0 0 0 II\n';
  const rec = WFDB.readWfdb(hea, { 'sk.dat': fmt16(frames) });
  assert.deepStrictEqual([...rec.leads[0].v], [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  assert.deepStrictEqual([...rec.leads[1].v.slice(0, 7)], [103, 104, 105, 106, 107, 108, 109]);
  assert.ok([...rec.leads[1].v.slice(7)].every(Number.isNaN), 'past the end: missing');
  // only the first seconds read: the skewed signal's frames come from further on
  const long = Array.from({ length: 300 }, (_, i) => [i, 1000 + i]);
  const r2 = WFDB.readWfdb('sk 2 10 300\nsk.dat 16 1/mV 16 0 0 0 0 I\nsk.dat 16:5 1/mV 16 0 0 0 0 II\n', { 'sk.dat': fmt16(long) }, 10);
  assert.strictEqual(r2.leads[0].v.length, 100);
  assert.strictEqual(r2.leads[1].v.length, 100);
  assert.strictEqual(r2.leads[1].v[99], 1104);
});

test('a multi-segment WFDB record: segments in order, a null segment and a missing lead as gaps', async () => {
  const fs = require('node:fs');
  const path = require('node:path');
  const dir = path.join(__dirname, '../../vitalstream/testdata/session/ms');
  const get = async (f) => { const b = fs.readFileSync(path.join(dir, f)); return b.buffer.slice(b.byteOffset, b.byteOffset + b.length); };
  const text = fs.readFileSync(path.join(dir, 'ms_rec.hea'), 'utf8');
  assert.deepStrictEqual(WFDB.segmentTable(text).segments.map((s) => [s.name, s.n]),
    [['ms_rec_layout', 0], ['ms_rec_0001', 500], ['~', 125], ['ms_rec_0002', 250]]);
  assert.strictEqual(WFDB.segmentTable(fs.readFileSync(path.join(dir, 'ms_rec_0001.hea'), 'utf8')), null);
  const rec = await WFDB.readWfdbAny(text, get, 0);
  assert.strictEqual(rec.name, 'ms_rec');
  assert.deepStrictEqual(rec.leads.map((l) => [l.name, l.v.length]), [['II', 875], ['V', 875]]);
  const [ii, v] = rec.leads;
  assert.ok(Number.isFinite(ii.v[499]) && Number.isNaN(ii.v[500]) && Number.isNaN(ii.v[624]) && Number.isFinite(ii.v[625]));
  assert.ok(Number.isFinite(v.v[0]) && Number.isNaN(v.v[700]));   // V is not in the second segment
  assert.strictEqual(rec.start, Date.UTC(2024, 2, 1, 12, 0, 10) / 1000);
  // the first 10 s by default: here the whole record (3.5 s)
  assert.strictEqual((await WFDB.readWfdbAny(text, get)).leads[0].v.length, 875);
  // a single-segment record through the same call
  const one = await WFDB.readWfdbAny(fs.readFileSync(path.join(dir, 'ms_rec_0001.hea'), 'utf8'), get);
  assert.deepStrictEqual(one.leads.map((l) => l.name), ['II', 'V']);
});
