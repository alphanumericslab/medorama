# Performance

This page reports how Medorama behaves as the data grow, as measured on synthetic data, so that
an institution can size a deployment and a contributor can see where the time goes. It is for
administrators and contributors. The design behind these numbers is in
[design/sources.md](design/sources.md) and [architecture.md](architecture.md).

## Contents

- [The measurement](#the-measurement): [Imports](#imports) · [Pages](#pages) ·
  [Query counts](#query-counts)
- [Many users at once](#many-users-at-once)
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
