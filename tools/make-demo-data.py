#!/usr/bin/env python3
"""Make Medorama's demo: invented patients, their databases and files, for trying every feature.

    .venv/bin/python tools/make-demo-data.py [OUT]                 # the full demo (default OUT: sample_data/demo)
    .venv/bin/python tools/make-demo-data.py OUT --db-builder .venv/bin/db-builder   # and build its databases
    .venv/bin/python tools/make-demo-data.py OUT --small           # the first six patients only (two databases)

Needs NumPy and wfdb-x (the version the Python viewer pins; docs/building.md).  The same seed
makes the same demo, byte for byte (--seed N; default 20240101).

The demo is made from two things: models (patients, stays, results, notes, vital signs, sleep
and EEG signals, echo, angiography and synthetic images) and the demo library in
tools/demo/library: short pieces of real ECGs and CT series, transformed so that they represent
no one (heart beats re-timed, blended, their leads mixed; CT slices re-sliced, deformed,
re-mapped; chest radiographs projected from them), made by a maintainer-only tool and checked
against re-identification (tools/demo/README.md).  Without the library (--no-library, or a
clone without it) everything is made from models.

Every person is invented: names, MRNs (SYN…), patient IDs, dates of birth, every date (in an
invented period), every text.  No identifier of any real person or recording is in the demo.

Writes, under OUT (README.txt repeats this text; demo.json says what went in):

  databases/  the main hospital's tables (db-builder makes them into demo.sqlite): patients,
              encounters (an Excel sheet), labs, vitals, medications, diagnoses (ICD-10),
              microbiology, procedures, transfusions, life support, assessments, pathology,
              allergies, notes, and the indexes of the files below
  icu/        ICU recordings: WFDB collections (ECG I, II, III; arterial pressure; pleth;
              respiration; vital signs; alarms; R-wave and PVC markers; a break; a stretch
              without stated time), named UNIT_BED-<Unix time> as bedside monitors name them
  holter/     Holter ECGs (WFDB, 128 Hz)
  sleep/      an overnight sleep study (EDF+, scored)
  sleeplab/   more sleep studies, and eeglab/ routine EEGs (BDF+): folders of recordings that
              db-builder adds by a file-name pattern ({patient}_*.edf, {patient}_*.bdf)
  ecg/        12-lead ECGs: WFDB records, GE MUSE XML, DICOM ECGs, PDF and PNG printouts
  imaging/    DICOM: chest radiographs, CT series (a folder each, with the attribution of the
              public collection a library series came from)
  echo/       echo loops (DICOM cine; one as PNG frames) and structured reports
  cath/       coronary angiography runs (DICOM XA)
  north/      a second hospital's tables (db-builder: north/north.sqlite): some of the same
              patients by the same MRN authority, one MRN in lower case (matched ignoring case),
              one identity conflict, patient IDs written PT_123 (rules.toml makes them PT123),
              files under a folder that moves after the database is built (moved.toml, for
              db-builder rewrite)
  omop/       omop.sqlite: a third hospital's OMOP CDM database
  registry/   registry.sqlite: a research registry's own tables, connected through SQL views
              (kind views; its views in demo-databases.json and registry/views.toml)
  muse/       an ECG archive: MUSE XML files (xml/<year>/) and an ECG metadata database of
              them (ecg_metadata.sqlite), connected through its views (kind views; muse/views.toml)
  secure/     a db-builder database encrypted with SQLCipher (secure/secure.sqlite); its key
              in keys/demo_secure.key (a demo key, published with the demo: never do this
              with a real database)
  actions/    the example plug-in (recording-summary), for the Actions tab
  demo-databases.json   what `medorama init --demo` connects: each database's name, kind, file,
              settings and key file
  demo.json   what went in: "invented" (models only), "library" (the demo library), "examples"
              and "dicom" (local files given with --examples and --dicom)

With --db-builder, the databases are built (the installers' --demo does it):

  db-builder new OUT/demo.sqlite; db-builder add … --issuer DEMO --mrn-authority DEMO
  db-builder recordings OUT/demo.sqlite OUT/sleeplab --pattern '{patient}_*.edf' --category sleep …
  db-builder add OUT/north/north.sqlite … --rules OUT/north/rules.toml; the folder moved;
  db-builder rewrite OUT/north/north.sqlite --rules OUT/north/moved.toml
  db-builder --encrypt new/add OUT/secure/secure.sqlite (DB_BUILDER_KEY from keys/demo_secure.key)

and `medorama init --workspace DIR --demo` connects them all (docs/administration.md, "The
demo"; Medorama's data_roots, if set, must include OUT).  The first line of this text, which
README.txt repeats, tells such a folder from others.

    .venv/bin/python tools/make-demo-data.py OUT --examples sample_data/sample-ecg
    .venv/bin/python tools/make-demo-data.py OUT --dicom sample_data/sample-dicom

also give some of the first six patients local examples of real ECGs or DICOM studies (never
in git), de-identified on the way (a demo patient's identity, dates moved to the demo's).
demo.json then says so, and tools/screenshots.py refuses to take published pictures of it.
"""

import csv
import json
import os
import shutil
import sqlite3
import math
import random
import struct
import subprocess
from array import array
import sys
import zipfile
import zlib
from datetime import datetime, timedelta, timezone
from pathlib import Path
from xml.sax.saxutils import escape

try:
    import numpy as np
except ImportError:          # the tests of the Excel writer need none; making the demo does
    np = None

ROOT = Path(__file__).resolve().parent.parent
LIBRARY = ROOT / 'tools' / 'demo' / 'library'
SEED = 20240101

R = random.Random(SEED)
UTC = timezone.utc


def t(s):
    return datetime.strptime(s, '%Y-%m-%d %H:%M').replace(tzinfo=UTC)


def fmt(d):
    return d.strftime('%Y-%m-%d %H:%M')


# ------------------------------------------------------------------ the features it shows
#
# Every feature of Medorama the demo shows is listed here, with where the demo shows it and a
# check that the demo made holds it.  tools/README.md ("The demo's features") lists the same
# keys in a table, and tools/tests/test_make_demo_data.py checks that both agree and that every
# check passes.  A new feature of Medorama adds its part to the demo (a function below) and its
# row here and in that table.

FEATURES = {}


def demo_marker(db):
    """Mark a database the demo makes as a demo database (invented patients): Medorama connects
    it to a demo installation only, and a site's installation refuses it (its
    src/installation.rs; db-builder's `new --demo` marks its databases so)."""
    db.executescript("CREATE TABLE medorama_demo (note TEXT NOT NULL);"
                     "INSERT INTO medorama_demo VALUES ('A demo database: every patient in it is invented.');")


def _marked(out, d):
    """Whether a demo database (not encrypted) holds the demo's marker; one not built: not checked."""
    f = Path(out) / d['file']
    if not f.exists():
        return 'build' in d
    with sqlite3.connect(f) as db:
        return db.execute("SELECT COUNT(*) FROM sqlite_master WHERE name = 'medorama_demo'").fetchone()[0] == 1


def _muse_indexes(out):
    """The demo MUSE database's indexes on a measurement (its search indexes)."""
    f = Path(out) / 'muse/ecg_metadata.sqlite'
    if not f.exists():
        return 0
    with sqlite3.connect(f) as db:
        return sum(1 for (n,) in db.execute("SELECT name FROM sqlite_master WHERE type = 'index'")
                   if n.startswith('ecg_') and n not in ('ecg_patient', 'ecg_time'))


def _muse_rows(out, where):
    """The demo MUSE database's ECGs that hold this condition (SQL)."""
    f = Path(out) / 'muse/ecg_metadata.sqlite'
    if not f.exists():
        return 0
    with sqlite3.connect(f) as db:
        return db.execute(f'SELECT COUNT(*) FROM ecg WHERE {where}').fetchone()[0]


def feature(key, title, where):
    """Registers the check of a feature: a function of the demo folder, true when the demo holds it."""
    def wrap(check):
        FEATURES[key] = {'title': title, 'where': where, 'check': check}
        return check
    return wrap


def _rows(out, rel):
    p = Path(out) / rel
    if not p.is_file():
        return []
    with open(p, newline='', encoding='utf-8') as f:
        return list(csv.DictReader(f))


def _manifest(out):
    p = Path(out) / 'demo-databases.json'
    return json.loads(p.read_text(encoding='utf-8')) if p.is_file() else {'databases': []}


def _db(out, name):
    return next((d for d in _manifest(out)['databases'] if d['name'] == name), None)


feature('patients', 'About 40 invented patients (pseudo names, SYN MRNs, shifted dates of birth)', 'databases/patients.csv')(
    lambda out: len(_rows(out, 'databases/patients.csv')) >= 36 and all(r['MRN'].startswith('SYN') for r in _rows(out, 'databases/patients.csv')))
feature('age-90', 'A patient over 89 (shown as 90+ in Privacy Mode)', 'databases/patients.csv')(
    lambda out: any(r['DOB'] < '1934' for r in _rows(out, 'databases/patients.csv')))
feature('visits', 'Several visits per patient: admissions, clinic visits, ICU and bed stays', 'databases/encounters.xlsx')(
    lambda out: (Path(out) / 'databases/encounters.xlsx').is_file())
feature('labs', 'Laboratory results with reference ranges and flags (LOINC)', 'databases/labs.csv')(
    lambda out: any(r['Abnormal Flag'] == 'H' for r in _rows(out, 'databases/labs.csv')) and any(r['Abnormal Flag'] == 'L' for r in _rows(out, 'databases/labs.csv')))
feature('vitals', 'Charted vital signs', 'databases/vitals.csv')(lambda out: len(_rows(out, 'databases/vitals.csv')) > 500)
feature('micro', 'Microbiology cultures', 'databases/microbiology.csv')(lambda out: len(_rows(out, 'databases/microbiology.csv')) >= 5)
feature('meds', 'Medications with start and stop times', 'databases/medications.csv')(lambda out: len(_rows(out, 'databases/medications.csv')) >= 20)
feature('blood', 'Transfusions', 'databases/transfusions.csv')(lambda out: len(_rows(out, 'databases/transfusions.csv')) >= 2)
feature('support', 'Life support: ventilation, dialysis', 'databases/life_support.csv')(lambda out: len(_rows(out, 'databases/life_support.csv')) >= 2)
feature('procedures', 'Procedures', 'databases/procedures.csv')(lambda out: len(_rows(out, 'databases/procedures.csv')) >= 3)
feature('diagnoses', 'Diagnoses with ICD-10 codes', 'databases/diagnoses.csv')(lambda out: len(_rows(out, 'databases/diagnoses.csv')) >= 20)
feature('observations', 'Assessments and scores (GCS, SOFA, pain, NYHA)', 'databases/assessments.csv')(lambda out: len(_rows(out, 'databases/assessments.csv')) >= 10)
feature('pathology', 'Pathology reports', 'databases/pathology.csv')(lambda out: len(_rows(out, 'databases/pathology.csv')) >= 1)
feature('allergies', 'Allergies', 'databases/allergies.csv')(lambda out: len(_rows(out, 'databases/allergies.csv')) >= 3)
feature('notes-privacy', "Notes that name the patient and give the date of birth (replaced in Privacy Mode)", 'databases/notes.csv')(
    lambda out: any('DOB' in r['note_text'] for r in _rows(out, 'databases/notes.csv')))
feature('reports', 'Imaging, echo and cath reports with measurement columns', 'databases/imaging_index.csv, echo_index.csv, cath_index.csv')(
    lambda out: all(_rows(out, f'databases/{n}_index.csv') for n in ('imaging', 'echo', 'cath')))
feature('icu-collection', 'ICU recordings: WFDB collections with vital signs, alarms, markers', 'icu/')(
    lambda out: len(list(Path(out).glob('icu/*/index.csv'))) >= 10 and all((p.parent / 'events.csv').is_file() for p in Path(out).glob('icu/*/index.csv')))
feature('icu-gaps', 'A break in a recording, and a stretch without stated time', 'icu/*/gaps.csv')(
    lambda out: any(r.get('start_unix') == '' for p in Path(out).glob('icu/*/index.csv') for r in _rows(p.parent, 'index.csv'))
    and any(_rows(p.parent, 'gaps.csv') for p in Path(out).glob('icu/*/index.csv')))
feature('holter', 'Holter ECGs (heart rate derived by the index action)', 'holter/')(lambda out: len(list(Path(out).glob('holter/*/*.hea'))) >= 2)
feature('sleep-edf', 'Sleep studies as EDF+ with scoring annotations', 'sleep/, sleeplab/')(
    lambda out: len(list(Path(out).glob('sleep/*.edf'))) + len(list(Path(out).glob('sleeplab/*.edf'))) >= 2)
feature('eeg-bdf', 'Routine EEGs as BDF+ (24-bit)', 'eeglab/')(lambda out: any(p.read_bytes()[:8] == b'\xffBIOSEMI' for p in Path(out).glob('eeglab/*.bdf')))
feature('recordings-pattern', 'Folders of recordings added by a file-name pattern (db-builder recordings)', 'sleeplab/, eeglab/')(
    lambda out: any(s[0] == 'recordings' for d in _manifest(out)['databases'] for s in d.get('build', [])))
feature('ecg-formats', '12-lead ECGs as WFDB, MUSE XML, DICOM, PDF and PNG', 'ecg/')(
    lambda out: all(list(Path(out).glob(f'ecg/*.{x}')) for x in ('hea', 'xml', 'dcm', 'pdf', 'png')))
feature('echo', 'Echo loops (DICOM cine, PNG frames) and structured reports', 'echo/')(lambda out: len(list(Path(out).glob('echo/*_a4c.dcm'))) >= 3)
feature('ct', 'CT series', 'imaging/ct_*')(lambda out: len([p for p in Path(out).glob('imaging/ct_*') if p.is_dir()]) >= 4)
feature('xray', 'Chest radiographs', 'imaging/cxr_*')(lambda out: len(list(Path(out).glob('imaging/cxr_*.dcm'))) >= 3)
feature('cath', 'Coronary angiography runs', 'cath/')(lambda out: len(list(Path(out).glob('cath/*.dcm'))) >= 2)
feature('library', 'Real signals and pixels, transformed (the demo library), when present', 'demo.json')(
    lambda out: json.loads((Path(out) / 'demo.json').read_text())['library'] == (LIBRARY / 'manifest.json').is_file())
feature('second-hospital', 'A second hospital sharing patients by MRN authority', 'north/')(lambda out: _db(out, 'demo_north') is not None)
feature('mrn-case', 'An MRN differing only in letter case, linked ignoring case', 'north/patients.csv')(
    lambda out: any(r['MRN'] != r['MRN'].upper() for r in _rows(out, 'north/patients.csv')))
feature('identity-conflict', 'An identity conflict (an MRN of one patient, the ID of another)', 'north/patients.csv')(
    lambda out: any(r.get('Issuer') == 'DEMO' for r in _rows(out, 'north/patients.csv')))
feature('rules', 'Rewrite rules: identifiers normalised (PT_123 to PT123), paths rewritten', 'north/rules.toml')(
    lambda out: (Path(out) / 'north/rules.toml').is_file() and any(r['Patient ID'].startswith('PT_') for r in _rows(out, 'north/patients.csv')))
feature('rewrite', 'A moved folder, repointed with db-builder rewrite', 'north/moved.toml')(lambda out: (Path(out) / 'north/moved.toml').is_file())
feature('omop', 'An OMOP CDM database', 'omop/omop.sqlite')(lambda out: (Path(out) / 'omop/omop.sqlite').is_file() and _db(out, 'demo_omop') is not None)
feature('views', 'A SQL database connected through canonical views', 'registry/')(
    lambda out: (_db(out, 'demo_registry') or {}).get('kind') == 'views')
feature('muse-db', "A MUSE ECG archive and its ECG metadata database, connected through views: median beats with onsets, "
        'a 250 Hz ECG and one without its sampling rate', 'muse/')(
    lambda out: (_db(out, 'demo_muse') or {}).get('kind') == 'views' and (Path(out) / 'muse/ecg_metadata.sqlite').is_file()
    and any(b'<SampleBase>250</SampleBase>' in p.read_bytes() for p in Path(out).glob('muse/xml/*/*.xml'))
    and any(b'<SampleBase></SampleBase>' in p.read_bytes() for p in Path(out).glob('muse/xml/*/*.xml')))
feature('catalog', 'A catalog view: what demo_muse holds, named without reading its entries', 'muse/views.toml')(
    lambda out: 'variables' in (_db(out, 'demo_muse') or {}).get('settings', {})
    and 'variables =' in (Path(out) / 'muse/views.toml').read_text(encoding='utf-8'))
feature('demo-marked', 'Every demo database marked as invented: only a demo installation connects it', 'demo-databases.json')(
    lambda out: all(_marked(out, d) for d in _manifest(out)['databases'] if not d.get('encrypt')))
feature('person-dates', "Each MUSE patient's first and latest ECG, from the persons view (the list's dates without visits)", 'muse/views.toml')(
    lambda out: 'AS earliest' in (Path(out) / 'muse/views.toml').read_text(encoding='utf-8'))
feature('has-view', "A has view: who has an ECG in demo_muse, read for `has ecg` without its measurements", 'muse/views.toml')(
    lambda out: 'has' in (_db(out, 'demo_muse') or {}).get('settings', {})
    and 'has =' in (Path(out) / 'muse/views.toml').read_text(encoding='utf-8'))
feature('search-indexes', "An index per searched MUSE measurement (value, patient, time): a threshold read from the index", 'muse/ecg_metadata.sqlite')(
    lambda out: _muse_indexes(out) >= len(MUSE_MEASURES))
feature('muse-only', "A patient known to the ECG archive alone: the patient page without its left column", 'muse/ecg_metadata.sqlite')(
    lambda out: _muse_rows(out, f"patient_id = '{MUSE_ONLY[1]}'") >= 1
    and not any(r['MRN'] == MUSE_ONLY[1] for r in _rows(out, 'databases/patients.csv')))
feature('muse-ids', 'A test ECG whose patient ID is a word, left out by the views', 'muse/ecg_metadata.sqlite')(
    lambda out: _muse_rows(out, "patient_id = 'test'") == 1 and "GLOB 'SYN[0-9]*'" in (Path(out) / 'muse/views.toml').read_text(encoding='utf-8'))
feature('ecg-compare', 'Patients with two ECGs or more: each ECG compared with the previous one', 'muse/ecg_metadata.sqlite')(
    lambda out: _muse_rows(out, "patient_id IN (SELECT patient_id FROM ecg GROUP BY patient_id HAVING COUNT(*) > 1)") >= 4)
feature('ecg-median', 'MUSE ECGs with their median beats and P, QRS and T onsets', 'ecg/*.xml')(
    lambda out: all(b'<WaveformType>Median</WaveformType>' in p.read_bytes() and b'<QOnset>' in p.read_bytes()
                    for p in Path(out).glob('ecg/*.xml')))
feature('encrypted', 'An encrypted db-builder database, its key in a file', 'secure/, keys/')(
    lambda out: (_db(out, 'demo_secure') or {}).get('key_file') and (Path(out) / 'keys/demo_secure.key').is_file())
feature('actions', 'The example action plug-in, for the Actions tab', 'actions/')(
    lambda out: (Path(out) / 'actions/recording-summary/action.toml').is_file())
feature('cohorts', 'Enough patients and values for cohort criteria (creatinine, lactate, troponin, ICU, sex, age)', 'databases/labs.csv')(
    lambda out: len({r['MRN'] for r in _rows(out, 'databases/labs.csv') if r['Test Name'] == 'Creatinine' and float(r['Result']) > 1.5}) >= 4)


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

