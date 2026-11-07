# Privacy and patient data

Medorama and its viewers process protected health information (PHI): bedside-monitor
recordings, ECGs, images, and the databases that describe patients. This page lists where
patient identifiers can appear in the programs, and the rules that keep this repository free of
them. It is for administrators, reviewers and contributors. How a deployment protects patient
data is explained in [protecting patient data](docs/protecting-patient-data.md) and in the
[security review](docs/security.md).

- [In the programs](#in-the-programs)
- [In this repository](#in-this-repository)
- [If patient data was committed](#if-patient-data-was-committed)

## In the programs

### Medorama's catalog

The *catalog*, Medorama's own database, is the one place that holds identifiers: medical record
numbers (MRNs), names, dates of birth and encounter numbers. An administrator imports them from
the databases that name patients ([administration](docs/administration.md#rules)).

| Question | Answer |
|---|---|
| Where are they stored? | In the identifier tables of the catalog's database. That database is either in the catalog's home folder, outside the repository, readable by its owner only and optionally encrypted; or in a PostgreSQL database that the institution runs, encrypted at rest by its server and reached over verified TLS. |
| Do recordings add identifiers? | Some do. The `identify` action reads the identifiers that a recording's file states, so that reviewers can cross-check them against the databases. The adapter of a licensed format reads them for that format ([adapters](docs/adapters.md)); Medorama reads those of EDF and BDF files from the header's patient and recording fields; WFDB records hold none. They are stored with the other identifiers and nowhere else. |
| What about connected databases? | Connected databases, such as an OMOP instance or a research warehouse, are read in place and read-only. Only the identifiers of their persons who are linked to patients are copied into the catalog. |
| Who sees them? | Only users with the *PHI permission* who have switched Privacy Mode off. Their searches and views are written to the audit log. |
| What does everyone else see? | Everyone else works in *Privacy Mode*, the default. The server replaces identifiers with placeholders and pseudonyms, moves dates back, and de-identifies WFDB, EDF, GE MUSE XML and DICOM headers before they leave ([protecting patient data](docs/protecting-patient-data.md)). |
| What never receives them? | The viewers (including the VitalStream viewer that Medorama runs), the job descriptions given to actions, and the results folders. They receive pseudonyms only. |
| What can Privacy Mode not clean? | Pictures, PDFs, videos and DICOM pixels are sent as they are, and their tabs are marked with an amber warning triangle, because text burned into them may still name the patient. Free text (notes and reports) is shown by default with the patient's own names and date of birth replaced, and is marked, because it may still name the patient or other people. An administrator can withhold it (**Admin › Privacy Mode**). |
| Are files changed? | No. Recordings and files are only read, never written. Every recording opened through the VitalStream tab is written to the audit log. |
| What else holds identifiers? | Backups of the catalog's database. |

### The viewers and add-ons

- **The VitalStream viewers** (`vitalstream-py`, `vitalstream-rs` and `vitalstream-cpp`) never
  read, keep or show a patient's identity, and listen on 127.0.0.1 only. On shared machines,
  protect them with a password: run any of them with `--set-password`; one password serves all
  three.
- **Adapters** answer `index` and `serve` without identifiers. Only `identify` returns them,
  and only Medorama asks for it.

### What still identifies a patient

Some data identify a patient even without a name:

- A recording's file name. Bedside-monitor systems name recordings `UNIT_BED-<Unix time>`: a
  unit, a bed and a time.
- Everything derived from a recording: values, times and paths.

Treat the index folders (Medorama's `index_dir` and the viewers' `cache_dir`) and the results
folders like the recordings themselves.

## In this repository

- **Local data only in `sample_data/`.** The folders `sample_data/` (Medorama) and
  `viewers/vitalstream/sample_data/` (VitalStream) are the only places for data and results in a
  clone. Git ignores everything in them except their README.
- **No licensed material.** Licensed add-ons and vendor material are not part of this
  repository; they are developed in repositories of their own.
- **A privacy check before each commit, and in CI.** `tools/git-hooks/check-files` reads what
  git stores (the staged files, not the working tree's copies) and refuses:
  - anything in a `sample_data/` folder, at any depth;
  - a recording name (`UNIT_BED-<Unix time>`, which dates a stay, also with a space in the bed)
    in a file's path or anywhere in its content, text or binary;
  - outside the synthetic test data and examples (`viewers/vitalstream/testdata/`,
    `medorama/tests/fixtures/`, `medorama/fuzz/seeds/`, `viewers/ecg/tests/fixtures/`,
    `viewers/ecg/examples/`, `viewers/pacs/examples/`): recordings, WFDB and EDF data and signal
    arrays; tables (`.csv`, `.tsv`, `.xlsx`, Parquet); databases and their journals (`.sqlite`,
    `.db`, `-wal`, `-shm`, `-journal`, since a catalog holds identifiers), SQL files and dumps
    (except the catalog's schema, `medorama/src/schema/`); archives (`.zip`, `.gz`, `.7z`,
    `.tar`); and DICOM files and GE MUSE XML ECGs, recognised by their content whatever their
    name;
  - files over 5 MB (the user's manual, `docs/user-manual/medorama-user-manual.pdf`, over 20 MB), and notebooks with saved outputs or that cannot be read.

  The pre-commit hook runs it on each commit and the pre-merge-commit hook on each merge (enable
  them with `git config core.hooksPath tools/git-hooks`; the installers do this). CI runs it on
  every commit of each pull request and push, and on every file (`.github/workflows/checks.yml`),
  so a commit made without the hooks is still caught before it is merged. `.gitignore` also
  ignores recordings and data files outside the synthetic test data.
- **Synthetic data only.** Tests, demos and screenshots use synthetic data only:
  `tools/make-demo-data.py` (invented people, with `SYN` MRNs),
  `viewers/vitalstream/testdata/generate.py` and Medorama's test fixtures.
- **Generic names.** In documentation, examples and tests, call recordings by generic names
  (`recording`) and use made-up times. Never paste values, headers or paths from a real
  recording.
- **Clean notebooks.** Clear the outputs of notebooks run on real data before committing them.

## If patient data was committed

Do not simply delete it in a new commit: it would remain in the history. Stop pushing, and ask
the maintainer to remove it from the history.
