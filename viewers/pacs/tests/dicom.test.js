// node --test viewers/pacs/tests/*.test.js
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const Dicom = require('../dicom.js');

const LONG = new Set(['OB', 'OW', 'SQ', 'UN', 'UT']);

/** One element, explicit or implicit VR little endian. */
function el(g, e, vr, value, explicit = true) {
  let data;
  if (Buffer.isBuffer(value)) data = value;
  else if (vr === 'US') { data = Buffer.alloc(2); data.writeUInt16LE(value); }
  else if (vr === 'UL') { data = Buffer.alloc(4); data.writeUInt32LE(value); }
  else if (vr === 'FD') { data = Buffer.alloc(8); data.writeDoubleLE(value); }
  else { data = Buffer.from(String(value), 'latin1'); if (data.length % 2) data = Buffer.concat([data, Buffer.from(vr === 'UI' ? '\0' : ' ')]); }
  const tag = Buffer.alloc(4);
  tag.writeUInt16LE(g); tag.writeUInt16LE(e, 2);
  if (!explicit) {
    const len = Buffer.alloc(4); len.writeUInt32LE(data.length);
    return Buffer.concat([tag, len, data]);
  }
  if (LONG.has(vr)) {
    const h = Buffer.alloc(8); h.write(vr, 0, 'latin1'); h.writeUInt32LE(data.length, 4);
    return Buffer.concat([tag, h, data]);
  }
  const h = Buffer.alloc(4); h.write(vr, 0, 'latin1'); h.writeUInt16LE(data.length, 2);
  return Buffer.concat([tag, h, data]);
}
const item = (body) => { const h = Buffer.alloc(8); h.writeUInt16LE(0xfffe); h.writeUInt16LE(0xe000, 2); h.writeUInt32LE(body.length, 4); return Buffer.concat([h, body]); };
const delim = (e) => { const h = Buffer.alloc(8); h.writeUInt16LE(0xfffe); h.writeUInt16LE(e, 2); return h; };

function part10(transfer, body) {
  const meta = Buffer.concat([el(0x0002, 0x0001, 'OB', Buffer.from([0, 1])), el(0x0002, 0x0010, 'UI', transfer)]);
  return Buffer.concat([Buffer.alloc(128), Buffer.from('DICM'), el(0x0002, 0x0000, 'UL', meta.length), meta, body]);
}

test('explicit VR: two signed 16-bit frames, rescale, window, an ultrasound region', () => {
  const rows = 3; const cols = 4; const frames = 2;
  const px = Buffer.alloc(rows * cols * frames * 2);
  for (let i = 0; i < rows * cols * frames; i++) px.writeInt16LE(i - 10, 2 * i);
  // an undefined-length sequence with one undefined-length item
  const region = Buffer.concat([el(0x0018, 0x6024, 'US', 3), el(0x0018, 0x602c, 'FD', 0.05), el(0x0018, 0x602e, 'FD', 0.05)]);
  const sqHead = Buffer.alloc(12); sqHead.writeUInt16LE(0x0018); sqHead.writeUInt16LE(0x6011, 2); sqHead.write('SQ', 4, 'latin1'); sqHead.writeUInt32LE(0xffffffff, 8);
  const itemU = Buffer.alloc(8); itemU.writeUInt16LE(0xfffe); itemU.writeUInt16LE(0xe000, 2); itemU.writeUInt32LE(0xffffffff, 4);
  const sq = Buffer.concat([sqHead, itemU, region, delim(0xe00d), delim(0xe0dd)]);
  const body = Buffer.concat([
    el(0x0008, 0x0060, 'CS', 'CT'), el(0x0008, 0x103e, 'LO', 'Chest'), el(0x0010, 0x0010, 'PN', 'Doe^Jane'),
    sq,
    el(0x0018, 0x1063, 'DS', '33.3'),
    el(0x0020, 0x000e, 'UI', '1.2.3'), el(0x0020, 0x0013, 'IS', '7'),
    el(0x0028, 0x0002, 'US', 1), el(0x0028, 0x0004, 'CS', 'MONOCHROME2'), el(0x0028, 0x0008, 'IS', '2'),
    el(0x0028, 0x0010, 'US', rows), el(0x0028, 0x0011, 'US', cols),
    el(0x0028, 0x0100, 'US', 16), el(0x0028, 0x0101, 'US', 16), el(0x0028, 0x0103, 'US', 1),
    el(0x0028, 0x1050, 'DS', '40\\50'), el(0x0028, 0x1051, 'DS', '400'), el(0x0028, 0x1052, 'DS', '-1024'), el(0x0028, 0x1053, 'DS', '2'),
    el(0x7fe0, 0x0010, 'OW', px),
  ]);
  const info = Dicom.image(part10('1.2.840.10008.1.2.1', body));
  assert.strictEqual(info.modality, 'CT');
  assert.strictEqual(info.seriesDescription, 'Chest');
  assert.strictEqual(info.series, '1.2.3');
  assert.strictEqual(info.instance, 7);
  assert.strictEqual(info.frames, 2);
  assert.deepStrictEqual([info.center, info.width, info.slope, info.intercept], [40, 400, 2, -1024]);
  assert.strictEqual(Dicom.fps(info), 1000 / 33.3);
  assert.strictEqual(info.regions.length, 1);
  assert.strictEqual(info.regions[0].dx, 0.05);
  assert.ok(!JSON.stringify(Object.keys(info)).includes('Doe'), 'no identity in what is shown');
  const f1 = Dicom.nativeFrame(info, 1);
  assert.strictEqual(f1.length, rows * cols);
  assert.strictEqual(f1[0], (12 - 10) * 2 - 1024);
  const rgba = Dicom.toRgba(info, Float32Array.from([-160, 40, 240]), 40, 400, new Uint8ClampedArray(12));
  assert.deepStrictEqual([rgba[0], rgba[8]], [0, 255]);
  assert.ok(Math.abs(rgba[4] - 127.5) <= 0.5, 'the centre is mid-grey');
});

