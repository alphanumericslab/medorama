# Building a database with db-builder: inputs and their columns

This page is the reference for what [db-builder](../db-builder/README.md) reads: which file types,
which column names it recognizes for each kind of table, how rows find their patients and
encounters, and how times, identifiers, units, categories and files are read. It is written for
data managers and for the people who export data from a hospital's systems.

**Medorama itself imports nothing.** It connects databases and reads them in place
([databases.md](databases.md)). Tables, known exports and folders of recordings become a database
with db-builder first, and that SQLite file is then connected to Medorama as kind `dbb`.
db-builder's database is open (its schema: [db-builder/docs/schema.md](../db-builder/docs/schema.md))
and not specific to Medorama. The commands are in [db-builder/README.md](../db-builder/README.md).

## Contents

- [How db-builder works](#how-db-builder-works)
- [Table format](#table-format)
- [Column reference](#column-reference)
  - [How column names are recognized](#how-column-names-are-recognized)
  - [Patients](#patients)
  - [Encounters](#encounters)
  - [Bed stays](#bed-stays)
  - [Results and measurements](#results-and-measurements)
  - [Medications](#medications)
  - [Conditions, procedures and allergies](#conditions-procedures-and-allergies)
  - [Notes and reports](#notes-and-reports)
  - [Files and recordings](#files-and-recordings)
  - [Measurement columns](#measurement-columns)
  - [All recognized column names](#all-recognized-column-names)
- [Categories](#categories)
- [Identifiers: patient IDs, MRNs and authorities](#identifiers-patient-ids-mrns-and-authorities)
- [How rows find their patients](#how-rows-find-their-patients)
- [Times and time zones](#times-and-time-zones)
- [Values, units and flags](#values-units-and-flags)
- [Files](#files)
- [Known exports and declared formats](#known-exports-and-declared-formats)
- [Examples](#examples)
- [Building and connecting](#building-and-connecting)
- [Privacy](#privacy)
- [Checklist](#checklist)

## How db-builder works

- **Inputs are exported from the institution's systems:** an EHR's patients, encounters, results
  and notes; an ECG management system's index; a PACS index; a CLIF folder; folders of
  recordings. `db-builder add` adds tables and known exports; `db-builder recordings` adds a
  folder of recordings.
- **Each file is a source** (`sources`), identified by its path and fingerprint. The same file
  added again unchanged is not added twice; a file that changed replaces its earlier version.
  `db-builder remove-source` takes a source out, with the persons and visits no other source
  names.
- **Tables can come in any order.** A row that names an encounter no table has named yet keeps
  the encounter's key, and gets its person when the encounters' table is added. A row that names
  only its encounter (`encounter_id`, no `patient_id` or `mrn`, nothing that describes the
  encounter) refers to that encounter; it does not make one.
- **One `add` is all or nothing:** its files are added in one transaction, and if one fails,
  nothing is added.
- **Rows that cannot be read** are left out, with a warning that names the file and the line.
  `add` prints, for each source, what it added and how many rows it left out.
- **Nothing is lost.** Every column db-builder does not recognize is kept in `extras`, under its
  own header, with the row it belongs with. Column names are never changed.
- **It writes only its output database**, never into the folders it reads.

## Table format

| Property | Rule |
|---|---|
| File types | CSV (comma-separated, UTF-8, with or without a byte-order mark); Parquet (recognized by its content); Excel (`.xlsx`, `.xls`); OpenDocument (`.ods`). Of a spreadsheet, only the first sheet is read, and a spreadsheet larger than 100 MB is refused (it is read whole: save a larger table as CSV). CSV and Parquet files are read row by row, of any size. |
| Encoding and separator | A file whose columns are separated by semicolons or tabs, a UTF-16 file, and a file that is not UTF-8 (Latin-1, Windows-1252) are refused, with a message that says which; they are not guessed at, so that no text is read wrongly. Save the table as CSV in UTF-8 ("CSV UTF-8" in Excel). |
| Header | The first row holds one name per column. When two columns would be read as the same field (`value,value`), the first is; the other goes to `extras`, and a warning says so. |
| Rows | One row per patient, encounter, bed stay, result, note or file, or several of these at once: one row may describe a patient, the patient's encounter and a result. Empty rows are skipped. |
| Cells | Text or numbers. Leading and trailing spaces are removed. An empty cell means "no value". |
| Warnings | A row that cannot be read is named by its file and line (`labs.csv, line 12`). |

## Column reference

### How column names are recognized

db-builder maps each column of a table onto one of its **fields** (`patient_id`, `mrn`, `time`,
`value` and so on). It recognizes a column by its header:

1. Headers are compared ignoring case, spaces and punctuation: `Patient ID`, `patient_id` and
   `PATIENTID` are the same name.
2. A header that is a field's own name is taken first. Then the other recognized names
   (synonyms) are tried. Each header is used for one field only.
3. A column named `unit` beside a `bed` column, in a table without a `value` column, is read as
   the care unit.
4. A column that is not recognized goes to `extras` under its own name. A column whose name
   carries a unit becomes a searchable measurement ([Measurement columns](#measurement-columns)).

When the names differ, map the columns once with a mapping file (`--mapping`, a TOML or JSON file
of `field = "header"`, under `[columns]` or at the top level). When a mapping is given, synonyms
are not used: a header is read as a field only if the mapping names it for that field, or if the
header is that field's own name (ignoring case) and the mapping names no other header for that
field.

The tables below list, for each kind of table, the columns that matter and the names they are
recognized by. Names are written in lower case without spaces or punctuation. **PHI** marks the
identifiers that Medorama shows only with the `phi` grant and the PHI permission, and hides in
Privacy Mode.

### Patients

A patients table names each patient and describes them. Each row needs at least one identifier.

| Field | Required | Recognized names | Content |
|---|---|---|---|
| `patient_id` | `patient_id` or `mrn` | patientid, patient, subjectid, subject, pid, personid | The institution's or the study's patient identifier. |
| `patient_id_issuer` | With `patient_id` (or `--issuer`) | patientidissuer, issuer, idissuer | Who issued the patient ID ([Identifiers](#identifiers-patient-ids-mrns-and-authorities)). |
| `mrn` | `patient_id` or `mrn` | mrn, medicalrecordnumber, medrecno, mrno | The medical record number (**PHI**). |
| `mrn_authority` | With `mrn` (or `--mrn-authority`) | mrnauthority, authority, assigningauthority | The system that issued the MRN. |
| `last_name` | No | lastname, surname, familyname | **PHI**. |
| `first_name` | No | firstname, givenname | **PHI**. |
| `birth_date` | No | birthdate, dob, dateofbirth | `YYYY-MM-DD`, `MM/DD/YYYY` or `YYYYMMDD` (**PHI**). |
| `sex` | No | sex, gender | `F`, `M`; any other value goes to `extras`. |

Names, date of birth and sex describe a patient; they never identify one. A later row fills what
is empty and never overwrites what an earlier row gave; a warning says when they differ.

### Encounters

An encounter is a hospital stay or a visit (a `visits` row). A row creates or describes an
encounter only when it has an `encounter_id`; the encounter belongs to the row's patient.

| Field | Required | Recognized names | Content |
|---|---|---|---|
| `encounter_id` | Yes | encounterid, encounter, visitid, visit, admissionid, hadmid, stayid | The encounter's identifier (its `visit_key`). |
| `patient_id` or `mrn` | Yes, for a new encounter | as above | The encounter's patient. |
| `csn` | No | csn, accountnumber, account, fin | The account or financial number (**PHI**; in `extras`). Entries of other tables can name the encounter by it. |
| `admit_time` | No | admittime, admit, admissiontime, admitted, admitdate, admissiondate | The start of the stay ([Times](#times-and-time-zones)). |
| `discharge_time` | No | dischargetime, discharge, dischtime, discharged, dischargedate | The end of the stay. |
| `admit_unit` | No | admitunit, admissionunit | The admitting unit or service. |
| `disposition` | No | disposition, dischargedisposition | The discharge disposition (in `extras`). |

A visit is an intensive-care stay (`icu` = 1) when its admitting unit or a care unit of the stay
is an intensive-care unit.

### Bed stays

A bed stay is a stretch of an encounter spent in one bed of one care unit. A bed stay in an
intensive-care unit makes the stay an intensive-care stay, and the **ICU** column of the
patient page draws the bed stays in intensive-care units, from `bed_start` to `bed_end` (a visit
without them is drawn whole). Bed stays can be columns of an encounters table (one row per bed stay, with the
encounter repeated) or a table of their own. They are kept in `extras` as `bed_stays` rows.

| Field | Required | Recognized names | Content |
|---|---|---|---|
| `encounter_id` | Yes | as above | The encounter the bed stay belongs to. |
| `care_unit` | Yes, unless `bed`, `bed_start` or `recording_file` is given | careunit, ward, location | The unit (`ICU7`, `MICU`). A unit whose name contains ICU, CCU, CICU or ITU, or the words "intensive" or "critical care", is an intensive-care unit, unless it is a step-down, post-ICU or intermediate-care unit (`ICU step-down`, `SDU`, `Post-ICU`, `IMCU`, "intermediate"). |
| `bed` | No | bed, bedlabel | The bed label. |
| `bed_start` | No | bedstart, intime, transferin | When the patient arrived in the bed. |
| `bed_end` | No | bedend, outtime, transferout | When the patient left it. |
| `recording_file` | No | recordingfile, recording, recordingname | A recording made during the bed stay, by its file name or path. |

db-builder does not link recordings to encounters by bed and time: a recording gets its patient
and encounter from the row or table that names it ([Files and recordings](#files-and-recordings)).

### Results and measurements

Results and measurements are laboratory results, vital signs, scores, ECG measurements and any
other dated value. They and all other dated items are **entries**, shown in the patient page's
journey and report.

| Field | Required | Recognized names | Content |
|---|---|---|---|
| `patient_id`, `mrn`, `encounter_id` or `csn` | At least one | as above | The patient (and encounter) the entry belongs to ([How rows find their patients](#how-rows-find-their-patients)). |
| `name` | Yes (or `value`, `text` or `file`) | name, test, testname, item, parameter, label, drug, medication, description, notetype, type, study | What was measured: `Lactate`, `Heart rate`. |
| `time` | Recommended | time, datetime, charttime, date, timestamp, resulttime, collected, performed, starttime, start | When it was measured or collected. |
| `end_time` | No | endtime, end, stop, stoptime | The end, for anything that lasts. |
| `value` | No | value, result, valuenum, resultvalue, dose, amount, measurement | The result. A number is stored as a number (`value_num`); anything else as text (`value_text`). |
| `unit` | No | unit, units, uom, valueuom | The unit of the value (`mmol/L`). |
| `ref_low`, `ref_high` | No | reflow, low, lower, normallow, rangelow; refhigh, high, upper, normalhigh, rangehigh | The normal range. Must be numbers when given. |
| `flag` | No | flag, abnormal, abnormalflag | The abnormal flag ([Values, units and flags](#values-units-and-flags)). |
| `code`, `vocabulary` | No | code, icd, icd10, cpt, loinc, ndc, rxnorm; vocabulary, codesystem, system | A code and its vocabulary: `2524-7`, `LOINC`. |
| `category` | No | category, kind | The entry's category ([Categories](#categories)). |
| `domain` | No | domain | `measurement`, `drug`, `observation`, `condition`, `procedure` or `note`. Used to choose the category when none is given. |
| `service`, `author` | No | service, department, specialty; author, provider, clinician, physician | The service that produced the entry (in `extras`), and who wrote it. |

A row whose `name`, `value`, `text` and `file` are all empty holds no entry: it only describes
its patient or encounter.

### Medications

Medications use the same columns as results. Give the table the category `med`: with
`--category med`, a `category` column, or a file name such as `medications.csv`.

| Field | Content |
|---|---|
| `name` | The drug (`Norepinephrine`). |
| `time`, `end_time` | The start and end of the administration or infusion. |
| `value`, `unit` | The dose and its unit (`0.05`, `mcg/kg/min`). |
| `code`, `vocabulary` | For example an RxNorm code (`7512`, `RxNorm`). |

### Conditions, procedures and allergies

| Kind | Category | Columns |
|---|---|---|
| Conditions (diagnoses, problems) | `dx` | `name`, `code` and `vocabulary` (`A41.9`, `ICD10CM`), `time` (onset or diagnosis), `end_time` (resolution). Shown in the patient column of the patient page and in the journey. |
| Procedures | `proc` | `name`, `code` and `vocabulary` (`CPT`), `time`, `end_time`. |
| Allergies | `allergy` | `name` (the substance) and `text` (the reaction). Shown with the patient, in the patient bar, not as a journey column. |

### Notes and reports

| Field | Required | Content |
|---|---|---|
| `patient_id`, `mrn`, `encounter_id` or `csn` | At least one | The note's patient and encounter. |
| `text` | Yes | The note's or the report's text. Recognized names: text, note, notetext, report, reporttext, interpretation, comment, comments, body, impression, reaction, finding, findings, conclusion. |
| `time` | Recommended | When it was written. |
| `name` | No | The type of note (`Progress note`, `Discharge summary`). |
| `author`, `service` | No | Who wrote it, and the service. |

Free text may name patients and staff. In Privacy Mode Medorama shows it with the patient's own
names and date of birth replaced, or withholds it, as the administrator chooses
([Privacy](#privacy)).

### Files and recordings

A row with a `file` column brings in a file: a recording, a 12-lead ECG, a DICOM study, a picture,
a video or a document. **Every file a row names becomes a `recordings` row**, with the row's
patient, encounter and category.

| Field | Required | Recognized names | Content |
|---|---|---|---|
| `patient_id` or `mrn` (or `encounter_id`) | Yes | as above | The file's patient. |
| `file` | Yes | file, path, filepath, image, link, filename | The file's path ([Files](#files)). |
| `time` | Recommended | as above | When the file was recorded or acquired (used when its header gives no start). |
| `category` | Recommended | category, kind | Decides whether the file is a recording or an attachment, and which viewer opens it. |
| `name`, `text` | No | as above | The entry's name (and the recording's label; default: the file's name without its extension) and a text such as an interpretation. |
| Measurement columns | No | `QTc (ms)`, `LVEF (%)` | Values that cohorts can search ([Measurement columns](#measurement-columns)). |

An ECG table, such as the one `muse-py index` writes, is a table of this kind: one row per ECG,
with its patient, its time, its measurements as columns, its interpretation and its file.

A **recording** (ICU monitoring, a Holter, an EEG, a sleep study) reaches its patient's page
through the row's patient (or the patient of its `encounter_id`). A recording that names no
patient is kept without a person, and `db-builder check` lists it.

A whole **folder of recordings** is added with `db-builder recordings OUT FOLDER`, with the
patients from a table (`--patients`: a `file` or `recording_file` column and the patient's
columns) or from a pattern of the files' names (`--pattern '{patient}_*.edf'`)
([db-builder/README.md, Folders of recordings](../db-builder/README.md#folders-of-recordings)).

### Measurement columns

A column that is not one of the fields becomes a **measurement column**, a number that cohorts
can search and compare, when its header names a unit or when it is a known ECG measurement:

- **A unit in brackets:** `QT (ms)`, `LVEF (%)`, `Lactate [mmol/L]`. The unit is the text in the
  last pair of round or square brackets, at most 12 characters.
- **A known ECG measurement on its own**, in its usual unit:

  | Header | Unit |
  |---|---|
  | `Vent. rate`, `Ventricular rate`, `Atrial rate`, `Heart rate`, `HR` | bpm |
  | `PR`, `QRS`, `QT`, `QTc`, `QTcB`, `QTcF`, `RR` | ms |
  | `P axis`, `R axis`, `QRS axis`, `T axis` | ° |

Each number of a measurement column becomes one more entry, with the row's patient, encounter,
time and category, the measurement's name and unit. These rules are in `measure_column` and the
list `MEASURES` in [db-builder/src/tables.rs](../db-builder/src/tables.rs). Cohorts compare
values in the unit a criterion names (`QT > 0.4 s`).

### All recognized column names

This table is the complete list (`GROUPS` and `SYNONYMS` in
[db-builder/src/tables.rs](../db-builder/src/tables.rs)).

| Group | Field | Recognized names |
|---|---|---|
| Patient | `patient_id` | patientid, patient, subjectid, subject, pid, personid |
| | `patient_id_issuer` | patientidissuer, issuer, idissuer |
| | `mrn` | mrn, medicalrecordnumber, medrecno, mrno |
| | `mrn_authority` | mrnauthority, authority, assigningauthority |
| | `last_name` | lastname, surname, familyname |
| | `first_name` | firstname, givenname |
| | `birth_date` | birthdate, dob, dateofbirth |
| | `sex` | sex, gender |
| Encounter | `encounter_id` | encounterid, encounter, visitid, visit, admissionid, hadmid, stayid |
| | `csn` | csn, accountnumber, account, fin |
| | `admit_time` | admittime, admit, admissiontime, admitted, admitdate, admissiondate |
| | `discharge_time` | dischargetime, discharge, dischtime, discharged, dischargedate |
| | `admit_unit` | admitunit, admissionunit |
| | `disposition` | disposition, dischargedisposition |
| Location | `care_unit` | careunit, ward, location |
| | `bed` | bed, bedlabel |
| | `bed_start` | bedstart, intime, transferin |
| | `bed_end` | bedend, outtime, transferout |
| | `recording_file` | recordingfile, recording, recordingname |
| Item | `category` | category, kind |
| | `name` | name, test, testname, item, parameter, label, drug, medication, description, notetype, type, study |
| | `time` | time, datetime, charttime, date, timestamp, resulttime, collected, performed, starttime, start |
| | `end_time` | endtime, end, stop, stoptime |
| | `value` | value, result, valuenum, resultvalue, dose, amount, measurement |
| | `unit` | unit, units, uom, valueuom |
| | `ref_low` | reflow, low, lower, normallow, rangelow |
| | `ref_high` | refhigh, high, upper, normalhigh, rangehigh |
| | `flag` | flag, abnormal, abnormalflag |
| | `code` | code, icd, icd10, cpt, loinc, ndc, rxnorm |
| | `vocabulary` | vocabulary, codesystem, system |
| | `text` | text, note, notetext, report, reporttext, interpretation, comment, comments, body, impression, reaction, finding, findings, conclusion |
| | `author` | author, provider, clinician, physician |
| | `service` | service, department, specialty |
| | `domain` | domain |
| File | `file` | file, path, filepath, image, link, filename |
| Other | `source` | source (the row's source label, kept in `extras`) |

## Categories

A **category** is a column of the patient page's journey and a group of its report. Categories
follow the way a physician reads a chart, not the systems the data came from: one table may fill
several categories, and several tables one. An entry's category comes from, in this order:

1. its row's `category` column;
2. the category given for the table (`--category`);
3. the table's file name (`labs.csv`, `medications.xlsx`) or headers;
4. its `domain` and name: drugs are `med`, conditions `dx`, procedures `proc`, notes `note`,
   observations `obs`; a measurement named like a vital sign (`HR`, `SpO2`, `Temperature`,
   `Resp rate`) is `vit`, and any other is `lab`. `RR` alone is not a vital sign: it is also an
   ECG's RR interval, in ms; name the respiratory rate `Resp rate` or `RR (resp)`.

A category is written as its key, its label, or one of its words, in any letter case; db-builder
stores the key. The list is `CATEGORIES` in [db-builder/src/categories.rs](../db-builder/src/categories.rs)
(the same as Medorama's [medorama/src/categories.rs](../medorama/src/categories.rs)).

| Key | Label | Words that also name it |
|---|---|---|
| `icu` | ICU | intensive care, critical care, monitoring, bedside monitoring, waveforms, bedmaster |
| `vit` | Vitals | vital, vitals, vital signs, vital sign, flowsheet |
| `lab` | Labs | labs, laboratory, chemistry, hematology, coagulation, blood gas, blood gases |
| `micro` | Microbiology | microbiology, culture, cultures, sensitivity, sensitivities |
| `path` | Pathology | pathology, biopsy, biopsies, cytology, histology |
| `ecg` | ECG | ekg, 12-lead, 12 lead, electrocardiogram, muse |
| `holter` | Holter | ambulatory ecg, event monitor, patch monitor |
| `eeg` | EEG | electroencephalography, electroencephalogram, neurophysiology, ieeg, seeg |
| `sleep` | Sleep | sleep study, sleep studies, polysomnography, psg |
| `echo` | Echo | echocardiography, echocardiogram, tte, tee |
| `cath` | Cath lab | cath lab, catheterization, cardiac catheterization, angiography, angiogram, coronary angiography, xa, ivus, oct, pci |
| `img` | Imaging | imaging, radiology, ct, mri, x-ray, xray, ultrasound, dicom, pacs |
| `proc` | Procedures | procedure, procedures, surgery, operation, operations, cpt |
| `med` | Medications | meds, medication, medications, drug, drugs, mar, pharmacy |
| `blood` | Transfusions | blood, transfusion, transfusions, blood product, blood products, blood bank |
| `support` | Life support | life support, ventilation, ventilator, respiratory support, dialysis, crrt, ecmo |
| `dx` | Conditions | condition, conditions, diagnosis, diagnoses, problem, problems, problem list, icd |
| `obs` | Assessments | observation, observations, assessment, assessments, score, scores |
| `note` | Notes | note, notes, clinical note, clinical notes, document, documents, report, reports |
| `allergy` | Allergies | allergy, allergies, intolerance, intolerances |

The **recording categories** are `icu`, `holter`, `eeg` and `sleep`. Their files are recordings,
indexed in the background by Medorama and opened in VitalStream, the waveform viewer. Files of
the other categories are attachments, opened by that category's viewer ([Files](#files)).

## Identifiers: patient IDs, MRNs and authorities

Medorama joins databases on exact (issuer, identifier) pairs, and on nothing else
([databases.md, Requirements](databases.md#requirements-on-a-connected-database)). Every row should
therefore name its patient's issuer.

| Identifier | Rule |
|---|---|
| `patient_id` | Names one patient together with its issuer, `patient_id_issuer` (else `--issuer`). Compared exactly, as text. |
| `patient_id_issuer` | Who issued the patient ID: a study (`STUDY-A`), a hospital's system. A patient ID without an issuer is, in Medorama, the database's own: it links no other database's persons. |
| `mrn` | Names a patient only together with its authority. MRNs are compared without surrounding spaces and, within db-builder, ignoring letter case. A patient may have an MRN and a patient ID. |
| `mrn_authority` | The system that issued the MRN (`HOSP`, `CLINIC`), else `--mrn-authority`. Give it whenever MRNs from different systems could collide. |
| `encounter_id` | Names one encounter in the database, compared exactly. An encounter belongs to one patient. |
| `csn` | An encounter's account number. Entries can name their encounter by it. |

`db-builder add` warns about rows without an issuer, and `db-builder check` lists persons whose
patient ID names no issuer or whose MRN names no authority, and identifiers given to two persons
by the same issuer. A person whose patient ID and MRN name two different patients of another
database is an identity conflict in Medorama: it is listed there and linked to nobody, and is
corrected in the inputs.

## How rows find their patients

- **A patient** is found by its patient ID and issuer, else by its MRN and authority; else the
  patient is added. A patient known only by an MRN takes the patient ID of a later row that gives
  both. A person's key is its patient ID when it has one that no other person has as a key.
- **An encounter** is created by a row with an `encounter_id`, for the row's patient. Its `csn`
  is kept with it.
- **An entry** (a result, a note, a file) is matched by the first key it has:
  1. `encounter_id`: that encounter, and its patient;
  2. `csn`: the encounter with that account number;
  3. `patient_id` or `mrn`: the patient, with no encounter.

  An entry that names an encounter no table has named yet keeps the encounter's key, and gets
  its patient when the encounters' table is added.
- **Recordings** get their patient from the row that names them, from their encounter, or, for a
  folder, from the patients table or the file-name pattern. db-builder does not place entries or
  recordings in encounters by time, bed or file name.

## Times and time zones

A db-builder database has one time zone (`db-builder new --timezone`, or the first `add`;
default `America/New_York`). Every time is written as ISO 8601 with its offset.

| Written as | Read as |
|---|---|
| `2024-03-02T07:10:00-05:00`, `2024-03-02T12:10:00Z`, `2024-03-02 12:10:00+00:00` | That moment: the offset or `Z` is used. A space may replace the `T`. |
| `2024-03-02 07:10`, `2024-03-02 07:10:00`, `2024-03-02T07:10`, `03/02/2024 07:10`, `02.03.2024 07:10` | Local time in the database's time zone. Fractions of a second are allowed. |
| `2024-03-02`, `03/02/2024`, `02.03.2024` | Local midnight. |
| `1709381400` | Unix seconds. Only a number above 10^8 is a time; a smaller one is refused ("not a time"). A number of 10^11 or more is refused too: it counts milliseconds (divide it by 1000). |
| An Excel date | As Excel stores it, in local time. |
| A Parquet timestamp | An instant (`isAdjustedToUTC`), or a local time in the database's time zone when the column says it is not adjusted to UTC. |
| A recording header's clock time (WFDB, EDF) | UTC, as the recording engine and the viewers read it, so that a recording's start is the one its index gives. |

- A date with slashes is month first by default, the American way (`03/02/2024` is March 2), and
  day first with `--day-first` (`03/02/2024` is February 3). A date with dots (`02.03.2024`) is
  always day first.
- A year has its four digits: `03/02/24` is refused ("write the year with its four digits"),
  never read as the year 24. A time before 1700 or after 2500 is refused, and a date of birth
  before 1700 or in the future (the range is wide for research datasets whose dates are shifted:
  MIMIC-IV's run past 2200, MIMIC-III's births of patients over 89 lie in the 1800s).
- An offset written as hours alone or without its colon (`-05`, `+0530`, as PostgreSQL writes a
  `timestamp with time zone` as text) is read as `-05:00`, `+05:30`.
- CLIF's times are UTC: a CLIF table's time written without an offset is read as UTC, not in the
  database's time zone.
- A local time that a daylight-saving change skips is read with the offset after the change:
  02:30 on 2024-03-10 in New York is the moment shown as 01:30.
- Set the time zone when the database is made, since it decides how every local time is read
  (`parse_time` in [db-builder/src/util.rs](../db-builder/src/util.rs)).
- A time that cannot be read makes its row invalid; the warning names the line.

## Values, units and flags

- **Values:** a `value` that is a number (`4.1`, `-2`, `1e3`) is stored as a number and can be
  compared in cohorts; any other value (`positive`, `<0.5`) is stored as text.
- **Units:** write the unit as the source system does (`mmol/L`, `mg/dL`, `bpm`). Cohorts
  compare values in the unit a criterion names, converting between units where they can.
- **Normal ranges:** `ref_low` and `ref_high` must be numbers. A value outside them is shown as
  abnormal even without a flag.
- **Flags** are stored as written, and set how Medorama shows a value in the journey and the
  report (`level` in [medorama/src/patient/journey.rs](../medorama/src/patient/journey.rs)):

  | Shown as | Flags (any letter case) |
  |---|---|
  | Critical (orange) | `HH`, `LL`, `C`, `crit`, `critical`, `panic`, `AA`, `!` |
  | Abnormal (amber) | `H`, `L`, `A`, `abn`, `abnormal`, `high`, `low`, `pos`, `positive`, `R`, `resistant`, `detected` |
  | Normal | anything else, unless the value is outside its range |

## Files

- **Where the file is:** a `file` cell holds a path, relative to the table's folder or absolute.
  db-builder stores it relative to the database file's folder when it lies inside that folder,
  else absolute. Files are read where they are, never copied, moved or written.
- **Medorama reads files only inside `data_roots`** (when that setting names folders), and never
  inside its workspace. Keep the database file and its recordings inside the data roots.
- **A missing file** is left out with a warning; the rest of the row is added. `db-builder check`
  lists the recordings whose file is missing later.
- **Recording or attachment:** when the row's or the table's category is a recording category
  (`icu`, `holter`, `eeg`, `sleep`) or names none, a file that is a recording becomes a
  **recording**:
  - a WFDB record (its `.hea` file; a multi-segment record whole), or a folder of them
    ([wfdb-collection.md](wfdb-collection.md));
  - an EDF, EDF+, BDF or BDF+ file ([edf.md](edf.md));
  - a file that an installed **adapter** reads, such as BedMasterEx `.Stp` files with the
    stp-adapter add-on ([adapters.md](adapters.md); the stp-adapter is private and available to
    institutions under agreement).

  A recording without a category goes to the ICU column. Any other file is an **attachment**,
  opened by the category's viewer; it is a `recordings` row too, of the row's category:

  | Category | Files | Opened in |
  |---|---|---|
  | ICU, Holter, EEG, Sleep | WFDB records and folders; EDF, EDF+, BDF, BDF+; `.Stp` with the stp-adapter add-on | VitalStream |
  | ECG | WFDB (`.hea` with its signal files), GE MUSE XML, DICOM ECGs, EDF and BDF, the ECG viewer's JSON form, PNG, JPEG, PDF | the ECG viewer |
  | Echo, Cath lab, Imaging | DICOM files, a DICOM series' folder (files without an extension are recognized), videos, pictures, PDF | the PACS viewer |
  | Any other | pictures, videos and PDF | the entry's detail, on the patient page |

- **A WFDB record is its header:** a row whose file is a signal file (`.dat`, `.mat`) is left
  out, with a warning that names the header to give in its place
  ([wfdb-collection.md](wfdb-collection.md#one-record)).
- **db-builder built without WFDB support** (`--no-default-features`) still lists a WFDB record
  by its header, without its start and end.
- **Privacy Mode:** Medorama's privacy gate sends only files it can de-identify or mark: WFDB
  headers and their `.dat` and `.mat` signal files, GE MUSE XML, DICOM, EDF and BDF files,
  pictures, videos and PDF. It refuses the rest: JSON ECGs, text files, and signal files with an
  extension other than `.dat` or `.mat` (`file_kind` in [medorama/src/gate.rs](../medorama/src/gate.rs)).
- **Tables made from the files themselves:** `dicom-py index FOLDER -o imaging.csv` and
  `muse-py index FOLDER -o ecg.csv` write a table with one row per DICOM series or ECG: its
  patient (in a column `mrn`), its time, its category, its measurements and its file
  ([viewers/README.md](../viewers/README.md)). Their paths are relative to the CSV file's
  folder; `db-builder add` reads them from there.

## Known exports and declared formats

Some systems' exports are recognized by their columns and converted as they are added; such a
conversion is a **format**. `db-builder formats` lists them; `--format KEY` names one, `--format
none` turns recognition off.

| Key | What |
|---|---|
| `ge-muse` | GE MUSE ECG metadata (one row per resting ECG, its XML elements as columns), the patient by MRN. Recognized by its columns. |
| `ge-muse-patient-id` | The same, the patient by a patient ID (named with `--format`). |
| `clif-…` | CLIF's tables (CLIF 2.1, and five tables of 3.0), each recognized by its columns. A CLIF folder given to `add` adds each of its tables, patients and hospitalizations first, and puts each susceptibility on its culture. |
| declared | A site's export, described in a TOML file given with `--formats` (the file, or a folder of them). |

A format keeps what is worth showing and searching, and leaves out what names staff or the
visit's bookkeeping (physicians, technicians, account numbers, rooms); rows it cannot use are
counted, never named.

**A declared format** describes an export column by column; no code and no new build are needed
(the rules: [db-builder/src/formats/declared.rs](../db-builder/src/formats/declared.rs)). This
example converts a laboratory system's nightly export whose columns are `HospNo`, `TestName`,
`TestCode`, `CodeSystem`, `ResultValue`, `ResultUnit`, `CollDate`, `CollTime`, `AbnFlag` and
`Clerk`:

```toml
# formats/acme-labs.toml
key = "acme-labs"                    # lowercase letters, digits and -, starting with a letter
label = "ACME laboratory export"
category = "lab"                     # the category of its entries
about = "One row per result, from the ACME LIS nightly export."
detect = ["HospNo", "TestCode", "ResultValue"]   # recognized when a table has all of these
required = ["mrn", "name"]           # rows with any of these empty are left out (counted)

[[column]]
name = "mrn"                         # a recognized field, or a column name of its own
from = "HospNo"                      # copied, trimmed

[[column]]
name = "name"
from = "TestName"

[[column]]
name = "code"
from = "TestCode"

[[column]]
name = "vocabulary"
from = "CodeSystem"

[[column]]
name = "value"
from = "ResultValue"
number = true                        # kept only when it is a number

[[column]]
name = "unit"
from = "ResultUnit"

[[column]]
name = "time"
from = "CollDate"
date = "%d/%m/%Y"                    # the form of the date (chrono's format), written as ISO 8601
time_from = "CollTime"               # the time of day, from this column
time = "%H%M"

[[column]]
name = "flag"
from = "AbnFlag"
map = { HIGH = "H", LOW = "L" }      # values replaced; any other value becomes empty
```

A column that the description does not name is left out: the `Clerk` column above, which names
staff, never enters the database. A column of numbered parts can be joined with
`join = "Statement/{n}/Text"` (and `separator`) instead of `from`. The following commands list the
format and add a table of that export:

```bash
db-builder formats --formats formats/
db-builder add study.sqlite acme-2024-03-01.csv --formats formats/ --mrn-authority HOSP
```

## Examples

The names and identifiers below are invented. A patients table:

```csv
MRN,Patient ID,Issuer,DOB,Sex
X0000001,S0001,STUDY-A,1950-03-04,F
```

Encounters with their bed stays (one row per bed stay; the encounter repeats):

```csv
Encounter ID,Patient ID,CSN,Admit Time,Discharge Time,Admit Unit,Care Unit,Bed,Bed Start,Bed End
E0002,S0001,9000000102,2023-12-28 07:40,2024-01-09 13:00,Cardiology,ICU7,12,2024-01-01 11:00,2024-01-03 11:00
```

Laboratory results, matched by MRN and encounter:

```csv
MRN,Encounter ID,Test Name,Result,Units,Ref Low,Ref High,Abnormal Flag,Collected,Code,Code System
X0000001,E0002,Lactate,4.1,mmol/L,0.5,2,H,2024-01-02 06:10,2524-7,LOINC
```

An index of ECGs, with files and measurements:

```csv
patient_id,encounter_id,time,QT (ms),QTc (ms),interpretation,file
S0001,E0002,2024-01-02 08:00,410,452,Sinus tachycardia,ecg/ecg_S0001_20240102.hea
```

ICU recordings, by patient and encounter:

```csv
patient_id,encounter_id,file
S0001,E0002,icu/recording.Stp
```

A sleep study, an EDF+ file in the Sleep category:

```csv
patient_id,encounter_id,time,study,category,file
S0004,E0010,2024-03-05 22:30,Polysomnography,sleep,sleep/S0004_psg_20240305.edf
```

`tools/make-demo-data.py` writes a complete synthetic set of such tables and files; the
installers' `--demo` option builds it into a db-builder database with `--issuer DEMO
--mrn-authority DEMO`, and Medorama connects it as `demo`
([administration.md](administration.md)).

## Building and connecting

```bash
db-builder new study.sqlite --timezone Europe/Paris
db-builder add study.sqlite patients.csv encounters.xlsx labs.parquet --issuer STUDY-A --mrn-authority HOSP
db-builder add study.sqlite notes.xlsx --category note
db-builder add study.sqlite export.csv --mapping mapping.toml
db-builder add study.sqlite muse.csv --format ge-muse-patient-id
db-builder add study.sqlite clif_folder/
db-builder recordings study.sqlite /data/icu --patients icu_patients.csv --issuer HOSP
db-builder check study.sqlite
medorama databases connect study --kind dbb --url sqlite:/data/study/study.sqlite
```

| `add` option | Meaning |
|---|---|
| `--mapping FILE` | Map columns by hand: a TOML or JSON file of `field = "header"`, under `[columns]` or at the top level. |
| `--issuer NAME` | The issuer of the patient IDs of rows that name none. |
| `--mrn-authority NAME` | The authority of the MRNs of rows that name none. |
| `--timezone TZ` | The time zone of a new database; for an existing one, it must be the database's. |
| `--category KEY` | The category of the entries of rows that name none. |
| `--format KEY` | `auto` (the default: a known export recognized by its columns), `none`, or a format's key. |
| `--formats PATH` | A TOML file describing a format, or a folder of them (repeatable). |
| `--day-first` | Dates with slashes are day first. |

There is no dry run: build into a scratch file first, read `db-builder info` and `db-builder
check`, then build the real one. Once connected, Medorama notices each new version of the file at
its next check (within a minute) and reads it again.

## Privacy

- The database holds what its inputs hold: identifiers, names and dates of birth when the tables
  give them. In Medorama, users see them only with the `phi` grant on the database and the PHI
  permission, outside Privacy Mode; everyone else sees placeholders and pseudonyms
  ([security.md, Privacy Mode](security.md#7-privacy-mode-and-the-privacy-gate)).
- Export the identifiers you need for linking and nothing more. A study that needs no names or
  dates of birth should not include them: Medorama links by (issuer, identifier) pairs alone.
- Free text (notes, reports, interpretations) may name patients and staff. By default, Privacy
  Mode shows it with the patient's own names and date of birth replaced, and marked; the
  administrator can withhold it.
- Keep the inputs and the database on an encrypted disk, delete exports once built, and never
  commit them to a repository ([PRIVACY.md](../PRIVACY.md)). db-builder's `--encrypt` writes an
  encrypted database, which Medorama connects with its key.

## Checklist

1. The database's time zone is set when it is made, and `data_roots` (in Medorama) covers the
   database file and the files it names.
2. Every row names its patient (`patient_id` or `mrn`) and its issuer (`patient_id_issuer` or
   `--issuer`, `mrn_authority` or `--mrn-authority`), the same as other databases use for the same
   patient.
3. Each table is a UTF-8 CSV file, a Parquet file or a spreadsheet with one header row; times
   are in one of the formats above.
4. File paths are relative to the table's folder, inside the data roots.
5. `db-builder add` reports no unexpected rows left out, and `db-builder check` finds no problem.
6. After connecting, Medorama's Databases tab shows the database online, and no identity
   conflicts.
