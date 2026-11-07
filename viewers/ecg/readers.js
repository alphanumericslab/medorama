// The ECG viewer's readers: one registry of the file formats it opens, each format with its own
// reader, and every reader giving the same record.
// No dependencies (ecg.js reads the JSON form); a classic script (it works from file://), and a
// CommonJS module for tests.
//
// THE RECORD
//   Whatever the file's format, once read an ECG is one record:
//     { name, start, fs, leads: [{ name, units, fs, v }], meta }
//   - name: what the record calls itself (a WFDB record's name, "MUSE ECG"…); the page shows the
//     file's name, not this.
//   - start: when it was recorded, in Unix seconds (UTC), or null when the file does not say.
//   - fs: its sampling rate (Hz); each lead may state its own `fs` (a WFDB signal with several
//     samples per frame).
//   - leads: in the file's order; `name` is the lead's label (I, II, aVR, V1, "ECG II"…; the
//     twelve standard leads are found by name), `units` is 'mV', and `v` is a Float64Array of
//     millivolts, NaN where a sample is missing.
//   - meta (null, or each of its parts optional): what the page prints above the paper:
//       { fields: [[label, text]], measurements: [{ name, value, unit, lead? }],
//         statements: [{ source: 'computer' | 'confirmed' | 'edited', program?, by?, time?, lines: [text] }],
//         notes: [text] }
//   Patient identity is never part of a record: no name, identifier or birth date, in any field.
//   ECG.checked (ecg.js) refuses a record the paper cannot draw (no leads, no samples, no rate).
//
// THE VIEWER'S JSON FORM
//   A record as JSON, which the viewer opens like any other file (.json):
//     { name, start, fs, leads: [{ name, fs?, units, samples: [number | null] }], meta? }
//   with `start` in Unix seconds or as ISO 8601 text (or null), `fs` the record's rate (or each
//   lead's own), `units` 'mV', 'uV' (µV) or 'V', null for a missing sample, and `meta` as above
//   (checked: unknown parts and identifying fields are dropped).  The MUSE and DICOM readers
//   give this form, which ecg.js turns into the record.  A reader written in another language
//   (the Python MUSE reader, python/muse, writes it) can convert a format the page does not read
//   into this form, and the page opens the result like any other ECG.
//
// A READER
//   Each format's file (wfdb.js, muse.js, dicom-ecg.js, edf.js…) adds its reader with
//   Readers.add({ ... }):
//     key          a short identifier: 'wfdb', 'muse', 'edf'
//     label        what it reads, as the page lists it: 'GE MUSE XML'
//     extensions   the file name extensions it reads, lower case, without the dot: ['xml']
//     sniff(name, bytes, type) -> bool
//                  whether the file's content (and its media type, when known) is this format
//     read(name, bytes, sibling) -> Promise<record>
//                  the record; an Error (its message is shown) when the file cannot be read.
//                  `sibling(fileName) -> Promise<ArrayBuffer>` gives a file next to this one
//                  (a WFDB header's signal files).
//   and, optionally:
//     companions   extensions of the files read with it, not opened alone: WFDB's ['dat']
//     parts(name, bytes) -> [file names]
//                  the files of a folder that belong to this record and are not ECGs of their
//                  own (a multi-segment WFDB record's segment headers)
//     loose        true when its sniff is a guess (WFDB's: a line of text): tried after the others
//   `bytes` is an ArrayBuffer or a typed array of the whole file; `type` its media type, or ''.
//   The registry itself holds the page's own kinds, after the added ones: the JSON form above,
//   and the files shown as they are (pictures, PDF), which are not records: they have `shown`
//   ('picture' or 'pdf') in place of `read`.  The readers are added in the order of the scripts
//   of index.html, which is also the order the page lists them in.
//
// WHAT A FILE IS
//   Readers.find(name, bytes, type): the reader that claims the file's extension (when several
//   do, the first whose sniff says yes); else the first whose sniff says yes, in order (loose
//   ones last); else null, and the page says it reads Readers.labels().  A folder's ECGs are
//   the files whose extension a reader claims (Readers.byName), less their parts.
//
// ADDING A FORMAT
//   Write its reader in its own file: it maps the format to the record above (ecg.js has the
//   helpers: units to mV, metadata from "key: value" lines, ECG.checked), and never reads the
//   patient's identity.  End the file with Readers.add, load it in index.html after readers.js,
//   list it in Medorama's built-in page files (medorama/src/server.rs, page!("ecg", …)), and
//   test it in tests/ (tests/readers.test.js: what the registry finds).

