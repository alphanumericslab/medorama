// What the page writes about a recording, as plain text (no HTML, no DOM): the channel groups
// guessed from labels, and the contents of the Recording panel.  The page puts this text into
// the document with textContent only; tests/web/ checks it without a browser.

import { fmtDuration, fmtFull } from './timefmt.js';

/** Text as HTML text or an attribute value (for the few places that still build HTML). */
export function escapeHtml(t) {
  return String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// A unit that is a voltage, or no unit at all: what an EEG electrode's signal has.
const VOLTAGE = /^([nµμum]?v)?$/i;
// `(?![a-z])`, not `\b`: `\b` fails between a letter and a digit (ECG1) and before `_` (ECG_II).
const ECG = /^(ecg|ekg)(?![a-z])|^(i|ii|iii|avr|avl|avf|v[1-6]|mcl\d?)$/i;
const CO2 = /^(e?t?co2|fico2|capno)/i;
const SPO2 = /spo2|sao2|pleth|ppg|(^|[^a-z])sat(?![a-z])/i;
// airway pressures (ventilator), before the blood pressures
const VENT = /^(awp|paw|pip|peep|pplat|pmean|airway)(?![a-z])/i;
const PRESSURE = /^(art|abp|ap|pap|cvp|icp|cpp|lap|rap|uap|fap|bp|nbp|nibp)(?![a-z])/i;
// numbered pressures of patient monitors (P1, P2 ...): pressures, unless the unit is a voltage
// (then they are parietal EEG electrodes)
const NUMBERED_PRESSURE = /^p\d+$/i;
const GAS = /^(o2|fio2|eto2|n2o|etn2o|agent)(?![a-z0-9])/i;
const RESP = /^(resp|flow|airflow|nasal|thor|thorax|chest|abd|abdomen|effort|snore|imp)/i;
// EEG, EOG and EMG: named so, the AASM eye and mastoid electrodes (E1, E2, M1, M2), and the
// 10-20 and 10-10 electrodes (Fp1, C3, O2, Pz ...), alone or as a derivation (C3-M2)
const EXG = /^(eeg|eog|emg|loc|roc|chin|leg|lleg|rleg|lat|rat)(?![a-z])/i;
const ELECTRODE = /^(e[12]|m[12]|a[12]|(fp|af|f|ft|fc|t|tp|c|cp|p|po|o)(\d{1,2}|z))(?![a-z0-9])/i;

/**
 * A channel's group when its reader names none (a plain WFDB record, an EDF file: `wfdb`,
 * `edf`), from its label and unit: what the signal is, as monitors and sleep laboratories label
 * it.  Only for grouping and colours on the page; the index keeps the reader's modality.
 */
export function displayModality(c) {
  if (c.modality && !['wfdb', 'edf', ''].includes(c.modality)) return c.modality;
  const label = String(c.label || '').trim();
  const unit = String(c.unit || '').trim();
  const volts = VOLTAGE.test(unit);
  const derivation = label.includes('-');
  if (ECG.test(label)) return 'ecg';
  if (CO2.test(label)) return 'co2';
  if (SPO2.test(label)) return 'spo2';
  if (VENT.test(label)) return 'vent';
  if (PRESSURE.test(label)) return 'pressure';
  if (NUMBERED_PRESSURE.test(label) && !(unit && volts)) return 'pressure';
  if (RESP.test(label)) return 'resp';
  if (EXG.test(label)) return 'eeg';
  // a bare electrode name with a unit that is not a voltage is something else (O2 in %, T1 in °C)
  if (ELECTRODE.test(label) && (derivation || volts)) return 'eeg';
  if (GAS.test(label)) return 'gas';
  return c.modality;
}

/**
 * The Recording panel: `rows` ([label, value] pairs) and `notes` (paragraphs), plain text.
 * `range`: the recording's [t0, t1]; `tz`: the display time zone.
 */
export function infoContent(ds, range, tz) {
  const rows = [
    ['Format', [ds.format, ds.description].filter(Boolean).join(' · ')],
    ['Start', fmtFull(range[0], tz, 3600)],
    ['End', fmtFull(range[1], tz, 3600)],
    ['Duration', fmtDuration(range[1] - range[0])],
    ['Waveforms', `${ds.channels.length} channels in ${ds.pieces.length} contiguous pieces`],
    ['Vital signs', `${ds.vitals.length} series`],
    [ds.alarms.length && ds.alarms.every((a) => a.kind === 'annotation') ? 'Annotations' : 'Alarms', `${ds.alarms.length}`],
    ['Breaks', `${ds.gaps.length}`],
  ];
  const mk = ds.markers || {};
  if (mk.count) rows.push(['Markers', `${Number(mk.count).toLocaleString()} (${(mk.names || []).join(', ')})`]);
  if (ds.time_resolution_s) rows.push(['Time stamps', `${ds.time_resolution_s >= 1 ? '1 s' : `${ds.time_resolution_s * 1000} ms`} resolution`]);
  const notes = (ds.notes || []).map(String);
  const skipped = Object.entries(ds.skipped || {});
  if (skipped.length) notes.push(`Not shown: ${skipped.map(([k, v]) => `${k} (${v})`).join('; ')}`);
  return { rows, notes };
}
