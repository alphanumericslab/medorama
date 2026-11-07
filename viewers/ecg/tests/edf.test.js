// node --test viewers/ecg/tests/ — EDF, EDF+, BDF and BDF+ files (edf.js) as ECG records, from
// synthetic files written here (edfFile, after viewers/vitalstream/testdata/generate.py write_edf).
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const ECG = require('../ecg.js');
const EDF = require('../edf.js');
const Readers = require('../readers.js');
const Paper = require('../paper.js');

const LEADS = ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'];
const T0 = Date.UTC(2024, 2, 1, 13, 0, 0) / 1000;   // 2024-03-01 13:00:00
const MON = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC'];

/** An EDF header field: left-aligned, padded with spaces. */
function fld(text, width) {
  const b = Buffer.from(String(text), 'latin1');
  assert.ok(b.length <= width, text);
  return Buffer.concat([b, Buffer.alloc(width - b.length, 0x20)]);
}

/** An EDF+ time-stamped annotation list; the time-keeping one has no text. */
function tal(onset, texts = [''], duration = null) {
  const head = `${onset >= 0 ? '+' : ''}${onset}${duration !== null ? `\x15${duration}` : ''}`;
  return Buffer.concat([Buffer.from(head, 'latin1'), Buffer.from([0x14]), ...texts.map((t) => Buffer.concat([Buffer.from(t, 'utf8'), Buffer.from([0x14])])), Buffer.from([0])]);
}

/**
 * An EDF, EDF+ or BDF file.  signals: [{ label, unit, pmin, pmax, dmin, dmax, spr, f(t) }];
 * onsets: each data record's start, in seconds after `start`; version: 'EDF+C', 'EDF+D', 'BDF+C'
 * (EDF+ and BDF+ files get an annotation signal whose time-keeping annotations state the onsets);
 * notes: { record: [[onset, duration or null, text]] }; nRecords: the count in the header (-1:
 * unknown); extra: bytes after the last record (a record cut short).
 */
function edfFile({ start = T0, duration = 1, signals, onsets, notes = {}, version = '', nRecords, bdf = false, patient = 'X X X X', recording, extra = Buffer.alloc(0) }) {
  const d = new Date(start * 1000);
  const p2 = (n) => String(n).padStart(2, '0');
  const width = bdf ? 3 : 2;
  const sigs = [...signals];
  if (version) {
    const [lo, hi] = bdf ? [-8388608, 8388607] : [-32768, 32767];
    sigs.push({ label: bdf ? 'BDF Annotations' : 'EDF Annotations', unit: '', pmin: -1, pmax: 1, dmin: lo, dmax: hi, spr: 30, f: null });
  }
  const rec = recording ?? (version ? `Startdate ${p2(d.getUTCDate())}-${MON[d.getUTCMonth()]}-${d.getUTCFullYear()} X X X` : 'Synthetic test file');
  const parts = [
    bdf ? Buffer.from([0xff, ...Buffer.from('BIOSEMI')]) : fld('0', 8), fld(patient, 80), fld(rec, 80),
    fld(`${p2(d.getUTCDate())}.${p2(d.getUTCMonth() + 1)}.${p2(d.getUTCFullYear() % 100)}`, 8),
    fld(`${p2(d.getUTCHours())}.${p2(d.getUTCMinutes())}.${p2(d.getUTCSeconds())}`, 8),
    fld(256 * (sigs.length + 1), 8), fld(version || (bdf ? '24BIT' : ''), 44),
    fld(nRecords ?? onsets.length, 8), fld(duration, 8), fld(sigs.length, 4),
  ];
  for (const [get, w] of [[(s) => s.label, 16], [() => '', 80], [(s) => s.unit, 8], [(s) => s.pmin, 8], [(s) => s.pmax, 8],
    [(s) => s.dmin, 8], [(s) => s.dmax, 8], [(s) => s.prefilter || '', 80], [(s) => s.spr, 8], [() => '', 32]]) {
    for (const s of sigs) parts.push(fld(get(s), w));
  }
  onsets.forEach((onset, r) => {
    for (const s of sigs) {
      if (!s.f) {
        const b = Buffer.concat([tal(onset), ...(notes[r] || []).map(([o, du, t]) => tal(o, [t], du))]);
        assert.ok(b.length <= s.spr * width);
        parts.push(b, Buffer.alloc(s.spr * width - b.length));
        continue;
      }
      const out = Buffer.alloc(s.spr * width);
      for (let i = 0; i < s.spr; i++) {
        const v = s.f(onset + (i * duration) / s.spr);
        const q = s.dmax === s.dmin ? Math.round(v)
          : Math.max(s.dmin, Math.min(s.dmax, Math.round(((v - s.pmin) * (s.dmax - s.dmin)) / (s.pmax - s.pmin) + s.dmin)));
        if (bdf) out.writeIntLE(q, 3 * i, 3); else out.writeInt16LE(q, 2 * i);
      }
      parts.push(out);
    }
  });
  parts.push(extra);
  return Buffer.concat(parts);
}