(function (root) {
  'use strict';

  const added = [];
  let own = [];   // the page's own kinds (below), after the added ones
  const all = () => [...added, ...own];

  /** A file name's extension, lower case ('' without one). */
  const ext = (name) => (/\.([a-z0-9]+)$/i.exec(name || '') || [])[1]?.toLowerCase() || '';

  /** The bytes as a Uint8Array (no copy). */
  const u8 = (b) => (b instanceof Uint8Array ? b : ArrayBuffer.isView(b) ? new Uint8Array(b.buffer, b.byteOffset, b.byteLength) : new Uint8Array(b || 0));

  /** Bytes [a, b) of a file as Latin-1 text (signatures, headers). */
  const latin1 = (bytes, a, b) => String.fromCharCode(...u8(bytes).subarray(a, b));

  function valid(r) {
    if (!r || !r.key || !r.label || !Array.isArray(r.extensions) || typeof r.sniff !== 'function') throw new Error('a reader needs key, label, extensions and sniff');
    if (typeof r.read !== 'function' && !r.shown) throw new Error(`the reader ${r.key} needs read (or shown)`);
    return r;
  }

  /** Add a reader (see above); one with the key of another replaces it. */
  function add(r) {
    valid(r);
    const i = added.findIndex((e) => e.key === r.key);
    if (i >= 0) added[i] = r; else added.push(r);
    return r;
  }

  /** The readers claiming a file name's extension, in order. */
  const claiming = (name) => { const e = ext(name); return e ? all().filter((r) => r.extensions.includes(e)) : []; };

  /** The reader of a file from its name alone (its extension), or null. */
  const byName = (name) => claiming(name)[0] || null;

  /** The reader of a file, or null: by its extension, else by its content. */
  function find(name, bytes, type) {
    const sniff = (r) => { try { return !!r.sniff(name, bytes || new Uint8Array(), type || ''); } catch { return false; } };
    const mine = claiming(name);
    if (mine.length) return mine.find(sniff) || mine[0];
    if (!bytes && !type) return null;
    const rs = all();
    return rs.filter((r) => !r.loose).find(sniff) || rs.filter((r) => r.loose).find(sniff) || null;
  }

  /** What the page reads, in order. */
  const labels = () => all().map((r) => r.label);

  /** For a file chooser: every extension read, with the companions ('.hea,.dat,…'). */
  const accept = () => [...new Set(all().flatMap((r) => [...r.extensions, ...(r.companions || [])]))].map((e) => `.${e}`).join(',');

  // ---------------------------------------------------------------- the page's own kinds
  const ECG = () => (typeof module !== 'undefined' && module.exports ? require('./ecg.js') : root.ECG);
  own = [
    {
      key: 'json', label: 'the viewer\'s JSON form', extensions: ['json'],
      sniff: (name, bytes, type) => /json/.test(type),
      read: async (name, bytes) => ECG().readJson(JSON.parse(new TextDecoder().decode(u8(bytes)))),
    },
    {
      key: 'picture', label: 'pictures (PNG, JPEG)', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp'], shown: 'picture',
      sniff: (name, bytes, type) => {
        const h = u8(bytes);
        return /^image\//.test(type) || latin1(bytes, 0, 4) === '\x89PNG' || (h[0] === 0xff && h[1] === 0xd8);
      },
    },
    {
      key: 'pdf', label: 'PDF', extensions: ['pdf'], shown: 'pdf',
      sniff: (name, bytes) => latin1(bytes, 0, 4) === '%PDF',
    },
  ].map(valid);

  const api = { add, find, byName, labels, accept, all, ext, u8, latin1 };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Readers = api;
})(typeof self !== 'undefined' ? self : this);
