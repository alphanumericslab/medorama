# Connected databases: a subject index, a catalog, and queries sent to each database

**Status: built (Medorama 0.3.0, November 2026).** This page is the design note of what Medorama
keeps of a connected database, and what it reads from the database when it is asked. It refines
[databases-only.md](databases-only.md) (Medorama connects databases and imports nothing): 0.2
copied more of each database than it needed, and connecting a large one took hours. It is written
for contributors and reviewers, and for site engineers who connect large databases.

## Contents

- [Summary](#summary)
- [Decisions](#decisions)
- [The standard pattern](#the-standard-pattern)
- [What is kept](#what-is-kept)
- [The subject index](#the-subject-index)
- [The catalog](#the-catalog)
- [Files and indexes](#files-and-indexes)
- [What is read when](#what-is-read-when)
- [The patient list](#the-patient-list)
- [Privacy Mode's dictionary](#privacy-modes-dictionary)
- [Refreshing](#refreshing)
- [Upgrading from 0.2](#upgrading-from-02)
- [A large ECG database](#a-large-ecg-database)
- [Later](#later)

## Summary

Medorama links patients across databases by exact identifiers, and reads their data from each
database when they are asked for. Of each connected database it keeps only:

1. **The subject index**: each person's identifiers (patient ID, MRN, their issuers), names,
   date of birth and sex, linked to a Medorama patient number; and their encounters (key, start,
   end, intensive care or not).
2. **The catalog**: the names and units of the variables it holds (what the cohort builder
   completes and the planner sends criteria by), and the categories of its recordings. Counted
   only when asked.
3. **What Medorama did itself**: the recordings it has opened (their files checked and indexed),
   their indexes, the results of actions, cohorts.

Nothing else: no entry, no count of anyone's data, no list of files. Connecting is one pass over
the persons and one over the visits, plus the catalog: minutes for millions of persons, where 0.2
took hours.

## Decisions

Agreed with the project lead, 2026-11-17:

- **Medorama is not an identity resolver.** It joins databases on exact (issuer, identifier)
  pairs; a person whose pairs name two patients is a conflict, listed and never merged.
- **Kept per database, encrypted with its key:** names, MRN, patient ID, date of birth, sex, and
  each encounter's key, start and end. Nothing more.
- **The catalog holds names and units.** Counts are read when a manager asks (**Count now**);
  a count below 11 is shown as "< 11".
- **Files are read when a patient is opened.** A recording is indexed when first opened, and its
  index kept in the database's workspace. A database that will hold its own index (a future
  `.Stp` database's) is read for it first; until then, or when it has none, Medorama indexes the
  recording and keeps the index.
- **Refreshing reads the subjects and the catalog again** when the database's version changes.
  Reading only what changed is for later.
- **An offline database's patients stay listed**; their data show as offline.

## The standard pattern

This is a federated query architecture, the pattern of the systems that search several
institutions' data without copying them:

| Medorama | The pattern | Where it is done this way |
|---|---|---|
| The subject index: each database's identifiers, linked to one patient number | A patient identifier cross-reference | IHE PIX (Patient Identifier Cross-referencing); i2b2's `patient_mapping`; a hospital's master patient index |
| A patient's page: their data read from every database at once | Federated query | i2b2 SHRINE, TriNetX, PCORnet; FHIR `Patient/$everything` |
| The catalog: what each database holds, by name and unit | A metadata catalog | i2b2's ontology tables; OHDSI's vocabulary and Achilles counts; FHIR's `CapabilityStatement` |
| Criteria written once, sent to each database as SQL | Query translation and push-down | OHDSI ATLAS (Circe, SqlRender); i2b2's query engine |

What Medorama adds is joining a patient across sources, and opening their waveforms, ECGs and
images.

## What is kept

```
workspace/db-0007/                one connected database, encrypted with its data key
├── persons.sqlite                persons: identifiers, issuers, names, birth date, sex, patient
│                                 number, the pairs' hashes; encounters: key, person, start, end,
│                                 ICU; conflicts
├── variables.sqlite              category, name, vocabulary, code, unit, numeric (entries and
│                                 persons once counted); the version read; its recordings'
│                                 categories; when it was counted
├── recordings.sqlite             the recordings Medorama has opened (or indexed when asked):
│                                 key, person, path, kind, size, fingerprint, format, times,
│                                 label, category, present, problem; derived series
├── index.sqlite                  their indexes (index format 3)
└── results-<action>.sqlite, results/   what actions made of them
```

The control store keeps, as in 0.2, patient numbers, their pseudonyms and date draws, and keyed
hashes of the identifier pairs (`patient_keys`), never an identifier.

| 0.2 kept | 0.3 |
|---|---|
| `persons.sqlite`: a summary per person (visits, latest time, entries, categories), two grouped queries over every entry | Gone. The encounters instead; the rest is read for the page shown |
| `variables.sqlite`: entries and persons per variable, one grouped query with `COUNT(DISTINCT)` over every entry | Names and units, from a `variables` view or one grouped query; counts when asked |
| `recordings.sqlite`: every recording the database lists, each file found, typed and fingerprinted | Only those opened, or all of them when a manager asks (`medorama databases index`) |

## The subject index

[medorama/src/persons.rs](../../medorama/src/persons.rs). Connecting and refreshing read every
person of the database (its `persons` view, however many: the `max_rows` bound is for criteria)
and link each to a patient by the hashes of their pairs, as in 0.2. Then every visit (its
`visits` view) is kept as an encounter. Nothing is read of the entries.

What the index answers without the database: who is a patient, in which databases, their
identity (with the `phi` grant), their pseudonym and age, their encounters' count and latest
date. What it cannot answer: anything about their data.

## The catalog

[medorama/src/variables.rs](../../medorama/src/variables.rs).

- **A `variables` view** (optional, kind `views`): one row per variable, with the columns
  `category, name, vocabulary, code, unit, numeric`. When a database gives one, connecting reads
  no entry for the catalog. A database whose variables are its columns (a table of ECG
  measurements) writes it as a list of constants; an OMOP site may write it over Achilles'
  results or its `concept` table.
- **Without one**, one grouped query over the entries: `GROUP BY category, name, vocabulary,
  code, unit` (whether a value is numeric), with no counting.
- **Count now** (`medorama databases count NAME`, `POST /databases/{id}/count`, who manages the
  database): one grouped query with `COUNT(*)` and `COUNT(DISTINCT person_key)`. The counts are
  kept until the catalog is read again; the API, the page and the command show a count from 1
  to 10 as "< 11" ([`variables::shown`](../../medorama/src/variables.rs)).
- **Its recordings' categories** (`SELECT DISTINCT category` over the `recordings` view) are
  kept with it: the Patient Journey's columns and the patient list's filters.

## Files and indexes

[medorama/src/recordings.rs](../../medorama/src/recordings.rs).

- **Connecting reads no recording.** A patient's recordings are read from the database's
  `recordings` view when their page, a job or a cohort's selection asks for them
  (`sync_persons`): their files are checked (present, inside `data_roots`, not in the workspace,
  readable), fingerprinted and kept, and those a reader indexes are queued for indexing.
- **A database offline:** its recordings kept from before are shown.
- **All at once, when asked:** `medorama databases index NAME` (**Index all recordings**) reads
  every recording the database lists, as connecting did in 0.2, and queues the new or changed
  ones. For a database of ICU recordings that should all be indexed ahead of time.
- **The Recordings tab** lists the recordings Medorama keeps: those opened or indexed.
- **A cohort's `has` criterion** asks the database (`SELECT DISTINCT person_key, category` over
  its recordings), not the recordings kept.
- **Indexes inside the database (later):** a recording database may hold its recordings'
  indexes itself. Medorama will read an index from it when it has one, and otherwise index the
  recording and keep the index in the workspace, as now.

## What is read when

| When | Read from the database | Kept |
|---|---|---|
| Connecting, a new version, **Refresh** | Its persons, its visits; the catalog (a `variables` view, else one grouped query); its recordings' categories | The subject index, the catalog |
| Every check (60 s) | Each view answers; the version query (else a count of persons and recordings) | Status |
| The patient list | For the page shown (50 rows): each online database's entries and recordings of those persons, grouped | Nothing |
| A filter on the list (`has ECG`), a cohort's `has` | Each online database: the persons with data of that kind | Nothing |
| A patient's page | Their entries, visits, stays and recordings, from every database granted | The recordings (checked and kept) |
| A cohort, a search | The criteria, sent to each database that may hold them | The cohort's members (its own store) |
| **Count now** | One grouped count over the entries | The counts |
| **Index all recordings** | Every recording it lists | The recordings, then their indexes |

## The patient list

[medorama/src/patient/list.rs](../../medorama/src/patient/list.rs). The list is the subject
index: every patient of the databases the user may view, with their identity (with `phi`), sex,
age, encounters and latest encounter, in any database online or not. It is sorted by the
latest encounter (as shown, in Privacy Mode). For the page shown, each online database is asked
what it holds of those persons (one grouped query over their entries and recordings), which
gives the **Data** and **Entries** columns and a later latest date; a database offline leaves
them out ("—"). A filter on a kind of data asks each online database which persons have it.

## Privacy Mode's dictionary

The gate's dictionary ([medorama/src/gate.rs](../../medorama/src/gate.rs)) names every
identifier Medorama knows: from the subject index, and the paths and names of the recordings it
keeps. Recordings are now kept as patients are opened, so:

- Keeping new recordings moves `data_version.recordings`; the dictionary in use takes the new
  ones without being built again (`Dict::with_new_recordings`).
- A recording whose path or label changed, or that is gone, moves `data_version.identifiers`:
  the dictionary is built again, as when persons change.
- The gate is applied after an answer is made, so the recordings an answer read are in the
  dictionary that clears it.

## Refreshing

- A version query (`version` view; OMOP's `cdm_source`; a db-builder database's
  `meta.version`) decides when a database is read again. Without one, the version is
  `count:<persons>:<recordings>`: entries are no longer counted at each check.
- A new version reads the persons, the visits and the catalog again. The counts are dropped
  (counted again when asked).

## Upgrading from 0.2

Migration 2 of the control store adds `data_version.recordings` and clears every database's
`variables_version`, so that each database is read again at its next check. A workspace's
0.2 tables are replaced when they are first opened: `persons.sqlite`'s summary is dropped, and
`variables.sqlite`'s table, whose counts were required, is made again. The recordings 0.2 kept
stay kept. Nothing needs to be done by hand.

## A large ECG database

A metadata database of a GE MUSE archive (the demo's `demo_muse` is a small one): one row per
ECG, millions of rows, the XML files on a network share. In 0.2, connecting it read the 14-arm
`entries` view twice (the catalog's counts and the per-person summary) and found, typed and
fingerprinted every file: hours. In 0.3:

| Step | What runs |
|---|---|
| Subjects | Its `persons` view, a `GROUP BY patient_id` (an index on `ecg(patient_id)`) |
| Encounters | None: it has no visits |
| Catalog | Its `variables` view: 14 constant rows, no entry read |
| Recordings' categories | `SELECT DISTINCT category` over its `recordings` view: one scan |
| A patient's page | `WHERE patient_id = ?` on each view (the index), and their few XML files |
| `QTc > 450 ms` | The `QTc` arm of the `entries` view: one scan of a column |

## Later

- **Reading only what changed** at a refresh, by a watermark the database states.
- **Indexes inside a recording database**, read before indexing.
- **Progress** of a long refresh, shown in the Databases tab.
