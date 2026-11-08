// node --test viewers/ecg/tests/ — the registry of readers (readers.js): what a file is, by its
// extension or by its content, and which reader reads it; files the page does not read.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');

// the readers, loaded as the page loads them: the scripts of index.html that add one, in order
const PAGE = path.join(__dirname, '..');
const scripts = [...fs.readFileSync(path.join(PAGE, 'index.html'), 'utf8').matchAll(/<script src="([^"]+)"/g)].map((m) => m[1]);
for (const s of scripts) if (/Readers\.add\(/.test(fs.readFileSync(path.join(PAGE, s), 'utf8'))) require(path.join(PAGE, s));
const Readers = require('../readers.js');
const { ecgFile } = require('../../pacs/tests/make.js');

const ascii = (s) => Buffer.from(s, 'latin1');
const key = (name, bytes, type) => Readers.find(name, bytes, type)?.key ?? null;
/** The fixed part of an EDF header (256 bytes) and one signal's (256), with this patient field. */
function edfHead(patient = 'X X X X', version = '0') {
  const f = (t, w) => String(t).padEnd(w, ' ');
  return ascii(f(version, 8) + f(patient, 80) + f('Startdate X', 80) + '01.03.2413.00.00' + f(512, 8) + f('', 44) + f(1, 8) + f(1, 8) + f(1, 4)
    + f('ECG', 16) + f('', 80) + f('mV', 8) + f(-5, 8) + f(5, 8) + f(-32768, 8) + f(32767, 8) + f('', 80) + f(1, 8) + f('', 32) + '\0\0');
}

test('every reader of the page is in the registry, in order, the page\'s own kinds last', () => {
  assert.deepStrictEqual(Readers.all().map((r) => r.key), ['wfdb', 'muse', 'dicom', 'edf', 'json', 'picture', 'pdf']);
  for (const r of Readers.all()) {
    assert.ok(r.label && r.extensions.length && typeof r.sniff === 'function', r.key);
    assert.ok(typeof r.read === 'function' || ['pdf', 'picture'].includes(r.shown), r.key);
  }
  assert.deepStrictEqual(Readers.labels(), Readers.all().map((r) => r.label));
  const accept = Readers.accept().split(',');
  for (const e of ['.hea', '.xml', '.dcm', '.edf', '.bdf', '.json', '.pdf', '.png', '.jpg']) assert.ok(accept.includes(e), e);
  // a WFDB record is chosen by its header; its signal files after it, with a chooser of their own
  assert.ok(!accept.includes('.dat') && !accept.includes('.mat'));
  assert.strictEqual(Readers.companions(), '.dat,.mat');
  assert.strictEqual(Readers.companionOf('100.dat')?.key, 'wfdb');
  assert.strictEqual(Readers.companionOf('A0001.MAT')?.key, 'wfdb');
  assert.strictEqual(Readers.companionOf('100.hea'), null);
  assert.strictEqual(Readers.companionOf('a.xml'), null);
});

test('by extension, in any case', () => {
  const cases = {
    'rec.hea': 'wfdb', 'a.XML': 'muse', 'a.dcm': 'dicom', 'a.dicom': 'dicom', 'night.edf': 'edf', 'eeg.BDF': 'edf',
    'ecg.json': 'json', 'print.pdf': 'pdf', 'scan.png': 'picture', 'scan.JPG': 'picture', 'scan.jpeg': 'picture',
  };
  for (const [name, k] of Object.entries(cases)) {
    assert.strictEqual(key(name, new Uint8Array(0), ''), k, name);
    assert.strictEqual(Readers.byName(name)?.key, k, name);
  }
  // the extension wins over the content: a .hea that does not parse is still read as WFDB
  assert.strictEqual(key('x.hea', ascii('%PDF-1.4')), 'wfdb');
});

test('by content, when no reader claims the extension', () => {
  const dicom = ecgFile(true);
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const cases = [
    [ascii('%PDF-1.7\n'), 'pdf'], [png, 'picture'], [Buffer.from([0xff, 0xd8, 0xff, 0xe0]), 'picture'],
    [dicom, 'dicom'], [ascii('<?xml version="1.0"?><!DOCTYPE RestingECG SYSTEM "restecg.dtd"><RestingECG>'), 'muse'],
    [edfHead(), 'edf'], [Buffer.concat([Buffer.from([0xff]), ascii('BIOSEMI'), edfHead().subarray(8)]), 'edf'],
    [ascii('100 2 360 650000\n100.dat 212 200 11 1024 995 -22131 0 MLII\n'), 'wfdb'],
  ];
  for (const [bytes, k] of cases) assert.strictEqual(key('download', bytes, ''), k, `${k}: ${bytes.subarray(0, 12)}`);
  // by the media type a server states
  assert.strictEqual(key('ecg', ascii('{"leads": []}'), 'application/json'), 'json');
  assert.strictEqual(key('ecg', new Uint8Array(4), 'image/webp'), 'picture');
  // an EDF whose patient field reads like a WFDB record line is EDF: WFDB's loose sniff is tried last
  assert.strictEqual(key('download', edfHead('12 34 5'), ''), 'edf');
});

test('files the page does not read', () => {
  assert.strictEqual(key('notes.txt', ascii('a few words of text'), 'text/plain'), null);
  assert.strictEqual(key('blob', Buffer.from([1, 2, 3, 4, 5, 6, 7, 8, 9]), ''), null);
  assert.strictEqual(key('', new Uint8Array(0), ''), null);
  assert.strictEqual(key('nothing', null, ''), null);
  // a WFDB signal file is read with its header, not alone
  assert.strictEqual(Readers.byName('rec.dat'), null);
  assert.strictEqual(key('rec.dat', Buffer.alloc(64, 7), ''), null);
  assert.strictEqual(Readers.byName('readme'), null);
});

test('reading through the registry: the same record as each reader', async () => {
  const ECG = require('../ecg.js');
  const WFDB = require('../wfdb.js');
  const hea = 'r 2 250 3\nr.dat 16 1000/mV 16 0 0 0 0 I\nr.dat 16 1000/mV 16 0 0 0 0 II\n';
  const dat = Buffer.from(new Int16Array([1000, 2000, 0, 500, -1000, 250]).buffer);
  const rec = await Readers.find('r.hea').read('r.hea', ascii(hea), async (f) => { assert.strictEqual(f, 'r.dat'); return dat; });
  assert.deepStrictEqual(rec, WFDB.readWfdb(hea, { 'r.dat': dat }));
  assert.deepStrictEqual([...rec.leads[1].v], [2, 0.5, 0.25]);
  const obj = { fs: 500, leads: [{ name: 'II', units: 'uV', samples: [1000, -500] }] };
  assert.deepStrictEqual(await Readers.find('x.json').read('x.json', ascii(JSON.stringify(obj))), ECG.readJson(obj));
  const dicom = await Readers.find('e.dcm').read('e.dcm', ecgFile(true));
  assert.deepStrictEqual(dicom.leads.map((l) => l.name), ['I', 'II']);
  await assert.rejects(Readers.find('a.xml').read('a.xml', ascii('<?xml version="1.0"?><Other/>')), /not a MUSE resting ECG/);
  // pictures and PDF are not records: the page shows them as they are
  assert.strictEqual(Readers.find('a.pdf').read, undefined);
});

test('a multi-segment WFDB record\'s segments belong to it, not listed as ECGs of their own', () => {
  const dir = path.join(__dirname, '../../vitalstream/testdata/session/ms');
  const wfdb = Readers.byName('ms_rec.hea');
  assert.deepStrictEqual(wfdb.parts('ms_rec.hea', fs.readFileSync(path.join(dir, 'ms_rec.hea'))), ['ms_rec_layout.hea', 'ms_rec_0001.hea', 'ms_rec_0002.hea']);
  assert.deepStrictEqual(wfdb.parts('ms_rec_0001.hea', fs.readFileSync(path.join(dir, 'ms_rec_0001.hea'))), []);
});

test('a reader is added only with its interface', () => {
  assert.throws(() => Readers.add({ key: 'x' }), /needs key, label, extensions and sniff/);
  assert.throws(() => Readers.add({ key: 'x', label: 'X', extensions: [], sniff: () => false }), /read/);
});