# each core encounter's story (STORIES below)
CORE_STORIES = {'E9100001': 'diabetes', 'E9100002': 'nstemi_sepsis', 'E9100003': 'followup', 'E9100004': 'pneumonia',
                'E9100005': 'abdominal_pain', 'E9100006': 'appendicitis', 'E9100007': 'hypertension', 'E9100008': 'palpitations',
                'E9100010': 'sleep', 'E9100009': 'af'}

# invented names for the generated patients: common given names and family names from many
# places, put together at random (with an apostrophe, a hyphen and accents, for the pages' sake)
FIRST_F = ['Maria', 'Amara', 'Sofia', 'Leila', 'Grace', 'Ingrid', 'Mei', 'Priya', 'Olga', 'Fatima', 'Chloé', 'Nadia',
           'Rosa', 'Hana', 'Zoe', 'Elena', 'Aisha', 'Lucía']
FIRST_M = ['John', 'Kwame', 'Luca', 'Omar', 'Pavel', 'Kenji', 'Arjun', 'Diego', 'Lars', 'Samuel', 'Théo', 'Ivan',
           'Malik', 'Tomás', 'Felix', 'Rahul', 'Noah', 'Jorge']
LAST = ['Smith-Doe', 'Moreau', 'Novak', 'Haddad', 'Kowalski', 'Silva', 'Nakamura', 'Mensah', 'Ivanova', "O'Neill",
        'García', 'Nguyen', 'Rossi', 'Schmidt', 'Dubois', 'Petrov', 'Kim', 'Singh', 'Cohen', 'Larsen', 'Moyo', 'Fischer',
        'Ali', 'Santos', 'Berg', 'Popescu', 'Yilmaz', 'Costa', 'Hughes', 'Sato', 'Okoye', 'Lund', 'Wójcik', 'Reyes']

# the stories of the generated patients' stays, in turn (each one at least once)
ROTATION = ['sepsis', 'nstemi', 'af', 'pneumonia', 'hf', 'ckd', 'diabetes', 'surgery', 'checkup', 'palpitations',
            'seizure', 'sleep', 'copd', 'stroke', 'gi_bleed', 'hf', 'sepsis', 'diabetes', 'checkup', 'nstemi', 'pneumonia',
            'ckd', 'af', 'surgery', 'copd', 'checkup', 'hf', 'gi_bleed', 'sepsis', 'seizure', 'sleep', 'diabetes', 'stroke', 'checkup']

PERIOD = (t('2023-09-01 00:00'), t('2024-05-31 00:00'))      # the invented period of every stay


def generated(seed=SEED, n=34):
    """The generated patients and their encounters (the same for the same seed):
    ([patient tuples], [encounter tuples], {encounter: story})."""
    rng = random.Random(seed * 7 + 1)
    pats, encs, stories = [], [], {}
    used = set()
    for i in range(n):
        pid, mrn = f'P9{i + 7:05d}', f'SYN{i + 7:07d}'
        sex = 'F' if i % 2 == 0 else 'M'
        while True:
            first = rng.choice(FIRST_F if sex == 'F' else FIRST_M)
            last = rng.choice(LAST)
            if (first, last) not in used:
                used.add((first, last))
                break
        story = ROTATION[i % len(ROTATION)]
        age = {'checkup': rng.randint(25, 60), 'sleep': rng.randint(35, 65), 'seizure': rng.randint(19, 45)}.get(story, rng.randint(45, 85))
        if i == 8:
            age = 93          # over 89: shown as 90+ in Privacy Mode
        born = datetime(2024 - age, rng.randint(1, 12), rng.randint(1, 28))
        pats.append((pid, mrn, last, first, f'{born:%Y-%m-%d}', sex))
        start = PERIOD[0] + timedelta(days=rng.randint(0, 200), hours=rng.randint(7, 20), minutes=rng.choice([0, 10, 20, 30, 40, 50]))
        plan = STORY_PLANS.get(story, [story])
        for k, st in enumerate(plan):
            kind, days, unit, icu_days = STORY_SHAPE[st]
            eid, csn = f'E92{i + 7:03d}{k + 1:02d}', f'91{i + 7:04d}{k + 1:04d}'
            a = start
            if kind == 'out':
                b = a + timedelta(minutes=60 if st != 'sleep' else 570)
                if st == 'sleep':
                    a = a.replace(hour=21, minute=30)
                    b = a + timedelta(hours=9, minutes=30)
                icu = None
            else:
                b = a + timedelta(days=days, hours=rng.randint(0, 8))
                icu = None
                if icu_days:
                    s = a + timedelta(hours=rng.randint(2, 6))
                    unit_name = {'nstemi': 'CCU', 'stroke': 'NICU'}.get(st, rng.choice(['ICU7', 'ICU8', 'MICU']))
                    icu = (unit_name, str(rng.randint(1, 16)), fmt(s), fmt(s + timedelta(days=icu_days, hours=rng.randint(0, 10))))
            encs.append((eid, pid, csn, kind, fmt(a), fmt(b), unit, 'Home' if st != 'sepsis' or k else 'Rehabilitation', icu))
            stories[eid] = st
            start = b + timedelta(days=rng.randint(20, 70), hours=rng.randint(-3, 3))
    return pats, encs, stories


# kind, length (days), admitting unit, ICU days
STORY_SHAPE = {
    'sepsis': ('in', 9, 'Emergency', 3), 'nstemi': ('in', 5, 'Cardiology', 2), 'af': ('in', 3, 'Medicine', 0),
    'pneumonia': ('in', 6, 'Medicine', 0), 'hf': ('in', 6, 'Cardiology', 0), 'ckd': ('out', 0, 'Nephrology clinic', 0),
    'diabetes': ('out', 0, 'Endocrinology clinic', 0), 'surgery': ('in', 4, 'Surgery', 0), 'checkup': ('out', 0, 'Primary care', 0),
    'palpitations': ('out', 0, 'Cardiology clinic', 0), 'seizure': ('in', 2, 'Neurology', 0), 'sleep': ('out', 0, 'Sleep laboratory', 0),
    'copd': ('in', 5, 'Pulmonology', 0), 'stroke': ('in', 7, 'Neurology', 2), 'gi_bleed': ('in', 4, 'Gastroenterology', 1),
    'followup': ('out', 0, 'Cardiology clinic', 0), 'ckd_dialysis': ('in', 4, 'Nephrology', 0),
}
# a story told over several stays
STORY_PLANS = {'ckd': ['ckd', 'ckd', 'ckd_dialysis'], 'diabetes': ['diabetes', 'diabetes'], 'hf': ['hf', 'followup'],
               'nstemi': ['nstemi', 'followup'], 'sepsis': ['sepsis', 'followup'], 'sleep': ['checkup', 'sleep']}


def all_people(full=True, seed=SEED):
    """The main hospital's patients and encounters, and each encounter's story."""
    if not full:
        return list(PATIENTS), list(ENCOUNTERS), dict(CORE_STORIES)
    p, e, s = generated(seed)
    return PATIENTS + p, ENCOUNTERS + e, {**CORE_STORIES, **s}


LABS = [
    # name, loinc, unit, low, high, normal, sick (value when ill)
    ('Sodium', '2951-2', 'mmol/L', 135, 145, 139, 131),
    ('Potassium', '2823-3', 'mmol/L', 3.5, 5.1, 4.2, 5.6),
    ('Creatinine', '2160-0', 'mg/dL', 0.6, 1.2, 0.9, 2.3),
    ('BUN', '3094-0', 'mg/dL', 7, 20, 14, 38),
    ('Glucose', '2345-7', 'mg/dL', 70, 140, 110, 190),
    ('Lactate', '2524-7', 'mmol/L', 0.5, 2.0, 1.1, 4.1),
    ('WBC', '6690-2', '10^9/L', 4.0, 11.0, 7.5, 18.2),
    ('Hemoglobin', '718-7', 'g/dL', 12.0, 16.0, 13.1, 7.6),     # a woman's range: by_sex gives a man's
    ('Platelets', '777-3', '10^9/L', 150, 400, 240, 98),
    ('Troponin T', '67151-1', 'ng/L', 0, 14, 8, 380),          # high-sensitivity troponin T, in ng/L
    ('Procalcitonin', '33959-8', 'ng/mL', 0, 0.5, 0.1, 6.4),
    ('INR', '6301-6', '', 0.8, 1.2, 1.0, 1.6),
    ('NT-proBNP', '33762-6', 'pg/mL', 0, 125, 80, 5200),
    ('HbA1c', '4548-4', '%', 4.0, 5.6, 5.3, 8.9),
    ('CRP', '1988-5', 'mg/L', 0, 5, 2, 160),
]
# the laboratory tests each story makes abnormal (the others stay normal), and the tests drawn
SICK = {
    'nstemi_sepsis': {'Creatinine', 'BUN', 'Lactate', 'WBC', 'Platelets', 'Procalcitonin', 'Troponin T', 'Glucose', 'CRP', 'Hemoglobin'},
    'sepsis': {'Creatinine', 'BUN', 'Lactate', 'WBC', 'Platelets', 'Procalcitonin', 'CRP', 'Sodium'},
    'nstemi': {'Troponin T', 'Glucose'}, 'af': {'NT-proBNP'}, 'pneumonia': {'WBC', 'CRP', 'Procalcitonin', 'Sodium'},
    'hf': {'NT-proBNP', 'Sodium', 'Creatinine', 'BUN'}, 'ckd': {'Creatinine', 'BUN', 'Potassium', 'Hemoglobin'},
    'ckd_dialysis': {'Creatinine', 'BUN', 'Potassium', 'Hemoglobin'}, 'diabetes': {'Glucose', 'HbA1c'},
    'surgery': {'WBC', 'CRP'}, 'appendicitis': {'WBC', 'CRP'}, 'abdominal_pain': {'WBC'}, 'copd': {'WBC', 'CRP'},
    'stroke': {'Glucose'}, 'gi_bleed': {'Hemoglobin', 'BUN', 'Platelets', 'INR'}, 'seizure': {'Lactate'},
}
CORE_LABS = ['Sodium', 'Potassium', 'Creatinine', 'BUN', 'Glucose', 'Lactate', 'WBC', 'Hemoglobin', 'Platelets', 'Procalcitonin', 'INR']
EXTRA_LABS = {'nstemi_sepsis': ['Troponin T', 'CRP'], 'nstemi': ['Troponin T'], 'hf': ['NT-proBNP'], 'af': ['NT-proBNP'],
              'diabetes': ['HbA1c'], 'pneumonia': ['CRP'], 'sepsis': ['CRP'], 'copd': ['CRP'], 'surgery': ['CRP'], 'appendicitis': ['CRP']}


def illness(enc, when):
    """How ill the patient is at `when` (0..1): a hump over the ICU stay."""
    icu = enc[8]
    if not icu:
        return 0.35 if enc[3] == 'in' else 0.0
    a, b = t(icu[2]), t(icu[3])
    mid = a + (b - a) * 0.55
    width = (b - a).total_seconds() * 0.45
    return max(0.0, math.exp(-((when - mid).total_seconds() / width) ** 2))


def by_sex(name, lo, hi, norm, sex):
    """A test's range and usual value for the patient's sex: hemoglobin F 12-16, M 13.5-17.5 g/dL."""
    if name == 'Hemoglobin' and sex == 'M':
        return 13.5, 17.5, 14.8
    return lo, hi, norm


def flag(v, lo, hi):
    if v < lo:
        return 'L'
    if v > hi:
        return 'H'
    return ''


def age_at(born, when):
    b = datetime.strptime(born, '%Y-%m-%d')
    return when.year - b.year - ((when.month, when.day) < (b.month, b.day))


# ------------------------------------------------------------------ the demo library

class Library:
    """The demo library (tools/demo/library): beat templates, pulse models, CT series and
    radiographs, transformed from real recordings and pictures (tools/demo/README.md)."""

    def __init__(self, folder):
        self.folder = Path(folder)
        self.items = json.loads((self.folder / 'manifest.json').read_text(encoding='utf-8'))['items']

    def of(self, kind):
        return [i for i in self.items if i['kind'] == kind]

    def array(self, item):
        a = np.fromfile(self.folder / item['file'], '<i2' if item['dtype'] == 'int16' else '<u2')
        return a.reshape(item['shape']).astype(np.float64)

    def pick(self, kind, k):
        items = self.of(kind)
        return items[k % len(items)] if items else None


def library(path):
    p = Path(path)
    return Library(p) if (p / 'manifest.json').is_file() else None


def overlap_add(template, pre, fs, beats, n, stretch=True):
    """Beats of `template` (leads x samples, R at `pre`) placed at `beats` [(time s, RR s)],
    the part after the R wave stretched with the RR (QT follows the rate), summed into n
    samples.  The rendering tools/demo/README.md describes (and its checks measure)."""
    out = np.zeros((template.shape[0], n))
    L = template.shape[1]
    i = np.arange(L, dtype=float)
    for tb, rr in beats:
        k = math.sqrt(min(1.4, rr) / 0.85) if stretch else 1.0
        src = np.where(i < pre, i, pre + (i - pre) / k)
        beat = np.stack([np.interp(src, i, v) for v in template])
        s = int(round(tb * fs)) - pre
        a, b = max(0, s), min(n, s + L)
        if a < b:
            out[:, a:b] += beat[:, a - s:b - s]
    return out


def pvc_beat(template, pre):
    """A premature ventricular beat from a template: no P wave, a wide QRS, the T wave inverted."""
    L = template.shape[1]
    i = np.arange(L, dtype=float)
    src = pre + (i - pre) / 2.2
    w = np.stack([np.interp(src, i, v) for v in template])
    w[:, :max(1, pre - int(0.02 * L))] *= np.linspace(0, 1, max(1, pre - int(0.02 * L)))
    return -0.8 * w


def pulse_cycle(kind, rng, model=None):
    """A model pulse cycle (128 points, 0 to 1): arterial pressure (a fast upstroke, a decay with
    a dicrotic notch) or pleth (a slower upstroke, a smooth decay); its time to peak and decay
    drawn from the library's ranges (fitted to real recordings) when given."""
    n = 128
    tt = np.linspace(0, 1, n)
    up = rng.uniform(*((model or {}).get('upstroke') or ((0.1, 0.16) if kind == 'abp' else (0.22, 0.32))))
    k = rng.uniform(*((model or {}).get('decay') or (2.6, 3.6)))
    c = np.where(tt < up, np.sin(np.clip(tt / up, 0, 1) * np.pi / 2), np.exp(-(tt - up) * k))
    if kind == 'abp':
        c = c + rng.uniform(0.04, 0.1) * np.exp(-((tt - up - rng.uniform(0.22, 0.28)) / 0.035) ** 2)
    c = c - np.linspace(c[0], c[-1], n)
    return (c - c.min()) / np.ptp(c)


def pulse_wave(shape, beats, fs, n, lo, hi, delay, rng, breath_hz):
    """A pulse signal: one cycle of `shape` per beat [(time s, RR s)], `delay` s after the R
    wave, scaled between lo and hi (arrays per sample), each cycle 4 % higher or lower, the
    amplitude and baseline modulated by breathing (as tools/demo/README.md describes)."""
    out = np.zeros(n)
    xs = np.linspace(0, 1, len(shape))
    for tb, rr in beats:
        a, b = int((tb + delay) * fs), int((tb + delay + rr) * fs)
        a2, b2 = max(0, a), min(n, b)
        if a2 < b2 and b > a:
            out[a2:b2] = rng.normal(1, 0.04) * np.interp((np.arange(a2, b2) - a) / (b - a), xs, shape)
    tt = np.arange(n) / fs
    ph = rng.random(2) * 2 * np.pi
    out = out * (1 + 0.12 * np.sin(2 * np.pi * breath_hz * tt + ph[0])) + 0.08 * np.sin(2 * np.pi * breath_hz * tt + ph[1])
    return lo + (hi - lo) * out


def beat_times(hr_at, seconds, rng, af=False, start=None):
    """[(time s, RR s)] of beats over `seconds`, the rate hr_at(t) (bpm), 3 % variability (an
    irregular rhythm with af)."""
    out, tb = [], start if start is not None else rng.uniform(0.1, 0.6)
    while tb < seconds + 1.5:
        rr = 60 / hr_at(max(0.0, tb)) * (rng.uniform(0.62, 1.32) if af else rng.gauss(1, 0.03))
        rr = max(0.3, rr)
        out.append((tb, rr))
        tb += rr
    return out


def ecg12_lib(lib, k, hr, st=False, af=False, lvh=False, fs=500, seconds=10, seed=0):
    """Twelve leads (mV) of a resting ECG from library template k: beats at a new RR series,
    the template's P waves taken out for atrial fibrillation (fibrillatory waves added), ST
    depression and LV hypertrophy drawn in, baseline wander and noise."""
    item = lib.pick('ecg12_beat', k)
    tpl = lib.array(item) / 1000.0
    pre = item['r_index']
    rng = np.random.default_rng(seed)
    idx = {name: i for i, name in enumerate(item['leads'])}
    if af:
        m = pre - int(0.07 * fs)
        tpl[:, :m] *= np.concatenate([np.linspace(0, 0, m - 20), np.linspace(0, 1, 20)])
    tt = (np.arange(tpl.shape[1]) - pre) / fs
    if lvh:
        q = np.exp(-(tt / 0.03) ** 2)
        for lead, g in (('V5', 0.9), ('V6', 0.6), ('I', 0.4), ('aVL', 0.5), ('V1', 0.7), ('V2', 0.6)):
            tpl[idx[lead]] *= 1 + g * q
        for lead in ('V5', 'V6', 'I', 'aVL'):
            tpl[idx[lead]] -= 0.12 * np.exp(-((tt - 0.26) / 0.06) ** 2)
    if st:
        for lead, d in (('I', 0.07), ('II', 0.07), ('V4', 0.12), ('V5', 0.16), ('V6', 0.10)):
            tpl[idx[lead]] -= d * np.exp(-((tt - 0.13) / 0.05) ** 2)
    n = int(fs * seconds)
    beats = beat_times(lambda x: hr, seconds, random.Random(seed), af=af)
    out = overlap_add(tpl, pre, fs, beats, n)
    x = np.arange(n) / fs
    out += 0.03 * np.sin(2 * np.pi * rng.uniform(0.1, 0.3) * x + rng.random() * 6)[None] + rng.normal(0, 0.008, out.shape)
    if af:
        f = 0.05 * np.sin(2 * np.pi * 5.7 * x + 0.3 * np.sin(x))
        for lead in ('V1', 'II'):
            out[idx[lead]] += 1.4 * f
    I, II = out[idx['I']], out[idx['II']]
    out[idx['III']], out[idx['aVR']], out[idx['aVL']], out[idx['aVF']] = II - I, -(I + II) / 2, I - II / 2, II - I / 2
    return [out[idx[name]] for name in LEADS12]


def ecg12_any(lib, k, hr, st=False, af=False, lvh=False):
    """A 10 s 12-lead ECG: from the library when there is one, else the model (ecg12)."""
    if lib is not None and lib.of('ecg12_beat'):
        return ecg12_lib(lib, k, hr, st, af, lvh, seed=k * 1009 + hr)
    return ecg12(hr, st, af)


# ------------------------------------------------------------------ WFDB

