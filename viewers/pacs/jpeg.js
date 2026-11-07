// A JPEG decoder for what browsers do not decode in DICOM files: lossless JPEG (process 14,
// all predictors, any precision from 2 to 16 bits) and sequential Huffman DCT with 12-bit
// precision (the extended process, used by CT).  It reads 8-bit DCT too (tests), but the viewer
// leaves those to the browser.  Progressive, arithmetic-coded and hierarchical JPEG are refused.
// The decoded samples come back as numbers, interleaved, before any colour conversion.
// No dependencies.  A classic script (it works from file://) and a CommonJS module for tests.
// (ITU-T T.81: Annex F is the DCT process, Annex H the lossless one.)

(function (root) {
  'use strict';

  // the order of the coefficients in a block
  const ZIGZAG = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
  ];
  // the inverse DCT's basis: COS[u * 8 + x] = C(u) / 2 · cos((2x + 1) u π / 16)
  const COS = new Float64Array(64);
  for (let u = 0; u < 8; u++) for (let x = 0; x < 8; x++) COS[u * 8 + x] = (u ? 1 : Math.SQRT1_2) / 2 * Math.cos(((2 * x + 1) * u * Math.PI) / 16);

  function unsupported(what) {
    const e = new Error(`${what} is not decoded here`);
    e.unsupported = true;
    return e;
  }

  /** A Huffman table from its counts of codes per length (1–16) and its symbols. */
  function huffman(counts, symbols) {
    const maxcode = new Int32Array(17).fill(-1);
    const mincode = new Int32Array(17);
    const valptr = new Int32Array(17);
    let code = 0;
    let k = 0;
    for (let l = 1; l <= 16; l++) {
      valptr[l] = k;
      mincode[l] = code;
      code += counts[l - 1];
      k += counts[l - 1];
      if (counts[l - 1]) maxcode[l] = code - 1;
      code <<= 1;
    }
    return { maxcode, mincode, valptr, symbols };
  }

  /** The bits of entropy-coded data from `pos`: stuffed bytes removed, zeros at a marker. */
  function bitReader(data, pos) {
    let byte = 0;
    let left = 0;
    const r = {
      pos,
      bit() {
        if (!left) {
          byte = 0;
          if (r.pos < data.length) {
            if (data[r.pos] !== 0xff) byte = data[r.pos++];
            else if (data[r.pos + 1] === 0) { byte = 0xff; r.pos += 2; }
            // else a marker: zeros until the next restart
          }
          left = 8;
        }
        left--;
        return (byte >> left) & 1;
      },
      receive(s) {
        let v = 0;
        for (let i = 0; i < s; i++) v = (v << 1) | r.bit();
        return v;
      },
      /** A difference of `s` bits (its sign in the first bit). */
      diff(s) {
        if (!s) return 0;
        const v = r.receive(s);
        return v < 1 << (s - 1) ? v - (1 << s) + 1 : v;
      },
      decode(h) {
        if (!h) throw new Error('JPEG: a scan uses a missing Huffman table');
        let code = r.bit();
        let l = 1;
        while (code > h.maxcode[l]) {
          if (++l > 16) throw new Error('JPEG: bad Huffman code');
          code = (code << 1) | r.bit();
        }
        return h.symbols[h.valptr[l] + code - h.mincode[l]];
      },
      /** Skip to after the next restart marker (RST0–7). */
      restart() {
        left = 0;
        while (r.pos + 1 < data.length && !(data[r.pos] === 0xff && data[r.pos + 1] >= 0xd0 && data[r.pos + 1] <= 0xd7)) r.pos++;
        r.pos += 2;
      },
      /** The position after the scan's data (the next marker). */
      end() {
        let p = r.pos;
        while (p + 1 < data.length && !(data[p] === 0xff && data[p + 1] !== 0 && !(data[p + 1] >= 0xd0 && data[p + 1] <= 0xd7))) p++;
        return p;
      },
    };
    return r;
  }

  /** One scan of lossless JPEG (H.1): each sample is predicted from its neighbours. */
  function losslessScan(rd, frame, comps, predictor, pt, restart) {
    const W = frame.width;
    const H = frame.height;
    if (comps.length > 1 && comps.some((c) => c.h !== 1 || c.v !== 1)) throw unsupported('Subsampled lossless JPEG');
    const init = 1 << (frame.precision - pt - 1);
    for (const c of comps) if (!c.plane) { c.plane = new Int32Array(W * H); c.stride = W; c.h = c.v = 1; }
    let lineY = 0;     // the first line of the restart interval: predicted from the left
    let lineX = 0;
    let n = 0;
    for (let y = 0; y < H; y++) {
      for (let x = 0; x < W; x++, n++) {
        if (restart && n && n % restart === 0) { rd.restart(); lineY = y; lineX = x; }
        const i = y * W + x;
        for (const c of comps) {
          const s = rd.decode(c.dc);
          const d = s === 16 ? 32768 : rd.diff(s);
          const p = c.plane;
          let px;
          if (y === lineY) px = x === lineX ? init : p[i - 1];
          else if (x === 0) px = p[i - W];
          else {
            const a = p[i - 1];
            const b = p[i - W];
            const cc = p[i - W - 1];
            switch (predictor) {
              case 1: px = a; break;
              case 2: px = b; break;
              case 3: px = cc; break;
              case 4: px = a + b - cc; break;
              case 5: px = a + ((b - cc) >> 1); break;
              case 6: px = b + ((a - cc) >> 1); break;
              case 7: px = (a + b) >> 1; break;
              default: px = a;
            }
          }
          p[i] = (px + d) & 0xffff;
        }
      }
    }
  }

  /** The inverse DCT of a block, level-shifted and clamped, into a plane. */
  function idct(block, tmp, plane, at, stride, precision) {
    for (let v = 0; v < 8; v++) {
      for (let x = 0; x < 8; x++) {
        let s = 0;
        for (let u = 0; u < 8; u++) s += block[v * 8 + u] * COS[u * 8 + x];
        tmp[v * 8 + x] = s;
      }
    }
    const shift = 1 << (precision - 1);
    const max = (1 << precision) - 1;
    for (let y = 0; y < 8; y++) {
      for (let x = 0; x < 8; x++) {
        let s = 0;
        for (let v = 0; v < 8; v++) s += COS[v * 8 + y] * tmp[v * 8 + x];
        const g = Math.round(s) + shift;
        plane[at + y * stride + x] = g < 0 ? 0 : g > max ? max : g;
      }
    }
  }

  /** One scan of sequential DCT JPEG (F.2). */
  function dctScan(rd, frame, comps, qt, restart) {
    const { hmax, vmax } = frame;
    const mcusX = Math.ceil(frame.width / (8 * hmax));
    const mcusY = Math.ceil(frame.height / (8 * vmax));
    for (const c of frame.comps) {
      if (c.plane) continue;
      c.stride = mcusX * c.h * 8;
      c.plane = new Int32Array(c.stride * mcusY * c.v * 8);
    }
    const block = new Float64Array(64);
    const tmp = new Float64Array(64);
    const one = (c, row, col) => {
      const q = qt[c.tq];
      if (!q) throw new Error('JPEG: a missing quantization table');
      block.fill(0);
      c.pred += rd.diff(rd.decode(c.dc));
      block[0] = c.pred * q[0];
      for (let k = 1; k < 64;) {
        const rs = rd.decode(c.ac);
        const s = rs & 15;
        const run = rs >> 4;
        if (!s) {
          if (run !== 15) break;   // end of block
          k += 16;
          continue;
        }
        k += run;
        if (k > 63) break;
        const z = ZIGZAG[k];
        block[z] = rd.diff(s) * q[z];
        k++;
      }
      idct(block, tmp, c.plane, row * 8 * c.stride + col * 8, c.stride, frame.precision);
    };
    for (const c of comps) c.pred = 0;
    let n = 0;
    const next = () => {
      if (restart && n && n % restart === 0) {
        rd.restart();
        for (const c of comps) c.pred = 0;
      }
      n++;
    };
    if (comps.length === 1) {
      // a scan of one component: its blocks in raster order, not in MCUs
      const c = comps[0];
      const bw = Math.ceil(Math.ceil((frame.width * c.h) / hmax) / 8);
      const bh = Math.ceil(Math.ceil((frame.height * c.v) / vmax) / 8);
      for (let r = 0; r < bh; r++) for (let col = 0; col < bw; col++) { next(); one(c, r, col); }
      return;
    }
    for (let my = 0; my < mcusY; my++) {
      for (let mx = 0; mx < mcusX; mx++) {
        next();
        for (const c of comps) for (let v = 0; v < c.v; v++) for (let h = 0; h < c.h; h++) one(c, my * c.v + v, mx * c.h + h);
      }
    }
  }

  /**
   * Decode a JPEG stream (Uint8Array): { width, height, components, precision, lossless, data },
   * the samples interleaved (Uint16Array, or Uint8Array for 8 bits).
   */
  function decode(data) {
    if (data[0] !== 0xff || data[1] !== 0xd8) throw new Error('not a JPEG stream');
    const u16 = (p) => (data[p] << 8) | data[p + 1];
    const qt = [];
    const dc = [];
    const ac = [];
    let frame = null;
    let restart = 0;
    let p = 2;
    while (p + 1 < data.length) {
      if (data[p] !== 0xff) { p++; continue; }
      const m = data[p + 1];
      p += 2;
      if (m === 0xff) { p--; continue; }                 // fill bytes
      if (m === 0xd8 || m === 0x01 || (m >= 0xd0 && m <= 0xd7)) continue;
      if (m === 0xd9) break;
      if (p + 2 > data.length) break;
      const end = p + u16(p);
      const s = p + 2;
      if (m === 0xdb) {
        for (let q = s; q < end;) {
          const pq = data[q] >> 4;
          const t = new Int32Array(64);
          qt[data[q] & 15] = t;
          q++;
          for (let i = 0; i < 64; i++, q += pq ? 2 : 1) t[ZIGZAG[i]] = pq ? u16(q) : data[q];
        }
      } else if (m === 0xc4) {
        for (let q = s; q < end;) {
          const tc = data[q] >> 4;
          const th = data[q] & 15;
          const counts = data.subarray(q + 1, q + 17);
          const total = counts.reduce((a, b) => a + b, 0);
          (tc ? ac : dc)[th] = huffman(counts, data.slice(q + 17, q + 17 + total));
          q += 17 + total;
        }
      } else if (m === 0xdd) {
        restart = u16(s);
      } else if (m === 0xc0 || m === 0xc1 || m === 0xc3) {
        frame = { lossless: m === 0xc3, precision: data[s], height: u16(s + 1), width: u16(s + 3), comps: [] };
        for (let i = 0, q = s + 6; i < data[s + 5]; i++, q += 3) frame.comps.push({ id: data[q], h: data[q + 1] >> 4 || 1, v: data[q + 1] & 15 || 1, tq: data[q + 2] });
        frame.hmax = Math.max(...frame.comps.map((c) => c.h));
        frame.vmax = Math.max(...frame.comps.map((c) => c.v));
        if (!frame.width || !frame.height || !frame.comps.length) throw unsupported('A JPEG without its size (DNL)');
        if (frame.width * frame.height * frame.comps.length > 1 << 27) throw new Error('JPEG: too large an image');
        if (frame.precision < 2 || frame.precision > 16 || (!frame.lossless && frame.precision !== 8 && frame.precision !== 12)) throw new Error(`JPEG: precision ${frame.precision}`);
      } else if (m === 0xc2 || m === 0xc6 || m === 0xca || m === 0xce) {
        throw unsupported('Progressive JPEG');
      } else if ((m >= 0xc5 && m <= 0xcf && m !== 0xc8 && m !== 0xcc) || m === 0xc9 || m === 0xcb) {
        throw unsupported(m >= 0xc9 ? 'Arithmetic-coded JPEG' : 'Hierarchical JPEG');
      } else if (m === 0xda) {
        if (!frame) throw new Error('JPEG: a scan before the frame');
        const comps = [];
        for (let i = 0, q = s + 1; i < data[s]; i++, q += 2) {
          const c = frame.comps.find((x) => x.id === data[q]);
          if (!c) throw new Error('JPEG: a scan of an unknown component');
          c.dc = dc[data[q + 1] >> 4];
          c.ac = ac[data[q + 1] & 15];
          comps.push(c);
        }
        const q = s + 1 + 2 * data[s];
        const rd = bitReader(data, end);
        if (frame.lossless) losslessScan(rd, frame, comps, data[q], data[q + 2] & 15, restart);
        else dctScan(rd, frame, comps, qt, restart);
        frame.pt = frame.lossless ? data[q + 2] & 15 : 0;
        p = rd.end();
        continue;
      }
      p = end;
    }
    if (!frame || frame.comps.some((c) => !c.plane)) throw new Error('JPEG: no image data');
    const { width: W, height: H, comps } = frame;
    const nc = comps.length;
    const out = frame.precision > 8 ? new Uint16Array(W * H * nc) : new Uint8Array(W * H * nc);
    comps.forEach((c, ci) => {
      // subsampled components are repeated (nearest neighbour)
      const sx = frame.lossless ? 1 : frame.hmax / c.h;
      const sy = frame.lossless ? 1 : frame.vmax / c.v;
      const pt = frame.pt || 0;
      for (let y = 0; y < H; y++) {
        const row = Math.floor(y / sy) * c.stride;
        for (let x = 0; x < W; x++) out[(y * W + x) * nc + ci] = c.plane[row + Math.floor(x / sx)] << pt;
      }
    });
    return { width: W, height: H, components: nc, precision: frame.precision, lossless: frame.lossless, data: out };
  }

  const api = { decode };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Jpeg = api;
})(typeof self !== 'undefined' ? self : this);
