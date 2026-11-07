# Administrator's guide

This guide is for the people who set up Medorama, bring data into it, manage its users and keep
it running. It assumes Medorama is installed; [getting-started.md](getting-started.md) covers the
first installation and [building.md](building.md) every way of building it. How people use the
page is in the [user's guide](user-guide.md). The security design, for the reviewers who must
approve a deployment, is in [security.md](security.md), and how patient identifiers are hidden is
in [protecting-patient-data.md](protecting-patient-data.md).

## Contents

- [Rules](#rules)
- [Set up a catalog](#set-up-a-catalog)
- [Settings](#settings)
- [Put the catalog in PostgreSQL](#put-the-catalog-in-postgresql)
- [Run the server](#run-the-server)
- [Command reference](#command-reference)
- [Users, roles and the PHI permission](#users-roles-and-the-phi-permission)
- [Choose the sign-in method](#choose-the-sign-in-method)
- [Bringing data in](#bringing-data-in): [Databases](#databases) ·
  [Files and recordings](#files-and-recordings) · [Import formats](#import-formats) ·
  [Column mappings](#column-mappings) · [Imports and sources](#imports-and-sources)
- [Patients and encounters across databases](#patients-and-encounters-across-databases):
  [Linking recordings](#linking-recordings) ·
  [Identifiers in recordings](#identifiers-in-recordings) · [The Review tab](#the-review-tab)
- [Connected databases](#connected-databases)
- [Add-ons](#add-ons)
- [The waveform viewer](#the-waveform-viewer)
- [The audit log](#the-audit-log)
- [The database](#the-database)
- [Deployment](#deployment): [On a shared Linux cluster](#on-a-shared-linux-cluster) ·
  [Run as a service](#run-as-a-service)
- [Backup, update, uninstall](#backup-update-uninstall)
- [The demo](#the-demo)
- [Troubleshooting](#troubleshooting)

## Rules

Medorama keeps the following rules in every deployment. They are what a reviewer can rely on,
and every change to the code must keep them.

1. **Recordings are only read.** The catalog stores the paths of recordings, never copies of
   them. It never writes, moves or deletes a recording, and it never writes into a recording's
   folder: neither the folder that holds a recording file nor anywhere inside a WFDB folder. An
   import is refused if the catalog's own folders lie in such a position.
2. **Removing data removes catalog entries only.** Removing a recording, an encounter, an import
   or a source deletes entries in the catalog's database. With the *purge* option, the catalog's
   own outputs for a recording are deleted too, but only inside `results_dir` and `index_dir`.
   Every removal can be previewed first.
3. **Only administrators add or remove data and users.**
4. **A file comes in only with its patient.** Databases are the only way in. A recording, an
   ECG, an image or any other file enters as a row that names its patient (by patient ID, MRN or
   encounter) and links the file. No file is imported on its own.
5. **Identifiers live only in the catalog's database.** MRNs, names, dates of birth, encounter
   numbers, and what the recordings' own files say about the patient are protected health
   information (PHI). They are sent, shown and searchable only for users with the PHI permission
   who have switched Privacy Mode off, and those users' searches and views are written to the
   audit log. The viewers, the job descriptions given to actions, the results folders and the
   indexes carry pseudonyms only.
6. **Outputs go to configured folders.** Everything the catalog writes goes to `results_dir`,
   `index_dir` or its home folder, all chosen when the catalog is created.

## Set up a catalog

A **catalog** is one Medorama installation's data: its settings, its database and its output
folders. Before you create one, choose two places:

| Folder | What goes there | Advice |
|---|---|---|
| **Home folder** | `medorama.toml` (the settings), `medorama.sqlite` (the database), `actions/` (action plug-ins), `formats/` (import formats). | Put it on a **local disk**, because SQLite's locking is unreliable on network file systems such as NFS or SMB home folders. Make it private to the account that runs the catalog, and encrypted, because the database holds identifiers. The default is `~/.local/share/medorama` (Windows: `%LOCALAPPDATA%\medorama`). |
| **Results folder** | `index/` (the recordings' indexes) and `actions/` (the results of actions). | Leave room for the indexes, roughly 1 to 5 % of the recordings' size. Never choose a folder that holds recordings. |

To create a catalog in the default home folder, or in a home folder of your own, run one of:

```bash
medorama init --results /data/medorama-results
medorama init --results /data/medorama-results --home ~/medorama
```

`init` asks twice for the password of the first user, `admin`, who has the PHI permission. The
password needs at least 10 characters.

| `init` option | Default | Meaning |
|---|---|---|
| `--home DIR` | See below. | Where the catalog lives. It is remembered for later commands. It is a global option, which every other command accepts too. |
| `--results DIR` | Required. | The results folder. |
| `--index-dir DIR` | `<results>/index` | Where indexes go. Point it at a viewer's index folder to share its indexes. |
| `--admin NAME` | `admin` | The first user's name. |
| `--timezone TZ` | `America/New_York` | The time zone for times in tables written without an offset, and for display. |
| `--port N` | 8090 | The port of the web page and the API. |
| `--password-stdin` | | Reads the password from standard input, for scripts. |
| `--database-url URL`, `--database-password-file FILE` | SQLite in the home folder | Puts the catalog in PostgreSQL ([Put the catalog in PostgreSQL](#put-the-catalog-in-postgresql)). |
| `--no-demo` | | Leaves out the demo that the installer made with `--demo` ([The demo](#the-demo)). Without that installer option there is nothing to leave out. |

**Which catalog a command uses.** A command uses the first of these that applies:
1. the `--home DIR` option;
2. the environment variable `$MEDORAMA_HOME`;
3. the catalog that `init` created last, or the one chosen with `medorama --home DIR use`
   (remembered in `~/.config/medorama/home`, on Windows `%APPDATA%\medorama\home`);
4. `~/.local/share/medorama` (on Windows `%LOCALAPPDATA%\medorama`).

`serve` prints the catalog it uses. The home folder is readable by its owner only.

## Settings

The settings are in `<home>/medorama.toml`. `init` writes the file with a comment above each
setting. Relative paths are relative to the home folder. Restart the server after changing the
file. An unknown key is an error: the command or the server stops and names it, so that a typing
mistake is never silently ignored.

| Key | Default | Meaning |
|---|---|---|
| `results_dir` | Set by `init`. | Where actions write their results: `actions/<action>/r<recording>/j<job>/`. |
| `index_dir` | `<results_dir>/index` | The recordings' indexes, one folder per index key. |
| `adapters_dir` | `~/.local/share/medorama/adapters` | The adapter folder ([Add-ons](#add-ons)). |
| `database` | `medorama.sqlite` | The SQLite database file. It holds identifiers. |
| `database_url` | Empty | A PostgreSQL database instead, such as `postgres://user@host/db?sslmode=require`, written without its password. `$MEDORAMA_DATABASE_URL` overrides it. |
| `database_password_file` | None | The file that holds that database's password. `$MEDORAMA_DATABASE_PASSWORD` overrides it. |
| `database_ca_file` | None | Certificate authorities (PEM) trusted for the database server's certificate, in addition to the system's. |
| `host`, `port` | `127.0.0.1`, `8090` | The address and port of the web page and the API. |
| `workers` | 2 | The number of jobs that run at the same time. A value below 1 is read as 1. |
| `auto_index` | `true` | While the server runs, it queues indexing of new and changed recordings. |
| `check_interval_s` | 900 | How often, in seconds, every recording is checked again; 0 means never. |
| `timezone` | `America/New_York` | The time zone of times written without an offset, and of the times shown. |
| `viewer` | `auto` | The VitalStream viewer that Medorama runs: `auto`, `cpp`, `rs`, `py`, `none`, or the path of a viewer program ([The waveform viewer](#the-waveform-viewer)). |
| `viewer_url` | Empty | Instead, a viewer running elsewhere, opened in a new browser tab. |
| `qc_verbose` | `false` | The viewer that the catalog runs also outlines, with a dashed line, the data a reader repaired, such as data blocks that a file stored out of order and the reader put back in order. Breaks are shaded either way. |
| `data_roots` | `[]` (anywhere) | The folders that files may be imported from. |
| `actions_dirs` | `["actions"]` | The folders of action plug-ins ([actions.md](actions.md)). |
| `formats_dirs` | `["formats"]` | The folders of import formats ([Import formats](#import-formats)). |
| `session_hours` | 12 | How long a sign-in lasts. A value below 1 is read as 1. |
| `session_idle_minutes` | 60 | A sign-in that is not used for this long ends; 0 means only `session_hours` ends it. The page's own background checks for new data do not count as use. |
| `secure_cookies` | `false` | Set to `true` when Medorama is served over HTTPS. |
| `sign_in` | `password` | How users sign in: `password` (Medorama's own accounts) or `proxy` (the institution's authenticating reverse proxy). See [Choose the sign-in method](#choose-the-sign-in-method). |
| `proxy_user_header` | `X-Remote-User` | With `sign_in = "proxy"`: the request header in which the proxy names the user. It must be a valid header name. |
| `proxy_addresses` | `["127.0.0.1", "::1"]` | With `sign_in = "proxy"`: the IP addresses the proxy connects from. The header is believed only on requests from these addresses. Each entry must be an IP address, not a host name, and the list must not be empty. |
| `proxy_logout_url` | Empty | With `sign_in = "proxy"`: where **Sign out** sends the browser, normally the proxy's own sign-out address, such as `/oauth2/sign_out`. |
| `key_file` | None | The database key, for builds with encryption ([The database](#the-database)). |
| `audit_export` | None | A file to which the audit log's sealed entries are appended, on storage that the database's administrators cannot change ([The audit log](#the-audit-log)). |

Setting `proxy_user_header`, `proxy_addresses` or `proxy_logout_url` without
`sign_in = "proxy"` is an error, so that a half-finished change to the sign-in method stops the
server instead of being ignored.

## Put the catalog in PostgreSQL

SQLite, the default, suits one server: there is nothing to install, and the file can be
encrypted. Put the catalog in **PostgreSQL** (version 14 or later) to run several Medorama
servers on one catalog, to serve many users or large catalogs, or where the institution already
runs PostgreSQL (on-site, Amazon RDS or Aurora, Azure Database for PostgreSQL). The same program
supports both, and every test runs on both.

1. Create an empty database, with an account for Medorama alone that owns it:
   ```sql
   CREATE ROLE medorama LOGIN PASSWORD '…';
   CREATE DATABASE medorama OWNER medorama;
   \c medorama
   CREATE EXTENSION IF NOT EXISTS pg_trgm;   -- the text search's index (needs a superuser once)
   ```
2. Store the password in a file that only the catalog's account can read (`chmod 600`), or in
   `$MEDORAMA_DATABASE_PASSWORD`. Never put it in the URL.
3. Create the catalog. Its settings and folders stay in the home folder:
   ```bash
   medorama init --results /data/medorama-results \
       --database-url "postgres://medorama@db.example.org/medorama" --database-password-file /etc/medorama/db-password
   ```
   A database server on another host must offer TLS. Medorama requires it, unless the URL says
   `sslmode=disable`, and verifies the server's certificate and name. `database_ca_file` adds a
   certificate authority, such as a private CA or Amazon RDS's bundle.
4. To run several servers, give each the same `medorama.toml` (the same `database_url` and
   `data_roots`, and the results and index folders on shared storage) and put them behind a
   reverse proxy. Any server answers any request, and the servers take jobs one at a time from
   the shared queue.

Each server opens at most 32 connections to the database (a request waits for one beyond that),
checks a connection that was idle before using it again, and, unless the URL sets them, gives
each connection 10 seconds to connect, TCP keepalives, and 5 minutes for a statement
(`?connect_timeout=…`, `keepalives=0`, `options=-c statement_timeout=…` in the URL change them).
The URL holds no password, neither before the host (`user:password@`) nor as a parameter
(`?password=`): Medorama refuses one that does.

**Move a SQLite catalog to PostgreSQL.** The following command copies every table and row in one
transaction (each table read a range of rows at a time, never whole), builds each database's
table of numbers (`dbcol_<import>`) again in PostgreSQL, and prints the lines to add to
`medorama.toml`:

```bash
medorama copy-database --to postgres://medorama@db.example.org/medorama --password-file FILE
```

The SQLite file is left as it was. Keep it as a backup, then delete it securely.

## Run the server

This command starts the web page, the API and the background work:

```bash
medorama serve
```

Open <http://127.0.0.1:8090> and sign in. Ctrl+C (or the SIGTERM a service manager sends) stops
the server: its plug-ins are stopped (TERM to each one's process group, KILL 5 seconds later) and
their jobs go back to the queue, and so does `medorama work`'s. At start-up, an import that a
stop or a crash interrupted is removed ([Imports and sources](#imports-and-sources)). While the
server runs:
- **workers** (the `workers` setting) run queued jobs;
- **automatic indexing** queues new and changed recordings within about 15 seconds;
- every recording is **checked again** every `check_interval_s` seconds, which finds missing and
  changed recordings and lost indexes.

The commands can do the same work without the server: `medorama run index --all` queues indexing
of every recording, and `medorama work` runs the queue in the terminal until it is empty.
`serve --no-workers` serves the page only, while another process runs `work`.

## Command reference

`--home DIR` is a global option: every command accepts it, before or after the command's name.
`medorama <command> --help` shows a command's options.

| Command | What it does |
|---|---|
| `init --results DIR …` | Creates a catalog. |
| `--home DIR use` | Makes that catalog the default for later commands. |
| `serve [--host H] [--port N] [--no-workers]` | Runs the web page, the API and the background work. |
| `import-table FILE… [--name N] [--source S] [--category C] [--mapping F] [--format F] [--dry-run]` | Imports a database ([Databases](#databases)). |
| `import-clif FOLDER [--source S] [--dry-run]` | Imports a CLIF folder, each table as a database. Unlike the page and the API, the command does not check the folder against `data_roots`. |
| `formats [--check FILE]` | Lists the import formats, or checks a format file. |
| `imports list`, `imports remove ID [--purge-results] [--dry-run]` | Lists the imports, or removes one with what it added. |
| `sources list`, `sources remove NAME [--purge-results] [--dry-run]` | Lists the sources, or removes everything that came from one. |
| `connections add NAME --kind omop\|sql --url URL …`, `list`, `check NAME`, `sync NAME`, `remove NAME` | Manages [connected databases](#connected-databases). |
| `demo add [DIR]`, `demo remove [--dry-run]` | Loads [the demo](#the-demo) (the installer's, or a folder made by `tools/make-demo-data.py`), or removes it with everything it added. |
| `addons list`, `addons install PROGRAM`, `addons remove NAME` | Manages [add-ons](#add-ons). |
| `list [--q SEARCH] [--unit U] [--status S] [--limit N] [--phi] [--json]` | Lists encounters ([search syntax](user-guide.md#searching)). `--limit` defaults to 100. |
| `recordings [--q SEARCH] [--unlinked] [--limit N] [--json]` | Lists recordings. `--limit` defaults to 100. |
| `remove ID\|PATH… [--search Q] [--purge-results] [--dry-run]` | Removes recordings from the catalog. The files are not touched. |
| `remove-encounters ID…\|--search Q\|--all [--dry-run]` | Removes encounters with their bed stays and items. Their recordings stay. |
| `check` | Checks the recordings again: missing, changed, lost indexes. |
| `actions` | Lists the installed actions. |
| `run ACTION… --all\|--encounter ID…\|--recording ID…\|--search TEXT\|--cohort N [--force] [--param A.K=V] [--priority N]` | Queues actions. |
| `work [--workers N]` | Runs the queue in this process until it is empty. |
| `jobs [--state S] [--limit N] [--json]`, `cancel --job N\|--run N\|--all` | Lists the queue (`--limit` defaults to 50), or cancels jobs. `cancel --job N --run M` cancels job N only if it belongs to run M. |
| `user add NAME --role R [--phi] [--display-name D] [--password-stdin \| --no-password]`, `user list`, `user set NAME [--role R] [--phi true\|false] [--disabled true\|false] [--display-name D] [--password\|--password-stdin]`, `user remove NAME` | Manages users. The `--phi` and `--disabled` options of `user set` take a value, as in `--disabled true`. |
| `audit [--limit N]`, `audit --verify`, `audit --seal`, `audit --seal --verify` | Shows the audit log (`--limit` defaults to 50), checks whether its seals hold, seals it now, or seals and then checks. |
| `copy-database --to URL [--password-file F] [--ca-file F]` | Copies a SQLite catalog into an empty PostgreSQL database. The password comes from the file, or else from `$MEDORAMA_DATABASE_PASSWORD`; `--ca-file` names certificate authorities (PEM) in addition to the system's. |

**The rights of commands.** The commands act directly on the database, with administrator rights
and the PHI permission, as the operating-system user who can read the home folder. The audit log
records them as `local:<user>`. `list` shows identifiers only with `--phi`.

## Users, roles and the PHI permission

Every user has a **role**:

| Role | May |
|---|---|
| `viewer` | Search and look at patients, encounters, recordings, databases and results; keep a history; make cohorts. |
| `analyst` | Everything a viewer may do, and run and cancel actions. |
| `admin` | Everything an analyst may do, and import and remove data, review, manage users and settings, and read the audit log. |

**The PHI permission** is given per user, in addition to the role. It lets the user leave
Privacy Mode, and then see and search MRNs, names, dates of birth, account and encounter numbers
and real dates, and read notes. Without it, every session of the user is in Privacy Mode,
administrators' sessions included. With it, a session starts in Privacy Mode, except an
administrator's, which starts out of it. The **Privacy Mode** switch in the page's header changes
the mode, and the choice lasts until the user signs out (a cookie that lasts `session_hours`).

To add users and reset a password from the command line:

```bash
medorama user add alice --role analyst          # asks for her password
medorama user add bob --role viewer --phi       # may see identifiers
medorama user set alice --password              # a forgotten password
```

Admin › Users does the same in the page.

**Accounts and passwords.** `init` creates the first administrator, who has the PHI permission.
Passwords have at least 10 characters and are stored as Argon2id hashes. After 5 failed sign-ins,
a user name is locked for one minute. The catalog refuses any change that would leave it without
an enabled administrator.

**Sessions.** A session is a random 256-bit token in an HttpOnly, SameSite=Strict cookie, and the
database stores only its SHA-256 hash. Changing a user's password or disabling the user ends that
user's sessions. **Sign out** in Admin › Users ends a user's sessions everywhere.

### Privacy Mode's settings

Admin › Privacy Mode chooses what Privacy Mode withholds and how it moves dates. By default it
withholds what HIPAA's Safe Harbor method lists, except the text of notes. Dates move back by the
catalog's offset (by default 36,570 days; it can be given in years, months, days and hours) plus
each patient's own number of days, between 0 and `spread_days` (by default 365, at least 30, at most 3,650), drawn once per
patient and kept with the patient's pseudonym. How the privacy gate applies these settings is
explained in [protecting-patient-data.md](protecting-patient-data.md).

Admin › Dates and times chooses how every page writes dates and times (2024-01-31, 01/31/2024,
31/01/2024 or 31.01.2024, with a 24-hour or a 12-hour clock). Exports and the API always use
ISO 8601.

## Choose the sign-in method

Medorama has two sign-in methods, chosen by the `sign_in` setting. Whichever you choose, accounts,
their roles and the PHI permission stay in Medorama, and everything after sign-in (what a user may
see and do, Privacy Mode, the audit log) works the same way. [sign-in.md](sign-in.md) explains
both methods in full, gives proxy configurations for common single sign-on products, and shows
how to add another method.

| `sign_in` | Who signs the user in | Use it when |
|---|---|---|
| `password` (default) | Medorama, with its own accounts and passwords ([above](#users-roles-and-the-phi-permission)). | You run Medorama for yourself or a small team, or the institution has no sign-in service to connect to. |
| `proxy` | The institution's **authenticating reverse proxy**, such as oauth2-proxy, Apache mod_auth_openidc, Shibboleth, nginx `auth_request` or a VPN gateway. It signs the user in (single sign-on, multi-factor authentication) and passes the user's name to Medorama in a header. | The institution requires its own sign-in, for example Google, Microsoft Entra ID or Okta through OpenID Connect, or SAML. |

To use an authenticating proxy:

1. Configure the proxy to authenticate every request, to set the header (`X-Remote-User` by
   default) to the user's name on every request it forwards, and to remove any header of that
   name that the browser sent.
2. Create an account in Medorama for each user, named exactly as the proxy names them (the
   comparison ignores letter case), with its role and, where appropriate, the PHI permission.
   Include at least one administrator; for a new catalog, `medorama init --admin NAME` gives the
   first one that name. Medorama's passwords are not used while `sign_in = "proxy"`: add
   accounts with `medorama user add NAME --role ROLE --no-password`, which gives them a random
   password that nobody knows. Admin › Users does the same, and hides its password field.
3. Set the method in `medorama.toml` and restart the server:
   ```toml
   sign_in = "proxy"
   proxy_user_header = "X-Remote-User"        # the header the proxy sets
   proxy_addresses = ["127.0.0.1", "::1"]     # the addresses the proxy connects from
   proxy_logout_url = "/oauth2/sign_out"      # the proxy's own sign-out
   secure_cookies = true
   ```

With `sign_in = "proxy"`:
- Medorama believes the header only on requests that come from `proxy_addresses`. A request from
  any other address is treated as not signed in, whatever headers it carries.
- The password form is not shown, and `POST login` and password changes are refused.
- A user whom the proxy names but who has no enabled account in Medorama is refused (HTTP 403),
  and the page explains that an administrator must create the account.
- **Sign out** sends the browser to `proxy_logout_url`.
- The audit log records one `login` per visit, and one `login_failed` for a name without an
  account.

**Protect the port.** The default `proxy_addresses` trust any connection from the machine itself.
Run Medorama in proxy mode only on a machine where untrusted people cannot open connections to
its port, such as a dedicated server. If the proxy runs on another host, set `host` to an address
that only the proxy can reach, and `proxy_addresses` to the proxy's address.
[sign-in.md](sign-in.md#security-requirements-for-proxy-sign-in) lists every condition that proxy
sign-in depends on, and how to check that the header cannot be forged.

## Bringing data in

Every source of data about patients is a **database**. Data come into Medorama in two ways:
**imported** tables, which are copied into the catalog, and **connected databases**, which
Medorama reads where they are. [databases.md](databases.md) helps you choose between them and
shows how to build a new database for Medorama; [importing.md](importing.md) explains how to
prepare tables, column by column, with examples. This section is the reference for how Medorama
reads imported tables.

### Databases

An imported database is a CSV file, a Parquet file or a spreadsheet (Excel `.xlsx` or `.xls`,
OpenDocument `.ods`; the first sheet), imported as one batch. A database can also be read where
it is ([Connected databases](#connected-databases)).

- **In the page:** choose Import › **Add a database** › **Import tables** and choose the files.
  Medorama recognizes the kind of data and the format. Each catalog column has a drop-down list
  filled in from the table's headers. Choose **Preview all**, then **Import all**. A CLIF folder on
  the server can be imported from the same place. An import from the page runs in the background,
  shows its progress on every page, and can be stopped with **Cancel**; nothing of a cancelled
  import is kept.
- **From the command line:** run `medorama import-table FILE…`. Several files with the same
  columns make one database (`--name`). Each file is read row by row, so a database of millions
  of rows needs little memory.

For example:

```bash
medorama import-table encounters.xlsx                    # patients, encounters, bed stays
medorama import-table labs.csv --source labs-2025        # results; the category from the file name
medorama import-table ecg_index.csv --category ecg       # rows with a `file` column: ECGs, pictures, PDFs
medorama import-table icu_index.csv                      # rows linking recordings to their encounters
medorama import-table muse/*.csv --name muse_ecg         # several files, one database (a known export, converted)
medorama import-clif /data/clif                          # a CLIF folder (CSV or Parquet)
```

Medorama recognizes columns by their names (`Test Name` as the name, `Result` as the value,
`DOB` as the date of birth, and so on), or through a mapping ([Column mappings](#column-mappings)).
Each row may carry any combination of these groups of columns:

| Group | Columns | What they become |
|---|---|---|
| Patient | `patient_id`, `mrn`, `mrn_authority`, `last_name`, `first_name`, `birth_date`, `sex` | The patient. Names, MRNs and dates of birth are PHI. |
| Encounter | `encounter_id`, `csn`, `admit_time`, `discharge_time`, `admit_unit`, `disposition` | An encounter (a visit or a stay). |
| Location | `care_unit`, `bed`, `bed_start`, `bed_end`, `recording_file` | A bed stay, which links recordings by bed and time, or by file name. |
| Item | `category`, `name`, `time`, `end_time`, `value`, `unit`, `ref_low`, `ref_high`, `flag`, `code`, `vocabulary`, `text`, `author`, `service`, `domain` | An item: a result, a vital sign, a drug, a code, a note or a report. |
| File | `file` | A recording or an attachment ([Files and recordings](#files-and-recordings)). |
| Other | `source` | The row's source label. |

Medorama reads each row as follows:
- **A patient or an encounter.** A row describes a patient or an encounter when it names one
  (`patient_id` or `mrn`) and describes it, or holds nothing else
  ([Patients and encounters across databases](#patients-and-encounters-across-databases)).
- **An item.** A row is an item when it has a name, a value, a text or a file. The item is
  matched to its encounter (by `encounter_id`, else `csn`), or else to its patient (by `mrn`, else
  `patient_id`) and its time: the patient's encounter whose stay covers that time. An item of a
  known patient that falls outside every encounter stays with the patient. An item whose patient
  or encounter no database has named yet **waits**, and is matched as soon as another database
  names them. Databases can therefore be imported in any order.
- **Values.** A number is stored as a number, and `ref_low` and `ref_high` give its normal range.
  `flag` (`H`, `L`, `HH`, `LL`, `A`, `critical`, `positive` and so on) marks a value abnormal or
  critical, as does a value outside its range. A row identical to one already stored is not
  stored again; the item is then held by both databases.
- **Times.** A time can be written in ISO 8601 with an offset (`2024-03-02T07:10:00-05:00`, or
  ending in `Z`), as a local time in the catalog's time zone (`2024-03-02 07:10`,
  `03/02/2024 07:10`), as a date (read as local midnight), or as Unix seconds.
- **Categories.** Each item falls into the category that its `category` column names; else the
  one chosen for the table (`--category`); else the one its file name suggests (`labs.csv`,
  `ecg_index.csv`); else the one its domain and name suggest.

| Category | What it holds |
|---|---|
| ICU | Stays in intensive-care units, with the bedside recordings made during them. |
| Vitals, Labs, Microbiology, Pathology | Charted vital signs; laboratory results; cultures; biopsies and cytology. |
| ECG, Holter | Resting 12-lead ECGs; ambulatory ECG recordings. |
| EEG, Sleep | EEG recordings; sleep studies (polysomnography). |
| Echo, Cath lab, Imaging | Echocardiography; angiography; radiology, cardiac CT and MR, nuclear medicine. |
| Procedures, Medications, Transfusions, Life support | Procedures; drugs; blood products; ventilation, dialysis, ECMO. |
| Conditions, Assessments, Notes, Allergies | Diagnoses (ICD codes); scores; clinical notes and reports; allergies (shown with the patient). |

### Files and recordings

A row's `file` column names a file. A relative path is read from the table's folder, and the file
must lie inside `data_roots`. Files are only read, and they are shown only to users who may see
the patient.

| The row's category | The `file` is | It becomes |
|---|---|---|
| ICU, Holter, EEG, Sleep, or none | A WFDB folder, a WFDB header, an EDF or BDF file, or a file that an installed adapter reads | A **recording**, linked to the row's encounter, indexed in the background and shown in VitalStream. |
| ECG | A WFDB header, GE MUSE XML, a DICOM ECG, a JSON ECG, an EDF file, PNG, JPEG or PDF | The item's attachment, shown in the ECG viewer. |
| Echo, Cath lab, Imaging | DICOM files and series (a folder), videos, pictures (a folder of pictures with a frame rate is one loop), PDF | The item's attachment, shown in the PACS viewer. |
| Any other | A picture, a PDF, a video or a text file | The item's attachment, shown by the page. |

The scale of a picture or video and the frame rate of a loop come from the row's columns
(`pixel spacing (mm)`, `frame rate (fps)`).

**How Medorama finds recordings.** A folder named in a row is itself a recording when it is a
WFDB folder. It is then read with its subfolders, as the recording engine reads it, within the
engine's folder limits: 5000 records, 20 levels, and 10 seconds to count them. Medorama has no
setting for these limits; the VitalStream viewers do
([wfdb-collection.md, Limits](wfdb-collection.md#limits)). Any other folder adds only the
recordings directly inside it (WFDB records, WFDB folders, EDF and BDF files, files an add-on
reads), skipping hidden names; a subfolder that is not itself a WFDB folder is not searched. A
folder of EDF files is not yet read as one session: each file is a recording of its own. Paths
are stored with symbolic links resolved, so a recording is never in the catalog twice.

Each recording gets a fingerprint, its **index key**, which names its folder in the index folder.
A recording whose current index is already in `index_dir` is marked indexed at once. A **check**
(Import › Recordings › Re-check the files, `medorama check`, and every `check_interval_s`)
finds missing recordings, which stay in the catalog, marked, until an administrator removes them,
changed recordings and lost indexes.

### Import formats

Medorama recognizes some known exports by their columns, and converts them as it imports them.
The `--format auto|none|<format>` option chooses, and the page labels such a file.

| Format | What it reads | What it becomes |
|---|---|---|
| `ge-muse` | GE MUSE ECG metadata: one row per resting ECG, with its XML file's elements as columns. | One ECG per row, with its file linked and the patient found by MRN. The measurements become columns with their units (`QTc (ms)`), and the statements become text. |
| `ge-muse-patient-id` | The same, with the patient found by a patient ID. | |
| `clif-…` | [CLIF](https://github.com/Common-Longitudinal-ICU-data-Format) tables (CSV or Parquet): every table of CLIF 2.1, and CLIF 3.0's `input`, `output`, `intermittent_dialysis`, `validated_diagnosis` and `clinical_notes_facts`. | Patients, hospitalizations (encounters) and ADT (bed stays); vital signs, with invasive hemodynamics and intake and output; results; life support (ventilation, ECMO and MCS, CRRT, intermittent dialysis); drugs given and ordered; transfusions; assessments, with ICU orders, therapy sessions, the care team's roles, area indices and trial enrollment; cultures, non-culture tests and susceptibilities (each on its culture, by `organism_id`, when the folder holds the cultures); procedures and conditions by their codes; and the facts of notes (CLIF holds no note text). Left out: geography, the age at admission, devices' serial IDs, staff IDs and `lda_id`. |

**A site's own formats** are TOML files in a folder of `formats_dirs`, one file per format, which
describe the export column by column. They need no new build of Medorama. The full description
of the file is in [`declared.rs`](../medorama/src/formats/declared.rs). For example:

```toml
key = "acme-ecg"                  # --format acme-ecg
label = "ACME ECG export"
category = "ecg"
detect = ["EcgFile", "PatId", "AcqDate"]   # recognized when a table has all these columns
required = ["mrn", "file"]        # rows with these empty are left out (counted)

[[column]]
name = "mrn"                      # a catalog column, or one of the database's own
from = "PatId"
[[column]]
name = "time"
from = "AcqDate"
date = "%m-%d-%Y"                 # read in this form, written as ISO 8601
time_from = "AcqTime"
time = "%H:%M:%S"
[[column]]
name = "QTc (ms)"                 # a measurement: its unit in its name
from = "QTc"
number = true
[[column]]
name = "interpretation"
join = "Statement/{n}/Text"       # numbered columns, in order, joined
```

A column that a format does not describe is left out. `medorama formats` lists the formats, and
the files it could not read; `medorama formats --check FILE` checks a format file before you
install it.

### Column mappings

An export with other column names is read through a **mapping**, which names the header that
holds each catalog column. In the page, each catalog column has a drop-down list, filled in when
the names match. A mapping can be saved by name and chosen again for the next export of the same
kind. On the command line, a mapping is a TOML file given with `--mapping FILE`:

```toml
[columns]
patient_id = "PAT_ID"
mrn = "PAT_MRN_ID"
encounter_id = "PAT_ENC_CSN_ID"
admit_time = "HOSP_ADMSN_TIME"
```

The keys may also stand at the top level, without `[columns]`, in a file that holds nothing
else. Headers are matched without regard to letter case. With a mapping, Medorama does not guess
column names from their synonyms: a header is read as a catalog column only if the mapping names
it for that column, or if it is that column's own name (such as `mrn` or `TIME`) and the mapping
names no other header for it. The import reports the other columns as ignored by the catalog;
they are still kept with the database, under their own names.

### Imports and sources

Each import is a **batch** with a name and a **source** label. Both default to the table's file
name, and a `source` column overrides the label row by row. **Medorama records what each batch
added.** The same recording, patient, encounter or item may come from several batches, and so may
its values: each batch's name, date of birth, admission time and so on are kept apart.

| Remove | What is taken out | What stays |
|---|---|---|
| A batch (Import › Imports, `imports remove ID`) | What the batch added, its values and its held-back rows. | Anything another batch also has; the files on disk. |
| A source (Import › Sources, `sources remove NAME`) | What every batch added under that source. | The same. |
| The encounters a search finds (`remove-encounters --search`) | Those encounters, with their bed stays and items. | Their recordings, which become unlinked. |
| The recordings a search finds (`remove --search`) | Those recordings, and optionally their indexes and results. | The encounters. |

After a removal, the fields of what stays show the values the remaining batches gave, links are
computed again (decisions made manually still apply; the links the batch's rows gave its
recordings go), a generated patient ID (`GEN-…`) that the batch replaced comes back where no
other batch names the patient by the ID it gave, and items whose encounter was removed go back
to their patient or wait. Removing a connected database's batch disconnects it (its password
file in `<home>/secrets` goes too). Every import and removal can be previewed: with `--dry-run`
on the command line, and with the Preview and Remove confirmations in the page.

**Imports in progress.** Imports run one at a time, across the server and the commands; one that
starts while another runs waits. A batch is *loading* until its import ends. Until then, what
only it holds (patients, encounters, bed stays, recordings and items) is not shown: the patient
list and pages, encounters, recordings, cohorts, exports and the Databases tab leave it out, and
it appears when the import ends. An item or a patient that an earlier database also holds stays
shown; new values it gives a patient already shown (a name, a sex, an admission time) appear at
once. On SQLite, a table
is committed a chunk of rows at a time, so that the database's one write lock is free between
chunks (other changes and the audit log are written meanwhile); an import that fails, is
cancelled or is stopped part way is removed as a whole, at once or when the catalog next starts,
so that nothing of it is kept. A failure after the import is stored (linking recordings, matching
waiting items) is a warning of its report, done again by the next import.

## Patients and encounters across databases

The catalog keeps one patient as one patient across databases, in the way a master patient index
does: it keeps what each database said, computes the catalog's view from that, and keeps
decisions apart from the data.

- **Matching patients.** Rows are matched to a patient by `patient_id` (or an ID merged into
  another one), or else by an MRN of the same authority (`mrn_authority`), ignoring letter case
  and surrounding spaces. A row with an MRN but no `patient_id` gets a pseudonymous ID from the
  catalog (`GEN-000001` and so on), and the same MRN gets the same ID again after any removal and
  re-import.
- **Values.** Each field shows the newest batch's value; removing that batch brings back the
  previous one. A patient may have MRNs of several authorities, and `mrn:` searches find any of
  them.
- **Held-back rows.** A row that contradicts the catalog is held back: it is kept, with a
  warning, for the [Review tab](#the-review-tab), and counted as a conflict in the preview.

| Problem | What applying the row does |
|---|---|
| Its `patient_id` and MRN belong to two different patients. | Merges the MRN's patient into the ID's patient. |
| Its MRN belongs to a patient with another `patient_id`. | Records the two IDs as one person. |
| The patient already has another MRN of the same authority. | Adds the MRN. |
| Its encounter belongs to another patient. | Moves the encounter, with its items. |

The catalog never reassigns a patient on its own on the strength of an MRN. **A merge** moves the
patient's encounters, MRNs, values, items and sources to the other patient, and later rows that
name the merged ID go to the other patient. A merge cannot be undone.

### Linking recordings

After every import and every indexing, Medorama links each recording to an encounter:
1. **By a database row** that names the recording's file and the encounter (the `file` column), or
   by a bed stay's `recording_file`.
2. **By bed and time.** Bedside-monitor systems name their files `UNIT_BED-<Unix time>`. A
   recording is linked when the unit and bed in its name equal a bed stay's `care_unit`, `_` and
   `bed` (ignoring letter case), and its time span overlaps the stay. Before indexing, the time in
   the name is used; after indexing, the recording's real start and end.

**Decisions made manually** override both rules, and they outlive removing and re-importing the
databases. **Link** gives the recording exactly its manual links. **Unlink** ("never") means the
recording is never linked to that encounter automatically. Recordings that match nothing are
listed under Import › Recordings › "Not linked to an encounter".

### Identifiers in recordings

Many recordings store patient identifiers inside the file itself. The built-in action `identify`
reads them, once per recording and again whenever the file changes, so that a reviewer can check
the databases against what the files say. Medorama queues it for every recording automatically.

| Recording format | What `identify` reads |
|---|---|
| WFDB records | Nothing: WFDB headers hold no patient identifiers. |
| EDF, EDF+, BDF, BDF+ | The header's patient and recording fields ([medorama/src/identify.rs](../medorama/src/identify.rs), `from_edf`). In an EDF+ patient field (`code sex birthdate name`), the code becomes the patient ID, and the sex, the birth date and the name (with underscores read as spaces) are kept; `X` marks a part that is unknown. The admin code of an EDF+ recording field (`Startdate dd-MMM-yyyy admincode technician equipment`) becomes the encounter ID. A plain EDF patient field, not structured as EDF+, is kept whole as the patient ID. |
| Formats read by an add-on | What the add-on's adapter reports: names, patient and encounter IDs, birth date and sex ([adapters.md](adapters.md)). |

A file that names several patients is flagged. The values are kept only in the
`recording_identifiers` table of the catalog's database. They are shown only to users with the
PHI permission out of Privacy Mode: in the Encounters list (**In the files**), the encounter
panel, Import › Recordings, and the searches `file-name:`, `file-id:` and `file-dob:`. They never
reach job summaries, the viewers, the job descriptions given to actions, the results folders or
the logs. The EDF header fields are also never part of a recording's index, and in Privacy Mode
the privacy gate serves EDF files with these fields replaced ([edf.md](edf.md)).

### The Review tab

The Review tab lists what needs a person's decision.

| List | What it shows | Decisions |
|---|---|---|
| Held-back rows | Rows in conflict with the catalog. | Apply the row (the fix is shown), or dismiss it. |
| Possible duplicate patients (users with the PHI permission) | Patients with the same name and date of birth; with first and last names swapped; with day and month of birth swapped; or with the same last name, first initial and date of birth. | Merge into either one, or mark them as different people (never suggested again). |
| Recordings whose files disagree (users with the PHI permission) | Automatic links where the recording's file names another patient. | Keep the link (it becomes manual), or unlink. |
| Items waiting | Items whose patient or encounter no database names yet. | None: they are matched when a database names them. |

Every decision is recorded in the audit log.

## Connected databases

A **connected database** is read where it is, at each request, beside the catalog: an OMOP CDM
instance, a research warehouse, another hospital's data. Nothing is copied from it except its
persons' identifiers, once they are linked to the catalog's patients. The design and its limits
are in [design/sources.md](design/sources.md); a practical guide, including how to build a new
database for Medorama, is in [databases.md](databases.md).

| Aspect | Details |
|---|---|
| Kinds | `omop`: an OMOP CDM 5.3 or 5.4 database, read through built-in views. `sql`: any layout, read through three views that an administrator writes (`persons`, `visits`, `entries`). |
| Database engines | PostgreSQL (on-site, Amazon RDS and Aurora, Azure) and SQLite files. |
| Access | Read-only, and enforced: PostgreSQL sessions are read-only, and SQLite files are opened read-only. Each query has a time limit (`timeout_s`, 60 seconds by default), and each criterion a row limit (`max_rows`, 1,000,000 by default). |
| Secrets | The password is in a file (`--password-file`) or in `$MEDORAMA_SOURCE_PASSWORD_<NAME>`. A password given in the page is kept in `<home>/secrets/<name>.password` (mode 0600) and deleted when the database is disconnected. It is never stored in the database or the URL, and never sent back to the page. |
| From the page | Import › Add a database offers **Connect a database file** (a SQLite file inside `data_roots`) and **Connect a database server**. The connection is checked first. **Connected databases** lists them, with **Check**, **Link all** and **Disconnect**. |

For example, to connect an OMOP database on PostgreSQL and one in a SQLite file, connect a
warehouse through views, and then check, link and disconnect:

```bash
medorama connections add omop --kind omop --url "postgres://reader@omop.example.org/cdm?sslmode=require" \
    --password-file /etc/medorama/omop-password --schema cdm --vocabulary-schema vocab --mrn-authority HOSP
medorama connections add county --kind omop --url sqlite:/data/county/omop.sqlite --identifier mrn
medorama connections add warehouse --kind sql --url postgres://… --views warehouse-views.toml
medorama connections check omop      # each view answers; the data's version
medorama connections sync omop       # link every person now (otherwise as searches find them)
medorama connections remove omop     # its links and what it added go; the database is not touched
```

- `--identifier` says what OMOP's `person_source_value` holds: `mrn` (the default),
  `patient_id`, or `none`. With `none`, each person becomes a patient of its own, with the
  patient ID `<connection>:<person key>`.
- `--timezone` is the time zone of the database's local times (an IANA name; an unknown one is
  refused). Without it, the catalog's time zone is written into the connection when it is added
  (by the command or the page), and stays fixed from then on. On PostgreSQL, a `timestamp`
  column (without a time zone) is read in it; a `timestamptz` is an instant.
- `--schema` and `--vocabulary-schema` are plain names (letters, digits, `_`).
- A `sql` connection's views are run as written, in the database's own SQL (PostgreSQL's
  operators, `E''` strings, dollar quotes): Medorama puts them inside its own queries and does
  not translate them.
- `connections add` with a SQLite file checks what the page checks: the file is inside
  `data_roots`, and is not the catalog's own database. A name already used is refused before
  anything is written (a password given in the page included).
- `connections sync` (**Link all**) reads every person in one query, within `max_rows`; a
  database with more persons fails to sync.
- A person is linked to a patient the first time a source names them, by the same rules as an
  imported table. A person whose IDs name two patients is held for Review.

The linked persons' data appear on the patients' pages, in the Databases tab and in cohort
searches. When a connected database does not answer, the page names it and shows the others.

## Add-ons

Recording formats that Medorama does not read itself, including licensed formats, are read by
**adapters**: programs installed in the adapter folder (`adapters_dir`, by default
`~/.local/share/medorama/adapters`). [adapters.md](adapters.md) describes the protocol and how the
programs use adapters. One example is stp-adapter, which reads BedMasterEx `.Stp` files; it is
private and available to institutions under agreement.

To install, list and remove add-ons, run as the catalog's account:

```bash
medorama addons install stp-adapter-0.1.0-linux-x86_64/stp-adapter
medorama addons list                      # name, version, implementation, formats, program
medorama addons remove stp
```

[adapters.md, Installing an add-on](adapters.md#installing-an-add-on) gives the steps in full,
from the archive to the first recording: unblocking a downloaded program on macOS and Windows,
restarting the server, importing again the tables that named such files before, and what to do
when something goes wrong. Admin › Add-ons lists the installed adapters. Add-ons are installed on
the server only, never through the page, and every installation and removal is audited.

## The waveform viewer

Medorama runs one VitalStream viewer for all its users and shows it in its **VitalStream** tab
([architecture.md](architecture.md#running-separately-and-together)). The viewer programs are the
standard ones, unchanged.

| `viewer` setting | The viewer Medorama runs |
|---|---|
| `auto` (default) | The one that `tools/install-medorama` installed (recorded in `medorama.viewer` next to Medorama's program), or else the first found of `vitalstream-cpp`, `vitalstream-rs` and `vitalstream-py`. |
| `cpp`, `rs`, `py` | That viewer. |
| A path | That program. |
| `none` | No viewer. **View** and the VitalStream tab are grayed out, with the reason. |
| (`viewer_url` set) | The catalog runs no viewer, and **View** opens that viewer in a new browser tab. |

**How the viewer runs.** Medorama starts the viewer at the first **View** or the first visit of
the tab, as a separate process listening on 127.0.0.1 at a free port. It restarts the viewer if
it stops, and stops it with the catalog. The viewer listens only on 127.0.0.1, so that other
machines cannot reach it, and it answers only requests that the catalog has signed (below).
- **Its settings** (`<home>/viewer/vitalstream.toml`, written again at each start) give it the
  catalog's `index_dir` (every viewer writes the same index format), `adapters_dir`, time zone,
  `data_roots` and `qc_verbose`, and `require_password = true`, so that a viewer built without
  password support refuses to run under the catalog.
- **Its root.** Medorama's installer builds the viewer with the whole file system as its root
  (`tools/install-medorama.sh`), because Medorama's own rules, below, decide what each user may
  open.
- **Its password** (`<home>/viewer/auth.json`) holds a signing key that only the catalog knows.
  The catalog signs every request it passes on, users never see the viewer's sign-in, and the
  viewer's port is useless to anyone else.
- **Its output** goes to `<home>/viewer/viewer.log`.

**The gateway.** Medorama passes requests to the viewer at `/viewer/…` only after its own
sign-in, and only the following requests; everything else is refused.

| Viewer request | Administrators | Analysts and viewers |
|---|---|---|
| The page and its files | Allowed. | Allowed. |
| `GET api/v1/config` | As the viewer answers. | Its shortcuts point into the folders of the catalog's recordings. |
| `GET api/v1/browse`, `complete` (the Open dialog) | Inside `data_roots`. | Only folders that lead to recordings in the catalog, listing only those recordings. |
| `GET api/v1/scan` (what a folder of records holds, before it opens) | Inside `data_roots`. | A recording in the catalog. Refused in Privacy Mode. |
| `POST api/v1/open` | A recording inside `data_roots`. | A recording in the catalog. Every open is recorded in the audit log (`view`). |
| `POST api/v1/cancel` (stops building an index) | Datasets the user opened through the gateway. | The same. |
| `GET api/v1/datasets/{key}…` | Datasets the user opened through the gateway. | The same. |
| Anything else, including the viewer's `login` and `logout` | Refused. | Refused. |

Paths are compared after resolving symbolic links. The viewer's page may be framed only by the
catalog's page. The viewer receives paths and recordings, never identifiers. In Privacy Mode,
recordings are opened by token, folders are not browsed, and the gateway translates times. The
gateway answers 413 to a request body over 1 MiB, 415 to a `POST api/v1/open` whose body is not
sent as `application/json`, and 400 to a body that is not valid JSON.
`medorama/tests/gateway.rs` runs a real viewer behind the gateway and checks every rule.

**A viewer you run yourself**, on another port, can share the catalog's indexes: set its
`cache_dir` to the catalog's `index_dir` and its `adapters_dir` to the catalog's.

## The audit log

The audit log is append-only, and administrators read it in the Admin tab or with
`medorama audit`. It records:
- sign-ins and failed sign-ins (with proxy sign-in, one `login` per visit);
- imports, removals, link changes, runs, review decisions, and add-ons installed and removed;
- changes to users;
- Privacy Mode switched on and off;
- for users with the PHI permission, `phi_search` (with the search text) and `phi_view` (a patient
  or encounter opened, or a database viewed);
- for sessions in Privacy Mode, `privacy_search` (searches, filters and cohort criteria, with
  their text).

In Privacy Mode, the audit page shows no target or detail.

The log is **sealed** every minute. Each seal is a SHA-256 chain over the entries written since
the previous seal, so that an entry changed, deleted or added afterwards breaks the chain.
`medorama audit --verify` checks the seals, and `medorama audit --seal` seals now. With
`audit_export` set, each seal and its entries are also appended to that file, which keeps a copy
out of reach of the database's administrators ([security.md](security.md)).

An entry is never lost when the database cannot take it at once (busy beyond its wait, or
failing): it is kept, in order, and written before the next entry or by the next seal; with
`audit_export` set it is appended to that file at once (a line with `"pending": true`), and the
server's output says so.

## The database

**Engine.** The catalog's database is SQLite, in write-ahead-log mode (one writer at a time,
readers never blocked) with foreign keys on; or PostgreSQL 14 or later. The code is the same for
both: SQL is written once and translated for PostgreSQL, and every test runs on both. The schema
([`medorama/src/schema/`](../medorama/src/schema)) is applied when the database is created, and a
program older than its database refuses to open it. IDs are never reused, and times are stored
as Unix seconds (UTC).

| Group | Tables |
|---|---|
| People and stays | `patients`, `encounters` (with their pseudonyms), `pseudonym_aliases`, `patient_identifiers` (PHI), `patient_mrns` (PHI), `encounter_identifiers` (PHI), `bed_stays` |
| Data | `recordings`, `recording_identifiers` (PHI), `recording_links`, `recording_series` (derived series, such as the heart rate found from an ECG), `ehr_items`, `item_text` (the search index: SQLite FTS5 with trigrams, or a `pg_trgm` index on PostgreSQL) |
| Imports | `imports` (batches), `provenance` and `ehr_provenance` (which batch and source added what), `field_values` (PHI: each batch's values), `mappings` |
| Databases | `import_columns`, `import_rows` (PHI: the rows as imported, with what each became), `dbcol_<import>` (each database's numeric columns, indexed on demand; none for a database of more than 500 columns) |
| Cohorts | `cohorts`, `cohort_members`, `cohort_removed`, `cohort_shares` |
| Connected databases | `connections` (the URL without a password, and the settings), `source_persons` (PHI: each person's key, linked to a patient) |
| Decisions | `held_rows` (PHI), `patient_merges`, `patient_pairs`, `link_decisions`, `generated_ids` (PHI), `import_links` (the encounters imported rows gave recordings), `id_takeovers` (generated IDs an import replaced) |
| Work | `runs`, `jobs` (a finished job holds its result) |
| Access | `users`, `sessions`, `history`, `audit`, `audit_seals`, `settings` (Privacy Mode's settings and the catalog's secret) |

**Protecting the file.** On Unix, the database and its home folder are readable by their owner
only.

**Encryption at rest.** A build with `--features encryption`
([building.md](building.md#the-rust-workspace)) encrypts the database with SQLCipher. The key
comes from `$MEDORAMA_KEY` or from the file that `key_file` names, and it must be set **before**
`init`: a database created without a key stays unencrypted, and losing the key loses the
database. Without that build, and for the recordings in any case, use disk encryption (FileVault,
BitLocker, LUKS). On PostgreSQL, encryption is the database server's (an encrypted disk or cloud
volume).

**Several users at once.** One server process serves all users, and its workers and the commands
share the database safely. On PostgreSQL, several servers share one catalog: sessions, cohorts,
pseudonyms and the job queue are in the database, and schema updates, taking a job, automatic
indexing, imports and every change to patients and encounters (an import's chunk, a merge, a
connected database's persons linked) are done by one server at a time (advisory locks), so that
one MRN never names two patients. A new SQLite catalog opened by several processes at once is
created by one of them.

## Deployment

Choose the arrangement that fits how people will reach Medorama. Before real patient data come in,
go through the deployment checklist in [security.md](security.md).

- **One machine.** Run `medorama serve` and open <http://127.0.0.1:8090>.
- **A server used from your own computer.** Leave `host = "127.0.0.1"`, run Medorama as a
  service ([Run as a service](#run-as-a-service)), and connect through an SSH tunnel:
  `ssh -N -L 8090:127.0.0.1:8090 user@server`. The viewer comes through the same address
  (`/viewer/`), so one tunnel is enough.
- **A team, behind HTTPS.** Put Medorama behind a reverse proxy that terminates TLS (nginx,
  Caddy, IIS; it may serve Medorama under a path prefix), set `secure_cookies = true`, and leave
  `host = "127.0.0.1"` so that only the proxy reaches Medorama. Give each person an account of
  their own. The server prints a warning when it listens on another address without
  `secure_cookies`.
- **A team, with the institution's sign-in.** Use an authenticating proxy and
  `sign_in = "proxy"` ([Choose the sign-in method](#choose-the-sign-in-method),
  [sign-in.md](sign-in.md)).
- **Several servers.** With the catalog in PostgreSQL, run as many servers as needed behind the
  proxy, each with the same settings and with the results and index folders on shared storage.

**A TLS proxy.** The following nginx server block is a minimal example. Medorama accepts tables of
up to 1 GiB uploaded from the page, while nginx limits request bodies to 1 MB by default, hence
`client_max_body_size`:

```nginx
server {
    listen 443 ssl;
    server_name medorama.example.org;
    ssl_certificate     /etc/ssl/certs/medorama.pem;
    ssl_certificate_key /etc/ssl/private/medorama.key;
    client_max_body_size 1g;
    location / {
        proxy_pass http://127.0.0.1:8090;
        proxy_set_header Host $host;
    }
}
```

**Who can reach the database.** Anyone who can read the home folder can read the database, and
the commands run with administrator rights. Run the server under a dedicated account, and let
people use the web page. On PostgreSQL, give Medorama's account access to its own database only.

### On a shared Linux cluster

This procedure sets Medorama up on a shared cluster without administrator rights, with the data
on a shared file system and people reaching it from their own computers. The example paths are
those of a MUSE ECG collection; use your own.

1. **Choose where it runs:** a machine where a long-running program is allowed and that sees the
   data, such as a service node or a virtual machine. Do not use a login node or a batch job.
2. **Build Medorama in your account.** The Rust viewer needs no C++20 compiler:
   ```bash
   git clone https://github.com/alphanumericslab/medorama.git ~/medorama && cd ~/medorama
   module load gcc 2>/dev/null || true          # any C compiler, for SQLite (or tools/install-gcc.sh)
   tools/install-medorama.sh --viewer rs
   umask 077                                    # what you make next is yours alone
   ```
3. **Choose the catalog's database.** SQLite needs a local disk, because its locking is
   unreliable on NFS, Lustre or GPFS. For many users or millions of rows, use **PostgreSQL**:
   your institution's server, or one in your account, for example from conda-forge:
   ```bash
   conda create -y -p ~/pg -c conda-forge postgresql=16 && export PATH=~/pg/bin:$PATH
   initdb -D ~/pgdata -A scram-sha-256 -U medorama -W          # asks for a password
   pg_ctl -D ~/pgdata -o "-k /tmp -c listen_addresses=127.0.0.1 -p 15432" -l ~/pgdata/log start
   createdb -h 127.0.0.1 -p 15432 -U medorama medorama
   echo 'the password' > ~/.medorama-db-password && chmod 600 ~/.medorama-db-password
   ```
4. **Create the catalog:**
   ```bash
   H=~/medorama-home
   .venv/bin/medorama init --home $H --results ~/medorama-results --timezone America/New_York \
     --database-url postgres://medorama@127.0.0.1:15432/medorama        # or leave it out: SQLite in $H
   ```
   Then set these in `$H/medorama.toml`: `port = 18090` (a port no one else uses),
   `data_roots = ["/data/ecg/muse"]` and
   `database_password_file = "/home/you/.medorama-db-password"`.
5. **Import the data**, first as a trial that stores nothing. On a laptop, 500,000 rows take under
   a minute on SQLite and about a minute and a quarter on PostgreSQL.
   ```bash
   M=/data/ecg/muse/xml_metadata
   .venv/bin/medorama --home $H import-table $M/*.csv --name muse_ecg --source muse --dry-run
   .venv/bin/medorama --home $H import-table $M/*.csv --name muse_ecg --source muse
   ```
6. **Run it** so that it outlives your session, in `tmux` (or as a user service): run
   `tmux new -s medorama`, then `.venv/bin/medorama --home $H serve`.
7. **Reach it** from your computer through SSH, through the login node with `-J`:
   `ssh -N -J you@login.cluster.example.edu -L 8090:127.0.0.1:18090 you@service-node`, then open
   <http://127.0.0.1:8090>.
8. **Add people** in Admin › Users, and give the PHI permission only to those who may see
   identifiers. Everyone logged in to that machine can reach its loopback port, and so the
   sign-in page. Use strong passwords, keep `$H` readable by you alone, prefer a dedicated
   account, and put HTTPS in front for a team.

### Run as a service

**Linux (systemd, as your user).** These commands create a user service, start it, keep it
running after you log out, and show its log:

```bash
mkdir -p ~/.config/systemd/user
cat > ~/.config/systemd/user/medorama.service <<'EOF'
[Unit]
Description=Medorama

[Service]
ExecStart=%h/.cargo/bin/medorama serve
Restart=on-failure

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now medorama
loginctl enable-linger "$USER"        # keep it running when you are logged out (may need an administrator)
journalctl --user -u medorama         # its log
```

Add `--home DIR` before `serve` if the catalog is not in the default place. Without systemd, run
`tmux new -d -s medorama 'medorama serve'`.

**macOS (launchd, as your user).** These commands create a launch agent that starts Medorama at
log-in and restarts it if it stops:

```bash
cat > ~/Library/LaunchAgents/org.medorama.server.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>org.medorama.server</string>
  <key>ProgramArguments</key><array><string>$HOME/.cargo/bin/medorama</string><string>serve</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/medorama.log</string>
</dict></plist>
EOF
launchctl load ~/Library/LaunchAgents/org.medorama.server.plist
```

**Windows (started at log-on, without administrator rights).** These commands add Medorama to
your Startup folder and start it now:

```powershell
$exe = (Get-Command medorama).Source
$startup = [Environment]::GetFolderPath('Startup')
Set-Content "$startup\medorama.cmd" "@start `"Medorama`" /min `"$exe`" serve"
& "$startup\medorama.cmd"               # start it now
```

## Backup, update, uninstall

**Back up** the home folder: the settings, the database and the plug-ins. Indexes can always be
rebuilt; the results of actions may not be. A backup holds identifiers, so store it as securely
as the recordings.
- SQLite, while the server runs:
  `sqlite3 medorama.sqlite ".backup /backup/medorama-$(date +%F).sqlite"`. This does not work for
  an encrypted database: copy that one while the server is stopped.
- SQLite, while the server is stopped: a plain copy of `medorama.sqlite`.
- PostgreSQL: `pg_dump -Fc medorama > medorama-$(date +%F).dump`, or the database server's
  point-in-time recovery. Encrypt the dumps.

**Update.** Run `git pull`, build again ([building.md](building.md#update-and-uninstall)) and
restart the server. A newer version brings its database changes and applies them at its first
start. An older version refuses to open a database that a newer one has updated, so back up
first.

**Uninstall.** Stop the service, delete the program (`cargo uninstall medorama`, or the copy), and
delete the home and results folders if they are no longer needed. The recordings are not touched.

## The demo

The demo is a set of invented patients for trying Medorama and for showing what it does. It has
six patients with their encounters and bed stays, laboratory results, vital signs, medications,
diagnoses, microbiology, allergies and notes, and these files:
- short ICU recordings (WFDB collections with ECG, arterial pressure, SpO2 pleth, respiration,
  vital signs, alarms and R-wave markers);
- a 24-hour Holter ECG;
- an overnight sleep study for patient P900004: a two-hour EDF+ file with EEG, EOG, ECG, airflow,
  thoracic effort and SpO2, scored with sleep stages, apneas, desaturations and arousals as
  annotations;
- 12-lead ECGs (WFDB, GE MUSE XML, DICOM, PDF and PNG);
- chest X-rays and CT series, echo loops and reports, and a coronary angiography run;
- another hospital's OMOP database, which adds earlier stays and results to two of the patients.

Everything is invented: names, numbers, signals and pictures. `tools/make-demo-data.py` makes
them.

In the catalog, the demo is a cohort named **DEMO** that holds its patients. Every user sees it,
read-only, and each demo patient's page is marked DEMO. Its tables are imported as the source
`DEMO` (Import › Sources), and its OMOP database is connected as `demo_omop`.

**The demo is off by default.** A catalog starts empty unless the demo was asked for. Install it
only where Medorama is tried, demonstrated or taught, never on a server meant for real patients
(see the deployment checklist in [security.md](security.md)).

### Install with the demo

Give the installer `--demo` (on Windows, `-Demo`), with any other options:

```bash
tools/install-medorama.sh --demo              # or: tools/install-medorama.sh --viewer rs --demo
tools/install.sh --demo                       # the development install
```

```powershell
powershell -ExecutionPolicy Bypass -File tools\install-medorama.ps1 -Demo
powershell -ExecutionPolicy Bypass -File tools\install.ps1 -Demo
```

With this option, the installer:
1. installs NumPy and wfdb-x into `.venv` if they are not there (the demo's WFDB records are
   written with wfdb-x, at the commit that the Python viewer uses);
2. writes the demo's files into `.venv/share/medorama/demo` (on Windows
   `.venv\share\medorama\demo`; about 60 MB);
3. names that folder in `.venv/bin/medorama.demo` (on Windows `.venv\Scripts\medorama.demo`), next
   to the programs.

The demo needs WFDB support: its ICU and Holter recordings are WFDB records, so the installers
refuse `--demo` together with `--no-wfdb` ([building.md, Without WFDB](building.md#without-wfdb)).

Then create the catalog as usual. `medorama init --results DIR` loads the demo by itself, as the
first administrator, and says what it loaded. `medorama serve` indexes the demo's recordings in
the background. Sign in, open **Cohorts › DEMO**, and click a patient.

Running the installer again without `--demo` removes `medorama.demo`, and catalogs created after
that start empty. A catalog that already holds the demo keeps it until it is deleted (below), and
the demo's files stay in `.venv/share/medorama/demo` for it. `init --no-demo` creates an empty
catalog while `medorama.demo` is there.

### Remove the demo

An administrator deletes the cohort DEMO with **Cohorts › DEMO › Delete…**. The page first says
what will be removed (how many patients, encounters, bed stays, items and recordings), and then
removes, in one step:
- every patient, encounter, bed stay, result, note and file link that the demo's tables added,
  and the demo's recordings, with the catalog's indexes and results of them;
- the connection `demo_omop` (the OMOP file itself is not touched);
- the cohort DEMO. Cohorts made from it keep their members as they were, without the demo's
  patients.

Nothing else changes. What another source also holds stays (the removal works as described in
[Imports and sources](#imports-and-sources)), and the demo's files stay where they are. Only
administrators can delete the demo; other users see the cohort without the Delete button. On the
command line, `medorama demo remove --dry-run` says what would be removed, and
`medorama demo remove` removes it. Every step is recorded in the audit log (`demo_add`,
`demo_remove`).

### Load it later, or into another catalog

`medorama demo add` loads the installer's demo into the catalog in use (administrators only),
for example after `init --no-demo` or after deleting the demo. `medorama demo add DIR` loads a
demo folder that you made yourself:

```bash
.venv/bin/python tools/make-demo-data.py ~/medorama-demo    # the demo's files (NumPy and wfdb-x in .venv)
medorama demo add ~/medorama-demo                          # its tables, its OMOP database, the cohort DEMO
```

A catalog holds the demo once: `demo add` refuses while the cohort DEMO exists. If the catalog
has `data_roots`, the demo's folder must be inside one of them. Without the Python viewer in
`.venv`, add NumPy and wfdb-x first:

```bash
.venv/bin/pip install numpy "wfdb-x @ https://github.com/alphanumericslab/wfdb-x/archive/a707ba9ec1624241793bf41b938b219d76005526.tar.gz#subdirectory=python"
```

`tools/make-demo-data.py` can also give the demo patients local examples of real files, which are
kept in `sample_data` and never in git: `--examples DIR` adds ECGs and `--dicom DIR` adds DICOM
studies, de-identified on the way (`tools/make-demo-data.py --help`).

## Troubleshooting

| Problem | Cause and remedy |
|---|---|
| `no catalog in …/.local/share/medorama` | The catalog is elsewhere. Run `medorama --home DIR use`, or pass `--home DIR`, or set `MEDORAMA_HOME`. |
| `unknown settings: …` | `medorama.toml` has a key that Medorama does not know, often a typing mistake. Correct or remove it ([Settings](#settings)). |
| `proxy_user_header, proxy_addresses and proxy_logout_url need sign_in = "proxy"` | Proxy settings are present while the sign-in method is `password`. Add `sign_in = "proxy"`, or remove them. |
| `proxy_addresses: "…" is not an IP address` | `proxy_addresses` takes IP addresses, not host names. |
| `the password is too short` | Passwords need at least 10 characters. |
| `Use at least 8 characters.` (a viewer's `--set-password`) | The VitalStream viewers' own passwords need at least 8 characters. |
| `init`: `cannot create the database` | Most often, the home folder is on a network file system. Use `--home` with a folder on a local disk. |
| `cannot listen on 127.0.0.1:8090` | Another program uses the port. Use `--port N`, or set `port` in `medorama.toml`. |
| Import: `skipped …: the catalog writes to …` | The results, index or home folder is a recording's folder or inside one. Choose folders elsewhere. |
| Import: `outside the configured data_roots` | Add the folder to `data_roots` (an empty list allows any folder). |
| Import: `… not a recording Medorama reads. A format read by an add-on needs it installed …` | An ICU, Holter, EEG or Sleep row names a file that no installed reader reads. Install the add-on that reads it, restart the server, then remove that import and import the table again ([adapters.md, Installing an add-on](adapters.md#installing-an-add-on)). |
| A recording stays gray (not indexed) | Check that the server runs with workers and `auto_index = true`, or run `medorama run index --recording ID` and `medorama work`. When indexing failed, the Jobs tab shows the error. |
| Recordings are not linked to encounters | Linking needs a row that names the file, or a bed stay with the same `UNIT_BED` that overlaps the recording's time. Check `medorama recordings --unlinked` and the databases' times, in particular their time zone. |
| **View** is grayed out | Its tooltip says why. Usually no viewer is installed beside the catalog: run `tools/install-medorama.sh --viewer cpp`. |
| "too many failed attempts" | Five wrong passwords lock a user name for one minute. |
| A forgotten administrator password | Run `medorama user set NAME --password` on the server. |
| "your institution's sign-in names you …, and Medorama has no account of that name" | With proxy sign-in, the proxy named a user without an enabled account. Create the account with that exact name, or enable it. |
| Behind the proxy, every request is "sign in required" | The proxy does not set `proxy_user_header`, or it connects from an address that is not in `proxy_addresses`. |
| `this catalog signs users in through the institution's sign-in, not with a password` | A password sign-in was attempted while `sign_in = "proxy"`. Sign in through the proxy. |
| `the database has schema version N, newer than this program knows` | A newer `medorama` updated the database. Use that version. |
| `the PostgreSQL database … holds no catalog yet` | `database_url` names an empty database. Create the catalog there (`init --database-url`), or copy one into it (`copy-database`). |
| `PostgreSQL: … TLS …` | The database server offers no TLS, or a certificate that Medorama cannot verify. Install a certificate, or name its authority in `database_ca_file`. Use `sslmode=disable` only on a network you trust. |
| `database_url: leave the password out` | The password goes in `database_password_file` or `$MEDORAMA_DATABASE_PASSWORD`. |
| A query fails on PostgreSQL only | Run with `MEDORAMA_DEBUG_SQL=1`. It prints the statement that PostgreSQL refused, never its values. |
| A patient's page says `omop: not available` | That connected database did not answer. Run `medorama connections check omop`. |
| A criterion: `… names N measurements in omop` | OMOP's concept names are long. Write the one you mean, in quotes: `omop."Lactate [Moles/volume] in Blood" > 4`. |
