// The ECG record of the ECG viewer, which every reader gives (readers.js documents it), and its
// helpers: units to millivolts, metadata from "key: value" lines, the JSON form, the twelve
// leads, the report of a de-identified view.  The readers of the formats are in their own files
// (wfdb.js, muse.js, dicom-ecg.js, edf.js), listed in readers.js.
// No dependencies; a classic script (it works from file://), and a CommonJS module for tests.
//
// A record is { name, start (Unix seconds or null), fs (Hz), leads: [{ name, units, v: Float64Array (mV) }],
// meta }, where meta (all optional) is what the viewer prints above the paper:
//   { fields: [[label, text]], measurements: [{ name, value, unit, lead? }],
//     statements: [{ source: 'computer' | 'confirmed' | 'edited', program?, by?, time?, lines: [text] }], notes: [text] }
// Patient identity is never part of it.

(function (root) {
  'use strict';

  // ---------------------------------------------------------------- metadata
  // what never shows: a patient's identity (a key that names one anywhere in it: "Full name",
  // "PatientID", "Subject ID", "NHS number", "Born"); free text (notes, statements) shows as written
  const IDENTIFYING = new RegExp('patient|subject|name|maiden|initials|mrn|medical ?record|hospital ?(number|no)|accession|nhs|ssn|social ?security'
    + '|insurance|identifier|birth|born|address|street|zip|post ?code|postal|phone|e-?mail|(case|order|visit|encounter|episode|account|admission|chart) ?(number|no)'
    + '|(?<![a-z0-9])(pt|id|uid|dob|tel|fax|city)(?![a-z0-9])', 'i');
  // measurements by their usual names (in WFDB comments, cart exports), with default units
  const MEASURES = [
    [/^(vent\.?|ventricular)( rate)?$|^(hr|heart ?rate)$/i, 'Vent. rate', 'bpm'], [/^(atrial|atr\.?) ?rate$/i, 'Atrial rate', 'bpm'],
    [/^pr( int(erval)?)?$/i, 'PR', 'ms'], [/^qrs( dur(ation)?)?$/i, 'QRS', 'ms'], [/^qt( int(erval)?)?$/i, 'QT', 'ms'],
    [/^qtc$/i, 'QTc', 'ms'], [/^qtc ?b(azett)?$/i, 'QTcB', 'ms'], [/^qtc ?f(ridericia)?$/i, 'QTcF', 'ms'], [/^rr( int(erval)?)?$/i, 'RR', 'ms'],
    [/^p[- ]?axis$/i, 'P axis', '°'], [/^(r|qrs)[- ]?axis$/i, 'R axis', '°'], [/^t[- ]?axis$/i, 'T axis', '°'],
  ];
  const STATEMENTS = /^(dx|diagnos[ie]s|interpretation|statement|comment|12sl|reason)$/i;

  /** Metadata from free "key: value" lines (WFDB header comments). */
  const LEAD = /^(.*\S)\s*[ (]\s*(I|II|III|aVR|aVL|aVF|V[1-9])\)?$/;
  /**
   * Metadata from free "key: value" lines (WFDB header comments).  Statements: `Dx` (the
   * computer's), `Dx confirmed` with `Confirmed by` and `Confirmed at`, `Dx edited`; `Program`
   * names the computer's program.  `ST V5: -0.12 mV` is a lead's measurement.
   */
  function metaFromLines(lines) {
    const meta = { fields: [], measurements: [], statements: [], notes: [] };
    const st = { computer: [], confirmed: [], edited: [] };
    let program = '';
    let by = '';
    let at = '';
    for (const line of lines) {
      const m = /^([^:=]{1,40})[:=]\s*(.*)$/.exec(line);
      if (!m) { meta.notes.push(line); continue; }
      const key = m[1].trim();
      const value = m[2].trim();
      if (IDENTIFYING.test(key) || !value) continue;
      const num = /^(-?\d+(?:\.\d+)?)\s*([^\d\s].*)?$/.exec(value);
      const lead = LEAD.exec(key);
      const known = MEASURES.find(([re]) => re.test(lead ? lead[1] : key));
      const split = () => value.split(/\s*[;|]\s*/).filter(Boolean);
      if (num && lead) {
        meta.measurements.push({ name: known ? known[1] : lead[1], value: +num[1], unit: (num[2] || (known ? known[2] : '')).trim(), lead: lead[2] });
      } else if (known && num) {
        meta.measurements.push({ name: known[1], value: +num[1], unit: (num[2] || known[2]).trim() });
      } else if (/^(dx|diagnosis|interpretation|statements?) ?confirmed$|^confirmed (dx|diagnosis|interpretation|statements?)$/i.test(key)) {
        st.confirmed.push(...split());
      } else if (/^(dx|diagnosis|interpretation|statements?) ?edited$/i.test(key)) {
        st.edited.push(...split());
      } else if (/^confirmed by$|^overread(er)?( by)?$/i.test(key)) {
        by = value;
      } else if (/^(confirmed|edited) (at|on)$/i.test(key)) {
        at = value;
      } else if (/^(program|analysis program|12sl version)$/i.test(key)) {
        program = value;
      } else if (STATEMENTS.test(key)) {
        st.computer.push(...split());
      } else {
        meta.fields.push([key, value]);
      }
    }
    if (st.computer.length) meta.statements.push({ source: 'computer', program, lines: st.computer });
    if (st.confirmed.length) meta.statements.push({ source: 'confirmed', by, time: at, lines: st.confirmed });
    else if (st.edited.length) meta.statements.push({ source: 'edited', time: at, lines: st.edited });
    return meta;
  }

  /** The `meta` of the JSON form, checked: only known parts, never identity. */
  function cleanMeta(m) {
    if (!m || typeof m !== 'object') return null;
    const str = (x) => (x === null || x === undefined ? '' : String(x));
    return {
      fields: (Array.isArray(m.fields) ? m.fields : []).filter((f) => Array.isArray(f) && f.length >= 2 && !IDENTIFYING.test(str(f[0]))).map((f) => [str(f[0]), str(f[1])]),
      measurements: (Array.isArray(m.measurements) ? m.measurements : []).filter((x) => x && x.name !== undefined)
        .map((x) => ({ name: str(x.name), value: x.value, unit: str(x.unit), lead: x.lead ? str(x.lead) : '' })),
      statements: (Array.isArray(m.statements) ? m.statements : []).filter((x) => x && Array.isArray(x.lines))
        .map((x) => ({ source: ['confirmed', 'edited'].includes(x.source) ? x.source : 'computer', program: str(x.program), by: str(x.by), time: str(x.time), lines: x.lines.map(str) })),
      notes: (Array.isArray(m.notes) ? m.notes : []).map(str),
    };
  }

  /** Units to millivolts. */
  function toMv(units) {
    const u = (units || 'mV').toLowerCase();
    if (u === 'mv') return 1;
    if (u === 'uv' || u === 'µv' || u === 'μv') return 0.001;
    if (u === 'v') return 1000;
    return 1;
  }

  /** A record the paper can draw, else an Error saying why (no leads, no samples, no rate). */
  function checked(rec) {
    if (!rec.leads.length) throw new Error('this ECG has no leads');
    for (const l of rec.leads) {
      if (!(l.fs > 0) || !Number.isFinite(l.fs)) throw new Error(`lead ${l.name}: no sampling rate`);
    }
    if (!rec.leads.some((l) => l.v.some(Number.isFinite))) throw new Error('this ECG has no samples');
    return rec;
  }

  /** The viewer's JSON form: { name, start, leads: [{ name, fs, units, samples }] } (what a MUSE reader can write). */
  function readJson(obj) {
    if (!obj || !Array.isArray(obj.leads)) throw new Error('not an ECG in the viewer\'s JSON form (leads missing)');
    const fs = obj.fs ?? obj.leads[0]?.fs;
    return checked({
      name: obj.name || 'ECG',
      start: typeof obj.start === 'number' ? obj.start : obj.start ? Date.parse(obj.start) / 1000 : null,
      fs,
      leads: obj.leads.map((l) => {
        const k = toMv(l.units);
        return { name: l.name, units: 'mV', fs: l.fs ?? fs, v: Float64Array.from(l.samples, (x) => (x === null ? NaN : x * k)) };
      }),
      meta: cleanMeta(obj.meta),
    });
  }

  // ---------------------------------------------------------------- leads
  const STANDARD = ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'];

  /** The standard name of a lead label ("ECG II", "avr", "MLII" → "II", "aVR", null). */
  function standardLead(label) {
    const l = String(label).trim().replace(/^(ecg|ekg|lead)[\s_-]*/i, '').replace(/\s+/g, '');
    for (const s of STANDARD) if (l.toLowerCase() === s.toLowerCase()) return s;
    if (/^mlii$/i.test(l)) return 'II';
    return null;
  }

  /** The record's leads by standard name, when it has all twelve. */
  function twelveLead(rec) {
    const m = {};
    for (const lead of rec.leads) {
      const s = standardLead(lead.name);
      if (s && !m[s]) m[s] = lead;
    }
    return STANDARD.every((s) => m[s]) ? m : null;
  }

  // ---------------------------------------------------------------- de-identified
  const MON = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC'];
  const pad = (n, w = 2) => String(n).padStart(w, '0');
  /** Text with its dates moved back `shift` seconds: 2024-03-12[ 14:00[:05]], 12-MAR-2024. */
  function shiftText(text, shift) {
    if (!shift || typeof text !== 'string') return text;
    return text
      .replace(/(?<![\w-])(\d{4})-(\d{2})-(\d{2})(?:([ T])(\d{2}):(\d{2})(?::(\d{2}))?)?(?!\d)/g, (m, y, mo, d, sep, h, mi, se) => {
        if (+mo < 1 || +mo > 12 || +d < 1 || +d > 31) return m;
        const x = new Date(Date.UTC(+y, +mo - 1, +d, +(h || 0), +(mi || 0), +(se || 0)) - shift * 1000);
        const day = `${pad(x.getUTCFullYear(), 4)}-${pad(x.getUTCMonth() + 1)}-${pad(x.getUTCDate())}`;
        return sep ? `${day}${sep}${pad(x.getUTCHours())}:${pad(x.getUTCMinutes())}${se !== undefined ? `:${pad(x.getUTCSeconds())}` : ''}` : day;
      })
      .replace(/(?<![\w-])(\d{1,2})-([A-Za-z]{3})-(\d{4})(?!\d)/g, (m, d, mon, y) => {
        const mo = MON.indexOf(mon.toUpperCase());
        if (mo < 0) return m;
        const x = new Date(Date.UTC(+y, mo, +d) - shift * 1000);
        return `${pad(x.getUTCDate())}-${MON[x.getUTCMonth()]}-${x.getUTCFullYear()}`;
      });
  }

  /** An age above 89 years as 90+: in years unless its unit says months, weeks, days or hours ("1095 days", DICOM's "095M"). */
  const PER_YEAR = [[/^(mo|mos|months?|m)$/i, 12], [/^(w|wk|wks|weeks?)$/i, 52.18], [/^(d|days?)$/i, 365.25], [/^(h|hrs?|hours?)$/i, 8766]];
  function cap90(v) {
    const m = /^\s*(\d+(?:\.\d+)?)\s*([a-z]*)/i.exec(String(v));
    if (!m) return v;
    const per = PER_YEAR.find(([re]) => re.test(m[2]))?.[1] || 1;
    return +m[1] / per > 89 ? '90+' : v;
  }
  const AGE = /^<?\s*age\s*>?(\s*\((years?|y)\))?$/i;

  /** The report as a de-identified view shows it: dates moved back, ages above 89 as 90+. */
  function deidMeta(meta, shift, cap) {
    if (!meta || (!shift && !cap)) return meta;
    const s = (t) => shiftText(t, shift);
    return {
      fields: meta.fields.map(([k, v]) => [k, cap && AGE.test(k.trim()) ? cap90(v) : s(v)]),
      measurements: meta.measurements,
      statements: meta.statements.map((x) => ({ ...x, time: s(x.time), lines: x.lines.map(s) })),
      notes: meta.notes.map(s),
    };
  }

  /** The record's start as the page prints it ("2024-03-12 14:00:05"), moved back `shift` seconds (a de-identified view), or ''. */
  function startText(start, shift = 0) {
    if (typeof start !== 'number' || !Number.isFinite(start)) return '';
    const d = new Date((start - (shift || 0)) * 1000);
    return Number.isNaN(d.getTime()) ? '' : d.toISOString().replace('T', ' ').slice(0, 19);
  }

  const api = { shiftText, deidMeta, startText, readJson, standardLead, twelveLead, STANDARD, metaFromLines, cleanMeta, checked, toMv, MEASURES };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.ECG = api;
})(typeof self !== 'undefined' ? self : this);
