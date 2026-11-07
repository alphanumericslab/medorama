// node --test viewers/pacs/tests/*.test.js — RLE images, structured reports, encapsulated
// documents and waveforms, in synthetic DICOM files.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const Dicom = require('../dicom.js');
const { el, item, sq, coded, part10, encapsulated, rle, ecgFile } = require('./make.js');

const EXPLICIT = '1.2.840.10008.1.2.1';
const IMPLICIT = '1.2.840.10008.1.2';

test('RLE: 16-bit grey and 8-bit RGB frames', () => {
  const rows = 2; const cols = 3;
  const vals = [0, 1, 300, 300, 300, 65535];
  const hi = vals.map((v) => v >> 8);
  const lo = vals.map((v) => v & 0xff);
  const body = Buffer.concat([
    el(0x0008, 0x0060, 'CS', 'US'),
    el(0x0028, 0x0002, 'US', 1), el(0x0028, 0x0004, 'CS', 'MONOCHROME2'), el(0x0028, 0x0008, 'IS', '2'),
    el(0x0028, 0x0010, 'US', rows), el(0x0028, 0x0011, 'US', cols), el(0x0028, 0x0100, 'US', 16), el(0x0028, 0x0101, 'US', 16),
    encapsulated([rle([hi, lo]), rle([hi, lo].map((p) => p.map((x) => 255 - x)))]),
  ]);
  const info = Dicom.image(part10('1.2.840.10008.1.2.5', body));
  assert.ok(info.rle);
  assert.deepStrictEqual([...Dicom.nativeFrame(info, 0)], vals);
  assert.deepStrictEqual([...Dicom.nativeFrame(info, 1)], vals.map((v) => 65535 - v));
  const rgb = Buffer.concat([
    el(0x0028, 0x0002, 'US', 3), el(0x0028, 0x0004, 'CS', 'RGB'), el(0x0028, 0x0006, 'US', 1),
    el(0x0028, 0x0010, 'US', 1), el(0x0028, 0x0011, 'US', 2), el(0x0028, 0x0100, 'US', 8), el(0x0028, 0x0101, 'US', 8),
    encapsulated([rle([[10, 20], [30, 40], [50, 60]])]),
  ]);
  const ci = Dicom.image(part10('1.2.840.10008.1.2.5', rgb));
  assert.deepStrictEqual([...Dicom.nativeFrame(ci, 0)], [10, 30, 50, 20, 40, 60], 'RGB, interleaved');
});

test('compressed syntaxes browsers do not decode: said so, by name', () => {
  const body = Buffer.concat([el(0x0028, 0x0010, 'US', 1), el(0x0028, 0x0011, 'US', 1), encapsulated([Buffer.alloc(4)])]);
  assert.throws(() => Dicom.image(part10('1.2.840.10008.1.2.4.90', body)), (e) => e.unsupported && /JPEG 2000/.test(e.message));
});

function echoReport(explicit) {
  const x = explicit;
  const num = (code, meaning, v, unit) => item(
    el(0x0040, 0xa010, 'CS', 'CONTAINS', x), el(0x0040, 0xa040, 'CS', 'NUM', x), coded(0x0040, 0xa043, code, 'LN', meaning, x),
    sq(0x0040, 0xa300, [item(el(0x0040, 0xa30a, 'DS', String(v), x), coded(0x0040, 0x08ea, unit, 'UCUM', unit, x))], x),
  );
  const section = item(
    el(0x0040, 0xa010, 'CS', 'CONTAINS', x), el(0x0040, 0xa040, 'CS', 'CONTAINER', x), coded(0x0040, 0xa043, '29468-8', 'LN', 'Left Ventricle', x),
    sq(0x0040, 0xa730, [num('18043-0', 'Left Ventricular Ejection Fraction', 38, '%'), num('29436-5', 'Left Ventricle Internal End Diastolic Dimension', 6.1, 'cm')], x),
  );
  const finding = item(el(0x0040, 0xa010, 'CS', 'CONTAINS', x), el(0x0040, 0xa040, 'CS', 'TEXT', x), coded(0x0040, 0xa043, '121071', 'DCM', 'Finding', x),
    el(0x0040, 0xa160, 'UT', 'Dilated LV, reduced function', x));
  const observer = item(el(0x0040, 0xa010, 'CS', 'HAS OBS CONTEXT', x), el(0x0040, 0xa040, 'CS', 'PNAME', x), coded(0x0040, 0xa043, '121008', 'DCM', 'Person Observer Name', x),
    el(0x0040, 0xa123, 'PN', 'Reader^Test', x));
  return Buffer.concat([
    el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.88.33', x), el(0x0008, 0x0060, 'CS', 'SR', x),
    el(0x0010, 0x0010, 'PN', 'Doe^Jane', x), el(0x0020, 0x000d, 'UI', '1.2.3', x),
    el(0x0040, 0xa040, 'CS', 'CONTAINER', x), coded(0x0040, 0xa043, '125000', 'DCM', 'OB-GYN Ultrasound Procedure Report', x),
    sq(0x0040, 0xa730, [observer, section, finding], x),
  ]);
}

