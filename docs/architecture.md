# Architecture

This page describes how Medorama is built: its parts, where each one runs, how a request
travels through the server, how data reach it, and where the system can be adapted. It is
written for institutional IT security reviewers, for integrators who deploy Medorama, and for
contributors. The security controls are reviewed in detail in [security.md](security.md), and
the protection of patient identities is explained in
[protecting-patient-data.md](protecting-patient-data.md).

## Contents

- [Overview](#overview)
- [Design principles](#design-principles)
- [The parts](#the-parts)
- [Trust boundaries and deployment](#trust-boundaries-and-deployment)
- [A request's path through the server](#a-requests-path-through-the-server)
- [Sign-in](#sign-in)
- [Protecting patient data](#protecting-patient-data)
- [The gateway](#the-gateway)
- [Data flow](#data-flow)
- [Readers and data models](#readers-and-data-models)
- [Extension points](#extension-points)
- [Contracts between the parts](#contracts-between-the-parts)
- [Technology and dependencies](#technology-and-dependencies)
- [Code map](#code-map)
- [Status](#status)

## Overview

Medorama is a research system that connects an institution's databases (OMOP instances, research
warehouses, SQLite databases built with db-builder from tables, exports and folders of
recordings), reads them in place, read-only, and joins them on each patient's identifiers. It
shows each patient's journey on one page, lets researchers build cohorts across the databases
they are granted, and opens recordings and images in its own viewers. It imports nothing, and
keeps no copy of the patient data in its own control store. It is one server program that an
institution runs on its own host. Users reach it with a web browser. Every request passes
Medorama's sign-in and the grants on each database, and every answer to a user who may not see
identifiers leaves through the **privacy gate**, which replaces identifiers and moves dates back.

![Medorama at a glance](images/architecture.svg)

## Design principles

1. **Separate programs, joined by written contracts.** Each part builds, runs and is tested on
   its own, and parts meet only through a documented contract: a database schema, a file format,
   an HTTP API, a command-line protocol or a folder layout
   ([Contracts](#contracts-between-the-parts)).
2. **One way in.** When Medorama fronts the viewers, every request passes its sign-in, roles,
   grants and rules. The viewers stay simple and read only what Medorama hands them.
3. **One way out.** Identifiers stay in the connected databases, and in each database's
   encrypted workspace folder. In Privacy Mode, every answer the server sends (JSON, files,
   exports, the waveform viewer's answers) passes the privacy gate, and the server's last layer
   refuses any answer the gate did not clear, so a new endpoint cannot forget the rule.
4. **Databases are only read.** No part of Medorama writes into a connected database or a data
   folder. What Medorama generates (each database's persons as linked, its variables, its
   recordings' state, results, cohorts) goes to its workspace, encrypted per database; removing
   a database leaves no trace of it.
5. **Identity is the databases'.** Databases are joined on exact (issuer, identifier) pairs, and
   on nothing else: no matching by names, no merges.
6. **The server decides, the page draws.** What a user may see, and how data are searched, is
   decided on the server. The pages draw what they are sent, with no build step and no
   third-party code.
7. **Open formats inside, licensed formats outside.** The repository reads open formats (WFDB,
   EDF, DICOM, GE MUSE XML, tables). A format under a vendor's license is read by an
   **add-on**, a separate program installed by an administrator and distributed under its own
   terms; nothing licensed is compiled into Medorama or a viewer.
8. **Recordings are read identically three times.** The VitalStream viewer has three
   independent implementations (Rust, C++ and Python) that write the same index byte for byte
   and answer the same API, checked against each other, so that a misread recording does not go
   unnoticed.

## The parts

| Part | Folder | Language | Runs as |
|---|---|---|---|
| **Medorama server** | [`medorama/`](../medorama) | Rust | One program, `medorama serve`, on the institution's host; also the `medorama` command line for administration |
| **Medorama's web page** | [`medorama/web/`](../medorama/web) | HTML, CSS, JavaScript | Static files embedded in the server and run in the user's browser; light and dark themes |
| **db-builder** | [`db-builder/`](../db-builder/README.md) | Rust | A command that turns tables, known exports (GE MUSE, CLIF, declared TOML formats) and folders of recordings into an open SQLite database ([schema](../db-builder/docs/schema.md)); independent of Medorama |
| **VitalStream** (waveforms: ICU monitoring, Holter, EEG, sleep) | [`viewers/vitalstream/`](../viewers/vitalstream) | Rust, C++, Python (three viewers); a shared page | Alone: a local program with its own page. Inside Medorama: a child process on a private loopback port, behind the gateway |
| **ECG viewer** (12-lead ECGs on standard paper) | [`viewers/ecg/`](../viewers/ecg) | JavaScript; `muse-py` in Python | A web page: opened from disk, served by `muse-py serve`, or served by Medorama as a tab |
| **PACS viewer** (DICOM studies: echo and angiography loops, CT, MR, X-ray, reports) | [`viewers/pacs/`](../viewers/pacs) | JavaScript; `dicom-py` in Python | A web page, like the ECG viewer (`dicom-py serve` alone) |
| **Engine** | [`libs/engine/`](../libs/engine) (`recording-engine`), [`libs/signal-core/`](../libs/signal-core) | Rust | A library used by Medorama, db-builder and the Rust viewer: readers, the index, queries, the adapter protocol. WFDB through [wfdb-x](https://github.com/alphanumericslab/wfdb-x) |
| **Add-ons (adapters)** | Outside this repository | Any | Programs in the adapter folder, run as child processes; for example stp-adapter for BedMasterEx `.Stp` files (private, available to institutions under agreement) |
| **Plug-in actions** | An administrator's `actions_dirs` | Any | Programs run as child processes by the job queue ([actions.md](actions.md)) |
| **Control store** | Medorama's home folder, or a PostgreSQL server | SQLite or PostgreSQL | Medorama's own data: users, sessions, grants, audit log, settings, the list of connected databases, patient numbers, pseudonyms and keyed hashes of identifiers, cohort definitions, wrapped keys, the job queue. No identifiers. |
| **Workspace** | `workspace_dir` | SQLite (SQLCipher), files | One folder per connected database (its persons as linked, variables, recordings, results), encrypted with its own key, its recordings' waveform indexes among them (`index.sqlite`); one file per cohort; the viewer's own index store |
| **Connected databases** | The institution's servers, or SQLite files inside `data_roots` | SQL | OMOP, any SQL database through views, db-builder databases; and the recording files they name, read where they lie |

The viewers are programs of their own. Each is useful alone and never depends on Medorama;
Medorama opens them as tabs, by an address that names the file to show
([viewers/README.md](../viewers/README.md)).

## Trust boundaries and deployment

The browser and the network are untrusted: every request is authenticated and authorized on
the server, and the page decides nothing about access. The server host is trusted: one
operating-system account runs Medorama, its viewer and its plug-ins. The zones and their trust
are tabulated in [security.md, Architecture and trust
boundaries](security.md#2-architecture-and-trust-boundaries).

![System context and trust boundaries](images/system-context.svg)

### What listens where

| Program | Listens on (default) | Reached by |
|---|---|---|
| `medorama serve` | 127.0.0.1:8090 (`host`, `port`) | Browsers, through a TLS reverse proxy or an SSH tunnel |
| VitalStream under Medorama | 127.0.0.1, a free port chosen at start | Medorama's gateway only, with requests signed by a key only Medorama knows |
| VitalStream alone | 127.0.0.1:8050 (Python), 8060 (Rust), 8070 (C++) | A browser on the same computer, or through an SSH tunnel |
| `muse-py serve`, `dicom-py serve` | 127.0.0.1:8080, 127.0.0.1:8081 | The same |
| Adapters, plug-ins, db-builder | Nothing | Medorama and the viewers start adapters and plug-ins and talk over pipes and files; db-builder is run by a data manager |

Medorama listens on the loopback address by default so that nothing on the network can reach
it until an administrator puts a TLS proxy in front of it. If it is bound to another address
without `secure_cookies = true`, it prints a warning at start. It opens outgoing connections
only to its PostgreSQL control store and to connected databases, always over verified TLS to
another host; it sends nothing else anywhere.

### What is read and what is written

| Data | Lives in | Written by |
|---|---|---|
| Connected databases | The institution's database servers, or SQLite files inside `data_roots` | Their owners (db-builder for its databases); Medorama only reads, through read-only sessions |
| Recordings, ECGs, images, documents | Their own folders, inside `data_roots`, as the recording databases name them | Nobody: only read |
| Users, sessions, grants, audit log, the databases' list and status, patient numbers, pseudonyms, keyed hashes, cohort definitions, wrapped keys, jobs | The control store | Medorama |
| Each database's persons as linked (identifiers), variables, recordings' state, derived series, results of actions | `<workspace>/db-NNNN/`, SQLite encrypted with the database's key | Medorama |
| Cohort members and their provenance | `<workspace>/cohorts/c-NNNN.sqlite`, encrypted with the cohort's key | Medorama |
| Waveform indexes | Each recording database's index store, `<workspace>/db-NNNN/index.sqlite`, encrypted with the database's key; what the viewer indexes itself, `<workspace>/viewer/index.sqlite`, with a key of its own (index format 3) | The engine (adapters write their index files into a private temporary folder, which the engine packs into the store), the viewer |
| Files written by action plug-ins | `<workspace>/db-NNNN/results/<action>/r<n>/j<job>/` (not encrypted) | Plug-ins, through the job queue |

### Deployment topologies

- **One server with SQLite.** The default: `medorama init --workspace DIR` creates a home folder
  with `medorama.toml`, the SQLite control store (owner-only, optionally encrypted with
  SQLCipher), the master key file (unless the key comes from the environment or a command) and
  the workspace. One `medorama serve` process answers requests, checks the databases and runs
  jobs in worker threads. Suited to a team or a department.
- **The control store in PostgreSQL.** One server with its control store in PostgreSQL
  (`database_url`), for institutions that run PostgreSQL already. This version runs one server:
  the workspace is a local folder.
- **Behind a TLS reverse proxy.** nginx, Apache httpd or a load balancer terminates HTTPS and
  passes requests to Medorama on 127.0.0.1; `secure_cookies = true`. Users sign in with
  Medorama's passwords.
- **Behind the institution's sign-in.** An authenticating proxy (oauth2-proxy, Apache httpd
  with mod_auth_openidc, Shibboleth or Kerberos, nginx with `auth_request`) signs users in with
  single sign-on and multi-factor authentication and names them to Medorama in a header
  (`sign_in = "proxy"`; [sign-in.md](sign-in.md)).
- **Viewers alone on a workstation.** A researcher runs a VitalStream viewer, or opens the ECG or
  PACS viewer page, on their own computer, with no Medorama. The viewer reads files under a root
  folder fixed when it was built, and may ask for a password
  ([Running separately and together](#running-separately-and-together)).

### Running separately and together

**Separately**, each viewer runs on its own: the VitalStream viewers on their ports with their
own page and optional password (`viewers/vitalstream/install.sh`); the ECG and PACS viewers as
pages opened from disk or served by `muse-py serve` and `dicom-py serve`. Medorama runs on its
port and may link to a VitalStream viewer running elsewhere (`viewer_url`). db-builder runs
wherever the data manager prepares a database, on the command line or as its own web page
(`db-builder serve --root DIR`, with its own sign-in).

**Together** (`tools/install-medorama.sh --viewer cpp|rs|py|none`), Medorama is installed with
one VitalStream viewer and db-builder; the ECG and PACS viewers are built into Medorama.
Medorama's supervisor (`viewer.rs`) starts the VitalStream viewer on first use, on 127.0.0.1 and
a free port, with a settings file and a private password file that Medorama writes, restarts it
if its process ends or if a request cannot reach it and it then fails a health check, and stops
it when Medorama stops. A request to a running viewer waits for no other request and costs no
check, so a viewer busy with a large recording is never taken for a dead one. Add-ons are
installed into the adapter folder that Medorama and the viewers share (`medorama addons install
PROGRAM`). With `db_builder_root` set, Medorama also runs db-builder's web interface
(`dbbuilder.rs`) as a child process on 127.0.0.1, port `db_builder_port`, started again when it
stops; the Databases tab's **Create a database…** opens its page in a tab of its own. Medorama
does not proxy it, and db-builder keeps its own sign-in (administration.md, "db-builder").

In Medorama's page, the top bar offers **Cohorts**, **Patient Catalog** and **Databases** (three
ways to the patients), then the viewers' tabs **ECG**, **PACS** and **VitalStream**, and at the
right **History**, **Actions**, **Jobs**, **Admin** and the **Privacy Mode** switch. A patient's
recordings open in the VitalStream tab at the chosen moment, and their ECGs and images in the
viewers' tabs.

## A request's path through the server

![Inside the server: one request's path](images/components.svg)

Every HTTP request passes the same layers (axum middleware, [server.rs](../medorama/src/server.rs)
`router`):

1. **Guard** (`guard`). The server refuses any API request that changes something unless it
   carries the header `X-Medorama: 1`, which a page on another site cannot add. On the way out,
   it sets security headers on every answer: a Content Security Policy, framing rules,
   `nosniff`, `no-referrer`, and `no-store` on the API.
2. **Sign-in** ([signin.rs](../medorama/src/signin.rs), `identify`). The privacy layer asks the
   sign-in module who the request comes from, once per request: the account of a session
   cookie, or the account an authenticating proxy names.
3. **Privacy layer, inbound** (`privacy`). The server decides whether the session is in Privacy
   Mode. In Privacy Mode, the user is replaced by a copy without the PHI permission.
4. **Handler.** The route's handler receives the user as `Auth(User)` (HTTP 401 without one),
   checks the role and the grants it needs ([grants.rs](../medorama/src/grants.rs)), and reads the
   control store, the workspace and the connected databases through the core modules. A
   database's data key is unwrapped only for a request whose user holds a grant on it. A handler
   that knows its data de-identifies them itself (files, notes).
5. **Privacy layer, outbound: the gate.** For a session in Privacy Mode, every JSON answer
   passes the privacy gate; a file passes only if its handler cleared it; any other answer
   carrying data is refused. The answer then leaves through the guard.

The handlers are thin: the rules live in modules that the command line, the background workers
and the tests also use.

## Sign-in

How users sign in is decided in one module, [signin.rs](../medorama/src/signin.rs), chosen by
the setting `sign_in`:

- **`password`** (the default): Medorama's own accounts, with Argon2id password hashes,
  throttling, and session cookies holding a random token that is stored hashed.
- **`proxy`**: the institution's authenticating reverse proxy signs the user in (single sign-on
  through OpenID Connect or SAML, Kerberos, multi-factor authentication) and names the user in
  a header, which Medorama believes only on connections from the proxy's addresses.

![Sign-in: one seam](images/sign-in.svg)

Either way, accounts, roles and the PHI permission live in Medorama, and every handler receives
the signed-in user in the same form. Supporting another method means changing the sign-in
module only. Configuration, proxy examples and the code changes for a new method are in
[sign-in.md](sign-in.md).

## Protecting patient data

![The privacy gate: what leaves in Privacy Mode](images/privacy-gate.svg)

Identifiers (names, medical record numbers, patient IDs, dates of birth) stay in the connected
databases. Medorama keeps what each database says of its persons in that database's workspace
folder (`persons.sqlite`), encrypted with its key, and in the control store only keyed hashes of
the (issuer, identifier) pairs. A user sees a database's identifiers only with the `phi` grant on
it and the PHI permission, outside Privacy Mode. What leaves the server is decided at the moment
of sending: a session is in **Privacy Mode** unless its user has the PHI permission and has
switched Privacy Mode off, and in Privacy Mode the privacy gate
([gate.rs](../medorama/src/gate.rs), [deid.rs](../medorama/src/deid.rs),
[dicom.rs](../medorama/src/dicom.rs), [pseudonyms.rs](../medorama/src/pseudonyms.rs)):

- replaces identifiers with placeholders (`[Patient Name]`, `[MRN]`) and random pseudonyms
  (`MO-XXXX-XXXX`, visits `MO-E-…`);
- moves every date back by an offset plus each patient's own days, so that a patient's
  intervals are kept;
- de-identifies files at the source (WFDB headers, GE MUSE XML, DICOM headers following PS3.15's
  Basic Profile, EDF and BDF headers and annotations), passes pictures, videos and PDF documents
  with a warning, and refuses any other file;
- refuses searches of identifiers;
- applies to exports, to the VitalStream viewer through the gateway, and to every connected
  database's data; plug-ins receive pseudonyms only.

The full explanation is [protecting-patient-data.md](protecting-patient-data.md); the controls,
their code and their tests are in [security.md, Privacy Mode and the privacy
gate](security.md#7-privacy-mode-and-the-privacy-gate). The workspace and its keys are described
in [design/databases-only.md](design/databases-only.md#keys-and-encryption).

## The gateway

When Medorama runs a VitalStream viewer, the browser never talks to the viewer directly. Medorama
passes the viewer's requests through `/viewer/` ([gateway.rs](../medorama/src/gateway.rs)) and
decides each one.

- **Default deny.** Only the listed endpoints of the viewer's API (version 1) pass; anything
  else, including endpoints a newer viewer may add, is refused until it is given a rule.
- **Medorama's sign-in.** A signed-in Medorama user is required. The viewer runs with a private
  password generated by Medorama; Medorama signs each request it passes on and never forwards
  the user's own cookie. Users never see the viewer's sign-in, and the viewer's port is of no
  use to anyone else on the host.
- **Rules per user:**

| Viewer request | Administrators | Analysts and viewers |
|---|---|---|
| The Open dialog (`browse`, `complete`, places and the start folder in `config`) | Folders inside `data_roots` (everything when it is empty) | Only folders that hold recordings of databases granted to them with `view`, showing only those recordings |
| `scan` a folder and `open` a recording | Any recording inside `data_roots` | Only recordings of databases granted to them with `view` |
| `cancel` an index being built; `datasets/{key}…` (windows, vital signs, markers) | Only datasets they opened through the gateway | The same |
| In Privacy Mode (anyone) | Browsing and scanning refused; recordings opened by token (`mo-rec:<n>`); times translated both ways; paths and names replaced; the viewer's error texts (which name files) replaced by a fixed sentence, and the records of a folder it left out named by number | The same |

- **Changes are JSON.** A request that changes something (`open`, `cancel`) must be sent as
  `application/json`, which another site's page cannot send unasked.
- **The page's policy.** The viewer's page is served with a Content Security Policy of its own
  scripts and styles only (`default-src 'self'; script-src 'self'; img-src 'self' data:;
  object-src 'none'; base-uri 'none'; frame-ancestors 'self'`).
- **Audit.** Each opened recording is written to the audit log (`view`).
- **Paths** are compared after symbolic links are resolved, as the viewers do.
- **Settings.** The viewer starts with a settings file that Medorama writes (its cache and adapter
  folders among them), so a user's own `vitalstream.toml` cannot conflict with the gateway's
  rules.
- **Index stores.** The viewer is started with `--keys-on-stdin`: Medorama writes on its standard
  input one JSON line per index store, with its key (each recording database's, and the viewer's
  own), at each start and when a database is connected, and `{"forget": …}` before a database is
  removed. The keys are never on a command line, in the environment, in a file or in a log.

The ECG and PACS viewers need no gateway: they are pages that Medorama serves, and they read only
a recording's files through `patients/{id}/file/{item}` (`item` is the recording's number), which
Medorama checks like every other request ([api.md](api.md)).

## Data flow

### Databases are connected, never imported

![How data come in: databases, connected](images/data-in.svg)

Data reach Medorama only as connected databases ([databases.md](databases.md)). An admin connects
one (**Databases** › **Connect a database**, or `medorama databases connect`) of one of three
kinds:

| Kind | What | Read through |
|---|---|---|
| `omop` | An OMOP CDM 5.3 or 5.4 database (PostgreSQL or SQLite) | Views built into Medorama ([sources/omop.rs](../medorama/src/sources/omop.rs)) |
| `views` | Any SQL database | SQL views the site writes: `persons`, `visits`, `entries`, `recordings`, `version` |
| `dbb` | A SQLite database built by db-builder from tables, known exports and folders of recordings | Its tables, which have the canonical columns ([db-builder/docs/schema.md](../db-builder/docs/schema.md)) |

Connecting checks each view, makes the database's data key and workspace folder, grants the
database to the admin, reads its **variables** (what it holds), **links its persons** (exact
(issuer, identifier) pairs, so that a person in several databases is one patient with one
pseudonym; a person whose pairs name two patients is a conflict, listed and not linked) and lists
its **recordings**. Each database is then checked every minute (a green dot online, grey
offline); a new data version reads it again. Cohort criteria go to the databases the user ticked
and is granted, that are online and whose variables may hold them; each member records the
databases that put it there. The patient page reads every granted database live. Removing a
database destroys its key, its workspace folder and every cohort member it contributed to. The
design is in [design/databases-only.md](design/databases-only.md) and
[design/sources.md](design/sources.md).

### Building a database with db-builder

Tables (CSV, Excel, OpenDocument, Parquet), known exports (GE MUSE, CLIF, a site's export described
in TOML) and folders of recordings become a database with db-builder, before Medorama sees them.
db-builder recognizes their columns ([importing.md](importing.md)), names each patient by
(issuer, identifier), keeps every other column as it came, and lists every file a row names as a
recording. It writes only its output database.

### Recordings and indexing

A recording (a WFDB record, an EDF file, a folder of records, or a licensed format read by an
add-on) is listed by its recording database and read where it lies, inside `data_roots`; it is
never copied. Each recording is numbered `database × 1,000,000,000 + n` across Medorama.
Background workers index it: the engine, or the add-on, reads the recording once and writes an
**index** (channels, pieces, envelopes, vital signs, events) in the shared format of
[index-format.md](../viewers/vitalstream/docs/index-format.md), into its database's index
store (`<workspace>/db-NNNN/index.sqlite`, encrypted with the database's key; index format 3).
The viewers then open the recording quickly from its index. Workers also derive series (for example a
Holter's heart rate, kept in the database's `recordings.sqlite`) and run plug-in actions from the
job queue ([jobs.rs](../medorama/src/jobs.rs)), whose numbers go into the database's
`results-<action>.sqlite` and are searchable as `<action>.<field>`.

### Files read in place

ECGs, DICOM studies, pictures and documents are recordings of their database's category (`ecg`,
`img`, `echo`, `cath` …), served from where they lie, one at a time, through
`patients/{id}/file/{item}`. Medorama never writes to them; in Privacy Mode, it de-identifies them
as they are sent.

## Readers and data models

![Readers: one per format, four data models](images/readers.svg)

Each file format has its own **reader**, and every reader produces one of four **data models**:

| Data model | Produced by | Used by |
|---|---|---|
| Recording index | The engine's readers (WFDB, EDF/BDF) in Rust, C++ and Python; adapters for other formats | VitalStream; Medorama's indexing and cohorts |
| ECG record | The ECG viewer's readers (WFDB, GE MUSE XML, DICOM ECG, EDF, its JSON form) | The ECG viewer |
| DICOM objects | The PACS viewer's DICOM reader; `dicom-py`; the privacy gate's `dicom.rs` | The PACS viewer |
| Canonical columns (persons, visits, entries, recordings) | Connected databases' views; db-builder's tables (from its table readers and formats) | Patient pages, cohorts, the Databases tab |

Past its reader, a format is like any other: the viewers, cohorts and the privacy gate know only
the data models. A new format is one new reader. Where readers live and how to add one is
described in [readers.md](readers.md); EDF's reading rules in [edf.md](edf.md).

### The engine, adapters and the toolbox

**The engine** ([libs/engine](../libs/engine)) holds what is needed to read a recording quickly:
the readers (WFDB records, multi-segment records, folders of records read as one session,
[WFDB collections](wfdb-collection.md), EDF, EDF+, BDF and BDF+), the index, the dataset queries
the viewer's API answers (windows, envelopes, vital signs, markers), and a sample reader with
which Medorama derives a Holter's heart rate. The Rust viewer is a thin HTTP layer over it;
the C++ and Python viewers have their own engines that write the same index. db-builder uses it
to read a recording's start and end from its header.

**Adapters** read formats the engines do not. An adapter is a program named `<name>-adapter` in
the adapter folder; the engine asks it to `describe` itself, `detect` the files it reads,
`index` a recording (writing the shared index format) and `serve` samples over a pipe. It may be
written in any language ([adapters.md](adapters.md)).

**The toolbox** (planned) will hold processing on waveforms (filters, beat detection, signal
quality, machine-learning inference), each tool available as a Medorama action, on the command
line, from Python and as tracks in the viewer ([todo.md](todo.md#d2-the-toolbox-and-the-results-format)).

## Extension points

An institution adapts Medorama at a few documented places, without changing the rest.

| To adapt | Where | Contract |
|---|---|---|
| How users sign in | Configuration only, for an authenticating proxy (`sign_in = "proxy"`); or, in code, a new variant of `signin::Method` and its arm in `signin::identify` ([signin.rs](../medorama/src/signin.rs)) | [sign-in.md](sign-in.md) |
| An existing database, read in place | A database of kind `omop`, or `views` with views that give the canonical columns ([medorama/src/sources/](../medorama/src/sources)) | [databases.md](databases.md) |
| Tables or a new export | db-builder: recognized columns, a mapping, a TOML description (`--formats`, no code), or a converter in [db-builder/src/formats/](../db-builder/src/formats) | [importing.md](importing.md), [db-builder/README.md](../db-builder/README.md) |
| A new recording format | A reader in the engine ([libs/engine/src/sources/mod.rs](../libs/engine/src/sources/mod.rs)), or an add-on in any language | [adapters.md](adapters.md), [index-format.md](../viewers/vitalstream/docs/index-format.md), [readers.md](readers.md) |
| A new ECG format in the ECG viewer | A reader registered in `viewers/ecg/readers.js` | [readers.md](readers.md) |
| The master key's source | `master_key_command` (a keyring, Vault, a KMS command line), `$MEDORAMA_MASTER_KEY` or `master_key_file` | [design/databases-only.md](design/databases-only.md#keys-and-encryption) |
| An analysis on recordings | A plug-in action: a folder with `action.toml` in `actions_dirs` | [actions.md](actions.md) |
| Scripts and other clients | The HTTP API | [api.md](api.md), [`medorama/openapi.yaml`](../medorama/openapi.yaml) |

## Contracts between the parts

| Contract | Specification | Between | Checked by |
|---|---|---|---|
| db-builder's database, schema version 1 | [db-builder/docs/schema.md](../db-builder/docs/schema.md) | db-builder, Medorama and any SQLite reader | `db-builder/tests/`; `medorama/tests/databases.rs` |
| Canonical views; requirements on a connected database | [databases.md](databases.md) | The sites' databases and Medorama | `medorama/tests/databases.rs` |
| db-builder's inputs (tables and their columns) | [importing.md](importing.md) | Source systems and db-builder | `db-builder/tests/inputs.rs` |
| Index (folder layout, key, arrays) | [index-format.md](../viewers/vitalstream/docs/index-format.md) | The engines and add-ons, Medorama, the viewers | `viewers/vitalstream/tests/parity/index_parity.py` (the stores' rows, byte for byte) |
| VitalStream's API, version 1 | [viewer-api.md](../viewers/vitalstream/docs/viewer-api.md) | VitalStream's page and Medorama's gateway, and the three viewers | `viewers/vitalstream/tests/parity/`; `medorama/tests/gateway.rs` |
| Adapter protocol, version 1 | [adapters.md](adapters.md) | The engines, Medorama and db-builder, and add-ons | The engines' adapter tests; each add-on's conformance tests |
| EDF reading rules | [edf.md](edf.md) | The three engines and the ECG viewer | The engines' EDF tests, parity tests |
| WFDB collection | [wfdb-collection.md](wfdb-collection.md) | The engines, and the tools that write long recordings as WFDB records | The viewers' tests, parity tests |
| Viewer addresses (`#src`, `#list`, `#path`) | [viewers/README.md](../viewers/README.md) | Medorama and every viewer | The viewers' tests, Medorama's tests |
| Medorama's HTTP API, version 1 | [api.md](api.md), [`openapi.yaml`](../medorama/openapi.yaml) | Medorama and its page, scripts | `medorama/tests/openapi.rs` |
| Plug-in actions | [actions.md](actions.md) | Medorama and plug-ins | Medorama's tests |
| The control store on SQLite and PostgreSQL | [administration.md](administration.md) | Medorama and its database | The tests, on both (`MEDORAMA_TEST_POSTGRES`) |
| Names of programs and packages | [naming.md](../viewers/vitalstream/docs/naming.md) | Everything | Review |

## Technology and dependencies

- **Medorama** is written in Rust: axum and tokio for HTTP, rusqlite with SQLCipher (always
  built: building needs Perl and a C compiler for OpenSSL) for SQLite and the encrypted
  workspace, AES-256-GCM for the wrapped keys, tokio-postgres with rustls for PostgreSQL, argon2
  for passwords. Versions are pinned by `Cargo.lock` and checked by `cargo deny`.
- **db-builder** is written in Rust: rusqlite with SQLCipher, calamine for spreadsheets, parquet
  for Parquet files, and the engine for recording headers.
- **The web page and the ECG and PACS viewers** are plain HTML, CSS and JavaScript modules, with
  no build step and no third-party code, embedded in the server's binary.
- **VitalStream** has three implementations: Rust (over the engine), C++20 (CMake) and Python.
  All three read WFDB through wfdb-x, a public WFDB library.
- **The Python tools** `muse-py` and `dicom-py` write tables of GE MUSE ECGs and DICOM studies,
  which db-builder adds, and serve the ECG and PACS viewers alone.

Every third-party component and its license is listed in
[THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md). Building is described in
[building.md](building.md).

## Code map

The modules of the Medorama server (`medorama/src/`). A guide for contributors is in
[developing.md](developing.md).

| Module | Responsibility |
|---|---|
| `server.rs` | Routes, the guard and privacy layers, files in Privacy Mode, background workers (database checks, indexing, jobs) |
| `signin.rs`, `users.rs` | Sign-in methods (password and proxy); accounts, roles, the PHI permission, sessions |
| `grants.rs` | Grants on databases (`search`, `view`, `phi`, `manage`) and cohorts (`view`, `edit`, `manage`) |
| `gate.rs`, `deid.rs`, `dicom.rs`, `pseudonyms.rs` | The privacy gate: identifier dictionary and placeholders; dates, ages, WFDB, MUSE and EDF headers; DICOM; pseudonyms |
| `gateway.rs`, `viewer.rs` | The VitalStream gateway; the viewer's supervisor |
| `databases.rs` | Connecting a database, its status checks, refreshing, removing it with every trace |
| `sources/` | Reading connected databases: settings, read-only pools, the canonical views, OMOP's views |
| `persons.rs` | The patient index: (issuer, identifier) pairs, keyed hashes, conflicts; each database's `persons.sqlite` |
| `variables.rs` | What each database holds (`variables.sqlite`), for the planner and the Cohort tab |
| `recordings.rs`, `derived.rs`, `categories.rs` | Recordings of recording databases (`recordings.sqlite`, numbers, the index sweep); derived series and results databases of actions; categories |
| `workspace.rs` | The workspace: data keys, the master key, SQLCipher files, the index stores (`index_store`, `viewer_store`), shredding, rotation |
| `patient/`, `cohorts/`, `expr.rs`, `search.rs` | The patient pages; cohorts (criteria, the planner, members and provenance, exports); criteria written as text; the search syntax |
| `jobs.rs`, `actions.rs`, `addons.rs` | The job queue, plug-in actions, installed adapters |
| `audit.rs`, `db.rs`, `sql/`, `schema/`, `config.rs` | The sealed audit log; the control store and its schema; SQLite and PostgreSQL behind one interface; settings |
| `display.rs`, `xlsx.rs`, `util.rs` | How the page writes dates and times; the Excel writer for exports; small helpers |

db-builder's modules are in `db-builder/src/` (`add.rs`, `tables.rs`, `parquet_rows.rs`,
`formats/`, `recordings.rs`, `check.rs`, `db.rs`), described in its
[README](../db-builder/README.md).

## Status

Built: everything above, except the toolbox and its results format. Connecting databases only
(with the encrypted workspace, grants, the exact patient index and removal without trace) was
built in November 2026 ([design/databases-only.md](design/databases-only.md)), and so was index
format 3: the waveform indexes in each database's workspace folder, encrypted with its key
([performance.md, Index format 3](performance.md#index-format-3)). Not built yet, and the next
work: access limited to a cohort's patients. Later: built-in key managers; FHIR and other SQL engines;
the toolbox and the engine's streaming reader; viewing recordings while they are being recorded;
server-side decoding of codecs that browsers do not decode (JPEG-LS, JPEG 2000, MPEG); DICOM
Holter in VitalStream; free-text de-identification; DICOMweb; ONNX inference. Each item, with its
design and tests, is in [todo.md](todo.md).
