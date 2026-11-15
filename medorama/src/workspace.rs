//! The workspace (docs/design/databases-only.md, "The workspace" and "Keys and encryption"):
//! the folder where Medorama keeps what it generates, one folder per connected database and one
//! file per cohort, each encrypted with a data key of its own.
//!
//! Envelope encryption: a data key (256 random bits) per owner (`db:7`, `cohort:3`,
//! `derived:12`) is kept only wrapped, AES-256-GCM under the master key, in the control store's
//! `keys` table.  The master key comes from `$MEDORAMA_MASTER_KEY`, `master_key_command` (a
//! keyring, Vault, a KMS) or `master_key_file`, never from the workspace.  Files are SQLite
//! databases encrypted by SQLCipher with their owner's data key.  Shredding deletes the wrapped
//! key first: the files are then unreadable, before they are deleted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use recording_engine::store::Store;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::config::Config;
use crate::db::now;
use crate::sql::{params, Connection, OptionalExtension};

/// A 256-bit key, wiped from memory when dropped (and so are its copies as text: [`Key::hex`]).
pub struct Key([u8; 32]);

impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Key {
    pub fn random() -> anyhow::Result<Key> {
        let mut k = [0u8; 32];
        getrandom::fill(&mut k).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
        Ok(Key(k))
    }

    fn from_hex(s: &str) -> Option<Key> {
        let s = s.trim();
        if s.len() != 64 {
            return None;
        }
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(Key(k))
    }

    /// A key of these bytes (the gate's secret of an earlier version, kept in plain text).
    pub fn from_bytes(b: &[u8]) -> Option<Key> {
        let k: [u8; 32] = b.try_into().ok()?;
        Some(Key(k))
    }

    /// The key's bytes (a copy, wiped when dropped).
    pub fn bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.0.to_vec())
    }

    /// The key's 64 hex digits, wiped when dropped.
    fn hex(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::with_capacity(64));
        for b in self.0 {
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            out.push(char::from_digit((b & 15) as u32, 16).unwrap_or('0'));
        }
        out
    }
}

/// The master key, and its id (the first 16 hex digits of its SHA-256): which master key wrapped
/// a data key, so that a wrong one is named, not mistaken for damaged data.  It is held as one
/// cipher, made once (no copy of the key per use), wiped when dropped.
pub struct Master {
    cipher: Aes256Gcm,
    pub id: String,
}

impl Master {
    pub fn new(key: Key) -> Master {
        let id = Sha256::digest(key.0).iter().take(8).map(|b| format!("{b:02x}")).collect();
        Master { cipher: Aes256Gcm::new_from_slice(&key.0).expect("a 32-byte key"), id }
    }

    fn cipher(&self) -> &Aes256Gcm {
        &self.cipher
    }

    /// `owner`'s data key, wrapped: hex of nonce (12 bytes) and the sealed key.  The owner is
    /// the associated data: a wrapped key moved to another owner does not unwrap.
    pub fn wrap(&self, owner: &str, k: &Key) -> anyhow::Result<String> {
        self.seal(owner, &k.0)
    }

    pub fn unwrap(&self, owner: &str, wrapped: &str) -> anyhow::Result<Key> {
        let k = self.open(owner, wrapped)?;
        if k.len() != 32 {
            anyhow::bail!("{owner}: its wrapped key is damaged");
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&k);
        Ok(Key(out))
    }

