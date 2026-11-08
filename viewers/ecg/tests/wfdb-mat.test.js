// node --test viewers/ecg/tests/ — WFDB records whose signal file is a MATLAB file (.mat,
// docs/wfdb-collection.md, "MATLAB signal files"), the checks of the engine's
// libs/engine/tests/wfdb_mat.rs with the same messages: level 4 and level 5 files are read as the
// .dat file of the same samples; one that does not match its header is refused; no more frames
// are read than the header states; a missing signal file is named, for the page to ask for it.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const WFDB = require('../wfdb.js');

// two signals of eight samples each, frame by frame, as a .dat file and a MAT matrix of two rows
// (MATLAB stores a matrix column by column) both hold them
const SAMPLES = [[0, 800], [10, 810], [20, 790], [-15, 805], [5, 800], [30, 820], [-40, 780], [0, 800]];

function frames(be = false) {
  const b = Buffer.alloc(SAMPLES.length * 4);
  SAMPLES.flat().forEach((v, i) => (be ? b.writeInt16BE(v, 2 * i) : b.writeInt16LE(v, 2 * i)));
  return b;
}

const header = (name, file, fmt, nsamp = 8) =>
  `${name} 2 250 ${nsamp} 11:00:00 01/03/2024\n${file} ${fmt} 200/mV 16 0 0 0 0 II\n${file} ${fmt} 10/mmHg 16 0 800 0 0 ART\n`;

/** A level 4 MAT file: its type (MOPT), rows, columns and the name `val`, then `data`. */
function mat4(type, rows, cols, data, be = false) {
  const h = Buffer.alloc(20);
  [type, rows, cols, 0, 4].forEach((v, i) => (be ? h.writeUInt32BE(v, 4 * i) : h.writeUInt32LE(v, 4 * i)));
  return Buffer.concat([h, Buffer.from('val\0', 'latin1'), data]);
}

/** A level 5 MAT file of one matrix (little-endian): as wfdb2mat writes it (the samples at byte
 * 192), or as MATLAB does (the name in a small element: at byte 184). */
function mat5(rows, cols, data, { small = false, cls = 10, elem = 3, version = 0x0100 } = {}) {
  const head = Buffer.alloc(128);
  head.write('MATLAB 5.0', 0, 'latin1');
  head.writeUInt16LE(version, 124);
  head.write('IM', 126, 'latin1');
  const u32 = (...v) => { const b = Buffer.alloc(4 * v.length); v.forEach((x, i) => b.writeUInt32LE(x, 4 * i)); return b; };
  const name = small ? Buffer.concat([u32((3 << 16) | 1), Buffer.from('val\0', 'latin1')]) : Buffer.concat([u32(1, 3), Buffer.from('val\0\0\0\0\0', 'latin1')]);
  const body = Buffer.concat([u32(6, 8), Buffer.from([cls, 0, 0, 0, 0, 0, 0, 0]), u32(5, 8, rows, cols), name, u32(elem, data.length), data,
    Buffer.alloc((8 - (data.length % 8)) % 8)]);
  return Buffer.concat([head, u32(14, body.length), body]);
}

const leads = (rec) => rec.leads.map((l) => Array.from(l.v));

test('MAT files are read as the .dat file of the same samples', () => {
  const dat = leads(WFDB.readWfdb(header('r', 'r.dat', '16'), { 'r.dat': frames() }, 0));
  const cases = {
    m4: ['16+24', mat4(30, 2, 8, frames())],
    m5: ['16+192', mat5(2, 8, frames())],
    ml: ['16+184', mat5(2, 8, frames(), { small: true })],
    mb: ['61+24', mat4(1030, 2, 8, frames(true), true)],
  };
  for (const [name, [fmt, bytes]] of Object.entries(cases)) {
    assert.deepStrictEqual(leads(WFDB.readWfdb(header(name, `${name}.mat`, fmt), { [`${name}.mat`]: bytes }, 0)), dat, name);
  }
});

test('a MAT file that does not match its header is refused, with the reason', () => {
  const le = frames();
  const doubles = Buffer.alloc(SAMPLES.length * 16);
  SAMPLES.flat().forEach((v, i) => doubles.writeDoubleLE(v, 8 * i));
  const compressed = mat5(2, 8, le);
  compressed.writeUInt32LE(15, 128);
  const cases = [
    ['compressed', compressed, '16+192', 8, 'it is compressed'],
    ['hdf5', mat5(2, 8, le, { version: 0x0200 }), '16+192', 8, 'MATLAB 7.3 (HDF5)'],
    ['doubles', mat4(0, 2, 8, doubles), '16+24', 8, 'it holds little-endian double-precision numbers; format 16 reads little-endian 16-bit integers'],
    ['doubles5', mat5(2, 8, doubles, { cls: 6, elem: 9 }), '16+192', 8, 'double-precision'],
    ['transposed', mat4(30, 8, 2, le), '16+24', 8, 'its matrix has 8 rows; the header gives 2 signals in it, one row each (it is transposed: one column per signal)'],
    ['offset', mat4(30, 2, 8, le), '16+20', 8, 'its samples start at byte 24; the header gives +20'],
    ['no_offset', mat4(30, 2, 8, le), '16', 8, 'its samples start at byte 24; the header gives +0'],
    ['length', mat4(30, 2, 8, le), '16+24', 10, 'it holds 8 samples per signal; the header gives 10'],
    ['short', mat4(30, 2, 8, le.subarray(0, 20)), '16+24', 8, 'it is shorter than its 2 × 8 matrix'],
    ['order', mat4(1030, 2, 8, le, true), '16+24', 8, 'it holds big-endian 16-bit integers; format 16 reads little-endian 16-bit integers'],
    ['format', mat4(30, 2, 8, le), '212+24', 8, 'format 212, which no MAT matrix holds'],
    ['text', Buffer.from('hello, world: not a MAT file at all', 'latin1'), '16', 8, 'it is not a MAT file (level 4 or 5) that WFDB reads'],
  ];
  for (const [name, bytes, fmt, nsamp, why] of cases) {
    assert.throws(() => WFDB.readWfdb(header(name, `${name}.mat`, fmt, nsamp), { [`${name}.mat`]: bytes }, 0),
      (e) => e.message.startsWith(`${name}.mat does not match its header ${name}.hea: `) && e.message.includes(why), name);
  }
});

test('no more frames are read than the header states (a MAT file is padded to 8 bytes)', () => {
  // one signal of three samples: 6 bytes of samples, then 2 of padding
  const v = Buffer.alloc(6);
  [5, -5, 7].forEach((x, i) => v.writeInt16LE(x, 2 * i));
  const hea = 'p 1 250 3\np.mat 16+192 200/mV 16 0 0 0 0 II\n';
  const rec = WFDB.readWfdb(hea, { 'p.mat': mat5(1, 3, v) }, 0);
  assert.deepStrictEqual(Array.from(rec.leads[0].v), [5 / 200, -5 / 200, 7 / 200]);
});

test('a missing signal file is named', () => {
  assert.throws(() => WFDB.readWfdb(header('r', 'r.mat', '16+24'), {}), (e) => e.missing === 'r.mat' && /r\.mat is missing/.test(e.message));
});
