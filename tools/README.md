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
| [`install-medorama.sh`](install-medorama.sh), [`install-medorama.ps1`](install-medorama.ps1) | Builds Medorama, **one** VitalStream viewer and the database tools `muse-py` and `dicom-py` into `.venv`. `--viewer cpp` (the default), `rs`, `py` or `none` chooses the viewer (Windows: `-Viewer`); `--no-tools` leaves out the database tools; `--demo` also makes the demo; `--no-wfdb` builds Medorama and the viewer without WFDB support, with no wfdb-x code ([building.md, Without WFDB](../docs/building.md#without-wfdb)). The programs go into `.venv/bin` (Windows: `.venv\Scripts`), and the choice of viewer is written to `medorama.viewer` there, so that Medorama starts that viewer ([administration: the waveform viewer](../docs/administration.md#the-waveform-viewer)). Cargo builds into the clone's `target/` whatever `CARGO_TARGET_DIR` or a `build.target-dir` says, and the installer stops with an error if a program it installs is missing at the end. |
| [`install.sh`](install.sh), [`install.ps1`](install.ps1) | The development install: the Python packages, the Rust workspace and the C++ project, built in this clone. `--py`, `--rs` and `--cpp` choose among them (the Python packages are always installed); `--no-rustup` refuses to install Rust; `--demo` also makes the demo in `.venv/share/medorama/demo` for `medorama init` to load. On Windows, run it with `powershell -ExecutionPolicy Bypass -File tools\install.ps1`; it checks for the Microsoft C++ Build Tools and builds in a Visual Studio developer environment. Both installers install Rust with rustup when cargo is missing (on Windows, `rustup-init.exe` is checked against its published SHA-256 first). |
| [`install-gcc.sh`](install-gcc.sh) | For Linux without administrator rights, when the system compiler is too old for the C++ viewer (GCC 8.5 on RHEL, Rocky or Alma 8): installs GCC 13, CMake, Ninja and Python 3.12 from conda-forge into `~/miniforge3`, in an environment named `vs-build`. The Miniforge installer is checked against its published SHA-256 before it runs. |
| [`fetch-cpp-deps.sh`](fetch-cpp-deps.sh), [`fetch-cpp-deps.ps1`](fetch-cpp-deps.ps1) | For builds without internet access: downloads the C++ dependencies pinned in `viewers/vitalstream/cpp/cmake/deps.cmake` into `viewers/vitalstream/cpp/deps/` and checks their SHA-256 hashes. |

## Generators and checks

| Path | What it does |
|---|---|
| [`make-demo-data.py`](make-demo-data.py) | Writes the demo, a synthetic data set: invented patients, their databases, ICU recordings as WFDB collections, a Holter, an overnight EDF+ sleep study, ECGs, DICOM imaging and echo. It writes to `sample_data/demo` by default. The installers' `--demo` option runs it for `medorama init`, and `medorama demo add DIR` loads it into a catalog as the cohort DEMO ([the demo](../docs/administration.md#the-demo)). `--examples DIR` also gives some demo patients local examples of real ECGs (MUSE XML de-identified on the way; WFDB records, pictures and PDFs copied as they are), and `--dicom DIR` gives the demo patients they fit local DICOM studies from public sample collections, in place of the synthetic CT series, de-identified with pydicom with the pixels kept. Neither folder is in git. The same options give the same files, byte for byte, on every run. |
| [`make-viewer-examples.py`](make-viewer-examples.py) | Writes the example files of the ECG and PACS viewers (`viewers/ecg/examples/`, `viewers/pacs/examples/`) with the demo's generators: a 12-lead WFDB record, a GE MUSE XML ECG, a DICOM 12-lead ECG, an EDF+ ECG, a PDF and a PNG printout; an echo loop, a coronary angiography run and a chest X-ray. Every value is invented. It needs NumPy and wfdb-x. |
| [`gen_timezones.py`](gen_timezones.py) | Generates `viewers/vitalstream/cpp/vitalstream/src/timezones.cpp`, the IANA time-zone names that `vitalstream-cpp` accepts. |
| [`screenshots.py`](screenshots.py) | Takes the documentation's screenshots (`docs/images/screens/`) with headless Chrome, from a Medorama that serves the synthetic demo only, and refuses any other catalog ([refresh the screenshots](#refresh-the-screenshots)). Every request to Chrome has a time limit, so a page that never loads stops the script with an error. It needs only the Python standard library; its tests are in [`tests/`](tests/) (`python3 -m unittest discover -s tools/tests`). |
| [`load-test.py`](load-test.py) | Simulates many users at once on a Medorama server (`--users 20 --seconds 60`): sign-ins, catalog pages, patient pages, database rows and cohort searches without pause. It prints each request's median, 95th-percentile and slowest time, and changes nothing ([performance](../docs/performance.md#many-users-at-once)). It needs only the Python standard library. |
| [`git-hooks/check-files`](git-hooks/check-files) | The privacy check ([PRIVACY.md](../PRIVACY.md#in-this-repository)). It reads the files as git stores them, never the working tree's copies, and refuses: anything in a `sample_data/` folder at any depth; real recording names (`UNIT_BED-<Unix time>`) in a path or in any content, text or binary; files over 5 MB (the user's manual PDF over 20 MB); notebooks with saved outputs or that cannot be read; and, outside the synthetic test data and examples (`viewers/vitalstream/testdata/`, `medorama/tests/fixtures/`, `medorama/fuzz/seeds/`, `viewers/ecg/tests/fixtures/`, `viewers/ecg/examples/`, `viewers/pacs/examples/`), recordings and signal arrays (WFDB, EDF, `.Stp`, NumPy), tables (`.csv`, `.tsv`, `.xlsx`, Parquet), databases and their journals (`.sqlite`, `.db`, `-wal`, `-shm`, `-journal`), SQL files and dumps (except the catalog's schema), archives, and DICOM and GE MUSE XML files, recognised by their content. `check-files` checks what is staged; `--commits A..B` every commit of a range, each against its parent; `--tree REV` every file of a revision. |
| [`git-hooks/pre-commit`](git-hooks/pre-commit), [`git-hooks/pre-merge-commit`](git-hooks/pre-merge-commit) | Run `check-files` on what a commit or a merge would store, and refuse it if anything is refused. Enable them once per clone with `git config core.hooksPath tools/git-hooks`; the installers do this, and warn if the clone already uses other hooks. CI runs the same check on every commit of each pull request (`.github/workflows/checks.yml`). |
| [`release-archive.py`](release-archive.py), [`about.toml`](about.toml), [`about.hbs`](about.hbs) | For the release workflow (`.github/workflows/release.yml`): `release-archive.py` packs a release folder into a `.tar.gz` or `.zip` that is the same, byte for byte, whenever it is made from the same files (sorted entries, the commit's time, no owner); `cargo about` writes, with `about.toml` and `about.hbs`, the licences of the Rust crates compiled into the programs (`THIRD_PARTY_CRATES.txt`). |
| [`tests/`](tests/) | The tests of these tools: the privacy check in a scratch repository (`bash tools/tests/test-check-files.sh`); the screenshot tool, the demo's generator and the release archives (`python3 -B -m unittest discover -s tools/tests`); the website's link script (`node --test tools/tests/`). `make test-tools` runs them all. |

## Refresh the screenshots

The screenshots must come from a catalog that holds the synthetic demo and nothing else, because
every picture is published. The script checks this through the API, signed in as an admin,
before it turns Privacy Mode off: the catalog's only sources are the demo (`DEMO`) and its OMOP
database `demo_omop`, and every patient and recording came from them. Before the first picture
it checks every patient's identity against the demo's invented patients (each MRN starts with
`SYN`). It also reads the demo folder you name with `--demo`: its `demo.json`, which
`make-demo-data.py` writes, must say that the folder holds invented data alone, and every
recording of the catalog must lie in that folder. A demo made with `--examples` or `--dicom`
holds local ECGs and DICOM studies on the demo's own patients, so the script refuses it. If any
check fails, it stops without taking a picture. The following steps make such a catalog and take
the screenshots:

1. Write the demo's files:

   ```bash
   .venv/bin/python tools/make-demo-data.py /path/to/demo
   ```

2. Create an empty catalog in a separate home folder, with the administrator's password read
   from a file:

   ```bash
   echo 'a-demo-password' > pw
   medorama init --home /path/to/demo-catalog --results /path/to/demo-results --password-stdin --no-demo < pw
   ```

3. Load the demo (its tables, its OMOP database `demo_omop` and the cohort DEMO), index its
   recordings and start the server:

   ```bash
   medorama demo add /path/to/demo
   medorama run index --all && medorama work && medorama serve
   ```

4. In another terminal, take the screenshots:

   ```bash
   python3 tools/screenshots.py --url http://127.0.0.1:8090/ --password-file pw --demo /path/to/demo
   ```

`init --home` makes the new catalog the default for later commands. When you are done, point
your commands back at your usual catalog with `medorama use --home DIR`.
