// GE MUSE resting ECG XML for the ECG viewer: the same reading as the Python reader
// (python/muse, muse_reader_py.read_muse), giving the viewer's JSON form (ecg.js readJson).
// What is read: the rhythm strip (8 recorded leads, the other four derived), its rate and scale;
// the test's details; the measurements; the computer's statements (Marquette 12SL) and the
// confirmed or edited ones.  The patient's identity and the order are never read.
// No dependencies (its own small XML parser); its reader is added to readers.js when the page has it.
// A classic script, and a CommonJS module for tests.

(function (root) {
  'use strict';

  // ---------------------------------------------------------------- XML
  const ENT = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" };
  const unescape = (s) => s.replace(/&(#x[0-9a-f]+|#\d+|\w+);/gi, (m, e) => {
    if (e[0] === '#') return String.fromCodePoint(e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10));
    return ENT[e] ?? m;
  });

  /** The text of the bytes, in the encoding the XML declaration names (UTF-8 by default). */
  function decode(bytes) {
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const head = String.fromCharCode(...u8.subarray(0, 200));
    const enc = (/^\s*<\?xml[^>]*encoding\s*=\s*["']([\w.:-]+)["']/i.exec(head) || [])[1] || 'utf-8';
    let text;
    try { text = new TextDecoder(enc).decode(u8); } catch { text = new TextDecoder('utf-8').decode(u8); }
    return text.replace(/^﻿/, '');
  }

  // Markup: a comment, CDATA, a declaration or a processing instruction ends where it should or
  // runs to the end of the text (then it is not XML); a tag stops at the next '<'; a '<' that
  // starts none of them is not XML.  One pass whatever the text, without backtracking blow-ups.
  const MARKUP = /<!--[\s\S]*?(?:-->|$)|<!\[CDATA\[([\s\S]*?)(?:\]\]>|$)|<![^>]*(?:>|$)|<\?[\s\S]*?(?:\?>|$)|<\/\s*([^\s<>]+)\s*>|<([^\s/<>]+)(\s(?:[^<>"']|"[^<"]*"|'[^<']*')*|\/)?>|([^<]+)|</g;
  const ended = (t) => (t.startsWith('<!--') ? t.length >= 7 && t.endsWith('-->')
    : t.startsWith('<![CDATA[') ? t.length >= 12 && t.endsWith(']]>')
      : t.startsWith('<?') ? t.length >= 4 && t.endsWith('?>') : t.length > 1 && t.endsWith('>'));

  /** Elements as { tag, text, children }; attributes are not kept (MUSE puts nothing in them). */
  function parseXml(text) {
    const top = { tag: '', text: '', children: [] };
    const stack = [top];
    const re = new RegExp(MARKUP);
    let m;
    while ((m = re.exec(text))) {
      const cur = stack[stack.length - 1];
      if (m[0][0] === '<' && !ended(m[0])) throw new Error('not XML: a markup that does not end');
      if (m[1] !== undefined) cur.text += m[1];
      else if (m[2]) {
        if (stack.length < 2 || cur.tag !== m[2]) throw new Error(`not XML: </${m[2]}> does not close an element`);
        stack.pop();
      } else if (m[3]) {
        const el = { tag: m[3], text: '', children: [] };
        cur.children.push(el);
        if (!(m[4] || '').endsWith('/')) stack.push(el);
      } else if (m[5] !== undefined) cur.text += unescape(m[5]);
    }
    if (stack.length > 1) throw new Error(`not XML: <${stack[stack.length - 1].tag}> is not closed`);
    if (!top.children.length) throw new Error('not XML: no element');
    return top.children[0];
  }

  const find = (e, tag) => (e ? e.children.find((c) => c.tag === tag) || null : null);
  const findAll = (e, tag) => (e ? e.children.filter((c) => c.tag === tag) : []);
  const text = (e, tag) => { const x = find(e, tag); return x ? x.text.trim() : ''; };

  // ---------------------------------------------------------------- values
  const MEASUREMENTS = [
    ['VentricularRate', 'Vent. rate', 'bpm'], ['AtrialRate', 'Atrial rate', 'bpm'],
    ['PRInterval', 'PR', 'ms'], ['QRSDuration', 'QRS', 'ms'], ['QTInterval', 'QT', 'ms'],
    ['QTCorrected', 'QTc', 'ms'], ['QTcFrederica', 'QTcF', 'ms'], ['QTcFridericia', 'QTcF', 'ms'],
    ['PAxis', 'P axis', '°'], ['RAxis', 'R axis', '°'], ['TAxis', 'T axis', '°'],
    ['QRSCount', 'QRS count', 'beats'],
  ];
  const STANDARD = ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'];

  const number = (s) => {
    if (!/\S/.test(s)) return null;
    const v = Number(s);
    return Number.isFinite(v) ? v : null;
  };
  const r3 = (x) => Math.round(x * 1000) / 1000;
  /** MUSE dates: MM-DD-YYYY -> YYYY-MM-DD. */
  const date = (d) => {
    const m = /^(\d{1,2})[-/](\d{1,2})[-/](\d{4})$/.exec(d);
    return m ? `${m[3]}-${m[1].padStart(2, '0')}-${m[2].padStart(2, '0')}` : d;
  };
  const join = (...xs) => xs.filter(Boolean).join(' ');
  const capitalize = (s) => (s ? s[0].toUpperCase() + s.slice(1).toLowerCase() : s);

  const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  const B64I = Object.fromEntries([...B64].map((c, i) => [c, i]));
  /** Base64 to bytes; characters outside the alphabet are skipped (line breaks, a missing padding). */
  function base64(s) {
    const out = [];
    let acc = 0;
    let bits = 0;
    for (const c of s) {
      if (c === '=') break;
      const v = B64I[c];
      if (v === undefined) continue;
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) { bits -= 8; out.push((acc >> bits) & 0xff); }
    }
    return Uint8Array.from(out);
  }

  /** A diagnosis section's statements as lines: fragments joined until a line ends. */
  function lines(diagnosis) {
    const out = [];
    let cur = [];
    for (const st of findAll(diagnosis, 'DiagnosisStatement')) {
      const t = text(st, 'StmtText');
      if (t) cur.push(t);
      if (findAll(st, 'StmtFlag').some((f) => f.text.trim().toUpperCase() === 'ENDSLINE') && cur.length) { out.push(cur.join(' ')); cur = []; }
    }
    if (cur.length) out.push(cur.join(' '));
    return out;
  }

  /** The leads of a waveform section, in µV, with the four derived ones when missing. */
  function leads(w) {
    // the rate: SampleBase × 10^SampleExponent; a base that is not a positive number is 500 Hz,
    // an exponent beyond ±6 is refused (as the Python reader reads it)
    const b = number(text(w, 'SampleBase'));
    const base = b > 0 ? b : 500;
    const exp = number(text(w, 'SampleExponent')) || 0;
    if (Math.abs(exp) > 6) throw new Error(`not a sampling rate: SampleExponent ${exp}`);
    const fs = base * 10 ** exp;
    const got = new Map();
    for (const ld of findAll(w, 'LeadData')) {
      const raw = base64(text(ld, 'WaveFormData'));
      let n = raw.length >> 1;
      const count = number(text(ld, 'LeadSampleCountTotal'));
      if (Number.isInteger(count) && count > 0 && count < n) n = count;
      const dv = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
      const k = number(text(ld, 'LeadAmplitudeUnitsPerBit')) || 1;
      const scale = k * (text(ld, 'LeadAmplitudeUnits').toUpperCase() === 'MILLIVOLTS' ? 1000 : 1);
      const v = new Array(n);
      for (let i = 0; i < n; i++) v[i] = r3(dv.getInt16(2 * i, true) * scale);
      got.set(text(ld, 'LeadID'), v);
    }
    // III, aVR, aVL, aVF from I and II (Einthoven, Goldberger), as carts derive them
    if (got.has('I') && got.has('II')) {
      const a = got.get('I');
      const b = got.get('II');
      const n = Math.min(a.length, b.length);
      const make = (f) => Array.from({ length: n }, (_, i) => r3(f(a[i], b[i])));
      if (!got.has('III')) got.set('III', make((x, y) => y - x));
      if (!got.has('aVR')) got.set('aVR', make((x, y) => -(x + y) / 2));
      if (!got.has('aVL')) got.set('aVL', make((x, y) => x - y / 2));
      if (!got.has('aVF')) got.set('aVF', make((x, y) => y - x / 2));
    }
    const order = [...STANDARD.filter((l) => got.has(l)), ...[...got.keys()].filter((l) => !STANDARD.includes(l))];
    return { fs, leads: order.map((l) => ({ name: l, units: 'uV', samples: got.get(l) })) };
  }

  function filters(w) {
    const hp = number(text(w, 'HighPassFilter'));
    const lp = number(text(w, 'LowPassFilter'));
    const ac = number(text(w, 'ACFilter'));
    const parts = [];
    if (hp !== null) parts.push(lp !== null ? `${hp / 100}–${lp} Hz` : `from ${hp / 100} Hz`);   // high-pass in 0.01 Hz
    if (ac) parts.push(`${ac} Hz notch`);
    return parts.join(', ');
  }

  /** A MUSE XML file (its bytes, or its text) in the ECG viewer's JSON form. */
  function read(data) {
    const doc = parseXml(typeof data === 'string' ? data : decode(data));
    if (doc.tag !== 'RestingECG') throw new Error(`not a MUSE resting ECG (root element ${doc.tag})`);
    const test = find(doc, 'TestDemographics');
    const pat = find(doc, 'PatientDemographics');
    const rhythm = findAll(doc, 'Waveform').find((w) => text(w, 'WaveformType').toLowerCase() === 'rhythm');
    if (!rhythm) throw new Error('no rhythm waveform in this MUSE file');
    const { fs, leads: ls } = leads(rhythm);
    const age = text(pat, 'PatientAge');
    const ageUnits = text(pat, 'AgeUnits').toLowerCase();
    const gender = text(pat, 'Gender');
    const fields = [
      ['Acquired', join(date(text(test, 'AcquisitionDate')), text(test, 'AcquisitionTime'))],
      ['Device', join(text(test, 'AcquisitionDevice'), text(test, 'AcquisitionSoftwareVersion'))],
      ['Site', text(test, 'SiteName')],
      ['Location', text(test, 'LocationName')],
      ['Reason', text(test, 'TestReason') || text(find(doc, 'Order'), 'TestReason')],
      ['Status', capitalize(text(test, 'Status'))],
      ['Age', age ? `${age}${ageUnits === '' || ageUnits === 'years' ? '' : ` ${ageUnits}`}` : ''],
      ['Sex', { MALE: 'M', FEMALE: 'F' }[gender.toUpperCase()] || gender],
      ['Filters', filters(rhythm)],
    ].filter((f) => f[1]);
    const meas = find(doc, 'RestingECGMeasurements');
    const measurements = [];
    const seen = new Set();
    for (const [tag, name, unit] of MEASUREMENTS) {
      const v = number(text(meas, tag));
      if (v !== null && !seen.has(name)) { seen.add(name); measurements.push({ name, value: v, unit }); }
    }
    const statements = [];
    const original = find(doc, 'OriginalDiagnosis');
    const current = find(doc, 'Diagnosis');
    const version = text(test, 'AnalysisSoftwareVersion');
    const o = original ? lines(original) : [];
    if (o.length) statements.push({ source: 'computer', program: `Marquette 12SL${version ? ` v${version}` : ''}`, lines: o });
    const c = current ? lines(current) : [];
    if (c.length) {
      const edited = join(date(text(test, 'EditDate')), text(test, 'EditTime'));
      const same = original !== null && c.join('\n') === o.join('\n');
      if (text(test, 'Status').toUpperCase() === 'CONFIRMED') {
        statements.push({ source: 'confirmed', by: join(text(test, 'OverreaderFirstName'), text(test, 'OverreaderLastName')), time: edited, lines: c });
      } else if (!same) {
        statements.push({ source: 'edited', time: edited, lines: c });
      }
    }
    return { name: 'MUSE ECG', fs, leads: ls, meta: { fields, measurements, statements, notes: [] } };
  }

  /** Whether the bytes look like a MUSE resting ECG. */
  const isMuse = (bytes) => /<RestingECG[\s>]/.test(String.fromCharCode(...new Uint8Array(bytes).subarray(0, 400)));

  // the reader of the page (readers.js): the JSON form, read into the record by ecg.js
  const node = typeof module !== 'undefined' && module.exports;
  const Readers = node ? require('./readers.js') : root.Readers;
  if (Readers) {
    Readers.add({
      key: 'muse',
      label: 'GE MUSE XML',
      extensions: ['xml'],
      sniff: (name, bytes) => isMuse(bytes),
      read: async (name, bytes) => {
        if (!isMuse(bytes)) throw new Error('This XML file is not a MUSE resting ECG (RestingECG).');
        return (node ? require('./ecg.js') : root.ECG).readJson(read(bytes));
      },
    });
  }

  const api = { read, isMuse, parseXml, decode, base64 };
  if (node) module.exports = api;
  else root.MUSE = api;
})(typeof self !== 'undefined' ? self : this);
