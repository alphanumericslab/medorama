# muse-py: GE MUSE ECGs in Python, and the ECG viewer's server

This page describes the Python package `muse_reader_py` and its command, `muse-py`. It is part
of the [ECG viewer](../../README.md), and is written for data managers who convert or catalog
MUSE ECGs, and for administrators who serve the ECG viewer from a server. The package reads GE
MUSE resting ECG XML files with no dependencies. The viewer reads the same files in the browser
with `muse.js`; both readers give the same result, and both are tested on the same synthetic
file.

- [Commands](#commands)
- [What is read](#what-is-read)
- [The ECG database](#the-ecg-database)
- [serve: the ECG viewer, served from this computer](#serve-the-ecg-viewer-served-from-this-computer)
- [Tests](#tests)

## Commands

This command installs the package from a clone of the repository (editable, so that `serve`
uses the page in the clone):

```bash
pip install -e viewers/ecg/python/muse
```

| Command | Effect |
|---|---|
| `muse-py json ECG.xml > ecg.json` | Writes the ECG viewer's JSON form, with its report (`meta`). |
| `muse-py wfdb ECG.xml out/` | Writes a WFDB record (format 16, 1 µV per unit), with the report in its header comments, each value on one line (line breaks in it become spaces). |
| `muse-py index /path/to/muse -o ecg.csv` | Writes an ECG database: one row per file, to import into Medorama. Hidden files and folders are passed over. |
| `muse-py serve --root /data/ecg` | Serves the ECG viewer with the folders under `/data/ecg`. |

The reader can also be used from Python:

```python
from muse_reader_py import read_muse, patient_key, acquired
ecg = read_muse('ECG.xml')   # {name, fs, leads: [{name, units: 'uV', samples}], meta, median}
```

## What is read

| Part | Read from |
|---|---|
| Encoding | The byte order mark's (UTF-8, UTF-16LE, UTF-16BE), whatever the XML declaration says; else UTF-16 without a mark, or the encoding the declaration names. A declaration of UTF-16 in 8-bit text is read as UTF-8. The same as the ECG viewer reads it. |
| Leads | The rhythm waveform (`Waveform` of type `Rhythm`): base64 little-endian int16 samples, multiplied by `LeadAmplitudeUnitsPerBit`. Leads III, aVR, aVL and aVF are derived from I and II, as the carts derive them. A lead without an amplitude scale, or of samples other than 16-bit (`LeadSampleSize`), is refused. |
| Median beats | The `Median` waveform, read as the leads are (`median`: `fs`, `leads`), with `marks`: the P onset and offset, the QRS onset and offset and the T offset of `RestingECGMeasurements`, as sample numbers of the median beat. They are converted from the measurements' own rate (`ECGSampleBase`, which is 500 Hz even in 250 Hz files; else the original measurements' rate, the one rate at which they give the QRS duration, or the known format's). |
| Sampling rate | `SampleBase` × 10^`SampleExponent` of the rhythm waveform. When that is not a rate (not a positive number, an exponent that is not a whole number or is beyond ±6), the rate is unknown: `fs` is `None` and `rateUnknown` is `True`, the viewer shows the samples by number, and `wfdb` writes no record. It is never guessed. Numbers that are not finite (`NaN`) are not read. |
| Format and checks | The cart and 12SL program, with the rates and lengths, against a list of known GE formats (`FORMATS`, the same in the viewer's `muse.js`): the `Recording` detail names them and says whether the format is known. What does not fit is listed in `notes`, with these codes (`BAD_SAMPLE_BASE`, `NEW_FORMAT`, `RHYTHM_NOT_10_S`, `MEDIAN_NOT_1.2_S`, `QTRGGR_CONTRADICTS_FS` …; the viewer's README, MUSE XML). |
| Details | `TestDemographics`: the acquisition time, device, site, location, reason, status and filters; the patient's age and sex. |
| Measurements | `RestingECGMeasurements`: the ventricular and atrial rates, PR, QRS, QT, QTc, QTcF, the P, R and T axes, the QRS count and the blood pressure; `GlobalRR` as RR in ms (× 4 at 250 Hz). |
| Statements | `OriginalDiagnosis` gives the computer's statements (Marquette 12SL, with its version). `Diagnosis` gives the physician's: *confirmed* (with the overreader and the time of the edit) when the status is `CONFIRMED`, and *edited* when it is not confirmed but differs from the computer's. |

**Never in the outputs:** the patient's name, ID and date of birth, the order (accounts,
admission, physicians' IDs) and other identifiers. `patient_key` reads the patient ID on its
own, so that a database can link each ECG to its patient. The database written by `index`
therefore holds PHI (protected health information): keep it with Medorama's databases, never in
the repository.

## The ECG database

`muse-py index` writes a CSV file with these columns:

| Column | Content |
|---|---|
| `mrn` | The MUSE `PatientDemographics/PatientID`. Medorama links each row to its patient by this column, as an MRN. |
| `time`, `category`, `name` | The acquisition time, `ecg`, and `12-lead ECG`. |
| `value`, `unit` | The ventricular rate. |
| one column per measurement | `PR (ms)`, `QT (ms)`, `QTc (ms)` and the others. |
| `interpretation`, `confirmed` | The 12SL statements, and the confirmed ones. |
| `file` | The file's path, relative to the CSV. |

Imported with `medorama import-table`, each row becomes an ECG of its patient. The ECG opens in
the ECG viewer, and cohorts can search its measurements and statements. Files that cannot be
read are listed on standard error.

## serve: the ECG viewer, served from this computer

`muse-py serve` serves the [ECG viewer](../../README.md)'s page together with the folders under
a root, so that the ECGs of a server can be read without copying them, for example through an
SSH tunnel to its port. It opens `http://127.0.0.1:8080/browse`, which lists the folders and ECG
files under the root; each file is a link that opens it in the viewer (with `#src=`, where the
viewer hides its **Server folders…** button), and the browser's Back button returns to the list.
`serve` uses only the Python standard library
([`page_server.py`](muse_reader_py/page_server.py), the same file as `dicom-py serve`'s).

Files are sent as they are read, in part for a `Range` request, under
`Content-Security-Policy: sandbox`; other kinds than the viewers read are sent as downloads. A
listing holds at most 5000 entries (`"truncated": true` beyond). The sign-in cookie is signed
with a key of the running server as well, so another server on the same host cannot use it.

| Option | Default | Effect |
|---|---|---|
| `--root DIR` | the home folder | The folder served. Nothing outside it is listed or served, with every symbolic link resolved. |
| `--host`, `--port` | `127.0.0.1`, `8080` | Where it listens. A request must name the server by a loopback name, an IP address or `--host` itself (no DNS rebinding). |
| `--auth-file FILE` | `~/.config/vitalstream/auth.json` | The password file, shared with VitalStream and `dicom-py serve`. |
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

These commands install a package that serves at most `/data/ecg`, set a password, and start it
on a server:

```bash
VIEWER_ROOT=/data/ecg pip install viewers/ecg/python/muse
muse-py serve --set-password && muse-py serve --root /data/ecg --no-browser
```

Deployment, behind an SSH tunnel or a TLS reverse proxy, is described in
[the ECG viewer's README](../../README.md#serve-it-from-a-computer-muse-py-serve).

## Tests

Run the tests with `pytest viewers/ecg/python/muse/tests`. `test_muse.py` tests the reader, and
`test_muse_serve.py` tests `serve`: its root, the Host check, files and ranges, and its
password. The test file is `viewers/ecg/tests/fixtures/synthetic_muse.xml`, made by
`make_muse.py`, with an invented
patient and invented signals. `synthetic_muse.json` is this reader's output, against which the
JavaScript reader's tests compare.

The reader was written from the format, independently of GE's GPL-licensed `musexmlex.py`. It
is licensed, like the rest of the repository's software, under the PolyForm Noncommercial
License 1.0.0 ([LICENSE](../../../../LICENSE), [COMMERCIAL.md](../../../../COMMERCIAL.md)).
