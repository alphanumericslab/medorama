# Performance

This page reports how Medorama behaves as the data grow, as measured on synthetic data, and how
fast the three VitalStream viewers index and read recordings, so that an institution can size a
deployment and a contributor can see where the time goes. It is for administrators and
contributors. The design behind these numbers is in
[design/sources.md](design/sources.md) and [architecture.md](architecture.md).

## Contents

- [The measurement](#the-measurement): [Imports](#imports) · [Pages](#pages) ·
  [Query counts](#query-counts)
- [Many users at once](#many-users-at-once)
- [The VitalStream viewers](#the-vitalstream-viewers): [Recordings tested](#recordings-tested) ·
  [Indexing](#indexing) · [Reading](#reading) · [STP files](#stp-files) ·
  [What the numbers show](#what-the-numbers-show) · [Repeating it](#repeating-it)
- [How each part grows](#how-each-part-grows)
- [Repeating the measurement](#repeating-the-measurement)

## The measurement

The numbers below were measured under these conditions:

| Condition | Value |
|---|---|
| Data | 50,000 synthetic patients, 50,000 encounters and 500,000 laboratory results (four tests, ten results per patient), made by a script like `tools/make-demo-data.py` and imported as three tables. |
| Program | `medorama` built with `--release`; the server ran with no workers. |
| Machine | A laptop (Apple silicon, 2026), with PostgreSQL 17 on the same machine. A database across a network adds its round trips on top ([Query counts](#query-counts)). |
| Times | One request each, after a first request had warmed the caches, measured with `curl`, in seconds, for SQLite and for PostgreSQL. |

### Imports

The imports were measured with synthetic GE MUSE ECG lists (one row per ECG with its
measurements, 125,000 rows for 25,000 patients), and with synthetic tables of 5,000 patients,
5,000 encounters and 100,000 laboratory results. Each was imported into a new catalog. Times are
in seconds.

| Table | Rows | SQLite | PostgreSQL |
|---|---|---|---|
| ECG list, preview (`--dry-run`) | 125,000 | 6.0 | 18 |
| ECG list | 125,000 | 8.3 | 19 |
| ECG list (four files, one database) | 500,000 | 44 | 74 |
| Patients | 5,000 | 0.3 | 1.1 |
| Encounters | 5,000 | 0.3 | 1.1 |
| Laboratory results | 100,000 | 2.4 | 8.7 |

Four things make an import fast
([administrator's guide, Imports and sources](administration.md#imports-and-sources)):
- **Rows are processed a chunk at a time.** Rows are read 5,000 at a time. The patients and
  encounters of a chunk are read for all its rows at once and decided in memory, with the rows
  about the same patient grouped together. Those that follow the plain rules are written as
  multi-row statements (new patients and encounters with their IDs reserved, MRNs, field values,
  provenance and bed stays); the others are written row by row. For the chunk's items, a few
  queries find their patients and encounters, one query per few hundred items finds those
  imported before, and the new items are written many to a statement. This matters most on
  PostgreSQL, where each statement is a round trip. The resulting catalog is the same either way,
  as `tests/bulk.rs` checks.
- **The search index is written once a chunk.** SQLite's full-text index of the items is written
  for each chunk's items at once, as the chunk is committed, instead of being flushed and merged
  at every statement, which would take most of an import's time.
- **The write lock is free between chunks.** On SQLite, which has one writer at a time, a table
  is committed a chunk at a time (with a pause of 30 ms after each), under a batch that stays
  *loading* until the end; other writes (a page's change, the audit log) wait for at most one
  chunk, not for the whole import. On PostgreSQL an import is one transaction. What a loading
  batch holds is left out of every page by one indexed subquery, which costs a few milliseconds
  on the 50,000-patient catalog when no import runs.
- **Waiting items are matched as sets** after each import, however many are waiting.
- **Each database has a table of its numbers** (`dbcol_<import>`), written with the rows, so that
  criteria on a database's columns read an index. On 500,000 ECGs, `ecg.QTc > 600` takes 0.02 s
  on SQLite and 0.32 s on PostgreSQL; `ecg.QTc > 470`, which keeps a fifth of the rows, takes
  0.45 s and 1.2 s.

The catalog of 500,000 ECGs takes 503 MB on SQLite. An import holds 0.2 to 0.6 GB of memory,
whatever its size. SQLite's times vary by about a fifth between runs on this machine.

### Pages

| Request | SQLite | PostgreSQL |
|---|---|---|
| Patient Catalog, first page (`GET patients`) | 0.07 s | 0.07 s |
| The same, page 800 (`offset=40000`) | 0.09 s | 0.22 s |
| The same, filtered by a category (`has=labs`) | 0.18 s | 0.22 s |
| The same, a text search (`q=SYN0012345`) | 0.45 s | 0.48 s |
| A patient's summary, journey and report | 0.02, 0.02 and 0.001 s | 0.07, 0.07 and 0.001 s |
| A database's rows, one page | 0.03 s | 0.11 s |
| The same, sorted by a column | 0.55 s | 0.99 s |
| Cohort search `female AND lactate > 5.5` | 0.22 s | 0.62 s |
| Cohort search `latest troponin t > 290` | 0.006 s | 0.013 s |
| Making a cohort of 4,326 patients | 0.31 s | 0.50 s |
| A cohort's members, one page | 0.09 s | 0.12 s |
| A cohort's members as CSV | 0.17 s | 0.22 s |
| A cohort's data as CSV (every database's rows for 5,091 members) | 0.86 s | 1.7 s |

### Query counts

The number of queries a request makes determines how many round trips it costs when the database
is across a network. With PostgreSQL at 1 ms per round trip, each query adds 1 ms.

| Request | Queries |
|---|---|
| Patient Catalog, a page of 100 | 12 to 17 |
| A database's rows, a page | About 11 |
| A cohort search | About 19 (one or two per criterion, and per connected database) |
| A cohort's members as CSV | About 19 |
| A cohort's data as CSV | About 60 (a few per database) |

The data for a page are read for the whole page at once (`Connection::query_in`), never with a
query per row.

## Many users at once

[`tools/load-test.py`](../tools/load-test.py) signs in 20 users at once. Each asks without pause
for catalog pages (at random places), a filtered list, patients' summaries, journeys and reports,
a database's rows and cohort searches. Because no time is spent reading pages, this makes many
more requests than 20 people would. The test used the same data and machine as above and ran for
60 seconds. Times are in seconds per request.

| Request | SQLite median / 95th percentile | PostgreSQL median / 95th percentile |
|---|---|---|
| Patient Catalog, a page anywhere | 0.12–0.14 / 0.19–0.23 | 0.19 / 0.36 |
| The same, filtered by a category | 0.19–0.26 / 0.31–0.38 | 0.49 / 0.70 |
| A patient's summary or report | 0.00 / 0.02 | 0.00 / 0.03 |
| A patient's journey | 0.02 / 0.06 | 0.04 / 0.15 |
| A database's rows, one page | 0.01 / 0.04 | 0.13 / 0.48 |
| A cohort search | 0.26–0.29 / 0.72–0.91 | 0.44 / 1.72 |
| **Requests per second, all together** | **230–288** | **122** |

No request failed. With 50 users, SQLite answered 285 requests a second. The server used all of
the machine's cores. The two SQLite figures in each cell come from two runs of the same queries on
different days, with the machine under different loads.

**How the list's queries differ by database.** On PostgreSQL, the list's category filter is read
as one set of patients (`p.id IN (…)`), and every patient's latest encounter in one grouped pass,
rather than with an index probe per patient. The same set forms are slower on SQLite with many
users, because each builds a temporary table: the filtered page took 0.36 s and the plain page
0.72 s. SQLite therefore keeps the probe per patient, and the list's queries are chosen by
database (`patient/list.rs`). The "has none" filter (`!lab`) is a probe on both databases, because
PostgreSQL cannot hash a `NOT IN` over 500,000 rows (it took 27 s).

**Memory-mapped reads on SQLite.** Every SQLite connection reads the file memory-mapped
(`mmap_size`). With its own 2 MB page cache instead, each connection copies pages over and over,
and the same test gives 12 requests a second, with a catalog page taking 4.3 s.

PostgreSQL's numbers include a round trip per query. Its filtered catalog is the slowest request
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

## How each part grows

| Part | Grows with | Why it holds up |
|---|---|---|
| Patient Catalog (browsing, filters, a cohort's members) | The number of patients, for ordering IDs; the page size, for everything else. | Filtering and ordering happen in the database, and the page's details are read for that page only. |
| Patient Catalog (text search) | Every patient's identifiers and encounters. | The search matches every field of every patient in memory. Moving it into SQL is planned ([S4](todo.md#s4-the-patient-catalogs-text-search-in-sql)). |
| A patient's page | That patient's data. | The data are indexed by patient. |
| A database's rows | The page size; sorting reads the whole table. | The rows are indexed by import and line. |
| Cohort criteria | The rows of the result named (through an index on its name), or the whole table for text and `has` criteria. | Criteria are evaluated as sets of patient IDs. A threshold reads only the values that may pass, per unit (with the threshold converted), and tests them in Medorama. |
| A cohort's data | The members' rows, found through their items (`ehr_items` by patient, `import_rows` by item); the rows of patients and encounters, every one. | The data are read from the items to their rows, with SQLite told the order (`CROSS JOIN`). |
| A connected database | Its own indexes; the rows a criterion reads (`max_rows`). | The filters run in the connected database, and a page's reads are kept for 30 seconds. |
| Several servers | The database server. | Sessions, cohorts and jobs are in PostgreSQL, and advisory locks ensure that work meant to happen once does. |

## Repeating the measurement

These commands make the data, build Medorama, create a catalog, import a table (time each import),
start the server without workers (then time the requests above with `curl`), and run the load
test:

```bash
.venv/bin/python tools/make-demo-data.py        # or any synthetic tables of the size wanted
cargo build --release -p medorama
medorama init --results /tmp/r --home /tmp/h     # (--database-url postgres://… for PostgreSQL)
medorama --home /tmp/h import-table patients.csv
medorama --home /tmp/h serve --no-workers
MEDORAMA_PASSWORD=… python3 tools/load-test.py --url http://127.0.0.1:8090 --users 20 --seconds 60
```

`MEDORAMA_DEBUG_SQL=1` shows the statements PostgreSQL refuses, and PostgreSQL's
`log_statement = 'all'` lets you count the queries of a request.
