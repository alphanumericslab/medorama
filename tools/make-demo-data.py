#!/usr/bin/env python3
"""Make a synthetic demo data set for Medorama: fake patients, their databases and files.

    .venv/bin/python tools/make-demo-data.py [OUT]     # default: sample_data/demo (not in git)

WFDB records are written with wfdb-x (the Python package wfdbx, which Medorama's environment has:
docs/building.md).

Writes, under OUT:

  icu/        short synthetic ICU recordings (WFDB collections, 10 minutes each: ECG, arterial
              pressure, SpO2 pleth, respiration, vital signs, alarms, R-wave markers), named as
              bedside-monitor systems name their recordings (UNIT_BED-<Unix time>)
  holter/     a 24-hour, 2-lead Holter ECG (a WFDB record, 128 Hz)
  sleep/      an overnight sleep study (polysomnography: two hours, an EDF+ file): EEG, EOG, ECG,
              airflow, thoracic effort and SpO2, with its scoring as annotations (sleep stages,
              apneas, desaturations, arousals)
  ecg/        12-lead ECGs: WFDB records (500 Hz, the report in the header), GE MUSE XML, a DICOM
              ECG, one PDF and one PNG printout
  imaging/    DICOM: chest X-rays, CT series (a folder each)
  echo/       echo loops (DICOM multi-frame, 30 frames/s; one as PNG frames) and structured reports
  cath/       a coronary angiography run (DICOM XA, 15 frames/s)
  databases/  the tables to import, each a database of its own:
              patients.csv        who the patients are (MRN, name, date of birth, sex)
              encounters.xlsx     admissions, visits and bed stays (an Excel sheet)
              labs.csv            laboratory results with reference ranges and flags
              vitals.csv          vital signs charted on the ward and in the ICU
              medications.csv     drugs with start and stop times
              diagnoses.csv       ICD-10 codes
              microbiology.csv    cultures
              notes.csv           clinical notes (plain text)
              allergies.csv       allergies
              ecg_index.csv       links to the ECGs, with their measurements and interpretation
              icu_index.csv       links to the ICU recordings
              holter_index.csv    links to the Holter recordings
              sleep_index.csv     links to the sleep studies
              imaging_index.csv   links to the imaging studies, with their reports
              echo_index.csv      echo loops and reports, their measurements as columns
              cath_index.csv      the cath lab's angiography, its findings as columns
  omop/       omop.sqlite: another hospital's OMOP CDM database, to connect rather than import
              (docs/design/sources.md): two of the patients above (by their MRNs) and two of
              its own, with their stays, results, vital signs, conditions, drugs and notes.
              medorama connections add omop --kind omop --url sqlite:OUT/omop/omop.sqlite

Everything is invented: names, numbers, signals and pictures.  Nothing comes from a patient.

    .venv/bin/python tools/make-demo-data.py [OUT] --examples sample_data/sample-ecg

also adds local examples of real ECGs (never in git) to some demo patients: MUSE XML files are
de-identified on the way (a demo patient's identity, no other identifier, a synthetic
overreader, site and location, dates moved to that patient's encounter); WFDB records,
pictures and PDFs, already de-identified, are copied as they are.

    .venv/bin/python tools/make-demo-data.py [OUT] --dicom sample_data/sample-dicom

also gives local DICOM studies (public sample collections, never in git: see DICOM_EXAMPLES)
to the demo patients whose stories they fit, in place of the synthetic CT series.  They are
de-identified on the way (pydicom): the demo patient's identity, no other person's name and no
other identifier, private elements removed, new UIDs, dates moved to the demo encounter.  The
pixels are kept.  A collection's CITATION.txt (its licence) is copied beside it.

Load it into a catalog as the demo: its tables as the source DEMO, its OMOP database, and a
cohort DEMO that every user sees; deleting that cohort removes it all (docs/administration.md,
"The demo").  The installers' --demo option does this for `medorama init`.

    medorama demo add OUT

(the catalog's data_roots, if set, must include OUT).  The first line of this text, which
README.txt repeats, is how `medorama demo add` recognises such a folder.  demo.json says whether
the folder holds invented data alone or also local files (--examples, --dicom):
tools/screenshots.py takes pictures only of a demo of invented data alone.
"""

import csv
import json
import sqlite3
import math
import random
import struct
from array import array
import sys
import zipfile
import zlib
from datetime import datetime, timedelta, timezone
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent

R = random.Random(20240101)
UTC = timezone.utc


def t(s):
    return datetime.strptime(s, '%Y-%m-%d %H:%M').replace(tzinfo=UTC)


def fmt(d):
    return d.strftime('%Y-%m-%d %H:%M')


# ------------------------------------------------------------------ the patients (invented)

PATIENTS = [
    # patient_id, mrn, last, first, born, sex
    ('P900001', 'SYN0000001', 'Doe', 'Jane', '1950-03-04', 'F'),
    ('P900002', 'SYN0000002', 'Roe', 'Richard', '1961-03-05', 'M'),
    ('P900003', 'SYN0000003', 'Major', 'Ana', '1977-08-19', 'F'),
    ('P900004', 'SYN0000004', 'Okafor', 'Ade', '1958-11-02', 'M'),
    ('P900005', 'SYN0000005', 'Lindqvist', 'Eva', '1966-06-14', 'F'),
    ('P900006', 'SYN0000006', 'Tanaka', 'Hiro', '1971-02-25', 'M'),
]
MRN = {p[0]: p[1] for p in PATIENTS}

# encounter_id, patient, csn, kind, admit, discharge, unit, disposition, icu (unit, bed, start, end)
ENCOUNTERS = [
    ('E9100001', 'P900001', '9000000101', 'out', '2023-10-12 08:30', '2023-10-12 10:00', 'Cardiology clinic', 'Home', None),
    ('E9100002', 'P900001', '9000000102', 'in', '2023-12-28 07:40', '2024-01-09 13:00', 'Cardiology', 'Home',
     ('ICU7', '12', '2023-12-29 11:00', '2024-01-03 10:40')),
    ('E9100003', 'P900001', '9000000103', 'out', '2024-02-20 09:00', '2024-02-20 10:00', 'Cardiology clinic', 'Home', None),
    ('E9100004', 'P900002', '9000000201', 'in', '2024-01-15 02:10', '2024-01-21 15:00', 'Emergency', 'Rehabilitation',
     ('ICU8', '3', '2024-01-15 04:00', '2024-01-18 12:00')),
    ('E9100005', 'P900003', '9000000301', 'out', '2024-02-02 18:20', '2024-02-02 23:50', 'Emergency', 'Home', None),
    ('E9100006', 'P900003', '9000000302', 'in', '2024-03-10 11:00', '2024-03-14 16:30', 'Surgery', 'Home', None),
    ('E9100007', 'P900004', '9000000401', 'out', '2023-11-05 14:00', '2023-11-05 15:00', 'Cardiology clinic', 'Home', None),
    ('E9100008', 'P900004', '9000000402', 'out', '2024-03-01 10:00', '2024-03-01 11:00', 'Cardiology clinic', 'Home', None),
    ('E9100010', 'P900004', '9000000403', 'out', '2024-03-05 21:30', '2024-03-06 07:00', 'Sleep laboratory', 'Home', None),
    ('E9100009', 'P900005', '9000000501', 'in', '2024-02-12 20:30', '2024-02-20 11:00', 'Medicine', 'Home',
     ('ICU7', '14', '2024-02-13 01:00', '2024-02-16 09:00')),
]
# P900006 has laboratory results only (no encounter in any database)

LABS = [
    # name, loinc, unit, low, high, normal, sick (value when ill)
    ('Sodium', '2951-2', 'mmol/L', 135, 145, 139, 133),
    ('Potassium', '2823-3', 'mmol/L', 3.5, 5.1, 4.2, 4.9),
    ('Creatinine', '2160-0', 'mg/dL', 0.6, 1.2, 0.9, 2.3),
    ('BUN', '3094-0', 'mg/dL', 7, 20, 14, 38),
    ('Glucose', '2345-7', 'mg/dL', 70, 140, 110, 190),
    ('Lactate', '2524-7', 'mmol/L', 0.5, 2.0, 1.1, 4.1),
    ('WBC', '6690-2', '10^9/L', 4.0, 11.0, 7.5, 18.2),
    ('Hemoglobin', '718-7', 'g/dL', 12.0, 16.0, 13.1, 9.8),
    ('Platelets', '777-3', '10^9/L', 150, 400, 240, 138),
    ('Troponin T', '6598-7', 'ng/L', 0, 14, 8, 380),
    ('Procalcitonin', '33959-8', 'ng/mL', 0, 0.5, 0.1, 6.4),
    ('INR', '6301-6', '', 0.8, 1.2, 1.0, 1.6),
]


def illness(enc, when):
    """How ill the patient is at `when` (0..1): a hump over the ICU stay."""
    icu = enc[8]
    if not icu:
        return 0.15 if enc[3] == 'in' else 0.0
    a, b = t(icu[2]), t(icu[3])
    mid = a + (b - a) * 0.55
    width = (b - a).total_seconds() * 0.45
    return max(0.0, math.exp(-((when - mid).total_seconds() / width) ** 2))


def flag(v, lo, hi):
    if v < lo:
        return 'L'
    if v > hi:
        return 'H'
    return ''


# ------------------------------------------------------------------ ICU recordings

def ecg_wave(phase):
    """One heartbeat, phase 0..1: P, QRS, T (raw counts)."""
    g = lambda c, w, a: a * math.exp(-((phase - c) / w) ** 2)
    return int(g(0.18, 0.025, 90) - g(0.29, 0.008, 80) + g(0.31, 0.010, 900) - g(0.335, 0.010, 220) + g(0.58, 0.05, 220))


def abp_wave(phase, sys_, dia):
    """Arterial pressure (mmHg) over one beat."""
    up = 0.12
    if phase < up:
        return dia + (sys_ - dia) * math.sin(phase / up * math.pi / 2)
    return dia + (sys_ - dia) * math.exp(-(phase - up) * 3.2) + 6 * math.exp(-((phase - 0.36) / 0.03) ** 2)


def pleth_wave(phase):
    ph = (phase + 0.85) % 1.0
    return int(1500 + 900 * (math.sin(ph / 0.3 * math.pi / 2) if ph < 0.3 else math.exp(-(ph - 0.3) * 2.6)))


def wfdbx():
    """wfdb-x, which writes the WFDB records (in Medorama's environment: .venv/bin/python)."""
    try:
        import wfdbx as w
    except ImportError:
        sys.exit('wfdb-x is needed for the WFDB records: run this with .venv/bin/python, after\n'
                 '  .venv/bin/pip install "wfdb-x @ https://github.com/alphanumericslab/wfdb-x/archive/4a91916f5282fba44ca3056802fbc7def298af08.tar.gz#subdirectory=python"\n'
                 '(the Python viewer brings it too; docs/administration.md)')
    return w


def wfdb_record(folder, name, fs, signals, start, comments=()):
    """A WFDB record in format 16, written with wfdb-x; signals: [(description, unit, gain, digital
    samples)]; comments: the header's info lines (UTF-8 in the file)."""
    import numpy as np
    folder.mkdir(parents=True, exist_ok=True)
    w = wfdbx()
    w.write_record(name, np.array([s[3] for s in signals], dtype=np.int32).T, fs, fmt=16,
                   gains=[s[2] for s in signals], adcres=16, units=[s[1] for s in signals],
                   names=[s[0] for s in signals], base_time=f'{start:%H:%M:%S %d/%m/%Y}',
                   # wfdb-x writes one byte per character: give it the UTF-8 bytes
                   comments=[' ' + c.encode('utf-8').decode('latin-1') for c in comments], dir=str(folder))


def wfdb_markers(folder, record, marks):
    """<record>.mrk, written with wfdb-x: marks [(sample, code, chan, text)] in time order."""
    w = wfdbx()
    w.write_annotations(record, 'mrk', [m[0] for m in marks], codes=[m[1] for m in marks],
                        chans=[m[2] for m in marks], aux=[m[3] or None for m in marks], dir=str(folder),
                        sort=False)


