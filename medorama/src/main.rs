//! `medorama`: Medorama's command-line tool and server (docs/administration.md).
//!
//! Commands act directly on the control store and the workspace, as the operating-system user
//! who can open them (recorded in the audit log as `local:<user>`, with every grant); the
//! server's web page and API act as signed-in users.

use clap::{Args, Parser, Subcommand};
use medorama::actions::Registry;
use medorama::config::{default_home, Config, CONFIG_NAME};
use medorama::db::{audit, Db};
use medorama::jobs::{self, Selection};
use medorama::users::{self, Changes, Role, User};
use medorama::util::{fmt_local, tz};
use medorama::{recordings, server};
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Parser)]
#[command(
    name = "medorama",
    version,
    about = "Medorama: a research browser over connected clinical databases (EHR data, physiological waveforms, imaging and laboratory results), read in place"
)]
struct Cli {
    /// Medorama's home folder (medorama.toml, the control store, the master key); default:
    /// $MEDORAMA_HOME, else the one `init` created last, else ~/.local/share/medorama
    /// (%LOCALAPPDATA%\medorama on Windows)
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once, from the command line
enum Cmd {
    /// Set Medorama up: settings, the control store, the master key and the first admin
    Init(InitArgs),
    /// Run the web interface, the HTTP API and the background workers
    Serve(ServeArgs),
    /// Make the home given by --home the one later commands use without --home
    Use,
    /// Copy the control store (SQLite) into an empty PostgreSQL database, every table and row;
    /// then set database_url in medorama.toml to use it (docs/administration.md, "Settings")
    CopyDatabase {
        /// the PostgreSQL database (postgres://user@host/db?sslmode=require), without its password
        #[arg(long)]
        to: String,
        /// the file holding its password (else $MEDORAMA_DATABASE_PASSWORD)
        #[arg(long)]
        password_file: Option<PathBuf>,
        /// certificate authorities (PEM) for its server's certificate, besides the system's
        #[arg(long)]
        ca_file: Option<PathBuf>,
    },
    /// The connected databases: connect, list, check, refresh, grant, remove
    #[command(subcommand)]
    Databases(DatabasesCmd),
    /// The master key: wrap every data key again with a new one
    #[command(subcommand)]
    Keys(KeysCmd),
    /// List patients (of the connected databases)
    List(ListArgs),
    /// List recordings (of the connected recording databases)
    Recordings {
        /// words in their name, format, status or database
        #[arg(long, default_value = "")]
        q: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Re-check every recording now: missing, changed, its index still there
    Check,
    /// List the installed actions
    Actions,
    /// Add-ons: the adapters that read recording formats Medorama does not read itself
    /// (licensed formats; docs/adapters.md): list them, install or remove one
    #[command(subcommand)]
    Addons(AddonsCmd),
    /// Queue actions on recordings (then `serve` or `work` runs them)
    Run(RunArgs),
    /// Run queued jobs here until none is left
    Work {
        /// jobs at the same time (default: the `workers` setting)
        #[arg(long)]
        workers: Option<usize>,
    },
    /// List jobs (unfinished ones first)
    Jobs {
        #[arg(long)]
        state: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long)]
        json: bool,
    },
    /// Cancel queued or running jobs
    Cancel {
        #[arg(long)]
        job: Option<i64>,
        #[arg(long)]
        run: Option<i64>,
        /// every unfinished job
        #[arg(long)]
        all: bool,
    },
    /// Manage users
    #[command(subcommand)]
    User(UserCmd),
    /// Show the audit log
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// check the seals instead: whether a sealed entry was changed, deleted or added
        #[arg(long)]
        verify: bool,
        /// seal the entries not sealed yet now (the server does it every minute); with --verify,
        /// seal first, then check
        #[arg(long)]
        seal: bool,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once, from the command line
enum DatabasesCmd {
    /// Connect a database: checked, then kept with its own key and workspace folder; its
    /// variables read and its persons linked
    Connect {
        /// its name, as criteria write it (`omop.lactate`): letters, digits, - and _
        name: String,
        /// omop (an OMOP CDM 5.3/5.4), views (any SQL database, through views), dbb (a db-builder database)
        #[arg(long)]
        kind: String,
        /// postgres://user@host/db?sslmode=require, or sqlite:/path/to/file
        #[arg(long)]
        url: String,
        /// the file holding its password (else none, or $MEDORAMA_SOURCE_PASSWORD_<NAME>)
        #[arg(long)]
        password_file: Option<PathBuf>,
        /// read its password from standard input (a SQLite file's: its SQLCipher key, when it is encrypted): kept wrapped by the master key
        #[arg(long, conflicts_with = "password_file")]
        password_stdin: bool,
        /// certificate authorities (PEM) for its server's certificate
        #[arg(long)]
        ca_file: Option<PathBuf>,
        /// a TOML file with the views (persons, visits, entries, recordings, version): kind views
        #[arg(long)]
        views: Option<PathBuf>,
        /// OMOP: the schema of the CDM's tables, and of its vocabulary
        #[arg(long, default_value = "")]
        schema: String,
        #[arg(long, default_value = "")]
        vocabulary_schema: String,
        /// OMOP: what person_source_value holds: mrn (default), patient_id or none
        #[arg(long, default_value = "")]
        identifier: String,
        /// who issued its patient IDs (default: the database itself: they link only its own persons)
        #[arg(long, default_value = "")]
        issuer: String,
        /// the authority of its MRNs, when a row names none
        #[arg(long, default_value = "")]
        mrn_authority: String,
        /// the time zone of its times written without an offset (default: the `timezone` setting)
        #[arg(long, default_value = "")]
        timezone: String,
        /// the folder relative recording files are read from (default: a SQLite file's folder)
        #[arg(long, default_value = "")]
        files_root: String,
        #[arg(long, default_value_t = 0)]
        timeout_s: u64,
        #[arg(long, default_value_t = 0)]
        max_rows: u64,
    },
    /// List the connected databases and their status
    List {
        #[arg(long)]
        json: bool,
    },
    /// Check a database now (or every one): online or offline, and read again when its data changed
    Check {
        name: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Read what Medorama keeps of a database again: its catalog (variables' names and units),
    /// its persons and their encounters
    Refresh { name: String },
    /// Count a database's variables: how many entries and persons hold each (one pass over its
    /// entries; counts below 11 are shown as "< 11")
    Count { name: String },
    /// Read every recording a database lists, check its files and index the new or changed ones
    /// (connecting reads none: a patient's recordings are read when they are opened)
    Index { name: String },
    /// Remove a database and every trace of it: its key destroyed, its workspace folder, cohort
    /// rows and links removed (the database itself is not touched)
    Remove {
        name: String,
        /// do it without asking
        #[arg(long)]
        yes: bool,
    },
    /// What a database holds: its variables
    Variables {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// A database's identity conflicts: persons whose identifiers name two patients (by key)
    Conflicts { name: String },
    /// Who may use a database
    Grants { name: String },
    /// Grant a user a permission on a database: search, view, phi or manage
    Grant { name: String, user: String, permission: String },
    /// Revoke a permission (and the stronger ones)
    Revoke { name: String, user: String, permission: String },
}

#[derive(Subcommand)]
enum KeysCmd {
    /// Wrap every data key with a new master key; the data are not touched.  The new key: a file
    /// of 64 hex digits; then point master_key_file (or the key manager) at it
    Rotate {
        #[arg(long)]
        new_key_file: PathBuf,
    },
    /// Make a new master key file (64 random hex digits, readable by this account only)
    New { file: PathBuf },
}

#[derive(Subcommand)]
enum AddonsCmd {
    /// List the adapters installed in the adapter folder (the `adapters_dir` setting)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Install an adapter: copy its program (named <name>-adapter) into the adapter folder,
    /// after checking that it describes itself and speaks this protocol; an adapter of the
    /// same name is replaced
    Install { program: PathBuf },
    /// Remove an installed adapter, by its name (as `list` shows it)
    Remove { name: String },
}

#[derive(Args)]
struct InitArgs {
    /// the workspace: where Medorama keeps what it generates for each database, encrypted
    /// (required; never a folder of recordings or of a connected database)
    #[arg(long)]
    workspace: PathBuf,
    /// the first admin's user name
    #[arg(long, default_value = "admin")]
    admin: String,
    #[arg(long, default_value = "America/New_York")]
    timezone: String,
    #[arg(long, default_value_t = medorama::config::DEFAULT_PORT)]
    port: u16,
    /// read the admin's password from standard input (for scripts)
    #[arg(long)]
    password_stdin: bool,
    /// a PostgreSQL database instead of SQLite (postgres://user@host/db?sslmode=require), empty
    #[arg(long)]
    database_url: Option<String>,
    /// the file holding that database's password
    #[arg(long, requires = "database_url")]
    database_password_file: Option<PathBuf>,
    /// connect the demo's databases the installer made (tools/install-medorama.sh --demo), and
    /// add its example action plug-in
    #[arg(long)]
    demo: bool,
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// do not run jobs (another process runs `work`)
    #[arg(long)]
    no_workers: bool,
    /// run db-builder's web interface, building databases in this folder (overrides the
    /// setting db_builder_root)
    #[arg(long, value_name = "DIR")]
    db_builder_root: Option<PathBuf>,
}

#[derive(Args)]
struct ListArgs {
    /// search words (pseudonym, sex, has:…, database:…; with the PHI permission: name, MRN, date of birth)
    #[arg(long, default_value = "")]
    q: String,
    #[arg(long, default_value_t = 100)]
    limit: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct RunArgs {
    /// action names (e.g. index)
    #[arg(required = true)]
    actions: Vec<String>,
    /// every recording
    #[arg(long)]
    all: bool,
    /// recordings (their numbers, as `medorama recordings` shows them)
    #[arg(long = "recording")]
    recordings: Vec<i64>,
    /// the recordings of these patients (their pseudonyms)
    #[arg(long = "patient")]
    patients: Vec<String>,
    /// the recordings of a cohort's members (its number, as the Cohorts tab shows it)
    #[arg(long)]
    cohort: Option<i64>,
    /// run again where already done
    #[arg(long)]
    force: bool,
    /// parameter, as ACTION.KEY=VALUE (VALUE is JSON, or text)
    #[arg(long = "param")]
    params: Vec<String>,
    #[arg(long, default_value_t = 0)]
    priority: i64,
}

#[derive(Subcommand)]
enum UserCmd {
    /// Add a user
    Add {
        username: String,
        /// viewer, analyst or admin
        #[arg(long)]
        role: String,
        /// may see and search identifiers
        #[arg(long)]
        phi: bool,
        #[arg(long, default_value = "")]
        display_name: String,
        #[arg(long)]
        password_stdin: bool,
        /// no password: the account signs in through the institution's sign-in (sign_in = "proxy",
        /// docs/sign-in.md) and gets a random password nobody knows
        #[arg(long, conflicts_with = "password_stdin")]
        no_password: bool,
    },
    /// List users
    List,
    /// Change a user
    Set {
        username: String,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        phi: Option<bool>,
        #[arg(long)]
        disabled: Option<bool>,
        #[arg(long)]
        display_name: Option<String>,
        /// set a new password (asked for)
        #[arg(long)]
        password: bool,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Delete a user
    Remove { username: String },
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("medorama: {msg}");
    ExitCode::FAILURE
}

fn read_password(stdin: bool, prompt: &str) -> anyhow::Result<String> {
    if stdin {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let pw = line.trim_end_matches(['\r', '\n']).to_string();
        users::check_new_password(&pw)?;
        return Ok(pw);
    }
    for _ in 0..3 {
        let pw = rpassword::prompt_password(prompt)?;
        if let Err(e) = users::check_new_password(&pw) {
            eprintln!("{e}. Try again.");
            continue;
        }
        if rpassword::prompt_password("Repeat it: ")? != pw {
            eprintln!("The two entries differ. Try again.");
            continue;
        }
        return Ok(pw);
    }
    anyhow::bail!("no password set")
}

fn private_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The license terms the installers ask the user to accept (tools/license-notice.txt), shown again
/// by `init`, where Medorama is set up for others, and to whoever built it without an installer.
const LICENSE_NOTICE: &str = include_str!("../../tools/license-notice.txt");

fn init(home: &Path, a: InitArgs) -> anyhow::Result<()> {
    if home.join(CONFIG_NAME).exists() {
        anyhow::bail!("{} exists already; Medorama is set up there", home.join(CONFIG_NAME).display());
    }
    eprintln!("{LICENSE_NOTICE}\nSetting Medorama up and using it means accepting these terms.\n");
    let ws = PathBuf::from(recording_engine::paths::absolute(&a.workspace.to_string_lossy()));
    let mut cfg = Config::new(home, &ws);
    cfg.timezone = a.timezone;
    cfg.port = a.port;
    if let Some(u) = &a.database_url {
        cfg.database_url = u.trim().to_string();
        cfg.database_password_file =
            a.database_password_file.map(|f| PathBuf::from(recording_engine::paths::absolute(&f.to_string_lossy())));
    }
    cfg.check().map_err(anyhow::Error::msg)?;
    let pw = read_password(a.password_stdin, &format!("Password for the admin {}: ", a.admin))?;
    private_dir(home)?;
    std::fs::create_dir_all(home.join("actions"))?;
    private_dir(&cfg.workspace_dir)?;
    // the database first, the settings file last: a failed init leaves nothing half made
    let pg = !cfg.database_url.is_empty();
    if !pg && cfg.database.exists() {
        anyhow::bail!(
            "{} exists already (from an earlier setup): move it away, or choose another --home",
            cfg.database.display()
        );
    }
    let key_file = medorama::workspace::make_master_file(&cfg)?;
    medorama::workspace::load(&cfg)?;
    let made = (|| -> anyhow::Result<String> {
        let db = Db::open(&cfg, true)?;
        let c = db.get()?;
        let n: i64 = c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if n > 0 {
            anyhow::bail!("this database holds a control store already (it has users): choose an empty database");
        }
        users::add(&c, &a.admin, "", Role::Admin, true, &pw)?;
        audit(&c, Some(&User::local()), "init", &cfg.workspace_dir.to_string_lossy(), "");
        Ok(db.describe())
    })();
    let shown = match made {
        Ok(d) => d,
        Err(e) if pg => {
            anyhow::bail!("cannot create the control store in {}: {e}", medorama::db::shown_url(&cfg.database_url))
        }
        Err(e) => {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", cfg.database.display()));
            }
            anyhow::bail!(
                "cannot create the database {}: {e}\n  If this folder is on a network file system (NFS, SMB), SQLite's locking may not work there:\n  choose a --home on a local disk.",
                cfg.database.display()
            );
        }
    };
    if a.demo {
        if let Some(dir) = demo_folder() {
            for p in demo_actions(&dir) {
                if !cfg.actions_dirs.contains(&p) {
                    cfg.actions_dirs.push(p);
                }
            }
        }
    }
    std::fs::write(home.join(CONFIG_NAME), cfg.to_toml())?;
    println!("Medorama set up in {}", home.display());
    println!("  settings:      {}", home.join(CONFIG_NAME).display());
    println!("  control store: {shown} (no patient data)");
    println!("  workspace:     {} (encrypted per database, its indexes too)", cfg.workspace_dir.display());
    if let Some(f) = key_file {
        println!("  master key:    {} (keep it, and keep it apart from the workspace's backups)", f.display());
    }
    println!("  admin:         {} (with PHI permission)", a.admin);
    match medorama::config::remember_home(home) {
        Ok(p) => println!("Later commands use this home without --home (remembered in {}).", p.display()),
        Err(e) => println!("Could not remember this home ({e}): pass --home {} to later commands.", home.display()),
    }
    if a.demo {
        match demo_folder() {
            Some(dir) => {
                let db = Db::open(&cfg, false)?;
                let c = db.get()?;
                let admin = users::get_by_name(&c, &a.admin)?.ok_or_else(|| anyhow::anyhow!("no user {}", a.admin))?;
                for d in demo_databases(&dir, &cfg.timezone) {
                    if !d.file.is_file() {
                        continue;
                    }
                    let key = match &d.key_file {
                        Some(f) => match std::fs::read_to_string(f) {
                            Ok(k) => k.trim().to_string(),
                            Err(e) => {
                                println!(
                                    "The demo database {} was not connected (its key {}: {e})",
                                    d.name,
                                    f.display()
                                );
                                continue;
                            }
                        },
                        None => String::new(),
                    };
                    let n = medorama::sources::NewSource {
                        name: d.name.clone(),
                        kind: d.kind.clone(),
                        url: format!("sqlite:{}", d.file.display()),
                        settings: d.settings,
                        ..Default::default()
                    };
                    match medorama::databases::connect(&cfg, &c, &admin, &n, &key) {
                        Ok(id) => {
                            // the demo shows every recording indexed: its few are read now, as
                            // `medorama databases index` reads a database's (connecting reads none)
                            let r = medorama::databases::index_recordings(&cfg, &c, &admin, id);
                            let n = r.as_ref().map(|r| r.recordings).unwrap_or(0);
                            println!(
                                "Connected the demo database {} (invented patients){}",
                                d.name,
                                if n > 0 { format!("; {n} recording(s) to index") } else { String::new() }
                            );
                        }
                        Err(e) => println!("The demo database {} was not connected ({e:#})", d.name),
                    }
                }
            }
            None => println!("No demo: make it with tools/install-medorama.sh --demo"),
        }
    }
    println!("Next: medorama databases connect NAME --kind KIND --url URL, then medorama serve");
    Ok(())
}

/// The demo the installer made (tools/install-medorama.sh --demo): the folder named in
/// `medorama.demo` beside this program, which holds its db-builder database (`demo.sqlite`).
fn demo_folder() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let named = std::fs::read_to_string(exe.parent()?.join("medorama.demo")).ok()?;
    let dir = PathBuf::from(named.trim());
    dir.join("demo.sqlite").is_file().then_some(dir)
}

/// One database of the demo, as `init --demo` connects it.
#[derive(Debug)]
struct DemoDatabase {
    name: String,
    kind: String,
    file: PathBuf,
    settings: medorama::sources::Settings,
    key_file: Option<PathBuf>,
}

/// The demo's databases: those its `demo-databases.json` lists (tools/make-demo-data.py
/// writes it: each database's name, kind, file and settings, and the file of its key), so that
/// a new demo database needs no change here; without that file (a demo made before it), its
/// db-builder database and its OMOP one.  A database's time zone, when its settings give none,
/// is `timezone`.
fn demo_databases(dir: &Path, timezone: &str) -> Vec<DemoDatabase> {
    let mut out = Vec::new();
    let listed = std::fs::read_to_string(dir.join("demo-databases.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    match listed {
        Some(m) => {
            for d in m["databases"].as_array().into_iter().flatten() {
                let (Some(name), Some(kind), Some(file)) = (d["name"].as_str(), d["kind"].as_str(), d["file"].as_str())
                else {
                    continue;
                };
                let mut settings: medorama::sources::Settings =
                    serde_json::from_value(d["settings"].clone()).unwrap_or_default();
                if settings.timezone.is_empty() && kind != "dbb" {
                    settings.timezone = timezone.to_string();
                }
                out.push(DemoDatabase {
                    name: name.into(),
                    kind: kind.into(),
                    file: dir.join(file),
                    settings,
                    key_file: d["key_file"].as_str().map(|k| dir.join(k)),
                });
            }
        }
        None => {
            // its hospital (made by db-builder), and another hospital's OMOP database that holds
            // two of its patients, by their MRNs
            out.push(DemoDatabase {
                name: "demo".into(),
                kind: "dbb".into(),
                file: dir.join("demo.sqlite"),
                settings: Default::default(),
                key_file: None,
            });
            out.push(DemoDatabase {
                name: "demo_omop".into(),
                kind: "omop".into(),
                file: dir.join("omop").join("omop.sqlite"),
                settings: medorama::sources::Settings {
                    mrn_authority: "DEMO".into(),
                    timezone: timezone.into(),
                    ..Default::default()
                },
                key_file: None,
            });
        }
    }
    out
}

/// The folders of action plug-ins the demo brings (`actions_dirs` of its
/// `demo-databases.json`: the example plug-in), to add to Medorama's.
fn demo_actions(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(dir.join("demo-databases.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|m| m["actions_dirs"].as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(|s| dir.join(s)))
        .filter(|p| p.is_dir())
        .collect()
}

fn open(home: &Path) -> anyhow::Result<(Config, Db)> {
    let cfg = Config::load(home).map_err(anyhow::Error::msg)?;
    medorama::workspace::load(&cfg)?;
    let db = Db::open(&cfg, false)?;
    // removals a crash interrupted: finished first
    {
        let c = db.get()?;
        if medorama::databases::resume_removals(&cfg, &c)? > 0 {
            recordings::sweep_index(&cfg, &c)?;
        }
    }
    Ok((cfg, db))
}

fn t(x: Option<f64>, zone: chrono_tz::Tz) -> String {
    x.map(|x| {
        let s = fmt_local(x, zone);
        s.get(..16).unwrap_or(&s).to_string()
    })
    .unwrap_or_else(|| "-".into())
}

fn print_json(v: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn databases_cmd(cfg: &Config, db: &Db, me: &User, cmd: DatabasesCmd) -> anyhow::Result<()> {
    use medorama::sources;
    let c = db.get()?;
    let named = |name: &str| -> anyhow::Result<sources::Source> {
        sources::by_name(&c, name)?.ok_or_else(|| anyhow::anyhow!("no database {name} (medorama databases list)"))
    };
    let user = |name: &str| -> anyhow::Result<User> {
        users::get_by_name(&c, name)?.ok_or_else(|| anyhow::anyhow!("no user {name}"))
    };
    match cmd {
        DatabasesCmd::Connect {
            name,
            kind,
            url,
            password_file,
            password_stdin,
            ca_file,
            views,
            schema,
            vocabulary_schema,
            identifier,
            issuer,
            mrn_authority,
            timezone,
            files_root,
            timeout_s,
            max_rows,
        } => {
            let abs = |p: Option<PathBuf>| {
                p.map(|p| recording_engine::paths::absolute(&p.to_string_lossy())).unwrap_or_default()
            };
            let mut settings = sources::Settings {
                schema,
                vocabulary_schema,
                timezone: if timezone.is_empty() { cfg.timezone.clone() } else { timezone },
                identifier,
                issuer,
                mrn_authority,
                files_root,
                timeout_s,
                max_rows,
                ..Default::default()
            };
            if let Some(f) = views {
                let t: toml::Table = std::fs::read_to_string(&f)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?
                    .parse()
                    .map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
                let g = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                (settings.persons, settings.visits, settings.entries, settings.recordings, settings.version) =
                    (g("persons"), g("visits"), g("entries"), g("recordings"), g("version"));
                // optional: the stays (ICU and bed stays) within visits; the catalog of variables
                (settings.stays, settings.variables) = (g("stays"), g("variables"));
            }
            let password = if password_stdin {
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                line.trim_end_matches(['\r', '\n']).to_string()
            } else {
                String::new()
            };
            let n = sources::NewSource {
                name,
                kind,
                url,
                password_file: abs(password_file),
                ca_file: abs(ca_file),
                settings,
            };
            let id = medorama::databases::connect(cfg, &c, me, &n, &password)?;
            let d = medorama::databases::get(&c, id)?.ok_or_else(|| anyhow::anyhow!("not connected"))?;
            let (persons, patients, conflicts) = medorama::persons::counts(cfg, &c, id)?;
            println!(
                "Connected {} ({}): version {}; {persons} person(s) linked to {patients} patient(s); {} variable(s){}{}",
                d.source.name,
                d.source.kind,
                if d.data_version.is_empty() { "-" } else { &d.data_version },
                medorama::variables::of(cfg, &c, id)?.len(),
                if d.source.has_recordings() { "; recordings are read when a patient is opened" } else { "" },
                if conflicts > 0 { format!("; {conflicts} identity conflict(s): medorama databases conflicts {}", d.source.name) } else { String::new() }
            );
        }
        DatabasesCmd::List { json } => {
            let v = medorama::databases::shown(cfg, &c, me)?;
            if json {
                print_json(&v);
            } else if v.is_empty() {
                println!("no connected databases (medorama databases connect)");
            } else {
                for d in v {
                    println!(
                        "{:<16} {:<6} {:<8} {:<10} {} person(s), {} recording(s) opened{}",
                        d.name,
                        d.kind,
                        d.engine,
                        d.status,
                        d.persons.unwrap_or(0),
                        d.recordings,
                        if d.status_error.is_empty() { String::new() } else { format!("  ({})", d.status_error) }
                    );
                }
            }
        }
        DatabasesCmd::Check { name, all } => {
            if all || name.is_none() {
                let (on, off) = medorama::databases::check_all(cfg, &c)?;
                println!("{on} online, {off} offline");
            } else {
                let s = named(name.as_deref().unwrap_or_default())?;
                let online = medorama::databases::check(cfg, &c, s.id)?;
                let d = medorama::databases::get(&c, s.id)?.ok_or_else(|| anyhow::anyhow!("removed"))?;
                println!(
                    "{}: {}{}",
                    s.name,
                    if online { "online" } else { "offline" },
                    if d.status_error.is_empty() { String::new() } else { format!(" ({})", d.status_error) }
                );
            }
        }
        DatabasesCmd::Refresh { name } => {
            let s = named(&name)?;
            medorama::databases::refresh(cfg, &c, me, s.id)?;
            let (persons, patients, conflicts) = medorama::persons::counts(cfg, &c, s.id)?;
            println!("{}: {persons} person(s), {patients} patient(s), {conflicts} conflict(s)", s.name);
        }
        DatabasesCmd::Count { name } => {
            let s = named(&name)?;
            let n = medorama::databases::count_variables(cfg, &c, me, s.id)?;
            println!("{}: {n} variable(s) counted (medorama databases variables {})", s.name, s.name);
        }
        DatabasesCmd::Index { name } => {
            let s = named(&name)?;
            let r = medorama::databases::index_recordings(cfg, &c, me, s.id)?;
            let queued = medorama::jobs::queue_auto_index(&c, cfg)?;
            println!(
                "{}: {} recording(s) read ({} new, {} changed, {} missing, {} removed); {queued} queued for indexing (medorama serve indexes them)",
                s.name, r.recordings, r.added, r.changed, r.missing, r.removed
            );
        }
        DatabasesCmd::Remove { name, yes } => {
            let s = named(&name)?;
            if !yes {
                use std::io::IsTerminal;
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("removing {name} cannot be undone: give --yes");
                }
                eprint!("Remove {name} and every trace of it from Medorama (its key, workspace folder, cohort rows)? Type its name: ");
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                if line.trim() != name {
                    anyhow::bail!("not removed");
                }
            }
            medorama::databases::remove(cfg, &c, me, s.id)?;
            recordings::sweep_index(cfg, &c)?;
            println!("{name} removed: its key is destroyed, its workspace folder and cohort rows removed; the database itself is untouched");
        }
        DatabasesCmd::Variables { name, json } => {
            let s = named(&name)?;
            let v: Vec<medorama::variables::ShownVariable> =
                medorama::variables::of(cfg, &c, s.id)?.into_iter().map(Into::into).collect();
            if json {
                print_json(&v);
            } else {
                let counted = v.iter().any(|x| x.entries.is_some());
                for x in &v {
                    let n = |c: &Option<String>| c.clone().unwrap_or_else(|| "-".into());
                    println!(
                        "{:<6} {:<40} {:<10} {:>8} entries, {:>6} persons{}",
                        x.variable.category,
                        x.variable.name,
                        x.variable.unit,
                        n(&x.entries),
                        n(&x.persons),
                        if x.variable.code.is_empty() {
                            String::new()
                        } else {
                            format!("  {} {}", x.variable.vocabulary, x.variable.code)
                        }
                    );
                }
                if !counted && !v.is_empty() {
                    println!("(not counted: medorama databases count {})", s.name);
                }
            }
        }
        DatabasesCmd::Conflicts { name } => {
            let s = named(&name)?;
            let v = medorama::persons::conflicts(cfg, &c, s.id)?;
            if v.is_empty() {
                println!("{}: no conflicts", s.name);
            }
            for (k, d) in v {
                println!("{k}\t{d}");
            }
        }
        DatabasesCmd::Grants { name } => {
            let s = named(&name)?;
            for (_, u, p) in medorama::grants::of_object(&c, &format!("db:{}", s.id))? {
                println!("{u}\t{p}");
            }
        }
        DatabasesCmd::Grant { name, user: who, permission } => {
            let s = named(&name)?;
            let u = user(&who)?;
            medorama::grants::grant(&c, me, u.id, &format!("db:{}", s.id), &permission)?;
            println!("{who} may {permission} {}", s.name);
        }
        DatabasesCmd::Revoke { name, user: who, permission } => {
            let s = named(&name)?;
            let u = user(&who)?;
            medorama::grants::revoke(&c, me, u.id, &format!("db:{}", s.id), &permission)?;
            println!("{who} no longer may {permission} {}", s.name);
        }
    }
    Ok(())
}

fn keys_cmd(cfg: &Config, db: &Db, me: &User, cmd: KeysCmd) -> anyhow::Result<()> {
    match cmd {
        KeysCmd::New { file } => {
            let mut c = cfg.clone();
            c.master_key_file = Some(file.clone());
            c.master_key_command.clear();
            match medorama::workspace::make_master_file(&c)? {
                Some(f) => println!("a new master key: {}", f.display()),
                None => anyhow::bail!("{} exists already", file.display()),
            }
        }
        KeysCmd::Rotate { new_key_file } => {
            let old = medorama::workspace::loaded()?;
            let mut c2 = cfg.clone();
            c2.master_key_file = Some(new_key_file.clone());
            c2.master_key_command.clear();
            // the new key from its file alone (not the environment the old one may come from)
            let text = std::fs::read_to_string(&new_key_file)
                .map_err(|e| anyhow::anyhow!("{}: {e}", new_key_file.display()))?;
            let new = medorama::workspace::master_from_hex(&text)
                .ok_or_else(|| anyhow::anyhow!("{}: not 64 hex digits", new_key_file.display()))?;
            let c = db.get()?;
            let n = medorama::workspace::rotate(&c, old, &new)?;
            audit(&c, Some(me), "keys_rotate", &new.id, &format!("{n} keys and passwords wrapped again"));
            println!("{n} data key(s) and password(s) wrapped again with the master key {}", new.id);
            println!("Now point master_key_file (or MEDORAMA_MASTER_KEY, master_key_command) at the new key, and keep the old one until no backup needs it.");
        }
    }
    Ok(())
}

fn serve(cfg: Config, db: Db, a: ServeArgs) -> anyhow::Result<()> {
    let mut cfg = cfg;
    if let Some(h) = a.host {
        cfg.host = h;
    }
    if let Some(p) = a.port {
        cfg.port = p;
    }
    if a.no_workers {
        cfg.workers = 0;
    }
    if let Some(r) = a.db_builder_root {
        cfg.db_builder_root = Some(PathBuf::from(recording_engine::paths::absolute(&r.to_string_lossy())));
    }
    let cfg = Arc::new(cfg);
    let database = db.describe();
    let app = Arc::new(server::App::new(cfg.clone(), Arc::new(db)));
    for p in &app.reg.read().unwrap().problems {
        eprintln!("medorama: action not loaded: {p}");
    }
    let stop = Arc::new(AtomicBool::new(false));
    let threads = server::start_background(&app, stop.clone());
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind((cfg.host.as_str(), cfg.port))
            .await
            .map_err(|e| anyhow::anyhow!("cannot listen on {}:{}: {e}", cfg.host, cfg.port))?;
        println!("Medorama: open http://{}:{}/", cfg.host, cfg.port);
        println!("  home: {}", cfg.home.display());
        println!("  control store: {database}");
        if !recording_engine::sources::wfdb_folder::SUPPORTED {
            println!("  built without WFDB support: WFDB recordings are refused (not added, not opened)");
        }
        let v = app.viewer.status();
        match v.mode.as_str() {
            "embedded" => {
                println!("  viewer: {} (started on first use, in the VitalStream tab)", v.flavor.unwrap_or_default())
            }
            "link" => println!("  viewer: {} (viewer_url)", v.url),
            _ => println!("  viewer: none ({})", v.reason),
        }
        println!("  db-builder: {}", app.dbbuilder.describe());
        app.dbbuilder.start();
        println!("  workspace: {} (indexes in each database's folder)", cfg.workspace_dir.display());
        println!(
            "  {} worker(s); automatic indexing {}",
            cfg.workers,
            if cfg.auto_index && cfg.workers > 0 { "on" } else { "off" }
        );
        let deid = app.db.get().and_then(|c| medorama::deid::load(&c)).map(|s| medorama::deid::start_warning(&s));
        for w in cfg.start_warnings().into_iter().chain(deid.ok().flatten()) {
            println!("  WARNING: {w}");
        }
        axum::serve(
            listener,
            server::router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        // Ctrl-C, or the SIGTERM a service manager stops it with: either way the viewer it
        // started is stopped below, so the next start runs a new one (new add-ons, settings)
        .with_graceful_shutdown(async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                match signal(SignalKind::terminate()) {
                    Ok(mut term) => {
                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {}
                            _ = term.recv() => {}
                        }
                    }
                    Err(_) => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        })
        .await?;
        anyhow::Ok(())
    });
    stop.store(true, Ordering::Relaxed);
    stop_workers(threads);
    // jobs this process was running go back to the queue; audit entries that waited, written
    if let Ok(c) = app.db.get() {
        let _ = jobs::requeue_own(&c);
        if medorama::db::flush_audit(&c) > 0 {
            eprintln!("medorama: audit entries could not be written to the database (see audit_export)");
        }
    }
    app.viewer.stop();
    app.dbbuilder.stop();
    result
}

/// Workers asked to stop (their `stop` set): the plug-ins they run stopped (TERM, then KILL),
/// and the threads waited for a while (each puts its job back in the queue).
fn stop_workers(threads: Vec<std::thread::JoinHandle<()>>) {
    medorama::actions::stop_all();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for t in threads {
            let _ = t.join();
        }
        let _ = tx.send(());
    });
    if rx.recv_timeout(std::time::Duration::from_secs(20)).is_err() {
        eprintln!("medorama: a worker is still busy (indexing): its job goes back to the queue");
    }
}

/// Ctrl-C, or the SIGTERM a service manager stops a process with, sets `stop`.
fn on_stop_signal(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                match signal(SignalKind::terminate()) {
                    Ok(mut term) => {
                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {}
                            _ = term.recv() => {}
                        }
                    }
                    Err(_) => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        });
        eprintln!("medorama: stopping: the jobs running go back to the queue");
        stop.store(true, Ordering::Relaxed);
        medorama::actions::stop_all();
    });
}

