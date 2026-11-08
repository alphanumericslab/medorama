# Documentation

This index lists every page of Medorama's documentation, grouped by reader and task, each with a
one-line description. If you are new to Medorama, start with [Get started](getting-started.md);
the project's [front page](../README.md) gives the overview.

- [Using Medorama](#using-medorama)
- [Running Medorama](#running-medorama)
- [Reviewing security and privacy](#reviewing-security-and-privacy)
- [Bringing data in](#bringing-data-in)
- [Reading formats and integrating](#reading-formats-and-integrating)
- [The viewers](#the-viewers)
- [Developing Medorama](#developing-medorama)
- [Design notes](#design-notes)

## Using Medorama

For researchers, clinicians and anyone trying Medorama for the first time.

| Page | What it covers |
|---|---|
| [Install](install.md) | Commands to install Medorama, VitalStream, and the ECG and PACS viewers' servers on Linux, macOS and Windows. |
| [Get started](getting-started.md) | Install Medorama with the demo, sign in, take a guided tour, and remove the demo. |
| [User guide](user-guide.md) | The web page: cohorts, the Patient Catalog and patient pages, the Databases tab, Privacy Mode, searching, the viewers, actions and history. |
| [User's manual](user-manual/medorama-user-manual.pdf) | The complete manual as a book (LaTeX and PDF): every feature, setting and interface, installation and deployment, and the formats read. |

## Running Medorama

For administrators who install Medorama and run it for other people.

| Page | What it covers |
|---|---|
| [Administration](administration.md) | Setting up a catalog, settings, PostgreSQL, importing data, identity across databases, connected databases, add-ons, users and the PHI permission, the audit log, deployment, services, backups and troubleshooting. |
| [Install](install.md) | The installation commands for each operating system. |
| [Building and installing](building.md) | Building every part on Linux, macOS and Windows, including without administrator rights or internet access, and updating or uninstalling. |
| [Performance](performance.md) | Measured import times, page times and query counts at 50,000 patients on SQLite and PostgreSQL, and many users at once. |

## Reviewing security and privacy

For IT security offices, privacy officers and anyone who must approve a deployment on patient
data.

| Page | What it covers |
|---|---|
| [Security and privacy](security.md) | The review document: scope, trust boundaries, threat model, authentication, authorization, the privacy gate, data at rest and in transit, audit, regulatory mapping, deployment checklist and known limitations. |
| [Architecture](architecture.md) | The parts of the system, the server's layers and modules, the privacy gate, the contracts between parts, and the viewer gateway. |
| [Protecting patient data](protecting-patient-data.md) | How Medorama hides identifiers: the privacy gate, Privacy Mode, pseudonyms, placeholders, and the de-identification of files at the source. |
| [Sign-in](sign-in.md) | How users sign in, and how to connect institutional sign-in (SSO, MFA, VPN) through an authenticating reverse proxy. |
| [Privacy and patient data](../PRIVACY.md) | Where identifiers may appear in the programs, and how this repository stays free of patient data. |
| [Security policy](../SECURITY.md) | How to report a vulnerability. |

## Bringing data in

For data managers and database engineers who connect existing databases or build new ones.

| Page | What it covers |
|---|---|
| [Databases](databases.md) | The three ways in (importing tables, connecting an OMOP database, connecting any SQL database through views), what every database must contain, and how to build and check a new database. |
| [Importing](importing.md) | The tables to export, the columns Medorama recognizes, how rows are matched to patients, times and time zones, and linked files. |
| [EDF recordings](edf.md) | Opening EDF, EDF+ and BDF files, what the viewers show, how patient identity in their headers is handled, and the exact reading rules. |
| [WFDB records and collections](wfdb-collection.md) | How WFDB records are read: single records, multi-segment records, folders of records as one session, and the WFDB collection layout for long recordings. |

## Reading formats and integrating

For integrators who add a file format, connect another program or automate Medorama.

| Page | What it covers |
|---|---|
| [Readers and data models](readers.md) | How every format is read by its own reader into one of four data models, where the readers live, and how to add a format. |
| [Adapters](adapters.md) | The adapter protocol: how add-ons read recording formats that the engine does not read itself, such as licensed formats. |
| [HTTP API](api.md) | Medorama's HTTP API, route by route, with its [OpenAPI description](../medorama/openapi.yaml). |
| [Actions](actions.md) | Built-in actions on recordings, runs and jobs, and how to write a plug-in. |

## The viewers

For anyone who uses a viewer, inside Medorama or on its own.

| Page | What it covers |
|---|---|
| [The viewers](../viewers/README.md) | The three viewers on their own and inside Medorama: installing, running, passwords, themes and the addresses they open. |
| [VitalStream](../viewers/vitalstream/README.md) | The waveform viewer for ICU monitoring, Holter and sleep recordings, in Python, Rust and C++. |
| [VitalStream's specifications](../viewers/vitalstream/docs/README.md) | The [data model](../viewers/vitalstream/docs/data-model.md), the [index format](../viewers/vitalstream/docs/index-format.md), the [viewer API](../viewers/vitalstream/docs/viewer-api.md) and the [naming rules](../viewers/vitalstream/docs/naming.md) that the three viewers follow. |
| [ECG viewer](../viewers/ecg/README.md) | 12-lead ECGs on standard paper from WFDB, GE MUSE XML, DICOM, EDF, JSON, pictures and PDF. |
| [PACS viewer](../viewers/pacs/README.md) | DICOM studies, videos, pictures and PDFs, with calipers. |
| [muse-py](../viewers/ecg/python/muse/README.md) and [dicom-py](../viewers/pacs/python/dicom/README.md) | The viewers' Python tools, which serve the pages and build ECG and DICOM databases to import. |

## Developing Medorama

For contributors to Medorama and its viewers.

| Page | What it covers |
|---|---|
| [Developing](developing.md) | How the code is organized, a request end to end, running on synthetic data, finding faults, common changes and the tests. |
| [Contributing](../CONTRIBUTING.md) | The ground rules for changes, setup, coding style and the pre-commit hook. |
| [To do](todo.md) | What is not built yet, each item with its design, steps and tests, and a suggested order. |
| [Libraries](../libs/README.md) | The Rust libraries [`engine`](../libs/engine/README.md) and [`signal-core`](../libs/signal-core/README.md). |
| [Tools](../tools/README.md) | The installers, the demo generator, the screenshot tool and the git hooks. |
| [Continuous integration](../.github/README.md) | The CI workflows, releases and the website build. |

## Design notes

For designers and implementers who want the reasoning behind a part.

| Page | What it covers |
|---|---|
| [The patient page](design/patient-page.md) | The patient page and its Patient Journey: layout, report and detail, viewers, and the light and dark themes. |
| [Sources](design/sources.md) | The control store (SQLite or PostgreSQL), connected databases and their canonical views, OMOP, the person registry, and cohorts across sources. |

## Images

The diagrams in [images/](images) are drawn by [images/diagrams.py](images/diagrams.py): edit
that script and run it rather than editing the SVG files. The screenshots in
[images/screens/](images/screens) are taken by [`tools/screenshots.py`](../tools/screenshots.py)
from the synthetic demo only.