const range = (n, from = 0) => Array.from({ length: n }, (_, i) => from + i);
// a lead's value at time t (mV): its own small sine, so each lead differs
const wave = (j) => (t) => 0.5 * Math.sin(2 * Math.PI * (j + 1) * t);
const twelve = (fs, unit = 'uV') => LEADS.map((l, j) => ({
  label: l, unit, pmin: unit === 'uV' ? -5000 : -5, pmax: unit === 'uV' ? 5000 : 5, dmin: -32768, dmax: 32767, spr: fs, prefilter: 'HP:0.05Hz LP:150Hz',
  f: unit === 'uV' ? (t) => wave(j)(t) * 1000 : wave(j),
}));
const json = (rec) => JSON.stringify({ ...rec, leads: rec.leads.map((l) => ({ ...l, v: Array.from(l.v) })) });

test('EDF+C, twelve leads at 500 Hz in µV: mV, the first 10 s, the annotations within them', async () => {
  const f = edfFile({
    version: 'EDF+C', signals: twelve(500), onsets: range(12),
    notes: { 1: [[1.5, 2, 'Arousal'], [11.2, null, 'past the span']], 3: [[3.25, null, '  two   words ']] },
  });
  const rec = await Readers.find('rest.edf', f, '').read('rest.edf', f);
  assert.strictEqual(rec.fs, 500);
  assert.strictEqual(rec.start, T0);
  assert.deepStrictEqual(rec.leads.map((l) => l.name), LEADS);
  assert.ok(rec.leads.every((l) => l.v.length === 5000 && l.units === 'mV' && l.fs === 500));
  // physical values: pmin + (d - dmin) × (pmax - pmin) / (dmax - dmin), µV to mV
  const q = Math.round(((wave(4)(0.002) * 1000 + 5000) * 65535) / 10000 - 32768);
  assert.strictEqual(rec.leads[4].v[1], (-5000 + (q + 32768) * (10000 / 65535)) / 1000);
  assert.ok(Math.abs(rec.leads[0].v[125] - 0.5) < 1e-3);        // lead I's sine peaks at 0.25 s
  assert.ok(ECG.twelveLead(rec));
  assert.deepStrictEqual(rec.meta.fields, [['Format', 'EDF+'], ['Filters', 'HP:0.05Hz LP:150Hz']]);
  assert.deepStrictEqual(rec.meta.notes, [
    'The file\'s start time is taken as UTC.',
    'The first 10 s of 12 s are shown.',
    '1 annotation signal not drawn: the annotations within this ECG follow.',
    'Annotation at 1.5 s: Arousal (2 s)',
    'Annotation at 3.25 s: two words',
  ]);
  const d = Paper.draw(rec, { layout: '3x4', speed: 25, gain: 10, twelve: ECG.twelveLead(rec) });
  assert.ok(d.svg.includes('>aVR<') && !/NaN|Infinity/.test(d.svg));
  // all of it
  assert.strictEqual(EDF.readEdf(f, 0).leads[0].v.length, 6000);
});

test('the patient and recording identification fields never reach the record', async () => {
  const f = edfFile({
    version: 'EDF+C', signals: twelve(500, 'mV'), onsets: range(10),
    patient: 'MCH-0234567 F 02-MAY-1951 Haagse_Harry',
    recording: 'Startdate 01-MAR-2024 EMR-7788 Dr_Nobody Cart_9000',
  });
  const rec = await Readers.find('x.edf', f).read('x.edf', f);
  const text = json(rec);
  for (const s of ['MCH', '0234567', 'MAY', 'Haagse', 'Harry', 'EMR', '7788', 'Nobody', 'Cart_9000', 'Startdate']) {
    assert.ok(!text.includes(s), `${s} in the record`);
  }
  const meta = JSON.stringify(rec.meta);
  assert.ok(!meta.includes('1951') && !meta.includes('F '));
  assert.strictEqual(rec.start, T0);
  // the header the reader keeps holds none of it either
  assert.ok(!JSON.stringify(EDF.header(f)).includes('Haagse'));
});

