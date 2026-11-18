# Administrator's guide

This guide is for the people who set up Medorama, connect its databases, manage its users and
keep it running. It assumes Medorama is installed; [getting-started.md](getting-started.md)
covers the first installation and [building.md](building.md) every way of building it. How people
use the page is in the [user's guide](user-guide.md). How to prepare a database for Medorama is
in [databases.md](databases.md), and how to make one from tables, exports or folders of
recordings with db-builder is in [db-builder's README](../db-builder/README.md). The security
design, for the reviewers who must approve a deployment, is in [security.md](security.md), and
how patient identifiers are hidden is in [protecting-patient-data.md](protecting-patient-data.md).

## Contents

- [Rules](#rules)
- [Set up Medorama](#set-up-medorama)
- [Settings](#settings)
- [Put the control store in PostgreSQL](#put-the-control-store-in-postgresql)
- [Run the server](#run-the-server)
- [Command reference](#command-reference)
- [Users, roles and the PHI permission](#users-roles-and-the-phi-permission)
- [Choose the sign-in method](#choose-the-sign-in-method)
- [Connected databases](#connected-databases): [Connect a database](#connect-a-database) ·
  [Identity](#identity) · [Status, versions and variables](#status-versions-and-variables) ·
  [Files and recordings](#files-and-recordings)
- [Grants](#grants)
- [Keys](#keys)
- [Removing a database](#removing-a-database)
- [Add-ons](#add-ons)
- [The waveform viewer](#the-waveform-viewer)
- [db-builder](#db-builder)
- [The audit log](#the-audit-log)
- [The control store and the workspace](#the-control-store-and-the-workspace)
- [Deployment](#deployment): [On a shared Linux cluster](#on-a-shared-linux-cluster) ·
  [Run as a service](#run-as-a-service)
- [Backup, update, uninstall](#backup-update-uninstall)
- [The demo](#the-demo)
- [Troubleshooting](#troubleshooting)

## Rules

Medorama keeps the following rules in every deployment. They are what a reviewer can rely on,
and every change to the code must keep them.

1. **Medorama connects databases; it imports nothing.** It reads each connected database in
   place, read-only, and never writes to it. Tables, known exports and folders of recordings
   become a database with [db-builder](../db-builder/README.md) first, and are then connected.
2. **Recordings are only read.** Medorama stores no copy of a recording. It never writes, moves or
   deletes a recording, and never writes into a recording's folder.
3. **No patient data in the control store.** Medorama's own database (the *control store*)
   holds users, grants, the audit log, the list of connected databases, cohort definitions and
   the wrapped keys. It holds patient numbers and pseudonyms, but no identifier, name or value
   read from a database.
4. **What Medorama generates goes into its encrypted workspace.** One folder per connected
   database, and one file per cohort, each encrypted with a key of its own. Removing a database
   destroys its key first, then its folder, then every cohort row it contributed to.
5. **Access is granted per database and per cohort.** A user searches, views or sees the
   identifiers of a database only when granted it. Administrators are not granted every database
   by being administrators.
6. **Identifiers leave the server only for those allowed to see them.** MRNs, names and dates of
   birth are sent only to users who hold the `phi` grant on the database that holds them and the
   PHI permission, and who have switched Privacy Mode off; their searches and views are written
   to the audit log. The viewers, the job descriptions given to actions and their output folders
   carry pseudonyms only.
7. **Identity is the databases' responsibility.** A patient named by the same issuer and
   identifier in two databases is one patient; Medorama joins on exact matches only, and never
   merges patients.

## Set up Medorama

Medorama's data on one server live in two places, chosen when it is set up:

| Folder | What goes there | Advice |
|---|---|---|
| **Home folder** | `medorama.toml` (the settings), `medorama.sqlite` (the control store, unless it is in PostgreSQL), `master.key` (the master key, unless it comes from elsewhere: [Keys](#keys)), `actions/` (action plug-ins), `viewer/` (the waveform viewer's settings). | Put it on a **local disk**, because SQLite's locking is unreliable on network file systems such as NFS or SMB home folders. Make it private to the account that runs Medorama. The default is `~/.local/share/medorama` (Windows: `%LOCALAPPDATA%\medorama`). |
| **Workspace** | One folder per connected database (`db-NNNN/`, with its recordings' indexes in `index.sqlite`), the cohorts (`cohorts/`), and the viewer's own index store (`viewer/`). Its SQLite files are encrypted. | Leave room for the indexes, roughly 1 to 5 % of the recordings' size. Never choose a folder that holds recordings or a connected database. Keep it apart from the master key. |

To set up Medorama in the default home folder, or in a home folder of your own, run one of:

```bash
medorama init --workspace ~/medorama-workspace
medorama init --workspace /data/medorama-workspace --home ~/medorama
```

`init` asks twice for the password of the first user, `admin`, who has the PHI permission. The
password needs at least 10 characters. It then creates:
- the home folder and `medorama.toml`;
- the control store (`medorama.sqlite`, or the PostgreSQL database given), with the first
  administrator;
- the master key file `<home>/master.key` (mode 0600), unless `$MEDORAMA_MASTER_KEY` is set;
- the workspace (mode 0700).

It refuses a home folder that already holds `medorama.toml` or `medorama.sqlite`, and a
PostgreSQL database that already has users. A failed `init` leaves no settings file behind. It
ends by naming the next step: `medorama databases connect`, then `medorama serve`.

| `init` option | Default | Meaning |
|---|---|---|
| `--home DIR` | See below. | Where Medorama's home folder is. It is remembered for later commands (a demo installation's never is). It is a global option, which every other command accepts too. |
| `--workspace DIR` | Required. | The workspace. |
| `--admin NAME` | `admin` | The first user's name. |
| `--timezone TZ` | `America/New_York` | The time zone for times that a database writes without an offset (unless the database's own setting says otherwise), and for display. |
| `--port N` | 8090; 8095 with `--demo` | The port of the web page and the API (a demo installation's apart from a site's, so that both run at once). |
| `--password-stdin` | | Reads the password from standard input, for scripts. |
| `--database-url URL`, `--database-password-file FILE` | SQLite in the home folder | Puts the control store in PostgreSQL ([Put the control store in PostgreSQL](#put-the-control-store-in-postgresql)). |
| `--demo` | | Makes a **demo installation**, for good: it connects the demo databases that the installer made with `--demo`, and only demo databases; it makes four example cohorts; its home is never remembered ([The demo](#the-demo)). Without that installer option there is nothing to connect, and `init` says so. Without `--demo`, `init` makes a site's installation, which never connects a demo database. |

**Which home folder a command uses.** A command uses the first of these that applies:
1. the `--home DIR` option;
2. the environment variable `$MEDORAMA_HOME`;
3. the home folder that `init` created last, or the one chosen with `medorama --home DIR use`
   (remembered in `~/.config/medorama/home`, on Windows `%APPDATA%\medorama\home`);
4. `~/.local/share/medorama` (on Windows `%LOCALAPPDATA%\medorama`).

`serve` prints the home folder it uses. The home folder is readable by its owner only.

## Settings

The settings are in `<home>/medorama.toml`. `init` writes the file with a comment above each
setting. Relative paths are relative to the home folder. Restart the server after changing the
file. An unknown key is an error: the command or the server stops and names it, so that a typing
mistake is never silently ignored.

| Key | Default | Meaning |
|---|---|---|
| `workspace_dir` | Set by `init`; required. | The workspace: one encrypted folder per connected database, and the cohorts ([The control store and the workspace](#the-control-store-and-the-workspace)). |
| `master_key_file` | `master.key` (in the home folder) | The file that holds the master key, 64 hex digits ([Keys](#keys)). |
| `master_key_command` | Empty | A command that prints the master key, such as a keyring's or a key manager's command line. Used instead of the file. |
| `key_file` | None | The SQLite control store's own key (SQLCipher), when the control store is encrypted too ([The control store and the workspace](#the-control-store-and-the-workspace)). |
| `data_roots` | `[]` (anywhere) | The folders that the files Medorama reads may lie in: the SQLite databases it connects and the recordings' files. The page's **Choose…** dialog and the viewer's Open dialog show these folders. |
| `adapters_dir` | `~/.local/share/medorama/adapters` | The adapter folder ([Add-ons](#add-ons)). |
| `database` | `medorama.sqlite` | The SQLite control store's file. |
| `database_url` | Empty | A PostgreSQL database instead, such as `postgres://user@host/db?sslmode=require`, written without its password. `$MEDORAMA_DATABASE_URL` overrides it. |
| `database_password_file` | None | The file that holds that database's password. `$MEDORAMA_DATABASE_PASSWORD` overrides it. |
| `database_ca_file` | None | Certificate authorities (PEM) trusted for the database server's certificate, in addition to the system's. |
| `host`, `port` | `127.0.0.1`, `8090` | The address and port of the web page and the API. |
| `workers` | 2 | The number of jobs that run at the same time. A value below 1 is read as 1. |
| `auto_index` | `true` | While the server runs, it queues indexing of new and changed recordings. |
| `check_interval_s` | 900 | How often, in seconds, every recording is checked again; 0 means never. (Connected databases are checked every 60 seconds, whatever this setting says.) |
| `timezone` | `America/New_York` | The time zone of times written without an offset, for a database that does not set its own, and of the times shown. |
| `viewer` | `auto` | The VitalStream viewer that Medorama runs: `auto`, `cpp`, `rs`, `py`, `none`, or the path of a viewer program ([The waveform viewer](#the-waveform-viewer)). |
| `viewer_url` | Empty | Instead, a viewer running elsewhere, opened in a new browser tab. |
| `qc_verbose` | `false` | The viewer that Medorama runs also outlines, with a dashed line, the data a reader repaired, such as data blocks that a file stored out of order and the reader put back in order. Breaks are shaded either way. |
| `actions_dirs` | `["actions"]` | The folders of action plug-ins ([actions.md](actions.md)). |
| `session_hours` | 12 | How long a sign-in lasts. A value below 1 is read as 1. |
| `session_idle_minutes` | 60 | A sign-in that is not used for this long ends; 0 means only `session_hours` ends it. The page's own background checks for new data do not count as use. |
| `secure_cookies` | `false` | Set to `true` when Medorama is served over HTTPS. |
| `trusted_proxies` | `[]` | The reverse proxy's addresses. A sign-in from one is counted for the client it names in `X-Forwarded-For`, so that the failed-sign-in limits apply per client, not to everyone behind the proxy. Set it whenever a proxy is in front of Medorama. |
| `sign_in` | `password` | How users sign in: `password` (Medorama's own accounts) or `proxy` (the institution's authenticating reverse proxy). See [Choose the sign-in method](#choose-the-sign-in-method). |
| `proxy_user_header` | `X-Remote-User` | With `sign_in = "proxy"`: the request header in which the proxy names the user. It must be a valid header name. |
| `proxy_addresses` | `["127.0.0.1", "::1"]` | With `sign_in = "proxy"`: the IP addresses the proxy connects from. The header is believed only on requests from these addresses. Each entry must be an IP address, not a host name, and the list must not be empty. |
| `proxy_logout_url` | Empty | With `sign_in = "proxy"`: where **Sign out** sends the browser, normally the proxy's own sign-out address, such as `/oauth2/sign_out`. |
| `audit_export` | None | A file to which the audit log's sealed entries are appended, on storage that the control store's administrators cannot change ([The audit log](#the-audit-log)). |
| `db_builder_root` | Empty (off) | The folder in which the db-builder that Medorama runs builds databases, reading and writing nothing above it ([db-builder](#db-builder)). Empty: Medorama runs none. `serve --db-builder-root DIR` overrides it. |
| `db_builder_port` | 8091 | The port of that db-builder, on 127.0.0.1. |
| `db_builder_url` | Empty | Instead, a db-builder running elsewhere, which **Create a database…** opens; Medorama runs none. |

Setting `proxy_user_header`, `proxy_addresses` or `proxy_logout_url` without
`sign_in = "proxy"` is an error, so that a half-finished change to the sign-in method stops the
server instead of being ignored.

**Environment variables.** `$MEDORAMA_HOME` (the home folder), `$MEDORAMA_MASTER_KEY` (the
master key, before `master_key_command` and `master_key_file`), `$MEDORAMA_KEY` (the SQLite
control store's key, instead of `key_file`), `$MEDORAMA_DATABASE_URL` and
`$MEDORAMA_DATABASE_PASSWORD` (a PostgreSQL control store), and
`$MEDORAMA_SOURCE_PASSWORD_<NAME>` (a connected database's password).

## Put the control store in PostgreSQL

SQLite, the default, needs nothing installed. Put the control store in **PostgreSQL** (version 14
or later) where the home folder cannot be on a local disk, or where the institution already runs
PostgreSQL (on-site, Amazon RDS or Aurora, Azure Database for PostgreSQL) and wants Medorama's own
database there. The same program supports both, and every test runs on both. This version runs
**one server**: the workspace is a folder on that server's disk.

1. Create an empty database, with an account for Medorama alone that owns it:
   ```sql
   CREATE ROLE medorama LOGIN PASSWORD '…';
   CREATE DATABASE medorama OWNER medorama;
   ```
2. Store the password in a file that only Medorama's account can read (`chmod 600`), or in
   `$MEDORAMA_DATABASE_PASSWORD`. Never put it in the URL.
3. Set Medorama up. Its settings, master key and workspace stay on the server's disk:
   ```bash
   medorama init --workspace /data/medorama-workspace \
       --database-url "postgres://medorama@db.example.org/medorama" --database-password-file /etc/medorama/db-password
   ```
   A database server on another host must offer TLS. Medorama requires it, unless the URL says
   `sslmode=disable`, and verifies the server's certificate and name. `database_ca_file` adds a
   certificate authority, such as a private CA or Amazon RDS's bundle.

Medorama opens at most 32 connections to the database (a request waits for one beyond that),
checks a connection that was idle before using it again, and, unless the URL sets them, gives
each connection 10 seconds to connect, TCP keepalives, and 5 minutes for a statement
(`?connect_timeout=…`, `keepalives=0`, `options=-c statement_timeout=…` in the URL change them).
The URL holds no password, neither before the host (`user:password@`) nor as a parameter
(`?password=`): Medorama refuses one that does.

**Move a SQLite control store to PostgreSQL.** The following command copies every table and row
in one transaction (each table read a range of rows at a time, never whole), and prints the lines
to add to `medorama.toml`:

```bash
medorama copy-database --to postgres://medorama@db.example.org/medorama --password-file FILE
```

The workspace and the master key stay as they are. The SQLite file is left as it was. Keep it as
a backup, then delete it securely.

## Run the server

This command starts the web page, the API and the background work:

```bash
medorama serve
```

Open <http://127.0.0.1:8090> and sign in. Ctrl+C (or the SIGTERM a service manager sends) stops
the server: its plug-ins are stopped (TERM to each one's process group, KILL 5 seconds later) and
their jobs go back to the queue, and so does `medorama work`'s. At start-up, the server (and
every command) first finishes any removal of a database that a stop or a crash interrupted
([Removing a database](#removing-a-database)). While the server runs:
- every connected database is **checked** at start and then every 60 seconds: online or offline,
  and read again when its data changed ([Status, versions and variables](#status-versions-and-variables));
- **workers** (the `workers` setting) run queued jobs;
- **automatic indexing** queues new and changed recordings within about 15 seconds (a
  recording is kept, and so indexed, once its patient is opened, or when **Index all
  recordings** reads its database: [Files and recordings](#files-and-recordings));
- every recording kept is **checked again** every `check_interval_s` seconds, which finds missing
  and changed recordings and lost indexes.

The commands can do the same work without the server: `medorama databases index NAME` reads
every recording a database lists, `medorama run index --all` queues indexing of every recording
kept, and `medorama work` runs the queue in the terminal until it is empty.
`serve --no-workers` serves the page only, while another process runs `work`.

## Command reference

`--home DIR` is a global option: every command accepts it, before or after the command's name.
`medorama <command> --help` shows a command's options.

| Command | What it does |
|---|---|
| `init --workspace DIR …` | Sets Medorama up ([Set up Medorama](#set-up-medorama)). |
| `--home DIR use` | Makes that home folder the default for later commands. |
| `serve [--host H] [--port N] [--no-workers] [--db-builder-root DIR]` | Runs the web page, the API and the background work (and db-builder, building databases in `DIR`: [db-builder](#db-builder)). |
| `databases connect NAME --kind omop\|views\|dbb --url URL …` | Connects a database ([Connect a database](#connect-a-database)). |
| `databases list [--json]` | Lists the connected databases: kind, engine, status, persons, the recordings opened, and why an offline one does not answer. |
| `databases check [NAME\|--all]` | Checks one database (or every one) now: online or offline, and read again when its data changed. |
| `databases refresh NAME` | Reads again what Medorama keeps of a database: its catalog (the names and units of its variables), its persons and their encounters. No recording is read. |
| `databases views NAME --views FILE` | Replaces a connected database's views (kind `views`; an admin who manages it) with those of a TOML file, as `connect --views` takes it: checks them, and reads the database again. Its patients keep their numbers. |
| `databases count NAME` | Counts how many entries and persons hold each of a database's variables (**Count now**): one pass over its entries. |
| `databases index NAME` | Reads every recording a database lists, checks its files, and queues the new or changed ones for indexing (**Index all recordings**). |
| `databases variables NAME [--json]` | What a database holds: its variables, with their counts once counted (`-` before `databases count`; from 1 to 10 shown as `< 11`). |
| `databases conflicts NAME` | Its identity conflicts ([Identity](#identity)). |
| `databases grants NAME`, `databases grant NAME USER PERMISSION`, `databases revoke NAME USER PERMISSION` | Who may use a database; grants or revokes `search`, `view`, `phi` or `manage` ([Grants](#grants)). Revoking a permission revokes the stronger ones too. |
| `databases remove NAME [--yes]` | Removes a database and every trace of it from Medorama ([Removing a database](#removing-a-database)). Without `--yes`, it asks you to type the database's name. |
| `keys new FILE`, `keys rotate --new-key-file FILE` | Makes a new master key file; wraps every data key and stored password again with a new master key ([Keys](#keys)). |
| `addons list`, `addons install PROGRAM`, `addons remove NAME` | Manages [add-ons](#add-ons). |
| `list [--q SEARCH] [--limit N] [--json]` | Lists patients of the connected databases. The search takes pseudonyms, sex, `has:…` and `database:…`, and names, MRNs and dates of birth ([search syntax](user-guide.md#searching)). `--limit` defaults to 100. |
| `recordings [--q SEARCH] [--limit N] [--json]` | Lists the recordings Medorama keeps of the connected databases (those opened or indexed), with their numbers. `--limit` defaults to 100. |
| `check` | Checks every recording kept again now: missing, changed, lost indexes. |
| `actions` | Lists the installed actions. |
| `run ACTION… --all\|--recording N…\|--patient MO-…\|--cohort N [--force] [--param A.K=V] [--priority N]` | Queues actions: on every recording, on recordings by number, on the recordings of patients (by pseudonym), or on those of a cohort's members. |
| `work [--workers N]` | Runs the queue in this process until it is empty. |
| `jobs [--state S] [--limit N] [--json]`, `cancel --job N\|--run N\|--all` | Lists the queue (`--limit` defaults to 50), or cancels jobs. `cancel --job N --run M` cancels job N only if it belongs to run M. |
| `user add NAME --role R [--phi] [--display-name D] [--password-stdin \| --no-password]`, `user list`, `user set NAME [--role R] [--phi true\|false] [--disabled true\|false] [--display-name D] [--password\|--password-stdin]`, `user remove NAME` | Manages users. The `--phi` and `--disabled` options of `user set` take a value, as in `--disabled true`. |
| `audit [--limit N]`, `audit --verify`, `audit --seal`, `audit --seal --verify` | Shows the audit log (`--limit` defaults to 50), checks whether its seals hold, seals it now, or seals and then checks. |
| `copy-database --to URL [--password-file F] [--ca-file F]` | Copies a SQLite control store into an empty PostgreSQL database. The password comes from the file, or else from `$MEDORAMA_DATABASE_PASSWORD`; `--ca-file` names certificate authorities (PEM) in addition to the system's. |

**The rights of commands.** The commands act directly on the control store and the workspace,
with administrator rights, the PHI permission and every grant, as the operating-system user who
can read the home folder and the master key. The audit log records them as `local:<user>`.

## Users, roles and the PHI permission

Every user has a **role**, which says what the user may *do*:

| Role | May |
|---|---|
| `viewer` | Search and look at the patients, recordings and data of the databases granted to them; keep a history; make cohorts and share them. |
| `analyst` | Everything a viewer may do, and run and cancel actions. |
| `admin` | Everything an analyst may do, and connect databases, manage users and settings, read the audit log, and grant themselves a database ([Grants](#grants)). |

**Grants** say which databases and cohorts a user may use ([Grants](#grants)). A new user has
none, and sees no patient until granted a database.

**The PHI permission** is given per user, in addition to the role. It lets the user leave
Privacy Mode, and then see and search MRNs, names, dates of birth and real dates, and read notes,
of the databases on which the user holds the `phi` grant. Without it, every session of the user
is in Privacy Mode, administrators' sessions included. With it, a session starts in Privacy Mode,
except an administrator's, which starts out of it. The **Privacy Mode** switch in the page's
header changes the mode, and the choice lasts until the user signs out.

To add users and reset a password from the command line:

```bash
medorama user add alice --role analyst          # asks for her password
medorama user add bob --role viewer --phi       # may see identifiers (of the databases granted phi)
medorama user set alice --password              # a forgotten password
medorama databases grant omop alice view        # alice sees omop's patients and data
```

Admin › Users does the same in the page, and each database's page grants it.

**Accounts and passwords.** `init` creates the first administrator, who has the PHI permission.
Passwords have at least 10 characters and are stored as Argon2id hashes. After 5 failed sign-ins,
a user name is locked for one minute. Medorama refuses any change that would leave it without an
enabled administrator.

**Sessions.** A session is a random 256-bit token in an HttpOnly, SameSite=Strict cookie, and the
control store keeps only its SHA-256 hash. Changing a user's password or disabling the user ends
that user's sessions. **Sign out** in Admin › Users ends a user's sessions everywhere.

### Privacy Mode's settings

Admin › Privacy Mode chooses what Privacy Mode withholds and how it moves dates. Patients' names,
MRNs, dates of birth and account numbers are always withheld (no choice); the choices are
whether dates move, whether ages above 89 are capped, and whether free text (notes, reports, any
entry's text) is withheld; by default it is shown, marked. Withholding less (unticking a choice,
fewer days, a smaller offset) needs an administrator with the PHI permission, and so does
resetting the password of a user who has it. Dates move back by
Medorama's offset (drawn at random when Medorama is set up: 80 to 120 years and some days; an
installation made before keeps 36,570 days, and `medorama serve` warns about it until another
is set; it can be given in years, months, days and hours) plus
each patient's own number of days, between 0 and `spread_days` (by default 365, at least 30, at
most 3,650), drawn once per patient and kept with the patient's pseudonym. How the privacy gate
applies these settings is explained in [protecting-patient-data.md](protecting-patient-data.md).

Admin › Dates and times chooses how every page writes dates and times (2024-01-31, 01/31/2024,
31/01/2024 or 31.01.2024, with a 24-hour or a 12-hour clock). Exports and the API always use
ISO 8601.

## Choose the sign-in method

Medorama has two sign-in methods, chosen by the `sign_in` setting. Whichever you choose, accounts,
their roles, grants and the PHI permission stay in Medorama, and everything after sign-in (what a
user may see and do, Privacy Mode, the audit log) works the same way. [sign-in.md](sign-in.md)
explains both methods in full, gives proxy configurations for common single sign-on products, and
shows how to add another method.

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
   Include at least one administrator; for a new setup, `medorama init --admin NAME` gives the
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

<a id="databases"></a>

## Connected databases

Every source of data about patients is a **connected database**, which Medorama reads where it
is, at each request, read-only: an OMOP CDM instance, a research warehouse, a SQLite file made by
db-builder. Medorama keeps no copy of the data it reads. Of each database it keeps only what
links its patients and names what it holds: its persons as linked and their encounters, the
names and units of its variables (counted only when asked), and what Medorama did itself (the
recordings it has opened, their indexes, the results of actions), in that database's encrypted
folder of the workspace ([databases.md, What Medorama keeps of a database](databases.md#what-medorama-keeps-of-a-database)).
[databases.md](databases.md) says what a database must provide, and how to prepare one.

| Aspect | Details |
|---|---|
| Kinds | `omop`: an OMOP CDM 5.3 or 5.4 database, read through built-in views. `views`: any SQL database, read through views that the site writes (`persons`, `visits`, `entries`, and `recordings` for a database of recordings, a `version` query, an optional `variables` catalog and an optional `has` list). `dbb`: a database made by [db-builder](../db-builder/README.md), whose tables are the views already. |
| Database engines | PostgreSQL (on-site, Amazon RDS and Aurora, Azure) and SQLite files. A SQLite file must lie inside `data_roots`, and may not be Medorama's control store or lie inside its workspace. |
| Access | Read-only, and enforced: PostgreSQL sessions are read-only, and SQLite files are opened read-only. Each query has a time limit (`timeout_s`, 60 seconds by default), and each criterion a limit on the rows it keeps (`max_rows`, 1,000,000 by default: each row is tested as it is read, so a criterion reads a database of millions of rows; the persons behind `has …` are not bounded; it also bounds **Index all recordings**, but not the persons and visits read when connecting). |
| Passwords | A password given to Medorama (in the page, or with `databases connect --password-stdin`) is sealed with the master key and kept in the control store, never in a file and never shown again. Or it is in a file (`--password-file`), or in `$MEDORAMA_SOURCE_PASSWORD_<NAME>`. It is never stored in the URL, and never sent back to the page. |
| Who connects | Administrators. The administrator who connects a database gets every grant on it. |

Tables, spreadsheets, known exports (GE MUSE ECG metadata, CLIF folders, a site's export
described in TOML) and folders of recordings are not read by Medorama directly: make them into a
database with db-builder, then connect it ([db-builder's README](../db-builder/README.md)):

```bash
db-builder add /data/study/study.sqlite patients.csv labs.xlsx --issuer HOSP --mrn-authority HOSP
db-builder recordings /data/study/study.sqlite /data/icu --patients icu_index.csv
medorama databases connect study --kind dbb --url sqlite:/data/study/study.sqlite
```

### Connect a database

**In the page:** Databases › **Connect a database** (administrators). Give a name, the kind
(db-builder, OMOP or views), where it is (a file on this server, chosen with **Choose…**, or a
PostgreSQL server), its password if any, and its settings. Medorama checks the connection first:
each view must answer. It then makes the database's data key and workspace folder, reads its
catalog (the names and units of its variables), links its persons and keeps their encounters,
and shows the database's page. It counts no entry and reads no recording: one pass over the
persons and one over the visits, minutes for millions of persons.

**From the command line**, for example an OMOP database on PostgreSQL, another in a SQLite file,
and a warehouse through views:

```bash
medorama databases connect omop --kind omop --url "postgres://reader@omop.example.org/cdm?sslmode=require" \
    --password-file /etc/medorama/omop-password --schema cdm --vocabulary-schema vocab --mrn-authority HOSP
medorama databases connect county --kind omop --url sqlite:/data/county/omop.sqlite --identifier mrn
medorama databases connect warehouse --kind views --url postgres://… --views warehouse-views.toml --issuer HOSP
medorama databases check omop        # online or offline; read again if its data changed
medorama databases refresh omop      # read its persons, encounters and catalog again now
medorama databases views warehouse --views warehouse-views.toml   # new views; its patients keep their numbers
medorama databases count omop        # count each variable's entries and persons
```

The command prints what it found: the data's version, how many persons were linked to how many
patients, the variables, whether its recordings are read when a patient is opened, and the
identity conflicts if any.

| `databases connect` option | Meaning |
|---|---|
| `NAME` | Its name, as criteria write it (`omop.lactate`): letters, digits, `-` and `_`. A name already used is refused. |
| `--kind omop\|views\|dbb` | Its kind (above). |
| `--url URL` | `postgres://user@host/db?sslmode=require`, or `sqlite:/path/to/file`. |
| `--password-file F`, `--password-stdin` | Its password: in a file, or read from standard input and kept sealed by the master key. Without either, `$MEDORAMA_SOURCE_PASSWORD_<NAME>` is used if set. |
| `--ca-file F` | Certificate authorities (PEM) for its server's certificate. |
| `--views FILE.toml` | Kind `views`: a TOML file with the SQL of `persons`, `visits`, `entries`, `recordings`, `stays`, `version`, `variables` and `has` ([databases.md](databases.md#canonical-views-reference)). The views are run as written, in the database's own SQL: Medorama puts them inside its own queries and does not translate them. |
| `--schema S`, `--vocabulary-schema S` | OMOP: the schema of the CDM's tables, and of its vocabulary. Plain names (letters, digits, `_`). |
| `--identifier mrn\|patient_id\|none` | OMOP: what `person_source_value` holds (by default `mrn`). With `none`, each person is a patient of its own, unique to this database. |
| `--issuer I` | Who issued its patient IDs ([Identity](#identity)). Empty: the database itself, so that its patient IDs link only its own persons. |
| `--mrn-authority A` | The authority of its MRNs, for rows that name none. |
| `--timezone TZ` | The time zone of its times written without an offset (an IANA name; an unknown one is refused). Without it, the `timezone` setting is written into the database's settings when it is connected, and stays fixed from then on. On PostgreSQL, a `timestamp` column (without a time zone) is read in it; a `timestamptz` is an instant. |
| `--files-root DIR` | The folder that relative recording paths are read from. Without it: a SQLite database's own folder. |
| `--timeout-s N`, `--max-rows N` | The time limit of each query, and the limit on the rows each criterion keeps and on **Index all recordings** (0: the defaults). A large database needs no higher `--max-rows` to connect: its persons and visits are read however many. |

Connecting refuses a database that does not answer, a SQLite file outside `data_roots`, and a
name already used. If the database answers but reading it fails part way, it stays connected, the
command says so, and **Refresh** reads it again.

### Identity

Medorama does not decide who is who: each database names its patients, and Medorama joins
databases on exact matches. [databases.md](databases.md) sets out what this requires of a
database.

- **Each person has (issuer, identifier) pairs**: the patient ID under its issuer (the
  `patient_id_issuer` column, else the database's `issuer` setting, else the database itself),
  and the MRN under `mrn:<authority>`. A person with neither is a patient of this database alone.
- **A pair equal in two databases is one patient.** Medorama keeps, in the control store, a keyed
  hash of each pair and the patient number it leads to; the identifiers themselves are kept only
  in the database's workspace folder (`persons.sqlite`, encrypted).
- **Conflicts are listed, never merged.** A person whose pairs lead to two different patients is
  an identity conflict: it is listed on the database's page (**Identity conflicts**) and by
  `medorama databases conflicts NAME`, and it is not linked. Correct the database, then refresh
  it. Medorama has no scored matching, merging or splitting of patients.
- **Linking** reads every person of a database when it is connected or refreshed (however
  many), with their encounters (each visit's key, start, end and whether it was in intensive
  care), and links new persons as searches find them.

### Status, versions and variables

- **Status.** Each database is checked when the server starts, every 60 seconds, and on demand
  (**Check now**, `medorama databases check`): a SQLite file must be present, and each view must
  answer. The Databases tab shows a green dot for online and a grey one for offline, with when it
  was last checked; those who manage the database also see why it did not answer. Offline
  databases are listed after the online ones.
- **An offline database keeps everything**: its workspace, its variables, its grants, and its
  patients on the patient list, with their encounters (their data show as offline). It is not
  searched: a cohort search names it as left out, and a patient's page lists it as not
  available.
- **Versions.** Each check reads the data's version: OMOP's `cdm_source`, a db-builder
  database's `meta.version`, a views database's `version` query, or else counts of its persons
  and recordings (not of its entries). A changed version reads its persons, their encounters and
  its catalog again, and drops the counts. **Refresh** (`medorama databases refresh NAME`) does
  it on demand. `medorama databases views NAME --views FILE` gives a views database new views
  (a persons view that leaves out IDs that are not MRNs, say) and reads it again: a person is
  linked by the same identifier pairs, whatever view gives them, so its patients keep their
  numbers.
- **Variables.** What a database holds (each measurement's name, code, vocabulary and unit;
  diagnoses, drugs and other categories) is its catalog, kept in its `variables.sqlite` with the
  categories of its recordings. It comes from the database's `variables` view when it has one
  (no entry read), else from one grouped query over its entries, with no counting. The
  database's page lists them (**Variables**), and so does `medorama databases variables NAME`. A
  search sends each criterion only to the databases whose variables may hold it.
- **Counts.** How many entries and persons hold each variable is counted only when someone who
  manages the database asks: **Count now** on its page, or `medorama databases count NAME` (one
  grouped query over its entries, which takes a while on a large database). The counts are kept
  until the catalog is read again. A count from 1 to 10 is shown as "< 11", in the page, the
  command and the API.

### Files and recordings

A database's `recordings` (a db-builder database's table, or a `recordings` view) names each file
with its patient, visit, start, end, format and category. A relative path is read from
`files_root`, else from a SQLite database's own folder; the file must lie inside `data_roots`.
Files are only read, and they are shown only to users granted `view` on the database.

| The row's category | The file is | It is |
|---|---|---|
| ICU, Holter, EEG, Sleep | A WFDB folder, a WFDB header, an EDF or BDF file, or a file that an installed adapter reads | A **recording**, indexed in the background and opened in VitalStream. |
| ECG | A WFDB header, GE MUSE XML, a DICOM ECG, a JSON ECG, an EDF file, PNG, JPEG or PDF | Attached to the patient's page, shown in the ECG viewer. |
| Echo, Cath lab, Imaging | DICOM files and series (a folder), videos, pictures, PDF | Attached to the patient's page, shown in the PACS viewer. |
| Any other | A picture, a PDF, a video or a text file | Attached to the patient's page, shown by the page. |

**When recordings are read.** Connecting reads no recording. A patient's recordings are read
from the database when their page, a job or a cohort's selection asks for them: their files are
then checked (present, inside `data_roots`, not in the workspace, readable), fingerprinted and
kept in the database's `recordings.sqlite`, and those a reader indexes are queued for indexing.
**Index all recordings** on the database's page (`medorama databases index NAME`, who manages
it) reads every recording the database lists at once (at most `max_rows`), for a database of
ICU recordings that should all be indexed ahead of time; `medorama init --demo` does it for the
demo's databases. The Recordings tab and `medorama recordings` list the recordings kept: those
opened or indexed. A cohort's `has` asks the database itself. When a database is offline, its
recordings kept from before are shown.

Every recording has a number across Medorama (the database's id times 1,000,000,000, plus its
own number), which the page, jobs and `medorama recordings` show. Each recording gets a
fingerprint, its **index key**, under which its index is kept in its database's index store
(`db-NNNN/index.sqlite` in the workspace, encrypted with the database's key; index format 3:
[index-format.md, Version 3](../viewers/vitalstream/docs/index-format.md#version-3-the-index-store)).
After a recording changes or goes, the indexes that no recording of its database uses any more are
removed from the store. A **check** (`medorama check`,
and every `check_interval_s`) finds missing and changed recordings and lost indexes. A recording
file that changes in place is a new recording: databases should not change files in place.

**How a folder is read.** A folder named as a recording is itself a recording when it is a WFDB
folder. It is then read with its subfolders, as the recording engine reads it, within the
engine's folder limits: 5000 records, 20 levels, and 10 seconds to count them. Medorama has no
setting for these limits; the VitalStream viewers do
([wfdb-collection.md, Limits](wfdb-collection.md#limits)). Paths are compared with symbolic links
resolved.

## Grants

A user may use a connected database or a cohort only when granted it. Grants are kept in the
control store, per user, per database or cohort, and per permission.

| On a database | Lets the user |
|---|---|
| `search` | Find its patients with cohort criteria. |
| `view` | See its data on patients' pages, its recordings, and open them in the viewers. Implies `search`. |
| `phi` | See its identifiers (names, MRNs, dates of birth), with the user's PHI permission, outside Privacy Mode. Implies `view`. |
| `manage` | Check, refresh and remove it, grant it to others, and see its URL, settings and identity conflicts. Implies `phi`. |

| On a cohort | Lets the user |
|---|---|
| `view` | See the cohort and its members. |
| `edit` | Change its criteria and members. Implies `view`. |
| `manage` | Share it (grant it) and delete it. Implies `edit`. |

- **Who has them.** The administrator who connects a database, and the user who makes a cohort,
  get every grant on it. Other administrators are **not** granted a database by being
  administrators: they see it listed, so that they can grant it, and may grant it to themselves
  (**Grant it to me** on its page), which is written to the audit log (`grant_self`).
- **Where.** A database's page (Databases › the database › **Who may use it**) lists its grants,
  grants a permission to a user, and revokes a user's permissions. On the command line:
  `medorama databases grants NAME`, `databases grant NAME USER PERMISSION`,
  `databases revoke NAME USER PERMISSION` (revoking a permission revokes the stronger ones too).
  A cohort is shared from its page.
- **What they protect.** The server unwraps a database's data key only for a request whose user
  holds a grant on it: keys protect the workspace's files at rest, grants protect them in use.
  Revoking a grant changes nothing in the data. Every grant and revocation is audited.
- **The terminal.** The commands, run by the account that owns the home folder, hold every grant
  ([Command reference](#command-reference)).

## Keys

Everything in the workspace's SQLite files is encrypted with **envelope encryption**: each owner
(a connected database, a cohort, a derived database) has its own 256-bit **data key**, and the
control store keeps that key only **wrapped** (AES-256-GCM) by the **master key**. The master key
is never in the workspace, and never in the control store.

**Where the master key comes from**, the first of:
1. `$MEDORAMA_MASTER_KEY`: 64 hex digits, set by the service manager (for example an environment
   file readable by Medorama's account only);
2. `master_key_command`: a command that prints the 64 hex digits, such as the operating system's
   keyring (`security find-generic-password -s medorama -w` on macOS, `secret-tool lookup service
   medorama` on Linux) or the command line of a key manager (HashiCorp Vault, a cloud KMS).
   Medorama has no built-in client for these: it runs the command;
3. `master_key_file`: by default `<home>/master.key`, made by `init` (mode 0600).

**Where to keep it.** Apart from the workspace and from the workspace's backups: a copy of the
workspace without the master key cannot be read. The default file in the home folder suits one
machine; on a server, prefer a key manager or the service manager. Keep a copy of the master key
in a safe place of its own: **without it, nothing in the workspace can be read**.

**Make a new master key and rotate.** Rotation wraps every data key, and seals the passwords of
connected databases given to Medorama, again with a new master key; the encrypted files are not
touched (a rotation interrupted can be run again):

```bash
medorama keys new /etc/medorama/master-2027.key            # 64 random hex digits, mode 0600
medorama keys rotate --new-key-file /etc/medorama/master-2027.key
```

Then point `master_key_file` (or `$MEDORAMA_MASTER_KEY`, or the key manager) at the new key, and
restart the server. Keep the old key until no backup needs it: a backup of the control store made
before the rotation holds keys wrapped by the old one. Rotation is written to the audit log
(`keys_rotate`).

**Backups and shredding.** A backup of the control store holds the wrapped data keys of its day.
Restoring an old backup of both the control store and the workspace brings back the files of a
database removed since. Removing a database is therefore final once the backups older than the
removal have expired; the removal's audit entry gives the date from which it holds
([Backup, update, uninstall](#backup-update-uninstall)).

## Removing a database

**Databases › the database › Remove…** (typing its name to confirm), or
`medorama databases remove NAME`, for a user who manages it. It removes every trace of the
database from Medorama, in six steps:

1. The database is marked `removing`: it is no longer offered, searched or opened. The viewer
   is told to forget its index store.
2. **Its data key is destroyed**, and those of the databases derived from it. From then on its
   workspace files, its index store included, cannot be read.
3. Its workspace folder is deleted, with its index store, and the folders of what was derived
   from it.
4. Every cohort row whose provenance includes it is deleted, in every cohort.
5. Its patient links (keyed hashes) are deleted; patients that no database links any more are
   deleted with their pseudonyms, and taken out of the cohorts.
6. Its jobs, visit pseudonyms, grants, derived databases and its row are deleted, and the
   connections to it are closed. The audit log records the removal (who, when, its name; no
   patient data).

Each step can be repeated. A removal that a stop or a crash interrupted is finished when the
server or any command next starts. The indexes that no remaining recording uses are then
removed from the other databases' stores. **The database itself is never touched**: removing it from Medorama does not change or
delete a single row or file of it.

The audit log keeps its earlier entries, which name patients by number: health-records law
requires audit logs to be kept, and the numbers lead nowhere once the links are gone.

## Add-ons

Recording formats that Medorama does not read itself, including licensed formats, are read by
**adapters**: programs installed in the adapter folder (`adapters_dir`, by default
`~/.local/share/medorama/adapters`). [adapters.md](adapters.md) describes the protocol and how the
programs use adapters. One example is stp-adapter, which reads BedMasterEx `.Stp` files; it is
private and available to institutions under agreement.

To install, list and remove add-ons, run as Medorama's account:

```bash
medorama addons install stp-adapter-0.1.0-linux-x86_64/stp-adapter
medorama addons list                      # name, version, implementation, formats, program
medorama addons remove stp
```

[adapters.md, Installing an add-on](adapters.md#installing-an-add-on) gives the steps in full,
from the archive to the first recording: unblocking a downloaded program on macOS and Windows,
restarting the server, and what to do when something goes wrong. Admin › Add-ons lists the
installed adapters. Add-ons are installed on the server only, never through the page, and every
installation and removal is audited.

## The waveform viewer

Medorama runs one VitalStream viewer for all its users and shows it in its **VitalStream** tab
([architecture.md](architecture.md#running-separately-and-together)). The viewer programs are the
standard ones, unchanged.

| `viewer` setting | The viewer Medorama runs |
|---|---|
| `auto` (default) | The one that `tools/install-medorama` installed (recorded in `medorama.viewer` next to Medorama's program), or else the first found of `vitalstream-cpp`, `vitalstream-rs` and `vitalstream-py`. |
| `cpp`, `rs`, `py` | That viewer. |
| A path | That program. |
| `none` | No viewer. **View** and the VitalStream tab are greyed out, with the reason. |
| (`viewer_url` set) | Medorama runs no viewer, and **View** opens that viewer in a new browser tab. |

**How the viewer runs.** Medorama starts the viewer at the first **View** or the first visit of
the tab, as a separate process listening on 127.0.0.1 at a free port. It restarts the viewer if
it stops, and stops it with the server. The viewer listens only on 127.0.0.1, so that other
machines cannot reach it, and it answers only requests that Medorama has signed (below).
- **Its settings** (`<home>/viewer/vitalstream.toml`, written again at each start) give it
  Medorama's `adapters_dir`, time zone, `data_roots` and `qc_verbose`, `cache_dir =
  <workspace>/viewer`, and `require_password = true`, so that a viewer built without password
  support refuses to run under Medorama.
- **Its index stores.** Medorama starts the viewer with `--keys-on-stdin` and writes on its
  standard input one JSON line per store: each recording database's store
  (`<workspace>/db-NNNN/index.sqlite`) with that database's key, and the viewer's own store
  (`<workspace>/viewer/index.sqlite`, with a key of its own, `"own": true`), where the viewer
  writes what it indexes itself. It writes the lines at each start, a line again when a database
  is connected, and `{"forget": …}` before a database is removed. The viewer keeps the keys in
  memory only; a key is never on a command line, in the environment, in a file or in a log
  ([index-format.md, The key on a pipe](../viewers/vitalstream/docs/index-format.md#version-3-the-index-store)).
- **Its root.** Medorama's installer builds the viewer with the whole file system as its root
  (`tools/install-medorama.sh`), because Medorama's own rules, below, decide what each user may
  open.
- **Its password** (`<home>/viewer/auth.json`) holds a signing key that only Medorama knows.
  Medorama signs every request it passes on, users never see the viewer's sign-in, and the
  viewer's port is useless to anyone else.
- **Its output** goes to `<home>/viewer/viewer.log`.

**The gateway.** Medorama passes requests to the viewer at `/viewer/…` only after its own
sign-in, and only the following requests; everything else is refused.

| Viewer request | Administrators | Analysts and viewers |
|---|---|---|
| The page and its files | Allowed. | Allowed. |
| `GET api/v1/config` | As the viewer answers. | Its shortcuts point into the folders of the recordings the user may view. |
| `GET api/v1/browse`, `complete` (the Open dialog) | Inside `data_roots`. | Only folders that lead to recordings of databases granted `view`, listing only those recordings. |
| `GET api/v1/scan` (what a folder of records holds, before it opens) | Inside `data_roots`. | A recording of a database granted `view`. Refused in Privacy Mode. |
| `POST api/v1/open` | A recording inside `data_roots`. | A recording of a database granted `view`. Every open is recorded in the audit log (`view`). |
| `POST api/v1/cancel` (stops building an index) | Datasets the user opened through the gateway. | The same. |
| `GET api/v1/datasets/{key}…` | Datasets the user opened through the gateway. | The same. |
| Anything else, including the viewer's `login` and `logout` | Refused. | Refused. |

Paths are compared after resolving symbolic links. The viewer's page may be framed only by
Medorama's page. The viewer receives paths and recordings, never identifiers. In Privacy Mode,
recordings are opened by token, folders are not browsed, and the gateway translates times. The
gateway answers 413 to a request body over 1 MiB, 415 to a `POST api/v1/open` whose body is not
sent as `application/json`, and 400 to a body that is not valid JSON.
`medorama/tests/gateway.rs` runs a real viewer behind the gateway and checks every rule.

**A viewer you run yourself**, on another port, keeps its own index store (`cache_dir/index.sqlite`,
encrypted when its `index_key_file` is set): it cannot read Medorama's stores, whose keys only
Medorama has. It can use Medorama's adapters: set its `adapters_dir` to Medorama's.

## db-builder

[db-builder](../db-builder/README.md) turns tables, exports and folders of recordings into the
databases Medorama connects. Its web interface (`db-builder serve`: [db-builder/README.md, Web
interface](../db-builder/README.md#web-interface)) can run under Medorama, so that an
administrator builds a database in one browser tab and connects it in Medorama's, next to it.

1. **Choose its folder**: `db_builder_root = "/data/build"` in `medorama.toml` (or `medorama
   serve --db-builder-root /data/build`). db-builder makes databases there, reads its inputs from
   there, and lists, reads and writes nothing above it. Put it inside `data_roots`, so that what it
   builds can be connected: `serve` warns at start when it is not.
2. **Set its password once**, as the operating-system user who runs Medorama:

   ```bash
   db-builder serve --set-password --home <home>/db-builder      # <home>: Medorama's home folder
   ```

   Its account is its own, `admin` unless `--user NAME` names another; Medorama's accounts do not
   sign in to it. Until the password is set, Medorama does not start it, and says so (at start, and
   beside **Create a database…**).
3. **Restart Medorama.** It finds the `db-builder` program where it finds the viewer (next to its
   own program, `.venv/bin`, the workspace's `target/release`, the `PATH`;
   `tools/install-medorama` installs it there) and runs `db-builder serve --root
   <db_builder_root> --host 127.0.0.1 --port <db_builder_port> --home <home>/db-builder` as its own
   process, from its start to its end. It starts it again if it stops (after 5 seconds, longer if
   it keeps stopping), and stops it when it stops. Its output goes to
   `<home>/db-builder/db-builder.out`, its log of actions to `<home>/db-builder/db-builder.log`.

The **Databases** tab's **Create a database…** (admins only: `GET api/v1/config` names
db-builder's address to admins alone) opens db-builder's page in a browser tab named
`db-builder`, so that a second click shows the same tab. Medorama does not pass db-builder's
requests on, and shares no sign-in, cookie or key with it: db-builder listens on 127.0.0.1, so its
page opens in a browser on the server's computer (or through an SSH tunnel to its port). It is
not started with `--encrypt`; to build encrypted databases, run `db-builder serve --encrypt`
yourself, with its key in `$DB_BUILDER_KEY`.

**A db-builder running elsewhere**: set `db_builder_url = "http://host:8091/"` and Medorama runs
none; **Create a database…** opens that address.

## The audit log

The audit log is append-only, and administrators read it in the Admin tab or with
`medorama audit`. It records:
- sign-ins and failed sign-ins (with proxy sign-in, one `login` per visit);
- databases connected, checked, refreshed and removed, persons linked, grants given (also to
  oneself) and revoked, and master key rotations;
- runs of actions, and add-ons installed and removed;
- changes to users and settings;
- Privacy Mode switched on and off;
- for users with the PHI permission, `phi_search` (with the search text), `patient_view` (a
  patient opened) and `phi_export`;
- for sessions in Privacy Mode, `privacy_search` (searches, filters and cohort criteria, with
  their text);
- cohorts made, changed, refreshed, deleted and exported, and recordings opened in the viewer
  (`view`).

In Privacy Mode, the audit page shows no target or detail. Entries are kept after a database is
removed ([Removing a database](#removing-a-database)).

The log is **sealed** every minute. Each seal is a SHA-256 chain over the entries written since
the previous seal, so that an entry changed, deleted or added afterwards breaks the chain.
`medorama audit --verify` checks the seals, and `medorama audit --seal` seals now. With
`audit_export` set, each seal and its entries are also appended to that file, which keeps a copy
out of reach of the control store's administrators ([security.md](security.md)).

An entry is never lost when the control store cannot take it at once (busy beyond its wait, or
failing): it is kept, in order, and written before the next entry or by the next seal; with
`audit_export` set it is appended to that file at once (a line with `"pending": true`), and the
server's output says so.

<a id="the-database"></a>

## The control store and the workspace

**The control store** is Medorama's own database: SQLite, in write-ahead-log mode (one writer at
a time, readers never blocked) with foreign keys on; or PostgreSQL 14 or later. The code is the
same for both: SQL is written once and translated for PostgreSQL, and every test runs on both.
The schema ([`medorama/src/schema/`](../medorama/src/schema)) is applied when the control store
is created, and a program older than its control store refuses to open it. IDs are never reused,
and times are stored as Unix seconds (UTC).

| Group | Tables |
|---|---|
| Access | `users`, `sessions`, `grants`, `history` (the patients a user opened, by number) |
| Databases | `databases` (each connected database: kind, URL without a password, its password sealed if one was given, settings, status, version), `derived`, `derived_sources` (results databases and the databases they come from) |
| Keys | `keys` (each data key, wrapped by the master key, with the master key's id) |
| Patients | `patients` (numbers and pseudonyms only, and each patient's draw for Privacy Mode's date offset), `patient_keys` (keyed hashes of (issuer, identifier) pairs, with the patient number each leads to), `visit_pseudonyms` |
| Cohorts | `cohorts` (definitions: name, owner, criteria, the databases searched, their versions) |
| Work | `runs`, `jobs` (a finished job holds its result) |
| Audit and settings | `audit`, `audit_seals`, `settings` (Privacy Mode's settings), `data_version` (the gate's secret is with the wrapped keys, `keys`, owner `gate`) |

**The workspace** (`workspace_dir`) holds what Medorama generates, each SQLite file encrypted
with its owner's data key (SQLCipher, AES-256):

| Path | What it holds |
|---|---|
| `db-NNNN/persons.sqlite` | The database's persons as linked: identifiers, names, birth dates, sex, patient numbers and their hashes; their encounters (key, start, end, intensive care); identity conflicts. |
| `db-NNNN/variables.sqlite` | Its catalog: each variable's category, name, vocabulary, code, unit and whether it is numeric (its entries and persons once counted); its recordings' categories; the version they were read at. |
| `db-NNNN/recordings.sqlite` | The recordings Medorama has opened or indexed: path, kind, size, fingerprint, format, start and end, label, category, whether it is present; derived series. |
| `db-NNNN/results-<action>.sqlite` | The numbers of an action's results, searchable as `<action>.<field>`. |
| `db-NNNN/results/<action>/r<n>/j<job>/` | An action plug-in's output folder: the files it writes. **Not encrypted.** |
| `cohorts/c-NNNN.sqlite` | One cohort's members, how each came in, and the databases that put it there. |
| `db-NNNN/index.sqlite` | The index store of the database's recordings (index format 3: waveform envelopes, vital-sign series, markers), encrypted with the database's key. Temporary folders of indexes being built are in `db-NNNN/index.tmp/`, readable by their owner only, and removed when the build ends. |
| `viewer/index.sqlite` | The viewer's own index store: what the viewer indexes itself, encrypted with a key of its own (owner `viewer`). |

Folders are created with mode 0700 and files with 0600. A workspace file does not open without
its key; once a database or cohort is removed, its files cannot be read even from a copy of the
disk: removing a database destroys its key first, then its folder with its index store. The
plug-ins' output folders are the only part of the workspace not encrypted in this version: keep
the workspace on an encrypted disk.

**Encrypting the control store.** The SQLite control store holds no patient data read from a
database, but its password hashes, wrapped keys, keyed hashes and audit log are worth protecting.
It is encrypted with SQLCipher when its key comes from `$MEDORAMA_KEY` or from the file that
`key_file` names; set it **before** `init`: a control store created without a key stays
unencrypted, and losing the key loses it. On PostgreSQL, encryption at rest is the database
server's (an encrypted disk or cloud volume), and `key_file` is refused.

**Several users at once.** One server process serves all users, and its workers and the commands
share the control store safely.

## Deployment

Choose the arrangement that fits how people will reach Medorama. Before real patient data are
connected, go through the deployment checklist in [security.md](security.md).

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

**A TLS proxy.** The following nginx server block is a minimal example:

```nginx
server {
    listen 443 ssl;
    server_name medorama.example.org;
    ssl_certificate     /etc/ssl/certs/medorama.pem;
    ssl_certificate_key /etc/ssl/private/medorama.key;
    location / {
        proxy_pass http://127.0.0.1:8090;
        proxy_set_header Host $host;
    }
}
```

**Who can reach the data.** Anyone who can read the home folder and the master key can read the
control store and the workspace, and the commands run with every grant. Run the server under a
dedicated account, and let people use the web page. On PostgreSQL, give Medorama's account access
to its own database only. Give Medorama read-only accounts on the databases it connects.

### On a shared Linux cluster

This procedure sets Medorama up on a shared cluster without administrator rights, with the data
on a shared file system and people reaching it from their own computers. The example paths are
those of a MUSE ECG collection; use your own.

1. **Choose where it runs:** a machine where a long-running program is allowed and that sees the
   data, such as a service node or a virtual machine. Do not use a login node or a batch job.
2. **Build Medorama in your account.** The Rust viewer needs no C++20 compiler. The build needs a
   C compiler and Perl (for OpenSSL, which SQLCipher uses):
   ```bash
   git clone https://github.com/alphanumericslab/medorama.git ~/medorama && cd ~/medorama
   module load gcc 2>/dev/null || true          # any C compiler (or tools/install-gcc.sh)
   tools/install-medorama.sh --viewer rs        # asks you to accept the license terms first
   umask 077                                    # what you make next is yours alone
   ```
3. **Choose the control store.** SQLite needs a local disk, because its locking is unreliable on
   NFS, Lustre or GPFS. Where the home folder can only be on such a file system, use
   **PostgreSQL**: your institution's server, or one in your account, for example from
   conda-forge:
   ```bash
   conda create -y -p ~/pg -c conda-forge postgresql=16 && export PATH=~/pg/bin:$PATH
   initdb -D ~/pgdata -A scram-sha-256 -U medorama -W          # asks for a password
   pg_ctl -D ~/pgdata -o "-k /tmp -c listen_addresses=127.0.0.1 -p 15432" -l ~/pgdata/log start
   createdb -h 127.0.0.1 -p 15432 -U medorama medorama
   echo 'the password' > ~/.medorama-db-password && chmod 600 ~/.medorama-db-password
   ```
4. **Set Medorama up:**
   ```bash
   H=~/medorama-home
   .venv/bin/medorama init --home $H --workspace ~/medorama-workspace --timezone America/New_York \
     --database-url postgres://medorama@127.0.0.1:15432/medorama        # or leave it out: SQLite in $H
   ```
   Then set these in `$H/medorama.toml`: `port = 18090` (a port no one else uses),
   `data_roots = ["/data/ecg/muse", "/home/you/medorama-databases"]` and
   `database_password_file = "/home/you/.medorama-db-password"`.
5. **Make the data into a database and connect it.** db-builder reads the MUSE metadata where it
   is and writes one SQLite file, outside the workspace:
   ```bash
   M=/data/ecg/muse/xml_metadata
   mkdir -p ~/medorama-databases
   .venv/bin/db-builder add ~/medorama-databases/muse.sqlite $M/*.csv --issuer HOSP --mrn-authority HOSP
   .venv/bin/db-builder check ~/medorama-databases/muse.sqlite
   .venv/bin/medorama --home $H databases connect muse --kind dbb --url sqlite:$HOME/medorama-databases/muse.sqlite
   ```
6. **Run it** so that it outlives your session, in `tmux` (or as a user service): run
   `tmux new -s medorama`, then `.venv/bin/medorama --home $H serve`.
7. **Reach it** from your computer through SSH, through the login node with `-J`:
   `ssh -N -J you@login.cluster.example.edu -L 8090:127.0.0.1:18090 you@service-node`, then open
   <http://127.0.0.1:8090>.
8. **Add people** in Admin › Users, grant them the database (Databases › muse › **Who may use
   it**), and give the PHI permission and the `phi` grant only to those who may see identifiers.
   Everyone logged in to that machine can reach its loopback port, and so the sign-in page. Use
   strong passwords, keep `$H` readable by you alone, prefer a dedicated account, and put HTTPS in
   front for a team.

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

Add `--home DIR` before `serve` if the home folder is not in the default place. To give the
master key from the service rather than from the file in the home folder, add
`EnvironmentFile=%h/.config/medorama/master.env` under `[Service]`, where that file (mode 0600)
holds the line `MEDORAMA_MASTER_KEY=` followed by the 64 hex digits; or set `master_key_command`.
Without systemd, run `tmux new -d -s medorama 'medorama serve'`.

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

On macOS, the master key can be kept in the login keychain instead of a file, and read with
`master_key_command = "security find-generic-password -s medorama -w"`.

**Windows (started at log-on, without administrator rights).** These commands add Medorama to
your Startup folder and start it now:

```powershell
$exe = (Get-Command medorama).Source
$startup = [Environment]::GetFolderPath('Startup')
Set-Content "$startup\medorama.cmd" "@start `"Medorama`" /min `"$exe`" serve"
& "$startup\medorama.cmd"               # start it now
```

## Backup, update, uninstall

**Back up** three things, and keep the third apart from the other two:
1. **the home folder**: the settings, the control store (if SQLite) and the plug-ins;
2. **the workspace**: its encrypted files (cohorts' members, results of actions, the databases'
   persons, encounters and variables). The indexes in it can always be rebuilt;
3. **the master key**, in a safe place of its own. Without it no backup of the workspace can be
   read; with it, anyone who holds the workspace's backup and the control store's can read them.

- SQLite control store, while the server runs:
  `sqlite3 medorama.sqlite ".backup /backup/medorama-$(date +%F).sqlite"`. This does not work for
  a control store encrypted with `key_file`: copy that one while the server is stopped.
- PostgreSQL: `pg_dump -Fc medorama > medorama-$(date +%F).dump`, or the database server's
  point-in-time recovery. Encrypt the dumps.
- The workspace: copy it while the server is stopped.

**Backups and removals.** Restoring a backup of the control store and of the workspace made before
a database was removed brings that database's files back. Removal is final only once every such
backup has expired: set backup retention with this in mind ([Keys](#keys)). The connected
databases' own backups are their owners'.

**Update.** Run `git pull`, build again ([building.md](building.md#update-and-uninstall)) and
restart the server. A newer version brings its control store's changes and applies them at its
first start. An older version refuses to open a control store that a newer one has updated, so
back up first. From 0.2 to 0.3 nothing is done by hand: the control store's migration clears
each database's catalog version, so that each database is read again (persons, encounters,
catalog) at its next check, and each workspace's 0.2 tables are replaced when they are first
opened (the per-person summary dropped, the catalog made again without counts); the recordings
0.2 kept stay kept. The exception is 0.1.0: 0.2.0 refuses its control store and is set up in a
new home ([upgrading from 0.1.0](install.md#upgrading-from-010)).

**Uninstall.** Stop the service, delete the program (`cargo uninstall medorama`, or the copy), and
delete the home folder, the workspace and the master key if they are no longer needed. The
connected databases and the recordings are not touched.

## The demo

The demo is a set of invented patients for trying Medorama, showing what it does and teaching
it: about 40 patients in six databases, as three hospitals, a research registry and an ECG
archive would hold them, with enough data for every feature.

| Database | Kind | What it holds |
|---|---|---|
| `demo` | `dbb` | The main hospital (db-builder, from `databases/`): 40 patients, their visits, ICU and bed stays, laboratory results with ranges and flags, vital signs, medications, transfusions, life support, procedures, ICD-10 diagnoses, assessments and scores, microbiology, pathology, allergies, notes that name the patient (for Privacy Mode to replace), imaging, echo and cath reports with measurement columns; and files: ICU recordings (WFDB collections with ECG, arterial pressure, pleth, respiration, vital signs, alarms, R-wave and PVC markers, a break and a stretch without stated time), Holter ECGs, overnight sleep studies (EDF+, scored), routine EEGs (BDF+), 12-lead ECGs (WFDB, GE MUSE XML, DICOM, PDF and PNG), echo loops and structured reports, CT series, chest radiographs and coronary angiography. Some sleep studies and EEGs are added as folders of recordings by a file-name pattern (`db-builder recordings --pattern`). |
| `demo_north` | `dbb` | A second hospital: some of the same patients by the same MRN authority (`DEMO`), one of them with its MRN in lower case (linked ignoring case), one identity conflict (one patient's MRN with another's patient ID), patient IDs written `PT_123` that its rules file makes `PT123`, and ECG files whose folder is moved after the database is built and repointed with `db-builder rewrite`. |
| `demo_omop` | `omop` | A third hospital's OMOP CDM database: some of the demo's patients by MRN, and patients of its own. |
| `demo_registry` | `views` | A heart-failure registry's own tables, connected through canonical SQL views ([databases.md](databases.md#connect-any-sql-database-through-views)): scores, NT-proBNP, LVEF, notes; some of the demo's patients by MRN, two of its own by study ID. |
| `demo_muse` | `views` | An ECG archive: older MUSE XML files (2019 to 2022) of eight of the demo's patients, and an ECG metadata database of them (one row per file by MD5), connected through its views as it is ([databases.md](databases.md#a-muse-ecg-metadata-database)), the MUSE patient ID read as the MRN of the authority `DEMO`. The MUSE files carry their median beats with the P, QRS and T onsets; one is a 250 Hz ECG (a MAC5K with 12SL 237), and one gives no sampling rate. Its views include a `has` view, and its file an index per searched measurement ([databases.md, Indexes for searches](databases.md#indexes-for-searches)). |
| `demo_secure` | `dbb` | A transplant clinic's db-builder database, encrypted with SQLCipher; its key, a demo key published with the demo, is `keys/demo_secure.key` in the demo's folder. |

Everyone in the demo is invented: pseudo names (such as Jane Doe), MRNs starting with `SYN`,
patient IDs, dates of birth and every date (in an invented period), and every text. The ECGs are
built from heart-beat templates, and the CT series and two of the chest radiographs from
slices of public CT collections, that a maintainer transformed from real recordings and pictures
(beats re-timed, blended and their leads mixed; slices re-sliced, deformed and re-mapped) and
checked against re-identification with a tool kept outside the repository; only the transformed
pieces ship, in `tools/demo/library` ([tools/demo/README.md](../tools/demo/README.md)), with the
public collections' attributions. Everything else is made from models. `tools/make-demo-data.py`
makes the demo; [tools/README.md](../tools/README.md#the-demos-features) lists which feature each
part of it demonstrates.

**The demo is off by default, and an installation of its own.** Medorama starts with no database
unless the demo was asked for. Use it only where Medorama is tried, demonstrated or taught, never
on a server meant for real patients (see the deployment checklist in [security.md](security.md)).
An installation is either the demo's or a site's, from `medorama init` on, and never both
([design/demo.md](design/demo.md)):

- `medorama init --demo` makes a **demo installation**; `medorama init` a **site's
  installation**. The mode is kept in the control store and never changes.
- A demo database says so: it holds a table `medorama_demo` (`db-builder new --demo`; the
  databases `tools/make-demo-data.py` writes itself get it too). A demo installation refuses a
  database without it, a site's installation a database with it; each refusal says what to do.
- Every command (and `medorama serve`) checks first that the installation holds no database of
  the other kind; `medorama databases list` and `remove` stay open, to put it right.
- A demo installation shows a band on every page, "Demo · every patient here is invented: this
  installation holds no real patient's data", in neutral colours (red is for alarms); the
  browser tab's title begins with "Demo"; its cohort exports are named `demo-cohort-…`.
- `init --demo` is never remembered as the home that commands without `--home` reach: a site's
  installation on the same computer stays the default. Give the demo its own home, workspace and
  port.

**Installations made before 0.3.1** have no mode. At the first command, Medorama decides it by
what the control store connects: demo databases only, the demo's; otherwise a site's. A database
is a demo database by its marker, or, for a demo made before 0.3.1 (without the marker), by being
a file of the demo the installer made (under `…/share/medorama/demo/`). A control store that
connects both is refused until the databases of one kind are removed (`medorama databases remove
NAME`), the demo's named in the message; the demo is then made as its own installation.

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
2. writes the demo into `.venv/share/medorama/demo` (on Windows `.venv\share\medorama\demo`)
   and builds its db-builder databases there, as a site would
   (`tools/make-demo-data.py DIR --db-builder .venv/bin/db-builder`: the steps are listed in the
   demo's `demo-databases.json`; each made with `db-builder new --demo`, marked a demo
   database): `demo.sqlite` (`db-builder add` with `--issuer DEMO
   --mrn-authority DEMO`, then `db-builder recordings` on its folders of recordings),
   `north/north.sqlite` (with `--rules north/rules.toml`, then its ECG folder moved and
   `db-builder rewrite --rules north/moved.toml`), and `secure/secure.sqlite` (`db-builder
   --encrypt`, with `DB_BUILDER_KEY` from `keys/demo_secure.key`); its OMOP and registry databases
   are SQLite files already;
3. names that folder in `.venv/bin/medorama.demo` (on Windows `.venv\Scripts\medorama.demo`), next
   to the programs.

The demo needs WFDB support: its ICU and Holter recordings are WFDB records, so the installers
refuse `--demo` together with `--no-wfdb` ([building.md, Without WFDB](building.md#without-wfdb)).

Then set the demo up as its own installation, with `--demo`, its own home, workspace and port:

```bash
medorama --home ~/medorama-demo init --workspace ~/medorama-demo-workspace --demo
medorama --home ~/medorama-demo serve                   # port 8095
```

A site's installation on the same computer is set up apart, and stays the default home:

```bash
medorama init --workspace ~/medorama-workspace          # a site's installation
medorama serve                                          # port 8090
```

`init --demo` connects every database the demo's `demo-databases.json` lists, as the first
administrator, who is then granted each: their kinds and settings (the OMOP database's MRN
authority `DEMO`, the registry's views) come from that file, and the encrypted database's key
from its key file, so a new demo database needs no change in Medorama. It also adds the demo's
`actions` folder (the example plug-in **Recording summary**, run with the installer's Python) to
`actions_dirs`. A demo folder made before that file existed connects as before: `demo` and
`demo_omop`. It reads every recording of the demo's databases (as `medorama databases index`
does: connecting alone reads none), and `serve` indexes them in the background. It makes four
example cohorts: *ICU stays* (`icu`), *Long QTc* (`demo_muse.QTc > 450 ms`), *Women over 65 with
an ECG* (`female AND age > 65 AND has ecg`) and *Sepsis* (`dx sepsis`). Sign in, open **Patient
Catalog**, and click a patient. Running the installer again without `--demo` removes
`medorama.demo`, and `init --demo` then has nothing to connect.

**What to try.** The patient search and the Patient Journey (Jane Doe, `P900001`, has an ICU
stay, ECGs as WFDB and MUSE XML, echo, cath, CT and a radiograph, and records in three databases);
patients linked across databases (Eva Lindqvist and Richard Roe are in `demo`, `demo_north` and
`demo_omop`); the identity conflict (`medorama --home ~/medorama-demo databases conflicts
demo_north`); an ECG of `demo_muse` compared with the patient's previous one (every patient
there has two ECGs or more; its patients listed by their latest ECG, and a test ECG whose patient
ID is a word left out by its views); the example cohorts, and others
(`demo.creatinine > 1.5 OR has icu`, `female AND age > 70`, `has eeg`); Privacy Mode (names in
notes replaced, dates moved, a patient over 89 shown as `90+`); trends, the Recordings tab, the
viewers, **Actions** (Index and the example plug-in) and **Jobs**, and **History**.

To connect the demo's databases to a demo installation by hand (one made with `init --demo`
before the installer made the demo, or again after removing them), connect them yourself, as
`demo-databases.json` describes them; their folder must be inside `data_roots` when that is set.
A site's installation refuses them:

```bash
D=.venv/share/medorama/demo
M="medorama --home ~/medorama-demo"
$M databases connect demo --kind dbb --url sqlite:$D/demo.sqlite
$M databases connect demo_north --kind dbb --url sqlite:$D/north/north.sqlite
$M databases connect demo_omop --kind omop --url sqlite:$D/omop/omop.sqlite --mrn-authority DEMO
$M databases connect demo_registry --kind views --url sqlite:$D/registry/registry.sqlite --views $D/registry/views.toml --mrn-authority DEMO
$M databases connect demo_muse --kind views --url sqlite:$D/muse/ecg_metadata.sqlite --views $D/muse/views.toml --mrn-authority DEMO
$M databases connect demo_secure --kind dbb --url sqlite:$D/secure/secure.sqlite --password-stdin < $D/keys/demo_secure.key
for d in demo demo_north demo_muse; do $M databases index $d; done   # their recordings now, as init --demo does
```

### Remove the demo

A demo installation stays one: it never becomes a site's installation. To be rid of the demo,
stop its server and delete its home folder and its workspace (`~/medorama-demo` and
`~/medorama-demo-workspace` above); nothing of it is in a site's installation. The demo's files
stay where the installer wrote them, for a new demo installation later. Its databases can also be
removed one by one, as any other database ([Removing a database](#removing-a-database)): on each
database's page, **Remove…**, or

```bash
for d in demo demo_north demo_omop demo_registry demo_muse demo_secure; do medorama --home ~/medorama-demo databases remove $d; done
```

An installation made before 0.3.1 that connects both the demo's databases and real ones is
refused until the databases of one kind are removed: remove the demo's (the command above,
without `--home`, in that installation), then make the demo as its own installation.

`tools/make-demo-data.py` can also write the demo into a folder of your own
(`.venv/bin/python tools/make-demo-data.py ~/medorama-demo --db-builder .venv/bin/db-builder`,
with NumPy and wfdb-x in `.venv`); `--small` writes the first six patients and two databases
only, `--no-library` uses models alone, `--seed N` makes another demo (the same seed makes the
same files, byte for byte). It can also give some of the first patients local examples of real
files, which are kept in `sample_data` and never in git: `--examples DIR` adds ECGs and `--dicom
DIR` adds DICOM studies, de-identified on the way (`tools/make-demo-data.py --help`); such a demo
is never used for published screenshots. Without the Python viewer in `.venv`, add NumPy and
wfdb-x first:

```bash
.venv/bin/pip install numpy "wfdb-x @ https://github.com/alphanumericslab/wfdb-x/archive/4a91916f5282fba44ca3056802fbc7def298af08.tar.gz#subdirectory=python"
```

## Troubleshooting

| Problem | Cause and remedy |
|---|---|
| `Medorama is not set up in …` | The home folder is elsewhere. Run `medorama --home DIR use`, or pass `--home DIR`, or set `MEDORAMA_HOME`. Or set Medorama up: `medorama init --workspace DIR`. |
| `…: workspace_dir is required` | `medorama.toml` has no `workspace_dir` (a settings file from before workspaces). Set Medorama up again with `init --workspace DIR`. |
| `unknown settings: …` | `medorama.toml` has a key that Medorama does not know, often a typing mistake, or a setting that no longer exists (`results_dir`, `formats_dirs`). Correct or remove it ([Settings](#settings)). |
| `…: index_dir is no longer a setting: …; remove the line` | `medorama.toml` was written by an earlier version. Indexes are now kept in each database's workspace folder (`db-NNNN/index.sqlite`). Remove the `index_dir` line; the recordings are indexed again into their databases' stores, and the old index folder can be deleted. |
| `…: no master key` | Neither `$MEDORAMA_MASTER_KEY`, `master_key_command` nor the file of `master_key_file` gives the master key. Restore the key file, or point the setting at it ([Keys](#keys)). |
| `…: its key was wrapped by another master key` | The master key in use is not the one that wrapped the data keys (for example, after a rotation the setting still names the old key, or a backup was restored with the wrong key). Point the setting at the right key. |
| `…: the master key must be 64 hex digits` | The key file, the variable or the command's output is not a master key. |
| `proxy_user_header, proxy_addresses and proxy_logout_url need sign_in = "proxy"` | Proxy settings are present while the sign-in method is `password`. Add `sign_in = "proxy"`, or remove them. |
| `proxy_addresses: "…" is not an IP address` | `proxy_addresses` takes IP addresses, not host names. |
| `the password is too short` | Passwords need at least 10 characters. |
| `Use at least 8 characters.` (a viewer's `--set-password`) | The VitalStream viewers' own passwords need at least 8 characters. |
| `init`: `cannot create the database` | Most often, the home folder is on a network file system. Use `--home` with a folder on a local disk. |
| `cannot listen on 127.0.0.1:8090` | Another program uses the port. Use `--port N`, or set `port` in `medorama.toml`. |
| Connect: `the file is outside the folders data may come from (data_roots)` | Add the database's folder to `data_roots` (an empty list allows any folder), and restart the server. |
| Connect: `that file is in Medorama's workspace` | A connected database may not lie inside the workspace. Keep databases elsewhere. |
| Connect: `only admins connect databases` | Ask an administrator to connect it and grant it to you. |
| A database shows a grey dot (offline) | It did not answer at its last check: a SQLite file that moved, a stopped server, a password that changed. Its page says when and why (for those who manage it); `medorama databases check NAME` checks it again. |
| A database is listed, but its patients are not | You are not granted it. Ask whoever manages it, or, as an administrator, **Grant it to me**. |
| A patient appears twice, once from each database | The two databases name the patient with different (issuer, identifier) pairs: an issuer missing or spelled differently, or different identifiers. Correct the database (or its `--issuer`, `--mrn-authority`), then refresh it ([Identity](#identity)). |
| `N identity conflict(s)` | Some persons' identifiers name two patients. List them with `medorama databases conflicts NAME`, correct the database, then refresh it. |
| A recording is `missing` | The file is not where the database says, or not inside `data_roots`. Check the database's `file` column and `--files-root`. |
| A recording stays grey (not indexed) | Check that the server runs with workers and `auto_index = true`, or run `medorama run index --recording N` and `medorama work`. When indexing failed, the Jobs tab shows the error. |
| **View** is greyed out | Its tooltip says why. Usually no viewer is installed beside Medorama: run `tools/install-medorama.sh --viewer cpp`. |
| "too many failed attempts" | Five wrong passwords lock a user name for one minute. |
| A forgotten administrator password | Run `medorama user set NAME --password` on the server. |
| "your institution's sign-in names you …, and Medorama has no account of that name" | With proxy sign-in, the proxy named a user without an enabled account. Create the account with that exact name, or enable it. |
| Behind the proxy, every request is "sign in required" | The proxy does not set `proxy_user_header`, or it connects from an address that is not in `proxy_addresses`. |
| `the database has schema version N, newer than this program knows` | A newer `medorama` updated the control store. Use that version. |
| `the PostgreSQL database … holds no catalog yet` | `database_url` names an empty database. Set Medorama up there (`init --database-url`), or copy a control store into it (`copy-database`). |
| `PostgreSQL: … TLS …` | The database server offers no TLS, or a certificate that Medorama cannot verify. Install a certificate, or name its authority in `database_ca_file` (or the connected database's `--ca-file`). Use `sslmode=disable` only on a network you trust. |
| `database_url: leave the password out` | The password goes in `database_password_file` or `$MEDORAMA_DATABASE_PASSWORD`. |
| A query fails on PostgreSQL only | Run with `MEDORAMA_DEBUG_SQL=1`. It prints the statement that PostgreSQL refused, never its values. |
| A patient's page says `omop: not available` | That connected database did not answer. Run `medorama databases check omop`. |
| A criterion: `… names N measurements in omop` | OMOP's concept names are long. Write the one you mean, in quotes: `omop."Lactate [Moles/volume] in Blood" > 4`. |
