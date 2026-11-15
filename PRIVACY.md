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

### Medorama

Medorama imports nothing: it connects databases (an OMOP instance, a research warehouse read
through SQL views, a SQLite file made by db-builder) and reads them in place, read-only. Those
databases hold the identifiers: medical record numbers (MRNs), names, dates of birth and patient
IDs ([administration](docs/administration.md#rules)).

| Question | Answer |
|---|---|
| Where does Medorama keep identifiers? | Only in its *workspace*, in each connected database's `persons.sqlite`: the identifiers of the persons it linked, which the privacy gate needs to recognize them. Each such file is encrypted (SQLCipher, AES-256) with that database's own key, which is kept only wrapped by a master key held outside the workspace. Medorama's own database, the *control store*, holds no identifier: to join databases it keeps keyed hashes of the identifiers, and patients as numbers and pseudonyms. |
| Who may see them? | Only users with the *PHI permission* who hold the `phi` grant on the database that holds them, and who have switched Privacy Mode off. Their searches and views are written to the audit log. A user who is not granted a database sees nothing of it. |
| What does everyone else see? | Everyone else works in *Privacy Mode*, the default. The server replaces identifiers with placeholders and pseudonyms, moves dates back, and de-identifies WFDB, EDF, GE MUSE XML and DICOM headers before they leave ([protecting patient data](docs/protecting-patient-data.md)). |
| What never receives them? | The viewers (including the VitalStream viewer that Medorama runs), the job descriptions given to actions, and the actions' output folders. They receive pseudonyms only. |
| What can Privacy Mode not clean? | Pictures, PDFs, videos and DICOM pixels are sent as they are, and their tabs are marked with an amber warning triangle, because text burned into them may still name the patient. Free text (notes and reports) is shown by default with the patient's own names and date of birth replaced, and is marked, because it may still name the patient or other people. An administrator can withhold it (**Admin › Privacy Mode**). |
| Are files or databases changed? | No. Connected databases, recordings and files are only read, never written. Every recording opened through the VitalStream tab is written to the audit log. |
| What happens when a database is removed? | Its key is destroyed first, then its workspace folder, then every cohort row it contributed to, its patient links and the patients no other database holds. The database itself is not touched. |
| What else holds identifiers? | Backups of the workspace (readable with the master key), which bring a removed database's files back until they expire; the connected databases and their own backups; and the audit log's search texts, which may name a patient. |

### The viewers and add-ons

- **The VitalStream viewers** (`vitalstream-py`, `vitalstream-rs` and `vitalstream-cpp`) never
  read, keep or show a patient's identity, and listen on 127.0.0.1 only. On shared machines,
  protect them with a password: run any of them with `--set-password`; one password serves all
  three.
- **Adapters** answer `index` and `serve` without identifiers. Only their `identify` command
  returns them, and Medorama does not use it.

### What still identifies a patient

Some data identify a patient even without a name:

- A recording's file name. Bedside-monitor systems name recordings `UNIT_BED-<Unix time>`: a
  unit, a bed and a time.
- Everything derived from a recording: values, times and paths.

Treat the index stores and the actions' output folders like the recordings themselves. Medorama
keeps each database's indexes in that database's workspace folder (`db-NNNN/index.sqlite`),
encrypted with the database's key; the output folders of action plug-ins
(`db-NNNN/results/<action>/…`) are not encrypted, so keep the workspace on an encrypted disk. A
viewer run on its own encrypts its store (`cache_dir/index.sqlite`) only when `index_key_file`
is set.

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
    `viewers/ecg/examples/`, `viewers/pacs/examples/`) and the demo library
    (`tools/demo/library/`): recordings, WFDB and EDF data and signal arrays (the library's
    `.i16` and `.u16` included); tables (`.csv`, `.tsv`, `.xlsx`, Parquet); databases and their journals (`.sqlite`,
    `.db`, `-wal`, `-shm`, `-journal`, since a database may hold identifiers), SQL files and
    dumps (except Medorama's schema, `medorama/src/schema/`); archives (`.zip`, `.gz`, `.7z`,
    `.tar`); and DICOM files and GE MUSE XML ECGs, recognised by their content whatever their
    name (refused in the demo library too);
  - files over 5 MB (the user's manual, `docs/user-manual/medorama-user-manual.pdf`, about
    6 MB, may be up to 20 MB), and notebooks with saved outputs or that cannot be read.

  The pre-commit hook runs it on each commit and the pre-merge-commit hook on each merge (enable
  them with `git config core.hooksPath tools/git-hooks`; the installers do this). CI runs it on
  every commit of each pull request and push, and on every file (`.github/workflows/checks.yml`),
  so a commit made without the hooks is still caught before it is merged. `.gitignore` also
  ignores recordings and data files outside the synthetic test data.
- **Synthetic data only.** Tests, demos and screenshots use synthetic data only:
  `tools/make-demo-data.py` (invented people, with `SYN` MRNs),
  `viewers/vitalstream/testdata/generate.py` and Medorama's test fixtures.
- **The demo library: transformed, checked, nothing else.** `tools/demo/library/` holds short
  pieces of real signals and pixels (heart-beat templates from ECGs, re-sliced and deformed CT
  slices of public collections, radiographs projected from them), transformed so that they
  represent no particular person, from which the demo builds its ECGs and images. A maintainer
  made them with a tool that reads real files and is therefore kept outside this repository, on
  purpose: no code, configuration, list of sources, cache or log that touches real data is ever
  committed. Before writing anything, that tool checked that no library file holds any string
  of its sources' text (names, identifiers, dates, headers, tags, file names), that no rendering
  of a waveform correlates with any 10-second window of a source at 0.9 or more, and that no
  image equals or correlates at 0.9 or more with a source image at the same geometry
  ([tools/demo/README.md](tools/demo/README.md) gives the transformations, the checks and their
  results). The library holds raw 16-bit arrays and a manifest, with no text of any source; every
  identifier, date and text of the demo is invented. Nothing else derived from real data may be
  committed, and any change to the library goes through the same tool and checks.
- **Generic names.** In documentation, examples and tests, call recordings by generic names
  (`recording`) and use made-up times. Never paste values, headers or paths from a real
  recording.
- **Clean notebooks.** Clear the outputs of notebooks run on real data before committing them.

## If patient data was committed

Do not simply delete it in a new commit: it would remain in the history. Stop pushing, and ask
the maintainer to remove it from the history.
