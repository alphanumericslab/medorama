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
      leads: g.channels.map((c) => ({ name: c.name, units: dicomUnits(c.units), samples: Array.from(c.samples) })),
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

  const api = { dicomJson };
  if (node) module.exports = api;
  else root.DicomEcg = api;
})(typeof self !== 'undefined' ? self : this);
