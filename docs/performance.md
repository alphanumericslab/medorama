# Performance

This page reports how Medorama behaves as the data grow, as measured on synthetic data, and how
fast the three VitalStream viewers index and read recordings, so that an institution can size a
deployment and a contributor can see where the time goes. It is for administrators and
contributors. The design behind these numbers is in
[design/sources.md](design/sources.md) and [architecture.md](architecture.md).

## Contents

- [What a criterion reads](#what-a-criterion-reads): [Answers kept](#answers-kept) ·
  [Reads in parallel](#reads-in-parallel) · [The subject index in memory](#the-subject-index-in-memory)
- [A MUSE-shaped database](#a-muse-shaped-database)
- [The measurement](#the-measurement): [Pages](#pages) · [Query counts](#query-counts)
- [Many users at once](#many-users-at-once)
- [The VitalStream viewers](#the-vitalstream-viewers): [Recordings tested](#recordings-tested) ·
  [Indexing](#indexing) · [Reading](#reading) · [STP files](#stp-files) ·
  [What the numbers show](#what-the-numbers-show) · [Repeating it](#repeating-it)
- [Index format 3](#index-format-3)
- [How each part grows](#how-each-part-grows)
- [Repeating the measurement](#repeating-the-measurement)

## What a criterion reads

Medorama reads a connected database in place: each cohort criterion is sent as SQL to each
database of the search that may hold it ([design/connected-databases.md, Searching a
database](design/connected-databases.md#searching-a-database)). What it asks for decides how
much the database reads, and which of its indexes help
([databases.md, Indexes for searches](databases.md#indexes-for-searches)).

| Criterion | What the database is asked for | What Medorama tests on each row read |
|---|---|---|
| A threshold on a measurement (`QTc > 450`, `lactate 2..4 mmol/L`) | Its names, as the database's catalog spells them (`name IN (…)`), and per unit of the catalog the values that may pass, the threshold converted into that unit (`COALESCE(unit, '') = 'ms' AND value_num >= CAST(? AS REAL)`), one unit at a time; the criterion's dates (SQLite, below). Only `person_key, time, name, value_num, unit`. | The value, in the criterion's unit; the dates. |
| A measurement without a threshold, with `latest`, or with `!=` | Its names; every value in the criterion's dates (SQLite, below). | `latest`: each person's latest value; the dates. |
| A value written without a unit, where the data hold several units | Its names; every value. | Which unit is meant: "write the unit" is asked, as before. |
| `has ecg` (and the Patient Catalog's filter by kind of data) | The database's `has` view, when it has one; else the persons with entries of that category and recordings of it (`SELECT DISTINCT person_key`). Visits for `enc` and `icu`. | Nothing. |
| A diagnosis, a drug, a text | The entries of the category with the code's prefix, a word of the name, or the text; at most `max_rows`. | The dates. |

- **The same patients as testing every row.** The bounds sent are a few billionths wider than
  the criterion, and Medorama still tests every row it reads, so a threshold finds what reading
  every value would. Values of a unit the criterion's does not compare with are still read, and
  counted in the result's "left out" warning.
- **Only from a catalog of the data's version.** A threshold is sent only when the database's
  catalog (the names and units of its variables) was read at its current data version (the
  `version` view). Otherwise the criterion is read as written, every value of its name.
- **A `variables` view's units** may not be all those of the entries: the rows of other units
  are read too, in a query of their own, and tested.
- **A measurement's dates** (`from`, `until`) are sent to a SQLite database as days in the
  database's time zone, two days wider on each side (a time with an offset, the dates as shown in
  Privacy Mode). Only times written as ISO 8601 text (`YYYY-MM-DD…`) are compared there; a time
  written as a number, or as text of another form, is read whatever its date. Medorama tests the
  dates of every row read exactly, so the patients found are the same. PostgreSQL databases are
  asked without the dates (their time columns' types differ), and every value is read.
- **A database that cannot compare a bound** (a PostgreSQL view whose `value_num` is text) is
  asked again without the bounds, and the values are compared in Medorama.
- **Reasons for the page shown.** A search's reasons ("QTc 478 ms on 2024-01-01") are kept as
  values and written out only for the rows of the page shown, not for each of the hundreds of
  thousands of patients a search may find.
- **SQL injection.** The units written into the SQL are literals taken from the database's own
  catalog, never from what a user types; the values are bound parameters; the text of a
  criterion never becomes SQL.

### Answers kept

Each criterion's read (a measurement's numbers, the persons with a category, the entries a
filter finds) is kept in memory, keyed by the database, its time zone, the read and the data's
version (the `version` view's value). A search repeated, or a cohort edited one criterion at a
time, reads only what changed; within one search, each read is made once.

- A new version reads again. A database without a `version` view is read every time.
- At most 4,000,000 rows are kept in all, the least recently used dropped first; a larger
  answer is not kept.
- What is kept is the database's own answer, the same for every user: linking, Privacy Mode,
  grants and the dates as shown are applied afterwards, by each search.
- **The `version` view must change whenever the data change.** If the data change and the
  version does not, Medorama answers from what it kept (cohorts already rely on it).

### Reads in parallel

A search reads all its criteria in all the databases of its scope at once, 4 at a time, before
it combines them: it waits for its longest read, not for their sum. A read that failed is made
again in its turn, so the search's error is as before.

### The subject index in memory

Each database's persons, as kept in its subject index (`persons.sqlite`), are read once and
shared, at most 5,000,000 persons in all, and read again when Medorama writes the index or its
file changes (another process's refresh). A search that links more than 5,000 persons links
them from it, not with thousands of queries; its result page reads only the patients it
matched, and its evaluation starts from the matches, not from every patient.

## A MUSE-shaped database

`QTc > 450` and `has ecg` were timed on a synthetic SQLite copy shaped like a MUSE ECG metadata
database ([databases.md, A MUSE ECG metadata database](databases.md#a-muse-ecg-metadata-database)):
2.5 million invented ECGs, connected through views, on a laptop (Apple silicon), a release build.
Seconds per search:

| Search | Before 0.3.2 | 0.3.2: first | 0.3.2: repeated | 0.3.3: first | 0.3.3: repeated |
|---|---|---|---|---|---|
| `QTc > 450` | 15.6 | 4.7 | 3.0 | 4.4 | 2.4 |
| `QTc > 500` | | 2.1 | | 2.1 | |
| `QTc > 600` | | 0.3 | | 0.3 | |
| `has ecg` | 53 | 4.3 | | 4.4 | 3.3 |
| `QTc > 450` from 2024-01-01 (32,350 patients) | | | | 0.7 | 0.5 |

0.3.2 and 0.3.3 were timed with an index per searched measurement column holding (value, patient
key, time), as the demo's `demo_muse` has them, and a `has` view. Most of what remains is
Medorama's handling of about 500,000 matches, not the database's reading; 0.3.3 writes their
reasons out for the page shown only, and sends a criterion's dates to the database, which then
reads only the ECGs of about those days ([What a criterion reads](#what-a-criterion-reads)).

## The measurement

**These server figures come from the earlier design**, in which Medorama imported tables into a
catalog of its own. Medorama now imports nothing: it reads the connected databases in place, and
keeps only its patient index, variables and summaries in its workspace
([design/databases-only.md](design/databases-only.md)). The figures below have not been measured
again on this design, and the time a request takes now includes the connected database's own.
They remain a guide to the page's cost on SQLite and PostgreSQL. How fast db-builder makes a
database has not been measured.

The numbers below were measured under these conditions:

| Condition | Value |
|---|---|
| Data | 50,000 synthetic patients, 50,000 encounters and 500,000 laboratory results (four tests, ten results per patient), made by a script like `tools/make-demo-data.py` and imported as three tables (the earlier design). |
| Program | `medorama` built with `--release`; the server ran with no workers. |
| Machine | A laptop (Apple silicon, 2026), with PostgreSQL 17 on the same machine. A database across a network adds its round trips on top ([Query counts](#query-counts)). |
| Times | One request each, after a first request had warmed the caches, measured with `curl`, in seconds, for SQLite and for PostgreSQL. |

### Pages

| Request | SQLite | PostgreSQL |
|---|---|---|
| Patient Catalog, first page (`GET patients`) | 0.07 s | 0.07 s |
| The same, page 800 (`offset=40000`) | 0.09 s | 0.22 s |
| The same, filtered by a category (`has=labs`) | 0.18 s | 0.22 s |
| The same, a text search (`q=SYN0012345`) | 0.45 s | 0.48 s |
| A patient's summary, journey and report | 0.02, 0.02 and 0.001 s | 0.07, 0.07 and 0.001 s |
| Cohort search `female AND lactate > 5.5` | 0.22 s | 0.62 s |
| Cohort search `latest troponin t > 290` | 0.006 s | 0.013 s |
| Making a cohort of 4,326 patients | 0.31 s | 0.50 s |
| A cohort's members, one page | 0.09 s | 0.12 s |
| A cohort's members as CSV | 0.17 s | 0.22 s |
| A cohort's data as CSV (the data of 5,091 members) | 0.86 s | 1.7 s |

### Query counts

The number of queries a request makes determines how many round trips it costs when the database
is across a network. With PostgreSQL at 1 ms per round trip, each query adds 1 ms.

| Request | Queries |
|---|---|
| Patient Catalog, a page of 100 | 12 to 17 |
| A cohort search | About 19 (one or two per criterion, and per connected database) |
| A cohort's members as CSV | About 19 |
| A cohort's data as CSV | About 60 (a few per database) |

The data for a page are read for the whole page at once (`Connection::query_in`), never with a
query per row.

## Many users at once

[`tools/load-test.py`](../tools/load-test.py) signs in 20 users at once. Each asks without pause
for Patient Catalog pages (at random places), a filtered list, patients' summaries, journeys and
reports, a database's rows (a page of the earlier design) and cohort searches. Because no time is spent reading pages, this makes many
more requests than 20 people would. The test used the same data and machine as above and ran for
60 seconds. Times are in seconds per request.

| Request | SQLite median / 95th percentile | PostgreSQL median / 95th percentile |
|---|---|---|
| Patient Catalog, a page anywhere | 0.12–0.14 / 0.19–0.23 | 0.19 / 0.36 |
| The same, filtered by a category | 0.19–0.26 / 0.31–0.38 | 0.49 / 0.70 |
| A patient's summary or report | 0.00 / 0.02 | 0.00 / 0.03 |
| A patient's journey | 0.02 / 0.06 | 0.04 / 0.15 |
| A database's rows, one page (the earlier design) | 0.01 / 0.04 | 0.13 / 0.48 |
| A cohort search | 0.26–0.29 / 0.72–0.91 | 0.44 / 1.72 |
| **Requests per second, all together** | **230–288** | **122** |

No request failed. With 50 users, SQLite answered 285 requests a second. The server used all of
the machine's cores. The two SQLite figures in each cell come from two runs of the same queries on
different days, with the machine under different loads.

**How the list's queries differ by database.** On PostgreSQL, the list's category filter is read
as one set of patients (`p.id IN (…)`), and every patient's latest encounter in one grouped pass,
rather than with an index probe per patient. The same set forms were slower on SQLite with many
users, because each builds a temporary table: the filtered page took 0.36 s and the plain page
0.72 s. The "has none" filter (`!lab`) was a probe on both databases, because PostgreSQL cannot
hash a `NOT IN` over 500,000 rows (it took 27 s). (Since 0.3 the list is read from each
database's persons and encounters in the workspace, which are SQLite files, and a filter on a
kind of data asks each online database.)

**Memory-mapped reads on SQLite.** Every SQLite connection reads the file memory-mapped
(`mmap_size`). With its own 2 MB page cache instead, each connection copies pages over and over,
and the same test gave 12 requests a second, with a Patient Catalog page taking 4.3 s.

PostgreSQL's numbers include a round trip per query. Its filtered list was the slowest request
and the first to improve; [S4](todo.md#s4-the-patient-catalogs-text-search-in-sql) touches the
same queries.

## The VitalStream viewers

The three viewers (Rust, C++ and Python) were measured on the same recordings, each through its
own HTTP API, as the page uses it, on 8 November 2026:

| Condition | Value |
|---|---|
| Machine | A laptop: Apple M2 Max (8 performance and 4 efficiency cores), 32 GB of memory, an internal SSD, macOS 26. |
| Programs | `vitalstream-rs` and `vitalstream-cpp` built for release, and `vitalstream-py` on Python 3.15 with numpy and wfdb-x. The STP add-on in its three implementations, built for release. |
| Method | [`tools/bench-viewers.py`](../tools/bench-viewers.py). Each recording gets a viewer of its own, started with an empty index folder. |
| Indexing | The time from the `open` request until the recording is ready, the median of three runs (the first `open`, then two with `reindex`). The Python add-on's times are from one run. |
| Reading | The median time of each request, from sending it to having the whole answer, on `127.0.0.1`. The requests are: opening the recording again once it is indexed (5 times); the whole recording at 1200 points per channel (5 times); 20 windows of 10 minutes and 20 windows of 10 seconds, at random places, every channel (10 seconds gives raw samples); and every vital sign over the whole recording (5 times). |
| Memory | The resident memory of the viewer and of the programs it starts (index workers, the add-on), sampled every 20 ms: its peak while indexing. |
| Files | Read again and again, so they were in the system's file cache. A first read from a slow disk adds that disk's time. |

The three viewers answered the same data to every request, for every recording. So did every
viewer with every add-on.

### Recordings tested

| Recording | What it is | Size |
|---|---|---|
| WFDB, one day | 8 signals at 250 Hz for 24 hours, 16-bit samples (synthetic) | 346 MB |
| WFDB with a MATLAB file, one day | The same day, its samples in a `.mat` file (MATLAB level 4) | 346 MB |
| EDF, one day | The same day as EDF | 346 MB |
| BDF, one day | The same day as BDF, 24-bit samples | 518 MB |
| WFDB, three days | 8 signals at 250 Hz for 72 hours | 1,037 MB |
| Holter | The demo's 24-hour Holter: 2 leads at 128 Hz | 44 MB |
| A folder of ICU records | The demo's 24 short ICU records in 31 folders, opened as one session (46 days) | 7 MB |
| WFDB from STP files | 3 records written by `stp2wfdb` from bedside-monitor files, with 3,888 waveform markers, opened as one session | 2 MB |
| STP files | 16 BedMasterEx recordings from intensive care units (15 GE Unity or Carescape, 1 Philips PIICiX), read by the STP add-on: 2 to 38 hours, 6 to 10 waveforms, 17 to 31 vital signs, up to 32 million waveform markers | 3 to 232 MB, 1.7 GB in all |

### Indexing

Seconds, and the peak memory while indexing.

| Recording | Rust | C++ | Python | Peak memory: Rust / C++ / Python |
|---|---|---|---|---|
| WFDB, one day | 1.24 | 1.54 | 1.10 | 227 / 221 / 358 MB |
| WFDB with a MATLAB file, one day | 1.26 | 1.54 | 1.06 | 240 / 218 / 373 MB |
| EDF, one day | 1.24 | 1.07 | 0.51 | 221 / 214 / 360 MB |
| BDF, one day | 1.33 | 1.10 | 0.84 | 226 / 192 / 365 MB |
| WFDB, three days | 3.81 | 4.74 | 3.37 | 566 / 507 / 738 MB |
| Holter | 0.17 | 0.21 | 0.13 | 32 / 44 / 216 MB |
| A folder of ICU records | 0.07 | 0.08 | 0.11 | 11 / 22 / 177 MB |
| WFDB from STP files | 0.02 | 0.03 | 0.05 | 9 / 6 / 171 MB |

A viewer with nothing open takes 4 MB (Rust), 3 MB (C++) or 88 MB (Python). The Python viewer's
first index after it starts takes about 0.25 s more, while its worker process starts.

### Reading

Milliseconds per request: Rust / C++ / Python.

| Recording | Open again | Whole recording | 10-minute window | 10-second window |
|---|---|---|---|---|
| WFDB, one day | 1 / 1 / 2 | 9 / 4 / 9 | 18 / 8 / 17 | 6 / 3 / 18 |
| WFDB with a MATLAB file, one day | 1 / 1 / 2 | 9 / 4 / 9 | 19 / 8 / 17 | 5 / 3 / 18 |
| EDF, one day | <1 / <1 / 1 | 15 / 4 / 9 | 31 / 7 / 17 | 9 / 2 / 6 |
| BDF, one day | <1 / <1 / 1 | 13 / 4 / 9 | 25 / 7 / 18 | 7 / 2 / 6 |
| WFDB, three days | 1 / 1 / 3 | 7 / 3 / 7 | 19 / 8 / 18 | 6 / 3 / 20 |
| Holter | 1 / 1 / 2 | 1 / 1 / 2 | 3 / 1 / 3 | 1 / 1 / 4 |
| A folder of ICU records | 12 / 12 / 24 | <1 / <1 / 1 | <1 / <1 / <1 | <1 / <1 / <1 |
| WFDB from STP files | 2 / 2 / 5 | 4 / 1 / 4 | 12 / 5 / 14 | 4 / 3 / 12 |

### STP files

The STP add-on indexes a `.Stp` file and reads its samples, so its implementation matters more
than the viewer's. A "typical file" is one of the 12 GE recordings of 75 to 170 MB (14 to 27
hours). Indexing times are in seconds, reading times in milliseconds (medians over the 16 files).

| Viewer and add-on | All 16 files | Typical file: median (range) | The largest file (Philips, 232 MB, 29 h) | The file with the most markers (GE, 24 MB, 38 h, 32 million) | Whole recording | Vital signs | 10-minute window | 10-second window | Peak memory: typical file / most |
|---|---|---|---|---|---|---|---|---|---|
| Rust viewer, Rust add-on | 62 | 3.1 (2.1–5.8) | 10.8 | 7.5 | 9 | 19 | 59 | 7 | 341 MB / 1.9 GB |
| Rust viewer, C++ add-on | 52 | 2.9 (1.9–5.0) | 6.0 | 7.1 | 9 | 19 | 68 | 9 | 468 MB / 2.6 GB |
| Rust viewer, Python add-on | 1,271 | 68 (41–133) | 88 | 229 | 9 | 18 | 821 | 78 | 397 MB / 5.8 GB |
| C++ viewer, Rust add-on | 62 | 3.1 (2.1–6.0) | 10.7 | 7.7 | 3 | 9 | 50 | 5 | 337 MB / 1.9 GB |
| Python viewer, Rust add-on | 65 | 3.3 (2.2–7.0) | 10.8 | 8.1 | 9 | 39 | 58 | 8 | 496 MB / 2.2 GB |

The Rust and C++ add-ons index 27 to 32 MB of `.Stp` file a second, and the Python add-on 1.3 MB.

### What the numbers show

- **WFDB, EDF and BDF are about as fast in the three viewers.** A day of 8 signals at 250 Hz is
  indexed in 0.5 to 1.5 s, and three days in 3.4 to 4.7 s: the time grows with the number of
  samples. The Python viewer reads these formats with numpy and wfdb-x, compiled code, and is
  the fastest to index EDF and BDF.
- **Reading takes milliseconds in every viewer.** On WFDB, EDF and BDF recordings every median
  was under 35 ms, and on `.Stp` files under 70 ms with the Rust or C++ add-on. The C++ viewer
  answers fastest (2 to 8 ms for a window); the Python viewer's 10-second windows of WFDB records
  take 18 to 20 ms.
- **The Python viewer needs more memory**: 88 MB with nothing open, and about 1.5 times the
  others' peak while indexing.
- **For `.Stp` files, choose the Rust or C++ add-on.** They index a typical day-long file in 2 to 6
  s; the Python add-on takes 40 s to 2 minutes, and its zoomed views are ten times slower (80 ms
  for 10 seconds, 0.8 s for 10 minutes). With the same add-on, the three viewers take the same
  time (62, 62 and 65 s for the 16 files).
- **Markers and breaks cost time and memory, as samples do.** The 24 MB file with 32 million
  waveform markers takes longer than files five times its size, and its index needs 1.9 to 2.6 GB
  of memory (5.8 GB with the Python add-on). Files with thousands of breaks are also slower than
  their size alone suggests.
- **A recording already indexed opens at once**: in 25 ms at most, whatever its size.

### Repeating it

These commands make the synthetic recordings (2.5 GB), build the viewers, measure each one and
print the tables. `run` takes any recordings as `NAME=PATH` pairs; the results and the report
show only the names given, never the files' own names, and the indexes are made in a temporary
folder, never beside the recordings.

```bash
python3 tools/bench-viewers.py make-data /tmp/bench        # prints the NAME=PATH pairs
.venv/bin/python tools/make-demo-data.py                   # the demo, its Holter in sample_data/demo
cargo build --release -p vitalstream-rs
(cd viewers/vitalstream/cpp && cmake --preset release && cmake --build --preset release)
REC="wfdb_day=/tmp/bench/wfdb_day/day.hea mat_day=/tmp/bench/mat_day/day.hea \
     edf_day=/tmp/bench/edf_day/day.edf bdf_day=/tmp/bench/bdf_day/day.bdf \
     wfdb_3days=/tmp/bench/wfdb_3days/days.hea holter=sample_data/demo/holter"
python3 tools/bench-viewers.py run --viewer rs --out results.json $REC
python3 tools/bench-viewers.py run --viewer cpp --out results.json $REC
.venv/bin/python tools/bench-viewers.py run --viewer py --out results.json $REC   # with vitalstream-py's requirements
python3 tools/bench-viewers.py report results.json
```

For `.Stp` files, give an adapter folder holding the add-on (`--adapters DIR`) and a label for
each combination (`--label rs-viewer-py-addon`).

## Index format 3

Index format 3 keeps a recording's index in an SQLite store, encrypted with its database's key
([index-format.md, Version 3](../viewers/vitalstream/docs/index-format.md#version-3-the-index-store)),
where format 2 kept it in a folder of memory-mapped files. Format 2 goes only if format 3 is no
slower in a way a user notices. This compares the two in the Rust engine, on the same synthetic
recordings as the viewers' measurement (`tools/bench-viewers.py make-data`): the recording
indexed into a folder, into a plain store and into an encrypted store (4,096 values per chunk),
then opened afresh and read as the page reads it: the whole recording at 1200 points, and 40
windows each of 10 minutes, 1 hour and 10 seconds (raw samples) at random places, every channel.
Apple M2 Max, 32 GB, release build; times in milliseconds (median / 95th percentile).

| Recording | Read | Format 2 (folder) | Format 3 (store) | Format 3 (encrypted) |
|---|---|---|---|---|
| WFDB, one day, 8 channels | open | 0.9 | 0.3 | 0.5 |
| | whole recording | 8.5 / 8.9 | 8.6 / 9.1 | 8.5 / 9.4 |
| | 10 min windows | 17.7 / 18.2 | 18.8 / 19.8 | 21.5 / 21.8 |
| | 1 h windows | 6.3 / 6.6 | 7.1 / 7.3 | 7.4 / 7.7 |
| | 10 s windows | 5.0 / 5.2 | 5.0 / 5.1 | 5.0 / 5.1 |
| EDF, one day | open | 0.9 | 0.3 | 0.5 |
| | whole recording | 14.6 / 14.7 | 14.6 / 15.3 | 14.6 / 15.3 |
| | 10 min windows | 29.7 / 30.0 | 30.9 / 31.1 | 33.7 / 34.0 |
| | 1 h windows | 10.5 / 10.6 | 11.2 / 11.4 | 11.6 / 11.9 |
| | 10 s windows | 8.5 / 8.6 | 8.6 / 8.9 | 8.5 / 8.6 |
| WFDB, three days | open | 1.1 | 0.4 | 0.5 |
| | whole recording | 6.4 / 6.7 | 6.5 / 7.7 | 6.6 / 7.3 |
| | 10 min windows | 17.5 / 18.1 | 18.8 / 19.5 | 23.0 / 23.7 |
| | 1 h windows | 6.3 / 6.4 | 7.1 / 7.2 | 8.8 / 9.3 |
| | 10 s windows | 5.2 / 5.2 | 5.0 / 5.3 | 5.2 / 5.4 |

Indexing takes 15 to 35 % longer (1.2 s against 1.5 s for a day; 3.7 s against 5.0 s for three
days, encrypted), and the store is 3 % larger than the folder. A window reads at most 5 ms more
from an encrypted store, under a third of one screen refresh: not a difference a user notices.

**The chunk size was chosen by these measurements.** With 65,536 values per chunk, an encrypted
store took 67 ms for a 10-minute window (each window decrypted whole 256 KB chunks per channel);
with 16,384, 29 ms; 8,192, 24 ms; 4,096, 21.6 ms. Opening an encrypted store took 130 to 320 ms
while readers summed the chunks' lengths (decrypting every chunk); with the arrays' lengths in a
table of their own (`arrays`), under 1 ms. To repeat:

```bash
python3 tools/bench-viewers.py make-data DIR
cargo run --release -p recording-engine --example store_bench -- DIR/wfdb_day/day.hea WORKDIR
CHUNK_VALUES=16384 cargo run --release -p recording-engine --example store_bench -- …   # another chunk size
```

**The three viewers on format 3**, through their HTTP API (`tools/bench-viewers.py`, each with its
own plain store, the same machine), against their format 2 numbers above (Rust / C++ / Python):

| Recording | Index (s) | Whole recording (ms) | 10-minute window (ms) | 10-second window (ms) | Peak memory (MB) |
|---|---|---|---|---|---|
| WFDB, one day, format 2 | 1.24 / 1.54 / 1.10 | 9 / 4 / 9 | 18 / 8 / 17 | 6 / 3 / 18 | 227 / 221 / 358 |
| WFDB, one day, format 3 | 1.21 / 1.51 / 1.07 | 9 / 4 / 9 | 19 / 9 / 20 | 5 / 3 / 19 | 213 / 259 / 442 |
| EDF, one day, format 2 | 1.24 / 1.07 / 0.51 | 15 / 4 / 9 | 31 / 7 / 17 | 9 / 2 / 6 | 221 / 214 / 360 |
| EDF, one day, format 3 | 1.21 / 1.03 / 0.44 | 15 / 4 / 10 | 32 / 8 / 20 | 9 / 2 / 6 | 233 / 232 / 440 |

Every read is within 3 ms of format 2, and every viewer answered the same data. The Python
viewer's peak memory is about 80 MB higher (its SQLCipher binding and the chunks it keeps); the
others' are within 40 MB.

## How each part grows

| Part | Grows with | Why it holds up |
|---|---|---|
| Patient Catalog (browsing, a cohort's members) | The number of patients of the databases the user may view; for the page shown (50 rows), those patients' entries and recordings in each online database. | The list is read from the persons and encounters Medorama keeps of each database (`persons.sqlite` in the workspace), so it also answers while a database is offline. Its **Data** and **Entries** columns are one grouped query per online database, over the page's persons only. |
| Patient Catalog (filters on a kind of data) | The persons with data of that kind in each online database. | Each online database is asked which persons have it (`has`), as a cohort's `has` criterion is: from its `has` view when it has one, and kept while its version stays ([Answers kept](#answers-kept)). |
| Patient Catalog (text search) | Every patient's identity and encounters. | The search matches every field of every patient in memory. Moving it into SQL is planned ([S4](todo.md#s4-the-patient-catalogs-text-search-in-sql)). |
| A patient's page | That patient's data in each database. | It is read from each database the user may view, by the patient's person keys there. |
| Cohort criteria | The rows a criterion reads in each database searched: for a threshold, the values that may pass; `max_rows` bounds the rows a criterion keeps. | A criterion goes only to the databases whose variables may hold it, and its filters run in the connected database. A threshold reads only the values that may pass, per unit (with the threshold converted), and tests them in Medorama ([What a criterion reads](#what-a-criterion-reads)). The answers are kept while the data's version stays, and read 4 at a time. Criteria are combined as sets of patient numbers. |
| A cohort's data | The members' data in each database. | Read from the connected databases, as a patient's page is. |
| A connected database | Its own indexes ([databases.md, Indexes for searches](databases.md#indexes-for-searches)); the rows a criterion reads (all that its filters find; `max_rows` bounds those kept). | The filters run in the connected database, a page's reads are kept for 30 seconds, and a criterion's while the data's version stays. |
| Connecting, refreshing a database | Its persons and visits. | One pass over its persons and one over its visits, however many; its catalog from its `variables` view or one grouped query without counting; no recording file is read. Minutes for millions of persons. Counting its variables (**Count now**) and reading all its recordings (**Index all recordings**) are done only when a manager asks. |
| Several servers | | Not in this version: the workspace is one server's folder ([design/databases-only.md](design/databases-only.md)). |

## Repeating the measurement

These commands make the data, build Medorama and db-builder, make the tables into a database,
set Medorama up and connect it, start the server without workers (then time the requests above
with `curl`), and run the load test:

```bash
.venv/bin/python tools/make-demo-data.py /tmp/d  # or any synthetic tables of the size wanted
cargo build --release -p medorama -p db-builder
db-builder new /tmp/d/data.sqlite
db-builder add /tmp/d/data.sqlite /tmp/d/databases/patients.csv /tmp/d/databases/encounters.xlsx /tmp/d/databases/*.csv
medorama --home /tmp/h init --workspace /tmp/w   # (--database-url postgres://… for the control store in PostgreSQL)
medorama --home /tmp/h databases connect data --kind dbb --url sqlite:/tmp/d/data.sqlite
medorama --home /tmp/h serve --no-workers
MEDORAMA_PASSWORD=… python3 tools/load-test.py --url http://127.0.0.1:8090 --users 20 --seconds 60
```

`MEDORAMA_DEBUG_SQL=1` shows the statements PostgreSQL refuses, and PostgreSQL's
`log_statement = 'all'` lets you count the queries of a request.