def recording(folder, start, minutes, ill):
    """An ICU recording as a WFDB collection (docs/wfdb-collection.md): ECG I, II, III at 240 Hz,
    arterial pressure at 120 Hz, SpO2 pleth and respiration at 60 Hz, one record each; vital
    signs every 2 s; an alarm while the heart rate is high; the ECG's R waves as markers."""
    t0 = start.timestamp()
    seconds = minutes * 60
    state = []                                  # per quarter second: hr, rr, sys, dia, spo2
    for q in range(seconds * 4):
        sick = ill(datetime.fromtimestamp(t0 + q / 4, UTC))
        state.append((72 + 40 * sick + 4 * math.sin(q / (seconds * 4) * 9), 14 + 10 * sick,
                      128 - 34 * sick, 76 - 22 * sick, 97 - 5 * sick))

    def wave(fs, f):
        out, beat, breath = [], 0.0, 0.0
        for i in range(seconds * fs):
            hr, rr, sys_, dia, spo2 = state[i * 4 // fs]
            out.append(f(beat, breath, sys_, dia))
            beat = (beat + hr / 60 / fs) % 1.0
            breath = (breath + rr / 60 / fs) % 1.0
        return out

    ecg = {lead: wave(240, lambda b, r, s, d, k=k: int(ecg_wave(b) * k) + R.randint(-6, 6))
           for lead, k in (('I', 0.6), ('II', 1.0), ('III', 0.4))}
    art = wave(120, lambda b, r, s, d: round(abp_wave(b, s, d) * 10))
    pleth = wave(60, lambda b, r, s, d: pleth_wave(b))
    resp = wave(60, lambda b, r, s, d: int(1000 + 400 * math.sin(2 * math.pi * r)))
    name = folder.name.lower().replace('-', '_')
    records = [('ecg', 240, [(lead, 'mV', 409.836, x) for lead, x in ecg.items()]),
               ('pressure', 120, [('ART', 'mmHg', 10, art)]),
               ('spo2', 60, [('Pleth', 'adu', 1, pleth)]),
               ('resp', 60, [('Resp', 'adu', 1, resp)])]
    index = []
    for modality, fs, signals in records:
        rec = f'{name}_{modality}_{fs}_0001'
        wfdb_record(folder / modality, rec, fs, signals, start)
        (folder / modality / 'RECORDS').write_text(rec + '\n', encoding='ascii')
        index.append([f'{modality}/{rec}', modality, fs, len(signals), ';'.join(x[0] for x in signals),
                      ';'.join(x[1] for x in signals), len(signals[0][3]), seconds, f'{t0:.1f}', 'header time'])
        if modality == 'ecg':
            beats, beat, prev = [], 0.0, 0.0
            for i in range(seconds * fs):
                beat = (beat + state[i * 4 // fs][0] / 60 / fs) % 1.0
                if prev < 0.31 <= beat:
                    beats.append((i, 1, 1, 'R_WAVE'))
                prev = beat
            wfdb_markers(folder / modality, rec, beats)
    write_csv(folder / 'index.csv', ['record', 'modality', 'fs', 'n_sig', 'signals', 'units', 'sig_len',
                                     'duration_s', 'start_unix', 'time_source'], index)
    vitals, events, high = [], [], None
    for q in range(0, seconds * 4, 8):          # every 2 s
        hr, rr, sys_, dia, spo2 = state[q]
        when = f'{t0 + q / 4:.1f}'
        for label, value, unit in (('HR', round(hr), 'bpm'), ('SpO2', round(spo2), '%'), ('ART-S', round(sys_), 'mmHg'),
                                   ('ART-D', round(dia), 'mmHg'), ('ART-M', round((sys_ + 2 * dia) / 3), 'mmHg'),
                                   ('RR', round(rr), '/min')):
            vitals.append(['monitor', when, label, value, unit, '0'])
        if hr > 105 and high is None:
            high = t0 + q / 4
        elif hr <= 105 and high is not None:
            events.append([f'{high:.1f}', when, 2, 'HR HIGH > 105', 'alarm'])
            high = None
    if high is not None:
        events.append([f'{high:.1f}', f'{t0 + seconds:.1f}', 2, 'HR HIGH > 105', 'alarm'])
    write_csv(folder / 'vitals.csv', ['source', 'time', 'label', 'value', 'unit', 'validity'], vitals)
    write_csv(folder / 'events.csv', ['start_unix', 'end_unix', 'level', 'text', 'kind'], events)


def holter(folder, start, hours=24, fs=128):
    """A WFDB record of two ECG leads (II and V5, format 16, 200 adu/mV): the heart rate
    follows the day (slower asleep), with baseline wander and noise."""
    folder.mkdir(parents=True, exist_ok=True)
    table = [ecg_wave(i / 1000) / 4 for i in range(1000)]   # about 1.1 mV R waves
    n = hours * 3600 * fs
    data = array('h', bytes(4 * n))
    beat, wander = 0.0, 0.0
    t0 = start.hour * 3600 + start.minute * 60
    for i in range(n):
        if i % fs == 0:   # once a second: the rate of the hour, a little variable
            clock = (t0 + i / fs) % 86400 / 3600
            asleep = 1 if clock >= 23 or clock < 6.5 else 0
            hr = (82 - 22 * asleep + 6 * math.sin(clock / 24 * 2 * math.pi)) * R.uniform(0.97, 1.03)
        beat = (beat + hr / 60 / fs) % 1.0
        wander += 2 * math.pi * 0.25 / fs
        v = table[int(beat * 1000)]
        bw = 25 * math.sin(wander)
        data[2 * i] = int(v + bw) + R.randint(-4, 4)
        data[2 * i + 1] = int(0.7 * v - 0.5 * bw) + R.randint(-4, 4)
    wfdb_record(folder, 'holter', fs, [('II', 'mV', 200, data[0::2]), ('V5', 'mV', 200, data[1::2])], start)
    (folder / 'RECORDS').write_text('holter\n', encoding='ascii')


# ------------------------------------------------------------------ 12-lead ECGs

# P, Q, R, S, T amplitudes (mV) of the leads that are recorded; III, aVR, aVL and aVF follow from
# I and II (Einthoven, Goldberger), as on a real cart
ECG_WAVES = {
    'I': (.08, -.04, .65, -.08, .20), 'II': (.14, -.05, 1.0, -.15, .30),
    'V1': (.07, 0, .25, -.95, -.05), 'V2': (.08, 0, .45, -1.4, .45), 'V3': (.07, -.02, .8, -.85, .45),
    'V4': (.07, -.05, 1.3, -.4, .42), 'V5': (.06, -.06, 1.15, -.2, .35), 'V6': (.05, -.06, .9, -.12, .26),
}
ECG_COLUMNS = {'PR': 'ms', 'QRS': 'ms', 'QT': 'ms', 'QTc': 'ms', 'R axis': '°'}   # the ECG database's measurement columns
LEADS12 = ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6']


def edf_plus(path, start, signals, notes, duration=1):
    """An EDF+ file (continuous): signals [(label, unit, pmin, pmax, samples per record, values)],
    notes [(onset s, duration s or None, text)], each in the data record it falls in.  The patient
    field is EDF+'s anonymous one: the database, not the file, says whose recording this is."""
    field = lambda t, w: t.encode('ascii').ljust(w)  # noqa: E731
    n = min(len(v) // spr for *_, spr, v in signals)
    by_record = {}
    for o, d, text in notes:
        by_record.setdefault(int(o // duration), []).append((o, d, text))
    tal = lambda o, d, t: (f'{o:+g}' + (f'\x15{d:g}' if d else '') + '\x14' + t + '\x14\x00').encode()  # noqa: E731
    spr_notes = max(30, max((sum(len(tal(*x)) for x in v) + 16 for v in by_record.values()), default=0) // 2 + 1)
    sigs = [(label, unit, pmin, pmax, -32768, 32767, spr, v) for label, unit, pmin, pmax, spr, v in signals]
    sigs.append(('EDF Annotations', '', -1, 1, -32768, 32767, spr_notes, None))
    d = start.astimezone(timezone.utc)
    head = field('0', 8) + field('X X X X', 80) + field(f'Startdate {d.strftime("%d-%b-%Y").upper()} X X X', 80)
    head += field(d.strftime('%d.%m.%y'), 8) + field(d.strftime('%H.%M.%S'), 8) + field(str(256 * (len(sigs) + 1)), 8)
    head += field('EDF+C', 44) + field(str(n), 8) + field(str(duration), 8) + field(str(len(sigs)), 4)
    for get, w in ((0, 16), (None, 80), (1, 8), (2, 8), (3, 8), (4, 8), (5, 8), (None, 80), (6, 8), (None, 32)):
        head += b''.join(field('' if get is None else str(x[get]), w) for x in sigs)
    out = bytearray(head)
    for r in range(n):
        for label, unit, pmin, pmax, dmin, dmax, spr, v in sigs:
            if v is None:
                b = tal(r * duration, None, '') + b''.join(tal(*x) for x in by_record.get(r, []))
                out += b.ljust(2 * spr, b'\0')
                continue
            k = (dmax - dmin) / (pmax - pmin)
            out += array('h', [max(dmin, min(dmax, round((x - pmin) * k + dmin))) for x in v[r * spr:(r + 1) * spr]]).tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(out))


def sleep_study(path, start, hours=2.0):
    """An overnight sleep study (polysomnography), scored: stages every 30 s (wake, N1, N2, N3,
    REM in cycles), obstructive apneas with the desaturations and arousals that follow them."""
    rng = random.Random(7)
    epochs = int(hours * 3600 // 30)
    cycle = ['W'] * 4 + ['N1'] * 4 + ['N2'] * 30 + ['N3'] * 40 + ['N2'] * 20 + ['R'] * 30 + ['N2'] * 12
    stages = [(cycle * 3)[i] for i in range(epochs)]
    names = {'W': 'Sleep stage W', 'N1': 'Sleep stage N1', 'N2': 'Sleep stage N2', 'N3': 'Sleep stage N3', 'R': 'Sleep stage R'}
    notes = [(30.0 * i, 30, names[st]) for i, st in enumerate(stages)]
    apneas = [(t, 18 + rng.random() * 14) for t in range(900, int(hours * 3600) - 120, 260) if stages[t // 30] in ('N1', 'N2', 'R')]
    for t, d in apneas:
        notes += [(float(t), round(d, 1), 'Obstructive apnea'), (round(t + d + 8, 1), 20, 'Oxygen desaturation'),
                  (round(t + d, 1), 6, 'Arousal')]
    notes.sort()
    secs = int(hours * 3600)
    stage_at = lambda t: stages[min(int(t // 30), epochs - 1)]  # noqa: E731
    in_apnea = lambda t: any(a <= t < a + d for a, d in apneas)  # noqa: E731

    def eeg(fs):
        out = []
        for i in range(secs * fs):
            t = i / fs
            st = stage_at(t)
            f, a = {'W': (10, 18), 'N1': (6, 22), 'N2': (13, 14), 'N3': (1.2, 70), 'R': (5, 18)}[st]
            x = a * math.sin(2 * math.pi * f * t) + 8 * math.sin(2 * math.pi * 2.7 * t + 1) + rng.gauss(0, 5)
            if st == 'N2' and (t % 9) < 1:  # spindles
                x += 25 * math.sin(2 * math.pi * 13 * t) * math.sin(math.pi * (t % 9))
            out.append(x)
        return out

    def eog(fs):
        return [(60 * math.sin(2 * math.pi * 0.4 * i / fs) if stage_at(i / fs) in ('R', 'W') else 5 * math.sin(2 * math.pi * 0.1 * i / fs))
                + rng.gauss(0, 4) for i in range(secs * fs)]

    def ecg(fs):
        return [ecg_wave((i / fs * 62 / 60) % 1.0) / 1000 for i in range(secs * fs)]

    def breath(fs, scale):
        return [0.0 if in_apnea(i / fs) and scale > 1 else scale * math.sin(2 * math.pi * 0.25 * i / fs) * (0.35 if in_apnea(i / fs) else 1)
                for i in range(secs * fs)]

    spo2 = []
    for t in range(secs):
        drop = max((4 + 3 * (d > 25)) * math.exp(-((t - (a + d + 18)) / 12) ** 2) for a, d in apneas) if apneas else 0
        spo2.append(round(96 - drop + rng.gauss(0, 0.3), 1))
    edf_plus(path, start, [
        ('EEG C4-M1', 'uV', -500, 500, 100, eeg(100)),
        ('EOG E1-M2', 'uV', -500, 500, 50, eog(50)),
        ('ECG', 'mV', -5, 5, 200, ecg(200)),
        ('Flow', '', -400, 400, 25, breath(25, 300)),
        ('Thorax', '', -400, 400, 25, breath(25, 1)),
        ('SpO2', '%', 0, 100, 1, spo2),
    ], notes)


def ecg12(hr, st_depression=False, af=False, fs=500, seconds=10):
    """Twelve leads (mV) of a synthetic resting ECG: sums of Gaussian P, Q, R, S and T waves
    around each beat; atrial fibrillation: irregular beats, no P, fibrillatory waves; ischaemia:
    ST depression in V4-V6, I and II."""
    n = fs * seconds
    rng = random.Random(hr * 7 + af)
    beats, t = [], 0.35
    while t < seconds + 1:
        beats.append(t)
        rr = 60 / hr * (rng.uniform(0.62, 1.32) if af else rng.uniform(0.985, 1.015))
        t += rr
    g = lambda x, c, w: math.exp(-((x - c) / w) ** 2)
    out = {}
    for lead, (P, Q, Rr, S, T) in ECG_WAVES.items():
        v = [0.0] * n
        for k, b in enumerate(beats):
            rr = (beats[k + 1] - b) if k + 1 < len(beats) else 60 / hr
            qt = 0.27 * math.sqrt(min(1.4, rr))
            for i in range(max(0, int((b - 0.3) * fs)), min(n, int((b + 0.6) * fs))):
                x = i / fs - b
                y = (0 if af else P * g(x, -0.16, 0.022)) + Q * g(x, -0.022, 0.007) + Rr * g(x, 0, 0.009) + S * g(x, 0.024, 0.008) + T * g(x, qt, 0.045)
                if st_depression and lead in ('V4', 'V5', 'V6', 'I', 'II'):
                    y -= (0.16 if lead[0] == 'V' else 0.07) * g(x, 0.13, 0.05)
                v[i] += y
        for i in range(n):
            x = i / fs
            v[i] += 0.03 * math.sin(2 * math.pi * 0.18 * x + len(lead)) + rng.gauss(0, 0.008)
            if af:
                v[i] += 0.05 * math.sin(2 * math.pi * 5.7 * x + 0.3 * math.sin(x)) * (1.4 if lead in ('V1', 'II') else 0.6)
        out[lead] = v
    I, II = out['I'], out['II']
    out['III'] = [b - a for a, b in zip(I, II)]
    out['aVR'] = [-(a + b) / 2 for a, b in zip(I, II)]
    out['aVL'] = [a - b / 2 for a, b in zip(I, II)]
    out['aVF'] = [b - a / 2 for a, b in zip(I, II)]
    return [out[k] for k in LEADS12]


def ecg_measures(hr, af=False, lvh=False):
    """The cart's measurements of an ecg12() ECG, as GE's 12SL reports them: rates, PR, QRS,
    QT, QTc (Bazett) and QTcF (Fridericia), the P, R and T axes; with LV hypertrophy, a wider
    QRS and a leftward axis."""
    rr = 60 / hr
    qt = round((0.27 * math.sqrt(min(1.4, rr)) + 0.1) * 1000) + (24 if lvh else 0)
    m = {'Vent. rate': (hr, 'bpm')}
    if not af:
        m.update({'Atrial rate': (hr, 'bpm'), 'PR': (152 + hr % 7 + (14 if lvh else 0), 'ms')})
    m.update({'QRS': (88 + hr % 5 + (16 if lvh else 0), 'ms'), 'QT': (qt, 'ms'), 'QTc': (round(qt / math.sqrt(rr)), 'ms'),
              'QTcF': (round(qt / rr ** (1 / 3)), 'ms'),
              'R axis': (-38 if lvh else 58 + hr % 9, '°'), 'T axis': (118 if lvh else 44 + hr % 6, '°')})
    if not af:
        m['P axis'] = (52, '°')
    return m


def twelve_sl(m, af, st, lvh, prev=None):
    """The statements GE's Marquette 12SL would print for these measurements: rhythm, axis,
    hypertrophy, ST changes, QT, the comparison with the previous ECG, the verdict.
    `prev`: (time, had ST depression, had atrial fibrillation) of the previous ECG, or None."""
    hr = m['Vent. rate'][0]
    if af:
        out = ['Atrial fibrillation with rapid ventricular response' if hr > 100 else 'Atrial fibrillation']
    else:
        out = ['Sinus tachycardia' if hr > 100 else 'Sinus bradycardia' if hr < 60 else 'Normal sinus rhythm']
    axis = m['R axis'][0]
    if axis < -30:
        out.append('Left axis deviation')
    if lvh:
        out.append('Left ventricular hypertrophy with repolarization abnormality')
    if st:
        out.append('ST depression, consider subendocardial injury or digitalis effect')
    elif af:
        out.append('Nonspecific ST and T wave abnormality')
    long_qt = m['QTc'][0] > 460
    if long_qt:
        out.append('Prolonged QT')
    if prev:
        when, had, had_af = prev
        if had_af and not af:
            change = 'Sinus rhythm has replaced atrial fibrillation'
        elif af and not had_af:
            change = 'Atrial fibrillation has replaced sinus rhythm'
        elif st and not had:
            change = 'ST depression now present'
        elif had and not st:
            change = 'ST depression no longer present'
        else:
            change = 'No significant change was found'
        out.append(f'When compared with ECG of {when:%d}-{when:%b}'.rstrip() + f'-{when:%Y %H:%M}, {change}')
        out[-1] = out[-1].replace(f'-{when:%b}-', f'-{when:%b}-'.upper())
    out.append('Abnormal ECG' if st or af or lvh or long_qt else 'Borderline ECG' if axis < -30 else 'Normal ECG')
    return out


# the leads' ST levels (mV) an ecg12() ECG with ST depression has, as 12SL measures them
ST_DEPRESSION = {'I': -0.07, 'II': -0.07, 'V4': -0.12, 'V5': -0.16, 'V6': -0.10}


def ecg_muse(path, patient, when, leads, measures, computer, confirmed, fs=500):
    """A GE MUSE resting ECG XML file (ISO-8859-1), as MUSE exports them: the patient (invented)
    named inside, the eight recorded leads in base64 (4.88 µV per bit), the measurements, the
    computer's statements (12SL) and the physician's confirmed ones."""
    import base64
    pid, mrn, last, first, born, sex = patient
    b = datetime.strptime(born, '%Y-%m-%d')
    age = when.year - b.year - ((when.month, when.day) < (b.month, b.day))
    by = dict(zip(LEADS12, leads))
    def lead(name):
        v = [max(-32767, min(32767, round(x * 1000 / 4.88))) for x in by[name]]
        data = base64.b64encode(struct.pack(f'<{len(v)}h', *v)).decode()
        data = '\n'.join(data[i:i + 76] for i in range(0, len(data), 76))
        return (f'<LeadData><LeadByteCountTotal>{2 * len(v)}</LeadByteCountTotal><LeadSampleCountTotal>{len(v)}</LeadSampleCountTotal>'
                f'<LeadAmplitudeUnitsPerBit>4.88</LeadAmplitudeUnitsPerBit><LeadAmplitudeUnits>MICROVOLTS</LeadAmplitudeUnits>'
                f'<LeadID>{name}</LeadID><WaveFormData>\n{data}\n</WaveFormData></LeadData>\n')
    tags = {'Vent. rate': 'VentricularRate', 'Atrial rate': 'AtrialRate', 'PR': 'PRInterval', 'QRS': 'QRSDuration',
            'QT': 'QTInterval', 'QTc': 'QTCorrected', 'QTcF': 'QTcFrederica', 'P axis': 'PAxis', 'R axis': 'RAxis', 'T axis': 'TAxis'}
    meas = ''.join(f'<{tags[k]}>{v}</{tags[k]}>' for k, (v, _) in measures.items() if k in tags)
    stmts = lambda ls: ''.join(f'<DiagnosisStatement><StmtFlag>ENDSLINE</StmtFlag><StmtText>{escape(x)}</StmtText></DiagnosisStatement>' for x in ls)
    edit = when + timedelta(hours=20)
    xml = f"""<?xml version="1.0" encoding="ISO-8859-1"?>
<!DOCTYPE RestingECG SYSTEM "restecg.dtd">
<RestingECG>
<MuseInfo><MuseVersion>9.0.10</MuseVersion></MuseInfo>
<PatientDemographics><PatientID>{mrn}</PatientID><PatientAge>{age}</PatientAge><AgeUnits>YEARS</AgeUnits>
<DateofBirth>{b:%m-%d-%Y}</DateofBirth><Gender>{'FEMALE' if sex == 'F' else 'MALE'}</Gender>
<PatientLastName>{escape(last.upper())}</PatientLastName><PatientFirstName>{escape(first.upper())}</PatientFirstName></PatientDemographics>
<TestDemographics><DataType>RESTING</DataType><SiteName>Synthetic Hôpital</SiteName><AcquisitionDevice>MAC5500</AcquisitionDevice>
<Status>CONFIRMED</Status><LocationName>CARDIOLOGY</LocationName><TestReason>Synthetic test, not a patient</TestReason>
<AcquisitionTime>{when:%H:%M:%S}</AcquisitionTime><AcquisitionDate>{when:%m-%d-%Y}</AcquisitionDate>
<AnalysisSoftwareVersion>241</AnalysisSoftwareVersion><EditTime>{edit:%H:%M:%S}</EditTime><EditDate>{edit:%m-%d-%Y}</EditDate>
<OverreaderLastName>RIVERA</OverreaderLastName><OverreaderFirstName>A</OverreaderFirstName></TestDemographics>
<RestingECGMeasurements>{meas}</RestingECGMeasurements>
<OriginalDiagnosis>{stmts(computer)}</OriginalDiagnosis>
<Diagnosis>{stmts(confirmed)}</Diagnosis>
<Waveform><WaveformType>Rhythm</WaveformType><SampleBase>{fs}</SampleBase><SampleExponent>0</SampleExponent>
<HighPassFilter>5</HighPassFilter><LowPassFilter>150</LowPassFilter><ACFilter>60</ACFilter>
{''.join(lead(k) for k in ('I', 'II', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'))}</Waveform>
</RestingECG>
"""
    path.write_bytes(xml.encode('iso-8859-1'))
    return path


def ecg_wfdb(folder, name, start, leads, fs=500, comments=()):
    """A WFDB record (format 16, 1000 adu/mV) of twelve leads, as ECG carts export them; the
    cart's report (age, sex, measurements, statements) in the header's comments."""
    signals = [(lead, 'mV', 1000, [max(-32767, min(32767, round(x * 1000))) for x in v])
               for lead, v in zip(LEADS12, leads)]
    wfdb_record(folder, name, fs, signals, start, comments=[*comments, 'synthetic, not a patient'])
    return folder / f'{name}.hea'


def paper_layout(leads, fs=500):
    """Traces of a 3 x 4 + rhythm layout in mm (25 mm/s, 10 mm/mV): [(label, x0, y0, [(x, y)])]."""
    rows = [['I', 'aVR', 'V1', 'V4'], ['II', 'aVL', 'V2', 'V5'], ['III', 'aVF', 'V3', 'V6']]
    by = dict(zip(LEADS12, leads))
    out = []
    for r, names in enumerate(rows + [['II']]):
        y0 = 30 + r * 38
        for c, lead in enumerate(names):
            span = 10 if r == 3 else 2.5
            t0 = 0 if r == 3 else c * 2.5
            x0 = 20 + t0 * 25
            pts = [(x0 + (i / fs - t0) * 25, y0 - by[lead][i] * 10) for i in range(int(t0 * fs), int((t0 + span) * fs), 2)]
            out.append((lead, x0, y0, pts))
    return out


def ecg_pdf(path, leads, title):
    """A 12-lead ECG on paper as a one-page vector PDF (A4 landscape), as carts print them."""
    k = 72 / 25.4
    H = 210
    c = ['0.5 w 1 0.82 0.80 RG']
    for x in range(0, 298):
        c.append(f'{"0.6 w 0.96 0.62 0.58 RG " if x % 5 == 0 else "0.25 w 1 0.84 0.82 RG "}{x * k:.2f} 0 m {x * k:.2f} {H * k:.2f} l S')
    for y in range(0, 211):
        c.append(f'{"0.6 w 0.96 0.62 0.58 RG " if y % 5 == 0 else "0.25 w 1 0.84 0.82 RG "}0 {y * k:.2f} m {297 * k:.2f} {y * k:.2f} l S')
    c.append('0.7 w 0.08 0.08 0.08 RG 0.2 0.15 0.15 rg')
    for lead, x0, y0, pts in paper_layout(leads):
        c.append(' '.join(f'{x * k:.2f} {(H - y) * k:.2f} {"m" if i == 0 else "l"}' for i, (x, y) in enumerate(pts)) + ' S')
        c.append(f'BT /F1 9 Tf {(x0 + 1) * k:.2f} {(H - y0 + 12) * k:.2f} Td ({lead}) Tj ET')
    c.append(f'BT /F1 10 Tf {20 * k:.2f} {(H - 12) * k:.2f} Td ({title}   25 mm/s   10 mm/mV) Tj ET')
    stream = zlib.compress('\n'.join(c).encode('latin-1'))
    objs = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {297 * k:.2f} {H * k:.2f}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>'.encode(),
            b'<< /Length ' + str(len(stream)).encode() + b' /Filter /FlateDecode >>\nstream\n' + stream + b'\nendstream',
            b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
    pdf, offs = bytearray(b'%PDF-1.4\n'), []
    for i, o in enumerate(objs, 1):
        offs.append(len(pdf))
        pdf += f'{i} 0 obj\n'.encode() + o + b'\nendobj\n'
    xref = len(pdf)
    pdf += f'xref\n0 {len(objs) + 1}\n0000000000 65535 f \n'.encode() + b''.join(f'{o:010d} 00000 n \n'.encode() for o in offs)
    pdf += f'trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode()
    path.write_bytes(bytes(pdf))


def png(path, w, h, rgb):
    """An RGB PNG from a bytearray of w*h*3 bytes."""
    raw = b''.join(b'\x00' + bytes(rgb[3 * w * y:3 * w * (y + 1)]) for y in range(h))
    chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


def ecg_png(path, leads, ppm=4):
    """A 12-lead ECG on paper as a picture (a scanned or exported ECG), 4 px per mm."""
    W, H = 290 * ppm, 180 * ppm
    img = bytearray(b'\xff\xf7\xf6' * (W * H))
    def put(x, y, col):
        if 0 <= x < W and 0 <= y < H:
            img[3 * (y * W + x):3 * (y * W + x) + 3] = col
    for x in range(W):
        for y in range(H):
            if x % (5 * ppm) == 0 or y % (5 * ppm) == 0:
                img[3 * (y * W + x):3 * (y * W + x) + 3] = b'\xf0\xa8\xa2'
            elif x % ppm == 0 or y % ppm == 0:
                img[3 * (y * W + x):3 * (y * W + x) + 3] = b'\xfb\xdc\xd9'
    ink = b'\x1a\x1a\x1a'
    for _, _, _, pts in paper_layout(leads):
        for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
            steps = int(max(abs(x1 - x0), abs(y1 - y0)) * ppm) + 1
            for s in range(steps + 1):
                x = round((x0 + (x1 - x0) * s / steps) * ppm)
                y = round((y0 + (y1 - y0) * s / steps - 8) * ppm)
                for dx, dy in ((0, 0), (1, 0), (0, 1), (1, 1)):
                    put(x + dx, y + dy, ink)
    png(path, W, H, img)


# ------------------------------------------------------------------ DICOM (imaging, echo)

UID_ROOT = '2.25.'


def uid():
    return UID_ROOT + str(R.getrandbits(100))


def el(g, e, vr, val):
    """One DICOM element, explicit VR little endian; an SQ's value is a list of items, each the
    bytes of its elements (dataset())."""
    if isinstance(val, bytes):
        data = val
    elif vr == 'US':
        data = struct.pack('<H', val)
    elif vr == 'SS':
        data = struct.pack('<h', val)
    elif vr == 'UL':
        data = struct.pack('<I', val)
    elif vr == 'FD':
        data = struct.pack('<d', val)
    elif vr == 'SQ':
        items = b''.join(b'\xfe\xff\x00\xe0' + struct.pack('<I', len(it)) + it for it in val)
        return struct.pack('<HH', g, e) + b'SQ\0\0' + struct.pack('<I', len(items)) + items
    else:
        data = str(val).encode('latin-1')
        if len(data) % 2:
            data += b'\0' if vr == 'UI' else b' '
    if vr in ('OB', 'OW', 'SQ', 'UN', 'UT'):
        return struct.pack('<HH', g, e) + vr.encode() + b'\0\0' + struct.pack('<I', len(data)) + data
    return struct.pack('<HH', g, e) + vr.encode() + struct.pack('<H', len(data)) + data


def dataset(*elements):
    """An item's bytes: (group, element, VR, value) elements, in tag order."""
    return b''.join(el(*x) for x in sorted(elements, key=lambda x: (x[0], x[1])))


def code(g, e, value, scheme, meaning):
    """A coded concept, as a sequence of one item."""
    return (g, e, 'SQ', [dataset((8, 0x100, 'SH', value), (8, 0x102, 'SH', scheme), (8, 0x104, 'LO', meaning))])


def dicom(path, elements, pixels, sop_class):
    """A DICOM Part 10 file, explicit VR little endian: (group, element, VR, value) elements and
    the pixel data (None: an object without an image).  No patient's name: the demo's files
    carry only an identifier."""
    sop = uid()
    meta = (el(2, 1, 'OB', b'\0\1') + el(2, 2, 'UI', sop_class) + el(2, 3, 'UI', sop) + el(2, 0x10, 'UI', '1.2.840.10008.1.2.1')
            + el(2, 0x12, 'UI', UID_ROOT + '1'))
    body = el(8, 0x16, 'UI', sop_class) + el(8, 0x18, 'UI', sop)
    for g, e, vr, v in sorted(elements, key=lambda x: (x[0], x[1])):
        body += el(g, e, vr, v)
    if pixels is not None:
        body += el(0x7fe0, 0x10, 'OW' if len(pixels) and elements and any(x[:2] == (0x28, 0x100) and x[3] == 16 for x in elements) else 'OB', pixels)
    path.write_bytes(b'\0' * 128 + b'DICM' + el(2, 0, 'UL', len(meta)) + meta + body)


def ct_series(folder, kind, pid, when, n=24, size=128):
    """A synthetic CT series (one file per slice, 5 mm apart): HU maps of the chest or abdomen,
    with bilateral lower-lobe consolidation (chest) or an inflamed appendix (abdomen)."""
    folder.mkdir(parents=True, exist_ok=True)
    series, study = uid(), uid()
    rng = random.Random(len(pid) + n)
    for z in range(n):
        f = z / (n - 1)            # 0: lowest slice, 1: highest
        hu = array('H', [0]) * (size * size)
        for y in range(size):
            for x in range(size):
                u, v = (x - size / 2) / (size / 2), (y - size / 2) / (size / 2)
                val = -1000
                if (u / 0.92) ** 2 + (v / 0.7) ** 2 < 1:
                    val = -90 if (u / 0.92) ** 2 + (v / 0.7) ** 2 > 0.8 else 45      # fat rim, soft tissue
                    if kind == 'chest':
                        for cx in (-0.42, 0.42):
                            if ((u - cx) / 0.3) ** 2 + ((v + 0.05) / (0.42 + 0.1 * f)) ** 2 < 1:
                                val = -830
                                if f < 0.35 and v > 0.05 and ((u - cx) / 0.22) ** 2 + ((v - 0.2) / 0.2) ** 2 < 1:
                                    val = 25                                         # consolidation
                        if ((u - 0.1) / 0.24) ** 2 + ((v + 0.1) / 0.22) ** 2 < 1 and f < 0.75:
                            val = 40                                                 # heart
                        if (u / 0.07) ** 2 + ((v + 0.25) / 0.07) ** 2 < 1:
                            val = 180                                                # aorta, with contrast
                    else:
                        if ((u + 0.4) / 0.38) ** 2 + ((v + 0.15) / 0.34) ** 2 < 1 and f > 0.4:
                            val = 62                                                 # liver
                        for cx in (-0.42, 0.42):
                            if ((u - cx) / 0.12) ** 2 + ((v - 0.3) / 0.2) ** 2 < 1 and 0.3 < f < 0.8:
                                val = 30                                             # kidneys
                        if ((u - 0.15) / 0.1) ** 2 + ((v + 0.2) / 0.08) ** 2 < 1 and rng.random() < 0.9 and 0.2 < f < 0.7:
                            val = -900                                               # bowel gas
                        if f < 0.35 and ((u - 0.45) / 0.08) ** 2 + ((v + 0.05) / 0.08) ** 2 < 1:
                            val = 45 if ((u - 0.45) / 0.05) ** 2 + ((v + 0.05) / 0.05) ** 2 > 1 else 15   # appendix, thick wall
                    if (u / 0.11) ** 2 + ((v - 0.5) / 0.11) ** 2 < 1:
                        val = 650                                                    # vertebra
                    if 0.78 < (u / 0.92) ** 2 + (v / 0.7) ** 2 < 0.84 and kind == 'chest' and int((math.atan2(v, u) + 4) * 6) % 2 == 0:
                        val = 500                                                    # ribs
                hu[y * size + x] = max(0, min(4095, val + 1024 + round(rng.gauss(0, 9))))
        if sys.byteorder != 'little':
            hu.byteswap()
        dicom(folder / f'IM{z + 1:04d}.dcm', [
            (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x60, 'CS', 'CT'), (8, 0x103e, 'LO', 'Chest with contrast' if kind == 'chest' else 'Abdomen and pelvis'),
            (0x10, 0x20, 'LO', pid), (0x18, 0x50, 'DS', '5'), (0x20, 0xd, 'UI', study), (0x20, 0xe, 'UI', series), (0x20, 0x11, 'IS', '2'),
            (0x20, 0x13, 'IS', str(z + 1)), (0x20, 0x32, 'DS', f'-160\\-160\\{z * 5 - 60}'), (0x20, 0x37, 'DS', '1\\0\\0\\0\\1\\0'),
            (0x20, 0x1041, 'DS', str(z * 5 - 60)), (0x28, 2, 'US', 1), (0x28, 4, 'CS', 'MONOCHROME2'), (0x28, 0x10, 'US', size),
            (0x28, 0x11, 'US', size), (0x28, 0x30, 'DS', '2.5\\2.5'), (0x28, 0x100, 'US', 16), (0x28, 0x101, 'US', 12),
            (0x28, 0x102, 'US', 11), (0x28, 0x103, 'US', 0), (0x28, 0x1050, 'DS', '40'), (0x28, 0x1051, 'DS', '400'),
            (0x28, 0x1052, 'DS', '-1024'), (0x28, 0x1053, 'DS', '1'),
        ], hu.tobytes(), '1.2.840.10008.5.1.4.1.1.2')
    return folder


def chest_xray(path, pid, when, opacity=None, size=256):
    """A synthetic frontal chest radiograph (DX), optionally with a lower-lobe opacity."""
    rng = random.Random(len(pid))
    px = array('H', [0]) * (size * size)
    for y in range(size):
        for x in range(size):
            u, v = (x - size / 2) / (size / 2), (y - size / 2) / (size / 2)
            val = 300
            if (u / 0.9) ** 2 + ((v - 0.1) / 0.95) ** 2 < 1:
                val = 1900
                for cx in (-0.42, 0.42):
                    if ((u - cx) / 0.3) ** 2 + ((v - 0.05) / 0.62) ** 2 < 1:
                        val = 700
                        if opacity == ('right' if cx < 0 else 'left') and v > 0.25:
                            val = 1500
                if ((u - 0.1) / 0.27) ** 2 + ((v - 0.35) / 0.26) ** 2 < 1:
                    val = 2200                                    # heart
                if abs(u) < 0.06:
                    val = max(val, 2600)                          # spine
                if 0.2 < abs(u) < 0.85 and int((v + 1) * 12) % 2 == 0 and val == 700:
                    val = 1000                                    # ribs
            px[y * size + x] = max(0, min(4095, val + round(rng.gauss(0, 30))))
    if sys.byteorder != 'little':
        px.byteswap()
    dicom(path, [
        (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x60, 'CS', 'DX'), (8, 0x103e, 'LO', 'Chest PA'), (0x10, 0x20, 'LO', pid),
        (0x20, 0xd, 'UI', uid()), (0x20, 0xe, 'UI', uid()), (0x20, 0x11, 'IS', '1'), (0x20, 0x13, 'IS', '1'),
        (0x28, 2, 'US', 1), (0x28, 4, 'CS', 'MONOCHROME2'), (0x28, 0x10, 'US', size), (0x28, 0x11, 'US', size),
        (0x28, 0x30, 'DS', '1.4\\1.4'), (0x28, 0x100, 'US', 16), (0x28, 0x101, 'US', 12), (0x28, 0x102, 'US', 11), (0x28, 0x103, 'US', 0),
        (0x28, 0x1050, 'DS', '1600'), (0x28, 0x1051, 'DS', '3000'),
    ], px.tobytes(), '1.2.840.10008.5.1.4.1.1.1.1')


def echo_frames(pid, ef, frames=40, w=160, h=120):
    """The frames (8-bit grey, frame after frame) of a synthetic apical four-chamber loop: a fan
    of speckle with a left ventricle that contracts with ejection fraction `ef`."""
    rng = random.Random(len(pid) + frames)
    speckle = [rng.random() for _ in range(w * h)]
    out = bytearray()
    for k in range(frames):
        ph = k / frames
        # volume over a beat: fast ejection, slower filling
        c = (1 - math.sin(math.pi * ph / 0.4)) if ph < 0.4 else 1.0
        scale = 1 - (1 - math.sqrt(1 - ef)) * (1 - c)
        for y in range(h):
            for x in range(w):
                u, v = (x - w / 2) / (h * 0.9), y / (h * 0.95)
                inside_fan = v > 0.02 and abs(math.atan2(u, v)) < 0.62 and math.hypot(u, v) < 1.05
                val = 0
                if inside_fan:
                    val = 18 + 40 * speckle[(y * w + x + 7 * k) % (w * h)]
                    for (cx, cy, rx, ry, s) in ((-0.17, 0.42, 0.13, 0.27, scale), (0.15, 0.48, 0.1, 0.2, 1), (-0.15, 0.82, 0.12, 0.1, 1), (0.14, 0.84, 0.1, 0.09, 1)):
                        d = ((u - cx) / (rx * s)) ** 2 + ((v - cy) / (ry * (0.75 + 0.25 * s))) ** 2
                        if d < 1:
                            val = 6 + 10 * speckle[(y * w + x) % (w * h)]     # blood: dark
                        elif d < 1.6:
                            val = max(val, 120 + 90 * speckle[(y * w + x + k) % (w * h)])   # myocardium: bright
                out.append(min(255, int(val)))
    return bytes(out)


def echo_loop(path, pid, when, ef, frames=40, w=160, h=120, study=None):
    """A synthetic apical four-chamber loop (US, multi-frame, 30 frames/s); scale 0.1 cm per pixel."""
    out = echo_frames(pid, ef, frames, w, h)
    region = (struct.pack('<HH', 0x18, 0x6018) + b'UL' + struct.pack('<HI', 4, 0) + struct.pack('<HH', 0x18, 0x601a) + b'UL' + struct.pack('<HI', 4, 0)
              + struct.pack('<HH', 0x18, 0x601c) + b'UL' + struct.pack('<HI', 4, w - 1) + struct.pack('<HH', 0x18, 0x601e) + b'UL' + struct.pack('<HI', 4, h - 1)
              + struct.pack('<HH', 0x18, 0x6024) + b'US' + struct.pack('<HH', 2, 3) + struct.pack('<HH', 0x18, 0x6026) + b'US' + struct.pack('<HH', 2, 3)
              + struct.pack('<HH', 0x18, 0x602c) + b'FD' + struct.pack('<Hd', 8, 0.1) + struct.pack('<HH', 0x18, 0x602e) + b'FD' + struct.pack('<Hd', 8, 0.1))
    dicom(path, [
        (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x60, 'CS', 'US'), (8, 0x103e, 'LO', 'Apical 4-chamber'), (0x10, 0x20, 'LO', pid),
        (0x18, 0x1063, 'DS', '33.333'), (0x18, 0x6011, 'SQ', [region]),
        (0x20, 0xd, 'UI', study or uid()), (0x20, 0xe, 'UI', uid()), (0x20, 0x11, 'IS', '1'), (0x20, 0x13, 'IS', '1'),
        (0x28, 2, 'US', 1), (0x28, 4, 'CS', 'MONOCHROME2'), (0x28, 8, 'IS', str(frames)), (0x28, 0x10, 'US', h), (0x28, 0x11, 'US', w),
        (0x28, 0x100, 'US', 8), (0x28, 0x101, 'US', 8), (0x28, 0x102, 'US', 7), (0x28, 0x103, 'US', 0),
    ], out, '1.2.840.10008.5.1.4.1.1.3.1')


def echo_pngs(folder, pid, ef, frames=40, w=160, h=120):
    """The same loop as pictures, one PNG per frame (as some carts export them): the frame rate
    and the scale are columns of the database, not in the files."""
    folder.mkdir(parents=True, exist_ok=True)
    out = echo_frames(pid, ef, frames, w, h)
    for k in range(frames):
        g = out[k * w * h:(k + 1) * w * h]
        png(folder / f'frame{k + 1:03d}.png', w, h, bytearray(b for v in g for b in (v, v, v)))
    return folder


def echo_report(path, pid, when, ef, lvidd, study):
    """A synthetic adult echo structured report (TID 5200 in spirit): LV measurements in a
    section, the mean of two ejection fractions, and the finding as text."""
    def num(meaning, value, unit, mean=False):
        items = [(0x40, 0xa010, 'CS', 'CONTAINS'), (0x40, 0xa040, 'CS', 'NUM'), code(0x40, 0xa043, 'x', 'LN', meaning),
                 (0x40, 0xa300, 'SQ', [dataset((0x40, 0xa30a, 'DS', f'{value:g}'), code(0x40, 0x8ea, unit, 'UCUM', unit))])]
        if mean:
            items.append((0x40, 0xa730, 'SQ', [dataset((0x40, 0xa010, 'CS', 'HAS CONCEPT MOD'), (0x40, 0xa040, 'CS', 'CODE'),
                                                       code(0x40, 0xa043, '121401', 'DCM', 'Derivation'), code(0x40, 0xa168, 'R-00317', 'SRT', 'Mean'))]))
        return dataset(*items)
    lv = dataset((0x40, 0xa010, 'CS', 'CONTAINS'), (0x40, 0xa040, 'CS', 'CONTAINER'), code(0x40, 0xa043, '29468-8', 'LN', 'Left Ventricle'),
                 (0x40, 0xa730, 'SQ', [num('Left Ventricular Ejection Fraction', round(ef * 100 + 2), '%'),
                                       num('Left Ventricular Ejection Fraction', round(ef * 100), '%', mean=True),
                                       num('Left Ventricle Internal End Diastolic Dimension', lvidd, 'cm'),
                                       num('Tricuspid Annular Plane Systolic Excursion', 21 if ef > 0.5 else 15, 'mm')]))
    finding = dataset((0x40, 0xa010, 'CS', 'CONTAINS'), (0x40, 0xa040, 'CS', 'TEXT'), code(0x40, 0xa043, '121071', 'DCM', 'Finding'),
                      (0x40, 0xa160, 'UT', 'Reduced LV systolic function, inferior hypokinesis.' if ef < 0.5 else 'Normal LV size and systolic function.'))
    dicom(path, [
        (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x30, 'TM', f'{when:%H%M%S}'), (8, 0x60, 'CS', 'SR'), (0x10, 0x20, 'LO', pid),
        (0x20, 0xd, 'UI', study), (0x20, 0xe, 'UI', uid()), (0x20, 0x11, 'IS', '99'),
        (0x40, 0xa040, 'CS', 'CONTAINER'), code(0x40, 0xa043, '125200', 'DCM', 'Adult Echocardiography Procedure Report'),
        (0x40, 0xa730, 'SQ', [lv, finding]),
    ], None, '1.2.840.10008.5.1.4.1.1.88.33')


def angio_run(path, pid, when, frames=24, size=128):
    """A synthetic coronary angiography run (XA, 15 frames/s): contrast fills a branching vessel
    tree over a grey background, with a tight stenosis on one branch; 0.3 mm per pixel at the
    detector."""
    rng = random.Random(len(pid) * 3)
    # vessel segments: (x0, y0, x1, y1, width, order of filling)
    segs = [(20, 30, 70, 60, 5, 0), (70, 60, 110, 70, 4, 1), (70, 60, 80, 110, 4, 1), (45, 45, 40, 95, 3, 1), (110, 70, 120, 100, 3, 2), (80, 110, 60, 122, 2.5, 2)]
    out = bytearray()
    for k in range(frames):
        fill = k / (frames * 0.6)
        img = [150 + rng.randint(-8, 8) for _ in range(size * size)]
        for x0, y0, x1, y1, wdt, order in segs:
            f = max(0.0, min(1.0, fill - order * 0.35))
            if f <= 0:
                continue
            n = int(max(abs(x1 - x0), abs(y1 - y0)) * 2)
            for i in range(int(n * f)):
                t2 = i / n
                cx, cy = x0 + (x1 - x0) * t2, y0 + (y1 - y0) * t2
                # the stenosis: the LAD-like branch narrows at its middle
                ww = wdt * (0.3 if (order == 1 and x1 == 110 and 0.3 < t2 < 0.7) else 1)
                r = int(ww) + 1
                for dy in range(-r, r + 1):
                    for dx in range(-r, r + 1):
                        if dx * dx + dy * dy <= ww * ww:
                            px, py = int(cx + dx), int(cy + dy)
                            if 0 <= px < size and 0 <= py < size:
                                img[py * size + px] = 40 + rng.randint(-5, 5)
        out += bytes(max(0, min(255, v)) for v in img)
    dicom(path, [
        (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x30, 'TM', f'{when:%H%M%S}'), (8, 0x60, 'CS', 'XA'), (8, 0x103e, 'LO', 'Left coronary, RAO caudal'),
        (0x10, 0x20, 'LO', pid), (0x18, 0x40, 'IS', '15'), (0x18, 0x1063, 'DS', '66.667'), (0x18, 0x1164, 'DS', '0.3\\0.3'),
        (0x20, 0xd, 'UI', uid()), (0x20, 0xe, 'UI', uid()), (0x20, 0x11, 'IS', '1'), (0x20, 0x13, 'IS', '1'),
        (0x28, 2, 'US', 1), (0x28, 4, 'CS', 'MONOCHROME2'), (0x28, 8, 'IS', str(frames)), (0x28, 0x10, 'US', size), (0x28, 0x11, 'US', size),
        (0x28, 0x100, 'US', 8), (0x28, 0x101, 'US', 8), (0x28, 0x102, 'US', 7), (0x28, 0x103, 'US', 0),
    ], bytes(out), '1.2.840.10008.5.1.4.1.1.12.1')


def ecg_dicom(path, pid, when, leads, measures, statements, fs=500):
    """A DICOM 12-lead ECG (the waveform IOD, as carts export them): the twelve leads in µV
    (2.5 µV per unit), the measurements and statements as annotations."""
    by = dict(zip(LEADS12, leads))
    names = LEADS12
    n = len(leads[0])
    data = array('h', [0]) * (n * len(names))
    for j, lead in enumerate(names):
        for i in range(n):
            data[len(names) * i + j] = max(-32767, min(32767, round(by[lead][i] * 1000 / 2.5)))
    if sys.byteorder != 'little':
        data.byteswap()
    chans = [dataset(code(0x3a, 0x208, f'5.6.3-9-{lead}', 'SCPECG', f'Lead {lead}'), (0x3a, 0x210, 'DS', '2.5'), code(0x3a, 0x211, 'uV', 'UCUM', 'microvolt'),
                     (0x3a, 0x212, 'DS', '1'), (0x3a, 0x213, 'DS', '0'), (0x3a, 0x21a, 'US', 16)) for lead in names]
    group = dataset((0x3a, 0x5, 'US', len(names)), (0x3a, 0x10, 'UL', n), (0x3a, 0x1a, 'DS', str(fs)), (0x3a, 0x20, 'SH', 'RHYTHM'),
                    (0x3a, 0x200, 'SQ', chans), (0x5400, 0x1004, 'US', 16), (0x5400, 0x1006, 'CS', 'SS'), (0x5400, 0x1010, 'OW', data.tobytes()))
    loinc = {'Vent. rate': '8867-4', 'PR': '8625-6', 'QRS': '8633-2', 'QT': '8634-8', 'QTc': '8636-3', 'R axis': '8632-4', 'T axis': '8638-1', 'P axis': '8626-4', 'Atrial rate': '8637-1'}
    words = {'Vent. rate': 'Heart rate', 'PR': 'PR interval', 'QRS': 'QRS duration', 'QT': 'QT interval', 'QTc': 'QTc interval', 'R axis': 'R axis', 'T axis': 'T axis', 'P axis': 'P axis', 'Atrial rate': 'Atrial rate'}
    ann = [dataset(code(0x40, 0xa043, loinc.get(k, 'x'), 'LN', words.get(k, k)), (0x40, 0xa30a, 'DS', f'{v:g}'), code(0x40, 0x8ea, 'deg' if u == '°' else ('/min' if u == 'bpm' else u), 'UCUM', u))
           for k, (v, u) in measures.items()]
    ann += [dataset((0x70, 0x6, 'ST', x)) for x in statements]
    dicom(path, [
        (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x2a, 'DT', f'{when:%Y%m%d%H%M%S}'), (8, 0x60, 'CS', 'ECG'), (0x10, 0x20, 'LO', pid),
        (0x20, 0xd, 'UI', uid()), (0x20, 0xe, 'UI', uid()), (0x40, 0xb020, 'SQ', ann), (0x5400, 0x100, 'SQ', [group]),
    ], None, '1.2.840.10008.5.1.4.1.1.9.1.1')


# ------------------------------------------------------------------ a minimal Excel writer

def write_xlsx(path, rows):
    """One sheet of text cells (inline strings): enough for the catalog's reader."""
    def col(i):
        s = ''
        i += 1
        while i:
            i, r = divmod(i - 1, 26)
            s = chr(65 + r) + s
        return s
    sheet = ['<?xml version="1.0" encoding="UTF-8" standalone="yes"?>',
             '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>']
    for r, row in enumerate(rows, 1):
        cells = ''.join(f'<c r="{col(i)}{r}" t="inlineStr"><is><t>{escape(str(v))}</t></is></c>' for i, v in enumerate(row) if v != '')
        sheet.append(f'<row r="{r}">{cells}</row>')
    sheet.append('</sheetData></worksheet>')
    files = {
        '[Content_Types].xml': '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>',
        '_rels/.rels': '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>',
        'xl/workbook.xml': '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Encounters" sheetId="1" r:id="rId1"/></sheets></workbook>',
        'xl/_rels/workbook.xml.rels': '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>',
        'xl/worksheets/sheet1.xml': '\n'.join(sheet),
    }
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, text in files.items():
            # a fixed time and system, not the clock's: the same workbook on every run
            info = zipfile.ZipInfo(name, (2024, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = 0o644 << 16
            z.writestr(info, text, zipfile.ZIP_DEFLATED)


def write_csv(path, header, rows):
    with open(path, 'w', newline='', encoding='utf-8') as f:
        w = csv.writer(f)
        w.writerow(header)
        w.writerows(rows)


# ------------------------------------------------------------------ the data set

# ------------------------------------------------------------------ local examples of real ECGs

def muse_deidentified(text, patient, when):
    """A MUSE XML file (its text) as a demo patient's: the patient's identity replaced by the
    demo patient's, other identifiers emptied, the staff and the place synthetic, the dates
    moved so that the ECG is taken at `when` (the edit one day later).  The waveforms, the
    measurements and the statements are kept."""
    import re
    pid, mrn, last, first, born, sex = patient
    b = datetime.strptime(born, '%Y-%m-%d')
    age = when.year - b.year - ((when.month, when.day) < (b.month, b.day))
    edit = when + timedelta(days=1, hours=2)
    values = {
        'PatientID': mrn, 'PatientLastName': last.upper(), 'PatientFirstName': first.upper(), 'DateofBirth': f'{b:%m-%d-%Y}',
        'PatientAge': str(age), 'AgeUnits': 'YEARS', 'Gender': 'FEMALE' if sex == 'F' else 'MALE', 'Race': '',
        'SecondaryID': '', 'OverreaderLastName': 'RIVERA', 'OverreaderFirstName': 'A', 'EditorLastName': 'RIVERA',
        'EditorFirstName': 'A', 'AcquisitionTechLastName': 'TECH', 'AcquisitionTechFirstName': 'DEMO', 'OverreaderID': '99',
        'EditorID': '99', 'SiteName': 'Synthetic Hospital', 'LocationName': 'CARDIOLOGY', 'Location': '1', 'Site': '1',
        'CartNumber': '1', 'AcquisitionDate': f'{when:%m-%d-%Y}', 'AcquisitionTime': f'{when:%H:%M:%S}',
        'EditDate': f'{edit:%m-%d-%Y}', 'EditTime': f'{edit:%H:%M:%S}', 'TestReason': '', 'Answer': '',
    }
    for tag, v in values.items():
        text = re.sub(rf'(<{tag}>)[^<]*(</{tag}>)', lambda m, v=v: m.group(1) + escape(v) + m.group(2), text)
    # the order (accounts, physicians) and any free text naming people
    text = re.sub(r'<Order>.*?</Order>', '', text, flags=re.S)
    return text


def add_examples(src, out, ecgs):
    """The local examples (`src`, e.g. sample_data/sample-ecg) given to demo patients, as rows of
    the ECG database.  Returns how many were added."""
    sys.path.insert(0, str(ROOT / 'viewers' / 'ecg' / 'python' / 'muse'))
    from muse_reader_py import read_muse
    src = Path(src)
    if not src.is_dir():
        print(f'no examples in {src}', file=sys.stderr)
        return 0
    by_patient = {p[0]: p for p in PATIENTS}
    # who gets which: the files of one record (D0_000306.hea, D0_000306_01.pdf …) go to one
    # demo patient who fits what they say (record 00306: female, 61; 00001: male, 52); a MUSE
    # file (of a female patient) to a female patient
    fits = {'000306': ('P900005', 'E9100009', t('2024-02-15 08:05')), '000001': ('P900006', '', t('2024-04-02 10:15'))}
    other = ('P900004', 'E9100008', t('2024-03-01 10:40'))
    muse = ('P900003', 'E9100006', t('2024-03-11 07:12'))
    files = sorted(f for f in src.iterdir() if f.suffix.lower() in ('.xml', '.hea', '.pdf', '.png', '.jpg', '.jpeg'))
    used = {}
    n = 0
    for f in files:
        import re
        rec = (re.search(r'_(\d{6})(?:_|$)', f.stem) or [None, ''])[1]
        slot = muse if f.suffix.lower() == '.xml' else fits.get(rec, other)
        pid, eid, when = slot
        k = used.get(pid, 0)
        used[pid] = k + 1
        when = when + timedelta(minutes=7 * k)
        name = f'example_{pid}_{when:%Y%m%d_%H%M}'
        meas, interp = {}, ''
        if f.suffix.lower() == '.xml':
            raw = f.read_bytes()
            enc = 'iso-8859-1' if b'ISO-8859-1' in raw[:200].upper() else 'utf-8'
            dest = out / 'ecg' / f'{name}.xml'
            dest.write_bytes(muse_deidentified(raw.decode(enc, 'replace'), by_patient[pid], when).encode(enc, 'replace'))
            try:
                m = read_muse(str(dest))['meta']
                meas = {x['name']: x['value'] for x in m['measurements']}
                st = m['statements']
                interp = '; '.join((st[0]['lines'] if st else []))
            except ValueError as e:
                print(f'{f.name}: not a MUSE file the reader reads ({e})', file=sys.stderr)
            file = dest.name
        elif f.suffix.lower() == '.hea':
            # a WFDB record: its header and the signal files it names, under its own name
            for g in [f, *src.glob(f'{f.stem}.*')]:
                if g.suffix.lower() != '.hea' and g.suffix.lower() not in ('.dat', '.mat'):
                    continue
                (out / 'ecg' / g.name).write_bytes(g.read_bytes())
            file = f.name
        else:
            dest = out / 'ecg' / f'{name}{f.suffix.lower()}'
            dest.write_bytes(f.read_bytes())
            file = dest.name
        hr = meas.get('Vent. rate', '')
        ecgs.append((pid, eid, fmt(when), '12-lead ECG', hr, 'bpm' if hr != '' else '',
                     *[meas.get(c, '') for c in ECG_COLUMNS], interp or f'Example ({f.suffix.lstrip(".").upper()})', f'../ecg/{file}'))
        n += 1
    return n


# ------------------------------------------------------------------ local examples of DICOM studies

# folder or file in --dicom DIR -> who gets it: (patient, encounter, time, database, study, impression,
# the copy's name)
# (the impressions are invented for the demo; the images come from public collections)
DICOM_EXAMPLES = [
    ('ct-chest-lidc-idri-series', 'P900001', 'E9100002', '2024-01-03 11:15', 'imaging', 'CT chest with contrast',
     'Bilateral lower-lobe consolidation with small effusions; a few small pulmonary nodules, follow-up CT advised. No pulmonary embolism.', 'ct_chest'),
    ('ct-lung-screening-nlst-series', 'P900002', 'E9100004', '2024-01-16 10:05', 'imaging', 'Low-dose CT chest',
     'Right lower lobe consolidation; no cavitation, no effusion.', 'ct_chest_lowdose'),
    ('ct-pancreas-pancreas-ct-series', 'P900003', 'E9100006', '2024-03-10 09:40', 'imaging', 'CT abdomen and pelvis',
     'Dilated appendix with periappendiceal fat stranding: acute appendicitis.', 'ct_abdomen'),
    ('ct-abdomen-c4kc-kits-series', 'P900005', 'E9100009', '2024-02-16 13:30', 'imaging', 'CT abdomen with contrast (arterial phase)',
     'No mesenteric ischaemia. Incidental renal mass: urology referral.', 'ct_abdomen_arterial'),
    ('2_skull_ct/DICOM', 'P900004', 'E9100008', '2024-03-01 12:10', 'imaging', 'CT head without contrast',
     'No acute intracranial abnormality.', 'ct_head'),
    ('series-000001', 'P900006', '', '2024-04-03 09:00', 'imaging', 'Dental panoramic X-ray', 'Class III malocclusion.', 'panoramic'),
    ('series-000001 2', 'P900006', '', '2024-04-17 14:00', 'imaging', 'Retinal photographs', 'Hard exudates and dot haemorrhages: diabetic retinopathy.', 'fundus'),
    ('Class 3 malocclusion/DICOM', 'P900006', '', '2024-04-03 09:40', 'imaging', 'Cone-beam CT, maxillofacial',
     'Class III skeletal relationship; for orthodontic and surgical planning.', 'cbct'),
    ('0002.DCM', 'P900001', 'E9100002', '2023-12-29 08:25', 'cath', 'Coronary angiography (RCA)', '', 'rca'),
]
# elements that identify someone or somewhere, removed wherever they are (person names, VR PN, too)
IDENTIFYING = {
    0x00080050, 0x00080080, 0x00080081, 0x00080082, 0x00080090, 0x00080092, 0x00080094, 0x00081010, 0x00081040,
    0x00081048, 0x00081049, 0x00081050, 0x00081052, 0x00081060, 0x00081062, 0x00081070, 0x00081072, 0x00181000,
    0x00181030, 0x00200010, 0x00321032, 0x00321033, 0x00380010, 0x00400244, 0x00400245, 0x00400253, 0x00400254,
    0x00400275, 0x0040A124, 0x00400555, 0x00402016, 0x00402017, 0x00401001,
}
DATE_TIME = [(0x00080020, 0x00080030), (0x00080021, 0x00080031), (0x00080022, 0x00080032), (0x00080023, 0x00080033),
             (0x00080012, 0x00080013)]


def dicom_deidentified(ds, patient, shift, uids):
    """A pydicom dataset as a demo patient's (in place): see the module's help (--dicom)."""
    from pydicom.uid import generate_uid
    pid, mrn, last, first, born, sex = patient

    def new_uid(old):
        if old not in uids:
            uids[old] = generate_uid()
        return uids[old]

    def walk(d):
        for el in list(d):
            # the patient (0010) and clinical trial (0012) modules go; the demo's identity is set below
            if el.tag.is_private or el.tag.group in (0x0010, 0x0012) or int(el.tag) in IDENTIFYING or el.VR == 'PN':
                del d[el.tag]
            elif el.VR == 'UI' and el.value and not str(el.value).startswith('1.2.840.10008.'):
                # instance, series, study, frame of reference UIDs (not the standard's: classes, syntaxes)
                el.value = new_uid(str(el.value))
            elif el.VR == 'SQ':
                for item in el.value:
                    walk(item)
            elif el.VR == 'DT' and el.value:
                try:
                    v = datetime.strptime(str(el.value)[:14], '%Y%m%d%H%M%S') + shift
                    el.value = f'{v:%Y%m%d%H%M%S}'
                except ValueError:
                    del d[el.tag]
    # dates and times, together where they come in pairs; a date alone moves by whole days
    for dt, tm in DATE_TIME:
        if dt in ds and ds[dt].value:
            try:
                tv = str(ds[tm].value).split('.')[0].ljust(6, '0')[:6] if tm in ds and ds[tm].value else '000000'
                v = datetime.strptime(str(ds[dt].value) + tv, '%Y%m%d%H%M%S') + shift
                ds[dt].value = f'{v:%Y%m%d}'
                if tm in ds:
                    ds[tm].value = f'{v:%H%M%S}'
            except ValueError:
                del ds[dt]
    walk(ds)
    for el in list(ds):
        if el.VR == 'DA' and el.value and int(el.tag) not in [d for d, _ in DATE_TIME]:
            try:
                ds[el.tag].value = f'{datetime.strptime(str(el.value), "%Y%m%d") + timedelta(days=round(shift.total_seconds() / 86400)):%Y%m%d}'
            except ValueError:
                del ds[el.tag]
    ds.PatientName = f'{last}^{first}'
    ds.PatientID = mrn
    ds.PatientBirthDate = born.replace('-', '')
    ds.PatientSex = sex
    when = datetime.strptime(str(ds.StudyDate), '%Y%m%d') if 'StudyDate' in ds else None
    if when:
        b = datetime.strptime(born, '%Y-%m-%d')
        ds.PatientAge = f'{when.year - b.year - ((when.month, when.day) < (b.month, b.day)):03d}Y'
    ds.PatientIdentityRemoved = 'YES'
    ds.DeidentificationMethod = 'Medorama demo: identity replaced, dates shifted'
    ds.SOPInstanceUID = new_uid(str(ds.SOPInstanceUID))
    ds.file_meta.MediaStorageSOPInstanceUID = ds.SOPInstanceUID
    return ds


def add_dicom_examples(src, out, imaging, caths):
    """The local DICOM studies (`src`) given to demo patients (DICOM_EXAMPLES), as rows of the
    imaging and cath databases.  Returns the (patient, time) of the studies they replace."""
    try:
        import pydicom
    except ImportError:
        sys.exit('--dicom needs pydicom: run this with the repository\'s .venv/bin/python')
    src = Path(src)
    by_patient = {p[0]: p for p in PATIENTS}
    done = set()
    for name, pid, eid, when, db, study, impression, stem in DICOM_EXAMPLES:
        path = src / name
        if not path.exists():
            print(f'--dicom: no {name} in {src}', file=sys.stderr)
            continue
        files = sorted(f for f in path.rglob('*') if f.is_file()) if path.is_dir() else [path]
        target = t(when).replace(tzinfo=None)   # DICOM times have no zone
        shift, uids = None, {}
        base = f'cath_{pid}_{target:%Y%m%d}_{stem}' if db == 'cath' else f'{stem}_{pid}_{target:%Y%m%d}'
        dest = out / db / base
        n = 0
        for f in files:
            if f.name == 'CITATION.txt':
                dest.mkdir(parents=True, exist_ok=True)
                (dest / f.name).write_bytes(f.read_bytes())
                continue
            if f.name.startswith('.') or f.name.upper() == 'DICOMDIR':
                continue
            try:
                ds = pydicom.dcmread(f)
            except Exception:  # not DICOM: a preview picture, a note — left out (it may name someone)
                continue
            if shift is None:
                # the study's first date and time, moved to the demo's
                d = next((str(ds.get(k)) for k in ('StudyDate', 'SeriesDate', 'AcquisitionDate', 'ContentDate') if ds.get(k)), '')
                tm = next((str(ds.get(k)).split('.')[0].ljust(6, '0')[:6] for k in ('StudyTime', 'SeriesTime', 'AcquisitionTime') if ds.get(k)), '000000')
                try:
                    shift = target - datetime.strptime(d + tm, '%Y%m%d%H%M%S')
                except ValueError:
                    shift = timedelta(0)
                    ds.StudyDate, ds.StudyTime = f'{target:%Y%m%d}', f'{target:%H%M%S}'
            dicom_deidentified(ds, by_patient[pid], shift, uids)
            if path.is_dir():
                dest.mkdir(parents=True, exist_ok=True)
                ds.save_as(dest / f'{n + 1:04d}.dcm', enforce_file_format=True)
            else:
                dest = out / db / f'{base}.dcm'
                ds.save_as(dest, enforce_file_format=True)
            n += 1
        if not n:
            continue
        link = f'../{db}/{base}/' if path.is_dir() else f'../{db}/{base}.dcm'
        if db == 'cath':
            caths.append((pid, eid, when, study, '', '', '', impression, link))
        else:
            imaging.append((pid, eid, when, study, 'imaging', impression, link))
        done.add((pid, when))
        print(f'--dicom: {name}: {n} file(s) to {pid}')
    return done


# ------------------------------------------------------------------ an OMOP database (invented)

OMOP_TABLES = '''
CREATE TABLE person (person_id INTEGER, gender_concept_id INTEGER, year_of_birth INTEGER, month_of_birth INTEGER,
    day_of_birth INTEGER, birth_datetime TEXT, person_source_value TEXT);
CREATE TABLE care_site (care_site_id INTEGER, care_site_name TEXT);
CREATE TABLE visit_occurrence (visit_occurrence_id INTEGER, person_id INTEGER, visit_concept_id INTEGER, visit_start_date TEXT,
    visit_start_datetime TEXT, visit_end_date TEXT, visit_end_datetime TEXT, care_site_id INTEGER);
CREATE TABLE visit_detail (visit_detail_id INTEGER, person_id INTEGER, visit_detail_concept_id INTEGER, visit_occurrence_id INTEGER);
CREATE TABLE measurement (measurement_id INTEGER, person_id INTEGER, measurement_concept_id INTEGER, measurement_date TEXT,
    measurement_datetime TEXT, value_as_number REAL, value_as_concept_id INTEGER, unit_concept_id INTEGER, range_low REAL,
    range_high REAL, visit_occurrence_id INTEGER, measurement_source_value TEXT, unit_source_value TEXT, value_source_value TEXT);
CREATE TABLE observation (observation_id INTEGER, person_id INTEGER, observation_concept_id INTEGER, observation_date TEXT,
    observation_datetime TEXT, value_as_number REAL, value_as_string TEXT, value_as_concept_id INTEGER, unit_concept_id INTEGER,
    visit_occurrence_id INTEGER, observation_source_value TEXT, unit_source_value TEXT);
CREATE TABLE condition_occurrence (condition_occurrence_id INTEGER, person_id INTEGER, condition_concept_id INTEGER,
    condition_start_date TEXT, condition_start_datetime TEXT, condition_end_date TEXT, condition_end_datetime TEXT,
    visit_occurrence_id INTEGER, condition_source_value TEXT, condition_source_concept_id INTEGER);
CREATE TABLE drug_exposure (drug_exposure_id INTEGER, person_id INTEGER, drug_concept_id INTEGER, drug_exposure_start_date TEXT,
    drug_exposure_start_datetime TEXT, drug_exposure_end_date TEXT, drug_exposure_end_datetime TEXT, visit_occurrence_id INTEGER,
    drug_source_value TEXT, dose_unit_source_value TEXT);
CREATE TABLE procedure_occurrence (procedure_occurrence_id INTEGER, person_id INTEGER, procedure_concept_id INTEGER,
    procedure_date TEXT, procedure_datetime TEXT, visit_occurrence_id INTEGER, procedure_source_value TEXT);
CREATE TABLE note (note_id INTEGER, person_id INTEGER, note_date TEXT, note_datetime TEXT, note_type_concept_id INTEGER,
    note_title TEXT, note_text TEXT, provider_id INTEGER, visit_occurrence_id INTEGER);
CREATE TABLE provider (provider_id INTEGER, provider_name TEXT);
CREATE TABLE concept (concept_id INTEGER, concept_name TEXT, vocabulary_id TEXT, concept_code TEXT);
CREATE TABLE cdm_source (cdm_source_name TEXT, source_release_date TEXT, cdm_release_date TEXT);
'''

# concept_id, name, vocabulary, code (the few the demo uses; real OMOP vocabularies hold millions)
OMOP_CONCEPTS = [
    (3047181, 'Lactate [Moles/volume] in Blood', 'LOINC', '2524-7'),
    (3016723, 'Creatinine [Mass/volume] in Serum or Plasma', 'LOINC', '2160-0'),
    (3019550, 'Troponin T.cardiac [Mass/volume] in Serum or Plasma', 'LOINC', '6598-7'),
    (3000963, 'Hemoglobin [Mass/volume] in Blood', 'LOINC', '718-7'),
    (3027018, 'Heart rate', 'LOINC', '8867-4'),
    (3004249, 'Systolic blood pressure', 'LOINC', '8480-6'),
    (8753, 'millimole per liter', 'UCUM', 'mmol/L'), (8840, 'milligram per deciliter', 'UCUM', 'mg/dL'),
    (8842, 'nanogram per liter', 'UCUM', 'ng/L'), (8713, 'gram per deciliter', 'UCUM', 'g/dL'),
    (8541, 'per minute', 'UCUM', '/min'), (8876, 'millimeter mercury column', 'UCUM', 'mm[Hg]'),
    (4329847, 'Myocardial infarction', 'SNOMED', '22298006'), (45576876, 'Non-ST elevation myocardial infarction', 'ICD10CM', 'I21.4'),
    (201826, 'Type 2 diabetes mellitus', 'SNOMED', '44054006'), (45591076, 'Type 2 diabetes mellitus without complications', 'ICD10CM', 'E11.9'),
    (132797, 'Sepsis', 'SNOMED', '91302008'), (45533176, 'Sepsis, unspecified organism', 'ICD10CM', 'A41.9'),
    (1367571, 'heparin', 'RxNorm', '5224'), (1503297, 'metformin', 'RxNorm', '6809'),
    (1717327, 'piperacillin / tazobactam', 'RxNorm', '74169'),
    (4006969, 'Coronary angiography', 'SNOMED', '33367005'),
    (9201, 'Inpatient Visit', 'Visit', 'IP'), (9202, 'Outpatient Visit', 'Visit', 'OP'),
    (44814637, 'Discharge summary', 'Type Concept', 'X'),
]


def write_omop(path):
    """An invented OMOP CDM database: another hospital's, with two of the demo patients (by
    their MRNs: SYN0000002, SYN0000005) and two patients of its own."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    db = sqlite3.connect(path)
    db.executescript(OMOP_TABLES)
    db.executemany('INSERT INTO concept VALUES (?, ?, ?, ?)', OMOP_CONCEPTS)
    # person_id, sex concept, born, MRN (person_source_value)
    persons = [(1, 8507, '1961-03-05', 'SYN0000002'), (2, 8532, '1966-06-14', 'SYN0000005'),
               (3, 8507, '1949-09-21', 'SYN0000101'), (4, 8532, '1983-12-02', 'SYN0000102')]
    db.executemany('INSERT INTO person VALUES (?, ?, ?, ?, ?, NULL, ?)',
                   [(p, g, int(b[:4]), int(b[5:7]), int(b[8:]), m) for p, g, b, m in persons])
    db.executemany('INSERT INTO care_site VALUES (?, ?)', [(1, 'County General MICU'), (2, 'County General Cardiology'),
                                                          (3, 'County General Clinic')])
    # visit, person, kind, start, end, care site, intensive care
    visits = [(1, 1, 9201, '2023-09-02 14:00', '2023-09-08 11:00', 2, False),
              (2, 2, 9202, '2023-11-20 09:00', '2023-11-20 10:00', 3, False),
              (3, 3, 9201, '2024-01-04 22:00', '2024-01-12 15:00', 1, True),
              (4, 4, 9202, '2024-02-07 13:00', '2024-02-07 13:40', 3, False)]
    db.executemany('INSERT INTO visit_occurrence VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
                   [(v, p, k, a[:10], a + ':00', b[:10], b + ':00', cs) for v, p, k, a, b, cs, _ in visits])
    db.executemany('INSERT INTO visit_detail VALUES (?, ?, 32037, ?)', [(v, p, v) for v, p, *_, icu in visits if icu])
    # measurement concept, unit concept, low, high, then (person, visit, time, value) …
    results = [
        (3047181, 8753, 0.5, 2.0, [(1, 1, '2023-09-02 15:10', 3.4), (1, 1, '2023-09-03 06:00', 1.9), (3, 3, '2024-01-04 23:00', 5.2),
                                   (3, 3, '2024-01-05 06:00', 3.8), (3, 3, '2024-01-06 06:00', 1.6)]),
        (3016723, 8840, 0.6, 1.2, [(1, 1, '2023-09-02 15:10', 1.4), (2, 2, '2023-11-20 09:15', 0.8), (3, 3, '2024-01-05 06:00', 2.1)]),
        (3019550, 8842, 0, 14, [(1, 1, '2023-09-02 15:10', 412), (1, 1, '2023-09-03 06:00', 655), (3, 3, '2024-01-05 06:00', 22)]),
        (3000963, 8713, 12, 16, [(2, 2, '2023-11-20 09:15', 12.9), (4, 4, '2024-02-07 13:10', 10.6)]),
        (3027018, 8541, 60, 100, [(1, 1, '2023-09-02 14:30', 104), (3, 3, '2024-01-04 22:30', 121), (3, 3, '2024-01-05 08:00', 98)]),
        (3004249, 8876, 90, 140, [(1, 1, '2023-09-02 14:30', 152), (3, 3, '2024-01-04 22:30', 84)]),
    ]
    m = 0
    for concept, unit, low, high, rows in results:
        for person, visit, t, value in rows:
            m += 1
            db.execute('INSERT INTO measurement VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, NULL, NULL, NULL)',
                       (m, person, concept, t[:10], t + ':00', value, unit, low, high, visit))
    # condition (SNOMED), its ICD-10 source concept and code, person, visit, start
    for i, (c, sc, code, person, visit, t) in enumerate([
            (4329847, 45576876, 'I21.4', 1, 1, '2023-09-02 15:00'), (201826, 45591076, 'E11.9', 2, 2, '2023-11-20 09:30'),
            (132797, 45533176, 'A41.9', 3, 3, '2024-01-04 23:30'), (201826, 45591076, 'E11.9', 3, 3, '2024-01-05 09:00')], 1):
        db.execute('INSERT INTO condition_occurrence VALUES (?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?)',
                   (i, person, c, t[:10], t + ':00', visit, code, sc))
    for i, (c, person, visit, a, b, unit) in enumerate([
            (1367571, 1, 1, '2023-09-02 16:00', '2023-09-06 16:00', 'U'), (1503297, 2, 2, '2023-11-20 10:00', '2024-05-20 10:00', 'mg'),
            (1717327, 3, 3, '2024-01-05 00:30', '2024-01-11 00:30', 'g')], 1):
        db.execute('INSERT INTO drug_exposure VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)',
                   (i, person, c, a[:10], a + ':00', b[:10], b + ':00', visit, unit))
    db.execute("INSERT INTO procedure_occurrence VALUES (1, 1, 4006969, '2023-09-03', '2023-09-03 10:00:00', 1, NULL)")
    db.executemany('INSERT INTO provider VALUES (?, ?)', [(1, 'Dr. Avery Quinn'), (2, 'Dr. Morgan Ellis')])
    notes = [
        (1, 1, '2023-09-08 10:00', 'Discharge summary',
         'Admitted with chest pain and a non-ST elevation myocardial infarction; troponin peaked at 655 ng/L. '
         'Coronary angiography: a tight LAD stenosis, stented. Discharged on dual antiplatelet therapy.', 2, 1),
        (2, 3, '2024-01-12 14:00', 'Discharge summary',
         'Sepsis of urinary origin with a lactate of 5.2 mmol/L on arrival, treated with piperacillin/tazobactam in the '
         'MICU. Lactate normalised within 48 hours.', 1, 3),
    ]
    db.executemany("INSERT INTO note VALUES (?, ?, ?, ?, 44814637, ?, ?, ?, ?)",
                   [(n, p, t[:10], t + ':00', title, text, prov, v) for n, p, t, title, text, prov, v in notes])
    db.execute("INSERT INTO cdm_source VALUES ('County General (synthetic)', '2024-03-01', '2024-03-02')")
    db.commit()
    db.close()


def main():
    args = sys.argv[1:]
    examples = dicoms = None
    if '--examples' in args:
        i = args.index('--examples')
        examples = args[i + 1] if i + 1 < len(args) else ''
        del args[i:i + 2]
    if '--dicom' in args:
        i = args.index('--dicom')
        dicoms = args[i + 1] if i + 1 < len(args) else ''
        del args[i:i + 2]
    if any(a in ('-h', '--help') for a in args):
        print(__doc__)
        return
    unknown = [a for a in args if a.startswith('-')]
    if unknown or len(args) > 1:
        sys.exit(f'make-demo-data.py: unexpected {" ".join(unknown or args[1:])} (--help shows the options)')
    out = Path(args[0] if args else ROOT / 'sample_data' / 'demo').resolve()
    for d in ('icu', 'holter', 'sleep', 'ecg', 'imaging', 'echo', 'cath', 'databases'):
        (out / d).mkdir(parents=True, exist_ok=True)
    db = out / 'databases'
    by_patient = {p[0]: p for p in PATIENTS}

    write_csv(db / 'patients.csv', ['MRN', 'Last Name', 'First Name', 'DOB', 'Sex', 'Patient ID'],
              [(p[1], p[2], p[3], p[4], p[5], p[0]) for p in PATIENTS])

    rows = [['Encounter ID', 'Patient ID', 'CSN', 'Admit Time', 'Discharge Time', 'Admit Unit', 'Disposition', 'Care Unit', 'Bed', 'Bed Start', 'Bed End']]
    for e in ENCOUNTERS:
        rows.append([e[0], e[1], e[2], e[4], e[5], e[6], e[7], '', '', '', ''])
        if e[8]:
            rows.append([e[0], e[1], e[2], e[4], e[5], e[6], e[7], e[8][0], e[8][1], e[8][2], e[8][3]])
    write_xlsx(db / 'encounters.xlsx', rows)

    labs, vitals, meds, dx, micro, notes, ecgs, icu, imaging, echos, caths = [], [], [], [], [], [], [], [], [], [], []
    last_ecg = {}            # each patient's previous ECG: (time, ST depression, AF), for 12SL's comparison
    for e in ENCOUNTERS:
        eid, pid, _, kind, a, b = e[0], e[1], e[2], e[3], t(e[4]), t(e[5])
        mrn = MRN[pid]
        p = by_patient[pid]
        ill = lambda when, e=e: illness(e, when)
        if e[6] == 'Sleep laboratory':
            # an overnight study: its recording (written below) and its report, nothing else
            notes.append((pid, eid, fmt(b - timedelta(minutes=30)), 'Sleep study report', 'Dr. S. Lee', 'Sleep medicine',
                          'Overnight polysomnography, 2 h recorded (EEG, EOG, ECG, airflow, thoracic effort, SpO2).\n'
                          'Obstructive apneas with oxygen desaturations to about 89 %; apnea-hypopnea index about 8 per hour.\n'
                          'Sinus rhythm throughout.\nImpression: mild obstructive sleep apnea. Discuss treatment at the next visit.'))
            continue
        # laboratory: a morning panel each day, every 6 h in the ICU; a small panel at visits
        times = []
        if kind == 'out':
            times = [a + timedelta(minutes=40)]
        else:
            d = a.replace(hour=6, minute=10)
            if d < a:
                d += timedelta(days=1)
            while d < b:
                times.append(d)
                if e[8] and t(e[8][2]) <= d <= t(e[8][3]):
                    times += [d + timedelta(hours=h) for h in (6, 12, 18)]
                d += timedelta(days=1)
            times.insert(0, a + timedelta(minutes=50))
        for when in sorted(times):
            s = ill(when)
            for name, loinc, unit, lo, hi, norm, sick in LABS:
                if kind == 'out' and name not in ('Sodium', 'Potassium', 'Creatinine', 'Glucose', 'Hemoglobin', 'WBC'):
                    continue
                if name in ('Lactate', 'Procalcitonin') and s < 0.2 and when.hour != 6:
                    continue
                if name == 'Troponin T' and not (eid == 'E9100002' and when < a + timedelta(days=3)):
                    continue
                v = norm + (sick - norm) * s + (norm * 0.03) * R.uniform(-1, 1)
                v = round(v, 1 if abs(norm) < 20 else 0)
                labs.append((mrn, eid, name, v, unit, lo, hi, flag(v, lo, hi), fmt(when), loinc, 'LOINC'))
        # vital signs: hourly in the ICU, every 4 h on the ward, once at a visit
        when = a
        while when < b:
            in_icu = bool(e[8]) and t(e[8][2]) <= when <= t(e[8][3])
            if in_icu or when.hour % 4 == 0 or kind == 'out':
                s = ill(when)
                for name, unit, lo, hi, v in (('Heart rate', '/min', 50, 110, 76 + 40 * s), ('SpO2', '%', 92, 100, 97 - 5 * s),
                                              ('Respiratory rate', '/min', 10, 22, 15 + 10 * s), ('Systolic blood pressure', 'mmHg', 90, 140, 128 - 36 * s),
                                              ('Temperature', '°C', 36.0, 38.0, 36.8 + 2.1 * s)):
                    v = round(v + R.uniform(-2, 2) * (0.1 if unit == '°C' else 1), 1)
                    vitals.append((pid, eid, name, v, unit, lo, hi, fmt(when)))
                if kind == 'out':
                    break
            when += timedelta(hours=1)
        # notes
        if kind == 'out':
            notes.append((pid, eid, fmt(a + timedelta(minutes=55)), 'Clinic note', 'Dr. M. Patel', 'Cardiology',
                          f'{p[3]} returns for follow-up.\nNo chest pain, no dyspnea at rest. Medications reviewed.\n'
                          'Exam: regular rhythm, no murmur, lungs clear.\nPlan: continue current therapy; follow-up in 3 months.'))
        else:
            notes.append((pid, eid, fmt(a + timedelta(hours=2)), 'Admission note', 'Dr. A. Rivera', e[6],
                          f'Admitted from the emergency department. History of present illness: {p[3]} presented with acute symptoms.\n'
                          'Assessment: see problem list. Plan: admit, monitor, labs in the morning.'))
            d = a.replace(hour=15, minute=40) + timedelta(days=1)
            day = 1
            while d < b - timedelta(hours=6):
                in_icu = bool(e[8]) and t(e[8][2]) <= d <= t(e[8][3])
                s = ill(d)
                text = ('Subjective: overnight events reviewed.\n'
                        f'Objective: HR {round(76 + 40 * s)}, SpO2 {round(97 - 5 * s)} %, temperature {36.8 + 2.1 * s:.1f} °C.\n'
                        f'Assessment: {"septic shock secondary to E. coli bacteremia, improving" if eid == "E9100002" and s > 0.4 else "clinically stable"}.\n'
                        'Plan: continue current management; repeat labs in the morning.')
                notes.append((pid, eid, fmt(d), 'ICU progress note' if in_icu else 'Progress note',
                              'Dr. K. Osei' if in_icu else 'Dr. A. Rivera', 'ICU' if in_icu else e[6], text))
                d += timedelta(days=1)
                day += 1
            notes.append((pid, eid, fmt(b - timedelta(hours=1)), 'Discharge summary', 'Dr. A. Rivera', e[6],
                          f'Hospital course: {p[3]} was admitted on {a:%b %d}. '
                          f'{"Course complicated by E. coli bacteremia and septic shock requiring norepinephrine. " if eid == "E9100002" else ""}'
                          f'Discharged to {e[7].lower()} in stable condition.'))
        # ECGs: what a cart and MUSE would hold (12SL measurements and statements, the
        # physician's confirmation, sometimes a corrected reading)
        for k, when in enumerate([a + timedelta(minutes=20)] + ([a + timedelta(days=2, hours=3)] if kind == 'in' else [])):
            s = ill(when)
            hr = round(72 + 40 * s)
            af = pid == 'P900005' and k == 0
            st = eid == 'E9100002' and k == 0
            lvh = pid == 'P900004'
            leads = ecg12(hr, st, af)
            meas = ecg_measures(hr, af, lvh)
            name = f'ecg_{pid}_{when:%Y%m%d_%H%M}'
            computer = twelve_sl(meas, af, st, lvh, last_ecg.get(pid))
            last_ecg[pid] = (when, st, af)
            interp = '; '.join(computer)
            reader = ('A. Rivera', when + timedelta(hours=6))
            if st:
                # the cardiologist reads it as ischaemia
                confirmed = [computer[0], 'Inferolateral ST depression, consistent with subendocardial ischemia', 'Abnormal ECG']
            elif af:
                confirmed = None        # not confirmed yet: the computer's reading only
            else:
                confirmed = computer
            if k == 1 and pid in ('P900001', 'P900002'):
                # MUSE XML; for P900001, the physician corrected the computer's reading
                if pid == 'P900001':
                    confirmed = [computer[0], 'Nonspecific T wave abnormality', 'Borderline ECG']
                file = ecg_muse(out / 'ecg' / f'{name}.xml', p, when, leads, meas, computer, confirmed or computer).name
            elif k == 1 and pid == 'P900003':
                ecg_dicom(out / 'ecg' / f'{name}.dcm', MRN[pid], when, leads, meas, computer)
                file = f'{name}.dcm'
            elif eid == 'E9100007':
                ecg_pdf(out / 'ecg' / f'{name}.pdf', leads, f'Synthetic 12-lead ECG (not a patient)   {computer[0]}   {hr} bpm')
                file = f'{name}.pdf'
            elif pid == 'P900003' and kind == 'out':
                ecg_png(out / 'ecg' / f'{name}.png', leads)
                file = f'{name}.png'
            else:
                born = datetime.strptime(p[4], '%Y-%m-%d')
                age = when.year - born.year - ((when.month, when.day) < (born.month, born.day))
                comments = [f'Age: {age}', f'Sex: {p[5]}', 'Device: MAC 5500 HD', 'Location: ' + e[6], 'Filters: 0.05-150 Hz, 60 Hz notch',
                            'Program: Marquette 12SL v243']
                comments += [f'{k2}: {v} {u}' for k2, (v, u) in meas.items()]
                if st:
                    comments += [f'ST {lead}: {v:+.2f} mV' for lead, v in ST_DEPRESSION.items()]
                if lvh:
                    comments += ['R amp V5: 3.4 mV', 'S amp V1: 1.9 mV']
                comments.append(f'Dx: {"; ".join(computer)}')
                if confirmed:
                    comments += [f'Dx confirmed: {"; ".join(confirmed)}', f'Confirmed by: {reader[0]}', f'Confirmed at: {reader[1]:%Y-%m-%d %H:%M}']
                file = ecg_wfdb(out / 'ecg', name, when, leads, comments=comments).name
            ecgs.append((pid, eid, fmt(when), '12-lead ECG', hr, 'bpm', *[meas[c][0] if c in meas else '' for c in ECG_COLUMNS], interp, f'../ecg/{file}'))
        # ICU recordings: two 10-minute pieces of each ICU stay, named as bedside monitors name
        # them (UNIT_BED-<Unix time>), which links them to their bed stay
        if e[8]:
            unit, bed, i0, i1 = e[8]
            for frac in (0.3, 0.62):
                st = t(i0) + (t(i1) - t(i0)) * frac
                st = st.replace(second=0, microsecond=0)
                fname = '_'.join((unit, bed)) + '-' + str(int(st.timestamp()))
                recording(out / 'icu' / fname, st, 10, ill)
                icu.append((pid, eid, f'../icu/{fname}'))
    # medications, diagnoses, microbiology, imaging (by hand, for the main stories)
    meds += [
        ('P900001', 'E9100002', 'Heparin', 18, 'U/kg/h', '2023-12-28 09:00', '2024-01-02 20:00'),
        ('P900001', 'E9100002', 'Piperacillin-tazobactam', 4.5, 'g', '2024-01-01 15:00', '2024-01-09 12:00'),
        ('P900001', 'E9100002', 'Norepinephrine', 0.12, 'ug/kg/min', '2024-01-01 16:00', '2024-01-02 18:00'),
        ('P900001', 'E9100002', 'Metoprolol', 50, 'mg', '2024-01-03 08:00', '2024-01-09 12:00'),
        ('P900001', 'E9100002', 'Insulin glargine', 18, 'U', '2023-12-28 21:00', '2024-01-09 12:00'),
        ('P900002', 'E9100004', 'Propofol', 30, 'ug/kg/min', '2024-01-15 04:30', '2024-01-17 09:00'),
        ('P900002', 'E9100004', 'Ceftriaxone', 2, 'g', '2024-01-15 06:00', '2024-01-21 12:00'),
        ('P900003', 'E9100006', 'Cefazolin', 2, 'g', '2024-03-10 12:00', '2024-03-11 12:00'),
        ('P900003', 'E9100006', 'Morphine', 2, 'mg', '2024-03-10 15:00', '2024-03-12 15:00'),
        ('P900005', 'E9100009', 'Amiodarone', 1, 'mg/min', '2024-02-13 02:00', '2024-02-14 14:00'),
        ('P900005', 'E9100009', 'Apixaban', 5, 'mg', '2024-02-15 08:00', '2024-02-20 10:00'),
    ]
    dx += [
        ('P900001', 'E9100001', 'E11.9', 'Type 2 diabetes mellitus', '2023-10-12 09:00'),
        ('P900001', 'E9100002', 'I21.4', 'Non-ST elevation myocardial infarction', '2023-12-28 09:00'),
        ('P900001', 'E9100002', 'A41.51', 'Sepsis due to Escherichia coli', '2024-01-02 01:05'),
        ('P900001', 'E9100002', 'N17.9', 'Acute kidney injury', '2024-01-01 12:00'),
        ('P900002', 'E9100004', 'J96.01', 'Acute respiratory failure with hypoxia', '2024-01-15 03:00'),
        ('P900002', 'E9100004', 'J18.9', 'Pneumonia', '2024-01-15 03:00'),
        ('P900003', 'E9100005', 'R10.31', 'Right lower quadrant pain', '2024-02-02 19:00'),
        ('P900003', 'E9100006', 'K35.80', 'Acute appendicitis', '2024-03-10 11:30'),
        ('P900004', 'E9100007', 'I10', 'Essential hypertension', '2023-11-05 14:30'),
        ('P900005', 'E9100009', 'I48.91', 'Atrial fibrillation', '2024-02-12 21:00'),
    ]
    micro += [
        ('P900001', 'E9100002', 'Blood culture', '2024-01-01 15:30', 'Escherichia coli, ESBL negative', 'positive', 'Notify team; sepsis bundle in progress'),
        ('P900001', 'E9100002', 'Urine culture', '2024-01-01 15:40', 'Escherichia coli > 100,000 CFU/mL', 'positive', ''),
        ('P900002', 'E9100004', 'Sputum culture', '2024-01-15 05:00', 'Streptococcus pneumoniae', 'positive', ''),
        ('P900005', 'E9100009', 'Blood culture', '2024-02-13 03:00', 'No growth at 5 days', '', ''),
    ]
    real = add_dicom_examples(dicoms, out, imaging, caths) if dicoms is not None else set()
    for pid, eid, when, name, kind, report in [
        ('P900001', 'E9100002', '2023-12-28 08:30', 'Chest X-ray', 'cxr', 'No acute cardiopulmonary process. Heart size upper normal.'),
        ('P900001', 'E9100002', '2024-01-03 11:15', 'CT chest with contrast', 'ct', 'Bilateral lower-lobe consolidation with small effusions; no pulmonary embolism.'),
        ('P900002', 'E9100004', '2024-01-15 03:20', 'Chest X-ray', 'cxr', 'Right lower lobe consolidation consistent with pneumonia.'),
        ('P900003', 'E9100006', '2024-03-10 09:40', 'CT abdomen and pelvis', 'ct', 'Dilated appendix with periappendiceal fat stranding: acute appendicitis.'),
    ]:
        if (pid, when) in real:
            continue
        base = f'{kind}_{pid}_{t(when):%Y%m%d}'
        if kind == 'ct':
            ct_series(out / 'imaging' / base, 'abdomen' if 'abdomen' in name else 'chest', pid, t(when))
            file = f'{base}/'
        elif kind == 'cxr':
            chest_xray(out / 'imaging' / f'{base}.dcm', pid, t(when), opacity='right' if pid == 'P900002' else None)
            file = f'{base}.dcm'
        imaging.append((pid, eid, when, name, 'imaging', report, f'../imaging/{file}'))
    # echocardiography: a database of its own, its measurements as columns (from the reports);
    # loops in DICOM, one loop as PNG frames (its scale and frame rate are columns)
    for pid, eid, when, ef, lvidd, impression, raw in [
        ('P900001', 'E9100002', '2023-12-30 09:40', 0.35, 6.1, 'LVEF 35 %, inferior hypokinesis, mild mitral regurgitation.', False),
        ('P900004', 'E9100007', '2023-11-05 14:20', 0.6, 4.8, 'LVEF 60 %, concentric LV hypertrophy, normal valves.', True),
    ]:
        base = f'echo_{pid}_{t(when):%Y%m%d}'
        study = uid()
        (out / 'echo').mkdir(exist_ok=True)
        echo_loop(out / 'echo' / f'{base}_a4c.dcm', pid, t(when), ef=ef, study=study)
        echo_report(out / 'echo' / f'{base}_sr.dcm', pid, t(when) + timedelta(minutes=25), ef, lvidd, study)
        tapse = 21 if ef > 0.5 else 15
        echos.append((pid, eid, when, 'Transthoracic echo', 'Apical 4-chamber', '', '', '', '', '', impression, f'../echo/{base}_a4c.dcm'))
        if raw:
            echo_pngs(out / 'echo' / f'{base}_a4c_frames', pid, ef)
            echos.append((pid, eid, when, 'Transthoracic echo', 'Apical 4-chamber (exported frames)', '', '', '', '1', '30', '', f'../echo/{base}_a4c_frames/'))
        echos.append((pid, eid, fmt(t(when) + timedelta(minutes=25)), 'Echo report', 'Report', round(ef * 100), lvidd, tapse, '', '', impression, f'../echo/{base}_sr.dcm'))
    # the cath lab: the NSTEMI's coronary angiography, its findings as columns
    (out / 'cath').mkdir(exist_ok=True)
    angio_run(out / 'cath' / 'cath_P900001_20231229_lca.dcm', 'P900001', t('2023-12-29 08:10'))
    caths.append(('P900001', 'E9100002', '2023-12-29 08:10', 'Coronary angiography', 90, 30, 22,
                  'Severe proximal LAD stenosis (90 %), moderate RCA disease; LVEDP 22 mmHg. PCI to the LAD.', '../cath/cath_P900001_20231229_lca.dcm'))
    # a patient known only by MRN, with laboratory results and no encounter
    for d in range(3):
        when = t('2024-03-20 08:00') + timedelta(days=30 * d)
        for name, loinc, unit, lo, hi, norm, _ in LABS[:8]:
            v = round(norm * R.uniform(0.95, 1.05), 1)
            labs.append(('SYN0000006', '', name, v, unit, lo, hi, flag(v, lo, hi), fmt(when), loinc, 'LOINC'))

    write_csv(db / 'labs.csv', ['MRN', 'Encounter ID', 'Test Name', 'Result', 'Units', 'Ref Low', 'Ref High', 'Abnormal Flag', 'Collected', 'Code', 'Code System'], labs)
    write_csv(db / 'vitals.csv', ['patient_id', 'encounter_id', 'name', 'value', 'unit', 'ref_low', 'ref_high', 'charttime'], vitals)
    write_csv(db / 'medications.csv', ['patient_id', 'encounter_id', 'Medication', 'Dose', 'Units', 'Start Time', 'Stop Time'], meds)
    write_csv(db / 'diagnoses.csv', ['patient_id', 'encounter_id', 'ICD10', 'Description', 'Date'], dx)
    write_csv(db / 'microbiology.csv', ['patient_id', 'encounter_id', 'Test', 'Collected', 'Result', 'Flag', 'Comment'], micro)
    write_csv(db / 'notes.csv', ['patient_id', 'encounter_id', 'time', 'note_type', 'author', 'service', 'note_text'], notes)
    write_csv(db / 'allergies.csv', ['patient_id', 'category', 'name', 'reaction'],
              [('P900001', 'allergy', 'Penicillin', 'rash'), ('P900002', 'allergy', 'Iodinated contrast', 'urticaria')])
    if examples is not None:
        print(f'{add_examples(examples, out, ecgs)} example ECG(s) added from {examples}')
    write_csv(db / 'ecg_index.csv', ['patient_id', 'encounter_id', 'time', 'name', 'value', 'unit',
                                     *[f'{c} ({u})' for c, u in ECG_COLUMNS.items()], 'interpretation', 'file'], ecgs)
    write_csv(db / 'icu_index.csv', ['patient_id', 'encounter_id', 'file'], icu)
    # a 24-hour Holter after a clinic visit (its own column, not the ICU's)
    holter(out / 'holter' / 'P900004_20240301', t('2024-03-01 11:30'))
    write_csv(db / 'holter_index.csv', ['patient_id', 'encounter_id', 'category', 'file'],
              [('P900004', 'E9100008', 'holter', '../holter/P900004_20240301/holter.hea')])
    # an overnight sleep study for the patient with palpitations (an EDF+ file, its own column):
    # 22:30 at the sleep laboratory (New York), which the file states as 03:30 (its clock time is
    # read as UTC)
    sleep_study(out / 'sleep' / 'P900004_psg_20240305.edf', t('2024-03-06 03:30'))
    write_csv(db / 'sleep_index.csv', ['patient_id', 'encounter_id', 'time', 'study', 'category', 'file'],
              [('P900004', 'E9100010', '2024-03-05 22:30', 'Polysomnography', 'sleep', '../sleep/P900004_psg_20240305.edf')])
    write_csv(db / 'imaging_index.csv', ['patient_id', 'encounter_id', 'time', 'study', 'category', 'impression', 'file'], imaging)
    write_csv(db / 'echo_index.csv', ['patient_id', 'encounter_id', 'time', 'study', 'view', 'LVEF (%)', 'LVIDd (cm)', 'TAPSE (mm)',
                                      'pixel spacing (mm)', 'frame rate (fps)', 'impression', 'file'], echos)
    write_csv(db / 'cath_index.csv', ['patient_id', 'encounter_id', 'time', 'procedure', 'LAD stenosis (%)', 'RCA stenosis (%)', 'LVEDP (mmHg)',
                                      'impression', 'file'], caths)
    write_omop(out / 'omop' / 'omop.sqlite')
    (out / 'README.txt').write_text(__doc__.replace('OUT', str(out)), encoding='utf-8')
    # what went in: invented data only, or also local files (--examples, --dicom); a picture of
    # the demo is published only when it holds invented data alone (tools/screenshots.py --demo)
    (out / 'demo.json').write_text(json.dumps({
        'made_by': 'tools/make-demo-data.py',
        'synthetic_only': examples is None and dicoms is None,
        'examples': examples is not None,
        'dicom': dicoms is not None,
    }, indent=2) + '\n', encoding='utf-8')
    print(f'demo data written to {out}: {len(PATIENTS)} patients, {len(ENCOUNTERS)} encounters, {len(icu)} ICU recordings, '
          f'{len(ecgs)} ECGs, {len(imaging)} images, {len(labs)} lab results, {len(notes)} notes')


if __name__ == '__main__':
    main()
