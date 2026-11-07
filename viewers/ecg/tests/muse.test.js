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

test('the rate as the Python reader reads it: a base that is not positive is 500 Hz, an exponent beyond ±6 refused', () => {
  const rate = (base, exp) => MUSE.decode(bytes).replaceAll('<SampleBase>500</SampleBase>', `<SampleBase>${base}</SampleBase>`)
    .replaceAll('<SampleExponent>0</SampleExponent>', `<SampleExponent>${exp}</SampleExponent>`);
  assert.strictEqual(MUSE.read(rate('250', '1')).fs, 2500);
  for (const base of ['nan', 'inf', '-500', '0', '']) assert.strictEqual(MUSE.read(rate(base, '0')).fs, 500, base);
  for (const exp of ['7', '-7', '3000000']) assert.throws(() => MUSE.read(rate('500', exp)), /SampleExponent/);
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
