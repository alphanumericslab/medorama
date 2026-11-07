// DICOM files made in the tests: elements, sequences, Part 10 files (explicit or implicit VR
// little endian).  Everything is synthetic.
'use strict';

const LONG = new Set(['OB', 'OW', 'SQ', 'UN', 'UT']);

/** One element; `value` is a Buffer, a number (US, UL, FD, SS) or text. */
function el(g, e, vr, value, explicit = true) {
  let data;
  if (Buffer.isBuffer(value)) data = value;
  else if (vr === 'US') { data = Buffer.alloc(2); data.writeUInt16LE(value); }
  else if (vr === 'SS') { data = Buffer.alloc(2); data.writeInt16LE(value); }
  else if (vr === 'UL') { data = Buffer.alloc(4); data.writeUInt32LE(value); }
  else if (vr === 'FD') { data = Buffer.alloc(8); data.writeDoubleLE(value); }
  else { data = Buffer.from(String(value), 'latin1'); if (data.length % 2) data = Buffer.concat([data, Buffer.from(vr === 'UI' ? '\0' : ' ')]); }
  const tag = Buffer.alloc(4);
  tag.writeUInt16LE(g); tag.writeUInt16LE(e, 2);
  if (!explicit) {
    const len = Buffer.alloc(4); len.writeUInt32LE(data.length);
    return Buffer.concat([tag, len, data]);
  }
  if (LONG.has(vr)) {
    const h = Buffer.alloc(8); h.write(vr, 0, 'latin1'); h.writeUInt32LE(data.length, 4);
    return Buffer.concat([tag, h, data]);
  }
  const h = Buffer.alloc(4); h.write(vr, 0, 'latin1'); h.writeUInt16LE(data.length, 2);
  return Buffer.concat([tag, h, data]);
}

/** An item of defined length. */
function item(...els) {
  const body = Buffer.concat(els);
  const h = Buffer.alloc(8); h.writeUInt16LE(0xfffe); h.writeUInt16LE(0xe000, 2); h.writeUInt32LE(body.length, 4);
  return Buffer.concat([h, body]);
}

/** A sequence of defined length. */
const sq = (g, e, items, explicit = true) => el(g, e, 'SQ', Buffer.concat(items), explicit);

/** A coded concept: (0008,0100) value, (0008,0102) scheme, (0008,0104) meaning, in a sequence. */
const coded = (g, e, value, scheme, meaning, explicit = true) =>
  sq(g, e, [item(el(0x0008, 0x0100, 'SH', value, explicit), el(0x0008, 0x0102, 'SH', scheme, explicit), el(0x0008, 0x0104, 'LO', meaning, explicit))], explicit);

function part10(transfer, body, sop = '') {
  const meta = Buffer.concat([el(0x0002, 0x0001, 'OB', Buffer.from([0, 1])), ...(sop ? [el(0x0002, 0x0002, 'UI', sop)] : []), el(0x0002, 0x0010, 'UI', transfer)]);
  return Buffer.concat([Buffer.alloc(128), Buffer.from('DICM'), el(0x0002, 0x0000, 'UL', meta.length), meta, body]);
}

/** Encapsulated pixel data: an empty offset table, then one fragment per frame. */
function encapsulated(frames) {
  const head = Buffer.alloc(12);
  head.writeUInt16LE(0x7fe0); head.writeUInt16LE(0x0010, 2); head.write('OB', 4, 'latin1'); head.writeUInt32LE(0xffffffff, 8);
  const frag = (b) => { const h = Buffer.alloc(8); h.writeUInt16LE(0xfffe); h.writeUInt16LE(0xe000, 2); h.writeUInt32LE(b.length, 4); return Buffer.concat([h, b]); };
  const end = Buffer.alloc(8); end.writeUInt16LE(0xfffe); end.writeUInt16LE(0xe0dd, 2);
  return Buffer.concat([head, frag(Buffer.alloc(0)), ...frames.map(frag), end]);
}

/** RLE (PackBits) segments of a frame: one segment per byte plane, most significant first. */
function rle(planes) {
  const enc = (bytes) => {
    // a literal run then a repeat, to test both kinds
    const out = [];
    let i = 0;
    while (i < bytes.length) {
      let j = i;
      while (j + 1 < bytes.length && bytes[j + 1] === bytes[i] && j - i < 127) j++;
      if (j > i) { out.push((1 - (j - i + 1)) & 0xff, bytes[i]); i = j + 1; continue; }
      const n = Math.min(128, bytes.length - i);
      let k = i;
      while (k < i + n && !(k + 1 < bytes.length && bytes[k + 1] === bytes[k])) k++;
      const len = Math.max(1, k - i);
      out.push(len - 1, ...bytes.slice(i, i + len));
      i += len;
    }
    if (out.length % 2) out.push(0x80);
    return Buffer.from(out);
  };
  const segs = planes.map(enc);
  const header = Buffer.alloc(64);
  header.writeUInt32LE(segs.length);
  let off = 64;
  segs.forEach((s, k) => { header.writeUInt32LE(off, 4 + 4 * k); off += s.length; });
  return Buffer.concat([header, ...segs]);
}

/** A 12-lead ECG waveform (two leads here), 500 Hz, 2.5 µV per unit, with annotations. */
function ecgFile(explicit) {
  const x = explicit;
  const n = 5;
  const data = Buffer.alloc(n * 2 * 2);
  for (let i = 0; i < n; i++) { data.writeInt16LE(i * 100, 4 * i); data.writeInt16LE(-i * 40, 4 * i + 2); }
  const channel = (name) => item(
    sq(0x003a, 0x0208, [item(el(0x0008, 0x0100, 'SH', `5.6.3-9-${name}`, x), el(0x0008, 0x0102, 'SH', 'SCPECG', x), el(0x0008, 0x0104, 'LO', `Lead ${name}`, x))], x),
    el(0x003a, 0x0210, 'DS', '2.5', x), coded(0x003a, 0x0211, 'uV', 'UCUM', 'microvolt', x), el(0x003a, 0x0212, 'DS', '1', x), el(0x003a, 0x0213, 'DS', '0', x),
  );
  const group = item(
    el(0x003a, 0x0005, 'US', 2, x), el(0x003a, 0x0010, 'UL', n, x), el(0x003a, 0x001a, 'DS', '500', x), el(0x003a, 0x0020, 'SH', 'RHYTHM', x),
    sq(0x003a, 0x0200, [channel('I'), channel('II')], x),
    el(0x5400, 0x1004, 'US', 16, x), el(0x5400, 0x1006, 'CS', 'SS', x), el(0x5400, 0x1010, 'OW', data, x),
  );
  const qt = item(coded(0x0040, 0xa043, '8634-8', 'LN', 'QT interval', x), el(0x0040, 0xa30a, 'DS', '412', x), coded(0x0040, 0x08ea, 'ms', 'UCUM', 'ms', x));
  const statement = item(el(0x0070, 0x0006, 'ST', 'Sinus rhythm', x));
  return part10(x ? '1.2.840.10008.1.2.1' : '1.2.840.10008.1.2', Buffer.concat([
    el(0x0008, 0x0016, 'UI', '1.2.840.10008.5.1.4.1.1.9.1.1', x), el(0x0008, 0x0060, 'CS', 'ECG', x), el(0x0010, 0x0010, 'PN', 'Doe^Jane', x),
    sq(0x0040, 0xb020, [qt, statement], x),
    sq(0x5400, 0x0100, [group], x),
  ]));
}

module.exports = { el, item, sq, coded, part10, encapsulated, rle, ecgFile };
