// A small DICOM reader for the PACS and ECG viewers: Part 10 files (or bare datasets) in
// explicit or implicit VR little endian; native pixel data, RLE, and encapsulated JPEG
// (8-bit baseline, which browsers decode; lossless and 12-bit, which jpeg.js decodes when the
// page has it); structured reports (their measurements and
// text), encapsulated documents (PDF), and waveforms (12-lead and other ECGs).  It reads what
// showing them needs, never shows a patient's identity, and has no dependencies.  A classic
// script (it works from file://) and a CommonJS module for tests.  viewers/ecg/dicom.js is the
// same file (tests check it).

(function (root) {
  'use strict';

  const IMPLICIT = '1.2.840.10008.1.2';
  const EXPLICIT = '1.2.840.10008.1.2.1';
  const JPEG = {
    '1.2.840.10008.1.2.4.50': 'baseline', '1.2.840.10008.1.2.4.51': 'extended',
    '1.2.840.10008.1.2.4.57': 'lossless', '1.2.840.10008.1.2.4.70': 'lossless',
  };
  const RLE = '1.2.840.10008.1.2.5';
  // what other transfer syntaxes are, for the messages
  const NAMES = {
    '1.2.840.10008.1.2.4.57': 'lossless JPEG', '1.2.840.10008.1.2.4.70': 'lossless JPEG', '1.2.840.10008.1.2.4.80': 'JPEG-LS',
    '1.2.840.10008.1.2.4.81': 'JPEG-LS', '1.2.840.10008.1.2.4.90': 'JPEG 2000', '1.2.840.10008.1.2.4.91': 'JPEG 2000',
    '1.2.840.10008.1.2.4.100': 'MPEG-2', '1.2.840.10008.1.2.4.102': 'MPEG-4', '1.2.840.10008.1.2.4.103': 'MPEG-4',
    '1.2.840.10008.1.2.4.110': 'MPEG-4', '1.2.840.10008.1.2.4.111': 'MPEG-4', '1.2.840.10008.1.2.4.112': 'MPEG-4',
    '1.2.840.10008.1.2.4.113': 'MPEG-4', '1.2.840.10008.1.2.4.200': 'HEVC', '1.2.840.10008.1.2.4.201': 'HEVC',
  };
  /** SOP classes: what a file is. */
  const SOP = {
    '1.2.840.10008.5.1.4.1.1.104.1': 'pdf', '1.2.840.10008.5.1.4.1.1.104.2': 'cda',
    '1.2.840.10008.5.1.4.1.1.9.1.1': 'ecg', '1.2.840.10008.5.1.4.1.1.9.1.2': 'ecg', '1.2.840.10008.5.1.4.1.1.9.1.3': 'holter',
    '1.2.840.10008.5.1.4.1.1.9.2.1': 'waveform', '1.2.840.10008.5.1.4.1.1.9.3.1': 'waveform', '1.2.840.10008.5.1.4.1.1.9.4.1': 'waveform',
    '1.2.840.10008.5.1.4.1.1.88.11': 'sr', '1.2.840.10008.5.1.4.1.1.88.22': 'sr', '1.2.840.10008.5.1.4.1.1.88.33': 'sr',
    '1.2.840.10008.5.1.4.1.1.88.34': 'sr', '1.2.840.10008.5.1.4.1.1.88.35': 'sr', '1.2.840.10008.5.1.4.1.1.88.50': 'sr',
    '1.2.840.10008.5.1.4.1.1.88.59': 'sr', '1.2.840.10008.5.1.4.1.1.88.65': 'sr', '1.2.840.10008.5.1.4.1.1.88.67': 'sr',
    '1.2.840.10008.5.1.4.1.1.88.68': 'sr', '1.2.840.10008.5.1.4.1.1.88.69': 'sr', '1.2.840.10008.5.1.4.1.1.88.70': 'sr',
    '1.2.840.10008.5.1.4.1.1.88.71': 'sr', '1.2.840.10008.5.1.4.1.1.88.72': 'sr', '1.2.840.10008.5.1.4.1.1.88.73': 'sr',
    '1.2.840.10008.5.1.4.1.1.88.74': 'sr', '1.2.840.10008.5.1.4.1.1.88.75': 'sr', '1.2.840.10008.5.1.4.1.1.88.76': 'sr',
  };
  const LONG_VR = new Set(['OB', 'OD', 'OF', 'OL', 'OV', 'OW', 'SQ', 'SV', 'UC', 'UN', 'UR', 'UT', 'UV']);

  // VRs of the elements read here, for implicit VR files
  const DICT = {
    '00020010': 'UI', '00080008': 'CS', '00080016': 'UI', '00080060': 'CS', '0008103E': 'LO', '00081030': 'LO',
    '00180050': 'DS', '00180040': 'IS', '00181063': 'DS', '00181065': 'DS', '00186011': 'SQ', '0018602C': 'FD', '0018602E': 'FD',
    '00186024': 'US', '00186026': 'US', '00186018': 'UL', '0018601A': 'UL', '0018601C': 'UL', '0018601E': 'UL',
    '0020000D': 'UI', '0020000E': 'UI', '00200011': 'IS', '00200013': 'IS', '00200032': 'DS', '00200037': 'DS', '00201041': 'DS',
    '00280002': 'US', '00280004': 'CS', '00280006': 'US', '00280008': 'IS', '00280010': 'US', '00280011': 'US',
    '00280030': 'DS', '00280100': 'US', '00280101': 'US', '00280102': 'US', '00280103': 'US',
    '00281050': 'DS', '00281051': 'DS', '00281052': 'DS', '00281053': 'DS', '7FE00010': 'OW',
    '00080100': 'SH', '00080102': 'SH', '00080104': 'LO', '00081032': 'SQ', '00181164': 'DS', '00181088': 'IS', '00182002': 'SH',
    '00080020': 'DA', '00080030': 'TM', '0008002A': 'DT', '00080021': 'DA', '00080031': 'TM', '00080023': 'DA', '00080033': 'TM',
    // structured reports
    '0040A730': 'SQ', '0040A043': 'SQ', '0040A040': 'CS', '0040A010': 'CS', '0040A300': 'SQ', '0040A30A': 'DS', '004008EA': 'SQ',
    '0040A160': 'UT', '0040A168': 'SQ', '0040A120': 'DT', '0040A121': 'DA', '0040A122': 'TM', '0040A491': 'CS', '0040A493': 'CS',
    // encapsulated documents
    '00420010': 'ST', '00420011': 'OB', '00420012': 'LO', '00420013': 'SQ',
    // waveforms
    '54000100': 'SQ', '003A0004': 'CS', '003A0005': 'US', '003A0010': 'UL', '003A001A': 'DS', '003A0020': 'SH',
    '003A0200': 'SQ', '003A0202': 'IS', '003A0203': 'SH', '003A0208': 'SQ', '003A0210': 'DS', '003A0211': 'SQ', '003A0212': 'DS',
    '003A0213': 'DS', '003A0214': 'DS', '54001004': 'US', '54001006': 'CS', '54001010': 'OW',
    '0040B020': 'SQ', '00700006': 'ST', '0040A180': 'US', '0040A0B0': 'US', '0040A132': 'UL', '0040A138': 'DS',
  };

  const hex = (g, e) => (g.toString(16).padStart(4, '0') + e.toString(16).padStart(4, '0')).toUpperCase();

  /** Parse a DICOM file.  Returns { tags: { 'GGGGEEEE': { vr, offset, length, items? } }, transfer, view, ... }. */
  function parse(buffer) {
    const u8 = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
    const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    let pos = 0;
    let transfer = IMPLICIT;
    const tags = {};
    if (u8.length > 132 && view.getUint32(128, false) === 0x4449434d) {     // "DICM"
      pos = 132;
      // the file meta information: explicit VR little endian, group 0002
      while (pos + 8 <= u8.length && view.getUint16(pos, true) === 0x0002) {
        const el = readElement(view, pos, true);
        tags[el.tag] = el;
        pos = el.next;
      }
      if (tags['00020010']) transfer = str(view, tags['00020010']);
    } else if (!(u8.length > 8 && view.getUint16(0, true) === 0x0008)) {
      throw new Error('not a DICOM file');
    }
    if (transfer === '1.2.840.10008.1.2.2') throw new Error('big-endian DICOM files are not read here');
    if (transfer === '1.2.840.10008.1.2.1.99') throw new Error('deflated DICOM files are not read here');
    const explicit = transfer !== IMPLICIT;
    readDataset(view, pos, u8.length, explicit, tags);
    return { tags, transfer, view, explicit };
  }

  function readDataset(view, pos, end, explicit, into) {
    while (pos + 8 <= end) {
      const g = view.getUint16(pos, true);
      const e = view.getUint16(pos + 2, true);
      if (g === 0xfffe && e === 0xe00d) return pos + 8;   // item delimitation
      const el = readElement(view, pos, explicit);
      into[el.tag] = el;
      pos = el.next;
    }
    return pos;
  }

  function readElement(view, pos, explicit) {
    const g = view.getUint16(pos, true);
    const e = view.getUint16(pos + 2, true);
    const tag = hex(g, e);
    let vr;
    let length;
    let p = pos + 4;
    if (explicit && g !== 0xfffe) {
      vr = String.fromCharCode(view.getUint8(p), view.getUint8(p + 1));
      if (LONG_VR.has(vr)) { length = view.getUint32(p + 4, true); p += 8; } else { length = view.getUint16(p + 2, true); p += 4; }
    } else {
      vr = DICT[tag] || 'UN';
      length = view.getUint32(p, true);
      p += 4;
    }
    const el = { tag, vr, offset: p, length };
    if (length === 0xffffffff) {
      if (tag === '7FE00010') {
        // encapsulated pixel data: items until the sequence delimiter
        el.fragments = [];
        let q = p;
        while (q + 8 <= view.byteLength) {
          const ig = view.getUint16(q, true);
          const ie = view.getUint16(q + 2, true);
          const il = view.getUint32(q + 4, true);
          if (ig === 0xfffe && ie === 0xe0dd) { q += 8; break; }
          el.fragments.push({ offset: q + 8, length: il });
          q += 8 + il;
        }
        el.next = q;
      } else {
        // a sequence of undefined length
        el.items = [];
        let q = p;
        while (q + 8 <= view.byteLength) {
          const ig = view.getUint16(q, true);
          const ie = view.getUint16(q + 2, true);
          const il = view.getUint32(q + 4, true);
          if (ig === 0xfffe && ie === 0xe0dd) { q += 8; break; }
          if (ig === 0xfffe && ie === 0xe000) {
            const item = {};
            q = il === 0xffffffff ? readDataset(view, q + 8, view.byteLength, explicit, item) : (readDataset(view, q + 8, q + 8 + il, explicit, item), q + 8 + il);
            el.items.push(item);
          } else {
            break;
          }
        }
        el.next = q;
      }
    } else {
      if (vr === 'SQ') {
        el.items = [];
        let q = p;
        while (q + 8 <= p + length) {
          const il = view.getUint32(q + 4, true);
          const item = {};
          if (il === 0xffffffff) q = readDataset(view, q + 8, p + length, explicit, item);
          else { readDataset(view, q + 8, q + 8 + il, explicit, item); q += 8 + il; }
          el.items.push(item);
        }
      }
      el.next = p + length;
    }
    return el;
  }

  function str(view, el) {
    if (!el) return '';
    let s = '';
    for (let i = 0; i < el.length; i++) s += String.fromCharCode(view.getUint8(el.offset + i));
    return s.replace(/[\0 ]+$/, '').trim();
  }

  /** A value: numbers for binary VRs, numbers for DS/IS, else text; multiple values as arrays. */
  function value(view, el) {
    if (!el) return undefined;
    switch (el.vr) {
      case 'US': return el.length >= 2 ? view.getUint16(el.offset, true) : undefined;
      case 'SS': return el.length >= 2 ? view.getInt16(el.offset, true) : undefined;
      case 'UL': return el.length >= 4 ? view.getUint32(el.offset, true) : undefined;
      case 'SL': return el.length >= 4 ? view.getInt32(el.offset, true) : undefined;
      case 'FD': return el.length >= 8 ? view.getFloat64(el.offset, true) : undefined;
      case 'FL': return el.length >= 4 ? view.getFloat32(el.offset, true) : undefined;
      case 'DS': case 'IS': {
        const v = str(view, el).split('\\').map(Number);
        return v.length > 1 ? v : v[0];
      }
      default: return str(view, el);
    }
  }

  /**
   * What showing the image needs: size, pixel format, frames, rescale, window, timing, geometry
   * and a few descriptions (no identity).
   */
  function image(buffer) {
    const d = parse(buffer);
    const { tags, view } = d;
    const v = (t) => value(view, tags[t]);
    const first = (x) => (Array.isArray(x) ? x[0] : x);
    const px = tags['7FE00010'];
    const info = {
      transfer: d.transfer,
      jpeg: JPEG[d.transfer] || null,
      modality: v('00080060') || '',
      series: v('0020000E') || '', study: v('0020000D') || '',
      seriesDescription: v('0008103E') || '', seriesNumber: v('00200011') ?? null,
      instance: v('00200013') ?? null,
      rows: v('00280010'), cols: v('00280011'),
      samples: v('00280002') || 1, photometric: v('00280004') || 'MONOCHROME2',
      bitsAllocated: v('00280100') || 8, bitsStored: v('00280101') || v('00280100') || 8,
      signed: v('00280103') === 1, planar: v('00280006') || 0,
      frames: Math.max(1, Math.floor(Number(v('00280008')) || 1)),
      slope: first(v('00281053')) ?? 1, intercept: first(v('00281052')) ?? 0,
      center: first(v('00281050')) ?? null, width: first(v('00281051')) ?? null,
      spacing: v('00280030') || null, thickness: v('00180050') ?? null,
      // projection images (XA, RF, DX): the spacing at the detector
      imagerSpacing: v('00181164') || null,
      heartRate: v('00181088') ?? null, frameTimes: v('00181065') ?? null,
      position: v('00200032') || null, orientation: v('00200037') || null, location: v('00201041') ?? null,
      // cine: frame time (ms) or rate (frames per second)
      frameTime: v('00181063') ?? null, cineRate: v('00180040') ?? null,
      regions: (tags['00186011']?.items || []).map((it) => ({
        dx: value(view, it['0018602C']), dy: value(view, it['0018602E']), unitsX: value(view, it['00186024']),
        x0: value(view, it['00186018']), y0: value(view, it['0018601A']), x1: value(view, it['0018601C']), y1: value(view, it['0018601E']),
      })),
    };
    if (!px) throw new Error('no image in this DICOM file');
    if (!info.rows || !info.cols) throw new Error('DICOM image without size');
    if (px.fragments) {
      if (d.transfer === RLE) info.rle = true;
      // lossless and 12-bit JPEG: decoded here (jpeg.js), the others by the browser
      else if (info.jpeg && (info.jpeg === 'lossless' || info.bitsStored > 8)) info.decoded = true;
      if ((!info.jpeg && !info.rle) || (info.decoded && !decoder())) {
        const err = new Error(`${NAMES[d.transfer] || `compressed DICOM (${d.transfer})`}: browsers do not decode it`);
        err.transfer = d.transfer;
        err.unsupported = true;
        throw err;
      }
      // no more frames than the pixel data holds (the header's count is the file's word)
      info.frames = Math.min(info.frames, Math.max(1, px.fragments.length - 1));
      info.encapsulated = frameFragments(px.fragments, info.frames, view);
    } else {
      if (d.transfer !== IMPLICIT && d.transfer !== EXPLICIT && !/^1\.2\.840\.10008\.1\.2(\.1)?$/.test(d.transfer)) {
        throw new Error(`DICOM transfer syntax ${d.transfer} is not read here`);
      }
      info.native = { offset: px.offset, length: px.length };
      const frameBytes = info.rows * info.cols * info.samples * info.bitsAllocated / 8;
      info.frames = Math.min(info.frames, Math.max(1, Math.floor(px.length / frameBytes) || 0));
    }
    info._view = view;
    return info;
  }

  /** The bytes of each frame of encapsulated pixel data (fragments after the offset table). */
  function frameFragments(fragments, frames, view) {
    const [bot, ...rest] = fragments;
    const offsets = [];
    for (let i = 0; i + 4 <= bot.length; i += 4) offsets.push(view.getUint32(bot.offset + i, true));
    if (rest.length === frames || offsets.length < 2) {
      if (rest.length === frames) return rest.map((f) => [f]);
      return [rest];   // one frame in several fragments
    }
    // offsets into the fragment items (from the first item after the table)
    const base = rest[0].offset - 8;
    const out = offsets.map(() => []);
    for (const f of rest) {
      const rel = f.offset - 8 - base;
      let k = 0;
      while (k + 1 < offsets.length && offsets[k + 1] <= rel) k++;
      out[k].push(f);
    }
    return out;
  }

  /** A JPEG frame's bytes (encapsulated pixel data). */
  function jpegFrame(info, i) {
    const parts = info.encapsulated[i] || [];
    const total = parts.reduce((a, f) => a + f.length, 0);
    const out = new Uint8Array(total);
    let o = 0;
    for (const f of parts) {
      out.set(new Uint8Array(info._view.buffer, info._view.byteOffset + f.offset, f.length), o);
      o += f.length;
    }
    return out;
  }

  /** The JPEG decoder (jpeg.js), when the page has it. */
  function decoder() {
    if (root.Jpeg) return root.Jpeg;
    try { return typeof require === 'function' ? require('./jpeg.js') : null; } catch { return null; }
  }

  /** A JPEG frame that jpeg.js decodes: its samples, interleaved, checked against the header. */
  function decodedFrame(info, i) {
    const j = decoder().decode(jpegFrame(info, i));
    if (j.width !== info.cols || j.height !== info.rows || j.components !== info.samples) {
      throw new Error(`a JPEG frame of ${j.width}×${j.height}×${j.components}, not the header's ${info.cols}×${info.rows}×${info.samples}`);
    }
    return j;
  }

  /** An RLE frame decoded into the native layout (little endian, samples interleaved). */
  function rleFrame(info, i) {
    const src = jpegFrame(info, i);       // the frame's bytes, fragments joined
    const dv = new DataView(src.buffer, src.byteOffset, src.byteLength);
    const nseg = dv.getUint32(0, true);
    const offsets = [];
    for (let k = 0; k < nseg; k++) offsets.push(dv.getUint32(4 + 4 * k, true));
    const plane = info.rows * info.cols;
    const bytes = info.bitsAllocated / 8;
    const out = new Uint8Array(plane * info.samples * bytes);
    for (let seg = 0; seg < nseg; seg++) {
      // segments: for each sample, its bytes from the most significant
      const sample = Math.floor(seg / bytes);
      const byte = bytes - 1 - (seg % bytes);
      let p = offsets[seg];
      const end = seg + 1 < nseg ? offsets[seg + 1] : src.length;
      let k = 0;
      const put = (b) => { if (k < plane) out[(k * info.samples + sample) * bytes + byte] = b; k++; };
      while (p < end && k < plane) {
        const n = (src[p] << 24) >> 24;
        p++;
        if (n >= 0) { for (let j = 0; j <= n; j++) put(src[p + j]); p += n + 1; }
        else if (n !== -128) { const b = src[p]; for (let j = 0; j < 1 - n; j++) put(b); p++; }
      }
    }
    return out;
  }

  /** A native frame's stored values as numbers (grey: rows*cols; colour: rows*cols*3, interleaved). */
  function nativeFrame(info, i) {
    const n = info.rows * info.cols * info.samples;
    const bytes = info.bitsAllocated / 8;
    let start;
    let v;
    let planar = info.planar;
    if (info.decoded) return fromSamples(info, decodedFrame(info, i));
    if (info.rle) {
      const raw = rleFrame(info, i);
      v = new DataView(raw.buffer);
      start = 0;
      planar = 0;
    } else {
      start = info.native.offset + i * n * bytes;
      v = info._view;
    }
    const out = info.samples === 1 ? new Float32Array(n) : new Uint8ClampedArray(n);
    if (info.samples === 1) {
      const mask = info.bitsStored < 32 ? (1 << info.bitsStored) - 1 : 0xffffffff;
      for (let k = 0; k < n; k++) {
        let x;
        if (bytes === 1) x = v.getUint8(start + k);
        else if (bytes === 2) x = v.getUint16(start + 2 * k, true);
        else x = v.getUint32(start + 4 * k, true);
        if (info.bitsStored < info.bitsAllocated) x &= mask;
        if (info.signed && x & (1 << (info.bitsStored - 1))) x -= 1 << info.bitsStored;
        out[k] = x * info.slope + info.intercept;
      }
    } else {
      // RGB, 8 bits; planar configuration 1 stores the planes one after the other
      const plane = info.rows * info.cols;
      for (let k = 0; k < plane; k++) {
        for (let c = 0; c < 3; c++) out[3 * k + c] = v.getUint8(start + (planar ? c * plane + k : 3 * k + c));
      }
    }
    return out;
  }

  /** Decoded JPEG samples as nativeFrame's values (grey: rescaled; colour: 8-bit RGB). */
  function fromSamples(info, j) {
    const src = j.data;
    if (info.samples === 1) {
      const out = new Float32Array(src.length);
      const bits = info.bitsStored;
      const mask = bits < 32 ? (1 << bits) - 1 : 0xffffffff;
      for (let k = 0; k < src.length; k++) {
        let x = src[k] & mask;
        if (info.signed && x & (1 << (bits - 1))) x -= 1 << bits;
        out[k] = x * info.slope + info.intercept;
      }
      return out;
    }
    const out = new Uint8ClampedArray(src.length);
    const down = Math.max(0, j.precision - 8);
    const ybr = !j.lossless && /^YBR/.test(info.photometric);
    for (let k = 0; k < src.length; k += 3) {
      const a = src[k] >> down;
      const b = src[k + 1] >> down;
      const c = src[k + 2] >> down;
      if (ybr) {
        out[k] = a + 1.402 * (c - 128);
        out[k + 1] = a - 0.344136 * (b - 128) - 0.714136 * (c - 128);
        out[k + 2] = a + 1.772 * (b - 128);
      } else { out[k] = a; out[k + 1] = b; out[k + 2] = c; }
    }
    return out;
  }

  /** A window (centre, width) that shows a grey frame: the file's; for 8-bit images (display-ready:
   * ultrasound, angiography), their whole range; else the frame's 1st–99th percentile. */
  function autoWindow(info, values) {
    if (info.center !== null && info.width !== null && info.width > 0) return [info.center, info.width];
    if (info.bitsStored <= 8 && !info.signed && info.slope === 1 && info.intercept === 0) {
      const n = 2 ** info.bitsStored;
      return [n / 2, n];
    }
    const s = Float32Array.from(values).sort();
    const lo = s[Math.floor(s.length * 0.01)];
    const hi = s[Math.floor(s.length * 0.99)];
    return [(lo + hi) / 2, Math.max(1, hi - lo)];
  }

  /** RGBA pixels of a grey frame through a window (MONOCHROME1 is inverted). */
  function toRgba(info, values, center, width, out) {
    const lo = center - width / 2;
    const k = 255 / width;
    const inv = info.photometric === 'MONOCHROME1';
    for (let i = 0; i < values.length; i++) {
      let g = (values[i] - lo) * k;
      g = g < 0 ? 0 : g > 255 ? 255 : g;
      if (inv) g = 255 - g;
      out[4 * i] = out[4 * i + 1] = out[4 * i + 2] = g;
      out[4 * i + 3] = 255;
    }
    return out;
  }

  /** Frames per second of a cine (default 30). */
  function fps(info) {
    if (info.frameTime > 0) return 1000 / info.frameTime;
    if (info.cineRate > 0) return info.cineRate;
    return 30;
  }

  // ---------------------------------------------------------------- other objects
  const code = (view, items) => {
    const it = items?.[0];
    return it ? { value: value(view, it['00080100']) || '', scheme: value(view, it['00080102']) || '', meaning: value(view, it['00080104']) || '' } : null;
  };

  /** What a file is: { kind: 'image' | 'pdf' | 'cda' | 'sr' | 'ecg' | 'holter' | 'waveform', sop, modality, study, series, title }. */
  function kind(buffer) {
    const d = parse(buffer);
    const { tags, view } = d;
    const sop = value(view, tags['00080016']) || value(view, tags['00020002']) || '';
    let k = SOP[sop];
    if (!k) k = tags['7FE00010'] ? 'image' : tags['0040A730'] ? 'sr' : tags['54000100'] ? 'waveform' : tags['00420011'] ? 'pdf' : 'other';
    return {
      kind: k, sop, modality: value(view, tags['00080060']) || '',
      study: value(view, tags['0020000D']) || '', series: value(view, tags['0020000E']) || '',
      title: value(view, tags['0008103E']) || value(view, tags['00420010']) || code(view, tags['0040A043']?.items)?.meaning || '',
      _d: d,
    };
  }

  /**
   * An encapsulated document: { mime, bytes, title, pdf }: the type the file states, and whether
   * it is a PDF (only a PDF is shown, and always as application/pdf: the stated type is the
   * file's word, and as text/html the document would run as a page of the viewer's own).
   */
  function documentOf(buffer) {
    const { tags, view } = parse(buffer);
    const el = tags['00420011'];
    if (!el) throw new Error('no document in this DICOM file');
    let len = el.length;
    const bytes = new Uint8Array(view.buffer, view.byteOffset + el.offset, len);
    // a PDF padded to an even length ends after its last %%EOF
    if (bytes[len - 1] === 0) len--;
    const mime = String(value(view, tags['00420012']) || 'application/pdf').trim();
    return { mime, pdf: /^application\/pdf$/i.test(mime), bytes: bytes.subarray(0, len), title: value(view, tags['00420010']) || '' };
  }

  // A report's items that say who the patient is (the subject context, TID 1006-1009: name, ID,
  // UID, birth date, the mother of a fetus; the accession number): never shown, whatever their type.
  const IDENTITY = new Set(['121022', '121028', '121029', '121030', '121031', '121036']);
  const identifying = (c) => !!c && ((c.scheme === 'DCM' && IDENTITY.has(c.value))
    || (/\b(subject|patient|mother)\b/i.test(c.meaning) && /\b(name|id|uid|identifier|birth)\b/i.test(c.meaning))
    || /\b(accession|medical record)\b/i.test(c.meaning));

  /**
   * A structured report: its title and content as a list of
   * { depth, relation, type, name, value, unit, code } (NUM, TEXT, CODE, DATE, CONTAINER …).
   * Identity (the observer's or the patient's names: PNAME items; the subject context) is left out.
   */
  function report(buffer) {
    const { tags, view } = parse(buffer);
    const out = [];
    const walk = (items, depth) => {
      for (const it of items || []) {
        const type = value(view, it['0040A040']) || '';
        const concept = code(view, it['0040A043']?.items);
        if (identifying(concept)) continue;
        const name = concept?.meaning || '';
        const row = { depth, relation: value(view, it['0040A010']) || '', type, name };
        if (type === 'NUM') {
          const mv = it['0040A300']?.items?.[0];
          if (mv) {
            const num = value(view, mv['0040A30A']);
            row.value = Array.isArray(num) ? num[0] : num;
            const u = code(view, mv['004008EA']?.items);
            row.unit = u ? (u.value && u.scheme === 'UCUM' ? u.value : u.meaning || u.value) : '';
          }
        } else if (type === 'TEXT') row.value = value(view, it['0040A160']);
        else if (type === 'CODE') row.value = code(view, it['0040A168']?.items)?.meaning || '';
        else if (type === 'DATE' || type === 'TIME' || type === 'DATETIME') row.value = value(view, it['0040A121']) || value(view, it['0040A122']) || value(view, it['0040A120']);
        if (type !== 'PNAME' && type !== 'UIDREF' && type !== 'IMAGE' && type !== 'COMPOSITE') out.push(row);
        walk(it['0040A730']?.items, depth + 1);
      }
    };
    walk(tags['0040A730']?.items, 0);
    return { title: code(view, tags['0040A043']?.items)?.meaning || 'Report', items: out };
  }

  /** The measurements of a report: [{ name, value, unit }] (the NUM items, named by their parent when generic). */
  function measurements(rep) {
    return rep.items.filter((r) => r.type === 'NUM' && Number.isFinite(r.value)).map((r) => ({ name: r.name, value: r.value, unit: r.unit || '' }));
  }

  /**
   * The waveforms of a file: [{ label, fs, channels: [{ name, units, samples (Float64Array, in units) }] }],
   * and its annotations: { measurements: [{ name, value, unit }], text: [line] }.
   */
  function waveforms(buffer) {
    const { tags, view } = parse(buffer);
    const groups = [];
    for (const it of tags['54000100']?.items || []) {
      const nch = value(view, it['003A0005']) || 0;
      const ns = value(view, it['003A0010']) || 0;
      const fs = Number(value(view, it['003A001A'])) || 0;
      const bits = value(view, it['54001004']) || 16;
      const interp = value(view, it['54001006']) || 'SS';
      const data = it['54001010'];
      if (!nch || !ns || !data) continue;
      const defs = (it['003A0200']?.items || []).map((c) => {
        const src = code(view, c['003A0208']?.items);
        const u = code(view, c['003A0211']?.items);
        const num = (t, d) => { const x = value(view, c[t]); return Number.isFinite(+x) && x !== '' && x !== undefined ? +x : d; };
        return {
          name: (src?.meaning || value(view, c['003A0203']) || '').replace(/^Lead\s+/i, '').trim(),
          units: u?.value || u?.meaning || '',
          k: num('003A0210', 1) * num('003A0212', 1),
          base: num('003A0213', 0),
        };
      });
      const size = bits / 8;
      const read = interp === 'UB' ? (o) => view.getUint8(o) : interp === 'SB' ? (o) => view.getInt8(o) : interp === 'US' ? (o) => view.getUint16(o, true)
        : interp === 'SL' ? (o) => view.getInt32(o, true) : interp === 'UL' ? (o) => view.getUint32(o, true) : (o) => view.getInt16(o, true);
      const n = Math.min(ns, Math.floor(data.length / (size * nch)));
      const channels = [];
      for (let ch = 0; ch < nch; ch++) {
        const d = defs[ch] || { name: `Channel ${ch + 1}`, units: '', k: 1, base: 0 };
        const samples = new Float64Array(n);
        for (let i = 0; i < n; i++) samples[i] = read(data.offset + (i * nch + ch) * size) * d.k + d.base;
        channels.push({ name: d.name || `Channel ${ch + 1}`, units: d.units, samples });
      }
      groups.push({ label: value(view, it['003A0020']) || '', fs, channels });
    }
    const ms = [];
    const text = [];
    for (const a of tags['0040B020']?.items || []) {
      const t = value(view, a['00700006']);
      if (t) { text.push(t); continue; }
      const name = code(view, a['0040A043']?.items)?.meaning || '';
      const num = value(view, a['0040A30A']);
      const u = code(view, a['004008EA']?.items);
      if (name && num !== undefined && num !== '') ms.push({ name, value: Array.isArray(num) ? num[0] : +num, unit: u?.value || u?.meaning || '' });
    }
    const date = value(view, tags['0008002A']) || `${value(view, tags['00080023']) || value(view, tags['00080020']) || ''}${value(view, tags['00080033']) || value(view, tags['00080030']) || ''}`;
    // YYYYMMDDHHMMSS → YYYY-MM-DD HH:MM:SS
    const m = /^(\d{4})(\d{2})(\d{2})(\d{2})?(\d{2})?(\d{2})?/.exec(date || '');
    const acquired = m ? `${m[1]}-${m[2]}-${m[3]}${m[4] ? ` ${m[4]}:${m[5] || '00'}${m[6] ? `:${m[6]}` : ''}` : ''}` : '';
    return { groups, annotations: { measurements: ms, text }, acquired, modality: value(view, tags['00080060']) || '' };
  }

  const api = { parse, image, nativeFrame, jpegFrame, autoWindow, toRgba, fps, value, kind, documentOf, report, measurements, waveforms, NAMES };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Dicom = api;
})(typeof self !== 'undefined' ? self : this);
