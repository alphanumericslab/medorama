// WFDB records for the ECG viewer: the header (.hea) and its signal files, single- or
// multi-segment (docs/wfdb-collection.md); formats 8, 16, 24, 32, 61, 80, 160 and 212.
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

  /**
   * A WFDB record from its header text and its signal files ({ name: ArrayBuffer }). Only the
   * first `seconds` (default 10, a resting ECG; 0 = all) are read.
   */
  function readWfdb(headerText, files, seconds = 10) {
    const h = parseHeader(headerText);
    const maxFrames = seconds > 0 ? Math.ceil(seconds * h.fs) : undefined;
    const byFile = new Map();
    h.signals.forEach((s, i) => {
      if (!byFile.has(s.file)) byFile.set(s.file, []);
      byFile.get(s.file).push({ ...s, i });
    });
    const leads = new Array(h.signals.length);
    for (const [file, sigs] of byFile) {
      const buf = files[file] ?? files[file.split('/').pop()];
      if (!buf) throw new Error(`the signal file ${file} is missing`);
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
    label: 'WFDB records (the .hea file with its signal files and segments)',
    extensions: ['hea'],
    companions: ['dat'],
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

  const api = { parseHeader, readWfdb, readWfdbAny, segmentTable, wfdbFiles, decodeFile };
  if (node) module.exports = api;
  else root.WFDB = api;
})(typeof self !== 'undefined' ? self : this);
