//! `medorama`: the catalog's command-line tool and server (docs/administration.md).
//!
//! Commands act directly on the database, as the operating-system user who can open it
//! (recorded in the audit log as `local:<user>`, with admin rights); the server's web page
//! and API act as signed-in catalog users.

use clap::{Args, Parser, Subcommand};
use medorama::actions::Registry;
use medorama::config::{default_home, Config, CONFIG_NAME};
use medorama::db::{audit, Db};
use medorama::imports::Opts;
use medorama::jobs::{self, Selection};
use medorama::query::{self, Search};
use medorama::users::{self, Changes, Role, User};
use medorama::util::{fmt_local, tz};
use medorama::{linkage, recordings, server};
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
    about = "Medorama: a unified patient-journey research platform linking EHR data, physiological waveforms, imaging and laboratory results across databases"
)]
struct Cli {
    /// the catalog's home folder (medorama.toml, database); default: $MEDORAMA_HOME,
    /// else the catalog `init` created last, else ~/.local/share/medorama
    /// (%LOCALAPPDATA%\medorama on Windows)
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a catalog: settings, database and the first admin
    Init(InitArgs),
    /// Run the web interface, the HTTP API and the background workers
    Serve(ServeArgs),
    /// Make the catalog given by --home the one later commands use without --home
    Use,
    /// Copy this catalog (SQLite) into an empty PostgreSQL database, every table and row; then
    /// set database_url in medorama.toml to use it (docs/administration.md, "Settings")
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
    /// Remove recordings from the catalog (by id or path, or --search); the files are never touched
    Remove {
        recordings: Vec<String>,
        /// the recordings matching a search (docs/user-guide.md, "Searching")
        #[arg(long)]
        search: Option<String>,
        /// also delete the catalog's results and indexes of them
        #[arg(long)]
        purge_results: bool,
        /// only say what would be removed
        #[arg(long)]
        dry_run: bool,
    },
    /// Re-check the recordings: missing, changed, index folders still there
    Check,
    /// Import a database: a CSV file, a Parquet file or a spreadsheet (.xlsx, .xls, .ods) of patients,
    /// encounters, measurements, notes, links to files... in any combination, as one batch.
    /// Several files with the same columns make one database (each read row by row).  Columns
    /// are recognised by their names, or mapped with --mapping (JSON or TOML); a known export
    /// (GE MUSE ECG metadata ...) is recognised and converted (--format); relative file paths
    /// are read from the table's folder
    ImportTable {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// a known export to convert: auto (recognised by its columns), none, or one of
        /// ge-muse (GE MUSE ECG metadata, the patient by MRN), ge-muse-patient-id (by a patient ID)
        #[arg(long, default_value = "auto")]
        format: String,
        #[arg(long)]
        mapping: Option<PathBuf>,
        /// the category of the table's items when it has no category column (lab, ecg, note, ...)
        #[arg(long, default_value = "")]
        category: String,
        #[command(flatten)]
        batch: BatchArgs,
    },
    /// Remove encounters (by encounter ID, --search or --all) with their bed stays and EHR items; recordings stay
    RemoveEncounters {
        encounters: Vec<String>,
        /// the encounters matching a search (docs/user-guide.md, "Searching")
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        all: bool,
        /// only say what would be removed
        #[arg(long)]
        dry_run: bool,
    },
    /// Imports (batches): list them, or remove one with everything it added
    #[command(subcommand)]
    Imports(ImportsCmd),
    /// Sources: list them, or remove everything from one
    #[command(subcommand)]
    Sources(SourcesCmd),
    /// The demo: invented patients in a cohort DEMO, for trying Medorama (docs/administration.md, "The demo")
    #[command(subcommand)]
    Demo(DemoCmd),
    /// Connected databases: live databases read beside the catalog (OMOP, a warehouse)
    #[command(subcommand)]
    Connections(ConnectionsCmd),
    /// List encounters
    List(ListArgs),
    /// List recordings
    Recordings {
        #[arg(long, default_value = "")]
        q: String,
        /// only recordings not linked to an encounter
        #[arg(long)]
        unlinked: bool,
        #[arg(long, default_value_t = 100)]
        limit: i64,
        #[arg(long)]
        json: bool,
    },
    /// Import a CLIF folder (the Common Longitudinal ICU data Format: its tables as CSV or
    /// Parquet files): each table a database (clif_patient, clif_vitals ...), in order (patients,
    /// hospitalizations, ADT, then the rest); a table in several files, one database
    ImportClif {
        dir: PathBuf,
        /// the source label (default: clif)
        #[arg(long, default_value = "clif")]
        source: String,
        /// check and count only; store nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// List the import formats: built in, and described in TOML files in `formats_dirs`
    /// (docs/administration.md, "Import formats"); --check a format file before installing it
    Formats {
        #[arg(long)]
        check: Option<PathBuf>,
    },
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

/// How an import is labelled (docs/administration.md, "Imports and sources").
#[derive(Args)]
struct BatchArgs {
    /// the batch's name (default: recordings, the folder they are in; a table, its file name)
    #[arg(long, default_value = "")]
    name: String,
    /// the source label (default: the name); a table's `source` column overrides it per row
    #[arg(long, default_value = "")]
    source: String,
    /// check and count only; store nothing
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum ImportsCmd {
    /// List the imports (newest first)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove one import and everything it added (what another import also added stays)
    Remove {
        id: i64,
        /// also delete the catalog's results and indexes of removed recordings
        #[arg(long)]
        purge_results: bool,
        #[arg(long)]
        dry_run: bool,
    },
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

#[derive(Subcommand)]
enum DemoCmd {
    /// Load the demo: its tables (the source DEMO), its OMOP database (demo_omop) and the cohort DEMO
    Add {
        /// a folder made by tools/make-demo-data.py (default: the one the installer made, --demo)
        dir: Option<PathBuf>,
    },
    /// Remove the demo and everything it added, as deleting the cohort DEMO does (its files stay)
    Remove {
        /// only say what would be removed
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum SourcesCmd {
    /// List the sources and what each contributes
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove everything a source contributed, in every import
    Remove {
        source: String,
        #[arg(long)]
        purge_results: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // command-line options, parsed once
enum ConnectionsCmd {
    /// Connect a database (docs/design/sources.md): checked first, each view must answer
    Add {
        /// its name, as criteria write it (omop.lactate)
        name: String,
        /// omop: an OMOP CDM 5.3 or 5.4 database; sql: any layout, its views in --views
        #[arg(long)]
        kind: String,
        /// postgres://user@host/db?sslmode=require (no password), or sqlite:/path/to/file
        #[arg(long)]
        url: String,
        /// the file holding the password (else $MEDORAMA_SOURCE_PASSWORD_<NAME>)
        #[arg(long)]
        password_file: Option<PathBuf>,
        /// certificate authorities (PEM) for the server's certificate, besides the system's
        #[arg(long)]
        ca_file: Option<PathBuf>,
        /// the schema of the CDM's tables
        #[arg(long, default_value = "")]
        schema: String,
        /// the schema of OMOP's vocabulary, when apart from the CDM's
        #[arg(long, default_value = "")]
        vocabulary_schema: String,
        /// the time zone of the database's times (default: the catalog's)
        #[arg(long, default_value = "")]
        timezone: String,
        /// what OMOP's person_source_value holds: mrn, patient_id or none
        #[arg(long, default_value = "mrn")]
        identifier: String,
        /// the authority that issued its MRNs
        #[arg(long, default_value = "")]
        mrn_authority: String,
        /// each query's time limit, seconds (PostgreSQL)
        #[arg(long, default_value_t = 60)]
        timeout_s: u64,
        /// the rows a criterion may read
        #[arg(long, default_value_t = 1_000_000)]
        max_rows: u64,
        /// kind sql: a TOML file with the views (persons, visits, entries, version) as SQL
        #[arg(long)]
        views: Option<PathBuf>,
    },
    /// List the connected databases
    List {
        #[arg(long)]
        json: bool,
    },
    /// Check that a connected database answers: each view, and its version
    Check { name: String },
    /// Link every person of a connected database to the catalog's patients
    Sync { name: String },
    /// Disconnect a database: its links and what it added to the catalog go
    Remove { name: String },
}

/// An import's progress, shown on one line of the terminal (when there is one) as it runs.
struct Shown {
    progress: Option<Arc<medorama::progress::Progress>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Shown {
    fn start() -> Shown {
        use std::io::IsTerminal;
        let stop = Arc::new(AtomicBool::new(false));
        if !std::io::stderr().is_terminal() {
            return Shown { progress: None, stop, thread: None };
        }
        let p = medorama::progress::Progress::new();
        let (q, s) = (p.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let v = q.view();
                let what = match v["phase"].as_str() {
                    Some("index") => "building the search index",
                    Some("match") => "matching waiting items and recordings",
                    _ => "reading",
                };
                let left =
                    v["left_s"].as_f64().map(|t| format!(" · about {} left", fmt_duration(t))).unwrap_or_default();
                eprint!(
                    "\r\x1b[2K  {what}: {} rows · {:.0}% · {} rows/s{left}",
                    v["rows"],
                    v["done"].as_f64().unwrap_or(0.0) * 100.0,
                    v["rate"]
                );
            }
            eprint!("\r\x1b[2K");
        });
        Shown { progress: Some(p), stop, thread: Some(thread) }
    }

    fn end(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Seconds as a person reads them: `40 s`, `3 min`, `1 h 20 min`.
fn fmt_duration(t: f64) -> String {
    let t = t.max(0.0).round() as u64;
    match t {
        0..=59 => format!("{t} s"),
        60..=3599 => format!("{} min", (t + 30) / 60),
        _ => format!("{} h {} min", t / 3600, (t % 3600) / 60),
    }
}

fn table_opts(file: &Path, b: &BatchArgs) -> Opts {
    let name = if b.name.is_empty() {
        file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        b.name.clone()
    };
    Opts::new(&name, &b.source, b.dry_run)
}

fn print_demo(d: &medorama::demo::Loaded) {
    println!(
        "Demo loaded: {} patient(s), {} encounter(s), {} item(s), {} recording(s) from {} table(s){}; the cohort {} holds them",
        d.patients,
        d.encounters,
        d.items,
        d.recordings,
        d.tables.len(),
        if d.connected { format!(", and the OMOP database {}", medorama::demo::CONNECTION) } else { String::new() },
        medorama::demo::NAME
    );
    for w in d.warnings.iter().take(10) {
        println!("  {w}");
    }
    println!(
        "Deleting the cohort {} (an admin) removes all of it: medorama demo remove does the same.",
        medorama::demo::NAME
    );
}

fn print_removal(r: &medorama::imports::Removal) {
    println!(
        "{}{} recording(s), {} patient(s), {} encounter(s), {} bed stay(s), {} EHR item(s){}",
        if r.dry_run { "would remove " } else { "removed " },
        r.recordings,
        r.patients,
        r.encounters,
        r.bed_stays,
        r.ehr_items,
        if r.kept_recordings + r.kept_encounters > 0 {
            format!(
                "; kept, because other imports also have them: {} recording(s), {} encounter(s)",
                r.kept_recordings, r.kept_encounters
            )
        } else {
            String::new()
        }
    );
}

#[derive(Args)]
struct InitArgs {
    /// where results and indexes go (required; never a folder of recordings)
    #[arg(long)]
    results: PathBuf,
    /// index folder (default: <results>/index); a viewer's cache_dir lets that viewer reuse the indexes
    #[arg(long)]
    index_dir: Option<PathBuf>,
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
    /// leave out the demo the installer made (tools/install-medorama.sh --demo); medorama demo add loads it later
    #[arg(long)]
    no_demo: bool,
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
}

#[derive(Args)]
struct ListArgs {
    /// search words (encounter ID, patient ID, MRN, name, date of birth)
    #[arg(long, default_value = "")]
    q: String,
    #[arg(long, default_value = "")]
    unit: String,
    /// ready, new, queued, indexing, stale, failed, missing or none
    #[arg(long, default_value = "")]
    status: String,
    #[arg(long, default_value_t = 100)]
    limit: i64,
    /// show identifiers
    #[arg(long)]
    phi: bool,
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
    /// encounters (encounter IDs)
    #[arg(long = "encounter")]
    encounters: Vec<String>,
    /// recordings (ids)
    #[arg(long = "recording")]
    recordings: Vec<i64>,
    /// encounters matching a search
    #[arg(long)]
    search: Option<String>,
    /// the encounters of a cohort's members (its number, as the Cohorts tab shows it)
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

fn init(home: &Path, a: InitArgs) -> anyhow::Result<()> {
    if home.join(CONFIG_NAME).exists() {
        anyhow::bail!("{} exists already; a catalog is set up there", home.join(CONFIG_NAME).display());
    }
    let results = PathBuf::from(recording_engine::paths::absolute(&a.results.to_string_lossy()));
    let mut cfg = Config::new(home, &results);
    if let Some(i) = a.index_dir {
        cfg.index_dir = PathBuf::from(recording_engine::paths::absolute(&i.to_string_lossy()));
    }
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
    std::fs::create_dir_all(&cfg.results_dir)?;
    std::fs::create_dir_all(&cfg.index_dir)?;
    // the database first, the settings file last: a failed init leaves no half-made catalog
    let pg = !cfg.database_url.is_empty();
    if !pg && cfg.database.exists() {
        anyhow::bail!(
            "{} exists already (from an earlier catalog): move it away, or choose another --home",
            cfg.database.display()
        );
    }
    let made = (|| -> anyhow::Result<String> {
        let db = Db::open(&cfg, true)?;
        let c = db.get()?;
        let n: i64 = c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if n > 0 {
            anyhow::bail!("this database holds a catalog already (it has users): choose an empty database");
        }
        users::add(&c, &a.admin, "", Role::Admin, true, &pw)?;
        audit(&c, Some(&User::local()), "init", &cfg.results_dir.to_string_lossy(), "");
        Ok(db.describe())
    })();
    let shown = match made {
        Ok(d) => d,
        Err(e) if pg => {
            anyhow::bail!("cannot create the catalog in {}: {e}", medorama::db::shown_url(&cfg.database_url))
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
    std::fs::write(home.join(CONFIG_NAME), cfg.to_toml())?;
    println!("Catalog created in {}", home.display());
    println!("  settings: {}", home.join(CONFIG_NAME).display());
    println!("  database: {shown} (holds identifiers: keep it private)");
    println!("  results:  {}", cfg.results_dir.display());
    println!("  indexes:  {}", cfg.index_dir.display());
    println!("  admin:    {} (with PHI permission)", a.admin);
    match medorama::config::remember_home(home) {
        Ok(p) => println!("Later commands use this catalog without --home (remembered in {}).", p.display()),
        Err(e) => println!("Could not remember this catalog ({e}): pass --home {} to later commands.", home.display()),
    }
    // the demo, when the installer made it (--demo), as the first admin's
    if let Some(dir) = medorama::demo::installed().filter(|_| !a.no_demo) {
        println!("Loading the demo from {} …", dir.display());
        let loaded = (|| -> anyhow::Result<medorama::demo::Loaded> {
            let db = Db::open(&cfg, false)?;
            let mut c = db.get()?;
            let admin = users::get_by_name(&c, &a.admin)?.ok_or_else(|| anyhow::anyhow!("no user {}", a.admin))?;
            medorama::demo::add(&mut c, &cfg, &admin, &dir)
        })();
        match loaded {
            Ok(d) => print_demo(&d),
            Err(e) => {
                println!("The demo was not loaded ({e}); the catalog is ready. Try again with: medorama demo add")
            }
        }
    }
    println!("Next: medorama import-table FILE (a database), then medorama serve");
    Ok(())
}

fn open(home: &Path) -> anyhow::Result<(Config, Db)> {
    let cfg = Config::load(home).map_err(anyhow::Error::msg)?;
    let db = Db::open(&cfg, false)?;
    Ok((cfg, db))
}

fn t(x: Option<f64>, zone: chrono_tz::Tz) -> String {
    x.map(|x| {
        let s = fmt_local(x, zone);
        s.get(..16).unwrap_or(&s).to_string()
    })
    .unwrap_or_else(|| "-".into())
}

fn recording_ids(c: &medorama::sql::Connection, args: &[String]) -> anyhow::Result<Vec<i64>> {
    let mut ids = vec![];
    for a in args {
        if let Ok(id) = a.parse::<i64>() {
            ids.push(id);
            continue;
        }
        let p = medorama::config::real(Path::new(&recording_engine::paths::absolute(a)));
        let id: Option<i64> = medorama::sql::OptionalExtension::optional(c.query_row(
            "SELECT id FROM recordings WHERE path = ?1",
            [p.to_string_lossy()],
            |r| r.get(0),
        ))?;
        ids.push(id.ok_or_else(|| anyhow::anyhow!("not in the catalog: {a}"))?);
    }
    Ok(ids)
}

fn encounter_ids(c: &medorama::sql::Connection, args: &[String]) -> anyhow::Result<Vec<i64>> {
    args.iter()
        .map(|e| {
            c.query_row("SELECT id FROM encounters WHERE encounter_id = ?1", [e], |r| r.get(0))
                .map_err(|_| anyhow::anyhow!("no encounter {e}"))
        })
        .collect()
}

fn connections_cmd(cfg: &Config, db: &Db, me: &User, cmd: ConnectionsCmd) -> anyhow::Result<()> {
    use medorama::sources;
    let mut c = db.get()?;
    let named = |c: &medorama::sql::Connection, name: &str| -> anyhow::Result<sources::Source> {
        sources::by_name(c, name)?.ok_or_else(|| anyhow::anyhow!("no connection {name} (medorama connections list)"))
    };
    match cmd {
        ConnectionsCmd::Add {
            name,
            kind,
            url,
            password_file,
            ca_file,
            schema,
            vocabulary_schema,
            timezone,
            identifier,
            mrn_authority,
            timeout_s,
            max_rows,
            views,
        } => {
            let abs = |p: Option<PathBuf>| {
                p.map(|p| recording_engine::paths::absolute(&p.to_string_lossy())).unwrap_or_default()
            };
            let mut settings = sources::Settings {
                schema,
                vocabulary_schema,
                timezone: if timezone.is_empty() { cfg.timezone.clone() } else { timezone },
                identifier,
                mrn_authority,
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
                (settings.persons, settings.visits, settings.entries, settings.version) =
                    (g("persons"), g("visits"), g("entries"), g("version"));
            }
            // a SQLite file: absolute, inside data_roots, not the catalog (as the page checks)
            let url = sources::checked_url(cfg, &url)?;
            let n = sources::NewSource {
                name,
                kind,
                url,
                password_file: abs(password_file),
                ca_file: abs(ca_file),
                settings,
            };
            let id = sources::add(&c, me, &n)?;
            let s = sources::get(&c, id)?.ok_or_else(|| anyhow::anyhow!("not added"))?;
            let ck = s.check()?;
            println!(
                "Connected {} ({}): views {}; version {}",
                s.name,
                s.kind,
                ck.views.join(", "),
                if ck.version.is_empty() { "-" } else { &ck.version }
            );
            println!(
                "Its persons are linked when a search finds them; `medorama connections sync {}` links them all.",
                s.name
            );
        }
        ConnectionsCmd::List { json } => {
            let v = sources::shown(&c)?;
            if json {
                print_json(&v);
            } else if v.is_empty() {
                println!("no connected databases (medorama connections add)");
            } else {
                for s in v {
                    println!("{}\t{}\t{} ({})\t{} person(s) linked", s.name, s.kind, s.url, s.database, s.persons);
                }
            }
        }
        ConnectionsCmd::Check { name } => {
            let s = named(&c, &name)?;
            let ck = s.check()?;
            println!(
                "{}: answers in {} ms; views {}; version {}",
                s.name,
                ck.ms,
                ck.views.join(", "),
                if ck.version.is_empty() { "-" } else { &ck.version }
            );
        }
        ConnectionsCmd::Sync { name } => {
            let s = named(&c, &name)?;
            let (n, linked) = sources::link::sync(&c, &s, s.zone(&cfg.timezone), me)?;
            println!("{}: {n} person(s); {linked} linked to the catalog's patients (the others are held for review, or have no valid identifier)", s.name);
        }
        ConnectionsCmd::Remove { name } => {
            let s = named(&c, &name)?;
            sources::remove(&mut c, cfg, me, s.id)?;
            println!("{} disconnected: its links and what it added to the catalog are removed", s.name);
        }
    }
    Ok(())
}

fn print_json(v: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
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
    let cfg = Arc::new(cfg);
    let database = db.describe();
    let app = Arc::new(server::App::new(cfg.clone(), Arc::new(db)));
    for p in &app.reg.read().unwrap().problems {
        eprintln!("medorama: action not loaded: {p}");
    }
    // imports a stop or a crash interrupted: removed (nothing of them is kept)
    match app.db.get().and_then(|mut c| medorama::imports::discard_unfinished(&mut c, &cfg, false)) {
        Ok(_) => {}
        Err(e) => eprintln!("medorama: imports left unfinished could not be removed yet: {e}"),
    }
    let stop = Arc::new(AtomicBool::new(false));
    let threads = server::start_background(&app, stop.clone());
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind((cfg.host.as_str(), cfg.port))
            .await
            .map_err(|e| anyhow::anyhow!("cannot listen on {}:{}: {e}", cfg.host, cfg.port))?;
        println!("Medorama: open http://{}:{}/", cfg.host, cfg.port);
        println!("  catalog: {}", cfg.home.display());
        println!("  database: {database}");
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
        println!("  results: {}; indexes: {}", cfg.results_dir.display(), cfg.index_dir.display());
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
        println!("Later commands use the catalog in {} (remembered in {}).", cfg.home.display(), p.display());
        return Ok(());
    }
    let me = User::local();
    let zone = tz(&cfg.timezone);
    match cmd {
        Cmd::Init(_) | Cmd::Use => unreachable!(),
        Cmd::Connections(cmd) => connections_cmd(&cfg, &db, &me, cmd)?,
        Cmd::CopyDatabase { to, password_file, ca_file } => {
            if !cfg.database_url.is_empty() {
                anyhow::bail!("this catalog is in PostgreSQL already ({})", db.describe());
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
        Cmd::Remove { recordings: r, search, purge_results, dry_run } => {
            let mut c = db.get()?;
            let mut ids = recording_ids(&c, &r)?;
            if let Some(q) = &search {
                ids.extend(recordings::search_ids(&c, &me, q, false, &cfg.timezone)?);
            }
            if dry_run {
                println!("would remove {} recording(s) from the catalog (files untouched)", ids.len());
                return Ok(());
            }
            let n = recordings::remove(&mut c, &cfg, &me, &ids, purge_results)?;
            println!("{n} recording(s) removed from the catalog (files untouched)");
        }
        Cmd::Check => {
            let r = recordings::check(&*db.get()?, &cfg)?;
            println!(
                "{} checked: {} missing, {} back, {} changed, {} index folders gone",
                r.checked, r.missing, r.returned, r.changed, r.index_lost
            );
        }
        Cmd::ImportTable { files, format, mapping, category, batch } => {
            let map = match mapping {
                Some(m) => linkage::read_mapping(&std::fs::read_to_string(&m)?)?,
                None => HashMap::new(),
            };
            let format = medorama::formats::Choice::parse(&format, &medorama::formats::all(&cfg).0)?;
            if files.len() > 1 && batch.name.trim().is_empty() {
                anyhow::bail!("several files make one database: give it a name (--name)");
            }
            let first = &files[0];
            let base = std::fs::canonicalize(first)
                .map_err(|e| anyhow::anyhow!("{}: {e}", first.display()))?
                .parent()
                .map(|p| p.to_path_buf());
            let parts: Vec<medorama::tables::Part> = files
                .iter()
                .map(|f| medorama::tables::Part {
                    name: f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    data: medorama::tables::PartData::File(f.clone()),
                })
                .collect();
            let mut c = db.get()?;
            let mut opts = table_opts(first, &batch);
            let shown = Shown::start();
            opts.progress = shown.progress.clone();
            let r = medorama::tables::import_parts(
                &mut c,
                &cfg,
                &me,
                &parts,
                &map,
                &category,
                base.as_deref(),
                &opts,
                format,
            );
            shown.end();
            let r = r?;
            println!(
                "{}{} rows: {} patient(s) and {} encounter(s) added, {} item(s) added ({} already there, {} waiting), {} recording(s) added, {} file(s) attached, {} row(s) held back",
                if r.dry_run { "(dry run) ".to_string() } else { format!("import {}: ", r.import_id) },
                r.rows, r.patients_added, r.encounters_added, r.items_added, r.duplicates, r.waiting, r.recordings_added,
                r.attachments, r.conflicts
            );
            if !r.ignored_columns.is_empty() {
                println!("  the database's own columns (in Databases and Cohorts): {}", r.ignored_columns.join(", "));
            }
            for w in &r.warnings {
                println!("  {w}");
            }
        }
        Cmd::RemoveEncounters { encounters, search, all, dry_run } => {
            let mut c = db.get()?;
            let mut ids = if all {
                let mut st = c.prepare("SELECT id FROM encounters")?;

                st.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?
            } else {
                encounter_ids(&c, &encounters)?
            };
            if let Some(q) = search {
                ids.extend(query::search_ids(&c, &me, &Search { q, ..Default::default() }, &cfg.timezone)?);
            }
            let r = linkage::remove_encounters(&mut c, &me, &ids, dry_run)?;
            print_removal(&r);
            println!("(recordings stay in the catalog, unlinked)");
        }
        Cmd::Imports(ImportsCmd::List { json }) => {
            let v = medorama::imports::list(&*db.get()?)?;
            if json {
                print_json(&v);
                return Ok(());
            }
            for b in v {
                println!(
                    "{:>5}  {:<10} {:<8} {}  {:<24} source {:<20} {} recording(s), {} encounter(s), {} EHR item(s)",
                    b.id,
                    b.kind,
                    b.state,
                    fmt_local(b.time, zone).get(..16).unwrap_or(""),
                    b.name,
                    b.source,
                    b.recordings,
                    b.encounters,
                    b.ehr_items
                );
            }
        }
        Cmd::Imports(ImportsCmd::Remove { id, purge_results, dry_run }) => {
            let mut c = db.get()?;
            let r = medorama::imports::remove_import(&mut c, &cfg, &me, id, purge_results, dry_run)?;
            print_removal(&r);
        }
        Cmd::Sources(SourcesCmd::List { json }) => {
            let v = medorama::imports::sources(&*db.get()?)?;
            if json {
                print_json(&v);
                return Ok(());
            }
            for x in v {
                println!(
                    "{:<30} {} recording(s), {} patient(s), {} encounter(s), {} EHR item(s), in {} import(s)",
                    x.source, x.recordings, x.patients, x.encounters, x.ehr_items, x.imports
                );
            }
        }
        Cmd::Demo(DemoCmd::Add { dir }) => {
            let dir = dir.or_else(medorama::demo::installed).ok_or_else(|| {
                anyhow::anyhow!(
                    "no demo was installed: give a folder made by tools/make-demo-data.py, or reinstall with --demo"
                )
            })?;
            let mut c = db.get()?;
            print_demo(&medorama::demo::add(&mut c, &cfg, &me, &dir)?);
        }
        Cmd::Demo(DemoCmd::Remove { dry_run }) => {
            let mut c = db.get()?;
            let r = medorama::demo::remove(&mut c, &cfg, &me, dry_run)?;
            print_removal(&r);
            if !dry_run {
                println!(
                    "the cohort {} and the connection {} are gone too",
                    medorama::demo::NAME,
                    medorama::demo::CONNECTION
                );
            }
        }
        Cmd::Sources(SourcesCmd::Remove { source, purge_results, dry_run }) => {
            let mut c = db.get()?;
            let r = medorama::imports::remove_source(&mut c, &cfg, &me, &source, purge_results, dry_run)?;
            print_removal(&r);
        }
        Cmd::List(a) => {
            let c = db.get()?;
            let mut user = me.clone();
            user.phi = a.phi;
            let s = Search { q: a.q, unit: a.unit, status: a.status, limit: Some(a.limit), ..Default::default() };
            let (total, rows) = query::search(&c, &user, &s, &cfg.timezone)?;
            if a.json {
                print_json(&serde_json::json!({"total": total, "items": rows}));
                return Ok(());
            }
            println!("{total} encounter(s)");
            println!(
                "{:<10} {:<12} {:<9} {:<16} {:<16} {:<14} {:>4}  {}",
                "status",
                "encounter",
                "patient",
                "admit",
                "discharge",
                "units",
                "recs",
                if a.phi { "identifiers" } else { "" }
            );
            for e in rows {
                let phi = e
                    .phi
                    .as_ref()
                    .map(|p| format!("{} {}, {} {} {}", p.mrn, p.last_name, p.first_name, p.birth_date, p.csn))
                    .unwrap_or_default();
                println!(
                    "{:<10} {:<12} {:<9} {:<16} {:<16} {:<14} {:>4}  {}",
                    e.status.map_or("-", |s| s.as_str()),
                    e.encounter_id,
                    e.patient_id,
                    t(e.admit_time, zone),
                    t(e.discharge_time, zone),
                    e.units.join(","),
                    e.recordings,
                    phi
                );
            }
        }
        Cmd::Recordings { q, unlinked, limit, json } => {
            let c = db.get()?;
            let (total, rows) = recordings::list(&c, &me, &q, unlinked, limit, 0, &cfg.timezone)?;
            if json {
                print_json(&serde_json::json!({"total": total, "items": rows}));
                return Ok(());
            }
            println!("{total} recording(s)");
            for r in rows {
                let progress = if r.status == recordings::Status::Indexing {
                    format!(" {:.0}%", r.progress * 100.0)
                } else {
                    String::new()
                };
                println!(
                    "{:>5}  {:<9} {:<5} {:<8} {:<16} {}{}{}",
                    r.id,
                    r.status.as_str(),
                    r.kind,
                    r.format,
                    t(r.t_start.or(r.name_time), zone),
                    r.path,
                    progress,
                    if r.error.is_empty() { String::new() } else { format!("  ({})", r.error) }
                );
            }
        }
        Cmd::ImportClif { dir, source, dry_run } => {
            let mut c = db.get()?;
            let shown = Shown::start();
            let done = medorama::formats::clif::import_folder(
                &mut c,
                &cfg,
                &me,
                &dir,
                &source,
                dry_run,
                shown.progress.clone(),
            );
            shown.end();
            let done = done?;
            for f in &done.skipped {
                println!("{}: not a CLIF table Medorama reads (skipped)", f.display());
            }
            for (name, files, r) in &done.imported {
                println!(
                    "{}{name} ({files} file(s)): {} rows: {} patient(s), {} encounter(s), {} bed stay(s), {} item(s) added, {} held back",
                    if r.dry_run { "(dry run) ".to_string() } else { format!("import {}: ", r.import_id) },
                    r.rows,
                    r.patients_added,
                    r.encounters_added,
                    r.bed_stays_added,
                    r.items_added,
                    r.conflicts
                );
                for w in r.warnings.iter().take(5) {
                    println!("  {w}");
                }
            }
        }
        Cmd::Formats { check } => {
            if let Some(f) = check {
                let text = std::fs::read_to_string(&f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
                let d = medorama::formats::declared::Declared::parse(&text, &f)?;
                println!(
                    "{}: the format {} ({}), {} column(s): {}",
                    f.display(),
                    d.key,
                    d.label,
                    d.columns.len(),
                    d.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                );
                return Ok(());
            }
            let (known, problems) = medorama::formats::all(&cfg);
            for f in &known {
                println!("{:<22} {:<6} {}  ({})", f.key(), f.category(), f.label(), f.origin());
            }
            for p in &problems {
                eprintln!("not read: {p}");
            }
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
            let sel = Selection {
                cohort: a.cohort,
                encounters: (!a.encounters.is_empty()).then(|| encounter_ids(&c, &a.encounters)).transpose()?,
                recordings: (!a.recordings.is_empty()).then_some(a.recordings),
                search: a.search.map(|q| Search { q, ..Default::default() }),
                all: a.all,
            };
            if sel.encounters.is_none()
                && sel.recordings.is_none()
                && sel.search.is_none()
                && sel.cohort.is_none()
                && !sel.all
            {
                anyhow::bail!("choose what to run on: --all, --encounter, --recording, --search or --cohort");
            }
            let r = jobs::create_run(&mut c, &reg, &me, &a.actions, &params, &sel, a.force, a.priority, &cfg.timezone)?;
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
            let rows = jobs::list(&c, state.as_deref(), None, limit)?;
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