fn run(home: &Path, cmd: Cmd) -> anyhow::Result<()> {
    if let Cmd::Init(a) = cmd {
        return init(home, a);
    }
    let (cfg, db) = open(home)?;
    // the recordings' readers open files inside data_roots only (none set: anywhere), symbolic
    // links resolved, whatever command reads or indexes them
    recording_engine::paths::set_allowed_roots((!cfg.data_roots.is_empty()).then(|| cfg.data_roots.clone()));
    if let Cmd::Use = cmd {
        let p = medorama::config::remember_home(&cfg.home)?;
        println!("Later commands use the home {} (remembered in {}).", cfg.home.display(), p.display());
        return Ok(());
    }
    let me = User::local();
    let zone = tz(&cfg.timezone);
    match cmd {
        Cmd::Init(_) | Cmd::Use => unreachable!(),
        Cmd::Databases(cmd) => databases_cmd(&cfg, &db, &me, cmd)?,
        Cmd::Keys(cmd) => keys_cmd(&cfg, &db, &me, cmd)?,
        Cmd::CopyDatabase { to, password_file, ca_file } => {
            if !cfg.database_url.is_empty() {
                anyhow::bail!("the control store is in PostgreSQL already ({})", db.describe());
            }
            let mut target = cfg.clone();
            target.database_url = to.trim().to_string();
            target.database_password_file = password_file;
            target.database_ca_file = ca_file;
            target.check().map_err(anyhow::Error::msg)?;
            let pg = Db::open(&target, true)?;
            println!("Copying {} into {} …", db.describe(), pg.describe());
            let copied = medorama::db::copy_to_postgres(&db, &pg)?;
            for (t, n) in &copied {
                println!("  {t}: {n}");
            }
            audit(&*pg.get()?, Some(&me), "copy_database", &db.describe(), "");
            println!("Done.  To use it, add to {}:", cfg.home.join(CONFIG_NAME).display());
            println!("  database_url = {}", toml::Value::String(target.database_url.clone()));
            if let Some(f) = &target.database_password_file {
                println!("  database_password_file = {}", toml::Value::String(f.to_string_lossy().to_string()));
            }
            println!("The SQLite file is unchanged: keep it as a backup, then delete it securely.");
        }
        Cmd::Serve(a) => serve(cfg, db, a)?,
        Cmd::Check => {
            let r = recordings::check(&cfg, &*db.get()?)?;
            println!(
                "{} checked: {} missing, {} back, {} changed, {} indexes lost",
                r.checked, r.missing, r.returned, r.changed, r.index_lost
            );
        }
        Cmd::List(a) => {
            let c = db.get()?;
            let (total, rows) = medorama::patient::list(&c, &cfg, &me, &a.q, &[], a.limit, 0)?;
            if a.json {
                print_json(&rows);
                return Ok(());
            }
            for p in &rows {
                println!(
                    "{:<14} {:<2} {:>4} visit(s)  latest {}  {}  [{}]",
                    p.pseudonym,
                    p.sex,
                    p.encounters,
                    t(p.latest, zone),
                    p.kinds.join(" "),
                    p.sources.join(", ")
                );
            }
            println!("{} of {total} patient(s)", rows.len());
        }
        Cmd::Recordings { q, limit, json } => {
            let c = db.get()?;
            let (total, rows) = recordings::list(&cfg, &c, &me, &q, limit, 0)?;
            if json {
                print_json(&rows);
                return Ok(());
            }
            for r in &rows {
                println!(
                    "{:>14}  {:<9} {:<10} {}  {}  {}",
                    r.id,
                    r.status.as_str(),
                    r.source,
                    t(r.t_start, zone),
                    r.name,
                    r.format
                );
            }
            println!("{} of {total} recording(s)", rows.len());
        }
        Cmd::Addons(AddonsCmd::List { json }) => {
            let v = medorama::addons::list(&cfg.adapters_dir);
            if json {
                print_json(&serde_json::Value::Array(v));
                return Ok(());
            }
            if v.is_empty() {
                println!("no adapters in {}", cfg.adapters_dir.display());
            }
            for a in v {
                let formats: Vec<String> = a["formats"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        format!(
                            "{} (.{})",
                            f["label"].as_str().unwrap_or(""),
                            f["extensions"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|e| e.as_str())
                                .collect::<Vec<_>>()
                                .join(", .")
                        )
                    })
                    .collect();
                println!(
                    "{:<12} {:<8} {:<4} {}  [{}]",
                    a["name"].as_str().unwrap_or(""),
                    a["version"].as_str().unwrap_or(""),
                    a["implementation"].as_str().unwrap_or(""),
                    formats.join("; "),
                    a["program"].as_str().unwrap_or("")
                );
            }
        }
        Cmd::Addons(AddonsCmd::Install { program }) => {
            let a = medorama::addons::install(&cfg.adapters_dir, &program)?;
            medorama::db::audit(
                &*db.get()?,
                Some(&me),
                "addon_install",
                a["name"].as_str().unwrap_or(""),
                &a.to_string(),
            );
            println!(
                "installed {} {} into {}",
                a["name"].as_str().unwrap_or(""),
                a["version"].as_str().unwrap_or(""),
                cfg.adapters_dir.display()
            );
        }
        Cmd::Addons(AddonsCmd::Remove { name }) => {
            let a = medorama::addons::remove(&cfg.adapters_dir, &name)?;
            medorama::db::audit(&*db.get()?, Some(&me), "addon_remove", &name, &a.to_string());
            println!("removed {name} ({})", a["program"].as_str().unwrap_or(""));
        }
        Cmd::Actions => {
            let reg = Registry::load(&cfg);
            for a in &reg.actions {
                println!(
                    "{:<20} v{:<6} {}{}{}",
                    a.name,
                    a.version,
                    a.title,
                    if a.requires.is_empty() {
                        String::new()
                    } else {
                        format!(" (requires {})", a.requires.join(", "))
                    },
                    if a.problem.is_empty() { String::new() } else { format!("  [cannot run: {}]", a.problem) }
                );
            }
            for p in &reg.problems {
                println!("not loaded: {p}");
            }
        }
        Cmd::Run(a) => {
            let reg = Registry::load(&cfg);
            let mut c = db.get()?;
            let mut params: HashMap<String, serde_json::Value> = HashMap::new();
            for p in &a.params {
                let (k, v) = p.split_once('=').ok_or_else(|| anyhow::anyhow!("--param {p}: use ACTION.KEY=VALUE"))?;
                let (action, key) =
                    k.split_once('.').ok_or_else(|| anyhow::anyhow!("--param {p}: use ACTION.KEY=VALUE"))?;
                let value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
                params.entry(action.to_string()).or_insert_with(|| serde_json::json!({}))[key] = value;
            }
            let patients: Vec<i64> = a
                .patients
                .iter()
                .map(|p| medorama::pseudonyms::patient_of(&c, p)?.ok_or_else(|| anyhow::anyhow!("no patient {p}")))
                .collect::<anyhow::Result<_>>()?;
            let sel = Selection {
                cohort: a.cohort,
                patients: (!patients.is_empty()).then_some(patients),
                recordings: (!a.recordings.is_empty()).then_some(a.recordings),
                all: a.all,
            };
            if sel.patients.is_none() && sel.recordings.is_none() && sel.cohort.is_none() && !sel.all {
                anyhow::bail!("choose what to run on: --all, --recording, --patient or --cohort");
            }
            let r = jobs::create_run(&mut c, &cfg, &reg, &me, &a.actions, &params, &sel, a.force, a.priority)?;
            println!(
                "run {}: {} recording(s), {} job(s) queued; {} already done, {} already queued, {} not applicable",
                r.run, r.recordings, r.queued, r.done_already, r.pending_already, r.not_applicable
            );
        }
        Cmd::Work { workers } => {
            let n = workers.unwrap_or(cfg.workers).max(1);
            let before = jobs::counts(&*db.get()?)?;
            let reg = Arc::new(RwLock::new(Registry::load(&cfg)));
            let (db, cfg) = (Arc::new(db), Arc::new(cfg));
            let stop = Arc::new(AtomicBool::new(false));
            on_stop_signal(stop.clone());
            let threads: Vec<_> = (0..n)
                .map(|k| {
                    let (db, cfg, reg, stop) = (db.clone(), cfg.clone(), reg.clone(), stop.clone());
                    std::thread::spawn(move || jobs::work(db, cfg, reg, k, stop, true))
                })
                .collect();
            for t in threads {
                let _ = t.join();
            }
            if stop.load(Ordering::Relaxed) {
                jobs::requeue_own(&*db.get()?)?;
            }
            let after = jobs::counts(&*db.get()?)?;
            let n = |m: &HashMap<String, i64>, k: &str| m.get(k).copied().unwrap_or(0);
            println!(
                "queue empty: {} job(s) succeeded and {} failed here (medorama jobs --state failed shows why)",
                n(&after, "done") - n(&before, "done"),
                n(&after, "failed") - n(&before, "failed"),
            );
        }
        Cmd::Jobs { state, limit, json } => {
            let c = db.get()?;
            let rows = jobs::list(&c, &cfg, state.as_deref(), None, limit)?;
            if json {
                print_json(&rows);
                return Ok(());
            }
            for j in rows {
                println!(
                    "{:>6}  {:<10} {:<16} {:>4.0}%  r{:<5} {}{}",
                    j.id,
                    j.state,
                    j.action,
                    j.progress * 100.0,
                    j.recording,
                    j.recording_name,
                    if j.error.is_empty() { String::new() } else { format!("  ({})", j.error) }
                );
            }
        }
        Cmd::Cancel { job, run, all } => {
            if job.is_none() && run.is_none() && !all {
                anyhow::bail!("give --job, --run or --all");
            }
            let n = jobs::cancel(&*db.get()?, &me, job, run)?;
            println!("{n} job(s) cancelled");
        }
        Cmd::User(u) => user_cmd(&db, &me, u)?,
        Cmd::Audit { seal, verify, .. } if seal || verify => {
            // --seal --verify: seal first, then check every seal, the new one too
            let c = db.get()?;
            if seal {
                match medorama::audit::seal(&c, cfg.audit_export.as_deref())? {
                    Some(s) => println!("sealed {} entries (seal {}, {})", s.entries, s.seal, s.hash),
                    None => println!("nothing to seal"),
                }
            }
            if !verify {
                return Ok(());
            }
            let v = medorama::audit::verify_with_export(&c, cfg.audit_export.as_deref())?;
            match &v.broken {
                None => println!(
                    "the audit log holds: {} seals, {} entries sealed, {} not sealed yet",
                    v.seals, v.entries, v.unsealed
                ),
                Some(b) => anyhow::bail!("the audit log was changed: {b}"),
            }
        }
        Cmd::Audit { limit, .. } => {
            let c = db.get()?;
            let mut st =
                c.prepare("SELECT time, username, event, target, detail FROM audit ORDER BY id DESC LIMIT ?1")?;
            let mut rows = st.query([limit])?;
            while let Some(r) = rows.next()? {
                println!(
                    "{}  {:<16} {:<18} {} {}",
                    fmt_local(r.get(0)?, zone),
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?
                );
            }
        }
    }
    Ok(())
}

