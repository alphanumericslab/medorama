//! Optional password protection, with the same scheme and file as the Python viewer: a
//! salted scrypt hash (N = 2^14, r = 8, p = 1, 32 bytes) and a random signing key in an auth
//! file readable only by its owner (default `~/.config/vitalstream/auth.json`).  The login is
//! a cookie `<expiry>.<HMAC-SHA256(key, expiry)>` valid for [`LOGIN_DAYS`].  A password set
//! with either viewer works for both.  The sign-ins ended by `logout` are kept in the auth file
//! (`revoked`: `{signature: expiry}`) until they would have expired, so that a copy of a cookie
//! stops working.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use signal_core::dict;
use signal_core::text::hex;
use signal_core::value::Value;
use std::path::Path;

pub const LOGIN_DAYS: u64 = 30;
/// Whether this build can protect the viewer with a password (a build without the `password`
/// feature cannot: `auth_none.rs`).
pub const SUPPORTED: bool = true;

#[derive(Debug, Clone)]
pub struct Auth {
    salt: Vec<u8>,
    hash: String,
    secret: Vec<u8>,
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() & 1 == 1 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn scrypt_hex(password: &str, salt: &[u8]) -> String {
    let params = scrypt::Params::new(14, 8, 1, 32).expect("valid scrypt parameters");
    let mut out = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut out).expect("scrypt output length");
    hex(&out)
}

/// The auth file's contents, or None when no password is set.
pub fn load(path: &Path) -> Option<Auth> {
    let v = Value::from_json(&std::fs::read_to_string(path).ok()?).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let (hash, secret) = (s("hash")?, s("secret")?);
    if hash.is_empty() || secret.is_empty() {
        return None;
    }
    Some(Auth { salt: unhex(&s("salt")?)?, hash, secret: unhex(&secret)? })
}

fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("system random number generator");
    b
}

/// Set (or change) the password; a new signing key ends earlier logins.
pub fn set_password(path: &Path, password: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let salt = random(16);
    let data = Value::Dict(dict! {
        "scheme" => "scrypt", "salt" => hex(&salt), "hash" => scrypt_hex(password, &salt), "secret" => hex(&random(32)),
    });
    write_private(path, &data.to_json())
}

pub fn clear_password(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The sign-ins ended by `logout` that have not expired (signature -> expiry).
pub fn revoked(path: &Path) -> std::collections::HashMap<String, u64> {
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|t| Value::from_json(&t).ok()) else {
        return Default::default();
    };
    let now = now();
    match v.get("revoked") {
        Some(Value::Dict(d)) => d
            .iter()
            .filter_map(|(k, e)| Some((k.clone(), u64::try_from(e.as_int()?).ok()?)))
            .filter(|(_, e)| *e > now)
            .collect(),
        _ => Default::default(),
    }
}

/// Write the sign-ins ended by `logout` into the auth file (the expired ones left out), its
/// other keys unchanged.
pub fn save_revoked(path: &Path, all: &std::collections::HashMap<String, u64>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let Ok(Value::Dict(mut d)) = Value::from_json(&text) else { return Err(std::io::Error::other("not an auth file")) };
    let now = now();
    let mut kept: Vec<(&String, &u64)> = all.iter().filter(|(_, e)| **e > now).collect();
    kept.sort();
    d.insert(
        "revoked".into(),
        Value::Dict(kept.into_iter().map(|(k, e)| (k.clone(), Value::Int(*e as i128))).collect()),
    );
    write_private(path, &Value::Dict(d).to_json())
}

/// Write `text` to `path` readable by its owner only, replacing it at once.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn eq_ct(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn check_password(auth: &Auth, password: &str) -> bool {
    eq_ct(scrypt_hex(password, &auth.salt).as_bytes(), auth.hash.as_bytes())
}

fn sign(auth: &Auth, expires: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(&auth.secret).expect("HMAC accepts any key length");
    mac.update(expires.to_string().as_bytes());
    hex(&mac.finalize().into_bytes())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn make_cookie(auth: &Auth) -> String {
    let expires = now() + LOGIN_DAYS * 86400;
    format!("{expires}.{}", sign(auth, expires))
}

pub fn cookie_ok(auth: &Auth, value: Option<&str>) -> bool {
    let Some((e, sig)) = value.and_then(|v| v.split_once('.')) else { return false };
    let Ok(expires) = e.parse::<u64>() else { return false };
    expires > now() && eq_ct(sig.as_bytes(), sign(auth, expires).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_and_cookie() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("auth.json");
        assert!(load(&p).is_none());
        set_password(&p, "correct horse").unwrap();
        let a = load(&p).unwrap();
        assert!(check_password(&a, "correct horse"));
        assert!(!check_password(&a, "wrong"));
        let c = make_cookie(&a);
        assert!(cookie_ok(&a, Some(&c)));
        assert!(!cookie_ok(&a, Some("123.abc")));
        assert!(!cookie_ok(&a, None));
        // a sign-in ended by logout stays ended (in the file), the password unchanged
        let sig = c.split_once('.').unwrap().1.to_string();
        save_revoked(&p, &[(sig.clone(), now() + 60), ("old".into(), 1)].into_iter().collect()).unwrap();
        assert_eq!(revoked(&p).into_keys().collect::<Vec<_>>(), [sig]);
        assert!(check_password(&load(&p).unwrap(), "correct horse"));
        clear_password(&p).unwrap();
        assert!(load(&p).is_none());
    }
}
