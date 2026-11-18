//! The `db-builder` command (README.md).

use clap::{Parser, Subcommand};
use db_builder::add::{Options, SourceReport};
use db_builder::{check, db};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "db-builder",
    version,
    about = "Tables, known exports and folders of recordings made into one open SQLite database"
)]
struct Cli {
    /// The database is (or is made) encrypted with SQLCipher, its key in $DB_BUILDER_KEY (64 hex digits).
    #[arg(long, global = true)]
    encrypt: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

/// What `add` and `recordings` share.
#[derive(clap::Args)]
struct Common {
    /// Map columns by hand: a TOML or JSON file of `field = "header"`.
    #[arg(long, value_name = "FILE")]
    mapping: Option<PathBuf>,
    /// The issuer of the patient IDs when a row names none.
    #[arg(long, value_name = "NAME", default_value = "")]
    issuer: String,
    /// The authority of the MRNs when a row names none.
    #[arg(long, value_name = "NAME", default_value = "")]
    mrn_authority: String,
    /// The time zone of local times in the inputs (a new database's; an existing one's must match).
    #[arg(long, value_name = "TZ")]
    timezone: Option<String>,
    /// The category of the entries (or recordings) when a row names none.
    #[arg(long, value_name = "KEY", default_value = "")]
    category: String,
    /// Dates with slashes are day first (03/02/2024 is 3 February).
    #[arg(long)]
    day_first: bool,
    /// Rewrite rules (TOML): files' paths, and identifiers' case and parts (README, "Rewrite rules").
    #[arg(long, value_name = "FILE")]
    rules: Option<PathBuf>,
    /// Only say what would be added (columns recognised, counts, warnings); the database is not
    /// changed, nor made.
    #[arg(long)]
    dry_run: bool,
}

/// `serve`'s options.
#[derive(clap::Args)]
struct ServeArgs {
    /// The one folder the web interface may browse, read and write in (never above it).
    #[arg(long, value_name = "DIR", required_unless_present = "set_password")]
    root: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = db_builder::serve::DEFAULT_PORT)]
    port: u16,
    /// Its home: the password file and the log of actions (default ~/.local/share/db-builder,
    /// %LOCALAPPDATA%\db-builder on Windows).
    #[arg(long, value_name = "DIR")]
    home: Option<PathBuf>,
    /// Set the web interface's password (read from the standard input when it is not a
    /// terminal; else asked twice), then stop.
    #[arg(long)]
    set_password: bool,
    /// The account's name, with --set-password.
    #[arg(long, value_name = "NAME", default_value = "admin", requires = "set_password")]
    user: String,
    /// Mark the session cookie Secure (served over HTTPS, behind a reverse proxy).
    #[arg(long)]
    secure_cookies: bool,
    /// A session unused this long ends (minutes).
    #[arg(long, value_name = "MINUTES", default_value_t = 60)]
    idle_minutes: u64,
}

