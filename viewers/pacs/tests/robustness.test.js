// The readers on damaged and random input: each must answer quickly, with a result or an Error
// (which the pages show as a message), never hang or allocate without bound.  Truncations of
// valid files are the common case (a copy cut short, a download stopped).
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const Dicom = require('../dicom.js');
const ECG = require('../../ecg/ecg.js');
const WFDB = require('../../ecg/wfdb.js');
const { dicomJson } = require('../../ecg/dicom-ecg.js');
const MUSE = require('../../ecg/muse.js');
const Paper = require('../../ecg/paper.js');
const fs = require('node:fs');
const path = require('node:path');
const { el, item, sq, coded, part10, encapsulated, rle, ecgFile } = require('./make.js');

let seed = 7;
const rand = () => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed / 0x7fffffff; };

/** Run f on input; an Error is an answer, anything else thrown, or a slow answer, fails. */
function answers(f, input, what) {
  const t = Date.now();
  let out;
  try { out = f(input); } catch (e) { assert.ok(e instanceof Error && !(e instanceof assert.AssertionError), `${what}: threw ${e}`); }
  assert.ok(Date.now() - t < 2000, `${what}: took ${Date.now() - t} ms`);
  return out;
}

const samples = () => {
  const img = part10('1.2.840.10008.1.2.1', Buffer.concat([
    el(0x0028, 0x0002, 'US', 1), el(0x0028, 0x0008, 'IS', '2'), el(0x0028, 0x0010, 'US', 4), el(0x0028, 0x0011, 'US', 4),
    el(0x0028, 0x0100, 'US', 8), el(0x7fe0, 0x0010, 'OB', Buffer.alloc(32, 9)),
  ]));
  const rl = part10('1.2.840.10008.1.2.5', Buffer.concat([
    el(0x0028, 0x0010, 'US', 2), el(0x0028, 0x0011, 'US', 2), el(0x0028, 0x0100, 'US', 8), encapsulated([rle([[1, 2, 3, 4]])]),
  ]));
  const sr = part10('1.2.840.10008.1.2.1', Buffer.concat([
    el(0x0040, 0xa040, 'CS', 'CONTAINER'),
    sq(0x0040, 0xa730, [item(el(0x0040, 0xa040, 'CS', 'NUM'), coded(0x0040, 0xa043, 'x', 'LN', 'EF'),
      sq(0x0040, 0xa300, [item(el(0x0040, 0xa30a, 'DS', '50'))]))]),
  ]));
  const muse = fs.readFileSync(path.join(__dirname, '..', '..', 'ecg', 'tests', 'fixtures', 'synthetic_muse.xml'));
  return { img, rl, sr, ecg: ecgFile(true), muse };
};

const readers = {
  image: (b) => { const i = Dicom.image(b); for (let k = 0; k < i.frames; k++) (i.encapsulated && !i.rle ? Dicom.jpegFrame : Dicom.nativeFrame)(i, k); },
  kind: (b) => Dicom.kind(b),
  report: (b) => Dicom.report(b),
  document: (b) => Dicom.documentOf(b),
  waveforms: (b) => ECG.readJson(dicomJson(Dicom.waveforms(b))),
  muse: (b) => ECG.readJson(MUSE.read(b)),
};

test('truncated and corrupted DICOM and MUSE files', () => {
  for (const [name, buf] of Object.entries(samples())) {
    for (let cut = 0; cut < buf.length; cut += Math.max(1, Math.floor(buf.length / 97))) {
      const part = buf.subarray(0, cut);
      for (const [r, f] of Object.entries(readers)) answers(f, part, `${r} of ${name} cut at ${cut}`);
    }
    for (let n = 0; n < 60; n++) {
      const bad = Buffer.from(buf);
      for (let k = 0; k < 8; k++) bad[Math.floor(rand() * bad.length)] = Math.floor(rand() * 256);
      for (const [r, f] of Object.entries(readers)) answers(f, bad, `${r} of corrupted ${name}`);
    }
  }
});

test('random bytes', () => {
  for (let n = 0; n < 300; n++) {
    const b = Buffer.alloc(Math.floor(rand() * 600));
    for (let i = 0; i < b.length; i++) b[i] = Math.floor(rand() * 256);
    if (n % 2) b.write('DICM', 128 < b.length - 4 ? 128 : 0, 'latin1');
    for (const [r, f] of Object.entries(readers)) answers(f, b, r);
  }
});

test('records the paper cannot draw are refused, not drawn as nonsense', () => {
  const bad = [
    { leads: [] },
    { fs: 500, leads: [{ name: 'I', units: 'mV', samples: [] }] },
    { fs: 0, leads: [{ name: 'I', units: 'mV', samples: [1, 2] }] },
    { fs: 500, leads: [{ name: 'I', units: 'mV', samples: [null, null] }] },
  ];
  for (const obj of bad) {
    let rec;
    try { rec = ECG.readJson(obj); } catch (e) { assert.ok(e instanceof Error); continue; }
    let d;
    try { d = Paper.draw(rec, { layout: '3x4', twelve: ECG.twelveLead(rec) }); } catch (e) { assert.ok(e instanceof Error); continue; }
    assert.ok(Number.isFinite(d.width) && Number.isFinite(d.height) && !/NaN|Infinity/.test(d.svg), `${JSON.stringify(obj)}: ${d.svg.slice(0, 80)}`);
  }
});

test('WFDB records with every edge value: drawn finite, or refused with a message', () => {
  let drawn = 0;
  for (const fs of ['500', '0', '-1', '1e9', 'NaN', '0.001']) {
    for (const gain of ['200/mV', '0/mV', '1e-9/mV', '200(5)/uV', '']) {
      for (const fmt of ['16', '212', '8', '80', '310', '61', '24', '32', '0']) {
        for (const n of ['4', '0', '1000000000', '']) {
          for (const len of [0, 3, 16, 4096]) {
            const text = `rec 2 ${fs} ${n}\nrec.dat ${fmt} ${gain} 16 0 0 0 0 I\nrec.dat ${fmt} ${gain} 16 0 0 0 0 II`;
            const d = answers(() => {
              const rec = WFDB.readWfdb(text, { 'rec.dat': Buffer.alloc(len, 7) });
              return Paper.draw(rec, { layout: '3x4', twelve: ECG.twelveLead(rec) });
            }, null, text);
            if (d) {
              drawn++;
              assert.ok(!/NaN|Infinity/.test(d.svg), `drawn with NaN: ${text} (${len} bytes)`);
            }
          }
        }
      }
    }
  }
  assert.ok(drawn > 500, `only ${drawn} drawn: the test no longer reaches the paper`);
});
