//! Whether an installation is the demo's or a site's (docs/design/demo.md).  An installation
//! is one or the other from `medorama init` on, and never both: a demo installation connects
//! only demo databases (invented patients), a site's installation never one.  A demo database
//! says so itself (a `medorama_demo` table, which db-builder's `new --demo` and
//! tools/make-demo-data.py write), so that neither can be mistaken for the other.

use crate::db::now;
use crate::sql::{params, Connection, OptionalExtension};

/// The table a demo database holds.
pub const MARKER: &str = "medorama_demo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The demo's: invented patients only.
    Demo,
    /// A site's: real patients only.
    Real,
}

impl Mode {
    fn word(self) -> &'static str {
        match self {
            Mode::Demo => "demo",
            Mode::Real => "real",
        }
    }
}

/// The installation's mode; none: a control store from before 0.3.1, not decided yet
/// ([`settle`]).
pub fn mode(c: &Connection) -> anyhow::Result<Option<Mode>> {
    let v: Option<String> =
        c.query_row("SELECT value FROM settings WHERE key = 'installation'", [], |r| r.get(0)).optional()?;
    Ok(match v.as_deref() {
        Some("\"demo\"") => Some(Mode::Demo),
        Some("\"real\"") => Some(Mode::Real),
        _ => None,
    })
}

/// Whether this is a demo installation.
pub fn is_demo(c: &Connection) -> bool {
    matches!(mode(c), Ok(Some(Mode::Demo)))
}

/// Decide the mode (`medorama init`, or [`settle`]): once only.
pub fn set(c: &Connection, m: Mode) -> anyhow::Result<()> {
    if let Some(was) = mode(c)? {
        if was != m {
            anyhow::bail!("this installation is a {} one: it cannot become a {} one", was.word(), m.word());
        }
        return Ok(());
    }
    c.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ('installation', ?1, NULL, ?2)",
        params![format!("\"{}\"", m.word()), now()],
    )?;
    Ok(())
}

/// Whether a database may be connected to this installation, by its marker: a demo
/// installation takes demo databases only, a site's none.
pub fn admits(c: &Connection, name: &str, demo: bool) -> anyhow::Result<()> {
    match (settle(c)?, demo) {
        (Mode::Demo, false) => anyhow::bail!(
            "this is a demo installation: it connects demo databases (invented patients) only, and {name} is not one: \
             connect it to a site's installation (medorama init, without --demo)"
        ),
        (Mode::Real, true) => anyhow::bail!(
            "{name} is a demo database (invented patients): connect it to a demo installation (medorama init --demo), \
             never to a site's"
        ),
        _ => Ok(()),
    }
}

/// The installation's mode, decided now for a control store from before 0.3.1 by what it
/// connects (demo databases only: the demo's; else a site's); and checked: a database of the
/// other kind connected is refused, with what to do.
pub fn settle(c: &Connection) -> anyhow::Result<Mode> {
    let mut dbs = crate::sources::list(c)?;
    if mode(c)?.is_none() {
        // connected before 0.3.1: whether each is a demo database is read now (offline: as kept)
        for s in dbs.iter_mut().filter(|s| !s.settings.demo && s.is_demo()) {
            s.settings.demo = true;
            c.execute(
                "UPDATE databases SET settings = ?1 WHERE id = ?2",
                params![serde_json::to_string(&s.settings)?, s.id],
            )?;
        }
    }
    let demo: Vec<&str> = dbs.iter().filter(|s| s.settings.demo).map(|s| s.name.as_str()).collect();
    let real: Vec<&str> = dbs.iter().filter(|s| !s.settings.demo).map(|s| s.name.as_str()).collect();
    let m = match mode(c)? {
        Some(m) => m,
        None => {
            let m = if !demo.is_empty() && real.is_empty() { Mode::Demo } else { Mode::Real };
            if m == Mode::Real && demo.is_empty() || m == Mode::Demo {
                set(c, m)?;
            }
            m
        }
    };
    match m {
        Mode::Real if !demo.is_empty() => anyhow::bail!(
            "demo databases (invented patients) are connected to this site's installation: {}. A site's installation \
             never holds them: remove them (medorama databases remove NAME), and run the demo as its own installation \
             (medorama init --demo, with its own --home)",
            demo.join(", ")
        ),
        Mode::Demo if !real.is_empty() => anyhow::bail!(
            "databases that are not the demo's are connected to this demo installation: {}. Remove them \
             (medorama databases remove NAME), and connect them to a site's installation",
            real.join(", ")
        ),
        _ => Ok(m),
    }
}
