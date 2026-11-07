// jpeg.js: lossless JPEG (every predictor, point transform, restarts, colour) and 12-bit DCT,
// against streams encoded here (a small encoder of each process), and damaged streams.
const test = require('node:test');
const assert = require('node:assert');
const Jpeg = require('../jpeg.js');

// ---------------------------------------------------------------- a small JPEG encoder

/** Bits into bytes, with 0xFF stuffed. */
function bitWriter() {
  const out = [];
  let acc = 0;
  let n = 0;
  return {
    put(v, len) {
      for (let i = len - 1; i >= 0; i--) {
        acc = (acc << 1) | ((v >> i) & 1);
        if (++n === 8) { out.push(acc); if (acc === 0xff) out.push(0); acc = 0; n = 0; }
      }
    },
    flush() { if (n) { this.put(0x7f, 8 - n); } return out; },
  };
}

/** A Huffman table giving every symbol of `symbols` a code of `len` bits (never all ones). */
function table(symbols, len) {
  const counts = new Array(16).fill(0);
  counts[len - 1] = symbols.length;
  const code = new Map(symbols.map((s, i) => [s, i]));
  return { counts, symbols, code, len };
}
const seg = (marker, body) => [0xff, marker, (body.length + 2) >> 8, (body.length + 2) & 255, ...body];
const dht = (cls, id, t) => seg(0xc4, [(cls << 4) | id, ...t.counts, ...t.symbols]);
const category = (d) => (d === 0 ? 0 : 32 - Math.clz32(Math.abs(d)));
function putDiff(w, t, d) {
  const s = category(d);
  w.put(t.code.get(s), t.len);
  if (s && s < 16) w.put(d < 0 ? d + (1 << s) - 1 : d, s);
}

/** Lossless JPEG of `planes` (arrays of W*H samples), interleaved. */
function encodeLossless(planes, W, H, { precision = 16, predictor = 1, pt = 0, restart = 0 } = {}) {
  const dc = table([...Array(17).keys()], 5);
  const w = bitWriter();
  const out = [0xff, 0xd8, ...dht(0, 0, dc)];
  if (restart) out.push(...seg(0xdd, [restart >> 8, restart & 255]));
  out.push(...seg(0xc3, [precision, H >> 8, H & 255, W >> 8, W & 255, planes.length, ...planes.flatMap((_, i) => [i + 1, 0x11, 0])]));
  out.push(...seg(0xda, [planes.length, ...planes.flatMap((_, i) => [i + 1, 0]), predictor, 0, pt]));
  const init = 1 << (precision - pt - 1);
  const v = planes.map((p) => p.map((x) => x >> pt));
  let lineY = 0;
  let lineX = 0;
  let rst = 0;
  let bytes = [];
  for (let y = 0, n = 0; y < H; y++) {
    for (let x = 0; x < W; x++, n++) {
      if (restart && n && n % restart === 0) {
        bytes.push(...w.flush().splice(0), 0xff, 0xd0 + (rst++ % 8));
        lineY = y; lineX = x;
      }
      for (const p of v) {
        const i = y * W + x;
        let px;
        if (y === lineY) px = x === lineX ? init : p[i - 1];
        else if (x === 0) px = p[i - W];
        else {
          const a = p[i - 1]; const b = p[i - W]; const c = p[i - W - 1];
          px = [0, a, b, c, a + b - c, a + ((b - c) >> 1), b + ((a - c) >> 1), (a + b) >> 1][predictor];
        }
        let d = (p[i] - px) & 0xffff;
        if (d >= 32768) d -= 65536;
        putDiff(w, dc, d === -32768 ? 32768 : d);
      }
    }
  }
  bytes.push(...w.flush());
  return Uint8Array.from([...out, ...bytes, 0xff, 0xd9]);
}

const ZIGZAG = [
  0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
  35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/** Sequential DCT JPEG of one grey plane (quantization 1: only rounding is lost). */
function encodeDct(plane, W, H, precision) {
  const dc = table([...Array(16).keys()], 5);
  const acSymbols = [0x00, 0xf0];
  for (let r = 0; r < 16; r++) for (let s = 1; s <= 15; s++) acSymbols.push((r << 4) | s);
  const ac = table(acSymbols, 8);
  const q = new Array(64).fill(1);
  const out = [0xff, 0xd8, ...seg(0xdb, [0x10, ...q.flatMap((x) => [0, x])]), ...dht(0, 0, dc), ...dht(1, 0, ac)];
  out.push(...seg(0xc1, [precision, H >> 8, H & 255, W >> 8, W & 255, 1, 1, 0x11, 0]));
  out.push(...seg(0xda, [1, 1, 0x00, 0, 63, 0]));
  const w = bitWriter();
  const shift = 1 << (precision - 1);
  let pred = 0;
  for (let by = 0; by < Math.ceil(H / 8); by++) {
    for (let bx = 0; bx < Math.ceil(W / 8); bx++) {
      const f = [];
      for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) f.push(plane[Math.min(H - 1, by * 8 + y) * W + Math.min(W - 1, bx * 8 + x)] - shift);
      const F = new Array(64);
      for (let v = 0; v < 8; v++) {
        for (let u = 0; u < 8; u++) {
          let s = 0;
          for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) s += f[y * 8 + x] * Math.cos(((2 * x + 1) * u * Math.PI) / 16) * Math.cos(((2 * y + 1) * v * Math.PI) / 16);
          F[v * 8 + u] = Math.round(((u ? 1 : Math.SQRT1_2) * (v ? 1 : Math.SQRT1_2) * s) / 4);
        }
      }
      putDiff(w, dc, F[0] - pred);
      pred = F[0];
      let run = 0;
      for (let k = 1; k < 64; k++) {
        const c = F[ZIGZAG[k]];
        if (!c) { run++; continue; }
        while (run > 15) { w.put(ac.code.get(0xf0), 8); run -= 16; }
        const s = category(c);
        w.put(ac.code.get((run << 4) | s), 8);
        w.put(c < 0 ? c + (1 << s) - 1 : c, s);
        run = 0;
      }
      if (run) w.put(ac.code.get(0x00), 8);
    }
  }
  return Uint8Array.from([...out, ...w.flush(), 0xff, 0xd9]);
}

