// WFDB records for the ECG viewer: the header (.hea) and its signal files, single- or
// multi-segment (docs/wfdb-collection.md); formats 8, 16, 24, 32, 61, 80, 160 and 212.  A signal
// file may be a MATLAB file (.mat) that matches its header ("MATLAB signal files" there).
// No dependencies but ecg.js (the record) and readers.js (the reader is added there); a classic
// script (it works from file://), and a CommonJS module for tests.

(function (root) {
  'use strict';
  const node = typeof module !== 'undefined' && module.exports;
  const ECG = node ? require('./ecg.js') : root.ECG;
  const Readers = node ? require('./readers.js') : root.Readers;
  const { checked, metaFromLines, toMv } = ECG;

  // ---------------------------------------------------------------- WFDB header
  /** The fields of a WFDB header (.hea) text: the record line and one entry per signal. */
  function parseHeader(text) {
    const all = text.split(/\r?\n/);
    const comments = all.filter((l) => /^\s*#/.test(l)).map((l) => l.replace(/^\s*#\s?/, '').trim()).filter(Boolean);
    const lines = all.map((l) => l.replace(/#.*$/, '').trim()).filter(Boolean);
    if (!lines.length) throw new Error('empty WFDB header');
    const rec = lines[0].split(/\s+/);
    const [nameSeg] = rec;
    if (nameSeg.includes('/')) throw new Error('a multi-segment WFDB record: read it with readWfdbAny');
    const nsig = parseInt(rec[1], 10);
    if (!(nsig >= 0)) throw new Error('not a WFDB header');
    // sampling frequency: "500", "500/1", "500(0)"
    const fs = rec[2] ? parseFloat(rec[2].split(/[/(]/)[0]) : 250;
    const nsamp = rec[3] ? parseInt(rec[3], 10) : null;
    const start = parseStart(rec[4], rec[5]);
    const signals = [];
    for (let i = 0; i < nsig; i++) {
      const f = (lines[1 + i] || '').split(/\s+/);
      if (f.length < 2) throw new Error(`WFDB header: signal ${i + 1} is missing`);
      const fmtField = f[1];
      const fmt = parseInt(fmtField, 10);
      const spf = /x(\d+)/.exec(fmtField);
      const skew = /:(\d+)/.exec(fmtField);
      const off = /\+(\d+)/.exec(fmtField);
      // gain(baseline)/units
      let gain = 200;
      let baseline = null;
      let units = 'mV';
      if (f[2]) {
        const m = /^([-+0-9.eE]+)(?:\(([-+0-9]+)\))?(?:\/(\S+))?$/.exec(f[2]);
        if (m) {
          gain = parseFloat(m[1]) || 200;
          if (m[2] !== undefined) baseline = parseInt(m[2], 10);
          if (m[3]) units = m[3];
        }
      }
      const adczero = f[4] !== undefined ? parseInt(f[4], 10) || 0 : 0;
      signals.push({
        file: f[0], fmt, spf: spf ? parseInt(spf[1], 10) : 1, skew: skew ? parseInt(skew[1], 10) : 0,
        offset: off ? parseInt(off[1], 10) : 0, gain, baseline: baseline ?? adczero, units,
        initval: f[5] !== undefined ? parseInt(f[5], 10) || 0 : 0,
        desc: f.slice(8).join(' ') || `signal ${i + 1}`,
      });
    }
    return { name: nameSeg, nsig, fs, nsamp, start, signals, comments };
  }


  function parseStart(time, date) {
    if (!time) return null;
    const t = /^(\d{1,2}):(\d{2})(?::(\d{2}(?:\.\d+)?))?$/.exec(time);
    if (!t) return null;
    const d = date && /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(date);
    const [y, mo, da] = d ? [+d[3], +d[2] - 1, +d[1]] : [1970, 0, 1];
    return Date.UTC(y, mo, da, +t[1], +t[2], 0) / 1000 + (t[3] ? parseFloat(t[3]) : 0);
  }


  // ---------------------------------------------------------------- WFDB samples
  /** Decode the samples of the signals stored in one signal file (all in the same format). */
  function decodeFile(buf, sigs, maxFrames) {
    const dv = new DataView(buf.buffer || buf, buf.byteOffset || 0, buf.byteLength);
    const fmt = sigs[0].fmt;
    if (sigs.some((s) => s.fmt !== fmt)) throw new Error('signals of one file in different formats');
    const per = sigs.reduce((a, s) => a + s.spf, 0);     // samples per frame in this file
    const start = sigs[0].offset;
    const out = sigs.map(() => []);
    const bits = { 8: 8, 16: 16, 61: 16, 24: 24, 32: 32, 80: 8, 160: 16, 212: 12 }[fmt];
    if (!bits) throw new Error(`WFDB format ${fmt} is not read here`);
    const total = Math.floor(((dv.byteLength - start) * 8) / bits);
    const frames = Math.min(Math.floor(total / per), maxFrames ?? Infinity);
    const at = (k) => readSample(dv, start, fmt, k);
    const prev = sigs.map((s) => s.initval);
    let k = 0;
    for (let fr = 0; fr < frames; fr++) {
      for (let j = 0; j < sigs.length; j++) {
        for (let r = 0; r < sigs[j].spf; r++) {
          let v = at(k++);
          if (fmt === 8) { prev[j] += v; v = prev[j]; }
          out[j].push(v);
        }
      }
    }
    return out;
  }

  function readSample(dv, start, fmt, k) {
    switch (fmt) {
      case 16: { const v = dv.getInt16(start + 2 * k, true); return v === -32768 ? NaN : v; }
      case 61: { const v = dv.getInt16(start + 2 * k, false); return v === -32768 ? NaN : v; }
      case 80: return dv.getUint8(start + k) - 128;
      case 8: return dv.getInt8(start + k);
      case 160: return dv.getUint16(start + 2 * k, true) - 32768;
      case 24: {
        const o = start + 3 * k;
        let v = dv.getUint8(o) | (dv.getUint8(o + 1) << 8) | (dv.getUint8(o + 2) << 16);
        if (v & 0x800000) v -= 0x1000000;
        return v === -8388608 ? NaN : v;
      }
      case 32: { const v = dv.getInt32(start + 4 * k, true); return v === -2147483648 ? NaN : v; }
      case 212: {
        // two 12-bit samples in three bytes
        const o = start + 3 * (k >> 1);
        const b0 = dv.getUint8(o);
        const b1 = dv.getUint8(o + 1);
        let v = k & 1 ? (((b1 & 0xf0) << 4) | dv.getUint8(o + 2)) : (((b1 & 0x0f) << 8) | b0);
        if (v & 0x800) v -= 0x1000;
        return v === -2048 ? NaN : v;
      }
      default: throw new Error(`WFDB format ${fmt}`);
    }
  }

  // ---------------------------------------------------------------- MAT signal files
  // As the readers of Medorama and VitalStream check them (libs/engine/src/sources/wfdb_files.rs),
  // with the same messages: a MAT file is read as WFDB reads a signal file, its matrix as frames
  // from the header's byte offset, so one of another layout would be read as noise.

  // element: [bytes, text]
  const MAT_ELEMS = {
    i8: [1, '8-bit integers'], u8: [1, '8-bit unsigned integers'], i16: [2, '16-bit integers'],
    u16: [2, '16-bit unsigned integers'], i32: [4, '32-bit integers'], u32: [4, '32-bit unsigned integers'],
    i64: [8, '64-bit integers'], u64: [8, '64-bit unsigned integers'], f32: [4, 'single-precision numbers'],
    f64: [8, 'double-precision numbers'],
  };
  // the elements a WFDB storage format reads from a MAT file, and their byte order (null: either,
  // for one byte); a format not here is held by no MAT matrix
  const MAT_OF_FORMAT = { 80: ['u8', null], 16: ['i16', false], 61: ['i16', true], 160: ['u16', false], 32: ['i32', false] };
  const MAT4_ELEMS = ['f64', 'f32', 'i32', 'i16', 'u16', 'u8'];
  const MAT5_ELEMS = { 1: 'i8', 2: 'u8', 3: 'i16', 4: 'u16', 5: 'i32', 6: 'u32', 7: 'f32', 9: 'f64', 12: 'i64', 13: 'u64' };
  const notMat = (why) => Object.assign(new Error(why), { notMat: true });

  /** The layout of the first matrix of a MAT file (level 4, or level 5 uncompressed):
   * { elem, bigEndian, rows, cols, start, bytes (level 5 only) }; an error saying why it is not
   * one WFDB can read. */
  function matLayout(b) {
    const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
    // level 5: "If any of the first four bytes is zero, it is a level 4 file" (the MAT-file format)
    if (b.length >= 4 && b[0] && b[1] && b[2] && b[3]) return mat5Layout(b, dv);
    if (b.length < 20) throw notMat('it is too short for a MAT file');
    const word = (i, be) => dv.getUint32(i, !be);
    // the type MOPT: M the byte order (0 little-endian, 1 big-endian), O 0, P the element, T the kind
    const valid = (t, m) => t < 5000 && Math.floor(t / 1000) === m && Math.floor(t / 100) % 10 === 0 && Math.floor(t / 10) % 10 <= 5;
    let be;
    if (valid(word(0, false), 0)) be = false;
    else if (valid(word(0, true), 1)) be = true;
    else throw notMat('it is not a MAT file (level 4 or 5) that WFDB reads');
    const t = word(0, be);
    if (t % 10) throw notMat(t % 10 === 1 ? 'it holds text, not a numeric matrix' : 'it holds a sparse matrix');
    if (word(12, be)) throw notMat('it holds a complex matrix');
    return { elem: MAT4_ELEMS[Math.floor(t / 10) % 10], bigEndian: be, rows: word(4, be), cols: word(8, be), start: 20 + word(16, be), bytes: null };
  }

  function mat5Layout(b, dv) {
    const order = b.length >= 128 ? String.fromCharCode(b[126], b[127]) : '';
    if (order !== 'IM' && order !== 'MI') throw notMat('it is not a MAT file (level 4 or 5) that WFDB reads');
    const be = order === 'MI';
    const version = dv.getUint16(124, !be);
    if (version === 0x0200) throw notMat("it is a MATLAB 7.3 (HDF5) file: save it with save(..., '-v6') or '-v4'");
    if (version !== 0x0100) throw notMat(`it is a MAT file of an unknown version (0x${version.toString(16).padStart(4, '0')})`);
    const u32 = (i) => {
      if (i + 4 > b.length) throw notMat('it ends inside its first matrix');
      return dv.getUint32(i, !be);
    };
    // a data element: its type, bytes, where its data start and where the next element does; a
    // small element packs type and bytes in 4 bytes, with its data in the next 4
    const tag = (i) => {
      const w = u32(i);
      if (w >>> 16) return { type: w & 0xffff, n: w >>> 16, data: i + 4, next: i + 8 };
      const n = u32(i + 4);
      return { type: w, n, data: i + 8, next: i + 8 + Math.ceil(n / 8) * 8 };
    };
    const m = tag(128);
    if (m.type === 15) throw notMat("it is compressed (MATLAB saves so by default): save it with save(..., '-v6') or '-v4'");
    if (m.type !== 14) throw notMat('its first variable is not a matrix');
    // array flags: the class in the low byte, the complex flag (0x08) in the next
    const fl = tag(m.data);
    const flags = u32(fl.data);
    if ((flags >>> 8) & 0x08) throw notMat('it holds a complex matrix');
    if ((flags & 0xff) === 5) throw notMat('it holds a sparse matrix');
    if ((flags & 0xff) < 6 || (flags & 0xff) > 15) throw notMat('its first variable is not a numeric matrix');
    // dimensions: two 32-bit integers
    const dims = tag(fl.next);
    if (dims.n !== 8) throw notMat(`its first variable has ${Math.floor(dims.n / 4)} dimensions, not 2`);
    const name = tag(dims.next);
    const real = tag(name.next);
    if (!MAT5_ELEMS[real.type]) throw notMat(`its elements are of an unknown type (${real.type})`);
    return { elem: MAT5_ELEMS[real.type], bigEndian: be, rows: u32(dims.data), cols: u32(dims.data + 4), start: real.data, bytes: real.n };
  }

  /** Why a MAT signal file does not match the header that names it, or null: the matrix must hold
   * the elements of the header's format, one row per signal of the file (one per sample of a
   * frame), one column per frame, starting at the header's byte offset.  sigs: the header's
   * signals in this file; nsamp: the record line's samples per signal (0 or null: not stated). */
  function matMismatch(bytes, sigs, nsamp) {
    let l;
    try {
      l = matLayout(bytes);
    } catch (e) {
      if (e.notMat) return e.message;
      throw e;
    }
    const s = sigs[0];
    const other = sigs.find((o) => o.fmt !== s.fmt);
    if (other) return `the header gives it two storage formats (${s.fmt} and ${other.fmt})`;
    if (!MAT_OF_FORMAT[s.fmt]) return `the header gives format ${s.fmt}, which no MAT matrix holds (8-bit: 80, 16-bit: 16 or 61, 32-bit: 32)`;
    const [elem, order] = MAT_OF_FORMAT[s.fmt];
    const endian = (be) => (be ? 'big-endian' : 'little-endian');
    if (l.elem !== elem || (order !== null && order !== l.bigEndian)) {
      const got = MAT_ELEMS[l.elem][0] > 1 ? `${endian(l.bigEndian)} ${MAT_ELEMS[l.elem][1]}` : MAT_ELEMS[l.elem][1];
      const want = order !== null ? `${endian(order)} ${MAT_ELEMS[elem][1]}` : MAT_ELEMS[elem][1];
      return `it holds ${got}; format ${s.fmt} reads ${want}`;
    }
    const rows = sigs.reduce((a, x) => a + x.spf, 0);
    if (l.rows !== rows) {
      const hint = l.cols === rows ? ' (it is transposed: one column per signal)' : '';
      return `its matrix has ${l.rows} rows; the header gives ${rows} signals in it, one row each${hint}`;
    }
    if (l.start !== s.offset) return `its samples start at byte ${l.start}; the header gives +${s.offset}`;
    if (nsamp > 0 && l.cols !== nsamp) return `it holds ${l.cols} samples per signal; the header gives ${nsamp}`;
    const need = l.rows * l.cols * MAT_ELEMS[l.elem][0];
    if ((l.bytes !== null && l.bytes !== need) || bytes.length < l.start + need) return `it is shorter than its ${l.rows} × ${l.cols} matrix`;
    return null;
  }

  /** An error for a signal file the record needs and does not have: `missing` names it (the page
   * asks for it). */
  const missingFile = (file) => Object.assign(new Error(`the signal file ${file} is missing`), { missing: file });

  /**
   * A WFDB record from its header text and its signal files ({ name: ArrayBuffer }). Only the
   * first `seconds` (default 10, a resting ECG; 0 = all) are read, and no more frames than the
   * header states (a MAT file's padding is not read as samples).
   */
  function readWfdb(headerText, files, seconds = 10) {
    const h = parseHeader(headerText);
    const limit = Math.min(seconds > 0 ? Math.ceil(seconds * h.fs) : Infinity, h.nsamp > 0 ? h.nsamp : Infinity);
    const maxFrames = Number.isFinite(limit) ? limit : undefined;
    const byFile = new Map();
    h.signals.forEach((s, i) => {
      if (!byFile.has(s.file)) byFile.set(s.file, []);
      byFile.get(s.file).push({ ...s, i });
    });
    const leads = new Array(h.signals.length);
    for (const [file, sigs] of byFile) {
      const buf = files[file] ?? files[file.split('/').pop()];
      if (!buf) throw missingFile(file);
      if (/\.mat$/i.test(file)) {
        const why = matMismatch(new Uint8Array(buf), sigs, h.nsamp);
        if (why) throw new Error(`${file.split('/').pop()} does not match its header ${h.name}.hea: ${why}`);
      }
      // a skewed signal (format:skew) is stored `skew` frames late: its frame n is the file's
      // n + skew (as the WFDB library reads it); past the file's end, its samples are missing
      const skew = Math.max(0, ...sigs.map((s) => s.skew));
      const raw = decodeFile(new Uint8Array(buf), sigs, maxFrames === undefined ? undefined : maxFrames + skew);
      const read = raw[0].length / sigs[0].spf;
      const frames = maxFrames === undefined ? read : Math.min(maxFrames, read);
      sigs.forEach((s, j) => {
        const k = toMv(s.units);
        const r = raw[j];
        const from = s.skew * s.spf;
        // a signal with several samples per frame is kept at its own rate: shown at fs × spf
        const v = new Float64Array(frames * s.spf);
        for (let n = 0; n < v.length; n++) v[n] = from + n < r.length ? ((r[from + n] - s.baseline) / s.gain) * k : NaN;
        leads[s.i] = { name: s.desc, units: 'mV', v, fs: h.fs * s.spf };
      });
    }
    return checked({ name: h.name, start: h.start, fs: h.fs, leads, meta: metaFromLines(h.comments) });
  }

  /** The signal files a header names. */
  function wfdbFiles(headerText) {
    return [...new Set(parseHeader(headerText).signals.map((s) => s.file))];
  }

  /**
   * The segments of a multi-segment WFDB record (record line `name/n`), else null:
   * { name, fs, start, segments: [{ name, n }] (`~`: a null segment), comments }.
   */
  function segmentTable(headerText) {
    const all = headerText.split(/\r?\n/);
    const lines = all.map((l) => l.replace(/#.*$/, '').trim()).filter(Boolean);
    const rec = (lines[0] || '').split(/\s+/);
    if (!rec[0] || !rec[0].includes('/')) return null;
    const [name, n] = rec[0].split('/');
    const nseg = parseInt(n, 10);
    const comments = all.filter((l) => /^\s*#/.test(l)).map((l) => l.replace(/^\s*#\s?/, '').trim()).filter(Boolean);
    const segments = lines.slice(1, 1 + nseg).map((l) => {
      const f = l.split(/\s+/);
      return { name: f[0], n: parseInt(f[1], 10) || 0 };
    });
    return { name, fs: rec[2] ? parseFloat(rec[2].split(/[/(]/)[0]) : 250, start: parseStart(rec[4], rec[5]), segments, comments };
  }

  /**
   * Any WFDB record, single- or multi-segment (docs/wfdb-collection.md): `get(name)` gives the
   * bytes of a file next to the header (a segment's header, a signal file).  A multi-segment
   * record's segments follow each other from its start; a null segment, and a signal a segment
   * does not hold, are gaps (NaN); its leads are its layout's (else its first segment's), by
   * name.  The first `seconds` are read.
   */
  async function readWfdbAny(headerText, get, seconds = 10) {
    const table = segmentTable(headerText);
    const text = (buf) => new TextDecoder().decode(buf);
    if (!table) {
      const files = {};
      for (const f of wfdbFiles(headerText)) files[f] = await get(f);
      return readWfdb(headerText, files, seconds);
    }
    const want = seconds > 0 ? Math.ceil(seconds * table.fs) : Infinity;
    const leads = new Map();   // name -> { name, units, fs, parts: [Float64Array] }
    const total = { n: 0 };
    const pad = (lead, n) => { if (n > 0) lead.parts.push(new Float64Array(n).fill(NaN)); };
    for (const seg of table.segments) {
      if (total.n >= want) break;
      const n = Math.min(seg.n, want - total.n);
      if (seg.name === '~' || seg.n === 0) {
        if (seg.name !== '~' && seg.n === 0) {
          // the layout segment: the record's signals, in order
          for (const sg of parseHeader(text(await get(`${seg.name}.hea`))).signals) {
            if (!leads.has(sg.desc)) leads.set(sg.desc, { name: sg.desc, units: 'mV', fs: table.fs, parts: [] });
          }
        }
        if (seg.name === '~') { for (const l of leads.values()) pad(l, n); total.n += n; }
        continue;
      }
      const segText = text(await get(`${seg.name}.hea`));
      const files = {};
      for (const f of wfdbFiles(segText)) files[f] = await get(f);
      const r = readWfdb(segText, files, n / table.fs);
      for (const l of r.leads) {
        if (!leads.has(l.name)) {
          const lead = { name: l.name, units: 'mV', fs: table.fs, parts: [] };
          pad(lead, total.n);
          leads.set(l.name, lead);
        }
      }
      for (const lead of leads.values()) {
        const l = r.leads.find((x) => x.name === lead.name);
        if (l) lead.parts.push(l.v.slice(0, n)); else pad(lead, n);
      }
      total.n += n;
    }
    const join = (parts) => {
      const v = new Float64Array(parts.reduce((a, p) => a + p.length, 0));
      let o = 0;
      for (const p of parts) { v.set(p, o); o += p.length; }
      return v;
    };
    const out = [...leads.values()].map((l) => ({ name: l.name, units: 'mV', fs: l.fs, v: join(l.parts) }));
    return checked({ name: table.name, start: table.start, fs: table.fs, leads: out, meta: metaFromLines(table.comments) });
  }


  /** The record line of a WFDB header: a name, a number of signals, a rate ("100 2 360 650000"). */
  const RECORD_LINE = /^[\w.-]+\s+\d+\s+\d/;

  Readers.add({
    key: 'wfdb',
    label: 'WFDB records (the .hea file, then its signal files: .dat or .mat)',
    extensions: ['hea'],
    companions: ['dat', 'mat'],
    loose: true,
    sniff: (name, bytes) => RECORD_LINE.test(Readers.latin1(bytes, 0, 64)),
    // single- or multi-segment: the files beside the header (segments' headers too)
    read: (name, bytes, sibling) => readWfdbAny(new TextDecoder().decode(Readers.u8(bytes)), sibling),
    // the segments and layout of a multi-segment record belong to it
    parts: (name, bytes) => {
      const t = segmentTable(new TextDecoder().decode(Readers.u8(bytes)));
      return t ? t.segments.filter((s) => s.name !== '~').map((s) => `${s.name}.hea`) : [];
    },
  });

  const api = { parseHeader, readWfdb, readWfdbAny, segmentTable, wfdbFiles, decodeFile, matMismatch };
  if (node) module.exports = api;
  else root.WFDB = api;
})(typeof self !== 'undefined' ? self : this);