test('a structured report: measurements with units, text, no names (explicit and implicit VR)', () => {
  for (const [transfer, x] of [[EXPLICIT, true], [IMPLICIT, false]]) {
    const f = part10(transfer, echoReport(x));
    assert.strictEqual(Dicom.kind(f).kind, 'sr');
    const r = Dicom.report(f);
    assert.strictEqual(r.items.find((i) => i.type === 'CONTAINER').name, 'Left Ventricle');
    assert.deepStrictEqual(Dicom.measurements(r), [
      { name: 'Left Ventricular Ejection Fraction', value: 38, unit: '%' },
      { name: 'Left Ventricle Internal End Diastolic Dimension', value: 6.1, unit: 'cm' },
    ]);
    assert.strictEqual(r.items.find((i) => i.type === 'TEXT').value, 'Dilated LV, reduced function');
    assert.ok(!JSON.stringify(r).includes('Reader') && !JSON.stringify(r).includes('Doe'), 'no names');
  }
});

test('an encapsulated PDF', () => {
  const pdf = Buffer.from('%PDF-1.4\n%%EOF\n');      // odd length: padded
  const f = part10(EXPLICIT, Buffer.concat([
    el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.104.1'), el(0x0042, 0x0010, 'ST', 'Echo report'),
    el(0x0042, 0x0011, 'OB', Buffer.concat([pdf, Buffer.alloc(1)])), el(0x0042, 0x0012, 'LO', 'application/pdf'),
  ]));
  assert.strictEqual(Dicom.kind(f).kind, 'pdf');
  const d = Dicom.documentOf(f);
  assert.strictEqual(d.title, 'Echo report');
  assert.strictEqual(Buffer.from(d.bytes).toString(), pdf.toString());
});