    /// Seal any secret (a database's password) as keys are wrapped.
    pub fn seal(&self, owner: &str, secret: &[u8]) -> anyhow::Result<String> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
        let sealed = self
            .cipher()
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: secret, aad: owner.as_bytes() })
            .map_err(|_| anyhow::anyhow!("wrapping a key failed"))?;
        Ok(nonce.iter().chain(sealed.iter()).map(|b| format!("{b:02x}")).collect())
    }

    pub fn open(&self, owner: &str, wrapped: &str) -> anyhow::Result<Vec<u8>> {
        let bytes: Vec<u8> = (0..wrapped.len() / 2)
            .map(|i| wrapped.get(2 * i..2 * i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .collect::<Option<_>>()
            .ok_or_else(|| anyhow::anyhow!("{owner}: its wrapped key is damaged"))?;
        if bytes.len() < 12 + 16 {
            anyhow::bail!("{owner}: its wrapped key is damaged");
        }
        self.cipher()
            .decrypt(Nonce::from_slice(&bytes[..12]), Payload { msg: &bytes[12..], aad: owner.as_bytes() })
            .map_err(|_| anyhow::anyhow!("{owner}: its key does not unwrap with this master key"))
    }
}

/// The master key of this process, loaded once ([`load`]): the server's, or a command's.
static MASTER: std::sync::OnceLock<Master> = std::sync::OnceLock::new();

/// Load the master key for this process (the server at start, each command).  A second call
/// keeps the first key.
pub fn load(cfg: &Config) -> anyhow::Result<&'static Master> {
    if let Some(m) = MASTER.get() {
        return Ok(m);
    }
    let m = master(cfg)?;
    Ok(MASTER.get_or_init(|| m))
}

/// The master key loaded for this process.
pub fn loaded() -> anyhow::Result<&'static Master> {
    MASTER.get().ok_or_else(|| anyhow::anyhow!("the master key is not loaded"))
}

/// A secret sealed for `owner` with the loaded master key.
pub fn seal_secret(owner: &str, secret: &str) -> anyhow::Result<String> {
    loaded()?.seal(owner, secret.as_bytes())
}

/// A secret sealed by [`seal_secret`].
pub fn unwrap_secret(owner: &str, wrapped: &str) -> anyhow::Result<String> {
    String::from_utf8(loaded()?.open(owner, wrapped)?).map_err(|_| anyhow::anyhow!("{owner}: not text"))
}

/// A master key from text (64 hex digits): a new key read from its file (`keys rotate`).
pub fn master_from_hex(s: &str) -> Option<Master> {
    Key::from_hex(s).map(Master::new)
}

/// The default master key file, made by `init`: `<home>/master.key`, readable by Medorama's
/// account only.  Outside the workspace, so that a copy of the workspace alone cannot be read.
pub fn default_master_file(cfg: &Config) -> PathBuf {
    cfg.home.join("master.key")
}

/// The master key: `$MEDORAMA_MASTER_KEY`, else `master_key_command`'s output, else
/// `master_key_file` (by default `<home>/master.key`).  64 hex digits in each case.
pub fn master(cfg: &Config) -> anyhow::Result<Master> {
    let bad = |from: &str| anyhow::anyhow!("{from}: the master key must be 64 hex digits (256 bits)");
    if let Some(v) = std::env::var("MEDORAMA_MASTER_KEY").ok().filter(|v| !v.trim().is_empty()) {
        return Key::from_hex(&v).map(Master::new).ok_or_else(|| bad("MEDORAMA_MASTER_KEY"));
    }
    if !cfg.master_key_command.trim().is_empty() {
        let out = shell(&cfg.master_key_command).output().map_err(|e| anyhow::anyhow!("master_key_command: {e}"))?;
        if !out.status.success() {
            anyhow::bail!("master_key_command failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        return Key::from_hex(&String::from_utf8_lossy(&out.stdout))
            .map(Master::new)
            .ok_or_else(|| bad("master_key_command"));
    }
    let f = cfg.master_key_file.clone().unwrap_or_else(|| default_master_file(cfg));
    let text = std::fs::read_to_string(&f).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "{}: no master key (docs/administration.md, \"Keys\"): set MEDORAMA_MASTER_KEY, master_key_command or master_key_file",
                f.display()
            )
        } else {
            anyhow::anyhow!("{}: {e}", f.display())
        }
    })?;
    Key::from_hex(&text).map(Master::new).ok_or_else(|| bad(&f.display().to_string()))
}

