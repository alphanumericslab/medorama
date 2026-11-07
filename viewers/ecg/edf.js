// EDF, EDF+, BDF and BDF+ files for the ECG viewer, read by the rules of docs/edf.md ("Reading
// rules"), as the engine reads them for VitalStream and Medorama (libs/engine/src/sources/edf.rs).
// - The leads: the ordinary signals at the sampling rate of the first ECG signal (I … V6, "ECG …",
//   "EKG …"; else the signals at the most common rate), physical values in mV; the first
//   seconds of the recording (10 by default), of its first uninterrupted part (EDF+D).
// - EDF+ and BDF+ annotations within that span are notes, with their times; so is what is left
//   out (other rates, annotation signals, the rest of an interrupted recording, a cut record).
// - The header's patient and recording identification fields (bytes 8–168) hold the patient's
//   identity and are never read, but for the year of an EDF+ "Startdate dd-MMM-yyyy" (its four
//   digits, bytes 105–109), which dates the recording.
// No dependencies but ecg.js (the record) and readers.js (the reader is added there); a classic
// script (it works from file://), and a CommonJS module for tests.

(function (root) {
  'use strict';
  const node = typeof module !== 'undefined' && module.exports;
  const ECG = node ? require('./ecg.js') : root.ECG;
  const Readers = node ? require('./readers.js') : root.Readers;
  const { u8, latin1 } = Readers;

  /** Records closer than this (seconds) to where the one before ends follow it without a break. */
  const CONTIGUOUS_S = 1e-6;
  /** At most this many annotations are listed. */
  const MAX_NOTES = 100;

  // ---------------------------------------------------------------- header
  const strip = (s) => s.replace(/^[\s\x1c-\x1f\x85]+|[\s\x1c-\x1f\x85]+$/g, '');
  /** A header field: its bytes as Latin-1 text, without the spaces around it. */
  const field = (b, at, n) => strip(latin1(b, at, at + n));
  const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i;

  function number(s, what) {
    const v = NUMBER.test(strip(s)) ? Number(strip(s)) : NaN;
    if (!Number.isFinite(v)) throw new Error(`EDF header: ${what} is not a number (${JSON.stringify(s)})`);
    return v;
  }
  function integer(s, what) {
    if (!/^[+-]?\d+$/.test(s)) throw new Error(`EDF header: ${what} is not a whole number (${JSON.stringify(s)})`);
    return parseInt(s, 10);
  }

  /** Days from 1970-01-01 to a date of the proleptic Gregorian calendar. */
  function daysFromCivil(y, m, d) {
    y = m <= 2 ? y - 1 : y;
    const era = Math.floor((y >= 0 ? y : y - 399) / 400);
    const yoe = y - era * 400;
    const doy = Math.floor((153 * ((m + 9) % 12) + 2) / 5) + d - 1;
    return era * 146097 + yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy - 719468;
  }

  /** Three two-digit numbers `aa?bb?cc` (any separators); the third may be anything when `loose`. */
  function three(s, loose) {
    if (!/^[\x00-\x7f]{8}$/.test(s) || !/^\d\d\D\d\d\D/.test(s)) return null;
    const n = (i) => +s.slice(i, i + 2);
    if (/^\d\d$/.test(s.slice(6))) return [n(0), n(3), n(6)];
    return loose ? [n(0), n(3), null] : null;
  }

  /**
   * The four-digit year of an EDF+ recording field that starts with `Startdate dd-MMM-yyyy `,
   * else null.  Only those bytes are looked at, and only the year is kept.
   */
  function plusYear(b) {
    if (latin1(b, 88, 98) !== 'Startdate ' || b[109] !== 0x20) return null;
    const y = latin1(b, 105, 109);
    return /^\d{4}$/.test(y) ? +y : null;
  }

  /** The start (Unix seconds, the header's clock time taken as UTC), or null. */
  function startOf(date, time, fullYear) {
    const d = three(date, fullYear !== null);
    const t = three(time, false);
    if (!d || !t) return null;
    const year = fullYear !== null ? fullYear : d[2] >= 85 ? 1900 + d[2] : 2000 + d[2];
    const [day, m] = d;
    const [hh, mi, ss] = t;
    if (m < 1 || m > 12 || day < 1 || day > 31 || hh > 23 || mi > 59 || ss > 59) return null;
    return daysFromCivil(year, m, day) * 86400 + hh * 3600 + mi * 60 + ss;
  }

  /**
   * What the header of an EDF or BDF file says, its identification fields left out:
   * { bdf, plus ('C', 'D' or null), start, headerBytes, declared, records, rest, duration,
   *   signals: [{ label, unit, prefilter, pmin, pmax, dmin, dmax, spr, annotations, offset }], recordBytes }.
   */
  function header(bytes) {
    const b = u8(bytes);
    if (b.length < 256) throw new Error('not an EDF or BDF file');
    const bdf = b[0] === 0xff && latin1(b, 1, 8) === 'BIOSEMI';
    if (!bdf && field(b, 0, 8) !== '0') throw new Error('not an EDF or BDF file');
    const reserved = field(b, 192, 44);
    const plus = ['EDF+C', 'EDF+D', 'BDF+C', 'BDF+D'].find((p) => reserved.startsWith(p))?.slice(-1) || null;
    const start = startOf(field(b, 168, 8), field(b, 176, 8), plus ? plusYear(b) : null);
    const headerBytes = integer(field(b, 184, 8), 'the number of header bytes');
    const declared = integer(field(b, 236, 8), 'the number of data records');
    const duration = number(field(b, 244, 8), 'the duration of a data record');
    const ns = integer(field(b, 252, 4), 'the number of signals');
    if (ns < 1 || headerBytes !== 256 * (ns + 1)) throw new Error(`EDF header: ${ns} signals do not fit its ${headerBytes} header bytes`);
    if (b.length < headerBytes) throw new Error('EDF header cut short');
    // the signal fields, each for every signal in turn
    const widths = [16, 80, 8, 8, 8, 8, 8, 80, 8, 32];
    const col = (c, k) => {
      let at = 256;
      for (let i = 0; i < c; i++) at += widths[i] * ns;
      return field(b, at + k * widths[c], widths[c]);
    };
    const width = bdf ? 3 : 2;
    const signals = [];
    let offset = 0;
    for (let k = 0; k < ns; k++) {
      const label = col(0, k);
      const spr = integer(col(8, k), 'a number of samples per data record');
      if (spr < 0) throw new Error(`EDF header: signal ${k + 1} has ${spr} samples per data record`);
      const s = {
        label, unit: col(2, k), prefilter: col(7, k),
        pmin: number(col(3, k), 'a physical minimum'), pmax: number(col(4, k), 'a physical maximum'),
        dmin: number(col(5, k), 'a digital minimum'), dmax: number(col(6, k), 'a digital maximum'),
        spr, annotations: label === 'EDF Annotations' || label === 'BDF Annotations', offset,
      };
      offset += spr * width;
      signals.push(s);
    }
    const recordBytes = offset;
    const body = Math.max(0, b.length - headerBytes);
    let records = recordBytes ? Math.floor(body / recordBytes) : 0;
    const rest = recordBytes ? body % recordBytes : body;
    if (declared >= 0) records = Math.min(records, declared);
    if (!(duration > 0) && signals.some((s) => !s.annotations && s.spr > 0)) throw new Error(`EDF header: a data record lasts ${duration} s`);
    return { bdf, plus, start, headerBytes, declared, records, rest, duration, signals, recordBytes };
  }

  /** `EDF`, `EDF+`, `BDF` or `BDF+`. */
  const formatOf = (h) => `${h.bdf ? 'BDF' : 'EDF'}${h.plus ? '+' : ''}`;

  // ---------------------------------------------------------------- annotations
  /** Python's `' '.join(text.split())`. */
  const words = (t) => t.split(/[\s\x1c-\x1f\x85]+/).filter(Boolean).join(' ');

  /**
   * The time-stamped annotation lists of an annotation signal's bytes:
   * [{ onset, duration (or null), texts }], each `+onset[\x15duration]\x14text\x14…\x14\x00`.
   */
  function tals(bytes) {
    const b = u8(bytes);
    const utf8 = new TextDecoder();
    const split = (x, c) => {
      const out = [];
      let p = 0;
      for (let i = 0; i <= x.length; i++) if (i === x.length || x[i] === c) { out.push(x.subarray(p, i)); p = i + 1; }
      return out;
    };
    const num = (x) => { const t = x ? strip(latin1(x, 0, x.length)) : ''; return NUMBER.test(t) ? Number(t) : null; };
    const out = [];
    for (const tal of split(b, 0)) {
      if (!tal.length) continue;
      const [time, ...texts] = split(tal, 0x14);
      const [on, du] = split(time, 0x15);
      const onset = num(on);
      if (onset === null) continue;
      out.push({ onset, duration: num(du), texts: texts.map((x) => words(utf8.decode(x))) });
    }
    return out;
  }

  // ---------------------------------------------------------------- the record
  /** An ECG lead's label: I … V6, "ECG …", "EKG …". */
  const isEcg = (label) => !!ECG.standardLead(label) || /^(ecg|ekg)/i.test(label);
  const seconds = (x) => `${+x.toFixed(3)} s`;
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  /**
   * An EDF or BDF file (its bytes) as an ECG record: the first `seconds` (default 10; 0 = all) of
   * its first uninterrupted part.
   */
  function readEdf(bytes, span = 10) {
    const b = u8(bytes);
    const h = header(b);
    const format = formatOf(h);
    const width = h.bdf ? 3 : 2;
    const ordinary = h.signals.map((s, k) => k).filter((k) => !h.signals[k].annotations && h.signals[k].spr > 0);
    const notesSig = h.signals.map((s, k) => k).filter((k) => h.signals[k].annotations);
    if (!ordinary.length) throw new Error(`This ${format} file holds no signal to draw, only annotations.`);
    if (!h.records) throw new Error(`This ${format} file holds no data records.`);

    // the records' onsets (EDF+: their time-keeping annotations), the runs, the annotations
    const events = [];
    let end = null;
    let runs = 0;
    let first = h.records;      // the records of the first run: [0, first)
    let onset0 = 0;
    for (let r = 0; r < h.records; r++) {
      const base = h.headerBytes + r * h.recordBytes;
      let onset = null;
      notesSig.forEach((k, i) => {
        const s = h.signals[k];
        tals(b.subarray(base + s.offset, base + s.offset + s.spr * width)).forEach((t, n) => {
          if (i === 0 && n === 0 && h.plus) onset = t.onset;
          for (const text of t.texts) if (text) events.push({ t: t.onset, d: t.duration > 0 ? t.duration : 0, text });
        });
      });
      if (onset === null) onset = end ?? 0;
      if (end === null || Math.abs(onset - end) > CONTIGUOUS_S) {
        runs++;
        if (runs === 1) onset0 = onset;
        if (runs === 2) first = r;
      }
      end = onset + h.duration;
    }

    // the leads: the signals at the first ECG signal's rate, else at the most common one
    const ecg = ordinary.find((k) => isEcg(h.signals[k].label));
    let spr;
    if (ecg !== undefined) spr = h.signals[ecg].spr;
    else {
      const count = new Map();
      for (const k of ordinary) count.set(h.signals[k].spr, (count.get(h.signals[k].spr) || 0) + 1);
      spr = [...count].reduce((a, c) => (c[1] > a[1] ? c : a))[0];
    }
    const fs = spr / h.duration;
    const chosen = ordinary.filter((k) => h.signals[k].spr === spr);
    const n = Math.min(span > 0 ? Math.ceil(span * fs) : Infinity, first * spr);
    // a label repeated gets its count ("ECG (2)"), as in VitalStream
    const seen = new Map();
    const labels = new Map(ordinary.map((k) => {
      const l = h.signals[k].label;
      seen.set(l, (seen.get(l) || 0) + 1);
      return [k, seen.get(l) === 1 ? l : `${l} (${seen.get(l)})`];
    }));
    const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
    const leads = chosen.map((k) => {
      const s = h.signals[k];
      const mv = ECG.toMv(s.unit);
      const gain = (s.pmax - s.pmin) / (s.dmax - s.dmin);
      const v = new Float64Array(n);
      for (let i = 0; i < n; i++) {
        const at = h.headerBytes + Math.floor(i / spr) * h.recordBytes + s.offset + (i % spr) * width;
        let d;
        if (h.bdf) {
          d = dv.getUint8(at) | (dv.getUint8(at + 1) << 8) | (dv.getUint8(at + 2) << 16);
          if (d & 0x800000) d -= 0x1000000;
        } else d = dv.getInt16(at, true);
        v[i] = (s.dmax === s.dmin ? d : s.pmin + (d - s.dmin) * gain) * mv;
      }
      return { name: labels.get(k), units: 'mV', fs, v };
    });

    // what the page says about the file
    const notes = [h.start !== null ? 'The file\'s start time is taken as UTC.' : 'The file states no valid start time.'];
    if (h.declared >= 0 && h.declared > h.records) notes.push(`The header states ${h.declared} data records; the file holds ${h.records}.`);
    else if (h.rest > 0) notes.push(`The file ends inside a data record: its last ${h.rest} bytes are left out.`);
    if (runs > 1) notes.push(`The recording is interrupted (${format}, ${runs} parts): only its first part, ${seconds(first * h.duration)}, is read.`);
    if (n < first * spr) notes.push(`The first ${seconds(n / fs)} of ${seconds(first * h.duration)} are shown.`);
    const other = ordinary.filter((k) => h.signals[k].spr !== spr);
    if (other.length) {
      const list = other.slice(0, 10).map((k) => `${labels.get(k)} (${+(h.signals[k].spr / h.duration).toFixed(3)} Hz)`).join(', ');
      notes.push(`${plural(other.length, 'signal', 'signals')} at another sampling rate than ${ecg !== undefined ? 'the ECG\'s' : 'the most common,'} ${+fs.toFixed(3)} Hz left out: ${list}${other.length > 10 ? ', …' : ''}.`);
    }
    const empty = h.signals.filter((s) => !s.annotations && s.spr === 0).length;
    if (empty) notes.push(`${plural(empty, 'signal', 'signals')} without samples left out.`);
    // the annotations within the span shown, at their times from its start
    const a = onset0;
    const z = onset0 + n / fs;
    const within = events.filter((e) => e.t <= z && e.t + e.d >= a).sort((x, y) => x.t - y.t);
    if (notesSig.length) {
      notes.push(`${plural(notesSig.length, 'annotation signal', 'annotation signals')} not drawn: ${within.length ? 'the annotations within this ECG follow' : 'no annotation falls within this ECG'}.`);
    }
    for (const e of within.slice(0, MAX_NOTES)) notes.push(`Annotation at ${seconds(e.t - a)}: ${e.text}${e.d > 0 ? ` (${seconds(e.d)})` : ''}`);
    if (within.length > MAX_NOTES) notes.push(`… and ${within.length - MAX_NOTES} more annotations.`);

    const filters = [...new Set(chosen.map((k) => h.signals[k].prefilter))];
    const fields = [['Format', format]];
    if (filters.length === 1 && filters[0]) fields.push(['Filters', filters[0]]);
    return ECG.checked({
      name: `${format} ECG`, start: h.start === null ? null : h.start + onset0, fs, leads,
      meta: { fields, measurements: [], statements: [], notes },
    });
  }

  /** Whether the bytes start as an EDF or BDF header does. */
  function isEdf(bytes) {
    const b = u8(bytes);
    if (b.length < 256 || !(field(b, 0, 8) === '0' || (b[0] === 0xff && latin1(b, 1, 8) === 'BIOSEMI'))) return false;
    const hb = field(b, 184, 8);
    const ns = field(b, 252, 4);
    return /^\d+$/.test(hb) && /^\d+$/.test(ns) && +ns >= 1 && +hb === 256 * (+ns + 1);
  }

  Readers.add({
    key: 'edf',
    label: 'EDF and BDF (EDF+, BDF+)',
    extensions: ['edf', 'bdf'],
    sniff: (name, bytes) => isEdf(bytes),
    read: async (name, bytes) => readEdf(bytes),
  });

  const api = { header, readEdf, tals, isEdf };
  if (node) module.exports = api;
  else root.EDF = api;
})(typeof self !== 'undefined' ? self : this);