def wfdbx():
    """wfdb-x, which writes the WFDB records (in Medorama's environment: .venv/bin/python)."""
    try:
        import wfdbx as w
    except ImportError:
        sys.exit('wfdb-x is needed for the WFDB records: run this with .venv/bin/python, after\n'
                 '  .venv/bin/pip install numpy "wfdb-x @ https://github.com/alphanumericslab/wfdb-x/archive/4a91916f5282fba44ca3056802fbc7def298af08.tar.gz#subdirectory=python"\n'
                 '(the Python viewer brings it too; docs/administration.md)')
    return w


def wfdb_record(folder, name, fs, signals, start, comments=()):
    """A WFDB record in format 16, written with wfdb-x; signals: [(description, unit, gain, digital
    samples)]; start None: a header that states no time; comments: the header's info lines
    (UTF-8 in the file)."""
    folder.mkdir(parents=True, exist_ok=True)
    w = wfdbx()
    extra = {'base_time': f'{start:%H:%M:%S %d/%m/%Y}'} if start is not None else {}
    w.write_record(name, np.array([np.asarray(s[3]) for s in signals], dtype=np.int32).T, fs, fmt=16,
                   gains=[s[2] for s in signals], adcres=16, units=[s[1] for s in signals],
                   names=[s[0] for s in signals],
                   # wfdb-x writes one byte per character: give it the UTF-8 bytes
                   comments=[' ' + c.encode('utf-8').decode('latin-1') for c in comments], dir=str(folder), **extra)


def wfdb_markers(folder, record, marks):
    """<record>.mrk, written with wfdb-x: marks [(sample, code, chan, text)] in time order."""
    w = wfdbx()
    w.write_annotations(record, 'mrk', [m[0] for m in marks], codes=[m[1] for m in marks],
                        chans=[m[2] for m in marks], aux=[m[3] or None for m in marks], dir=str(folder),
                        sort=False)


def write_csv(path, header, rows):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    with open(path, 'w', newline='', encoding='utf-8') as f:
        w = csv.writer(f)
        w.writerow(header)
        w.writerows(rows)


# ------------------------------------------------------------------ ICU recordings

def recording(folder, start, minutes, ill, lib, seed, untimed=False):
    """An ICU recording as a WFDB collection (docs/wfdb-collection.md): ECG I, II, III at 240 Hz,
    arterial pressure at 120 Hz, SpO2 pleth and respiration at 60 Hz; a 45 s break (signal loss)
    after its first third; with `untimed`, its last two minutes in records that state no time,
    after a break of known length; vital signs every 2 s; an alarm while the heart rate is high;
    R-wave and PVC markers.  The ECG's beats are a library template's (else the model's)."""
    rng = np.random.default_rng(seed)
    prng = random.Random(seed)
    seconds = minutes * 60
    t0 = start.timestamp()
    sick = np.array([ill(datetime.fromtimestamp(t0 + s, UTC)) for s in range(seconds + 2)])
    hr_s = 72 + 40 * sick + 4 * np.sin(np.arange(seconds + 2) / seconds * 9)
    hr_at = lambda x: float(hr_s[min(len(hr_s) - 1, int(x))])  # noqa: E731
    beats = beat_times(hr_at, seconds, prng)
    # premature ventricular beats, more when ill
    kinds = []
    for j, (tb, rr) in enumerate(beats):
        pvc = j > 2 and kinds[-1] == 'N' and prng.random() < 0.01 + 0.06 * sick[min(len(sick) - 1, int(tb))]
        kinds.append('V' if pvc else 'N')
    times = []
    for j, (tb, rr) in enumerate(beats):
        if kinds[j] == 'V':
            tb = times[-1][0] + 0.62 * times[-1][1]
        times.append((tb, rr))
    fs = 240
    n = seconds * fs
    item = lib.pick('monitor_beat', seed) if lib is not None else None
    if item is not None:
        tpl, pre = lib.array(item)[:2] / 1000.0, item['r_index']        # leads I and II
    else:
        x = (np.arange(192) - 72) / fs
        g = lambda c, w: np.exp(-((x - c) / w) ** 2)  # noqa: E731
        base = 0.09 * g(-0.16, 0.025) - 0.08 * g(-0.022, 0.008) + 0.9 * g(0, 0.01) - 0.22 * g(0.025, 0.01) + 0.22 * g(0.27, 0.05)
        tpl, pre = np.stack([0.6 * base, base]), 72
    normal = [b for b, k in zip(times, kinds) if k == 'N']
    ecg = overlap_add(tpl, pre, fs, normal, n)
    ecg += overlap_add(pvc_beat(tpl, pre), pre, fs, [b for b, k in zip(times, kinds) if k == 'V'], n, stretch=False)
    ecg += rng.normal(0, 0.012, ecg.shape) + 0.04 * np.sin(2 * np.pi * 0.2 * np.arange(n) / fs)[None]
    ecg = np.vstack([ecg, ecg[1] - ecg[0]])                              # III = II - I
    rr_br = 14 + 10 * sick
    breath_hz = float(np.mean(rr_br)) / 60
    models = {m['pulse']: m for m in (lib.of('pulse_model') if lib is not None else [])}
    sec = lambda f: np.repeat(sick[:seconds], f)  # noqa: E731
    art = pulse_wave(pulse_cycle('abp', rng, models.get('abp')), times, 120, seconds * 120,
                     76 - 22 * sec(120), 128 - 34 * sec(120), 0.12, rng, breath_hz)
    pleth = pulse_wave(pulse_cycle('pleth', rng, models.get('pleth')), times, 60, seconds * 60, 0, 1, 0.25, rng, breath_hz)
    ph = np.cumsum(np.repeat(rr_br[:seconds], 60) / 60 / 60) * 2 * np.pi
    resp = 1000 + 400 * np.sin(ph) + 60 * np.sin(2 * ph + 0.5) + rng.normal(0, 8, len(ph))
    name = folder.name.lower().replace('-', '_')
    # the segments: [0, a1) timed; a break of 45 s; [a1 + 45, b2) timed; with `untimed`, a
    # break of 20 s, then [b2 + 20, end) without stated time
    a1 = seconds // 3
    b2 = seconds - 120 if untimed else seconds
    segs = [(0, a1, True), (a1 + 45, b2, True)] + ([(b2 + 20, seconds, False)] if untimed else [])
    signals = [('ecg', 240, [(lead, 'mV', 1000, ecg[i]) for i, lead in enumerate(('I', 'II', 'III'))]),
               ('pressure', 120, [('ART', 'mmHg', 10, art)]),
               ('spo2', 60, [('Pleth', 'adu', 1000, pleth)]),
               ('resp', 60, [('Resp', 'adu', 1, resp)])]
    index, gaps = [], []
    for k, (sa, sb, timed) in enumerate(segs, 1):
        for modality, f, sigs in signals:
            rec = f'{name}_{modality}_{f}_{k:04d}'
            part = [(lab, unit, gain, np.round(np.asarray(v[sa * f:sb * f]) * gain).astype(np.int64)) for lab, unit, gain, v in sigs]
            when = datetime.fromtimestamp(t0 + sa, UTC) if timed else None
            wfdb_record(folder / modality, rec, f, part, when)
            index.append([f'{modality}/{rec}', modality, f, len(sigs), ';'.join(x[0] for x in sigs), ';'.join(x[1] for x in sigs),
                          (sb - sa) * f, sb - sa, f'{t0 + sa:.1f}' if timed else '', 'header time' if timed else 'none',
                          '' if timed else 'untimed-1'])
            if modality == 'ecg':
                marks = [(int(round((tb - sa) * f)), 1 if kd == 'N' else 5, 1, 'R_WAVE' if kd == 'N' else 'PVC')
                         for (tb, rr), kd in zip(times, kinds) if sa <= tb < sb - 0.01]
                if marks:
                    wfdb_markers(folder / modality, rec, marks)
    for modality, *_ in signals:
        (folder / modality / 'RECORDS').write_text(''.join(f'{r[0].split("/")[1]}\n' for r in index if r[1] == modality), encoding='ascii')
    recs = lambda k: ';'.join(r[0] for r in index if r[0].endswith(f'_{k:04d}'))  # noqa: E731
    gaps.append(['all', 'signal_loss', f'{t0 + a1:.1f}', f'{t0 + a1 + 45:.1f}', 45, 0.01, 'clock', 'True', 'leads off', recs(1), recs(2)])
    if untimed:
        gaps.append(['all', 'signal_loss', f'{t0 + b2:.1f}', '', 20, 1, 'monitor', 'False', 'clock lost after the break', recs(2), recs(3)])
    write_csv(folder / 'index.csv', ['record', 'modality', 'fs', 'n_sig', 'signals', 'units', 'sig_len', 'duration_s', 'start_unix',
                                     'time_source', 'stretch'], index)
    write_csv(folder / 'gaps.csv', ['scope', 'reason', 'end_before_unix', 'start_after_unix', 'gap_s', 'gap_uncertainty_s',
                                    'measured_by', 'exact', 'notes', 'records_before', 'records_after'], gaps)
    vitals, events, high = [], [], None
    timed_s = [s for sa, sb, tm in segs if tm for s in range(sa, sb, 2)]
    for s in timed_s:
        hr, rr = hr_s[s], rr_br[s]
        sys_, dia, spo2 = 128 - 34 * sick[s], 76 - 22 * sick[s], 97 - 5 * sick[s]
        when = f'{t0 + s:.1f}'
        for label, value, unit in (('HR', round(hr), 'bpm'), ('SpO2', round(spo2), '%'), ('ART-S', round(sys_), 'mmHg'),
                                   ('ART-D', round(dia), 'mmHg'), ('ART-M', round((sys_ + 2 * dia) / 3), 'mmHg'),
                                   ('RR', round(rr), '/min')):
            vitals.append(['monitor', when, label, value, unit, '0'])
        if hr > 105 and high is None:
            high = t0 + s
        elif hr <= 105 and high is not None:
            events.append([f'{high:.1f}', when, 2, 'HR HIGH > 105', 'alarm'])
            high = None
    if high is not None:
        events.append([f'{high:.1f}', f'{t0 + timed_s[-1]:.1f}', 2, 'HR HIGH > 105', 'alarm'])
    events.append([f'{t0 + a1:.1f}', f'{t0 + a1 + 45:.1f}', 1, 'ECG LEADS OFF', 'technical'])
    if any(k == 'V' for k in kinds):
        first = next(tb for (tb, rr), k in zip(times, kinds) if k == 'V')
        events.append([f'{t0 + first:.1f}', '', 1, 'PVC', 'arrhythmia'])
    events.sort(key=lambda e: float(e[0]))
    write_csv(folder / 'vitals.csv', ['source', 'time', 'label', 'value', 'unit', 'validity'], vitals)
    write_csv(folder / 'events.csv', ['start_unix', 'end_unix', 'level', 'text', 'kind'], events)


