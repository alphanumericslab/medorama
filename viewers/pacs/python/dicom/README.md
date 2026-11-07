# dicom-py: databases of DICOM studies, and the PACS viewer's server

This page describes the Python package `dicom_index_py` and its command, `dicom-py`. It is part
of the [PACS viewer](../../README.md), and is written for data managers who catalog DICOM
studies for [Medorama](../../../../README.md), and for administrators who serve the PACS viewer
from a server. The package builds databases of DICOM studies, converts DICOM ECGs to WFDB
records, and serves the PACS viewer with the folders of the computer it runs on.

- [Commands](#commands)
- [The database](#the-database)
- [serve: the PACS viewer, served from this computer](#serve-the-pacs-viewer-served-from-this-computer)
- [Tests](#tests)

## Commands

This command installs the package from a clone of the repository (editable, so that `serve`
uses the page in the clone):

```bash
pip install -e viewers/pacs/python/dicom
```

| Command | Effect |
|---|---|
| `dicom-py index /path/to/dicom -o imaging.csv` | Writes a database: one row per loop, run, series, report, PDF or ECG. |
| `dicom-py wfdb ECG.dcm out/` | Writes a DICOM ECG (12-lead or Holter) as a WFDB record. `--name` sets the record's name. Each value of the header's comments is on one line; each line of a text annotation is a statement. |
| `dicom-py serve --root /data/echo` | Serves the PACS viewer with the folders under `/data/echo`. |

The database is then imported into Medorama, which links each row to its patient and each file
to its viewer:

```bash
medorama import-table imaging.csv
```

## The database

`dicom-py index` reads every DICOM file under the folder (hidden files and folders passed over).
It reads the headers only, never the pixels, and writes one row per object a viewer shows; a
file it cannot read (a damaged header, sequences nested too deep) is listed on standard error,
and the others are indexed:

| Object | Row |
|---|---|
| An ultrasound loop, an angiography run, any multi-frame file | One per file, with its frames and frame rate. |
| A series of single-frame images (CT, MR, NM) | One per series, linking the series' folder. The row is written only when that folder holds one patient's files; otherwise the series is listed as not linked, and no row is written. |
| A single image (an X-ray) | One per file. |
| A structured report (echo, cath …) | One per file. Each measurement becomes a column `Name (unit)`, for example `Left Ventricular Ejection Fraction (%)`; a repeated measurement keeps its first value, or its mean when the report gives one. The report's text goes into `interpretation`. |
| An encapsulated PDF | One per file. |
| A DICOM ECG (12-lead, Holter) | One per file. Its measurements are named as the ECG viewer names them (`QT (ms)`, `QRS (ms)`, `Vent. rate (bpm)` …), and its statements go into `interpretation`. |

The columns are `mrn` (the patient ID), `time` (the device's clock), `category` and `name`, then
`modality`, `description`, `study_uid`, `series_uid`, `images`, `frames` and `fps`. The
measurement columns follow, then `interpretation` and `file` (the path, relative to the CSV).

| Category | Objects |
|---|---|
| `echo` | Ultrasound (US). |
| `cath` | Angiography, fluoroscopy, IVUS, OCT. |
| `ecg`, `holter` | ECG waveforms. |
| `img` | Everything else. |

A report or a PDF takes its category from the words of its title (echo, cath, ECG).

Medorama never merges the columns of different databases. An echo database's `LVEF (%)` and a
cath database's are distinct, and the cohort builder says which one a criterion uses
([docs/user-guide.md, Cohorts](../../../../docs/user-guide.md#cohorts)).

**Privacy.** The patient ID is written, because it is the key that links each row to its
patient. The database therefore holds PHI (protected health information): keep it with
Medorama's databases, never in the repository. Names and dates of birth are never written.

## serve: the PACS viewer, served from this computer

`dicom-py serve` serves the [PACS viewer](../../README.md)'s page together with the folders
under a root, so that the studies of a server can be read without copying them, for example
through an SSH tunnel to its port. It opens `http://127.0.0.1:8081/browse`, which lists the
folders under the root and the files the viewer reads. Each file is a link that opens it in the
viewer, and **Open the records in this folder** opens a folder's files. They open with `#src=`
(or `#list=`), where the viewer hides its **Server folders…** button, and the browser's Back
button returns to the list. `serve` uses only the Python standard library
([`page_server.py`](dicom_index_py/page_server.py), the same file as `muse-py serve`'s).

Files are sent as they are read, in part for a `Range` request, under
`Content-Security-Policy: sandbox`; other kinds than the viewers read are sent as downloads. A
listing holds at most 5000 entries (`"truncated": true` beyond). The sign-in cookie is signed
with a key of the running server as well, so another server on the same host cannot use it.

| Option | Default | Effect |
|---|---|---|
| `--root DIR` | the home folder | The folder served. Nothing outside it is listed or served, with every symbolic link resolved. |
| `--host`, `--port` | `127.0.0.1`, `8081` | Where it listens. A request must name the server by a loopback name, an IP address or `--host` itself (no DNS rebinding). |
| `--auth-file FILE` | `~/.config/vitalstream/auth.json` | The password file, shared with VitalStream and `muse-py serve`. |
| `--set-password`, `--clear-password` | | Set (or change), or remove, the password, then exit. A sign-in lasts 30 days, until the server stops, or until a browser signs out (every sign-in then ends). After a wrong password, the next try, from any browser, waits 1 s, then 2, 4 … up to 60 s. |
| `--no-auth` | | Runs without asking for the password. |
| `--no-browser` | | Does not open a browser window. |

Installing the package without `-e` copies the page into it ([`setup.py`](setup.py)). Two
environment variables, read at install time, are fixed in the package:
- `VIEWER_ROOT` is the most that `serve` ever serves (`~` is allowed). By default it is the whole
  file system. A `--root` outside it is refused, and one that contains it is narrowed to it.
- `VIEWER_PASSWORD=0` installs the package without password support.

An editable install (`pip install -e`) serves the page from the clone. The variables, when
given, are written into the clone, in a file git ignores; without them, the root is the whole
file system, with password support.

These commands install a package that serves at most `/data/echo`, set a password, and start it
on a server:

```bash
VIEWER_ROOT=/data/echo pip install viewers/pacs/python/dicom
dicom-py serve --set-password && dicom-py serve --root /data/echo --no-browser
```

Deployment, behind an SSH tunnel or a TLS reverse proxy, is described in
[the PACS viewer's README](../../README.md#serve-it-from-a-computer-dicom-py-serve).

## Tests

Run the tests with `pytest viewers/pacs/python/dicom/tests`. `test_index.py` tests the database
and the WFDB conversion, and `test_dicom_serve.py` tests `serve`: its root, the Host check,
files and ranges, and its password. The DICOM files are made in the tests, with invented
patients.

The package uses [pydicom](https://pydicom.github.io/) (MIT license) and NumPy (BSD-3-Clause
license).
