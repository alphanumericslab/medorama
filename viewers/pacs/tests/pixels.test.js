// node --test viewers/pacs/tests/: enhanced multi-frame functional groups, the extended offset
// table and JPEG frame splitting, 32-bit pixels, native YBR colour, MONOCHROME1 inversion.
// Everything is synthetic.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const Dicom = require('../dicom.js');
const { el, item, sq, part10 } = require('./make.js');

const EXPLICIT = '1.2.840.10008.1.2.1';
const IMPLICIT = '1.2.840.10008.1.2';
const BASELINE = '1.2.840.10008.1.2.4.50';

/** The image module's elements: size, samples, photometric, bits, frames. */
function header({ rows, cols, samples = 1, photometric = 'MONOCHROME2', bits = 16, stored = bits, signed = 0, frames = 1, planar = null }, x = true) {
  return [
    el(0x0008, 0x0060, 'CS', 'CT', x),
    el(0x0028, 0x0002, 'US', samples, x), el(0x0028, 0x0004, 'CS', photometric, x),
    ...(planar === null ? [] : [el(0x0028, 0x0006, 'US', planar, x)]),
    el(0x0028, 0x0008, 'IS', String(frames), x),
    el(0x0028, 0x0010, 'US', rows, x), el(0x0028, 0x0011, 'US', cols, x),
    el(0x0028, 0x0100, 'US', bits, x), el(0x0028, 0x0101, 'US', stored, x), el(0x0028, 0x0103, 'US', signed, x),
  ];
}

/** A functional groups item: Pixel Value Transformation, Pixel Measures, Frame VOI LUT (those given). */
function groups({ slope, intercept, spacing, thickness, center, width }, x = true) {
  const parts = [];
  if (spacing !== undefined || thickness !== undefined) {
    parts.push(sq(0x0028, 0x9110, [item(...(spacing ? [el(0x0028, 0x0030, 'DS', spacing.join('\\'), x)] : []), ...(thickness !== undefined ? [el(0x0018, 0x0050, 'DS', String(thickness), x)] : []))], x));
  }
  if (slope !== undefined) parts.push(sq(0x0028, 0x9145, [item(el(0x0028, 0x1052, 'DS', String(intercept), x), el(0x0028, 0x1053, 'DS', String(slope), x))], x));
  if (center !== undefined) parts.push(sq(0x0028, 0x9132, [item(el(0x0028, 0x1050, 'DS', String(center), x), el(0x0028, 0x1051, 'DS', String(width), x))], x));
  return item(...parts);
}

for (const x of [true, false]) {
  test(`enhanced multi-frame (${x ? 'explicit' : 'implicit'} VR): per-frame groups over shared over the top level`, () => {
    const rows = 2; const cols = 2; const frames = 3;
    const px = Buffer.alloc(rows * cols * frames * 2);
    for (let i = 0; i < rows * cols * frames; i++) px.writeUInt16LE(i, 2 * i);
    const body = Buffer.concat([
      ...header({ rows, cols, frames }, x),
      // a top-level rescale and window, which the functional groups override
      el(0x0028, 0x1050, 'DS', '7', x), el(0x0028, 0x1051, 'DS', '9', x), el(0x0028, 0x1052, 'DS', '5', x), el(0x0028, 0x1053, 'DS', '3', x),
      sq(0x5200, 0x9229, [groups({ slope: 2, intercept: -1000, spacing: [0.5, 0.7], thickness: 1.25, center: 40, width: 400 }, x)], x),
      sq(0x5200, 0x9230, [
        groups({}, x),
        groups({ slope: 1, intercept: 0, center: 100, width: 200 }, x),
        groups({ spacing: [0.3, 0.4], intercept: 10, slope: 0.5 }, x),
      ], x),
      el(0x7fe0, 0x0010, 'OW', px, x),
    ]);
    const info = Dicom.image(part10(x ? EXPLICIT : IMPLICIT, body));
    assert.strictEqual(info.frames, 3);
    // shared over the top level
    assert.deepStrictEqual([info.slope, info.intercept, info.center, info.width, info.thickness], [2, -1000, 40, 400, 1.25]);
    assert.deepStrictEqual(info.spacing, [0.5, 0.7]);
    assert.strictEqual(info.frameWindows, true);
    // frame 0: the shared values
    assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 0)), [-1000, -998, -996, -994]);
    assert.deepStrictEqual(Dicom.autoWindow(info, Dicom.nativeFrame(info, 0), 0), [40, 400]);
    // frame 1: its own rescale and window
    assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 1)), [4, 5, 6, 7]);
    assert.deepStrictEqual(Dicom.autoWindow(info, Dicom.nativeFrame(info, 1), 1), [100, 200]);
    // frame 2: its own rescale and spacing, the shared window
    assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 2)), [14, 14.5, 15, 15.5]);
    const a = Dicom.frameAttrs(info, 2);
    assert.deepStrictEqual(a.spacing, [0.3, 0.4]);
    assert.deepStrictEqual([a.center, a.width, a.thickness], [40, 400, 1.25]);
  });
}