def holter(folder, start, lib, seed, hours=24, fs=128):
    """A WFDB record of two ECG leads (II and V5, format 16, 200 adu/mV): the heart rate follows
    the day (slower asleep), a few premature ventricular beats, baseline wander and noise; the
    beats a library monitor template's (else the model's)."""
    folder.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(seed)
    prng = random.Random(seed)
    c0 = start.hour * 3600 + start.minute * 60
    hourly = [prng.uniform(0.97, 1.03) for _ in range(hours * 60 + 2)]

    def hr_at(x):
        clock = (c0 + x) % 86400 / 3600
        asleep = 1 if clock >= 23 or clock < 6.5 else 0
        return (82 - 22 * asleep + 6 * math.sin(clock / 24 * 2 * math.pi)) * hourly[int(x // 60)]
    seconds = hours * 3600
    beats = beat_times(hr_at, seconds, prng)
    item = lib.pick('monitor_beat', seed + 3) if lib is not None else None
    if item is not None:
        tpl = lib.array(item)[[1, 2]] / 1000.0                     # II and V
        tpl = np.stack([np.interp(np.arange(0, tpl.shape[1], item['fs'] / fs), np.arange(tpl.shape[1]), v) for v in tpl])
        pre = int(round(item['r_index'] * fs / item['fs']))
    else:
        x = (np.arange(102) - 38) / fs
        g = lambda c, w: np.exp(-((x - c) / w) ** 2)  # noqa: E731
        base = 0.09 * g(-0.16, 0.025) - 0.08 * g(-0.022, 0.008) + 1.1 * g(0, 0.012) - 0.22 * g(0.025, 0.012) + 0.25 * g(0.27, 0.05)
        tpl, pre = np.stack([base, 0.7 * base]), 38
    n = seconds * fs
    kinds = ['V' if j > 2 and prng.random() < 0.004 else 'N' for j in range(len(beats))]
    data = overlap_add(tpl, pre, fs, [b for b, k in zip(beats, kinds) if k == 'N'], n)
    data += overlap_add(pvc_beat(tpl, pre), pre, fs, [(b[0] - 0.3 * b[1], b[1]) for b, k in zip(beats, kinds) if k == 'V'], n, stretch=False)
    tt = np.arange(n) / fs
    bw = 0.12 * np.sin(2 * np.pi * 0.25 * tt)
    data[0] += bw
    data[1] -= 0.5 * bw
    data += rng.normal(0, 0.02, data.shape)
    sig = np.round(data * 200).astype(np.int64)
    wfdb_record(folder, 'holter', fs, [('II', 'mV', 200, sig[0]), ('V5', 'mV', 200, sig[1])], start)
    (folder / 'RECORDS').write_text('holter\n', encoding='ascii')


# ------------------------------------------------------------------ EDF+ and BDF+

def edf_plus(path, start, signals, notes, duration=1, bdf=False):
    """An EDF+ file (continuous), or with `bdf` a BDF+ file (24-bit samples): signals [(label,
    unit, pmin, pmax, samples per record, values)], notes [(onset s, duration s or None, text)],
    each in the data record it falls in.  The patient field is EDF+'s anonymous one: the
    database, not the file, says whose recording this is."""
    field = lambda t, w: t.encode('ascii').ljust(w)  # noqa: E731
    nb = 3 if bdf else 2
    dmin, dmax = (-8388608, 8388607) if bdf else (-32768, 32767)
    n = min(len(v) // spr for *_, spr, v in signals)
    by_record = {}
    for o, d, text in notes:
        by_record.setdefault(int(o // duration), []).append((o, d, text))
    tal = lambda o, d, t: (f'{o:+g}' + (f'\x15{d:g}' if d else '') + '\x14' + t + '\x14\x00').encode()  # noqa: E731
    spr_notes = max(30, max((sum(len(tal(*x)) for x in v) + 16 for v in by_record.values()), default=0) // nb + 1)
    sigs = [(label, unit, pmin, pmax, dmin, dmax, spr, v) for label, unit, pmin, pmax, spr, v in signals]
    sigs.append(('BDF Annotations' if bdf else 'EDF Annotations', '', -1, 1, dmin, dmax, spr_notes, None))
    d = start.astimezone(timezone.utc)
    head = (b'\xffBIOSEMI' if bdf else field('0', 8)) + field('X X X X', 80) + field(f'Startdate {d.strftime("%d-%b-%Y").upper()} X X X', 80)
    head += field(d.strftime('%d.%m.%y'), 8) + field(d.strftime('%H.%M.%S'), 8) + field(str(256 * (len(sigs) + 1)), 8)
    head += field('BDF+C' if bdf else 'EDF+C', 44) + field(str(n), 8) + field(str(duration), 8) + field(str(len(sigs)), 4)
    for get, w in ((0, 16), (None, 80), (1, 8), (2, 8), (3, 8), (4, 8), (5, 8), (None, 80), (6, 8), (None, 32)):
        head += b''.join(field('' if get is None else str(x[get]), w) for x in sigs)
    digital = []
    for label, unit, pmin, pmax, lo, hi, spr, v in sigs:
        if v is None:
            digital.append(None)
            continue
        k = (hi - lo) / (pmax - pmin)
        q = np.clip(np.round((np.asarray(v[:n * spr], float) - pmin) * k + lo), lo, hi).astype(np.int32)
        digital.append(q.reshape(n, spr))
    out = bytearray(head)
    for r in range(n):
        for (label, unit, pmin, pmax, lo, hi, spr, v), q in zip(sigs, digital):
            if q is None:
                b = tal(r * duration, None, '') + b''.join(tal(*x) for x in by_record.get(r, []))
                out += b.ljust(nb * spr, b'\0')
            elif bdf:
                out += q[r].astype('<i4').view(np.uint8).reshape(spr, 4)[:, :3].tobytes()
            else:
                out += q[r].astype('<i2').tobytes()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(out))


def sleep_study(path, start, seed, hours=2.0, ahi=8):
    """An overnight sleep study (polysomnography), scored: stages every 30 s (wake, N1, N2, N3,
    REM in cycles), obstructive apneas (about `ahi` per hour) with the desaturations and
    arousals that follow them."""
    rng = random.Random(seed)
    nrng = np.random.default_rng(seed)
    epochs = int(hours * 3600 // 30)
    cycle = ['W'] * 4 + ['N1'] * 4 + ['N2'] * 30 + ['N3'] * 40 + ['N2'] * 20 + ['R'] * 30 + ['N2'] * 12
    stages = [(cycle * 6)[i] for i in range(epochs)]
    names = {'W': 'Sleep stage W', 'N1': 'Sleep stage N1', 'N2': 'Sleep stage N2', 'N3': 'Sleep stage N3', 'R': 'Sleep stage R'}
    notes = [(30.0 * i, 30, names[st]) for i, st in enumerate(stages)]
    step = max(60, int(3600 / max(1, ahi)))
    apneas = [(x, 18 + rng.random() * 14) for x in range(900, int(hours * 3600) - 120, step) if stages[x // 30] in ('N1', 'N2', 'R')]
    for x, d in apneas:
        notes += [(float(x), round(d, 1), 'Obstructive apnea'), (round(x + d + 8, 1), 20, 'Oxygen desaturation'), (round(x + d, 1), 6, 'Arousal')]
    notes.sort()
    secs = int(hours * 3600)
    st_idx = np.array([['W', 'N1', 'N2', 'N3', 'R'].index(s) for s in stages])
    apnea_mask = lambda tt: np.any([(tt >= a) & (tt < a + d) for a, d in apneas], axis=0) if apneas else np.zeros(len(tt), bool)  # noqa: E731

    def eeg(fs):
        tt = np.arange(secs * fs) / fs
        st = st_idx[np.minimum((tt // 30).astype(int), epochs - 1)]
        f = np.array([10, 6, 13, 1.2, 5])[st]
        a = np.array([18, 22, 14, 70, 18])[st]
        ph = np.cumsum(2 * np.pi * f / fs)
        x = a * np.sin(ph) + 8 * np.sin(2 * np.pi * 2.7 * tt + 1) + nrng.normal(0, 5, len(tt))
        spindle = (st == 2) & ((tt % 9) < 1)
        x += np.where(spindle, 25 * np.sin(2 * np.pi * 13 * tt) * np.sin(np.pi * (tt % 9)), 0)
        return x

    def eog(fs):
        tt = np.arange(secs * fs) / fs
        st = st_idx[np.minimum((tt // 30).astype(int), epochs - 1)]
        return np.where((st == 4) | (st == 0), 60 * np.sin(2 * np.pi * 0.4 * tt), 5 * np.sin(2 * np.pi * 0.1 * tt)) + nrng.normal(0, 4, len(tt))

    def ecg(fs):
        tt = np.arange(secs * fs) / fs
        ph = (tt * 62 / 60) % 1.0
        g = lambda c, w, a: a * np.exp(-((ph - c) / w) ** 2)  # noqa: E731
        return (g(0.18, 0.025, 90) - g(0.29, 0.008, 80) + g(0.31, 0.010, 900) - g(0.335, 0.010, 220) + g(0.58, 0.05, 220)) / 1000

    def breath(fs, scale):
        tt = np.arange(secs * fs) / fs
        ap = apnea_mask(tt)
        return np.where(ap & (scale > 1), 0.0, scale * np.sin(2 * np.pi * 0.25 * tt) * np.where(ap, 0.35, 1))

    tt = np.arange(secs)
    drop = np.zeros(secs)
    for a, d in apneas:
        drop = np.maximum(drop, (4 + 3 * (d > 25)) * np.exp(-((tt - (a + d + 18)) / 12) ** 2))
    spo2 = np.round(96 - drop + nrng.normal(0, 0.3, secs), 1)
    edf_plus(path, start, [
        ('EEG C4-M1', 'uV', -500, 500, 100, eeg(100)),
        ('EOG E1-M2', 'uV', -500, 500, 50, eog(50)),
        ('ECG', 'mV', -5, 5, 200, ecg(200)),
        ('Flow', '', -400, 400, 25, breath(25, 300)),
        ('Thorax', '', -400, 400, 25, breath(25, 1)),
        ('SpO2', '%', 0, 100, 1, spo2),
    ], notes)


def routine_eeg(path, start, seed, minutes=20, fs=256, seizure=True):
    """A routine EEG as a BDF+ file (24-bit): eight channels of the 10-20 system, alpha at rest
    with the eyes closed, photic stimulation, hyperventilation, and (with `seizure`) a short
    run of 3 Hz spike-and-wave, as annotations."""
    rng = np.random.default_rng(seed)
    secs = minutes * 60
    tt = np.arange(secs * fs) / fs
    chans = ['Fp1', 'Fp2', 'C3', 'C4', 'O1', 'O2', 'T3', 'T4']
    eyes_closed = ((tt // 30) % 2) == 1
    sw0 = secs * 0.6
    out = []
    for i, ch in enumerate(chans):
        occ = 1.0 if ch[0] == 'O' else 0.35
        x = rng.normal(0, 6, len(tt)) + 10 * np.sin(2 * np.pi * 4.5 * tt + i)
        x += np.where(eyes_closed, 30 * occ * np.sin(2 * np.pi * 10 * tt + i * 0.3), 0)
        x += 4 * np.sin(2 * np.pi * 21 * tt + i)
        if ch[:2] == 'Fp':
            x += np.where((tt % 7) < 0.25, 120 * np.sin(np.pi * (tt % 7) / 0.25), 0)          # blinks
        if seizure:
            sw = (tt >= sw0) & (tt < sw0 + 8)
            ph = (tt - sw0) * 3 % 1
            x += np.where(sw, 140 * (np.exp(-((ph - 0.1) / 0.03) ** 2) - 0.45 * np.sin(np.pi * ph)), 0)
        out.append((ch, 'uV', -1000, 1000, fs, x))
    notes = [(0.0, 0, 'Recording start')] + [(30.0 * k, 30, 'Eyes closed' if k % 2 else 'Eyes open') for k in range(secs // 30)]
    notes += [(secs * 0.3, 60, 'Photic stimulation'), (secs * 0.45, 180, 'Hyperventilation')]
    if seizure:
        notes.append((sw0, 8, 'Generalized 3 Hz spike-and-wave'))
    notes.sort(key=lambda x: x[0])
    edf_plus(path, start, out, notes, bdf=True)


# ------------------------------------------------------------------ 12-lead ECGs

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


def median_beat(leads, fs=500, before=240, n=600):
    """The median beat of 12 leads (500 Hz): the beats of lead II aligned on their R waves, the
    median of each sample, `before` samples before the R wave and `n` in all (1.2 s), as the
    cart's 12SL forms it."""
    import statistics
    ii = [float(x) for x in leads[1]]
    peaks, last = [], -fs
    top = 0.6 * max(ii)
    for i in range(1, len(ii) - 1):
        if ii[i] > top and ii[i] >= ii[i - 1] and ii[i] > ii[i + 1] and i - last > 0.3 * fs:
            peaks.append(i)
            last = i
    wins = [r - before for r in peaks if r - before >= 0 and r - before + n <= len(ii)] or [0]
    return [[statistics.median(float(v[a + j]) for a in wins) for j in range(n)] for v in leads]


def ecg_muse(path, patient, when, leads, measures, computer, confirmed, fs=500, device=('MAC55', '241'), rate_given=True):
    """A GE MUSE resting ECG XML file (ISO-8859-1), as MUSE exports them: the patient (invented)
    named inside, the eight recorded leads in base64 (4.88 µV per bit), the median beat with its
    P, QRS and T onsets (at 500 Hz, as 12SL gives them), the measurements, the computer's
    statements (12SL) and the physician's confirmed ones.  `leads` are 10 s at 500 Hz; fs 250
    writes them at 250 Hz (as the MAC5K carts with 12SL 229-237 record some ECGs: QTRGGR 124,
    GlobalRR in 4 ms); `rate_given` False leaves SampleBase empty (a rate the viewer must not
    guess)."""
    import base64
    pid, mrn, last, first, born, sex = patient
    b = datetime.strptime(born, '%Y-%m-%d')
    age = when.year - b.year - ((when.month, when.day) < (b.month, b.day))
    med = median_beat(leads)
    step = 500 // fs

    def at_rate(v):
        v = [float(x) for x in v]
        return [sum(v[i:i + step]) / step for i in range(0, len(v) - step + 1, step)]
    by = {name: at_rate(v) for name, v in zip(LEADS12, leads)}
    med_by = {name: at_rate(v) for name, v in zip(LEADS12, med)}

    def lead(name, src):
        v = [max(-32767, min(32767, round(float(x) * 1000 / 4.88))) for x in src[name]]
        data = base64.b64encode(struct.pack(f'<{len(v)}h', *v)).decode()
        data = '\n'.join(data[i:i + 76] for i in range(0, len(data), 76))
        return (f'<LeadData><LeadByteCountTotal>{2 * len(v)}</LeadByteCountTotal><LeadSampleCountTotal>{len(v)}</LeadSampleCountTotal>'
                f'<LeadAmplitudeUnitsPerBit>4.88</LeadAmplitudeUnitsPerBit><LeadAmplitudeUnits>MICROVOLTS</LeadAmplitudeUnits>'
                f'<LeadID>{name}</LeadID><WaveFormData>\n{data}\n</WaveFormData></LeadData>\n')
    tags = {'Vent. rate': 'VentricularRate', 'Atrial rate': 'AtrialRate', 'PR': 'PRInterval', 'QRS': 'QRSDuration',
            'QT': 'QTInterval', 'QTc': 'QTCorrected', 'QTcF': 'QTcFrederica', 'P axis': 'PAxis', 'R axis': 'RAxis', 'T axis': 'TAxis'}
    meas = ''.join(f'<{tags[k]}>{v}</{tags[k]}>' for k, (v, _) in measures.items() if k in tags)
    # the onsets on the median beat, in 500 Hz samples (the R wave at 240, 12SL's trigger at 248)
    qrs = measures['QRS'][0]
    q_on = 240 - round(0.45 * qrs / 2)
    on = {'QOnset': q_on, 'QOffset': q_on + round(qrs / 2), 'TOffset': q_on + round(measures['QT'][0] / 2)}
    if 'PR' in measures:
        on.update(POnset=q_on - round(measures['PR'][0] / 2), POffset=q_on - round(measures['PR'][0] / 2) + 55)
    meas += ''.join(f'<{k}>{v}</{k}>' for k, v in on.items()) + '<ECGSampleBase>500</ECGSampleBase><ECGSampleExponent>0</ECGSampleExponent>'
    rr = round(60000 / measures['Vent. rate'][0])
    stmts = lambda ls: ''.join(f'<DiagnosisStatement><StmtFlag>ENDSLINE</StmtFlag><StmtText>{escape(x)}</StmtText></DiagnosisStatement>' for x in ls)
    edit = when + timedelta(hours=20)
    base = fs if rate_given else ''
    wave = lambda kind, src: (f'<Waveform><WaveformType>{kind}</WaveformType><SampleBase>{base}</SampleBase><SampleExponent>0</SampleExponent>\n'
                              f'<HighPassFilter>5</HighPassFilter><LowPassFilter>150</LowPassFilter><ACFilter>60</ACFilter>\n'
                              + ''.join(lead(k, src) for k in ('I', 'II', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6')) + '</Waveform>\n')
    xml = f"""<?xml version="1.0" encoding="ISO-8859-1"?>
<!DOCTYPE RestingECG SYSTEM "restecg.dtd">
<RestingECG>
<MuseInfo><MuseVersion>9.0.10</MuseVersion></MuseInfo>
<PatientDemographics><PatientID>{mrn}</PatientID><PatientAge>{age}</PatientAge><AgeUnits>YEARS</AgeUnits>
<DateofBirth>{b:%m-%d-%Y}</DateofBirth><Gender>{'FEMALE' if sex == 'F' else 'MALE'}</Gender>
<PatientLastName>{escape(last.upper())}</PatientLastName><PatientFirstName>{escape(first.upper())}</PatientFirstName></PatientDemographics>
<TestDemographics><DataType>RESTING</DataType><SiteName>Synthetic Hôpital</SiteName><AcquisitionDevice>{device[0]}</AcquisitionDevice>
<Status>CONFIRMED</Status><LocationName>CARDIOLOGY</LocationName><TestReason>Synthetic test, not a patient</TestReason>
<AcquisitionTime>{when:%H:%M:%S}</AcquisitionTime><AcquisitionDate>{when:%m-%d-%Y}</AcquisitionDate>
<AnalysisSoftwareVersion>{device[1]}</AnalysisSoftwareVersion><EditTime>{edit:%H:%M:%S}</EditTime><EditDate>{edit:%m-%d-%Y}</EditDate>
<OverreaderLastName>RIVERA</OverreaderLastName><OverreaderFirstName>A</OverreaderFirstName></TestDemographics>
<RestingECGMeasurements>{meas}</RestingECGMeasurements>
<OriginalDiagnosis>{stmts(computer)}</OriginalDiagnosis>
<Diagnosis>{stmts(confirmed)}</Diagnosis>
<QRSTimesTypes><GlobalRR>{rr if fs == 500 else round(rr / 4)}</GlobalRR><QTRGGR>{round(0.496 * fs)}</QTRGGR></QRSTimesTypes>
{wave('Median', med_by)}{wave('Rhythm', by)}</RestingECG>
"""
    path.parent.mkdir(parents=True, exist_ok=True)
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
    speckle = np.array([rng.random() for _ in range(w * h)])
    y, x = np.mgrid[0:h, 0:w]
    u, v = (x - w / 2) / (h * 0.9), y / (h * 0.95)
    fan = (v > 0.02) & (np.abs(np.arctan2(u, v)) < 0.62) & (np.hypot(u, v) < 1.05)
    flat = (y * w + x)
    out = bytearray()
    for k in range(frames):
        ph = k / frames
        # volume over a beat: fast ejection, slower filling
        c = (1 - math.sin(math.pi * ph / 0.4)) if ph < 0.4 else 1.0
        scale = 1 - (1 - math.sqrt(1 - ef)) * (1 - c)
        val = np.where(fan, 18 + 40 * speckle[(flat + 7 * k) % (w * h)], 0.0)
        for (cx, cy, rx, ry, s) in ((-0.17, 0.42, 0.13, 0.27, scale), (0.15, 0.48, 0.1, 0.2, 1), (-0.15, 0.82, 0.12, 0.1, 1), (0.14, 0.84, 0.1, 0.09, 1)):
            d = ((u - cx) / (rx * s)) ** 2 + ((v - cy) / (ry * (0.75 + 0.25 * s))) ** 2
            blood = fan & (d < 1)
            wall = fan & (d >= 1) & (d < 1.6)
            val = np.where(blood, 6 + 10 * speckle[flat % (w * h)], val)
            val = np.where(wall, np.maximum(val, 120 + 90 * speckle[(flat + k) % (w * h)]), val)
        out += np.minimum(255, val.astype(int)).astype(np.uint8).tobytes()
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
    """One sheet of text cells (inline strings): enough for db-builder's reader."""
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

# ------------------------------------------------------------------ an OMOP database (invented)

OMOP_TABLES = '''
CREATE TABLE person (person_id INTEGER, gender_concept_id INTEGER, year_of_birth INTEGER, month_of_birth INTEGER,
    day_of_birth INTEGER, birth_datetime TEXT, person_source_value TEXT);
CREATE TABLE care_site (care_site_id INTEGER, care_site_name TEXT);
CREATE TABLE visit_occurrence (visit_occurrence_id INTEGER, person_id INTEGER, visit_concept_id INTEGER, visit_start_date TEXT,
    visit_start_datetime TEXT, visit_end_date TEXT, visit_end_datetime TEXT, care_site_id INTEGER);
CREATE TABLE visit_detail (visit_detail_id INTEGER, person_id INTEGER, visit_detail_concept_id INTEGER, visit_occurrence_id INTEGER,
    visit_detail_start_date TEXT, visit_detail_start_datetime TEXT, visit_detail_end_date TEXT, visit_detail_end_datetime TEXT,
    care_site_id INTEGER);
CREATE TABLE measurement (measurement_id INTEGER, person_id INTEGER, measurement_concept_id INTEGER, measurement_date TEXT,
    measurement_datetime TEXT, value_as_number REAL, value_as_concept_id INTEGER, unit_concept_id INTEGER, range_low REAL,
    range_high REAL, visit_occurrence_id INTEGER, measurement_source_value TEXT, unit_source_value TEXT, value_source_value TEXT,
    operator_concept_id INTEGER);
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
    # (a site's own concept, above 2 000 000 000: the demo's vocabulary holds only these few)
    (2000000001, 'Troponin T.cardiac [Mass/volume] in Serum or Plasma by High sensitivity method', 'LOINC', '67151-1'),
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


def write_omop(path, extra=()):
    """An invented OMOP CDM database: another hospital's, with two of the demo patients (by
    their MRNs: SYN0000002, SYN0000005) and two patients of its own; `extra`: more persons
    [(MRN, sex concept, born, story)], each with a stay, results, a condition and a note."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    db = sqlite3.connect(path)
    demo_marker(db)
    db.executescript(OMOP_TABLES)
    db.executemany('INSERT INTO concept VALUES (?, ?, ?, ?)', OMOP_CONCEPTS)
    # person_id, sex concept, born, MRN (person_source_value)
    persons = [(1, 8507, '1961-03-05', 'SYN0000002'), (2, 8532, '1966-06-14', 'SYN0000005'),
               (3, 8507, '1949-09-21', 'SYN0000101'), (4, 8532, '1983-12-02', 'SYN0000102')]
    persons += [(5 + i, g, b, m) for i, (m, g, b, _) in enumerate(extra)]
    db.executemany('INSERT INTO person VALUES (?, ?, ?, ?, ?, NULL, ?)',
                   [(p, g, int(b[:4]), int(b[5:7]), int(b[8:]), m) for p, g, b, m in persons])
    db.executemany('INSERT INTO care_site VALUES (?, ?)', [(1, 'County General MICU'), (2, 'County General Cardiology'),
                                                          (3, 'County General Clinic')])
    # visit, person, kind, start, end, care site, intensive care
    visits = [(1, 1, 9201, '2023-09-02 14:00', '2023-09-08 11:00', 2, False),
              (2, 2, 9202, '2023-11-20 09:00', '2023-11-20 10:00', 3, False),
              (3, 3, 9201, '2024-01-04 22:00', '2024-01-12 15:00', 1, True),
              (4, 4, 9202, '2024-02-07 13:00', '2024-02-07 13:40', 3, False)]
    # measurement concept, unit concept, low, high, then (person, visit, time, value) …
    results = [
        (3047181, 8753, 0.5, 2.0, [(1, 1, '2023-09-02 15:10', 3.4), (1, 1, '2023-09-03 06:00', 1.9), (3, 3, '2024-01-04 23:00', 5.2),
                                   (3, 3, '2024-01-05 06:00', 3.8), (3, 3, '2024-01-06 06:00', 1.6)]),
        (3016723, 8840, 0.6, 1.2, [(1, 1, '2023-09-02 15:10', 1.4), (2, 2, '2023-11-20 09:15', 0.8), (3, 3, '2024-01-05 06:00', 2.1)]),
        (2000000001, 8842, 0, 14, [(1, 1, '2023-09-02 15:10', 412), (1, 1, '2023-09-03 06:00', 655), (3, 3, '2024-01-05 06:00', 22)]),
        (3000963, 8713, 12, 16, [(2, 2, '2023-11-20 09:15', 12.9), (4, 4, '2024-02-07 13:10', 10.6)]),
        (3027018, 8541, 60, 100, [(1, 1, '2023-09-02 14:30', 104), (3, 3, '2024-01-04 22:30', 121), (3, 3, '2024-01-05 08:00', 98)]),
        (3004249, 8876, 90, 140, [(1, 1, '2023-09-02 14:30', 152), (3, 3, '2024-01-04 22:30', 84)]),
    ]
    conditions = [(4329847, 45576876, 'I21.4', 1, 1, '2023-09-02 15:00'), (201826, 45591076, 'E11.9', 2, 2, '2023-11-20 09:30'),
                  (132797, 45533176, 'A41.9', 3, 3, '2024-01-04 23:30'), (201826, 45591076, 'E11.9', 3, 3, '2024-01-05 09:00')]
    notes = [
        (1, 1, '2023-09-08 10:00', 'Discharge summary',
         'Admitted with chest pain and a non-ST elevation myocardial infarction; troponin peaked at 655 ng/L. '
         'Coronary angiography: a tight LAD stenosis, stented. Discharged on dual antiplatelet therapy.', 2, 1),
        (2, 3, '2024-01-12 14:00', 'Discharge summary',
         'Sepsis of urinary origin with a lactate of 5.2 mmol/L on arrival, treated with piperacillin/tazobactam in the '
         'MICU. Lactate normalised within 48 hours.', 1, 3),
    ]
    rng = random.Random(len(extra) + 11)
    for i, (mrn, g, born, story) in enumerate(extra):
        person, visit = 5 + i, 5 + i
        a = t('2023-06-01 08:00') + timedelta(days=rng.randint(0, 80), hours=rng.randint(0, 10))
        inpatient = story in ('sepsis', 'nstemi', 'hf', 'pneumonia')
        b = a + (timedelta(days=rng.randint(3, 7)) if inpatient else timedelta(minutes=50))
        visits.append((visit, person, 9201 if inpatient else 9202, fmt(a), fmt(b), 1 if story == 'sepsis' else 2 if story in ('nstemi', 'hf') else 3, story == 'sepsis'))
        sick = 0.8 if inpatient else 0.3
        results[0][4].append((person, visit, fmt(a + timedelta(hours=1)), round(1.0 + (3.5 if story == 'sepsis' else 0.4) * sick, 1)))
        results[1][4].append((person, visit, fmt(a + timedelta(hours=1)), round(0.9 + (1.4 if story in ('sepsis', 'hf') else 0.2) * sick, 1)))
        results[4][4].append((person, visit, fmt(a + timedelta(minutes=30)), round(78 + 30 * sick)))
        if story == 'nstemi':
            results[2][4].append((person, visit, fmt(a + timedelta(hours=2)), rng.randint(250, 900)))
        code = {'sepsis': (132797, 45533176, 'A41.9'), 'nstemi': (4329847, 45576876, 'I21.4')}.get(story, (201826, 45591076, 'E11.9'))
        conditions.append((*code, person, visit, fmt(a + timedelta(hours=2))))
        notes.append((len(notes) + 1, person, fmt(b - timedelta(minutes=30)), 'Discharge summary' if inpatient else 'Clinic note',
                      {'sepsis': 'Sepsis treated in the MICU; recovered.', 'nstemi': 'Non-ST elevation myocardial infarction, treated medically.',
                       'hf': 'Decompensated heart failure; diuresed.', 'pneumonia': 'Community-acquired pneumonia, oral antibiotics.'}.get(
                          story, 'Diabetes follow-up; metformin continued.'), 1 + i % 2, visit))
    db.executemany('INSERT INTO visit_occurrence VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
                   [(v, p, k, a[:10], a + ':00', b[:10], b + ':00', cs) for v, p, k, a, b, cs, _ in visits])
    db.executemany('INSERT INTO visit_detail VALUES (?, ?, 32037, ?, ?, ?, ?, ?, ?)',
                   [(v, p, v, a[:10], a + ':00', b[:10], b + ':00', cs) for v, p, k, a, b, cs, icu in visits if icu])
    m = 0
    for concept, unit, low, high, rows in results:
        for person, visit, tm, value in rows:
            m += 1
            db.execute('INSERT INTO measurement VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, NULL, NULL, NULL, NULL)',
                       (m, person, concept, tm[:10], tm + ':00', value, unit, low, high, visit))
    for i, (c, sc, code, person, visit, tm) in enumerate(conditions, 1):
        db.execute('INSERT INTO condition_occurrence VALUES (?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?)',
                   (i, person, c, tm[:10], tm + ':00', visit, code, sc))
    for i, (c, person, visit, a, b, unit) in enumerate([
            (1367571, 1, 1, '2023-09-02 16:00', '2023-09-06 16:00', 'U'), (1503297, 2, 2, '2023-11-20 10:00', '2024-05-20 10:00', 'mg'),
            (1717327, 3, 3, '2024-01-05 00:30', '2024-01-11 00:30', 'g')], 1):
        db.execute('INSERT INTO drug_exposure VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)',
                   (i, person, c, a[:10], a + ':00', b[:10], b + ':00', visit, unit))
    db.execute("INSERT INTO procedure_occurrence VALUES (1, 1, 4006969, '2023-09-03', '2023-09-03 10:00:00', 1, NULL)")
    db.executemany('INSERT INTO provider VALUES (?, ?)', [(1, 'Dr. Avery Quinn'), (2, 'Dr. Morgan Ellis')])
    db.executemany("INSERT INTO note VALUES (?, ?, ?, ?, 44814637, ?, ?, ?, ?)",
                   [(n, p, tm[:10], tm + ':00', title, text, prov, v) for n, p, tm, title, text, prov, v in notes])
    db.execute("INSERT INTO cdm_source VALUES ('County General (synthetic)', '2024-03-01', '2024-03-02')")
    db.commit()
    db.close()
    return persons


# ------------------------------------------------------------------ what each story holds

# per story (STORIES[story]), offsets in hours from the admission (negative: before the discharge):
#   dx [(ICD-10, description, at)], meds [(drug, dose, unit, from, to)], micro [(test, at, result, flag, comment)],
#   procs [(procedure, code, vocabulary, at)], support [(support, from, to, value, unit)], blood [(product, units, at)],
#   imaging [(kind, study, report, at)], echo (LVEF, LVIDd, impression), cath (LAD %, RCA %, LVEDP, impression),
#   ecg {af, st, lvh}, assessment (the notes' words), holter, sleep, eeg
STORIES = {
    'nstemi_sepsis': dict(
        dx=[('I21.4', 'Non-ST elevation myocardial infarction', 1.3), ('A41.51', 'Sepsis due to Escherichia coli', 113), ('N17.9', 'Acute kidney injury', 100)],
        meds=[('Heparin', 18, 'U/kg/h', 1.3, 108), ('Piperacillin-tazobactam', 4.5, 'g', 103, -1), ('Norepinephrine', 0.12, 'ug/kg/min', 104, 130),
              ('Metoprolol', 50, 'mg', 144, -1), ('Insulin glargine', 18, 'U', 13, -1)],
        micro=[('Blood culture', 104, 'Escherichia coli, ESBL negative', 'positive', 'Notify team; sepsis bundle in progress'),
               ('Urine culture', 104.2, 'Escherichia coli > 100,000 CFU/mL', 'positive', '')],
        procs=[('Coronary angiography', '93458', 'CPT', 24.5), ('Percutaneous coronary intervention, LAD', '92928', 'CPT', 25.5),
               ('Central venous catheter insertion', '36556', 'CPT', 103.5)],
        support=[('Mechanical ventilation', 105, 150, 40, '% FiO2')], blood=[('Packed red blood cells', 1, 110)],
        imaging=[('cxr', 'Chest X-ray', 'No acute cardiopulmonary process. Heart size upper normal.', 0.8),
                 ('ct', 'CT chest with contrast', 'Bilateral lower-lobe consolidation with small effusions; no pulmonary embolism.', 147 + 35 / 60)],
        echo=(0.35, 6.1, 'LVEF 35 %, inferior hypokinesis, mild mitral regurgitation.'),
        cath=(90, 30, 22, 'Severe proximal LAD stenosis (90 %), moderate RCA disease; LVEDP 22 mmHg. PCI to the LAD.'),
        ecg=dict(st=True), assessment='septic shock secondary to E. coli bacteremia after a non-ST elevation myocardial infarction'),
    'nstemi': dict(
        dx=[('I21.4', 'Non-ST elevation myocardial infarction', 1)], meds=[('Heparin', 18, 'U/kg/h', 1, 48), ('Aspirin', 81, 'mg', 1, -1),
                                                                         ('Ticagrelor', 90, 'mg', 20, -1), ('Atorvastatin', 80, 'mg', 3, -1)],
        procs=[('Coronary angiography', '93458', 'CPT', 20), ('Percutaneous coronary intervention', '92928', 'CPT', 21)],
        echo=(0.45, 5.4, 'LVEF 45 %, anterior hypokinesis.'), cath=(80, 20, 18, 'Significant LAD stenosis (80 %); stented.'),
        ecg=dict(st=True), assessment='non-ST elevation myocardial infarction, chest pain resolved'),
    'af': dict(dx=[('I48.91', 'Atrial fibrillation', 0.5)], meds=[('Amiodarone', 1, 'mg/min', 5.5, 42), ('Apixaban', 5, 'mg', 60, -1)],
               micro=[('Blood culture', 6.5, 'No growth at 5 days', '', '')], ecg=dict(af=True),
               echo=(0.55, 5.0, 'LVEF 55 %, dilated left atrium.'), assessment='atrial fibrillation with rapid ventricular response, rate controlled'),
    'pneumonia': dict(
        dx=[('J18.9', 'Pneumonia', 1), ('J96.01', 'Acute respiratory failure with hypoxia', 1)],
        meds=[('Ceftriaxone', 2, 'g', 4, -2), ('Azithromycin', 500, 'mg', 4, 76), ('Propofol', 30, 'ug/kg/min', 2.3, 55)],
        micro=[('Sputum culture', 3, 'Streptococcus pneumoniae', 'positive', '')], support=[('Mechanical ventilation', 2, 56, 50, '% FiO2')],
        imaging=[('cxr-right', 'Chest X-ray', 'Right lower lobe consolidation consistent with pneumonia.', 1.2)],
        assessment='community-acquired pneumonia with hypoxic respiratory failure, improving'),
    'hf': dict(dx=[('I50.23', 'Acute on chronic systolic heart failure', 1)], meds=[('Furosemide', 40, 'mg', 1.5, -1), ('Sacubitril-valsartan', 49, 'mg', 50, -1)],
               imaging=[('cxr', 'Chest X-ray', 'Cardiomegaly with interstitial edema and small bilateral effusions.', 1)],
               echo=(0.28, 6.6, 'LVEF 28 %, dilated left ventricle, moderate functional mitral regurgitation.'),
               obs=[('NYHA class', 'III')], assessment='decompensated heart failure, diuresing well'),
    'ckd': dict(dx=[('N18.4', 'Chronic kidney disease, stage 4', 0.5)], meds=[('Calcitriol', 0.25, 'ug', 0.5, 2000)], assessment='chronic kidney disease, slowly progressing'),
    'ckd_dialysis': dict(dx=[('N18.6', 'End stage renal disease', 1)], procs=[('Arteriovenous fistula creation', '36821', 'CPT', 26)],
                         support=[('Hemodialysis', 6, 10, 300, 'mL/min'), ('Hemodialysis', 54, 58, 300, 'mL/min')], blood=[('Packed red blood cells', 1, 30)],
                         assessment='end-stage renal disease, hemodialysis started'),
    'diabetes': dict(dx=[('E11.9', 'Type 2 diabetes mellitus', 0.5)], meds=[('Metformin', 1000, 'mg', 0.5, 4000)], assessment='type 2 diabetes, glycemic control to improve'),
    'followup': dict(assessment='doing well since the last admission'),
    'checkup': dict(assessment='healthy; routine screening'),
    'hypertension': dict(dx=[('I10', 'Essential hypertension', 0.5)], meds=[('Amlodipine', 5, 'mg', 0.5, 4000)], ecg=dict(lvh=True),
                         echo=(0.6, 4.8, 'LVEF 60 %, concentric LV hypertrophy, normal valves.'), assessment='hypertension with LV hypertrophy'),
    'palpitations': dict(dx=[('R00.2', 'Palpitations', 0.5)], holter=True, ecg=dict(lvh=True), assessment='palpitations; a 24-hour Holter is fitted'),
    'surgery': dict(dx=[('K80.00', 'Calculus of gallbladder with acute cholecystitis', 1)], meds=[('Cefazolin', 2, 'g', 2, 26), ('Morphine', 2, 'mg', 4, 52)],
                    procs=[('Laparoscopic cholecystectomy', '47562', 'CPT', 20)],
                    imaging=[('ct-abdomen', 'CT abdomen and pelvis', 'Distended gallbladder with wall thickening and stones: acute cholecystitis.', 1.5)],
                    path=('Gallbladder', 'Acute and chronic cholecystitis with cholelithiasis. No dysplasia or malignancy.', 50),
                    assessment='acute cholecystitis, after laparoscopic cholecystectomy'),
    'appendicitis': dict(dx=[('K35.80', 'Acute appendicitis', 0.5)], meds=[('Cefazolin', 2, 'g', 1, 25), ('Morphine', 2, 'mg', 4, 52)],
                         procs=[('Laparoscopic appendectomy', '44970', 'CPT', 6)],
                         imaging=[('ct-abdomen', 'CT abdomen and pelvis', 'Dilated appendix with periappendiceal fat stranding: acute appendicitis.', 0.5)],
                         path=('Appendix', 'Acute suppurative appendicitis. No neoplasm.', 30), assessment='acute appendicitis, after appendectomy'),
    'abdominal_pain': dict(dx=[('R10.31', 'Right lower quadrant pain', 0.7)], assessment='right lower quadrant pain, discharged with advice'),
    'seizure': dict(dx=[('G40.309', 'Generalized epilepsy', 2)], meds=[('Levetiracetam', 500, 'mg', 2, -1)], eeg=True,
                    obs=[('Glasgow Coma Scale', 14)], assessment='a first generalized seizure; EEG shows generalized spike-and-wave'),
    'sleep': dict(dx=[('G47.33', 'Obstructive sleep apnea', 8)], sleep=True, assessment='overnight polysomnography'),
    'copd': dict(dx=[('J44.1', 'COPD with acute exacerbation', 1)], meds=[('Prednisone', 40, 'mg', 2, 98), ('Ipratropium-albuterol', 3, 'mL', 1, -1)],
                 support=[('Non-invasive ventilation', 2, 30, 35, '% FiO2')], imaging=[('cxr', 'Chest X-ray', 'Hyperinflated lungs; no consolidation.', 1)],
                 assessment='COPD exacerbation, weaned off non-invasive ventilation'),
    'stroke': dict(dx=[('I63.9', 'Cerebral infarction', 1)], meds=[('Alteplase', 0.9, 'mg/kg', 1.5, 2.5), ('Aspirin', 300, 'mg', 26, -1)],
                   procs=[('Intravenous thrombolysis', '37195', 'CPT', 1.5)], obs=[('NIH Stroke Scale', 9), ('Glasgow Coma Scale', 13)],
                   assessment='acute ischemic stroke, thrombolysed, improving'),
    'gi_bleed': dict(dx=[('K92.2', 'Gastrointestinal hemorrhage', 1)], meds=[('Pantoprazole', 8, 'mg/h', 2, 74)],
                     procs=[('Upper GI endoscopy with hemostasis', '43255', 'CPT', 8)], blood=[('Packed red blood cells', 2, 3), ('Fresh frozen plasma', 2, 4)],
                     path=('Gastric biopsy', 'Chronic gastritis; Helicobacter pylori organisms identified.', 30), assessment='upper GI bleeding from a gastric ulcer, treated endoscopically'),
}
STORIES['sepsis'] = dict(
    dx=[('A41.9', 'Sepsis, unspecified organism', 1), ('N17.9', 'Acute kidney injury', 20)],
    meds=[('Piperacillin-tazobactam', 4.5, 'g', 1.5, -1), ('Norepinephrine', 0.1, 'ug/kg/min', 4, 40), ('Hydrocortisone', 50, 'mg', 10, 70)],
    micro=[('Blood culture', 1, 'Klebsiella pneumoniae', 'positive', 'ESBL negative'), ('Urine culture', 1.2, 'Klebsiella pneumoniae > 100,000 CFU/mL', 'positive', '')],
    procs=[('Central venous catheter insertion', '36556', 'CPT', 3)], support=[('Mechanical ventilation', 5, 60, 45, '% FiO2'), ('CRRT', 30, 70, 2000, 'mL/h')],
    blood=[('Packed red blood cells', 2, 24)], imaging=[('ct-chest', 'CT chest with contrast', 'Bilateral dependent consolidation; no abscess.', 8),
                                                         ('cxr', 'Chest X-ray', 'Endotracheal tube and central line in good position.', 6)],
    obs=[('SOFA score', 9), ('Glasgow Coma Scale', 10)], assessment='septic shock of urinary origin, on vasopressors')

CPT_DX = {}
STAFF = {'in': ('Dr. A. Rivera', 'Dr. K. Osei'), 'out': ('Dr. M. Patel',)}


class Demo:
    """What the parts fill: the rows of each table, and the files written."""

    def __init__(self, out, lib, full, seed):
        self.out, self.lib, self.full, self.seed = out, lib, full, seed
        self.t = {k: [] for k in ('labs', 'vitals', 'meds', 'dx', 'micro', 'notes', 'ecgs', 'icu', 'imaging', 'echos', 'caths',
                                  'procs', 'blood', 'support', 'obs', 'path', 'holter', 'sleep')}
        self.lib_ct = {r: [i for i in (lib.of('ct_series') if lib else []) if i['region'] == r] for r in ('chest', 'abdomen')}
        self.lib_xray = list(lib.of('xray') if lib else [])
        self.n_ecg = 0
        self.icu_stays = 0
        self.formats = {}


def at(e, h):
    """The time `h` hours after the admission (negative: before the discharge), within the stay."""
    a, b = t(e[4]), t(e[5])
    when = a + timedelta(hours=h) if h >= 0 else b + timedelta(hours=h)
    return min(max(when, a), b)


def fill(D, p, e, story):
    """The rows of one encounter: results, vital signs, notes, ECGs, ICU recordings, and what
    its story adds."""
    eid, pid, _, kind, a, b = e[0], e[1], e[2], e[3], t(e[4]), t(e[5])
    mrn, last, first, born, sex = p[1], p[2], p[3], p[4], p[5]
    S = STORIES.get(story, {})
    rng = random.Random(f'{D.seed}-{eid}')
    sick_set = SICK.get(story, set())
    chronic = 0.6 if kind == 'out' and sick_set else 0
    ill = lambda when: max(chronic, illness(e, when)) if e[8] or kind == 'in' else chronic  # noqa: E731
    age = age_at(born, a)
    who = f'{first} {last}'
    if story == 'sleep':
        D.t['notes'].append((pid, eid, fmt(b - timedelta(minutes=30)), 'Sleep study report', 'Dr. S. Lee', 'Sleep medicine',
                             f'Patient: {who}, DOB {born}.\nOvernight polysomnography (EEG, EOG, ECG, airflow, thoracic effort, SpO2).\n'
                             'Obstructive apneas with oxygen desaturations to about 89 %; apnea-hypopnea index about 8 per hour.\n'
                             'Sinus rhythm throughout.\nImpression: mild obstructive sleep apnea. Discuss treatment at the next visit.'))
        for code, desc, h in S.get('dx', []):
            D.t['dx'].append((pid, eid, code, desc, fmt(at(e, h))))
        return
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
    panel = CORE_LABS + EXTRA_LABS.get(story, [])
    for when in sorted(times):
        s = ill(when)
        for name, loinc, unit, lo, hi, norm, sickv in LABS:
            if name not in panel:
                continue
            lo, hi, norm = by_sex(name, lo, hi, norm, sex)
            if kind == 'out' and name not in ('Sodium', 'Potassium', 'Creatinine', 'Glucose', 'Hemoglobin', 'WBC', *EXTRA_LABS.get(story, [])):
                continue
            if name in ('Lactate', 'Procalcitonin') and s < 0.2 and when.hour != 6:
                continue
            if name in ('Troponin T',) and when > a + timedelta(days=3):
                continue
            level = s if name in sick_set else 0.05 * s
            v = norm + (sickv - norm) * level + (norm * 0.03) * rng.uniform(-1, 1)
            v = round(v, 1 if abs(norm) < 20 else 0)
            D.t['labs'].append((mrn, eid, name, v, unit, lo, hi, flag(v, lo, hi), fmt(when), loinc, 'LOINC'))
    # vital signs: hourly in the ICU, every 4 h on the ward, once at a visit
    when = a
    while when < b:
        in_icu = bool(e[8]) and t(e[8][2]) <= when <= t(e[8][3])
        if in_icu or when.hour % 4 == 0 or kind == 'out':
            s = ill(when) if kind == 'in' else 0
            for name, unit, lo, hi, v in (('Heart rate', '/min', 50, 110, 76 + 40 * s), ('SpO2', '%', 92, 100, 97 - 5 * s),
                                          ('Respiratory rate', '/min', 10, 22, 15 + 10 * s), ('Systolic blood pressure', 'mmHg', 90, 140, 128 - 36 * s),
                                          ('Temperature', '°C', 36.0, 38.0, 36.8 + 2.1 * s)):
                v = round(v + rng.uniform(-2, 2) * (0.1 if unit == '°C' else 1), 1)
                D.t['vitals'].append((pid, eid, name, v, unit, lo, hi, fmt(when)))
            if kind == 'out':
                break
        when += timedelta(hours=1)
    # notes: they name the patient and give the date of birth, as notes do (Privacy Mode
    # replaces both)
    woman = 'woman' if sex == 'F' else 'man'
    assessment = S.get('assessment', 'clinically stable')
    if kind == 'out':
        D.t['notes'].append((pid, eid, fmt(a + timedelta(minutes=55)), 'Clinic note', STAFF['out'][0], e[6],
                             f'{who} (DOB {born}), a {age}-year-old {woman}, seen in clinic.\n'
                             'No chest pain, no dyspnea at rest. Medications reviewed.\n'
                             f'Assessment: {assessment}.\nPlan: continue current therapy; follow-up in 3 months.'))
    else:
        D.t['notes'].append((pid, eid, fmt(a + timedelta(hours=2)), 'Admission note', STAFF['in'][0], e[6],
                             f'{who} (DOB {born}) is a {age}-year-old {woman} admitted from the emergency department.\n'
                             f'Assessment: {assessment}.\nPlan: admit, monitor, labs in the morning.'))
        d = a.replace(hour=15, minute=40) + timedelta(days=1)
        while d < b - timedelta(hours=6):
            in_icu = bool(e[8]) and t(e[8][2]) <= d <= t(e[8][3])
            s = ill(d)
            text = ('Subjective: overnight events reviewed.\n'
                    f'Objective: HR {round(76 + 40 * s)}, SpO2 {round(97 - 5 * s)} %, temperature {36.8 + 2.1 * s:.1f} °C.\n'
                    f'Assessment: {assessment if s > 0.4 else "clinically stable"}.\n'
                    'Plan: continue current management; repeat labs in the morning.')
            D.t['notes'].append((pid, eid, fmt(d), 'ICU progress note' if in_icu else 'Progress note',
                                 STAFF['in'][1] if in_icu else STAFF['in'][0], 'ICU' if in_icu else e[6], text))
            d += timedelta(days=1)
        D.t['notes'].append((pid, eid, fmt(b - timedelta(hours=1)), 'Discharge summary', STAFF['in'][0], e[6],
                             f'Hospital course: {who} (DOB {born}) was admitted on {a:%b %d, %Y}: {assessment}. '
                             f'Discharged to {e[7].lower()} in stable condition.'))
    # what the story adds
    for code, desc, h in S.get('dx', []):
        D.t['dx'].append((pid, eid, code, desc, fmt(at(e, h))))
    for drug, dose, unit, h0, h1 in S.get('meds', []):
        D.t['meds'].append((pid, eid, drug, dose, unit, fmt(at(e, h0)), fmt(at(e, h1) if h1 < 1000 else at(e, 0) + timedelta(hours=h1))))
    for test, h, result, fl, comment in S.get('micro', []):
        D.t['micro'].append((pid, eid, test, fmt(at(e, h)), result, fl, comment))
    for name, code, vocab, h in S.get('procs', []):
        D.t['procs'].append((pid, eid, name, code, vocab, fmt(at(e, h))))
    for name, h0, h1, value, unit in S.get('support', []):
        D.t['support'].append((pid, eid, name, value, unit, fmt(at(e, h0)), fmt(at(e, h1))))
    for product, units, h in S.get('blood', []):
        D.t['blood'].append((pid, eid, product, units, 'units', fmt(at(e, h))))
    for name, value in S.get('obs', []):
        for k in range(3 if kind == 'in' else 1):
            when = at(e, 3 + 24 * k)
            v = value if isinstance(value, str) else max(0, round(value - k * (2 if value > 6 else 1)))
            D.t['obs'].append((pid, eid, name, v, fmt(when)))
    if kind == 'in':
        for k in range(min(3, max(1, (b - a).days))):
            D.t['obs'].append((pid, eid, 'Pain score (0-10)', max(0, 6 - 2 * k + rng.randint(-1, 1)), fmt(at(e, 4 + 24 * k))))
    if 'path' in S:
        spec, text, h = S['path']
        D.t['path'].append((pid, eid, fmt(at(e, h)), f'Surgical pathology: {spec}', 'Dr. P. Lindgren', text))
    ecgs(D, p, e, story, ill)
    imaging(D, p, e, story)
    if e[8]:
        unit, bed, i0, i1 = e[8]
        for j, frac in enumerate((0.3, 0.62)):
            st = (t(i0) + (t(i1) - t(i0)) * frac).replace(second=0, microsecond=0)
            fname = '_'.join((unit, bed)) + '-' + str(int(st.timestamp()))
            recording(D.out / 'icu' / fname, st, 10, ill, D.lib, seed=D.seed % 1000 + len(D.t['icu']), untimed=(j == 1 and D.icu_stays < 3))
            D.t['icu'].append((pid, eid, f'../icu/{fname}'))
        D.icu_stays += 1
    if S.get('holter'):
        hours = 24 if pid == 'P900004' else 12
        rel = f'{pid}_{a:%Y%m%d}'
        holter(D.out / 'holter' / rel, a + timedelta(minutes=90), D.lib, seed=len(D.t['holter']) + 5, hours=hours)
        D.t['holter'].append((pid, eid, 'holter', f'../holter/{rel}/holter.hea'))
    if S.get('eeg'):
        routine_eeg(D.out / 'eeglab' / f'{pid}_eeg_{a:%Y%m%d}.bdf', a + timedelta(hours=20), seed=len(pid) + int(pid[-3:]))


def ecgs(D, p, e, story, ill):
    """The encounter's 12-lead ECGs: what a cart and MUSE would hold (12SL measurements and
    statements, the physician's confirmation, sometimes a corrected reading), in the formats in
    turn: WFDB mostly, then MUSE XML, DICOM, PDF and PNG."""
    eid, pid, kind, a = e[0], e[1], e[3], t(e[4])
    flags = STORIES.get(story, {}).get('ecg', {})
    for k, when in enumerate([a + timedelta(minutes=20)] + ([a + timedelta(days=2, hours=3)] if kind == 'in' else [])):
        s = ill(when) if kind == 'in' else 0
        hr = round(72 + 40 * s) + (38 if flags.get('af') and k == 0 else 0)
        af = bool(flags.get('af')) and k == 0
        st = bool(flags.get('st')) and k == 0
        lvh = bool(flags.get('lvh'))
        leads = ecg12_any(D.lib, D.n_ecg, hr, st, af, lvh)
        D.n_ecg += 1
        meas = ecg_measures(hr, af, lvh)
        name = f'ecg_{pid}_{when:%Y%m%d_%H%M}'
        prev = D.formats.setdefault('last', {}).get(pid)
        computer = twelve_sl(meas, af, st, lvh, prev)
        D.formats['last'][pid] = (when, st, af)
        interp = '; '.join(computer)
        if st:
            confirmed = [computer[0], 'Inferolateral ST depression, consistent with subendocardial ischemia', 'Abnormal ECG']
        elif af:
            confirmed = None
        else:
            confirmed = computer
        # the format: the first patients' as before; then MUSE every 6th, DICOM, PDF and PNG once more each
        n = D.n_ecg
        fmt_ = 'wfdb'
        if k == 1 and pid in ('P900001', 'P900002'):
            fmt_ = 'muse'
        elif k == 1 and pid == 'P900003':
            fmt_ = 'dicom'
        elif eid == 'E9100007':
            fmt_ = 'pdf'
        elif pid == 'P900003' and kind == 'out':
            fmt_ = 'png'
        elif pid >= 'P900007':
            fmt_ = {20: 'dicom', 25: 'pdf', 30: 'png'}.get(n, 'muse' if n % 6 == 0 else 'wfdb')
        if fmt_ == 'muse':
            if pid == 'P900001':
                confirmed = [computer[0], 'Nonspecific T wave abnormality', 'Borderline ECG']
            file = ecg_muse(D.out / 'ecg' / f'{name}.xml', p, when, leads, meas, computer, confirmed or computer).name
        elif fmt_ == 'dicom':
            ecg_dicom(D.out / 'ecg' / f'{name}.dcm', p[1], when, leads, meas, computer)
            file = f'{name}.dcm'
        elif fmt_ == 'pdf':
            ecg_pdf(D.out / 'ecg' / f'{name}.pdf', leads, f'Synthetic 12-lead ECG (not a patient)   {computer[0]}   {hr} bpm')
            file = f'{name}.pdf'
        elif fmt_ == 'png':
            ecg_png(D.out / 'ecg' / f'{name}.png', leads)
            file = f'{name}.png'
        else:
            age = age_at(p[4], when)
            comments = [f'Age: {age}', f'Sex: {p[5]}', 'Device: MAC 5500 HD', 'Location: ' + e[6], 'Filters: 0.05-150 Hz, 60 Hz notch',
                        'Program: Marquette 12SL v243']
            comments += [f'{k2}: {v} {u}' for k2, (v, u) in meas.items()]
            if st:
                comments += [f'ST {lead}: {v:+.2f} mV' for lead, v in ST_DEPRESSION.items()]
            if lvh:
                comments += ['R amp V5: 3.4 mV', 'S amp V1: 1.9 mV']
            comments.append(f'Dx: {"; ".join(computer)}')
            if confirmed:
                comments += [f'Dx confirmed: {"; ".join(confirmed)}', 'Confirmed by: A. Rivera', f'Confirmed at: {when + timedelta(hours=6):%Y-%m-%d %H:%M}']
            file = ecg_wfdb(D.out / 'ecg', name, when, leads, comments=comments).name
        D.t['ecgs'].append((pid, eid, fmt(when), '12-lead ECG', hr, 'bpm', *[meas[c][0] if c in meas else '' for c in ECG_COLUMNS], interp, f'../ecg/{file}'))


ATTRIBUTION = {
    # the public collections the library's CT series and radiographs come from (tools/demo/README.md)
    'C1': 'C4KC-KiTS (Kidney Tumor Segmentation Challenge), The Cancer Imaging Archive. Heller N. et al. (2019), '
          'https://doi.org/10.7937/TCIA.2019.IX49E8NX. License: CC BY 3.0 (https://creativecommons.org/licenses/by/3.0/).',
    'C2': 'LIDC-IDRI (Lung Image Database Consortium), The Cancer Imaging Archive. Armato III S. G. et al. (2015), '
          'https://doi.org/10.7937/K9/TCIA.2015.LO9QL9SX. License: CC BY 3.0 (https://creativecommons.org/licenses/by/3.0/).',
    'C3': 'NLST (National Lung Screening Trial), The Cancer Imaging Archive. National Lung Screening Trial Research Team (2013), '
          'https://doi.org/10.7937/TCIA.HMQ8-J677. License: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/).',
    'C4': 'Pancreas-CT, The Cancer Imaging Archive. Roth H. et al., Data From Pancreas-CT (Version 2), '
          'https://doi.org/10.7937/K9/TCIA.2016.tNB1kqBU. License: CC BY 3.0 (https://creativecommons.org/licenses/by/3.0/).',
}


def attribution(folder, item):
    """ATTRIBUTION.txt beside a series made from the library: the collection it comes from, its
    licence, and what was changed (the licences ask for both)."""
    (folder / 'ATTRIBUTION.txt').write_text(
        'Derived from: ' + ATTRIBUTION.get(item['collection'], item['collection']) + '\n'
        'Changed for the Medorama demo: re-sliced, cropped, resampled, rotated, elastically deformed, intensity-remapped and noised'
        + (', projected into a radiograph' if item['kind'] == 'xray' else '') + '; every identifier and date invented.\n'
        'It shows no particular patient: the demo assigns it to an invented one.\n', encoding='utf-8')


def ct_from_library(folder, lib, item, pid, when, description):
    """A CT series (one file per slice) from a library series: its HU (+1024) in 12 bits."""
    folder.mkdir(parents=True, exist_ok=True)
    vol = lib.array(item).astype(np.uint16)
    series, study = uid(), uid()
    n, size = vol.shape[0], vol.shape[1]
    px, dz = item['pixel_mm'], item['slice_mm']
    for z in range(n):
        zpos = round(z * dz - n * dz / 2, 2)
        dicom(folder / f'IM{z + 1:04d}.dcm', [
            (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x60, 'CS', 'CT'), (8, 0x103e, 'LO', description),
            (0x10, 0x20, 'LO', pid), (0x18, 0x50, 'DS', f'{dz:g}'), (0x20, 0xd, 'UI', study), (0x20, 0xe, 'UI', series), (0x20, 0x11, 'IS', '2'),
            (0x20, 0x13, 'IS', str(z + 1)), (0x20, 0x32, 'DS', f'{-px * size / 2:g}\\{-px * size / 2:g}\\{zpos:g}'), (0x20, 0x37, 'DS', '1\\0\\0\\0\\1\\0'),
            (0x20, 0x1041, 'DS', f'{zpos:g}'), (0x28, 2, 'US', 1), (0x28, 4, 'CS', 'MONOCHROME2'), (0x28, 0x10, 'US', size),
            (0x28, 0x11, 'US', size), (0x28, 0x30, 'DS', f'{px:g}\\{px:g}'), (0x28, 0x100, 'US', 16), (0x28, 0x101, 'US', 12),
            (0x28, 0x102, 'US', 11), (0x28, 0x103, 'US', 0), (0x28, 0x1050, 'DS', '40'), (0x28, 0x1051, 'DS', '400'),
            (0x28, 0x1052, 'DS', '-1024'), (0x28, 0x1053, 'DS', '1'),
        ], vol[z].astype('<u2').tobytes(), '1.2.840.10008.5.1.4.1.1.2')
    attribution(folder, item)
    return folder


def xray_from_library(path, lib, item, pid, when):
    """A frontal chest radiograph (DX) from a library radiograph (12 bits)."""
    px = lib.array(item).astype(np.uint16)
    size = px.shape[0]
    dicom(path, [
        (8, 0x20, 'DA', f'{when:%Y%m%d}'), (8, 0x60, 'CS', 'DX'), (8, 0x103e, 'LO', 'Chest PA'), (0x10, 0x20, 'LO', pid),
        (0x20, 0xd, 'UI', uid()), (0x20, 0xe, 'UI', uid()), (0x20, 0x11, 'IS', '1'), (0x20, 0x13, 'IS', '1'),
        (0x28, 2, 'US', 1), (0x28, 4, 'CS', 'MONOCHROME2'), (0x28, 0x10, 'US', size), (0x28, 0x11, 'US', size),
        (0x28, 0x30, 'DS', f'{item["pixel_mm"]:g}\\{item["pixel_mm"]:g}'), (0x28, 0x100, 'US', 16), (0x28, 0x101, 'US', 12),
        (0x28, 0x102, 'US', 11), (0x28, 0x103, 'US', 0), (0x28, 0x1050, 'DS', '2000'), (0x28, 0x1051, 'DS', '4000'),
    ], px.astype('<u2').tobytes(), '1.2.840.10008.5.1.4.1.1.1.1')
    (path.parent / f'{path.stem}.ATTRIBUTION.txt').write_text(
        'Derived from: ' + ATTRIBUTION.get(item['collection'], item['collection']) + '\n'
        'Changed for the Medorama demo: projected from a CT volume into a radiograph, deformed, intensity-remapped and noised; '
        'every identifier and date invented.\n', encoding='utf-8')


def imaging(D, p, e, story):
    """The encounter's imaging (radiographs, CT series: from the library when it has them, else
    the models), echo (loops and reports) and cath lab (angiography)."""
    pid, eid = p[0], e[0]
    S = STORIES.get(story, {})
    for kind, name, report, h in S.get('imaging', []):
        when = at(e, h)
        if (pid, fmt(when)) in D.real:
            continue
        if kind.startswith('ct'):
            region = 'abdomen' if 'abdomen' in kind else 'chest'
            base = f'ct_{pid}_{when:%Y%m%d}'
            if D.lib_ct[region]:
                ct_from_library(D.out / 'imaging' / base, D.lib, D.lib_ct[region].pop(0), pid, when, name)
            else:
                ct_series(D.out / 'imaging' / base, region, pid, when)
            file = f'{base}/'
        else:
            base = f'cxr_{pid}_{when:%Y%m%d}'
            opacity = 'right' if kind == 'cxr-right' else None
            if D.lib_xray and opacity is None:
                xray_from_library(D.out / 'imaging' / f'{base}.dcm', D.lib, D.lib_xray.pop(0), pid, when)
            else:
                chest_xray(D.out / 'imaging' / f'{base}.dcm', pid, when, opacity=opacity)
            file = f'{base}.dcm'
        D.t['imaging'].append((pid, eid, fmt(when), name, 'imaging', report, f'../imaging/{file}'))
    if S.get('echo'):
        ef, lvidd, impression = S['echo']
        when = at(e, 26 if e[3] == 'in' else 0.3)
        base = f'echo_{pid}_{when:%Y%m%d}'
        study = uid()
        echo_loop(D.out / 'echo' / f'{base}_a4c.dcm', pid, when, ef=ef, study=study)
        echo_report(D.out / 'echo' / f'{base}_sr.dcm', pid, when + timedelta(minutes=25), ef, lvidd, study)
        tapse = 21 if ef > 0.5 else 15
        D.t['echos'].append((pid, eid, fmt(when), 'Transthoracic echo', 'Apical 4-chamber', '', '', '', '', '', impression, f'../echo/{base}_a4c.dcm'))
        if pid == 'P900004':
            echo_pngs(D.out / 'echo' / f'{base}_a4c_frames', pid, ef)
            D.t['echos'].append((pid, eid, fmt(when), 'Transthoracic echo', 'Apical 4-chamber (exported frames)', '', '', '', '1', '30', '', f'../echo/{base}_a4c_frames/'))
        D.t['echos'].append((pid, eid, fmt(when + timedelta(minutes=25)), 'Echo report', 'Report', round(ef * 100), lvidd, tapse, '', '', impression, f'../echo/{base}_sr.dcm'))
    if S.get('cath'):
        lad, rca, lvedp, impression = S['cath']
        when = at(e, 24.5 if story == 'nstemi_sepsis' else 20)
        name = f'cath_{pid}_{when:%Y%m%d}_lca.dcm'
        if (pid, fmt(when)) not in D.real:
            angio_run(D.out / 'cath' / name, pid, when)
            D.t['caths'].append((pid, eid, fmt(when), 'Coronary angiography', lad, rca, lvedp, impression, f'../cath/{name}'))


# ------------------------------------------------------------------ the second hospital (north)

def north(out, pats, seed):
    """A second hospital's tables: some of the main hospital's patients by the same MRN
    authority (one MRN in lower case), one identity conflict (one patient's MRN with another
    patient's ID, issued by the main hospital), two patients of its own; its patient IDs
    written PT_123 in its patients table and PT123 elsewhere (rules.toml makes them one); its
    ECG files under a folder that its tables name `legacy/` (rules.toml: archive/), moved to
    files/ after the database is built (moved.toml, for db-builder rewrite)."""
    rng = random.Random(seed + 77)
    d = out / 'north'
    for sub in ('archive', 'files'):
        shutil.rmtree(d / sub, ignore_errors=True)
    gen = [p for p in pats if p[0] >= 'P900007']
    shared = [pats[1], pats[4]] + gen[:3]                     # by MRN
    lower = gen[3]                                            # the same MRN, in lower case
    conflict_mrn, conflict_id = gen[4], gen[5]
    own = [('', 'SYN0005001', 'Mbeki', 'Thandi', '1969-04-11', 'F'), ('', 'SYN0005002', 'Varga', 'Andras', '1954-10-30', 'M')]
    rows, encs, labs, notes, ecg_rows = [], [], [], [], []
    people = [(p, p[1]) for p in shared] + [(lower, lower[1].lower())] + [(p, p[1]) for p in own]
    for i, (p, mrn) in enumerate(people):
        nid = f'{701 + i:06d}'
        rows.append((f'PT_{nid}', '', mrn, p[2], p[3], p[4], p[5]))
        a = t('2023-03-01 09:00') + timedelta(days=rng.randint(0, 150), hours=rng.randint(0, 8))
        b = a + timedelta(days=rng.randint(1, 4))
        eid = f'N{8000 + i}'
        encs.append((eid, f'PT{nid}', fmt(a), fmt(b), rng.choice(['Medicine', 'Cardiology', 'Surgery']), 'Home'))
        for name, loinc, unit, lo, hi, norm, sick in LABS[:8]:
            lo, hi, norm = by_sex(name, lo, hi, norm, p[5])
            v = round(norm * rng.uniform(0.9, 1.12), 1)
            labs.append((f'PT{nid}', eid, name, v, unit, lo, hi, flag(v, lo, hi), fmt(a + timedelta(hours=2)), loinc, 'LOINC'))
        notes.append((f'PT{nid}', eid, fmt(b - timedelta(hours=1)), 'Discharge letter', 'Dr. H. Andersen', 'Northside Hospital',
                      f'{p[3]} {p[2]} (DOB {p[4]}) was admitted to Northside Hospital and discharged home in good condition.'))
        if i < 4:
            when = a + timedelta(minutes=30)
            hr = rng.randint(62, 88)
            leads = ecg12(hr)
            meas = ecg_measures(hr)
            comp = twelve_sl(meas, False, False, False)
            fname = f'ecg_{when:%Y%m%d_%H%M}_{i}.xml'
            (d / 'archive' / 'ecg').mkdir(parents=True, exist_ok=True)
            ecg_muse(d / 'archive' / 'ecg' / fname, (p[0], mrn.upper(), p[2], p[3], p[4], p[5]), when, leads, meas, comp, comp)
            ecg_rows.append((f'PT{nid}', eid, fmt(when), '12-lead ECG', 'ecg', '; '.join(comp), f'legacy/ecg/{fname}'))
    # the identity conflict: a registration error, one patient's MRN with another's ID
    rows.append((conflict_id[0], 'DEMO', conflict_mrn[1], conflict_mrn[2], conflict_mrn[3], conflict_mrn[4], conflict_mrn[5]))
    write_csv(d / 'patients.csv', ['Patient ID', 'Issuer', 'MRN', 'Last Name', 'First Name', 'DOB', 'Sex'], rows)
    write_csv(d / 'encounters.csv', ['Encounter ID', 'Patient ID', 'Admit Time', 'Discharge Time', 'Admit Unit', 'Disposition'], encs)
    write_csv(d / 'labs.csv', ['Patient ID', 'Encounter ID', 'Test Name', 'Result', 'Units', 'Ref Low', 'Ref High', 'Abnormal Flag', 'Collected', 'Code', 'Code System'], labs)
    write_csv(d / 'notes.csv', ['Patient ID', 'Encounter ID', 'time', 'note_type', 'author', 'service', 'note_text'], notes)
    write_csv(d / 'ecg_index.csv', ['Patient ID', 'Encounter ID', 'time', 'name', 'category', 'interpretation', 'file'], ecg_rows)
    (d / 'rules.toml').write_text(
        '# db-builder rewrite rules for the second hospital (db-builder/README.md, "Rewrite rules")\n'
        '# its patients table writes PT_000701, its other tables PT000701: one patient ID\n'
        '[patient_id]\n'
        "replace = [{ regex = '(?i)^pt_', to = 'PT' }]\n\n"
        "# its export names its ECG files under legacy/; they are in archive/\n"
        '[[path]]\nprefix = "legacy"\nto = "archive"\n', encoding='utf-8')
    (d / 'moved.toml').write_text(
        '# after the database was built, archive/ was moved to files/: db-builder rewrite repoints its files\n'
        '# (the database keeps paths relative to its own folder; an absolute prefix works the same way)\n'
        '[[path]]\nprefix = "archive"\nto = "files"\n', encoding='utf-8')
    return [p for p, _ in people], (conflict_mrn, conflict_id)


# ------------------------------------------------------------------ the registry (kind views)

REGISTRY_VIEWS = {
    'persons': """SELECT p.id AS person_key, CASE WHEN p.mrn = '' THEN p.study_id ELSE '' END AS patient_id,
       CASE WHEN p.mrn = '' THEN 'REGISTRY' ELSE '' END AS patient_id_issuer, p.mrn, 'DEMO' AS mrn_authority,
       '' AS last_name, '' AS first_name, p.birth_date, p.sex
FROM reg_patient p""",
    'visits': """SELECT v.id AS visit_key, v.patient_id AS person_key, v.started AS start_time, v.ended AS end_time,
       v.site AS unit, 0 AS icu
FROM reg_visit v""",
    'entries': """SELECT 'm' || m.id AS entry_key, m.patient_id AS person_key, m.visit_id AS visit_key, m.category,
       m.measured AS time, NULL AS end_time, m.item AS name, m.code, m.vocabulary, m.value_num, m.value_text, m.unit,
       m.low AS ref_low, m.high AS ref_high, '' AS flag, '' AS author, '' AS text
FROM reg_measure m
UNION ALL
SELECT 'n' || n.id, n.patient_id, n.visit_id, 'note', n.written, NULL, n.title, '', '', NULL, '', '', NULL, NULL, '',
       n.author, n.body
FROM reg_note n""",
    'version': "SELECT MAX(loaded_at) FROM reg_load",
}


def registry(out, pats, stories, encs, seed):
    """A heart-failure registry's own tables (registry/registry.sqlite), connected through
    canonical views (REGISTRY_VIEWS): patients of the main hospital by MRN, two of its own by
    study ID, its visits, scores (NYHA, KCCQ, 6-minute walk), NT-proBNP and LVEF, notes."""
    d = out / 'registry'
    d.mkdir(parents=True, exist_ok=True)
    path = d / 'registry.sqlite'
    if path.exists():
        path.unlink()
    rng = random.Random(seed + 5)
    hf = [p for p in pats if any(stories.get(e[0]) in ('hf', 'hypertension', 'nstemi', 'nstemi_sepsis') for e in encs if e[1] == p[0])][:5]
    people = [(p[1], '', p[4], p[5]) for p in hf] + [('', 'HF-0101', '1957-07-07', 'M'), ('', 'HF-0102', '1948-02-19', 'F')]
    db = sqlite3.connect(path)
    demo_marker(db)
    db.executescript('''
CREATE TABLE reg_patient (id INTEGER PRIMARY KEY, study_id TEXT, mrn TEXT, birth_date TEXT, sex TEXT);
CREATE TABLE reg_visit (id INTEGER PRIMARY KEY, patient_id INTEGER, started TEXT, ended TEXT, site TEXT);
CREATE TABLE reg_measure (id INTEGER PRIMARY KEY, patient_id INTEGER, visit_id INTEGER, category TEXT, measured TEXT, item TEXT,
    code TEXT, vocabulary TEXT, value_num REAL, value_text TEXT, unit TEXT, low REAL, high REAL);
CREATE TABLE reg_note (id INTEGER PRIMARY KEY, patient_id INTEGER, visit_id INTEGER, written TEXT, title TEXT, author TEXT, body TEXT);
CREATE TABLE reg_load (loaded_at TEXT);
''')
    m = v = 0
    for i, (mrn, study, born, sex) in enumerate(people, 1):
        db.execute('INSERT INTO reg_patient VALUES (?, ?, ?, ?, ?)', (i, study or f'HF-{i:04d}', mrn, born, sex))
        nyha = rng.choice(['II', 'III', 'III', 'IV'])
        for k in range(3):
            v += 1
            a = t('2023-10-01 09:00') + timedelta(days=90 * k + rng.randint(0, 20), hours=rng.randint(0, 6))
            db.execute('INSERT INTO reg_visit VALUES (?, ?, ?, ?, ?)', (v, i, fmt(a), fmt(a + timedelta(hours=1)), 'Heart failure clinic'))
            better = k * rng.uniform(0.1, 0.3)
            for cat, item, code, vocab, num, text_, unit, lo, hi in (
                    ('obs', 'NYHA class', '', '', None, nyha if k < 2 else 'II', '', None, None),
                    ('obs', 'KCCQ overall summary score', '71940-4', 'LOINC', round(45 + 40 * better + rng.uniform(-5, 5)), '', 'points', None, None),
                    ('obs', '6-minute walk distance', '64098-7', 'LOINC', round(250 + 300 * better + rng.uniform(-30, 30)), '', 'm', None, None),
                    ('lab', 'NT-proBNP', '33762-6', 'LOINC', round(4200 * (1 - better) + rng.uniform(-200, 200)), '', 'pg/mL', 0, 125),
                    ('obs', 'LVEF', '10230-1', 'LOINC', round(28 + 12 * better), '', '%', 50, 70)):
                m += 1
                db.execute('INSERT INTO reg_measure VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)',
                           (m, i, v, cat, fmt(a + timedelta(minutes=10)), item, code, vocab, num, text_, unit, lo, hi))
            db.execute('INSERT INTO reg_note VALUES (?, ?, ?, ?, ?, ?, ?)',
                       (v, i, v, fmt(a + timedelta(minutes=50)), 'Registry visit', 'Research nurse J. Okonkwo',
                        f'Registry follow-up {k + 1} of 3. Symptoms {"improved" if k else "unchanged"}; medication titrated per protocol.'))
    db.execute("INSERT INTO reg_load VALUES ('2024-05-01 02:00')")
    db.commit()
    db.close()
    (d / 'views.toml').write_text('# the canonical views of the registry (docs/databases.md, "Connect any SQL database through views")\n'
                                  + ''.join(f"{k} = '''\n{v}'''\n" for k, v in REGISTRY_VIEWS.items()), encoding='utf-8')
    return [p for p in hf]


# ------------------------------------------------------------------ the MUSE archive (kind views)

# The views of a MUSE ECG metadata database (docs/databases.md, "A MUSE ECG metadata database"):
# written to muse/views.toml, the file to copy for such a database.
MUSE_MEASURES = [('vr', 'Vent. rate', 'ventricular_rate_bpm', 'bpm'), ('ar', 'Atrial rate', 'atrial_rate_bpm', 'bpm'),
                 ('pr', 'PR', 'pr_interval_ms', 'ms'), ('qrs', 'QRS', 'qrs_duration_ms', 'ms'), ('qt', 'QT', 'qt_interval_ms', 'ms'),
                 ('qtc', 'QTc', 'qtc_ms', 'ms'), ('qtcf', 'QTcF', 'qtc_fridericia_ms', 'ms'), ('rr', 'RR', 'global_rr_ms', 'ms'),
                 ('pax', 'P axis', 'p_axis_deg', '°'), ('rax', 'R axis', 'r_axis_deg', '°'), ('tax', 'T axis', 't_axis_deg', '°'),
                 ('sbp', 'Systolic BP', 'systolic_bp_mmhg', 'mmHg'), ('dbp', 'Diastolic BP', 'diastolic_bp_mmhg', 'mmHg')]
# the MRNs the views read: a patient ID typed at the cart that is not one (a name, "test") is
# left out (a site writes its own MRNs' form: digits only, `patient_id NOT GLOB '*[^0-9]*'`)
MUSE_MRN = "patient_id GLOB 'SYN[0-9]*'"
# an invented patient whose ECGs are the archive's only data on them
MUSE_ONLY = ('P990001', 'SYN0000990', 'Achterberg', 'Tibor', '1949-07-21', 'M')
MUSE_VIEWS = {
    'persons': f"""SELECT patient_id AS person_key, '' AS patient_id, '' AS patient_id_issuer,
       patient_id AS mrn, '' AS mrn_authority,
       COALESCE(last_name, '') AS last_name, COALESCE(first_name, '') AS first_name,
       COALESCE(birth_date, '') AS birth_date,
       CASE sex WHEN 'female' THEN 'F' WHEN 'male' THEN 'M' ELSE '' END AS sex,
       (SELECT MIN(e.acquisition_datetime) FROM ecg e WHERE e.patient_id = ecg.patient_id) AS earliest,
       MAX(acquisition_datetime) AS latest
FROM ecg WHERE {MUSE_MRN}
GROUP BY patient_id""",
    'entries': f"""SELECT 'e' || md5 AS entry_key, patient_id AS person_key, NULL AS visit_key, 'ecg' AS category,
       acquisition_datetime AS time, NULL AS end_time, '12-lead ECG' AS name, '' AS code,
       '' AS vocabulary, NULL AS value_num, '' AS value_text, '' AS unit, NULL AS ref_low,
       NULL AS ref_high, '' AS flag, '' AS author,
       COALESCE(NULLIF(diagnosis_text, ''), original_diagnosis_text, '') AS text
FROM ecg WHERE {MUSE_MRN}
""" + '\n'.join(f"UNION ALL SELECT 'm' || md5 || ':{k}', patient_id, NULL, 'ecg', acquisition_datetime, NULL, '{n}', '', '', {c}, '', '{u}', "
                f"NULL, NULL, '', '', '' FROM ecg WHERE {MUSE_MRN} AND {c} IS NOT NULL" for k, n, c, u in MUSE_MEASURES),
    'recordings': f"""SELECT 'e' || md5 || ':file' AS recording_key, patient_id AS person_key, NULL AS visit_key,
       source_file AS file, 'GE MUSE XML' AS format, acquisition_datetime AS start_time,
       NULL AS end_time, '' AS label, 'ecg' AS category, NULL AS size, md5 AS fingerprint
FROM ecg WHERE {MUSE_MRN}
  AND source_file IS NOT NULL AND source_file <> ''""",
    'version': "SELECT COUNT(*) || ' ' || MAX(added_at) FROM additions",
    # who has an ECG: `has ecg` reads this, not every measurement (docs/databases.md)
    'has': f"SELECT DISTINCT patient_id AS person_key, 'ecg' AS category FROM ecg WHERE {MUSE_MRN}",
    # the catalog, as constants: connecting reads no entry for it (docs/design/connected-databases.md)
    'variables': "SELECT 'ecg' AS category, '12-lead ECG' AS name, '' AS vocabulary, '' AS code, '' AS unit, 0 AS numeric\n"
                 + '\n'.join(f"UNION ALL SELECT 'ecg', '{n}', '', '', '{u}', 1" for _, n, _, u in MUSE_MEASURES),
}


def muse_archive(out, pats, lib, seed):
    """An ECG management system's archive (muse/): older resting ECGs of the main hospital's
    patients as MUSE XML files (xml/<year>/MUSE_<time>_<n>.xml), with an ECG metadata
    database of them (ecg_metadata.sqlite: one row per file by MD5, the columns the views read), connected through MUSE_VIEWS, the patient ID read as the MRN.  One ECG is at 250 Hz
    (a MAC5K with 12SL 237), and one does not give its sampling rate."""
    import hashlib
    d = out / 'muse'
    if d.exists():
        shutil.rmtree(d)
    rng = random.Random(seed + 13)
    # the main hospital's first patients, and one known to the ECG archive alone (no visit, no
    # medication, no condition: the patient page leaves its left column out)
    people = pats[:8] + [MUSE_ONLY]
    rows = []
    for i, p in enumerate(people):
        for k in range(2):
            when = t('2019-02-01 08:00') + timedelta(days=rng.randint(0, 4 * 365), hours=rng.randint(0, 9), minutes=rng.randint(0, 59))
            hr, lvh, af = rng.randint(56, 98), i == 3, i == 6 and k == 1
            leads = ecg12_any(lib, 100 + 2 * i + k, hr, False, af, lvh)
            m = ecg_measures(hr, af, lvh)
            computer = twelve_sl(m, af, False, lvh)
            device, fs, given = ('MAC55', '241'), 500, True
            if i == 1 and k == 0:
                device, fs = ('MAC5K', '237'), 250
            if i == 4 and k == 1:
                given = False
            rel = f'xml/{when:%Y}/MUSE_{when:%Y%m%d_%H%M%S}_{10000 + 2 * i + k}.xml'
            path = ecg_muse(d / rel, p, when, leads, m, computer, computer, fs=fs, device=device, rate_given=given)
            g = lambda name: m.get(name, (None, ''))[0]
            rr = round(60000 / hr) if fs == 500 else round(round(60000 / hr) / 4) * 4
            rows.append((hashlib.md5(path.read_bytes()).hexdigest(), 'computed from file', rel, p[1], p[2].upper(), p[3].upper(), p[4],
                         'female' if p[5] == 'F' else 'male', f'{when:%Y-%m-%dT%H:%M:%S}', device[0], device[1], 'CONFIRMED',
                         fs if given else None, 'waveform' if given else None, 'known' if given else 'unreadable',
                         '' if given else 'BAD_SAMPLE_BASE', g('Vent. rate'), g('Atrial rate'), g('PR'), g('QRS'), g('QT'), g('QTc'),
                         g('QTcF'), g('P axis'), g('R axis'), g('T axis'), None, None, rr if given else None,
                         json.dumps(computer), '\n'.join(computer), json.dumps(computer), '\n'.join(computer),
                         '2024-05-01T02:00:00', 'muse_2019_2023_ecg_db.csv'))
    # a cart's test ECG, its patient ID typed as a word: in the database, left out by the views
    rows.append((hashlib.md5(b'cart test').hexdigest(), 'computed from file', '', 'test', 'TEST', 'TEST', None, None,
                 '2023-01-09T07:30:00') + rows[0][9:])
    db = sqlite3.connect(d / 'ecg_metadata.sqlite')
    demo_marker(db)
    db.executescript('''
CREATE TABLE ecg (md5 TEXT PRIMARY KEY, md5_source TEXT, source_file TEXT, patient_id TEXT, last_name TEXT, first_name TEXT,
  birth_date TEXT, sex TEXT, acquisition_datetime TEXT, acquisition_device TEXT, analysis_software_version TEXT, status TEXT,
  fs REAL, fs_method TEXT, rate_format TEXT, rate_warnings TEXT, ventricular_rate_bpm REAL, atrial_rate_bpm REAL,
  pr_interval_ms REAL, qrs_duration_ms REAL, qt_interval_ms REAL, qtc_ms REAL, qtc_fridericia_ms REAL, p_axis_deg REAL,
  r_axis_deg REAL, t_axis_deg REAL, systolic_bp_mmhg REAL, diastolic_bp_mmhg REAL, global_rr_ms REAL,
  diagnosis_statements TEXT, diagnosis_text TEXT, original_diagnosis_statements TEXT, original_diagnosis_text TEXT,
  added_at TEXT, added_from TEXT);
CREATE INDEX ecg_patient ON ecg (patient_id);
CREATE INDEX ecg_time ON ecg (acquisition_datetime);
''' + ''.join(f'CREATE INDEX ecg_{c} ON ecg ({c}, patient_id, acquisition_datetime);\n' for _, _, c, _ in MUSE_MEASURES) + '''
CREATE TABLE additions (id INTEGER PRIMARY KEY, added_at TEXT, source TEXT, rows_read INTEGER, rows_added INTEGER,
  rows_duplicate_md5 INTEGER, md5_from_linkage INTEGER, md5_computed INTEGER, rows_excluded INTEGER);
''')
    db.executemany(f'INSERT INTO ecg VALUES ({", ".join("?" * len(rows[0]))})', rows)
    db.execute("INSERT INTO additions VALUES (1, '2024-05-01T02:00:00', 'muse_2019_2023_ecg_db.csv', ?, ?, 0, 0, ?, 0)",
               (len(rows), len(rows), len(rows)))
    db.commit()
    db.close()
    (d / 'views.toml').write_text("# the views of a MUSE ECG metadata database (docs/databases.md, \"A MUSE ECG metadata database\")\n"
                                  + ''.join(f"{k} = '''\n{v}'''\n" for k, v in MUSE_VIEWS.items()), encoding='utf-8')
    return people


# ------------------------------------------------------------------ the encrypted database (secure)

def secure(out, pats, seed):
    """A transplant clinic's tables, made into a SQLCipher-encrypted db-builder database: three
    of the main hospital's patients by MRN and one of its own; the demo key in keys/."""
    d = out / 'secure'
    rng = random.Random(seed + 9)
    gen = [p for p in pats if p[0] >= 'P900007']
    people = [pats[0]] + gen[10:12] + [('', 'SYN0006001', 'Aalto', 'Kaisa', '1975-05-05', 'F')]
    rows, labs, notes = [], [], []
    for i, p in enumerate(people):
        rows.append((p[1], p[2], p[3], p[4], p[5]))
        a = t('2024-04-01 08:30') + timedelta(days=7 * i)
        for k in range(4):
            when = a + timedelta(days=14 * k)
            v = round(rng.uniform(5, 13), 1)
            labs.append((p[1], 'Tacrolimus trough', v, 'ng/mL', 5, 10, flag(v, 5, 10), fmt(when), '11253-2', 'LOINC'))
        notes.append((p[1], fmt(a + timedelta(hours=1)), 'Transplant clinic note', 'Dr. E. Haugen', 'Transplant',
                      f'{p[3]} {p[2]} (DOB {p[4]}): kidney transplant follow-up; graft function stable.'))
    write_csv(d / 'patients.csv', ['MRN', 'Last Name', 'First Name', 'DOB', 'Sex'], rows)
    write_csv(d / 'labs.csv', ['MRN', 'Test Name', 'Result', 'Units', 'Ref Low', 'Ref High', 'Abnormal Flag', 'Collected', 'Code', 'Code System'], labs)
    write_csv(d / 'notes.csv', ['MRN', 'time', 'note_type', 'author', 'service', 'note_text'], notes)
    key = __import__('hashlib').sha256(f'medorama-demo-key-{seed}'.encode()).hexdigest()
    (out / 'keys').mkdir(exist_ok=True)
    (out / 'keys' / 'demo_secure.key').write_text(key + '\n', encoding='ascii')
    return people


# ------------------------------------------------------------------ the databases to build and connect

def manifest(out, full, tables):
    """demo-databases.json: each database's name, kind, file, settings, key file, and the
    db-builder steps that build it ('@' + a path relative to OUT)."""
    tz = 'America/New_York'
    dbs = [{'name': 'demo', 'kind': 'dbb', 'file': 'demo.sqlite', 'build': [
        ['new', '@demo.sqlite', '--timezone', tz, '--demo'],
        # the patients and their encounters first: every other table names them
        ['add', '@demo.sqlite', '@databases/patients.csv', '@databases/encounters.xlsx', *[f'@databases/{x}' for x in tables],
         '--issuer', 'DEMO', '--mrn-authority', 'DEMO'],
    ]}]
    if full:
        dbs[0]['build'] += [['recordings', '@demo.sqlite', '@sleeplab', '--pattern', '{patient}_*.edf', '--issuer', 'DEMO', '--category', 'sleep'],
                            ['recordings', '@demo.sqlite', '@eeglab', '--pattern', '{patient}_*.bdf', '--issuer', 'DEMO', '--category', 'eeg']]
        dbs.append({'name': 'demo_north', 'kind': 'dbb', 'file': 'north/north.sqlite', 'build': [
            ['new', '@north/north.sqlite', '--timezone', tz, '--demo'],
            ['add', '@north/north.sqlite', '@north/patients.csv', '@north/encounters.csv', '@north/labs.csv', '@north/notes.csv',
             '@north/ecg_index.csv', '--issuer', 'NORTH', '--mrn-authority', 'DEMO', '--rules', '@north/rules.toml'],
            ['move', '@north/archive', '@north/files'],
            ['rewrite', '@north/north.sqlite', '--rules', '@north/moved.toml'],
        ]})
    dbs.append({'name': 'demo_omop', 'kind': 'omop', 'file': 'omop/omop.sqlite', 'settings': {'mrn_authority': 'DEMO', 'timezone': tz}})
    if full:
        dbs.append({'name': 'demo_registry', 'kind': 'views', 'file': 'registry/registry.sqlite',
                    'settings': {**REGISTRY_VIEWS, 'mrn_authority': 'DEMO', 'timezone': tz}})
        dbs.append({'name': 'demo_muse', 'kind': 'views', 'file': 'muse/ecg_metadata.sqlite',
                    'settings': {**MUSE_VIEWS, 'mrn_authority': 'DEMO', 'timezone': tz}})
        dbs.append({'name': 'demo_secure', 'kind': 'dbb', 'file': 'secure/secure.sqlite', 'key_file': 'keys/demo_secure.key', 'encrypt': True, 'build': [
            ['new', '@secure/secure.sqlite', '--timezone', tz, '--demo'],
            ['add', '@secure/secure.sqlite', '@secure/patients.csv', '@secure/labs.csv', '@secure/notes.csv', '--issuer', 'SECURE', '--mrn-authority', 'DEMO'],
        ]})
    m = {'made_by': 'tools/make-demo-data.py', 'databases': dbs}
    if (out / 'actions').is_dir():
        m['actions_dirs'] = ['actions']
    (out / 'demo-databases.json').write_text(json.dumps(m, indent=1) + '\n', encoding='utf-8')
    return m


def build(out, exe):
    """Builds the demo's db-builder databases (demo-databases.json's steps) with `exe`."""
    m = json.loads((out / 'demo-databases.json').read_text(encoding='utf-8'))
    for d in m['databases']:
        if not d.get('build'):
            continue
        f = out / d['file']
        for suffix in ('', '-wal', '-shm', '-journal'):
            Path(f'{f}{suffix}').unlink(missing_ok=True)
        env = dict(os.environ)
        if d.get('key_file'):
            env['DB_BUILDER_KEY'] = (out / d['key_file']).read_text(encoding='ascii').strip()
        for step in d['build']:
            args = [str(out / a[1:]) if a.startswith('@') else a for a in step]
            if step[0] == 'move':
                shutil.rmtree(args[2], ignore_errors=True)
                shutil.move(args[1], args[2])
                continue
            cmd = [exe] + (['--encrypt'] if d.get('encrypt') else []) + args
            r = subprocess.run(cmd, env=env, capture_output=True, text=True, encoding='utf-8', errors='replace')
            if r.returncode:
                sys.exit(f'db-builder {step[0]} ({d["name"]}) failed:\n{r.stderr.strip() or r.stdout.strip()}')
            last = (r.stdout.strip().splitlines() or [''])[-1]
            print(f'  {d["name"]}: {step[0]}: {last}')


def identities(full=True, seed=SEED):
    """Every person of the demo's databases, as tools/screenshots.py checks them:
    [{'mrn', 'last_name', 'first_name', 'birth_date', 'patient_id'}] (MRNs as the main hospital
    writes them; persons of a database's own included)."""
    pats, encs, stories = all_people(full, seed)
    out = [{'patient_id': p[0], 'mrn': p[1], 'last_name': p[2], 'first_name': p[3], 'birth_date': p[4]} for p in pats]
    omop_own = [('SYN0000101', '1949-09-21'), ('SYN0000102', '1983-12-02')]
    if full:
        omop_own += [(m, b) for m, _, b, s in omop_extra(pats, encs, stories) if not any(p[1] == m for p in pats)]
        out += [{'patient_id': 'PT000707', 'mrn': 'SYN0005001', 'last_name': 'Mbeki', 'first_name': 'Thandi', 'birth_date': '1969-04-11'},
                {'patient_id': 'PT000708', 'mrn': 'SYN0005002', 'last_name': 'Varga', 'first_name': 'Andras', 'birth_date': '1954-10-30'},
                {'patient_id': '', 'mrn': 'SYN0006001', 'last_name': 'Aalto', 'first_name': 'Kaisa', 'birth_date': '1975-05-05'},
                {'patient_id': 'HF-0101', 'mrn': '', 'last_name': '', 'first_name': '', 'birth_date': '1957-07-07'},
                {'patient_id': 'HF-0102', 'mrn': '', 'last_name': '', 'first_name': '', 'birth_date': '1948-02-19'},
                # the ECG archive's own patient, named as the archive writes names (in capitals)
                {'patient_id': '', 'mrn': MUSE_ONLY[1], 'last_name': MUSE_ONLY[2].upper(), 'first_name': MUSE_ONLY[3].upper(),
                 'birth_date': MUSE_ONLY[4]}]
    out += [{'patient_id': '', 'mrn': m, 'last_name': '', 'first_name': '', 'birth_date': b} for m, b in omop_own]
    return out


def omop_extra(pats, encs, stories):
    """The OMOP database's persons beyond the first four: four of the main hospital's (by MRN)
    and two of its own [(MRN, sex concept, born, story)]."""
    gen = [p for p in pats if p[0] >= 'P900007']
    story = lambda p: next((stories[e[0]] for e in encs if e[1] == p[0]), 'checkup')  # noqa: E731
    shared = [(p[1], 8532 if p[5] == 'F' else 8507, p[4], story(p)) for p in gen[12:16]]
    return shared + [('SYN0000103', 8507, '1972-01-15', 'nstemi'), ('SYN0000104', 8532, '1990-08-08', 'hf')]


def copy_action(out):
    """The example action plug-in (medorama/actions/examples/recording-summary), run with this
    Python (any platform)."""
    src = ROOT / 'medorama' / 'actions' / 'examples' / 'recording-summary'
    if not src.is_dir():
        return
    dst = out / 'actions' / 'recording-summary'
    shutil.rmtree(dst, ignore_errors=True)
    dst.mkdir(parents=True)
    shutil.copy2(src / 'summary.py', dst / 'summary.py')
    text = (src / 'action.toml').read_text(encoding='utf-8')
    py = json.dumps(sys.executable)
    text = '\n'.join(f'command = [{py}, "summary.py"]' if line.startswith('command =') else line for line in text.splitlines()) + '\n'
    (dst / 'action.toml').write_text(text, encoding='utf-8')


def main():
    args = sys.argv[1:]
    opts = {}
    for name in ('--examples', '--dicom', '--db-builder', '--seed'):
        if name in args:
            i = args.index(name)
            opts[name] = args[i + 1] if i + 1 < len(args) else ''
            del args[i:i + 2]
    flags = {f for f in ('--small', '--no-library') if f in args}
    args = [a for a in args if a not in flags]
    if any(a in ('-h', '--help') for a in args):
        print(__doc__)
        return
    unknown = [a for a in args if a.startswith('-')]
    if unknown or len(args) > 1:
        sys.exit(f'make-demo-data.py: unexpected {" ".join(unknown or args[1:])} (--help shows the options)')
    if np is None:
        sys.exit('NumPy is needed: run this with .venv/bin/python (docs/administration.md, "The demo")')
    examples, dicoms = opts.get('--examples'), opts.get('--dicom')
    seed = int(opts.get('--seed') or SEED)
    full = '--small' not in flags
    R.seed(seed)
    out = Path(args[0] if args else ROOT / 'sample_data' / 'demo').resolve()
    lib = None if '--no-library' in flags else library(LIBRARY)
    for d in ('icu', 'holter', 'sleep', 'ecg', 'imaging', 'echo', 'cath', 'databases'):
        (out / d).mkdir(parents=True, exist_ok=True)
    pats, encs, stories = all_people(full, seed)
    by_patient = {p[0]: p for p in pats}
    db = out / 'databases'
    write_csv(db / 'patients.csv', ['MRN', 'Last Name', 'First Name', 'DOB', 'Sex', 'Patient ID'],
              [(p[1], p[2], p[3], p[4], p[5], p[0]) for p in pats])
    rows = [['Encounter ID', 'Patient ID', 'CSN', 'Admit Time', 'Discharge Time', 'Admit Unit', 'Disposition', 'Care Unit', 'Bed', 'Bed Start', 'Bed End']]
    for e in encs:
        rows.append([e[0], e[1], e[2], e[4], e[5], e[6], e[7], '', '', '', ''])
        if e[8]:
            rows.append([e[0], e[1], e[2], e[4], e[5], e[6], e[7], e[8][0], e[8][1], e[8][2], e[8][3]])
    write_xlsx(db / 'encounters.xlsx', rows)

    D = Demo(out, lib, full, seed)
    D.real = set()
    if dicoms is not None:
        D.real = add_dicom_examples(dicoms, out, D.t['imaging'], D.t['caths'])
    for e in encs:
        fill(D, by_patient[e[1]], e, stories[e[0]])
    # a patient known only by MRN, with laboratory results and no encounter
    for d in range(3):
        when = t('2024-03-20 08:00') + timedelta(days=30 * d)
        for name, loinc, unit, lo, hi, norm, _ in LABS[:8]:
            lo, hi, norm = by_sex(name, lo, hi, norm, 'M')           # Hiro Tanaka
            v = round(norm * R.uniform(0.95, 1.05), 1)
            D.t['labs'].append(('SYN0000006', '', name, v, unit, lo, hi, flag(v, lo, hi), fmt(when), loinc, 'LOINC'))
    allergies = [('P900001', 'allergy', 'Penicillin', 'rash'), ('P900002', 'allergy', 'Iodinated contrast', 'urticaria')]
    if full:
        rng = random.Random(seed + 3)
        for p in pats[6:]:
            if rng.random() < 0.3:
                allergies.append((p[0], 'allergy', *rng.choice([('Sulfonamides', 'rash'), ('Latex', 'contact dermatitis'), ('Peanuts', 'anaphylaxis'),
                                                                ('Codeine', 'nausea'), ('Penicillin', 'hives')])))
    T = D.t
    write_csv(db / 'labs.csv', ['MRN', 'Encounter ID', 'Test Name', 'Result', 'Units', 'Ref Low', 'Ref High', 'Abnormal Flag', 'Collected', 'Code', 'Code System'], T['labs'])
    write_csv(db / 'vitals.csv', ['patient_id', 'encounter_id', 'name', 'value', 'unit', 'ref_low', 'ref_high', 'charttime'], T['vitals'])
    write_csv(db / 'medications.csv', ['patient_id', 'encounter_id', 'Medication', 'Dose', 'Units', 'Start Time', 'Stop Time'], T['meds'])
    write_csv(db / 'diagnoses.csv', ['patient_id', 'encounter_id', 'ICD10', 'Description', 'Date'], T['dx'])
    write_csv(db / 'microbiology.csv', ['patient_id', 'encounter_id', 'Test', 'Collected', 'Result', 'Flag', 'Comment'], T['micro'])
    write_csv(db / 'notes.csv', ['patient_id', 'encounter_id', 'time', 'note_type', 'author', 'service', 'note_text'], T['notes'])
    write_csv(db / 'allergies.csv', ['patient_id', 'category', 'name', 'reaction'], allergies)
    tables = ['labs.csv', 'vitals.csv', 'medications.csv', 'diagnoses.csv', 'microbiology.csv', 'notes.csv', 'allergies.csv']
    if full:
        write_csv(db / 'procedures.csv', ['patient_id', 'encounter_id', 'Procedure', 'Code', 'Code System', 'Time'], T['procs'])
        write_csv(db / 'transfusions.csv', ['patient_id', 'encounter_id', 'Product', 'Amount', 'Units', 'Time'], T['blood'])
        write_csv(db / 'life_support.csv', ['patient_id', 'encounter_id', 'Support', 'Value', 'Units', 'Start Time', 'Stop Time'], T['support'])
        write_csv(db / 'assessments.csv', ['patient_id', 'encounter_id', 'Item', 'Value', 'Time'], T['obs'])
        write_csv(db / 'pathology.csv', ['patient_id', 'encounter_id', 'time', 'note_type', 'author', 'report'], T['path'])
        tables += ['procedures.csv', 'transfusions.csv', 'life_support.csv', 'assessments.csv', 'pathology.csv']
    if examples is not None:
        print(f'{add_examples(examples, out, T["ecgs"])} example ECG(s) added from {examples}')
    write_csv(db / 'ecg_index.csv', ['patient_id', 'encounter_id', 'time', 'name', 'value', 'unit',
                                     *[f'{c} ({u})' for c, u in ECG_COLUMNS.items()], 'interpretation', 'file'], T['ecgs'])
    write_csv(db / 'icu_index.csv', ['patient_id', 'encounter_id', 'file'], T['icu'])
    write_csv(db / 'holter_index.csv', ['patient_id', 'encounter_id', 'category', 'file'], T['holter'])
    # the first patients' overnight sleep study (an EDF+ file, its own column): 22:30 at the
    # sleep laboratory (New York), which the file states as 03:30 (its clock time is read as UTC)
    sleep_study(out / 'sleep' / 'P900004_psg_20240305.edf', t('2024-03-06 03:30'), seed=7)
    write_csv(db / 'sleep_index.csv', ['patient_id', 'encounter_id', 'time', 'study', 'category', 'file'],
              [('P900004', 'E9100010', '2024-03-05 22:30', 'Polysomnography', 'sleep', '../sleep/P900004_psg_20240305.edf')])
    write_csv(db / 'imaging_index.csv', ['patient_id', 'encounter_id', 'time', 'study', 'category', 'impression', 'file'], T['imaging'])
    write_csv(db / 'echo_index.csv', ['patient_id', 'encounter_id', 'time', 'study', 'view', 'LVEF (%)', 'LVIDd (cm)', 'TAPSE (mm)',
                                      'pixel spacing (mm)', 'frame rate (fps)', 'impression', 'file'], T['echos'])
    write_csv(db / 'cath_index.csv', ['patient_id', 'encounter_id', 'time', 'procedure', 'LAD stenosis (%)', 'RCA stenosis (%)', 'LVEDP (mmHg)',
                                      'impression', 'file'], T['caths'])
    tables += ['ecg_index.csv', 'icu_index.csv', 'holter_index.csv', 'sleep_index.csv', 'imaging_index.csv', 'echo_index.csv', 'cath_index.csv']
    extra = []
    if full:
        # more sleep studies, added by a pattern on their file names ({patient}_*.edf)
        for e in encs:
            if stories[e[0]] == 'sleep' and e[1] != 'P900004':
                a = t(e[4])
                sleep_study(out / 'sleeplab' / f'{e[1]}_psg_{a:%Y%m%d}.edf', a + timedelta(hours=5), seed=int(e[1][-3:]), hours=2.0, ahi=22)
        (out / 'eeglab').mkdir(exist_ok=True)
        north(out, pats, seed)
        extra = omop_extra(pats, encs, stories)
        registry(out, pats, stories, encs, seed)
        muse_archive(out, pats, lib, seed)
        secure(out, pats, seed)
        copy_action(out)
    write_omop(out / 'omop' / 'omop.sqlite', extra)
    manifest(out, full, tables)
    (out / 'README.txt').write_text(__doc__.replace('OUT', str(out)), encoding='utf-8')
    # what went in: models only, the demo library, local files (--examples, --dicom); a
    # picture of the demo is published only without local files (tools/screenshots.py --demo)
    (out / 'demo.json').write_text(json.dumps({
        'made_by': 'tools/make-demo-data.py',
        'full': full,
        'seed': seed,
        'invented': lib is None and examples is None and dicoms is None,
        'library': lib is not None,
        'examples': examples is not None,
        'dicom': dicoms is not None,
    }, indent=2) + '\n', encoding='utf-8')
    print(f'demo data written to {out}: {len(pats)} patients, {len(encs)} encounters, {len(T["icu"])} ICU recordings, '
          f'{len(T["ecgs"])} ECGs, {len(T["imaging"])} images, {len(T["labs"])} lab results, {len(T["notes"])} notes'
          + (' (with the demo library)' if lib else ' (models only)'))
    if opts.get('--db-builder'):
        build(out, opts['--db-builder'])


if __name__ == '__main__':
    main()