test('implicit VR without preamble; 8-bit RGB, planar', () => {
  const rows = 2; const cols = 2;
  const planes = Buffer.from([1, 2, 3, 4, 10, 20, 30, 40, 100, 110, 120, 130]);
  const body = Buffer.concat([
    el(0x0008, 0x0060, 'CS', 'US', false),
    el(0x0028, 0x0002, 'US', 3, false), el(0x0028, 0x0004, 'CS', 'RGB', false), el(0x0028, 0x0006, 'US', 1, false),
    el(0x0028, 0x0010, 'US', rows, false), el(0x0028, 0x0011, 'US', cols, false),
    el(0x0028, 0x0100, 'US', 8, false), el(0x0028, 0x0101, 'US', 8, false),
    el(0x7fe0, 0x0010, 'OW', planes, false),
  ]);
  const info = Dicom.image(body);
  assert.strictEqual(info.samples, 3);
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 0).slice(0, 6)), [1, 10, 100, 2, 20, 110]);
});

test('encapsulated JPEG: one fragment per frame', () => {
  const jpg = (n) => Buffer.from([0xff, 0xd8, n, n, 0xff, 0xd9]);
  const pxHead = Buffer.alloc(12); pxHead.writeUInt16LE(0x7fe0); pxHead.writeUInt16LE(0x0010, 2); pxHead.write('OB', 4, 'latin1'); pxHead.writeUInt32LE(0xffffffff, 8);
  const body = Buffer.concat([
    el(0x0028, 0x0002, 'US', 3), el(0x0028, 0x0004, 'CS', 'YBR_FULL_422'), el(0x0028, 0x0008, 'IS', '3'),
    el(0x0028, 0x0010, 'US', 16), el(0x0028, 0x0011, 'US', 16), el(0x0028, 0x0100, 'US', 8),
    pxHead, item(Buffer.alloc(0)), item(jpg(1)), item(jpg(2)), item(jpg(3)), delim(0xe0dd),
  ]);
  const info = Dicom.image(part10('1.2.840.10008.1.2.4.50', body));
  assert.strictEqual(info.jpeg, 'baseline');
  assert.strictEqual(info.frames, 3);
  assert.deepStrictEqual(Array.from(Dicom.jpegFrame(info, 2)), [0xff, 0xd8, 3, 3, 0xff, 0xd9]);
  // lossless JPEG is decoded here (jpeg.js); JPEG-LS is not
  assert.strictEqual(Dicom.image(part10('1.2.840.10008.1.2.4.70', body)).decoded, true);
  assert.throws(() => Dicom.image(part10('1.2.840.10008.1.2.4.80', body)), (e) => e.unsupported && /JPEG-LS: browsers do not decode it/.test(e.message));
  assert.throws(() => Dicom.image(Buffer.from('not dicom at all, really')), /not a DICOM file/);
});
