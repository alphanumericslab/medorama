// The JavaScript MUSE reader against the Python one: the same synthetic file (fixtures/make_muse.py)
// gives the same JSON form (fixtures/synthetic_muse.json, written by muse_reader_py).
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const MUSE = require('../muse.js');
const ECG = require('../ecg.js');

const FIX = path.join(__dirname, 'fixtures');
const bytes = fs.readFileSync(path.join(FIX, 'synthetic_muse.xml'));
const golden = JSON.parse(fs.readFileSync(path.join(FIX, 'synthetic_muse.json'), 'utf8'));

test('same as the Python reader', () => {
  const e = MUSE.read(bytes);
  assert.deepStrictEqual(e.meta, golden.meta);
  assert.strictEqual(e.fs, golden.fs);
  assert.deepStrictEqual(e.median.marks, golden.median.marks);
  assert.deepStrictEqual(e.median.leads.map((l) => l.samples), golden.median.leads.map((l) => l.samples));
  assert.deepStrictEqual(e.leads.map((l) => l.name), golden.leads.map((l) => l.name));
  e.leads.forEach((l, j) => {
    const g = golden.leads[j].samples;
    assert.strictEqual(l.samples.length, g.length);
    l.samples.forEach((v, i) => assert.ok(Math.abs(v - g[i]) < 0.002, `${l.name}[${i}] ${v} != ${g[i]}`));
  });
});

test('ISO-8859-1, identity never read, the viewer reads it', () => {
  assert.ok(MUSE.isMuse(bytes));
  const e = MUSE.read(bytes);
  assert.deepStrictEqual(e.meta.fields.find((f) => f[0] === 'Site'), ['Site', 'Clinique Médicale']);
  const s = JSON.stringify(e);
  for (const x of ['SYN000123', 'TESTPATIENT', 'SYNTHETIC', '1970']) assert.ok(!s.includes(x), x);
  const rec = ECG.readJson(e);
  assert.strictEqual(rec.leads[1].name, 'II');
  assert.ok(Math.max(...rec.leads[1].v) > 0.95);          // mV
  assert.strictEqual(rec.meta.statements[1].source, 'confirmed');
});

test('de-identified report', () => {
  const m = ECG.deidMeta(MUSE.read(bytes).meta, 86400, true);
  assert.deepStrictEqual(m.fields.find((f) => f[0] === 'Acquired'), ['Acquired', '2020-03-13 08:15:30']);
  assert.strictEqual(m.statements[1].time, '2020-03-14 10:02:00');
  assert.strictEqual(m.statements[1].lines[2], 'When compared with ECG of 31-DEC-2018');
  assert.deepStrictEqual(ECG.deidMeta({ fields: [['Age', '93']], measurements: [], statements: [], notes: [] }, 0, true).fields, [['Age', '90+']]);
});

test('damaged XML: refused at once, never a long search', () => {
  for (const bad of ['<RestingECG><a' + ' '.repeat(200000), '<a'.repeat(100000), '<!--'.repeat(50000), '<!'.repeat(100000), '<?'.repeat(100000),
    '<a "'.repeat(50000), '<RestingECG><![CDATA[x', '<RestingECG>x < y</RestingECG>']) {
    const t0 = Date.now();
    assert.throws(() => MUSE.parseXml(bad), /not XML/);
    assert.ok(Date.now() - t0 < 500, `${bad.slice(0, 16)}: ${Date.now() - t0} ms`);
  }
  const x = MUSE.parseXml('<?xml version="1.0"?><!DOCTYPE r><!-- c --><r a="/" b=\'x\'><e/><f x="1"/><g>t<![CDATA[<y>]]></g></r>');
  assert.deepStrictEqual(x.children.map((c) => c.tag), ['e', 'f', 'g']);
  assert.strictEqual(x.children[2].text, 't<y>');
});