fn shell(cmd: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

/// Make the master key file (`init`), unless the key comes from elsewhere or the file exists.
pub fn make_master_file(cfg: &Config) -> anyhow::Result<Option<PathBuf>> {
    if std::env::var("MEDORAMA_MASTER_KEY").is_ok_and(|v| !v.trim().is_empty())
        || !cfg.master_key_command.trim().is_empty()
    {
        return Ok(None);
    }
    let f = cfg.master_key_file.clone().unwrap_or_else(|| default_master_file(cfg));
    if f.exists() {
        return Ok(None);
    }
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = Zeroizing::new(format!("{}\n", Key::random()?.hex().as_str()));
    write_private(&f, text.as_bytes())?;
    Ok(Some(f))
}

fn write_private(f: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut file = o.open(f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
    file.write_all(bytes)?;
    Ok(())
}

// ------------------------------------------------------------------ data keys

/// Make `owner`'s data key and keep it wrapped.  An owner has one key for its life.
pub fn create_key(c: &Connection, m: &Master, owner: &str) -> anyhow::Result<Key> {
    let k = Key::random()?;
    let n = c.execute(
        "INSERT OR IGNORE INTO keys (owner, wrapped, master_id, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![owner, m.wrap(owner, &k)?, m.id, now()],
    )?;
    if n == 0 {
        anyhow::bail!("{owner} has a key already");
    }
    Ok(k)
}

/// `owner`'s data key, unwrapped; none once shredded.
pub fn key(c: &Connection, m: &Master, owner: &str) -> anyhow::Result<Option<Key>> {
    let row: Option<(String, String)> = c
        .query_row("SELECT wrapped, master_id FROM keys WHERE owner = ?1", [owner], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((wrapped, id)) = row else { return Ok(None) };
    if id != m.id {
        anyhow::bail!("{owner}: its key was wrapped by another master key ({id}; this one is {})", m.id);
    }
    m.unwrap(owner, &wrapped).map(Some)
}

/// Destroy `owner`'s data key: what it encrypted can no longer be read.
pub fn shred(c: &Connection, owner: &str) -> anyhow::Result<bool> {
    Ok(c.execute("DELETE FROM keys WHERE owner = ?1", [owner])? > 0)
}

/// Wrap every data key, and seal every connected database's password, again with a new master
/// key (`medorama keys rotate`); the data are not touched.  Returns how many were rewrapped.
pub fn rotate(c: &Connection, old: &Master, new: &Master) -> anyhow::Result<usize> {
    let rows: Vec<(String, String, String)> = c
        .prepare("SELECT owner, wrapped, master_id FROM keys ORDER BY owner")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let tx = c.unchecked_transaction()?;
    for (owner, wrapped, id) in &rows {
        if *id == new.id {
            continue;
        }
        if *id != old.id {
            anyhow::bail!("{owner}: wrapped by a master key that is neither the old one nor the new one ({id})");
        }
        let k = old.unwrap(owner, wrapped)?;
        c.execute(
            "UPDATE keys SET wrapped = ?1, master_id = ?2 WHERE owner = ?3",
            params![new.wrap(owner, &k)?, new.id, owner],
        )?;
    }
    // the connected databases' passwords, sealed by the master key too (sealed by the new one
    // already: a rotation interrupted and run again)
    let passwords: Vec<(i64, String)> = c
        .prepare("SELECT id, password_wrapped FROM databases WHERE password_wrapped <> ''")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (id, wrapped) in &passwords {
        let owner = format!("password:{id}");
        if new.open(&owner, wrapped).is_ok() {
            continue;
        }
        let secret = old.open(&owner, wrapped).map_err(|e| anyhow::anyhow!("database {id}'s password: {e}"))?;
        c.execute("UPDATE databases SET password_wrapped = ?1 WHERE id = ?2", params![new.seal(&owner, &secret)?, id])?;
    }
    tx.commit()?;
    Ok(rows.len() + passwords.len())
}

// ------------------------------------------------------------------ folders and files

/// The workspace folder of a connected database: `<workspace>/db-NNNN`.
pub fn database_dir(cfg: &Config, id: i64) -> PathBuf {
    cfg.workspace_dir.join(format!("db-{id:04}"))
}

/// The workspace folder of a derived database with sources in more than one database.
pub fn derived_dir(cfg: &Config, id: i64) -> PathBuf {
    cfg.workspace_dir.join(format!("d-{id:04}"))
}

/// A cohort's file: `<workspace>/cohorts/c-NNNN.sqlite`.
pub fn cohort_file(cfg: &Config, id: i64) -> PathBuf {
    cfg.workspace_dir.join("cohorts").join(format!("c-{id:04}.sqlite"))
}

pub fn database_owner(id: i64) -> String {
    format!("db:{id}")
}

pub fn cohort_owner(id: i64) -> String {
    format!("cohort:{id}")
}

pub fn derived_owner(id: i64) -> String {
    format!("derived:{id}")
}

/// Make a folder of the workspace, for Medorama's account only.
pub fn make_dir(d: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(d).map_err(|e| anyhow::anyhow!("{}: {e}", d.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Open (or create) an encrypted SQLite file of the workspace with its owner's key.  A file
/// that does not open with the key is an error, never a new empty database.
pub fn open(path: &Path, k: &Key, create: bool) -> anyhow::Result<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    if !create && !path.is_file() {
        anyhow::bail!("{}: no such file", path.display());
    }
    if let Some(d) = path.parent() {
        if create {
            make_dir(d)?;
        }
    }
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let c =
        rusqlite::Connection::open_with_flags(path, flags).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    // a raw key: no key derivation, the key is random already
    let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", k.hex().as_str()));
    c.execute_batch(&pragma)?;
    c.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).map_err(|_| {
        anyhow::anyhow!("{}: does not open with its key (another key, or not an encrypted file)", path.display())
    })?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute_batch("PRAGMA journal_mode = WAL;")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{suffix}", path.display()));
            if p.exists() {
                let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    Ok(c)
}

/// Open (or create) a file of a connected database's workspace folder with its key.  The key
/// is made when the database is connected; without it (the database removed) nothing opens.
pub fn open_database_file(
    cfg: &Config,
    c: &Connection,
    database: i64,
    file: &str,
) -> anyhow::Result<rusqlite::Connection> {
    let owner = database_owner(database);
    let k = key(c, loaded()?, &owner)?.ok_or_else(|| anyhow::anyhow!("database {database} has no key (removed)"))?;
    open(&database_dir(cfg, database).join(file), &k, true)
}

/// Open (or create) a cohort's file with its key.
pub fn open_cohort_file(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<rusqlite::Connection> {
    let owner = cohort_owner(cohort);
    let k = match key(c, loaded()?, &owner)? {
        Some(k) => k,
        None => create_key(c, loaded()?, &owner)?,
    };
    open(&cohort_file(cfg, cohort), &k, true)
}

// ------------------------------------------------------------------ index stores

/// The owner of the viewer's own index store (recordings it indexes that no database's store
/// holds yet).
pub const VIEWER_OWNER: &str = "viewer";

/// The index store of a recording database (index format 3: `<workspace>/db-NNNN/index.sqlite`,
/// encrypted with the database's key), opened once and kept.
pub fn index_store(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Arc<Store>> {
    let owner = database_owner(database);
    let k = key(c, loaded()?, &owner)?.ok_or_else(|| anyhow::anyhow!("database {database} has no key (removed)"))?;
    store_at(&database_dir(cfg, database).join("index.sqlite"), &k)
}

/// The viewer's own index store (`<workspace>/viewer/index.sqlite`, with a key of its own).
pub fn viewer_store(cfg: &Config, c: &Connection) -> anyhow::Result<(PathBuf, Key)> {
    let k = match key(c, loaded()?, VIEWER_OWNER)? {
        Some(k) => k,
        None => create_key(c, loaded()?, VIEWER_OWNER)?,
    };
    Ok((cfg.workspace_dir.join("viewer").join("index.sqlite"), k))
}

fn stores() -> &'static Mutex<HashMap<PathBuf, Arc<Store>>> {
    static STORES: OnceLock<Mutex<HashMap<PathBuf, Arc<Store>>>> = OnceLock::new();
    STORES.get_or_init(Default::default)
}

fn store_at(path: &Path, k: &Key) -> anyhow::Result<Arc<Store>> {
    let mut open = stores().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = open.get(path) {
        return Ok(s.clone());
    }
    let s = Arc::new(Store::open(path, Some(&k.0)).map_err(|e| anyhow::anyhow!("{e}"))?);
    open.insert(path.to_path_buf(), s.clone());
    Ok(s)
}

/// Close the index stores kept open under `dir` (before the folder is removed).
pub fn close_stores(dir: &Path) {
    stores().lock().unwrap_or_else(|e| e.into_inner()).retain(|p, _| !p.starts_with(dir));
}

/// A key's 64 hex digits, for the viewer's pipe (never written anywhere).
pub fn key_hex(k: &Key) -> String {
    k.hex().to_string()
}

/// The owner of the gate's secret among the wrapped keys (gate.rs `secret`).
pub const GATE_OWNER: &str = "gate";

/// Keep `owner`'s key wrapped, unless it has one already (another connection made it first):
/// the key it has now.
pub fn keep_key(c: &Connection, m: &Master, owner: &str, k: &Key) -> anyhow::Result<Key> {
    c.execute(
        "INSERT INTO keys (owner, wrapped, master_id, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![owner, m.wrap(owner, k)?, m.id, now()],
    )?;
    key(c, m, owner)?.ok_or_else(|| anyhow::anyhow!("{owner}: its key could not be kept"))
}

/// Remove a folder of the workspace and everything in it.  Refuses anything outside the
/// workspace, and a link.
pub fn remove_dir(cfg: &Config, d: &Path) -> anyhow::Result<()> {
    close_stores(d);
    remove_inside(cfg, d, true)
}

/// Remove a file of the workspace (and its `-wal`, `-shm`).
pub fn remove_file(cfg: &Config, f: &Path) -> anyhow::Result<()> {
    for suffix in ["-wal", "-shm", ""] {
        remove_inside(cfg, &PathBuf::from(format!("{}{suffix}", f.display())), false)?;
    }
    Ok(())
}

fn remove_inside(cfg: &Config, p: &Path, dir: bool) -> anyhow::Result<()> {
    let meta = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(anyhow::anyhow!("{}: {e}", p.display())),
    };
    let ws = crate::config::real(&cfg.workspace_dir);
    let parent = p.parent().map(crate::config::real).unwrap_or_default();
    if !crate::config::within(&parent, &ws) || meta.file_type().is_symlink() {
        anyhow::bail!("{}: not a file of the workspace {}", p.display(), cfg.workspace_dir.display());
    }
    if dir && meta.is_dir() {
        std::fs::remove_dir_all(p)?;
    } else if !meta.is_dir() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_keys_unwrap_for_their_owner_only() {
        let m = Master::new(Key::random().unwrap());
        let k = Key::random().unwrap();
        let w = m.wrap("db:1", &k).unwrap();
        assert_eq!(m.unwrap("db:1", &w).unwrap().0, k.0);
        assert!(m.unwrap("db:2", &w).is_err(), "moved to another owner");
        let other = Master::new(Key::random().unwrap());
        assert!(other.unwrap("db:1", &w).is_err(), "another master key");
        assert_ne!(m.id, other.id);
        let mut bad = w.clone();
        bad.replace_range(30..32, if &w[30..32] == "00" { "01" } else { "00" });
        assert!(m.unwrap("db:1", &bad).is_err(), "changed");
    }

    #[test]
    fn hex_keys() {
        assert!(Key::from_hex(&"ab".repeat(32)).is_some());
        assert!(Key::from_hex(&"ab".repeat(31)).is_none());
        assert!(Key::from_hex(&"zz".repeat(32)).is_none());
        let k = Key::random().unwrap();
        assert_eq!(Key::from_hex(&k.hex()).unwrap().0, k.0);
        assert_eq!(k.hex().len(), 64);
        assert_eq!(Key::from_bytes(&k.bytes()).unwrap().0, k.0);
        assert!(Key::from_bytes(&[1u8; 31]).is_none());
    }

    #[test]
    fn files_open_with_their_key_only() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.sqlite");
        let k = Key::random().unwrap();
        {
            let c = open(&f, &k, true).unwrap();
            c.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('secret value');").unwrap();
        }
        let bytes = std::fs::read(&f).unwrap();
        assert!(!bytes.windows(12).any(|w| w == b"secret value"), "the file is encrypted");
        assert!(!bytes.starts_with(b"SQLite format 3"));
        let c = open(&f, &k, false).unwrap();
        let v: String = c.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "secret value");
        drop(c);
        assert!(open(&f, &Key::random().unwrap(), false).is_err());
        assert!(open(&d.path().join("none.sqlite"), &k, false).is_err());
    }
}