#[derive(Subcommand)]
enum Cmd {
    /// Make a new, empty database.
    New {
        out: PathBuf,
        /// The time zone of local times in the inputs.
        #[arg(long, value_name = "TZ", default_value = db::DEFAULT_TIMEZONE)]
        timezone: String,
        /// Mark it a demo database (invented patients): Medorama connects it to a demo
        /// installation only, and a site's installation refuses it.
        #[arg(long)]
        demo: bool,
    },
    /// Add tables (CSV, Excel, OpenDocument, Parquet; known exports) or CLIF folders.
    Add {
        out: PathBuf,
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[command(flatten)]
        common: Common,
        /// `auto` (recognised by its columns), `none`, or a format's key (`db-builder formats`).
        #[arg(long, value_name = "KEY", default_value = "auto")]
        format: String,
        /// A TOML file describing a format, or a folder of them (repeatable).
        #[arg(long, value_name = "PATH")]
        formats: Vec<PathBuf>,
    },
    /// Add a folder of recordings, with their patients from a table or a pattern of their names.
    Recordings {
        out: PathBuf,
        folder: PathBuf,
        /// A table of each recording's file and patient.
        #[arg(long, value_name = "TABLE")]
        patients: Option<PathBuf>,
        /// The patient ID in the files' names: `{patient}` the ID, `*` any text (`{patient}_*.edf`).
        #[arg(long, value_name = "PATTERN")]
        pattern: Option<String>,
        #[command(flatten)]
        common: Common,
    },
    /// Report what a reader cannot rely on: missing issuers, shared identifiers, dangling keys, missing files.
    Check { out: PathBuf },
    /// Take sources out (their ids: `db-builder info`).
    RemoveSource {
        out: PathBuf,
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// What a database holds: its meta, its tables' rows, its sources.
    Info { out: PathBuf },
    /// Rewrite the paths of the files a database lists by the path rules of a rules file (data moved
    /// or mounted elsewhere).
    Rewrite {
        out: PathBuf,
        #[arg(long, value_name = "FILE")]
        rules: PathBuf,
        /// Only count what would change.
        #[arg(long)]
        dry_run: bool,
    },
    /// The web interface, on its own port: `serve --root DIR` (README, "Web interface").
    Serve(ServeArgs),
    /// The known formats.
    Formats {
        #[arg(long, value_name = "PATH")]
        formats: Vec<PathBuf>,
    },
}

fn options(common: &Common) -> anyhow::Result<Options> {
    let mapping = match &common.mapping {
        Some(f) => db_builder::tables::read_mapping(
            &std::fs::read_to_string(f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?,
        )
        .map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?,
        None => Default::default(),
    };
    if !common.category.is_empty() && db_builder::categories::named(&common.category).is_none() {
        anyhow::bail!("--category {:?}: not a category", common.category);
    }
    Ok(Options {
        issuer: common.issuer.trim().into(),
        mrn_authority: common.mrn_authority.trim().into(),
        category: db_builder::categories::named(&common.category).unwrap_or("").into(),
        mapping,
        day_first: common.day_first,
        rules: match &common.rules {
            Some(f) => db_builder::rules::Rules::read(f)?,
            None => Default::default(),
        },
        dry_run: common.dry_run,
        ..Default::default()
    })
}

fn say(r: &SourceReport, dry_run: bool) {
    match (r.already, r.source) {
        (Some(id), _) => println!("{}: already added (source {id}), nothing done", r.file),
        (None, Some(_)) if dry_run => {
            if !r.replaced.is_empty() {
                println!("{}: changed since it was added: would replace its earlier version", r.file);
            }
            println!(
                "{}: would add ({}{}): {} rows; {} persons and {} visits added; {} entries, {} recordings; {} left out",
                r.file,
                r.kind,
                if r.format.is_empty() { String::new() } else { format!(", {}", r.format) },
                r.rows,
                r.persons_added,
                r.visits_added,
                r.entries,
                r.recordings,
                r.left_out
            );
            for c in &r.columns {
                let what = match c.how.as_str() {
                    "field" => c.field.clone(),
                    "measure" if c.unit.is_empty() => format!("a measurement: {}", c.field),
                    "measure" => format!("a measurement: {} ({})", c.field, c.unit),
                    _ => "extras".into(),
                };
                println!("  column {:?} -> {what}", c.header);
            }
        }
        (None, Some(id)) => {
            if !r.replaced.is_empty() {
                let ids: Vec<String> = r.replaced.iter().map(|i| i.to_string()).collect();
                println!("{}: changed since it was added: source {} removed", r.file, ids.join(", "));
            }
            println!(
                "{}: source {id} ({}): {} rows; {} persons and {} visits added; {} entries, {} recordings; {} left out",
                r.file, r.kind, r.rows, r.persons_added, r.visits_added, r.entries, r.recordings, r.left_out
            );
        }
        _ => {}
    }
    for w in &r.warnings {
        println!("  warning: {w}");
    }
}

/// The database `add` and `recordings` write: made when it does not exist yet; in a dry run, a
/// database not made yet is one in memory (nothing is written to the disk).
fn open_to_add(out: &std::path::Path, how: &db::Opening, common: &Common) -> anyhow::Result<rusqlite::Connection> {
    if common.dry_run && !out.exists() {
        return db::scratch(common.timezone.as_deref().unwrap_or(db::DEFAULT_TIMEZONE));
    }
    db::open_or_create(out, how, common.timezone.as_deref())
}

/// `db-builder serve`: set the password (`--set-password`), or run the web interface.
fn serve(a: ServeArgs, encrypt: bool) -> anyhow::Result<i32> {
    use std::io::{BufRead, IsTerminal};
    let home = a.home.map(|h| db::absolute(&h)).unwrap_or_else(db_builder::serve::default_home);
    if a.set_password {
        let pw = if std::io::stdin().is_terminal() {
            let pw = rpassword::prompt_password(format!(
                "A password for {} (at least {} characters): ",
                a.user,
                db_builder::serve::auth::MIN_PASSWORD
            ))?;
            if rpassword::prompt_password("Repeat it: ")? != pw {
                anyhow::bail!("the two entries differ: no password set");
            }
            pw
        } else {
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            line.trim_end_matches(['\r', '\n']).to_string()
        };
        db_builder::serve::auth::set_password(&home, &a.user, &pw)?;
        println!(
            "The web interface's password is set for {} (in {}).",
            a.user,
            db_builder::serve::auth::password_file(&home).display()
        );
        return Ok(0);
    }
    let settings = db_builder::serve::Settings {
        root: a.root.expect("--root (clap requires it)"),
        home,
        key: if encrypt { Some(db::key_from_env()?) } else { None },
        port: a.port,
        secure_cookies: a.secure_cookies,
        idle: std::time::Duration::from_secs(a.idle_minutes.max(1) * 60),
    };
    db_builder::serve::run(settings, &a.host)?;
    Ok(0)
}

fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    if let Cmd::Serve(a) = cli.cmd {
        return serve(a, cli.encrypt);
    }
    let how = db::Opening::new(cli.encrypt)?;
    match cli.cmd {
        Cmd::Serve(_) => unreachable!(),
        Cmd::New { out, timezone, demo } => {
            let c = db::create(&out, &how, &timezone)?;
            if demo {
                db::mark_demo(&c)?;
            }
            println!(
                "{}: a new {}database (schema {}, local times in {timezone})",
                out.display(),
                if demo { "demo " } else { "" },
                db::SCHEMA
            );
        }
        Cmd::Add { out, inputs, common, format, formats } => {
            let mut opts = options(&common)?;
            opts.format = format;
            opts.formats = formats;
            let mut c = open_to_add(&out, &how, &common)?;
            for r in db_builder::add::add(&mut c, &out, &inputs, &opts)? {
                say(&r, common.dry_run);
            }
            if common.dry_run {
                println!("(a dry run: {} not changed)", out.display());
            }
        }
        Cmd::Recordings { out, folder, patients, pattern, common } => {
            let opts = options(&common)?;
            let mut c = open_to_add(&out, &how, &common)?;
            let who = db_builder::recordings::Patients { table: patients, pattern };
            say(&db_builder::recordings::add_folder(&mut c, &out, &folder, &who, &opts)?, common.dry_run);
            if common.dry_run {
                println!("(a dry run: {} not changed)", out.display());
            }
        }
        Cmd::Check { out } => {
            let c = db::open(&out, &how)?;
            let problems = check::check(&c, &out)?;
            if problems.is_empty() {
                println!("{}: no problem found", out.display());
                return Ok(0);
            }
            for p in &problems {
                println!("{}: {}", p.count, p.what);
                for e in &p.examples {
                    println!("  {e}");
                }
                if p.count > p.examples.len() {
                    println!("  ...");
                }
            }
            return Ok(1);
        }
        Cmd::Rewrite { out, rules, dry_run } => {
            let rules = db_builder::rules::Rules::read(&rules)?;
            if !rules.has_paths() {
                anyhow::bail!("the rules file has no [[path]] rule");
            }
            let mut c = db::open(&out, &how)?;
            let tx = c.transaction()?;
            let n = db::rewrite_paths(&tx, &rules, !dry_run)?;
            tx.commit()?;
            println!(
                "{}{} recordings, {} entries' files, {} sources' inputs",
                if dry_run { "would rewrite: " } else { "rewritten: " },
                n.recordings,
                n.files,
                n.sources
            );
            if !dry_run {
                println!("(db-builder check {} lists the files still missing)", out.display());
            }
        }
        Cmd::RemoveSource { out, ids } => {
            let mut c = db::open(&out, &how)?;
            let tx = c.transaction()?;
            for id in ids {
                let r = db::remove_source(&tx, id)?;
                println!(
                    "source {id} removed: {} entries, {} recordings, {} visits, {} persons",
                    r.entries, r.recordings, r.visits, r.persons
                );
            }
            db::bump(&tx)?;
            tx.commit()?;
        }
        Cmd::Info { out } => {
            let c = db::open(&out, &how)?;
            println!("{}{}", if how.key.is_some() { "encrypted   yes\n" } else { "" }, check::info(&c)?.trim_end());
        }
        Cmd::Formats { formats } => {
            let (known, problems) = db_builder::formats::all(&formats);
            for f in &known {
                println!("{:<34} {:<8} {}", f.key(), f.category(), f.label());
            }
            for p in problems {
                println!("warning: {p}");
            }
        }
    }
    Ok(0)
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("db-builder: {e:#}");
            std::process::exit(2);
        }
    }
}