test('a classic multi-frame file: the top-level values, no per-frame ones', () => {
  const px = Buffer.alloc(8);
  for (let i = 0; i < 4; i++) px.writeUInt16LE(i, 2 * i);
  const body = Buffer.concat([...header({ rows: 1, cols: 2, frames: 2 }), el(0x0028, 0x1052, 'DS', '1'), el(0x0028, 0x1053, 'DS', '3'), el(0x0028, 0x0030, 'DS', '0.2\\0.2'), el(0x7fe0, 0x0010, 'OW', px)]);
  const info = Dicom.image(part10(EXPLICIT, body));
  assert.strictEqual(info.perFrame, null);
  assert.strictEqual(info.frameWindows, false);
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 1)), [7, 10]);
  assert.deepStrictEqual(Dicom.frameAttrs(info, 1).spacing, [0.2, 0.2]);
});

// ---------------------------------------------------------------- encapsulated frames
const fragItem = (b) => { const h = Buffer.alloc(8); h.writeUInt16LE(0xfffe); h.writeUInt16LE(0xe000, 2); h.writeUInt32LE(b.length, 4); return Buffer.concat([h, b]); };
/** Encapsulated pixel data: an empty Basic Offset Table, then these fragments. */
function pixelItems(fragments) {
  const head = Buffer.alloc(12);
  head.writeUInt16LE(0x7fe0); head.writeUInt16LE(0x0010, 2); head.write('OB', 4, 'latin1'); head.writeUInt32LE(0xffffffff, 8);
  const end = Buffer.alloc(8); end.writeUInt16LE(0xfffe); end.writeUInt16LE(0xe0dd, 2);
  return Buffer.concat([head, fragItem(Buffer.alloc(0)), ...fragments.map(fragItem), end]);
}
const u64 = (xs) => { const b = Buffer.alloc(8 * xs.length); xs.forEach((v, i) => b.writeBigUInt64LE(BigInt(v), 8 * i)); return b; };
const jpegFile = (frames, fragments, extra = []) => part10(BASELINE, Buffer.concat([...header({ rows: 2, cols: 2, bits: 8, frames }), ...extra, pixelItems(fragments)]));
const bytesOf = (info, i) => Array.from(Dicom.jpegFrame(info, i));

test('encapsulated JPEG, empty offset table, more fragments than frames: a frame starts at each FF D8', () => {
  const f = [Buffer.from([0xff, 0xd8, 1, 2]), Buffer.from([3, 4]), Buffer.from([0xff, 0xd8, 5, 6]), Buffer.from([7, 0xff, 0xd9, 0])];
  const info = Dicom.image(jpegFile(2, f));
  assert.strictEqual(info.frames, 2);
  assert.deepStrictEqual(bytesOf(info, 0), [0xff, 0xd8, 1, 2, 3, 4]);
  assert.deepStrictEqual(bytesOf(info, 1), [0xff, 0xd8, 5, 6, 7, 0xff, 0xd9, 0]);
});

test('the Extended Offset Table says where frames start (over FF D8 guessing), its lengths drop the padding', () => {
  // frame 0 in two fragments, the second of which happens to start with FF D8; frame 1 in one
  const f = [Buffer.from([0xff, 0xd8, 1, 2]), Buffer.from([0xff, 0xd8, 3, 4]), Buffer.from([0xff, 0xd8, 5, 0])];
  const second = (8 + 4) * 2;
  const eot = [el(0x7fe0, 0x0001, 'OV', u64([0, second])), el(0x7fe0, 0x0002, 'OV', u64([8, 3]))];
  const info = Dicom.image(jpegFile(2, f, eot));
  assert.strictEqual(info.frames, 2);
  assert.deepStrictEqual(bytesOf(info, 0), [0xff, 0xd8, 1, 2, 0xff, 0xd8, 3, 4]);
  assert.deepStrictEqual(bytesOf(info, 1), [0xff, 0xd8, 5]);
  // without the table, the guess splits at every FF D8: three frames, of which the header's two are kept
  const guessed = Dicom.image(jpegFile(2, f));
  assert.deepStrictEqual(bytesOf(guessed, 0), [0xff, 0xd8, 1, 2]);
});

test('one fragment per frame, and one frame in several fragments, as before', () => {
  const f = [Buffer.from([0xff, 0xd8, 1, 2]), Buffer.from([0xff, 0xd8, 3, 4])];
  const two = Dicom.image(jpegFile(2, f));
  assert.deepStrictEqual(bytesOf(two, 1), [0xff, 0xd8, 3, 4]);
  const one = Dicom.image(jpegFile(1, [Buffer.from([0xff, 0xd8, 1, 2]), Buffer.from([3, 4])]));
  assert.strictEqual(one.frames, 1);
  assert.deepStrictEqual(bytesOf(one, 0), [0xff, 0xd8, 1, 2, 3, 4]);
});