test('plain EDF: a two-digit year (1999), the count of records unknown, a record cut short', () => {
  const start = Date.UTC(1999, 2, 2, 10, 0, 0) / 1000;
  const f = edfFile({
    start, duration: 2, nRecords: -1, onsets: [0, 2, 4],
    signals: [{ label: 'ECG', unit: 'mV', pmin: -3.2768, pmax: 3.2767, dmin: -32768, dmax: 32767, spr: 500, f: wave(0) }],
    extra: Buffer.alloc(100, 1),
  });
  const rec = EDF.readEdf(f);
  assert.strictEqual(rec.start, start);
  assert.strictEqual(rec.fs, 250);
  assert.strictEqual(rec.leads[0].v.length, 1500);
  assert.ok(Math.abs(rec.leads[0].v[1] - wave(0)(0.004)) < 1e-4);
  assert.deepStrictEqual(rec.meta.fields, [['Format', 'EDF']]);
  assert.deepStrictEqual(rec.meta.notes, ['The file\'s start time is taken as UTC.', 'The file ends inside a data record: its last 100 bytes are left out.']);
});

test('the header states more records than the file holds', () => {
  const f = edfFile({ nRecords: 12, onsets: range(10), signals: twelve(250) });
  const rec = EDF.readEdf(f);
  assert.strictEqual(rec.leads[0].v.length, 2500);
  assert.ok(rec.meta.notes.includes('The header states 12 data records; the file holds 10.'));
});

test('BDF+ 24-bit: values past 16 bits, a signal whose digital range is zero', () => {
  const f = edfFile({
    bdf: true, version: 'BDF+C', onsets: range(3),
    signals: [
      { label: 'EKG II', unit: 'uV', pmin: -262144, pmax: 262143, dmin: -8388608, dmax: 8388607, spr: 256, f: (t) => 200000 * Math.sin(2 * Math.PI * t) },
      { label: 'Status', unit: '', pmin: 0, pmax: 0, dmin: 0, dmax: 0, spr: 256, f: () => 70000 },
    ],
  });
  const rec = EDF.readEdf(f);
  assert.strictEqual(rec.fs, 256);
  assert.deepStrictEqual(rec.meta.fields, [['Format', 'BDF+']]);
  assert.ok(Math.abs(rec.leads[0].v[64] - 200) < 1e-3, rec.leads[0].v[64]);       // 200 000 µV at a quarter second
  assert.ok(Math.abs(rec.leads[0].v[192] + 200) < 1e-3);
  assert.strictEqual(rec.leads[1].v[0], 70000);                                     // the digital value itself
  assert.strictEqual(Readers.find('eeg.bdf', f).key, 'edf');
});

test('mixed rates: the ECG\'s rate; without an ECG, the most common rate', () => {
  const sig = (label, spr, unit = 'uV') => ({ label, unit, pmin: -3200, pmax: 3200, dmin: -32768, dmax: 32767, spr, f: (t) => 100 * Math.sin(t) });
  const f = edfFile({ version: 'EDF+C', onsets: range(5), signals: [sig('EEG Fp1', 128), sig('Resp', 32, ''), sig('ECG1', 256), sig('EEG C3', 256), sig('SpO2', 1, '%')] });
  const rec = EDF.readEdf(f);
  assert.strictEqual(rec.fs, 256);
  assert.deepStrictEqual(rec.leads.map((l) => l.name), ['ECG1', 'EEG C3']);
  assert.ok(rec.meta.notes.includes('3 signals at another sampling rate than the ECG\'s 256 Hz left out: EEG Fp1 (128 Hz), Resp (32 Hz), SpO2 (1 Hz).'));
  assert.ok(rec.meta.notes.includes('1 annotation signal not drawn: no annotation falls within this ECG.'));
  const g = edfFile({ onsets: range(2), signals: [sig('Resp', 32), sig('Fp1', 128), sig('C3', 128), sig('O1', 128), sig('Pleth', 64)] });
  const r2 = EDF.readEdf(g);
  assert.strictEqual(r2.fs, 128);
  assert.deepStrictEqual(r2.leads.map((l) => l.name), ['Fp1', 'C3', 'O1']);
  assert.ok(r2.meta.notes.includes('2 signals at another sampling rate than the most common, 128 Hz left out: Resp (32 Hz), Pleth (64 Hz).'));
});

