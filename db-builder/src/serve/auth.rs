//! The web interface's sign-in: one account, its name and Argon2id password hash in
//! `<home>/password` (`name:hash`, readable by its owner only), set with
//! `db-builder serve --set-password`; sessions are random tokens (their SHA-256 kept, in memory)
//! in an HttpOnly, SameSite=Strict cookie, ended after an idle time or 12 hours; failed sign-ins
//! are counted per address and in all, and refused for a minute beyond the limits.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The shortest password accepted.
pub const MIN_PASSWORD: usize = 10;
/// Failed sign-ins from one address within [`LOCK`] before it is refused for [`LOCK`].
const MAX_FAILS: u32 = 5;
/// Failed sign-ins from every address within [`LOCK`] before every sign-in is refused for it.
const MAX_ALL_FAILS: u32 = 30;
pub const LOCK: Duration = Duration::from_secs(60);
/// A session lasts at most this long, however active.
pub const SESSION_MAX: Duration = Duration::from_secs(12 * 3600);
/// The most sessions kept (the oldest end first).
const MAX_SESSIONS: usize = 100;

/// The password file in a home folder.
pub fn password_file(home: &Path) -> PathBuf {
    home.join("password")
}

/// The account: its name and its password's hash, if one was set.
pub fn read_account(home: &Path) -> anyhow::Result<Option<(String, String)>> {
    let f = password_file(home);
    let text = match std::fs::read_to_string(&f) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => anyhow::bail!("{}: {e}", f.display()),
    };
    let (name, hash) = text
        .trim()
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("{}: not a password file (name:hash)", f.display()))?;
    PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
    Ok(Some((name.to_string(), hash.to_string())))
}

/// A name the account may have: letters, digits and `. _ - @`, at most 64 bytes.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
}

/// Set the account's name and password (the home folder made, readable by its owner only).
pub fn set_password(home: &Path, name: &str, password: &str) -> anyhow::Result<()> {
    if !valid_name(name) {
        anyhow::bail!("{name:?}: a user name is letters, digits and . _ - @");
    }
    if password.chars().count() < MIN_PASSWORD {
        anyhow::bail!("the password is too short: use at least {MIN_PASSWORD} characters");
    }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow::anyhow!("random numbers: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    std::fs::create_dir_all(home)?;
    let tmp = home.join("password.tmp");
    std::fs::write(&tmp, format!("{name}:{hash}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, password_file(home))?;
    Ok(())
}

/// Whether `password` is the one of `hash`.
pub fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// A new random token (32 bytes, hex).
pub fn token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("system random number generator");
    crate::db::hex(&b)
}

fn digest(token: &str) -> String {
    crate::db::hex(&Sha256::digest(token.as_bytes()))
}

/// The sessions: by their token's hash, when made and when last used.
pub struct Sessions {
    map: Mutex<HashMap<String, (Instant, Instant)>>,
    idle: Duration,
}

impl Sessions {
    pub fn new(idle: Duration) -> Sessions {
        Sessions { map: Mutex::new(HashMap::new()), idle }
    }

    /// A new session: its token.
    pub fn start(&self) -> String {
        let t = token();
        let mut m = self.map.lock().unwrap();
        m.retain(|_, (made, last)| made.elapsed() < SESSION_MAX && last.elapsed() < self.idle);
        while m.len() >= MAX_SESSIONS {
            let Some(old) = m.iter().min_by_key(|(_, (made, _))| *made).map(|(k, _)| k.clone()) else { break };
            m.remove(&old);
        }
        m.insert(digest(&t), (Instant::now(), Instant::now()));
        t
    }

    /// Whether the token names a live session; `active`: the user's own request, which keeps
    /// it alive (not the page's polling of a task).
    pub fn check(&self, token: &str, active: bool) -> bool {
        let mut m = self.map.lock().unwrap();
        let k = digest(token);
        match m.get_mut(&k) {
            Some((made, last)) if made.elapsed() < SESSION_MAX && last.elapsed() < self.idle => {
                if active {
                    *last = Instant::now();
                }
                true
            }
            Some(_) => {
                m.remove(&k);
                false
            }
            None => false,
        }
    }

    pub fn end(&self, token: &str) {
        self.map.lock().unwrap().remove(&digest(token));
    }
}

/// Failed sign-ins, per address and in all, each with when the count began.
#[derive(Default)]
pub struct Fails {
    by_address: HashMap<Option<IpAddr>, (u32, Instant)>,
    all: Option<(u32, Instant)>,
}

impl Fails {
    /// An attempt from `ip`, counted before the password is checked (attempts made at once all
    /// count); false when the address, or every sign-in, is refused for now.
    pub fn reserve(&mut self, ip: Option<IpAddr>) -> bool {
        let fresh = |e: &mut (u32, Instant)| {
            if e.1.elapsed() >= LOCK {
                *e = (0, Instant::now());
            }
        };
        if self.by_address.len() > 10_000 {
            self.by_address.retain(|_, e| e.1.elapsed() < LOCK);
        }
        let all = self.all.get_or_insert((0, Instant::now()));
        fresh(all);
        let one = self.by_address.entry(ip).or_insert((0, Instant::now()));
        fresh(one);
        if all.0 >= MAX_ALL_FAILS || one.0 >= MAX_FAILS {
            return false;
        }
        all.0 += 1;
        one.0 += 1;
        true
    }

    /// A sign-in that succeeded: its attempt does not count.
    pub fn succeeded(&mut self, ip: Option<IpAddr>) {
        self.by_address.remove(&ip);
        if let Some(a) = self.all.as_mut() {
            a.0 = a.0.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_and_sessions() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_account(d.path()).unwrap().is_none());
        assert!(set_password(d.path(), "admin", "short").is_err());
        assert!(set_password(d.path(), "bad name", "long enough password").is_err());
        set_password(d.path(), "admin", "long enough password").unwrap();
        let (name, hash) = read_account(d.path()).unwrap().unwrap();
        assert_eq!(name, "admin");
        assert!(verify("long enough password", &hash) && !verify("wrong", &hash));
        let s = Sessions::new(Duration::from_secs(60));
        let t = s.start();
        assert!(s.check(&t, true) && !s.check("other", true));
        s.end(&t);
        assert!(!s.check(&t, true));
        let idle = Sessions::new(Duration::from_millis(0));
        let t = idle.start();
        assert!(!idle.check(&t, true), "idle: ended");
    }

    #[test]
    fn failures_are_limited() {
        let mut f = Fails::default();
        let a: Option<IpAddr> = Some("10.0.0.1".parse().unwrap());
        for _ in 0..MAX_FAILS {
            assert!(f.reserve(a));
        }
        assert!(!f.reserve(a), "the address is refused");
        assert!(f.reserve(Some("10.0.0.2".parse().unwrap())), "another is not");
    }
}