test('the rate: a base that is not a rate is unknown, and the strip is shown by sample number', () => {
  const rate = (base, exp) => MUSE.decode(bytes).replaceAll('<SampleBase>500</SampleBase>', `<SampleBase>${base}</SampleBase>`)
    .replaceAll('<SampleExponent>0</SampleExponent>', `<SampleExponent>${exp}</SampleExponent>`);
  const e = MUSE.read(rate('250', '1'));
  assert.strictEqual(e.fs, 2500);
  assert.ok(e.meta.notes.some((n) => n.includes('SAMPLE_EXPONENT_NOT_0')));
  for (const [base, exp] of [['nan', '0'], ['inf', '0'], ['-500', '0'], ['0', '0'], ['', '0'], ['500', '7'], ['500', '-7'], ['500', '3000000'], ['500', '0.5']]) {
    const u = MUSE.read(rate(base, exp));
    assert.strictEqual(u.fs, null, `${base} ${exp}`);
    assert.strictEqual(u.rateUnknown, true);
    assert.ok(u.meta.notes.some((n) => n.includes('BAD_SAMPLE_BASE')));
    assert.deepStrictEqual(u.median.marks, []);            // no rate, no marks
  }
  const rec = ECG.readJson(MUSE.read(rate('', '')));
  assert.strictEqual(rec.rateUnknown, true);
  assert.strictEqual(rec.leads[0].fs, null);
  const Paper = require('../paper.js');
  const d = Paper.draw(rec, { layout: '3x4', twelve: ECG.twelveLead(rec), highpass: true, notch: 60 });
  assert.ok(d.samples);
  assert.match(d.svg, /100 samples per 5 mm \(rate unknown\)/);
  assert.ok((d.svg.match(/class="trace" d="[^"]*"/)[0].match(/L/g) || []).length > 3500);   // every lead drawn (12 quarter strips and the rhythm: about 4000)
  assert.match(d.svg, /class="axis"[^>]*>500</);
  assert.doesNotMatch(d.svg, /high-pass|notch| Hz/);        // no filter, no rate
  const hit = d.snap(d.left + 10, 20);                       // 10 mm into lead I: sample 200
  assert.ok(hit && hit.lead === 'I' && Math.abs(hit.n - 200) <= 20);
});

/** A MUSE file of `fs` Hz: a 10 s rhythm strip and a 1.2 s median beat, the onsets at 500 Hz. */
function made({ fs = 250, device = 'MAC 8', program = '250', qtrggr = 124, upb = '4.88', rhythmN = 10 * fs } = {}) {
  const b64 = (n) => Buffer.from(new Int16Array(Array.from({ length: n }, (_, i) => Math.round(200 * Math.sin(i / 7)))).buffer).toString('base64');
  const wave = (kind, n) => `<Waveform><WaveformType>${kind}</WaveformType><SampleBase>${fs}</SampleBase><SampleExponent>0</SampleExponent>`
    + ['I', 'II', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'].map((l) => `<LeadData><LeadSampleCountTotal>${n}</LeadSampleCountTotal>`
      + `<LeadAmplitudeUnitsPerBit>${upb}</LeadAmplitudeUnitsPerBit><LeadAmplitudeUnits>MICROVOLTS</LeadAmplitudeUnits>`
      + `<LeadID>${l}</LeadID><WaveFormData>${b64(n)}</WaveFormData></LeadData>`).join('') + '</Waveform>';
  return `<RestingECG><TestDemographics><AcquisitionDevice>${device}</AcquisitionDevice><AnalysisSoftwareVersion>${program}</AnalysisSoftwareVersion></TestDemographics>`
    + '<RestingECGMeasurements><QRSDuration>92</QRSDuration><POnset>230</POnset><QOnset>300</QOnset><QOffset>346</QOffset><TOffset>500</TOffset>'
    + '<ECGSampleBase>500</ECGSampleBase></RestingECGMeasurements>'
    + `<QRSTimesTypes><GlobalRR>200</GlobalRR><QTRGGR>${qtrggr}</QTRGGR></QRSTimesTypes>`
    + `${wave('Median', Math.round(1.2 * fs))}${wave('Rhythm', rhythmN)}</RestingECG>`;
}

test('a 250 Hz format with its onsets at 500 Hz: known, the marks on the median beat at its own rate, RR in 4 ms', () => {
  const e = MUSE.read(made());
  assert.strictEqual(e.fs, 250);
  assert.deepStrictEqual(e.meta.notes, []);
  assert.deepStrictEqual(e.meta.fields.find((f) => f[0] === 'Recording'), ['Recording', 'MAC 8 · 12SL 250 · 250 Hz · known format']);
  assert.deepStrictEqual(e.median.marks, [{ name: 'P', sample: 115 }, { name: 'QRS', sample: 150 }, { name: 'QRS end', sample: 173 }, { name: 'T end', sample: 250 }]);
  assert.deepStrictEqual(e.meta.measurements.find((m) => m.name === 'RR'), { name: 'RR', value: 800, unit: 'ms' });
  assert.strictEqual(e.median.leads.length, 12);
  // the median beats on paper: 3 × 4, the marks dotted, named under the last row
  const Paper = require('../paper.js');
  const rec = ECG.readJson(e);
  const d = Paper.drawMedian(rec, { speed: 25, gain: 10 });
  for (const l of ECG.STANDARD) assert.ok(d.svg.includes(`>${l}</text>`), l);
  assert.match(d.svg, /class="mark" d="M/);
  assert.match(d.svg, /class="markname"[^>]*>QRS</);
  assert.match(d.svg, /Median beats {3}25 mm\/s {3}10 mm\/mV {3}250 Hz/);
  const hit = d.snap(d.left + 1, 20);
  assert.ok(hit && hit.lead === 'I' && hit.t >= 0);
  // the same cart at 500 Hz is not a known format; a wrong QTRGGR is reported, not used
  assert.ok(MUSE.read(made({ fs: 500 })).meta.notes.some((n) => n.includes('NEW_FORMAT')));
  const q = MUSE.read(made({ qtrggr: 248 }));
  assert.strictEqual(q.fs, 250);
  assert.ok(q.meta.notes.some((n) => n.includes('QTRGGR_CONTRADICTS_FS')));
});

test('what does not fit is reported: a short strip; a file without an amplitude scale or of other samples is refused', () => {
  const short = MUSE.read(made({ rhythmN: 1 }));
  assert.ok(short.meta.notes.some((n) => n.includes('RHYTHM_NOT_10_S')));
  assert.ok(short.meta.notes.some((n) => n.includes('NEW_FORMAT')));
  assert.throws(() => MUSE.read(made({ upb: '' })), /amplitude scale/);
  assert.throws(() => MUSE.read(made().replace('<LeadSampleCountTotal>', '<LeadSampleSize>1</LeadSampleSize><LeadSampleCountTotal>')), /16-bit/);
  assert.strictEqual(MUSE.FORMATS.length, 30);
});

test('edited, not confirmed; not MUSE', () => {
  const t = MUSE.decode(bytes).replace('<Status>CONFIRMED</Status>', '<Status>UNCONFIRMED</Status>');
  const st = MUSE.read(t).meta.statements;
  assert.deepStrictEqual(st.map((x) => x.source), ['computer', 'edited']);
  assert.strictEqual(ECG.cleanMeta({ statements: st }).statements[1].source, 'edited');
  assert.throws(() => MUSE.read('<?xml version="1.0"?><Other/>'), /not a MUSE/);
  assert.throws(() => MUSE.read('<RestingECG>'), /not XML/);
  assert.deepStrictEqual([...MUSE.base64('AQID')], [1, 2, 3]);
  assert.deepStrictEqual([...MUSE.base64('AQ\nI')], [1, 2]);
});

test('UTF-16 and UTF-8 files: by the byte order mark, by the zero bytes of UTF-16, by the declaration', async () => {
  const text = MUSE.decode(bytes);
  const golden16 = MUSE.read(bytes);
  const as = (t, decl) => t.replace('encoding="ISO-8859-1"', `encoding="${decl}"`);
  const le = (t) => Buffer.from(t, 'utf16le');
  const be = (t) => { const b = le(t); for (let i = 0; i + 1 < b.length; i += 2) [b[i], b[i + 1]] = [b[i + 1], b[i]]; return b; };
  const bom = (mark, b) => Buffer.concat([Buffer.from(mark), b]);
  const files = {
    'UTF-16LE, BOM': bom([0xff, 0xfe], le(as(text, 'UTF-16'))),
    'UTF-16BE, BOM': bom([0xfe, 0xff], be(as(text, 'UTF-16'))),
    'UTF-16LE, no BOM': le(as(text, 'UTF-16')),
    'UTF-16BE, no BOM': be(as(text, 'UTF-16')),
    'UTF-16LE, BOM, a wrong declaration': bom([0xff, 0xfe], le(text)),
    'UTF-8, BOM': bom([0xef, 0xbb, 0xbf], Buffer.from(as(text, 'UTF-8'))),
    'UTF-8, BOM, a wrong declaration': bom([0xef, 0xbb, 0xbf], Buffer.from(text)),
    'UTF-8 declared UTF-16': Buffer.from(as(text, 'UTF-16')),
  };
  assert.deepStrictEqual(MUSE.encodingOf(files['UTF-16BE, no BOM']), { enc: 'utf-16be', skip: 0 });
  assert.deepStrictEqual(MUSE.encodingOf(files['UTF-16LE, BOM']), { enc: 'utf-16le', skip: 2 });
  assert.deepStrictEqual(MUSE.encodingOf(bytes), { enc: 'iso-8859-1', skip: 0 });
  const Readers = require('../readers.js');
  for (const [what, b] of Object.entries(files)) {
    const u8 = new Uint8Array(b);
    assert.ok(MUSE.isMuse(u8), what);
    assert.ok(!MUSE.decode(u8).startsWith('﻿'), what);
    const e = MUSE.read(u8);
    assert.deepStrictEqual(e.meta, golden16.meta, what);
    assert.deepStrictEqual(e.leads, golden16.leads, what);
    assert.deepStrictEqual(e.meta.fields.find((f) => f[0] === 'Site'), ['Site', 'Clinique Médicale'], what);
    assert.strictEqual(Readers.find('ecg.xml', u8).key, 'muse', what);
    assert.strictEqual((await Readers.find('ecg.xml', u8).read('ecg.xml', u8)).leads.length, 12, what);
  }
});