test('EDF+D: the first uninterrupted part only, from its own onset', () => {
  const f = edfFile({
    version: 'EDF+D', duration: 0.5, onsets: [0.25, 0.75, 1.25, 5, 5.5, 9], signals: twelve(250),
    notes: { 0: [[0.5, null, 'in the first part']], 3: [[5.2, null, 'in the second part']] },
  });
  const rec = EDF.readEdf(f);
  assert.strictEqual(rec.start, T0 + 0.25);
  assert.strictEqual(rec.fs, 500);
  assert.strictEqual(rec.leads[0].v.length, 750);
  assert.ok(rec.meta.notes.includes('The recording is interrupted (EDF+, 3 parts): only its first part, 1.5 s, is read.'));
  assert.ok(rec.meta.notes.includes('Annotation at 0.25 s: in the first part'));
  assert.ok(!rec.meta.notes.some((n) => n.includes('second part')));
});

test('dates and time-stamped annotation lists, as the engine reads them', () => {
  // EDF+ "Startdate" gives the year in full, even past 2084 (a header year of "yy")
  const f = edfFile({ version: 'EDF+C', onsets: [0], signals: twelve(250), start: Date.UTC(2091, 2, 1, 13) / 1000 });
  f.write('01.03.yy', 168, 'latin1');
  assert.strictEqual(EDF.header(f).start, 3823592400);
  const plain = edfFile({ onsets: [0], signals: twelve(250) });
  plain.write('31.02.24', 168, 'latin1');
  plain.write('25.00.00', 176, 'latin1');
  assert.strictEqual(EDF.header(plain).start, null);
  assert.strictEqual(EDF.readEdf(plain).start, null);
  assert.strictEqual(EDF.readEdf(plain).meta.notes[0], 'The file states no valid start time.');
  const t = EDF.tals(Buffer.from('+0.25\x14\x14\x00+1.5\x152\x14Arousal\x14 two  words \x14\x00\x00\x00', 'latin1'));
  assert.deepStrictEqual(t, [
    { onset: 0.25, duration: null, texts: ['', ''] },
    { onset: 1.5, duration: 2, texts: ['Arousal', 'two words', ''] },
  ]);
});

test('files that are not EDF, or hold nothing to draw, are refused with a message', () => {
  assert.throws(() => EDF.readEdf(Buffer.from('not an EDF file')), /not an EDF or BDF file/);
  const notes = edfFile({ version: 'EDF+C', onsets: range(2), signals: [] });
  assert.throws(() => EDF.readEdf(notes), /only annotations/);
  const none = edfFile({ onsets: [], signals: twelve(250) });
  assert.throws(() => EDF.readEdf(none), /no data records/);
  // every truncation, and random bytes after a valid header: an answer or an Error, quickly
  const f = edfFile({ version: 'EDF+C', onsets: range(2), signals: twelve(100), notes: { 0: [[0.5, null, 'x']] } });
  let seed = 3;
  const rand = () => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed / 0x7fffffff; };
  const cuts = [...range(300), ...range(40).map(() => Math.floor(rand() * f.length))];
  for (const n of cuts) {
    const t = Date.now();
    try { EDF.readEdf(f.subarray(0, n)); } catch (e) { assert.ok(e instanceof Error && !(e instanceof assert.AssertionError), String(e)); }
    assert.ok(Date.now() - t < 1000);
  }
  for (let k = 0; k < 50; k++) {
    const g = Buffer.from(f);
    for (let i = 0; i < 20; i++) g[Math.floor(rand() * g.length)] = Math.floor(rand() * 256);
    try { EDF.readEdf(g); } catch (e) { assert.ok(e instanceof Error && !(e instanceof assert.AssertionError), String(e)); }
  }
});
