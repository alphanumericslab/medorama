# The demo: an installation of its own

**Status: built (Medorama 0.3.1, November 2026).** This is the design note of how Medorama keeps
its demo's invented patients apart from real ones. It is written for contributors, reviewers and
site administrators.

## Contents

- [Summary](#summary)
- [Decisions](#decisions)
- [An installation is the demo's or a site's](#an-installation-is-the-demos-or-a-sites)
- [A demo database says so](#a-demo-database-says-so)
- [What a user sees](#what-a-user-sees)
- [Installations made before 0.3.1](#installations-made-before-031)
- [Running both on one computer](#running-both-on-one-computer)

## Summary

The demo is a whole Medorama with invented patients: the same pages, cohorts, viewers and
actions, its databases all fabricated. A Medorama installation is either the demo's or a site's,
from `medorama init` on, and never both: a demo installation connects demo databases only, a
site's installation never one. Nothing is shared between them: home, control store, workspace,
port.

## Decisions

Agreed with the project lead, 2026-11-17:

- **Never both.** An installation runs with demo databases or with real ones, never a mix, at any
  level: no demo patient is linked to a real one, counted in a real cohort, named in a real
  installation's Privacy Mode dictionary, or exported from it.
- **The demo is fully functional.** It comes with example cohorts, and a user makes more from its
  databases by their own criteria, as in a site's installation.
- **Its own server.** The demo runs as its own installation (its own home, workspace and port),
  apart from a site's.

## An installation is the demo's or a site's

[medorama/src/installation.rs](../../medorama/src/installation.rs).

- `medorama init --demo` makes a demo installation; `medorama init` a site's. The mode is kept in
  the control store (`settings`, key `installation`) and never changes: a demo installation never
  becomes a site's.
- **Connecting** reads the database's marker ([below](#a-demo-database-says-so)): a demo
  installation refuses a database without it, a site's installation a database with it. Each
  refusal says what to do.
- **Every command** (and `medorama serve`) checks first that the installation holds no database
  of the other kind; listing and removing databases stay open, to put it right.
- `medorama init --demo` connects the demo's databases, adds its example action plug-in, and
  makes its example cohorts (ICU stays; long QTc; women over 65 with an ECG; sepsis).

## A demo database says so

A demo database holds a table `medorama_demo` (one row: "A demo database: every patient in it is
invented."). Medorama reads it the same way in SQLite, SQLCipher and PostgreSQL.

| Who writes it | How |
|---|---|
| db-builder | `db-builder new --demo` |
| tools/make-demo-data.py | Its db-builder databases are made with `new --demo`; the databases it writes itself (the OMOP, registry and MUSE databases) get the table |

A connected database keeps whether it is a demo database in its settings (`demo`), read when it
is connected.

## What a user sees

- **A band on every page:** "Demo · every patient here is invented: this installation holds no
  real patient's data", in neutral colours (red is for alarms); the browser tab's title begins
  with "Demo".
- **Exports** of a demo installation's cohorts are named `demo-cohort-…`.
- **The command line** says it: `init --demo` prints "a demo installation: invented patients
  only".
- Demo patients keep their invented names and `SYN` MRNs.

## Installations made before 0.3.1

A control store made before 0.3.1 has no mode. At its first command, Medorama decides it by what
it connects: demo databases only, the demo's; otherwise a site's. A database is a demo database by
its marker, or, for a demo made before 0.3.1 (without the marker), by being a file of the demo the
installer made (`…/share/medorama/demo/`). A store that connects both is refused until the
databases of one kind are removed (`medorama databases remove NAME`), the demo's named in the
message; the demo is then made as its own installation.

## Running both on one computer

```bash
medorama --home ~/medorama-demo init --workspace ~/medorama-demo-workspace --demo
medorama --home ~/medorama-demo serve                   # port 8095: init --demo's default
medorama init --workspace ~/medorama-workspace          # a site's installation, apart
medorama serve                                          # port 8090
```

Both can be open in one browser at once: each signs in with its own cookie, named by its port
(`medorama_8095`, `medorama_8090`), so that signing in to one never signs out of the other.

`init --demo` never becomes the home that commands without `--home` reach: a site's installation
on the same computer stays the default.