fn user_cmd(db: &Db, me: &User, cmd: UserCmd) -> anyhow::Result<()> {
    let mut c = db.get()?;
    let find = |c: &medorama::sql::Connection, name: &str| -> anyhow::Result<User> {
        users::get_by_name(c, name)?.ok_or_else(|| anyhow::anyhow!("no user {name}"))
    };
    match cmd {
        UserCmd::Add { username, role, phi, display_name, password_stdin, no_password } => {
            let role = Role::parse(&role).ok_or_else(|| anyhow::anyhow!("role: viewer, analyst or admin"))?;
            let pw = if no_password {
                users::unusable_password()?
            } else {
                read_password(password_stdin, &format!("Password for {username}: "))?
            };
            users::add(&c, &username, &display_name, role, phi, &pw)?;
            audit(&c, Some(me), "user_added", &username, &format!("role {}, phi {phi}", role.as_str()));
            println!("user {username} added ({}{})", role.as_str(), if phi { ", PHI" } else { "" });
        }
        UserCmd::List => {
            for u in users::list(&c)? {
                println!(
                    "{:<20} {:<8} {}{}  {}",
                    u.username,
                    u.role.as_str(),
                    if u.phi { "PHI" } else { "   " },
                    if u.disabled { " disabled" } else { "" },
                    u.display_name
                );
            }
        }
        UserCmd::Set { username, role, phi, disabled, display_name, password, password_stdin } => {
            let u = find(&c, &username)?;
            let role = match role {
                Some(r) => Some(Role::parse(&r).ok_or_else(|| anyhow::anyhow!("role: viewer, analyst or admin"))?),
                None => None,
            };
            let pw =
                if password || password_stdin { Some(read_password(password_stdin, "New password: ")?) } else { None };
            let ch = Changes { display_name: display_name.as_deref(), role, phi, disabled, password: pw.as_deref() };
            users::update(&mut c, u.id, &ch)?;
            audit(&c, Some(me), "user_changed", &username, "");
            println!("user {username} changed");
        }
        UserCmd::Remove { username } => {
            let u = find(&c, &username)?;
            users::delete(&mut c, u.id)?;
            audit(&c, Some(me), "user_deleted", &username, "");
            println!("user {username} deleted");
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = cli
        .home
        .map(|h| PathBuf::from(recording_engine::paths::absolute(&h.to_string_lossy())))
        .unwrap_or_else(default_home);
    match run(&home, cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_databases_from_the_manifest_and_without_it() {
        let dir = tempfile::tempdir().unwrap();
        // without demo-databases.json (a demo made before it): its db-builder and OMOP databases
        let old = demo_databases(dir.path(), "Europe/Paris");
        assert_eq!(old.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["demo", "demo_omop"]);
        assert_eq!(old[1].settings.mrn_authority, "DEMO");
        std::fs::write(
            dir.path().join("demo-databases.json"),
            r#"{"databases": [
                {"name": "demo", "kind": "dbb", "file": "demo.sqlite", "build": [["new", "@demo.sqlite"]]},
                {"name": "demo_registry", "kind": "views", "file": "registry/registry.sqlite",
                 "settings": {"persons": "SELECT 1", "entries": "SELECT 2", "mrn_authority": "DEMO"}},
                {"name": "demo_secure", "kind": "dbb", "file": "secure/secure.sqlite", "key_file": "keys/demo_secure.key"},
                {"kind": "dbb"}
            ], "actions_dirs": ["actions", "missing"]}"#,
        )
        .unwrap();
        let dbs = demo_databases(dir.path(), "Europe/Paris");
        assert_eq!(dbs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["demo", "demo_registry", "demo_secure"]);
        assert_eq!(dbs[1].file, dir.path().join("registry/registry.sqlite"));
        assert_eq!(dbs[1].settings.persons, "SELECT 1");
        assert_eq!(dbs[1].settings.timezone, "Europe/Paris");
        assert_eq!(dbs[0].settings.timezone, "");
        assert_eq!(dbs[2].key_file.as_deref(), Some(dir.path().join("keys/demo_secure.key").as_path()));
        // the plug-in folders that exist
        std::fs::create_dir(dir.path().join("actions")).unwrap();
        assert_eq!(demo_actions(dir.path()), [dir.path().join("actions")]);
    }
}