// ---------------------------------------------------------------- the tests

/** A smooth image with edges and noise, of `bits` bits. */
function picture(W, H, bits, seed = 1) {
  let r = seed;
  const rnd = () => ((r = (r * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  const max = (1 << bits) - 1;
  return Array.from({ length: W * H }, (_, i) => {
    const x = i % W; const y = Math.floor(i / W);
    const v = (0.5 + 0.3 * Math.sin(x / 5) * Math.cos(y / 7) + (x > W / 2 ? 0.15 : 0) + 0.02 * rnd()) * max;
    return Math.max(0, Math.min(max, Math.round(v)));
  });
}

test('lossless JPEG: every predictor gives the image back exactly', () => {
  const W = 37; const H = 23;
  const img = picture(W, H, 16);
  for (let p = 1; p <= 7; p++) {
    const j = Jpeg.decode(encodeLossless([img], W, H, { predictor: p }));
    assert.deepStrictEqual([j.width, j.height, j.components, j.precision, j.lossless], [W, H, 1, 16, true]);
    assert.deepStrictEqual(Array.from(j.data), img, `predictor ${p}`);
  }
});

test('lossless JPEG: 12 and 8 bits, a point transform, restarts, three components', () => {
  const W = 20; const H = 12;
  const a = picture(W, H, 12, 2);
  assert.deepStrictEqual(Array.from(Jpeg.decode(encodeLossless([a], W, H, { precision: 12, predictor: 6 })).data), a);
  const b = picture(W, H, 8, 3);
  const j8 = Jpeg.decode(encodeLossless([b], W, H, { precision: 8, predictor: 4 }));
  assert.ok(j8.data instanceof Uint8Array);
  assert.deepStrictEqual(Array.from(j8.data), b);
  // a point transform of 2 drops the two low bits
  const pt = Jpeg.decode(encodeLossless([a], W, H, { precision: 12, predictor: 1, pt: 2 }));
  assert.deepStrictEqual(Array.from(pt.data), a.map((x) => (x >> 2) << 2));
  // a restart every row and a half, then every two rows
  for (const restart of [30, 40]) {
    assert.deepStrictEqual(Array.from(Jpeg.decode(encodeLossless([a], W, H, { precision: 12, predictor: 7, restart })).data), a, `restart ${restart}`);
  }
  const rgb = [picture(W, H, 8, 4), picture(W, H, 8, 5), picture(W, H, 8, 6)];
  const c = Jpeg.decode(encodeLossless(rgb, W, H, { precision: 8, predictor: 1 }));
  assert.strictEqual(c.components, 3);
  assert.deepStrictEqual(Array.from(c.data), Array.from({ length: W * H * 3 }, (_, i) => rgb[i % 3][Math.floor(i / 3)]));
});

test('12-bit DCT JPEG (CT): the image back within rounding', () => {
  const W = 21; const H = 19;   // not a multiple of 8: the blocks at the edges are cropped
  for (const bits of [12, 8]) {
    const img = picture(W, H, bits, 7);
    const j = Jpeg.decode(encodeDct(img, W, H, bits));
    assert.deepStrictEqual([j.width, j.height, j.components, j.precision, j.lossless], [W, H, 1, bits, false]);
    let worst = 0;
    for (let i = 0; i < img.length; i++) worst = Math.max(worst, Math.abs(j.data[i] - img[i]));
    assert.ok(worst <= 2, `${bits} bits: off by ${worst}`);
  }
});

test('damaged and refused streams: an error, never a hang or a crash', () => {
  const W = 16; const H = 16;
  const good = encodeLossless([picture(W, H, 16)], W, H, { predictor: 1 });
  assert.throws(() => Jpeg.decode(new Uint8Array([1, 2, 3])), /not a JPEG/);
  // progressive and arithmetic-coded: said so
  const sof = good.indexOf(0xc3, 2);
  for (const [m, re] of [[0xc2, /Progressive/], [0xc9, /Arithmetic/], [0xc5, /Hierarchical/]]) {
    const b = good.slice();
    b[sof] = m;
    assert.throws(() => Jpeg.decode(b), (e) => e.unsupported && re.test(e.message));
  }
  // truncated anywhere, or with bytes changed: an Error (or an image), nothing else
  for (let cut = 2; cut < good.length; cut += 7) {
    try { Jpeg.decode(good.slice(0, cut)); } catch (e) { assert.ok(e instanceof Error); }
  }
  let r = 9;
  for (let k = 0; k < 200; k++) {
    const b = good.slice();
    for (let n = 0; n < 4; n++) b[(r = (r * 69069 + 1) >>> 0) % b.length] = r >>> 24;
    try { Jpeg.decode(b); } catch (e) { assert.ok(e instanceof Error); }
  }
});
