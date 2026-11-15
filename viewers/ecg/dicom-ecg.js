// DICOM ECGs for the ECG viewer: a 12-lead, general or ambulatory ECG waveform object, read by
// dicom.js (the PACS viewer's reader, a copy of viewers/pacs/dicom.js), in the viewer's JSON form.
// A DICOM file that is not an ECG is refused: the PACS viewer shows it.
// No dependencies but ecg.js, dicom.js and readers.js; a classic script (it works from file://),
// and a CommonJS module for tests.

(function (root) {
  'use strict';
  const node = typeof module !== 'undefined' && module.exports;
  const ECG = node ? require('./ecg.js') : root.ECG;
  const Dicom = node ? require('./dicom.js') : root.Dicom;
  const Readers = node ? require('./readers.js') : root.Readers;
  const { MEASURES } = ECG;

  /** A channel's units (UCUM `uV`, `mV`, or their meanings) as the JSON form names them; DICOM ECGs are in µV unless said. */
  const dicomUnits = (u) => (/^(mv|millivolt)/i.test(u || '') ? 'mV' : /^(v|volt)$/i.test(u || '') ? 'V' : 'uV');

  // the twelve leads by their codes in the Channel Source Sequence (003A,0208): MDC (ISO/IEEE
  // 11073-10101 MDC_ECG_LEAD_*, partition 2, as "2:1" or as its context-free number 131329) and
  // SCP-ECG ("5.6.3-9-1"; DICOM's CID 3001)
  const BY_CODE = { 1: 'I', 2: 'II', 3: 'V1', 4: 'V2', 5: 'V3', 6: 'V4', 7: 'V5', 8: 'V6', 61: 'III', 62: 'aVR', 63: 'aVL', 64: 'aVF' };
  /** A lead's standard name from its code ({ value, scheme }), or null. */
  function leadOfCode(code) {
    const v = String(code?.value ?? '').trim();
    const scheme = String(code?.scheme ?? '').trim().toUpperCase();
    let n = null;
    if (/^MDC/.test(scheme)) {
      const m = /^2:(\d+)$/.exec(v);
      n = m ? +m[1] : /^\d+$/.test(v) && (+v >> 16) === 2 ? (+v & 0xffff) : null;
    } else if (/^SCPECG/.test(scheme)) {
      const m = /^5\.6\.3-9-(\d+)$/.exec(v);
      n = m ? +m[1] : null;
    }
    return (n !== null && BY_CODE[n]) || null;
  }
  /** A channel's name: the standard lead its code or its label names, else its label. */
  const leadName = (c) => leadOfCode(c.code) || ECG.standardLead(c.name) || c.name;

  /**
   * A DICOM ECG (12-lead, general or ambulatory waveform; read by dicom.js) in the viewer's JSON
   * form: the longest group of channels (the rhythm, not the median beats), its annotations as
   * the report (measurements, statements), the acquisition time.  No identity.
   */
  function dicomJson(w) {
    const groups = w.groups.filter((g) => g.channels.length && g.fs > 0);
    if (!groups.length) throw new Error('no waveform in this DICOM file');
    const g = groups.reduce((a, b) => (b.channels[0].samples.length / b.fs > a.channels[0].samples.length / a.fs ? b : a));
    const measurements = [];
    for (const m of w.annotations.measurements) {
      const known = MEASURES.find(([re]) => re.test(m.name.replace(/\s+interval$|\s+duration$/i, '').trim()) || re.test(m.name));
      const unit = { '/min': 'bpm', '{beats}/min': 'bpm', deg: '°' }[m.unit] || m.unit;
      measurements.push({ name: known ? known[1] : m.name, value: m.value, unit: unit || (known ? known[2] : '') });
    }
    const lines = w.annotations.text.map((t) => t.trim()).filter(Boolean);
    return {
      name: 'DICOM ECG',
      fs: g.fs,
      leads: g.channels.map((c) => ({ name: leadName(c), units: dicomUnits(c.units), samples: Array.from(c.samples) })),
      meta: {
        fields: [['Acquired', w.acquired], ['Group', g.label]].filter((f) => f[1]),
        measurements,
        statements: lines.length ? [{ source: 'computer', lines }] : [],
        notes: [],
      },
    };
  }

  Readers.add({
    key: 'dicom',
    label: 'DICOM ECGs',
    extensions: ['dcm', 'dicom'],
    sniff: (name, bytes) => Readers.latin1(bytes, 128, 132) === 'DICM',
    read: async (name, bytes) => {
      const k = Dicom.kind(bytes);
      if (!['ecg', 'holter', 'waveform'].includes(k.kind)) throw new Error('This DICOM file is not an ECG: open it in the PACS viewer.');
      return ECG.readJson(dicomJson(Dicom.waveforms(bytes)));
    },
  });

  const api = { dicomJson, leadOfCode };
  if (node) module.exports = api;
  else root.DicomEcg = api;
})(typeof self !== 'undefined' ? self : this);
