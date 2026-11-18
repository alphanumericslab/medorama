# Tools

This folder holds the repository's tooling: the installers, the generators of synthetic data,
the screenshot tool and the git hooks. It is for administrators who install Medorama and for
contributors. None of these files is part of an installed package. The
[building](../docs/building.md) page covers installation in every situation, and the
[`Makefile`](../Makefile) at the repository root wraps these tools and the tests (`make help`
lists its targets).

## Installers

| Path | What it does |
|---|---|
| [`install-medorama.sh`](install-medorama.sh), [`install-medorama.ps1`](install-medorama.ps1) | Builds Medorama, [db-builder](../db-builder/README.md), **one** VitalStream viewer and the database tools `muse-py` and `dicom-py` into `.venv`. `--viewer cpp` (the default), `rs`, `py` or `none` chooses the viewer (Windows: `-Viewer`); `--no-tools` leaves out the database tools; `--demo` also makes the demo (`make-demo-data.py --db-builder`): its files, its six databases built (db-builder's, the second hospital's, the encrypted one, OMOP, the views registry, the MUSE ECG archive), in `.venv/share/medorama/demo`, named in `medorama.demo` beside the programs for `medorama init --demo` to connect; `--no-wfdb` builds Medorama, db-builder and the viewer without WFDB support, with no wfdb-x code ([building.md, Without WFDB](../docs/building.md#without-wfdb)). The programs go into `.venv/bin` (Windows: `.venv\Scripts`), and the choice of viewer is written to `medorama.viewer` there, so that Medorama starts that viewer ([administration: the waveform viewer](../docs/administration.md#the-waveform-viewer)). Cargo builds into the clone's `target/` whatever `CARGO_TARGET_DIR` or a `build.target-dir` says, and the installer stops with an error if a program it installs is missing at the end. |
| [`install.sh`](install.sh), [`install.ps1`](install.ps1) | The development install: the Python packages, the Rust workspace and the C++ project, built in this clone. `--py`, `--rs` and `--cpp` choose among them (the Python packages are always installed); `--no-rustup` refuses to install Rust; `--demo` also makes the demo in `.venv/share/medorama/demo` (with db-builder, when the Rust workspace is built) for `medorama init --demo` to connect. On Windows, run it with `powershell -ExecutionPolicy Bypass -File tools\install.ps1`; it checks for the Microsoft C++ Build Tools and builds in a Visual Studio developer environment. Both installers install Rust with rustup when cargo is missing (on Windows, `rustup-init.exe` is checked against its published SHA-256 first). |
| [`preflight.sh`](preflight.sh) | What building Medorama needs, checked by `install-medorama.sh` and `install.sh` before they build anything: Python 3.9 or later (`$PYTHON`, else `python3`), and for a Rust build `make` and a Perl with `IPC::Cmd` (OpenSSL's build; not for `install.sh --py` or `--cpp`). Every problem is told at once, with what to do with administrator rights and without (a Python from uv, a relocatable Perl: [docs/install.md](../docs/install.md#linux-without-administrator-rights-an-old-python-a-minimal-perl)); then the installer stops. |
| [`license-notice.txt`](license-notice.txt), [`license-accept.sh`](license-accept.sh), [`license-accept.ps1`](license-accept.ps1) | The license terms every installer shows, and asks the user to accept, before it builds anything (y or yes; `--accept-license`, `-AcceptLicense` or `MEDORAMA_ACCEPT_LICENSE=yes` for scripts), with the acceptance recorded in `~/.config/medorama/license-accepted` (Windows: `%APPDATA%\medorama\license-accepted`); `medorama init` shows the same terms ([docs/building.md, License terms](../docs/building.md#license-terms); tests: `tests/test_license_accept.py`). |
| [`install-gcc.sh`](install-gcc.sh) | For Linux without administrator rights, when the system compiler is too old for the C++ viewer (GCC 8.5 on RHEL, Rocky or Alma 8): installs GCC 13, CMake, Ninja and Python 3.12 from conda-forge into `~/miniforge3`, in an environment named `vs-build`. The Miniforge installer is checked against its published SHA-256 before it runs. |
| [`fetch-cpp-deps.sh`](fetch-cpp-deps.sh), [`fetch-cpp-deps.ps1`](fetch-cpp-deps.ps1) | For builds without internet access: downloads the C++ dependencies pinned in `viewers/vitalstream/cpp/cmake/deps.cmake` into `viewers/vitalstream/cpp/deps/` and checks their SHA-256 hashes. |

## Generators and checks

| Path | What it does |
|---|---|
| [`make-demo-data.py`](make-demo-data.py) | Writes the demo: about 40 invented patients in six databases, with recordings, ECGs and images of every kind, enough to try every feature ([the demo](#the-demo), and [its features](#the-demos-features)). It writes to `sample_data/demo` by default; `--db-builder PATH` also builds its db-builder databases; `--small` writes the first six patients and two databases only; `--no-library` uses models alone; `--seed N` another demo (the same seed, the same files, byte for byte). `--examples DIR` and `--dicom DIR` also give some of the first patients local examples of real ECGs and DICOM studies, de-identified on the way (never in git, never published). It needs NumPy and wfdb-x. |
| [`make-viewer-examples.py`](make-viewer-examples.py) | Writes the example files of the ECG and PACS viewers (`viewers/ecg/examples/`, `viewers/pacs/examples/`) with the demo's generators: a 12-lead WFDB record, a GE MUSE XML ECG, a DICOM 12-lead ECG, an EDF+ ECG, a PDF and a PNG printout; an echo loop, a coronary angiography run and a chest X-ray. Every value is invented. It needs NumPy and wfdb-x. |
| [`gen_timezones.py`](gen_timezones.py) | Generates `viewers/vitalstream/cpp/vitalstream/src/timezones.cpp`, the IANA time-zone names that `vitalstream-cpp` accepts. |
| [`screenshots.py`](screenshots.py) | Takes the documentation's screenshots (`docs/images/screens/`) with headless Chrome, from a Medorama that is connected to the demo's databases only, and refuses any other ([refresh the screenshots](#refresh-the-screenshots)). Every request to Chrome has a time limit, so a page that never loads stops the script with an error. It needs only the Python standard library; its tests are in [`tests/`](tests/) (`python3 -m unittest discover -s tools/tests`). |
| [`load-test.py`](load-test.py) | Simulates many users at once on a Medorama server (`--users 20 --seconds 60`): sign-ins, Patient Catalog pages, patient pages, databases' variables and cohort searches without pause. It prints each request's median, 95th-percentile and slowest time, and changes nothing ([performance](../docs/performance.md#many-users-at-once)). It needs only the Python standard library. |
| [`git-hooks/check-files`](git-hooks/check-files) | The privacy check ([PRIVACY.md](../PRIVACY.md#in-this-repository)). It reads the files as git stores them, never the working tree's copies, and refuses: anything in a `sample_data/` folder at any depth; real recording names (`UNIT_BED-<Unix time>`) in a path or in any content, text or binary; files over 5 MB (the user's manual PDF over 20 MB); notebooks with saved outputs or that cannot be read; and, outside the synthetic test data and examples (`viewers/vitalstream/testdata/`, `medorama/tests/fixtures/`, `medorama/fuzz/seeds/`, `viewers/ecg/tests/fixtures/`, `viewers/ecg/examples/`, `viewers/pacs/examples/`) and the demo library (`demo/library/`), recordings and signal arrays (WFDB, EDF, `.Stp`, NumPy, the library's `.i16` and `.u16`), tables (`.csv`, `.tsv`, `.xlsx`, Parquet), databases and their journals (`.sqlite`, `.db`, `-wal`, `-shm`, `-journal`), SQL files and dumps (except the control store's schema), archives, and DICOM and GE MUSE XML files, recognised by their content (refused in the demo library too). `check-files` checks what is staged; `--commits A..B` every commit of a range, each against its parent; `--tree REV` every file of a revision. |
| [`git-hooks/pre-commit`](git-hooks/pre-commit), [`git-hooks/pre-merge-commit`](git-hooks/pre-merge-commit) | Run `check-files` on what a commit or a merge would store, and refuse it if anything is refused. Enable them once per clone with `git config core.hooksPath tools/git-hooks`; the installers do this, and warn if the clone already uses other hooks. CI runs the same check on every commit of each push (`.github/workflows/checks.yml`). |
| [`release-archive.py`](release-archive.py), [`about.toml`](about.toml), [`about.hbs`](about.hbs) | For the release workflow (`.github/workflows/release.yml`): `release-archive.py` packs a release folder into a `.tar.gz` or `.zip` that is the same, byte for byte, whenever it is made from the same files (sorted entries, the commit's time, no owner); `cargo about` writes, with `about.toml` and `about.hbs`, the licences of the Rust crates compiled into the programs (`THIRD_PARTY_CRATES.txt`). |
| [`tests/`](tests/) | The tests of these tools: the privacy check in a scratch repository (`bash tools/tests/test-check-files.sh`); the screenshot tool, the demo's generator and the release archives (`python3 -B -m unittest discover -s tools/tests`); the website's link script (`node --test tools/tests/`). `make test-tools` runs them all. |

## The demo

The demo is what `make-demo-data.py` makes for trying, showing and teaching Medorama: about 40
invented patients, as six databases of three hospitals, a registry and an ECG archive would hold them, with
results, notes, stays and files of every kind ([administration.md, The
demo](../docs/administration.md#the-demo) says how to install it and what to try).

- **Everyone in it is invented.** Names are pseudo names (Jane Doe and others made up from lists
  of common names), MRNs start with `SYN`, patient IDs are made up, dates of birth and every
  date lie in an invented period; the notes name the patient and give the date of birth, so that
  Privacy Mode has something to replace.
- **Its signals and pictures come from two places.** Models (the vital signs and their trends,
  respiration, the sleep and EEG signals, echo, angiography, synthetic radiographs and CT) and
  the **demo library** in [`demo/library/`](demo/): short pieces of real ECGs and of public CT
  collections, transformed so that they represent no one and checked against re-identification
  ([demo/README.md](demo/README.md)). The 12-lead, ICU and Holter ECGs are made of the library's
  beat templates, re-timed to new heart rates; the arterial pressure and pleth follow models
  whose ranges were fitted to real recordings; the CT series and two of the chest radiographs
  are the library's. The library may be published, so a demo made with it can be shown in
  published screenshots; one made with `--examples` or `--dicom` cannot.
- **Its databases** (`demo-databases.json` lists them, with the db-builder steps that build them;
  `medorama init --demo` makes a demo installation and connects every one it lists, with its key
  when it has one, so a new demo database needs no change in Medorama). Each is marked a demo
  database: the db-builder ones are made with `db-builder new --demo`, and the OMOP, registry and
  MUSE databases it writes itself get the `medorama_demo` table, so that only a demo installation
  connects them ([docs/design/demo.md](../docs/design/demo.md)):

  | Name | Kind | What it shows |
  |---|---|---|
  | `demo` | dbb | The main hospital: 40 patients, their visits, ICU and bed stays, every category of entry, every kind of file; folders of recordings added by a file-name pattern. |
  | `demo_north` | dbb | A second hospital: some of the same patients by the same MRN authority (one MRN in lower case, linked ignoring case), one identity conflict, patient IDs normalised by rewrite rules, a folder moved after the build and repointed with `db-builder rewrite`. |
  | `demo_omop` | omop | A third hospital's OMOP CDM database. |
  | `demo_registry` | views | A heart-failure registry's own tables, read through canonical SQL views (scores, NT-proBNP, LVEF). |
  | `demo_muse` | views | An ECG archive: older MUSE XML files of eight patients and an ECG metadata database of them (one row per file by MD5), read through its views; the patient ID read as the MRN. One ECG at 250 Hz, one without its sampling rate. |
  | `demo_secure` | dbb | A transplant clinic's database, encrypted with SQLCipher; its demo key in `keys/demo_secure.key`. |

- **It shows every feature, and must go on doing so.** Each feature of Medorama the demo shows
  has one entry in `FEATURES` in `make-demo-data.py`: a key, what it is, where the demo shows it,
  and a check that the demo made holds it. The table below lists the same keys, and
  `tests/test_make_demo_data.py` checks that the two agree and that every check passes. **Every
  new feature of Medorama extends the demo**: add the data it needs (a story in `STORIES`, a
  table, a recording, a database in `manifest()`), its entry in `FEATURES`, and its row here.

## The demo's features

| Key | Feature | Where in the demo |
|---|---|---|
| `patients` | About 40 invented patients (pseudo names, SYN MRNs, shifted dates of birth) | `databases/patients.csv` |
| `age-90` | A patient over 89 (shown as 90+ in Privacy Mode) | `databases/patients.csv` |
| `visits` | Several visits per patient: admissions, clinic visits, ICU and bed stays | `databases/encounters.xlsx` |
| `labs` | Laboratory results with reference ranges and flags (LOINC) | `databases/labs.csv` |
| `vitals` | Charted vital signs | `databases/vitals.csv` |
| `micro` | Microbiology cultures | `databases/microbiology.csv` |
| `meds` | Medications with start and stop times | `databases/medications.csv` |
| `blood` | Transfusions | `databases/transfusions.csv` |
| `support` | Life support: ventilation, dialysis | `databases/life_support.csv` |
| `procedures` | Procedures | `databases/procedures.csv` |
| `diagnoses` | Diagnoses with ICD-10 codes | `databases/diagnoses.csv` |
| `observations` | Assessments and scores (GCS, SOFA, pain, NYHA) | `databases/assessments.csv` |
| `pathology` | Pathology reports | `databases/pathology.csv` |
| `allergies` | Allergies | `databases/allergies.csv` |
| `notes-privacy` | Notes that name the patient and give the date of birth (replaced in Privacy Mode) | `databases/notes.csv` |
| `reports` | Imaging, echo and cath reports with measurement columns | `databases/imaging_index.csv, echo_index.csv, cath_index.csv` |
| `icu-collection` | ICU recordings: WFDB collections with vital signs, alarms, markers | `icu/` |
| `icu-gaps` | A break in a recording, and a stretch without stated time | `icu/*/gaps.csv` |
| `holter` | Holter ECGs (heart rate derived by the index action) | `holter/` |
| `sleep-edf` | Sleep studies as EDF+ with scoring annotations | `sleep/, sleeplab/` |
| `eeg-bdf` | Routine EEGs as BDF+ (24-bit) | `eeglab/` |
| `recordings-pattern` | Folders of recordings added by a file-name pattern (db-builder recordings) | `sleeplab/, eeglab/` |
| `ecg-formats` | 12-lead ECGs as WFDB, MUSE XML, DICOM, PDF and PNG | `ecg/` |
| `echo` | Echo loops (DICOM cine, PNG frames) and structured reports | `echo/` |
| `ct` | CT series | `imaging/ct_*` |
| `xray` | Chest radiographs | `imaging/cxr_*` |
| `cath` | Coronary angiography runs | `cath/` |
| `library` | Real signals and pixels, transformed (the demo library), when present | `demo.json` |
| `second-hospital` | A second hospital sharing patients by MRN authority | `north/` |
| `mrn-case` | An MRN differing only in letter case, linked ignoring case | `north/patients.csv` |
| `identity-conflict` | An identity conflict (an MRN of one patient, the ID of another) | `north/patients.csv` |
| `rules` | Rewrite rules: identifiers normalised (PT_123 to PT123), paths rewritten | `north/rules.toml` |
| `rewrite` | A moved folder, repointed with db-builder rewrite | `north/moved.toml` |
| `omop` | An OMOP CDM database | `omop/omop.sqlite` |
| `views` | A SQL database connected through canonical views | `registry/` |
| `muse-db` | A MUSE ECG archive and its ECG metadata database, connected through views: median beats with onsets, a 250 Hz ECG and one without its sampling rate | `muse/` |
| `catalog` | A catalog view: what demo_muse holds, named without reading its entries | `muse/views.toml` |
| `demo-marked` | Every demo database marked as invented: only a demo installation connects it | `demo-databases.json` |
| `person-dates` | Each MUSE patient's first and latest ECG, from the persons view (the list's dates without visits) | `muse/views.toml` |
| `has-view` | A has view: who has an ECG in demo_muse, read for `has ecg` without its measurements | `muse/views.toml` |
| `search-indexes` | An index per searched MUSE measurement (value, patient, time): a threshold read from the index | `muse/ecg_metadata.sqlite` |
| `muse-only` | A patient known to the ECG archive alone: the patient page without its left column | `muse/ecg_metadata.sqlite` |
| `muse-ids` | A test ECG whose patient ID is a word, left out by the views | `muse/ecg_metadata.sqlite` |
| `ecg-compare` | Patients with two ECGs or more: each ECG compared with the previous one | `muse/ecg_metadata.sqlite` |
| `ecg-median` | MUSE ECGs with their median beats and P, QRS and T onsets | `ecg/*.xml` |
| `encrypted` | An encrypted db-builder database, its key in a file | `secure/, keys/` |
| `actions` | The example action plug-in, for the Actions tab | `actions/` |
| `cohorts` | Enough patients and values for cohort criteria (creatinine, lactate, troponin, ICU, sex, age) | `databases/labs.csv` |

## Refresh the screenshots

The screenshots must come from a Medorama connected to the demo and nothing else, because every
picture is published. The script checks this through the API, signed in as an admin, before it
turns Privacy Mode off: the databases connected are the demo's (`demo`, `demo_north`,
`demo_omop`, `demo_registry`, `demo_muse`, `demo_secure`, or the small demo's first two), the admin manages
them, their files lie in the demo's folder, and there are no more patients than the demo's
persons. Before the first picture it checks every patient's identity against the persons
`make-demo-data.py` invents (each MRN starts with `SYN`, in any letter case; names and date of
birth as invented). It also reads the demo folder you name with `--demo`: its `demo.json`, which
`make-demo-data.py` writes, must say that the folder holds invented data and the demo library
alone, and every recording must lie in that folder. A demo made with `--examples` or `--dicom`
holds local ECGs and DICOM studies on the demo's own patients, so the script refuses it. If any
check fails, it stops without taking a picture. The pictures include the Databases tab
(`databases.png`) and its **Connect a database** form (`connect.png`). The following steps set up
such a Medorama and take the screenshots:

1. Write the demo and build its databases:

   ```bash
   .venv/bin/python tools/make-demo-data.py /path/to/demo --db-builder .venv/bin/db-builder
   ```

   (The installers' `--demo` does this step in `.venv/share/medorama/demo`.)

2. Set up a Medorama of its own in a separate home folder and workspace, with the
   administrator's password read from a file, connecting the demo's databases: `init --demo`
   connects those of the folder named in `medorama.demo` beside the program (the installer writes
   it); otherwise connect each database of `demo-databases.json` with `medorama databases
   connect` ([administration.md, The demo](../docs/administration.md#the-demo)):

   ```bash
   echo 'a-demo-password' > pw
   medorama init --home /path/to/demo-home --workspace /path/to/demo-workspace --demo --password-stdin < pw
   ```

3. Index its recordings and start the server:

   ```bash
   H=/path/to/demo-home
   medorama --home $H run index --all && medorama --home $H work && medorama --home $H serve
   ```

   (`init --demo` is never remembered as the default home, so every command names it with
   `--home`.)

4. In another terminal, take the screenshots:

   ```bash
   python3 tools/screenshots.py --url http://127.0.0.1:8095/ --password-file pw --demo /path/to/demo
   ```

`init --home` makes the new home the default for later commands. When you are done, point your
commands back at your usual home with `medorama use --home DIR`.
