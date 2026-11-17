// GE MUSE resting ECG XML for the ECG viewer: the same reading as the Python reader
// (python/muse, muse_reader_py.read_muse), giving the viewer's JSON form (ecg.js readJson).
// What is read: the rhythm strip (8 recorded leads, the other four derived), its rate and scale,
// and the median beats with the P, QRS and T onsets marked; the test's details; the
// measurements; the computer's statements (Marquette 12SL) and the confirmed or edited ones.
// The rate is the rhythm strip's SampleBase, else unknown (the strip is then shown by sample
// number, never at a guessed rate); each file is checked against a list of known GE formats,
// and what does not fit is reported.
// The patient's identity and the order are never read.
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

  /**
   * The encoding of XML bytes: a byte order mark first (EF BB BF UTF-8, FF FE UTF-16LE, FE FF
   * UTF-16BE), then UTF-16 without one (the zero bytes of '<' in 3C 00 / 00 3C), then the
   * encoding the XML declaration names, else UTF-8.  { enc, skip (the mark's length) }.
   */
  function encodingOf(u8) {
    if (u8[0] === 0xef && u8[1] === 0xbb && u8[2] === 0xbf) return { enc: 'utf-8', skip: 3 };
    if (u8[0] === 0xff && u8[1] === 0xfe) return { enc: 'utf-16le', skip: 2 };
    if (u8[0] === 0xfe && u8[1] === 0xff) return { enc: 'utf-16be', skip: 2 };
    if (u8.length >= 4 && u8[0] && !u8[1] && u8[2] && !u8[3]) return { enc: 'utf-16le', skip: 0 };
    if (u8.length >= 4 && !u8[0] && u8[1] && !u8[2] && u8[3]) return { enc: 'utf-16be', skip: 0 };
    const head = String.fromCharCode(...u8.subarray(0, 200));
    const named = ((/^\s*<\?xml[^>]*encoding\s*=\s*["']([\w.:-]+)["']/i.exec(head) || [])[1] || 'utf-8').toLowerCase();
    // a declaration of UTF-16 in 8-bit text is wrong about it: the text is read as UTF-8
    return { enc: /^(utf-?16|ucs-?2)/.test(named) ? 'utf-8' : named, skip: 0 };
  }

  /** The text of the bytes (the first `limit` of them, when given), in their encoding (encodingOf). */
  function decode(bytes, limit) {
    let u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const { enc, skip } = encodingOf(u8);
    u8 = u8.subarray(skip, limit ? skip + limit : u8.length);
    let text;
    try { text = new TextDecoder(enc).decode(u8); } catch { text = new TextDecoder('utf-8').decode(u8); }
    return text.replace(/^\uFEFF/, '');
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

  // ---------------------------------------------------------------- formats and rates
  // The known GE formats: (cart, 12SL program) -> the rates and lengths they record with (the
  // same table as FORMATS of the Python reader).
  // [device, program, fs, median fs, rhythm samples, median samples, measurement rate]
  const FORMATS = [
    ['CASE', '12SL 20.1', 500, 500, 5000, 600, 500], ['CASE', '12SL V15C', 500, 500, 5000, 600, 500],
    ['CASE', '12SL V19', 500, 500, 5000, 600, 500], ['CASE', '12SL V21', 500, 500, 5000, 600, 500],
    ['CSYS', '237', 500, 500, 5000, 600, 500], ['CSYS', '239', 500, 500, 5000, 600, 500],
    ['D3K', '241', 500, 500, 5000, 600, 500], ['Datam', 'NO SERIAL', 500, 500, 5000, 600, 500],
    ['MAC 8', '250', 250, 250, 2500, 300, 500], ['MAC V', '243', 500, 500, 5000, 600, 500],
    ['MAC2K', '241', 500, 500, 5000, 600, 500], ['MAC35', '241', 500, 500, 5000, 600, 500],
    ['MAC5', '245', 500, 500, 5000, 600, 500],
    ['MAC55', '237', 500, 500, 5000, 600, 500], ['MAC55', '239', 500, 500, 5000, 600, 500],
    ['MAC55', '239', 250, 250, 2500, 300, 500], ['MAC55', '241', 500, 500, 5000, 600, 500],
    ['MAC55', '241 HD', 500, 500, 5000, 600, 500],
    ['MAC5K', '229', 250, 250, 2500, 300, 500], ['MAC5K', '231', 250, 250, 2500, 300, 500],
    ['MAC5K', '231', 500, 500, 5000, 600, 500], ['MAC5K', '233', 250, 250, 2500, 300, 500],
    ['MAC5K', '233', 500, 500, 5000, 600, 500], ['MAC5K', '235', 250, 250, 2500, 300, 500],
    ['MAC5K', '235', 500, 500, 5000, 600, 500], ['MAC5K', '237', 250, 250, 2500, 300, 500],
    ['MAC5K', '237', 500, 500, 5000, 600, 500], ['MAC5K', '239', 500, 500, 5000, 600, 500],
    ['MAC80', '239', 500, 500, 5000, 600, 500], ['MV360', '243', 500, 500, 5000, 600, 500],
  ];
  // what a check found, as the viewer says it
  const CHECKS = {
    BAD_SAMPLE_BASE: 'the sampling rate is not given (SampleBase): shown by sample number',
    NEW_FORMAT: 'a cart, 12SL program, rate or length not seen before',
    SAMPLE_EXPONENT_NOT_0: 'a SampleExponent other than 0',
    MEDIAN_RATE_DIFFERS: 'the median beats are not at the rhythm strip\'s rate',
    LEAD_LENGTHS_DIFFER: 'the leads have different lengths',
    RHYTHM_NOT_10_S: 'the rhythm strip is not 10 s long',
    'MEDIAN_NOT_1.2_S': 'the median beats are not 1.2 s long',
    MULTIPLE_RHYTHM: 'more than one rhythm strip (the first is shown)',
    MEASUREMENT_RATE_UNKNOWN: 'the rate of the P, QRS and T onsets is unknown: they are not marked',
    ONSETS_CONTRADICT_BASE: 'the onsets do not give the QRS duration at their rate',
    QTRGGR_CONTRADICTS_FS: 'the QRS trigger (QTRGGR) is not at 496 ms at this rate',
  };
  const RR_SCALE = { 500: 1, 250: 4 };   // ms per unit of GlobalRR, by fs
  const TRIGGER_MS = 496;                // QTRGGR = TRIGGER_MS × fs / 1000
  const ONSETS = [['POnset', 'P'], ['POffset', 'P end'], ['QOnset', 'QRS'], ['QOffset', 'QRS end'], ['TOffset', 'T end']];

  /** base × 10^exponent in Hz, or null when that is not a rate (a missing exponent is 0). */
  function rate(base, exponent) {
    const b = number(base);
    const e = /\S/.test(exponent) ? number(exponent) : 0;
    if (!(b > 0) || e === null || !Number.isInteger(e) || Math.abs(e) > 6) return null;
    return b * 10 ** e;
  }

  /** The samples of a waveform section: { rate (Hz, or null), raw: Map lead -> µV[] }. */
  function waveform(w) {
    const got = new Map();
    for (const ld of findAll(w, 'LeadData')) {
      const id = text(ld, 'LeadID');
      const size = number(text(ld, 'LeadSampleSize'));
      if (size !== null && size !== 2) throw new Error(`lead ${id}: samples of ${size} bytes (MUSE's are 16-bit)`);
      const k = number(text(ld, 'LeadAmplitudeUnitsPerBit'));
      if (!(k > 0)) throw new Error(`lead ${id}: no amplitude scale (LeadAmplitudeUnitsPerBit)`);
      const raw = base64(text(ld, 'WaveFormData'));
      let n = raw.length >> 1;
      const count = number(text(ld, 'LeadSampleCountTotal'));
      if (Number.isInteger(count) && count > 0 && count < n) n = count;
      const dv = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
      const scale = k * (text(ld, 'LeadAmplitudeUnits').toUpperCase() === 'MILLIVOLTS' ? 1000 : 1);
      const v = new Array(n);
      for (let i = 0; i < n; i++) v[i] = r3(dv.getInt16(2 * i, true) * scale);
      got.set(id, v);
    }
    return { rate: rate(text(w, 'SampleBase'), text(w, 'SampleExponent')), exponent: number(text(w, 'SampleExponent')) || 0, raw: got };
  }

  /** The leads of a waveform, in µV, with III, aVR, aVL, aVF derived from I and II when missing (Einthoven, Goldberger), as carts derive them. */
  function leads(raw) {
    const got = new Map(raw);
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
    return order.map((l) => ({ name: l, units: 'uV', samples: got.get(l) }));
  }

  /** The longest lead's length, and whether the leads differ in length. */
  const lengths = (raw) => {
    const ns = [...raw.values()].map((v) => v.length);
    return { n: ns.length ? Math.max(...ns) : 0, differ: new Set(ns).size > 1 };
  };

  /** Do the onsets of a measurement section give its QRS duration at rate r (within a sample and 1 ms)? null when not all given. */
  function qrsFits(m, r) {
    const [a, b, d] = ['QOnset', 'QOffset', 'QRSDuration'].map((k) => number(text(m, k)));
    if (a === null || b === null || d === null || !r) return null;
    return Math.abs(((b - a) * 1000) / r - d) <= 1000 / r + 1;
  }

  /**
   * The rates of a file and what does not fit the known formats: fs from the rhythm strip's SampleBase (else null: the
   * strip is shown by sample number), the median beats' rate, the rate of the onsets (the
   * measurements' ECGSampleBase, else the original set's, else the one rate at which they give
   * the QRS duration, else the known format's), and the checks' codes.
   */
  function sampling(doc, rhythms, rw, mw) {
    const test = find(doc, 'TestDemographics');
    const device = text(test, 'AcquisitionDevice');
    const program = text(test, 'AnalysisSoftwareVersion');
    const rows = FORMATS.filter((f) => f[0] === device && f[1] === program);
    const warnings = [];
    const warn = (c) => { if (!warnings.includes(c)) warnings.push(c); };
    if (rhythms > 1) warn('MULTIPLE_RHYTHM');
    const fs = rw.rate;
    if (fs === null) warn('BAD_SAMPLE_BASE');
    const r = lengths(rw.raw);
    const m = mw ? lengths(mw.raw) : { n: null, differ: false };
    const mfs = mw ? mw.rate : null;
    if (rw.exponent || (mw && mw.exponent)) warn('SAMPLE_EXPONENT_NOT_0');
    if (r.differ || m.differ) warn('LEAD_LENGTHS_DIFFER');
    if (fs && mfs && mfs !== fs) warn('MEDIAN_RATE_DIFFERS');
    if (fs && r.n !== 10 * fs) warn('RHYTHM_NOT_10_S');
    if (mfs && m.n !== Math.round(1.2 * mfs)) warn('MEDIAN_NOT_1.2_S');
    let format = 'unreadable';
    if (fs !== null) {
      format = rows.some((f) => f[2] === fs && f[3] === mfs && f[4] === r.n && f[5] === m.n) ? 'known' : 'new';
      if (format === 'new') warn('NEW_FORMAT');
    }
    const qtrggr = number(text(find(doc, 'QRSTimesTypes'), 'QTRGGR'));
    if (fs && qtrggr !== null && qtrggr !== (TRIGGER_MS * fs) / 1000) warn('QTRGGR_CONTRADICTS_FS');
    // the rate of the onsets (final measurements)
    const fin = find(doc, 'RestingECGMeasurements');
    const orig = find(doc, 'OriginalRestingECGMeasurements');
    const stored = (x) => rate(text(x, 'ECGSampleBase'), text(x, 'ECGSampleExponent'));
    const asOriginal = ['QOnset', 'QOffset'].every((k) => { const f = number(text(fin, k)); return f !== null && f === number(text(orig, k)); });
    const fits = [250, 500, 1000].filter((x) => qrsFits(fin, x));
    const known = [...new Set(rows.filter((f) => fs === null || f[2] === fs).map((f) => f[6]))];
    const edited = number(text(fin, 'QRSDuration')) !== number(text(orig, 'QRSDuration'));
    let mrate = stored(fin);
    if (mrate) {
      if (qrsFits(fin, mrate) === false && !(edited && asOriginal && qrsFits(orig, stored(orig)))) warn('ONSETS_CONTRADICT_BASE');
    } else if (asOriginal && stored(orig) && qrsFits(orig, stored(orig))) mrate = stored(orig);
    else if (fits.length === 1) [mrate] = fits;
    else if (known.length === 1) [mrate] = known;
    else {
      mrate = null;
      if (ONSETS.some(([k]) => number(text(fin, k)) !== null)) warn('MEASUREMENT_RATE_UNKNOWN');
    }
    const rr = number(text(find(doc, 'QRSTimesTypes'), 'GlobalRR'));
    return { device, program, fs, mfs, rhythmSamples: r.n, medianSamples: m.n, mrate, format, warnings, rrMs: rr !== null && RR_SCALE[fs] ? rr * RR_SCALE[fs] : null };
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

  /** The recording as the report prints it: "MAC5K · 12SL 237 · 250 Hz · known format". */
  function recording(sm) {
    const what = { known: 'known format', new: 'new format', unreadable: 'rate unknown' }[sm.format];
    return [sm.device, sm.program ? `12SL ${sm.program}` : '', sm.fs ? `${sm.fs} Hz` : '', what].filter(Boolean).join(' · ');
  }

  /** A MUSE XML file (its bytes, or its text) in the ECG viewer's JSON form. */
  function read(data) {
    const doc = parseXml(typeof data === 'string' ? data : decode(data));
    if (doc.tag !== 'RestingECG') throw new Error(`not a MUSE resting ECG (root element ${doc.tag})`);
    const test = find(doc, 'TestDemographics');
    const pat = find(doc, 'PatientDemographics');
    const waves = findAll(doc, 'Waveform');
    const kind = (w) => text(w, 'WaveformType').toLowerCase();
    const rhythms = waves.filter((w) => kind(w) === 'rhythm');
    const rhythm = rhythms[0];
    if (!rhythm) throw new Error('no rhythm waveform in this MUSE file');
    const medianW = waves.find((w) => kind(w) === 'median');
    const rw = waveform(rhythm);
    const mw = medianW ? waveform(medianW) : null;
    const sm = sampling(doc, rhythms.length, rw, mw);
    const fs = sm.fs;
    const ls = leads(rw.raw);
    const age = text(pat, 'PatientAge');
    const ageUnits = text(pat, 'AgeUnits').toLowerCase();
    const gender = text(pat, 'Gender');
    const fields = [
      ['Acquired', join(date(text(test, 'AcquisitionDate')), text(test, 'AcquisitionTime'))],
      ['Device', join(text(test, 'AcquisitionDevice'), text(test, 'AcquisitionSoftwareVersion'))],
      ['Recording', recording(sm)],
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
    const bp = [['SystolicBP', 'Systolic BP'], ['DiastolicBP', 'Diastolic BP']];
    for (const [tag, name] of bp) {
      const v = number(text(meas, tag));
      if (v !== null) measurements.push({ name, value: v, unit: 'mmHg' });
    }
    if (sm.rrMs !== null) measurements.push({ name: 'RR', value: sm.rrMs, unit: 'ms' });
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
    const notes = sm.warnings.map((c) => `Check: ${CHECKS[c] || c} (${c})`);
    const out = { name: 'MUSE ECG', fs, leads: ls, meta: { fields, measurements, statements, notes } };
    if (fs === null) out.rateUnknown = true;
    if (mw && mw.raw.size) {
      // the onsets and offsets on the median beat, in its samples: at the onsets' rate in the file
      const marks = [];
      if (mw.rate && sm.mrate) {
        for (const [tag, name] of ONSETS) {
          const v = number(text(meas, tag));
          if (v !== null) marks.push({ name, sample: r3((v * mw.rate) / sm.mrate) });
        }
      }
      out.median = { fs: mw.rate, leads: leads(mw.raw), marks };
      if (mw.rate === null) out.median.rateUnknown = true;
    }
    return out;
  }

  /** Whether the bytes look like a MUSE resting ECG (its root element near the start, in any of the encodings of decode). */
  const isMuse = (bytes) => /<RestingECG[\s>]/.test(decode(bytes, 800));

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

  const api = { read, isMuse, parseXml, decode, encodingOf, base64, rate, FORMATS, CHECKS };
  if (node) module.exports = api;
  else root.MUSE = api;
})(typeof self !== 'undefined' ? self : this);
