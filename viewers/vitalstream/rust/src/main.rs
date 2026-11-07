//! `vitalstream-rs`: the VitalStream viewer (Rust).  Starts the web server; `--index PATH`
//! only indexes.  Same options, settings keys, API and password as the Python viewer.

use clap::Parser;
use recording_engine::index;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use vitalstream_rs::{auth, server, settings};

#[derive(Parser)]
#[command(
    name = "vitalstream-rs",
    version,
    about = "Web viewer for physiological recordings (WFDB folders, and the formats of installed adapters)"
)]
struct Cli {
    /// settings file (TOML); default: <viewer>/vitalstream.toml (run from a clone's build
    /// folder) or ~/.config/vitalstream/vitalstream.toml if present (never one in the working
    /// directory); keys of its [rs] section apply here
    #[arg(long)]
    config: Option<String>,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// display time zone, e.g. America/New_York
    #[arg(long)]
    timezone: Option<String>,
    /// folder the viewer may open (repeatable), inside the viewer's root (set when it was
    /// built); default: the viewer's root
    #[arg(long = "root")]
    data_roots: Vec<String>,
    #[arg(long)]
    cache_dir: Option<String>,
    /// folder of adapters, programs that read other recording formats (docs/adapters.md)
    #[arg(long)]
    adapters_dir: Option<String>,
    /// also mark the irregularities a reader repaired (data blocks put back in order)
    #[arg(long)]
    qc_verbose: bool,
    /// do not open a browser window
    #[arg(long)]
    no_browser: bool,
    /// set (or change) the password the page asks for, then exit
    #[arg(long)]
    set_password: bool,
    /// remove the password, then exit
    #[arg(long)]
    clear_password: bool,
    /// run without asking for the password
    #[arg(long)]
    no_auth: bool,
    /// index PATH into the cache and exit
    #[arg(long, value_name = "PATH")]
    index: Option<String>,
}

fn open_browser(url: String) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let cmd = if cfg!(target_os = "macos") {
            ("open", vec![url])
        } else if cfg!(windows) {
            ("cmd", vec!["/C".into(), "start".into(), url])
        } else {
            ("xdg-open", vec![url])
        };
        let _ = std::process::Command::new(cmd.0)
            .args(cmd.1)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });
}

fn main() -> ExitCode {
    let a = Cli::parse();
    let mut s = match settings::load_settings(a.config.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("vitalstream-rs: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(h) = a.host {
        s.host = h;
    }
    if let Some(p) = a.port {
        s.port = p;
    }
    if let Some(t) = a.timezone {
        s.timezone = t;
    }
    if !a.data_roots.is_empty() {
        s.data_roots = a.data_roots;
    }
    if let Some(c) = a.cache_dir {
        s.cache_dir = PathBuf::from(settings::absolute(&c));
    }
    if let Some(d) = a.adapters_dir {
        s.adapters_dir = PathBuf::from(settings::absolute(&d));
    }
    if a.qc_verbose {
        s.qc_verbose = true;
    }
    let s = match s.finish() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("vitalstream-rs: {e}");
            return ExitCode::FAILURE;
        }
    };
    recording_engine::adapters::set_folders(vec![s.adapters_dir.clone()]);
    recording_engine::sources::wfdb_folder::set_limits(s.limits);
    // the readers open no file outside the data roots (a signal file a header names, a link)
    recording_engine::paths::set_allowed_roots(Some(s.data_roots.clone()));
    match &s.source {
        Some(f) => eprintln!("vitalstream-rs: settings from {}", f.display()),
        None => eprintln!("vitalstream-rs: no settings file (defaults)"),
    }
    for r in &s.outside {
        eprintln!("vitalstream-rs: {r} is outside this viewer's root ({}): left out", settings::built_root());
    }
    #[cfg(not(feature = "password"))]
    if a.set_password {
        eprintln!("vitalstream-rs: this viewer was built without password support");
        return ExitCode::FAILURE;
    }
    #[cfg(feature = "password")]
    if a.set_password {
        let Ok(pw) = rpassword::prompt_password("New VitalStream password: ") else { return ExitCode::FAILURE };
        if pw.chars().count() < 8 {
            println!("Use at least 8 characters.");
            return ExitCode::FAILURE;
        }
        if rpassword::prompt_password("Repeat it: ").ok().as_deref() != Some(pw.as_str()) {
            println!("The two entries differ; nothing changed.");
            return ExitCode::FAILURE;
        }
        if let Err(e) = auth::set_password(&s.auth_file, &pw) {
            eprintln!("vitalstream-rs: {}: {e}", s.auth_file.display());
            return ExitCode::FAILURE;
        }
        println!("Password set ({}). Restart the viewer for it to take effect.", s.auth_file.display());
        return ExitCode::SUCCESS;
    }
    if a.clear_password {
        if let Err(e) = auth::clear_password(&s.auth_file) {
            eprintln!("vitalstream-rs: {e}");
            return ExitCode::FAILURE;
        }
        println!("Password removed.");
        return ExitCode::SUCCESS;
    }
    if let Some(p) = a.index {
        let path = settings::absolute(&p);
        return match index::build_index(std::path::Path::new(&path), &s.cache_dir, &|_| {}) {
            Ok(key) => {
                println!("indexed {p} -> {}/{key}/", s.cache_dir.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("vitalstream-rs: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let protected = !a.no_auth && auth::load(&s.auth_file).is_some();
    if s.require_password && !protected {
        eprintln!(
            "vitalstream-rs: a password is required here (require_password){}",
            if auth::SUPPORTED {
                "; set one with --set-password"
            } else {
                ", and this viewer was built without password support"
            }
        );
        return ExitCode::FAILURE;
    }
    let app = match server::App::new(s.clone(), Some(protected)) {
        Ok(app) => Arc::new(app),
        Err(e) => {
            eprintln!("vitalstream-rs: {e}");
            return ExitCode::FAILURE;
        }
    };
    // an IPv6 address is written in brackets in a URL
    let url = if s.host.contains(':') {
        format!("http://[{}]:{}/", s.host, s.port)
    } else {
        format!("http://{}:{}/", s.host, s.port)
    };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    rt.block_on(async move {
        let listener = match tokio::net::TcpListener::bind((s.host.as_str(), s.port)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("vitalstream-rs: cannot listen on {}:{}: {e}", s.host, s.port);
                return ExitCode::FAILURE;
            }
        };
        println!("VitalStream (Rust): open {url}");
        println!(
            "  root: {}; data roots: {}; cache: {}; time zone {}",
            settings::built_root(),
            s.data_roots.join(", "),
            s.cache_dir.display(),
            s.timezone
        );
        if !recording_engine::sources::wfdb_folder::SUPPORTED {
            println!("  built without WFDB support: WFDB records and folders are refused");
        }
        if protected {
            println!("  password protected (vitalstream-rs --set-password to change it)");
        } else if !auth::SUPPORTED {
            println!("  built without password support: anyone who can connect to this port can read the files under its root");
        } else {
            println!(
                "  no password: anyone who can connect to this port can read your files through it.\n  On a shared machine, set one once with: vitalstream-rs --set-password"
            );
        }
        if ["0.0.0.0", "::"].contains(&s.host.as_str()) && s.allowed_hosts.is_empty() {
            println!(
                "  listening on every address: only requests to localhost are answered until the names\n  this machine is reached by are listed in allowed_hosts"
            );
        }
        if !a.no_browser {
            open_browser(url);
        }
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        let service = server::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>();
        match axum::serve(listener, service).with_graceful_shutdown(shutdown).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("vitalstream-rs: {e}");
                ExitCode::FAILURE
            }
        }
    })
}