// ---------------------------------------------------------------- 32-bit pixels
test('32-bit pixels: signed and unsigned, all 32 bits stored, and 24 of 32', () => {
  const s32 = [-1, -(2 ** 31), 2 ** 31 - 1, 0];
  const px = Buffer.alloc(16);
  s32.forEach((v, i) => px.writeInt32LE(v, 4 * i));
  const signed = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 2, cols: 2, bits: 32, signed: 1 }), el(0x7fe0, 0x0010, 'OW', px)])));
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(signed, 0)), s32.map(Math.fround));
  const unsigned = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 2, cols: 2, bits: 32 }), el(0x7fe0, 0x0010, 'OW', px)])));
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(unsigned, 0)), [2 ** 32 - 1, 2 ** 31, 2 ** 31 - 1, 0].map(Math.fround));
  // 24 bits stored: the high byte is not the value's
  const p24 = Buffer.alloc(16);
  [0xab800000, 0xcd7fffff, 0x00000005, 0xffffffff].forEach((v, i) => p24.writeUInt32LE(v, 4 * i));
  const s24 = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 2, cols: 2, bits: 32, stored: 24, signed: 1 }), el(0x7fe0, 0x0010, 'OW', p24)])));
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(s24, 0)), [-(2 ** 23), 2 ** 23 - 1, 5, -1]);
});

test('16-bit signed and 12 of 16 bits, unchanged', () => {
  const px = Buffer.alloc(6);
  [0xf800, 0x07ff, 0xffff].forEach((v, i) => px.writeUInt16LE(v, 2 * i));
  const info = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 1, cols: 3, bits: 16, stored: 12, signed: 1 }), el(0x7fe0, 0x0010, 'OW', px)])));
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 0)), [-2048, 2047, -1]);
});

// ---------------------------------------------------------------- native YBR colour
const clamp = (v) => Math.max(0, Math.min(255, Math.round(v)));
const rgb = (y, cb, cr) => [clamp(y + 1.402 * (cr - 128)), clamp(y - 0.344136 * (cb - 128) - 0.714136 * (cr - 128)), clamp(y + 1.772 * (cb - 128))];
const near = (a, b) => assert.ok(a.length === b.length && a.every((v, i) => Math.abs(v - b[i]) <= 1), `${a} ≉ ${b}`);
const YCC = [[128, 128, 128], [76, 85, 255], [255, 128, 128], [29, 255, 107]];

test('native YBR_FULL, interleaved and planar: converted to RGB', () => {
  const inter = Buffer.from(YCC.flat());
  const planar = Buffer.from([0, 1, 2].flatMap((c) => YCC.map((p) => p[c])));
  const want = YCC.flatMap((p) => rgb(...p));
  for (const [cfg, px] of [[0, inter], [1, planar]]) {
    const info = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 2, cols: 2, samples: 3, photometric: 'YBR_FULL', bits: 8, planar: cfg }), el(0x7fe0, 0x0010, 'OB', px)])));
    near(Array.from(Dicom.nativeFrame(info, 0)), want);
  }
  // red (76, 85, 255) is red
  near(rgb(76, 85, 255), [255, 0, 0]);
});

test('native YBR_FULL_422: two bytes a pixel, upsampled, frames of rows*cols*2 bytes', () => {
  // 2 frames of 2×2: per pair of pixels Y0 Y1 Cb Cr
  const f0 = [100, 150, 90, 200, 20, 240, 128, 128];
  const f1 = [50, 60, 200, 100, 255, 0, 160, 96];
  const info = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 2, cols: 2, samples: 3, photometric: 'YBR_FULL_422', bits: 8, frames: 2, planar: 0 }), el(0x7fe0, 0x0010, 'OB', Buffer.from([...f0, ...f1]))])));
  assert.strictEqual(info.frames, 2);
  const expand = (f) => [rgb(f[0], f[2], f[3]), rgb(f[1], f[2], f[3]), rgb(f[4], f[6], f[7]), rgb(f[5], f[6], f[7])].flat();
  near(Array.from(Dicom.nativeFrame(info, 0)), expand(f0));
  near(Array.from(Dicom.nativeFrame(info, 1)), expand(f1));
});

test('native RGB: unchanged', () => {
  const px = Buffer.from([1, 2, 3, 4, 5, 6]);
  const info = Dicom.image(part10(EXPLICIT, Buffer.concat([...header({ rows: 1, cols: 2, samples: 3, photometric: 'RGB', bits: 8, planar: 0 }), el(0x7fe0, 0x0010, 'OB', px)])));
  assert.deepStrictEqual(Array.from(Dicom.nativeFrame(info, 0)), [1, 2, 3, 4, 5, 6]);
});

// ---------------------------------------------------------------- MONOCHROME1 from the browser
test('invertRgba: what the page does to a browser-decoded MONOCHROME1 JPEG frame (alpha kept)', () => {
  const d = new Uint8ClampedArray([0, 10, 255, 255, 200, 200, 200, 128]);
  assert.deepStrictEqual(Array.from(Dicom.invertRgba(d)), [255, 245, 0, 255, 55, 55, 55, 128]);
  const app = require('node:fs').readFileSync(require('node:path').join(__dirname, '..', 'app.js'), 'utf8');
  assert.match(app, /photometric === 'MONOCHROME1'[\s\S]{0,400}Dicom\.invertRgba/);
});
