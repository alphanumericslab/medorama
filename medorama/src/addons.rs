//! Add-ons: the adapters that read recording formats Medorama does not read itself (licensed or
//! vendor formats; docs/adapters.md). They are programs in the adapter folder (`adapters_dir`);
//! this module lists them, and installs or removes one. Installing is done on the server only
//! (the command line), never through the web interface: an adapter is a program that runs with
//! the catalog's rights.

use anyhow::{anyhow, bail, Context, Result};
use recording_engine::adapters::{self, Adapter};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The adapters installed in `dir`, as the API and `medorama addons list` show them.
pub fn list(dir: &Path) -> Vec<Value> {
    adapters::in_folders(&[dir.to_path_buf()]).iter().map(describe).collect()
}

fn describe(a: &Adapter) -> Value {
    json!({
        "name": a.name, "version": a.version, "implementation": a.implementation,
        "program": a.program.to_string_lossy(),
        "formats": a.formats.iter().map(|f| json!({"kind": f.kind, "label": f.label, "extensions": f.extensions})).collect::<Vec<_>>(),
    })
}

fn is_adapter_name(p: &Path) -> bool {
    let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    n.ends_with("-adapter") || n.ends_with("-adapter.exe")
}

/// Install the adapter `program` into `dir`: it must be named `<name>-adapter` and answer
/// `describe` with this protocol. An installed adapter of the same name is replaced.
pub fn install(dir: &Path, program: &Path) -> Result<Value> {
    if !program.is_file() {
        bail!("{}: no such file", program.display());
    }
    if !is_adapter_name(program) {
        bail!("{}: an adapter's program is named <name>-adapter (docs/adapters.md)", program.display());
    }
    let new = adapters::describe(program).map_err(|e| anyhow!("{}: {e}", program.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("{}", dir.display()))?;
    let target: PathBuf = dir.join(program.file_name().expect("a file name"));
    // the installed copy itself: nothing to do (removing the old one would remove it)
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(program), std::fs::canonicalize(&target)) {
        if a == b {
            adapters::rescan();
            return Ok(describe(&new));
        }
    }
    // copied beside the old one, checked, then put in its place at once: a failure leaves the
    // adapter installed as it was
    let tmp =
        dir.join(format!(".{}.new-{}", target.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    std::fs::copy(program, &tmp).with_context(|| format!("{}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    let installed = match adapters::describe(&tmp) {
        Ok(a) if a.name == new.name => a,
        Ok(a) => {
            let _ = std::fs::remove_file(&tmp);
            bail!("{}: the copy names itself {}, not {}", tmp.display(), a.name, new.name);
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            bail!("{}: {e}", target.display());
        }
    };
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("{}", target.display()));
    }
    // an older copy of the same adapter under another file name goes
    for old in adapters::in_folders(&[dir.to_path_buf()]).iter().filter(|a| a.name == new.name) {
        if old.program != target {
            std::fs::remove_file(&old.program).with_context(|| format!("{}", old.program.display()))?;
        }
    }
    let installed = adapters::describe(&target).unwrap_or(installed);
    adapters::rescan();
    Ok(describe(&installed))
}

/// Remove the installed adapter `name` from `dir`; returns what was removed.
pub fn remove(dir: &Path, name: &str) -> Result<Value> {
    let Some(a) = adapters::in_folders(&[dir.to_path_buf()]).into_iter().find(|a| a.name == name) else {
        bail!("no adapter named {name} in {}", dir.display());
    };
    std::fs::remove_file(&a.program).with_context(|| format!("{}", a.program.display()))?;
    adapters::rescan();
    Ok(describe(&a))
}