test('an encapsulated document: shown only as a PDF, whatever type it states', () => {
  const doc = (mime) => Dicom.documentOf(part10(EXPLICIT, Buffer.concat([
    el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.104.1'),
    el(0x0042, 0x0011, 'OB', Buffer.from('<script>fetch("files/")</script>')), ...(mime ? [el(0x0042, 0x0012, 'LO', mime)] : []),
  ])));
  for (const mime of ['text/html', 'image/svg+xml', 'text/xml', 'application/pdf\\text/html', 'application/pdfx']) assert.strictEqual(doc(mime).pdf, false, mime);
  for (const mime of ['application/pdf', 'APPLICATION/PDF', '']) assert.strictEqual(doc(mime).pdf, true, mime);
  // the page builds the document's address as application/pdf, never with the stated type
  const app = fs.readFileSync(path.join(__dirname, '..', 'app.js'), 'utf8');
  assert.ok(!/blobUrl\(d\.bytes, d\.mime\)/.test(app) && /if \(d\.pdf\) items\.push\(.*blobUrl\(d\.bytes, 'application\/pdf'\)/.test(app));
});

test('frames: no more than the pixel data holds, whatever the header says', () => {
  const head = (frames) => [
    el(0x0008, 0x0060, 'CS', 'US'), el(0x0028, 0x0002, 'US', 1), el(0x0028, 0x0004, 'CS', 'MONOCHROME2'), el(0x0028, 0x0008, 'IS', frames),
    el(0x0028, 0x0010, 'US', 2), el(0x0028, 0x0011, 'US', 2), el(0x0028, 0x0100, 'US', 8), el(0x0028, 0x0101, 'US', 8),
  ];
  const native = (frames, bytes) => Dicom.image(part10(EXPLICIT, Buffer.concat([...head(frames), el(0x7fe0, 0x0010, 'OB', Buffer.alloc(bytes))])));
  assert.strictEqual(native('2000000000', 12).frames, 3);        // 3 frames of 4 bytes
  assert.strictEqual(native('2', 12).frames, 2);
  assert.strictEqual(native('-5', 12).frames, 1);
  assert.strictEqual(native('x', 12).frames, 1);
  assert.strictEqual(native('3', 2).frames, 1);                   // short: one (damaged) frame, an error when drawn
  const rle = Dicom.image(part10('1.2.840.10008.1.2.5', Buffer.concat([...head('2000000000'), encapsulated([Buffer.alloc(64), Buffer.alloc(64)])])));
  assert.strictEqual(rle.frames, 2);
  assert.strictEqual(rle.encapsulated.length, 2);
});

test('a structured report: the subject context (who the patient is) is left out', () => {
  const x = true;
  const ctx = (type, codeValue, scheme, meaning, valueEl) => item(el(0x0040, 0xa010, 'CS', 'HAS OBS CONTEXT', x), el(0x0040, 0xa040, 'CS', type, x),
    coded(0x0040, 0xa043, codeValue, scheme, meaning, x), valueEl);
  const f = part10(EXPLICIT, Buffer.concat([
    el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.88.33', x), el(0x0008, 0x0060, 'CS', 'SR', x),
    el(0x0040, 0xa040, 'CS', 'CONTAINER', x), coded(0x0040, 0xa043, '125000', 'DCM', 'OB-GYN Ultrasound Procedure Report', x),
    sq(0x0040, 0xa730, [
      ctx('TEXT', '121030', 'DCM', 'Subject ID', el(0x0040, 0xa160, 'UT', 'SYN-ID-1', x)),
      ctx('DATE', '121031', 'DCM', 'Subject Birth Date', el(0x0040, 0xa121, 'DA', '19500101', x)),
      ctx('TEXT', '121028', 'DCM', 'Subject UID', el(0x0040, 0xa160, 'UT', 'SYN-UID-2', x)),
      ctx('TEXT', '121022', 'DCM', 'Accession Number', el(0x0040, 0xa160, 'UT', 'SYN-ACC-3', x)),
      ctx('TEXT', 'P1', '99LOCAL', 'Patient identifier', el(0x0040, 0xa160, 'UT', 'SYN-MRN-4', x)),
      ctx('TEXT', '121071', 'DCM', 'Finding', el(0x0040, 0xa160, 'UT', 'Normal study', x)),
      ctx('CODE', '121032', 'DCM', 'Subject Sex', coded(0x0040, 0xa168, 'F', 'DCM', 'Female', x)),
    ], x),
  ]));
  const r = JSON.stringify(Dicom.report(f));
  for (const v of ['SYN-ID-1', '19500101', 'SYN-UID-2', 'SYN-ACC-3', 'SYN-MRN-4']) assert.ok(!r.includes(v), v);
  assert.ok(r.includes('Normal study') && r.includes('Female'));
});

test('a DICOM ECG: channels scaled to their units, lead names, annotations', () => {
  for (const x of [true, false]) {
    const f = ecgFile(x);
    assert.strictEqual(Dicom.kind(f).kind, 'ecg');
    const w = Dicom.waveforms(f);
    assert.strictEqual(w.groups.length, 1);
    const g = w.groups[0];
    assert.deepStrictEqual([g.label, g.fs, g.channels.map((c) => c.name), g.channels[0].units], ['RHYTHM', 500, ['I', 'II'], 'uV']);
    assert.deepStrictEqual([...g.channels[0].samples], [0, 250, 500, 750, 1000]);
    assert.deepStrictEqual([...g.channels[1].samples], [0, -100, -200, -300, -400]);
    assert.deepStrictEqual(w.annotations, { measurements: [{ name: 'QT interval', value: 412, unit: 'ms' }], text: ['Sinus rhythm'] });
  }
});

test('the ECG viewer carries the same reader', () => {
  const a = fs.readFileSync(path.join(__dirname, '..', 'dicom.js'), 'utf8');
  const b = fs.readFileSync(path.join(__dirname, '..', '..', 'ecg', 'dicom.js'), 'utf8');
  assert.strictEqual(a, b, 'viewers/ecg/dicom.js must be a copy of viewers/pacs/dicom.js');
});

