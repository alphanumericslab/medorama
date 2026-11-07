//! Users, roles and sign-in sessions.
//!
//! | Role | May |
//! |---|---|
//! | `viewer` | search and look at encounters and recordings, keep a history |
//! | `analyst` | + run actions (indexing and the others) |
//! | `admin` | + add and remove entries (recordings, linkage, EHR data), manage users, read the audit log |
//!
//! Separately from the role, the PHI permission lets a user leave Privacy Mode: see and search
//! identifiers (names, MRNs, dates of birth, account and encounter numbers, real dates).
//! Without it, every session of the user is in Privacy Mode (docs/security.md).  Passwords are
//! stored as Argon2id hashes; a session is a random token whose SHA-256 is stored.

use crate::db::now;
use crate::sql::{params, Connection, OptionalExtension};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MIN_PASSWORD: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Analyst,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "analyst" => Some(Role::Analyst),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Analyst => "analyst",
            Role::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    /// Sees identifiers in this session (the PHI permission, and Privacy Mode off).
    pub phi: bool,
    pub disabled: bool,
    /// May leave Privacy Mode: the PHI permission, whatever this session's mode.
    #[serde(default)]
    pub phi_allowed: bool,
    /// This session is in Privacy Mode (docs/security.md, "Privacy Mode"): its answers pass
    /// the privacy gate (gate.rs).
    #[serde(default)]
    pub privacy: bool,
    /// In Privacy Mode, whether the admin lets the text of notes through (de-identified as far
    /// as the gate can, and flagged: free text may still name the patient).
    #[serde(default)]
    pub privacy_notes: bool,
}

impl User {
    /// Add or remove entries, manage users.
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    /// May read the text of notes: with the PHI permission, or in Privacy Mode when its settings
    /// let notes through.
    pub fn reads_notes(&self) -> bool {
        self.phi || (self.privacy && self.privacy_notes)
    }

    /// The same user in Privacy Mode: no identifiers, whatever their permission.
    pub fn in_privacy_mode(&self, notes: bool) -> User {
        User { phi: false, privacy: true, privacy_notes: notes, ..self.clone() }
    }

    /// Run actions.
    pub fn can_run(&self) -> bool {
        matches!(self.role, Role::Admin | Role::Analyst)
    }

    /// Placeholder for command-line use: the person at the terminal, with the rights of the
    /// operating-system account that can open the database.
    pub fn local() -> User {
        User {
            id: 0,
            username: format!(
                "local:{}",
                std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default()
            ),
            display_name: "command line".into(),
            role: Role::Admin,
            phi: true,
            disabled: false,
            phi_allowed: true,
            privacy: false,
            privacy_notes: false,
        }
    }
}

fn row_user(r: &crate::sql::Row) -> crate::sql::Result<User> {
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        display_name: r.get(2)?,
        role: Role::parse(&r.get::<_, String>(3)?).unwrap_or(Role::Viewer),
        phi: r.get::<_, i64>(4)? != 0,
        disabled: r.get::<_, i64>(5)? != 0,
        phi_allowed: r.get::<_, i64>(4)? != 0,
        privacy: false,
        privacy_notes: false,
    })
}

const USER_COLS: &str = "id, username, display_name, role, phi, disabled";

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow::anyhow!("random numbers: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("{e}"))?.to_string())
}

fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

pub fn check_new_password(password: &str) -> anyhow::Result<()> {
    if password.chars().count() < MIN_PASSWORD {
        anyhow::bail!("the password is too short: use at least {MIN_PASSWORD} characters");
    }
    Ok(())
}

/// A name an account may have: letters, digits and `. _ - @`, at most 64 bytes.
pub fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
}

pub fn add(
    c: &Connection,
    username: &str,
    display: &str,
    role: Role,
    phi: bool,
    password: &str,
) -> anyhow::Result<i64> {
    if !valid_username(username) {
        anyhow::bail!("user names: letters, digits and . _ - @ (at most 64)");
    }
    check_new_password(password)?;
    if get_by_name(c, username)?.is_some() {
        anyhow::bail!("user {username} exists");
    }
    Ok(c.insert(
        "INSERT INTO users (username, display_name, role, phi, password_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![username, display, role.as_str(), phi as i64, hash_password(password)?, now()],
    )?)
}

pub fn get(c: &Connection, id: i64) -> anyhow::Result<Option<User>> {
    Ok(c.query_row(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"), [id], row_user).optional()?)
}

pub fn get_by_name(c: &Connection, username: &str) -> anyhow::Result<Option<User>> {
    Ok(c.query_row(&format!("SELECT {USER_COLS} FROM users WHERE username = ?1"), [username], row_user).optional()?)
}

/// The enabled account a sign-in names (an authenticating proxy's header), ignoring case; none
/// when no account, or more than one, has that name.
pub fn get_by_login(c: &Connection, name: &str) -> anyhow::Result<Option<User>> {
    let mut st = c.prepare("SELECT id FROM users WHERE lower(username) = lower(?1) AND disabled = 0 LIMIT 2")?;
    let ids: Vec<i64> = st.query_map([name], |r| r.get(0))?.collect::<Result<_, _>>()?;
    match ids.as_slice() {
        [id] => get(c, *id),
        _ => Ok(None),
    }
}

pub fn list(c: &Connection) -> anyhow::Result<Vec<User>> {
    let mut st = c.prepare(&format!("SELECT {USER_COLS} FROM users ORDER BY username"))?;
    let v = st.query_map([], row_user)?.collect::<Result<_, _>>()?;
    Ok(v)
}

pub fn count_admins(c: &Connection) -> anyhow::Result<i64> {
    Ok(c.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |r| r.get(0))?)
}

/// The user, if the name and password match an enabled account.
pub fn authenticate(c: &Connection, username: &str, password: &str) -> anyhow::Result<Option<User>> {
    let row: Option<(String, i64)> = c
        .query_row("SELECT password_hash, id FROM users WHERE username = ?1 AND disabled = 0", [username], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    match row {
        Some((hash, id)) if verify(password, &hash) => get(c, id),
        Some(_) => Ok(None),
        None => {
            // same work as a real check, so timing does not reveal which names exist
            static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let _ = verify(password, DUMMY.get_or_init(|| hash_password("not a password").unwrap_or_default()));
            Ok(None)
        }
    }
}

pub struct Changes<'a> {
    pub display_name: Option<&'a str>,
    pub role: Option<Role>,
    pub phi: Option<bool>,
    pub disabled: Option<bool>,
    pub password: Option<&'a str>,
}

/// Change a user; refuses to leave the catalog without an enabled admin.
pub fn update(c: &mut Connection, id: i64, ch: &Changes) -> anyhow::Result<()> {
    let Some(u) = get(c, id)? else { anyhow::bail!("no such user") };
    let tx = c.transaction()?;
    if let Some(d) = ch.display_name {
        tx.execute("UPDATE users SET display_name = ?1 WHERE id = ?2", params![d, id])?;
    }
    if let Some(r) = ch.role {
        tx.execute("UPDATE users SET role = ?1 WHERE id = ?2", params![r.as_str(), id])?;
    }
    if let Some(p) = ch.phi {
        tx.execute("UPDATE users SET phi = ?1 WHERE id = ?2", params![p as i64, id])?;
    }
    if let Some(d) = ch.disabled {
        tx.execute("UPDATE users SET disabled = ?1 WHERE id = ?2", params![d as i64, id])?;
        if d {
            tx.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
        }
    }
    if let Some(pw) = ch.password {
        check_new_password(pw)?;
        tx.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![hash_password(pw)?, id])?;
        tx.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
    }
    let admins: i64 =
        tx.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |r| r.get(0))?;
    if u.is_admin() && admins == 0 {
        anyhow::bail!("the catalog needs at least one enabled admin");
    }
    tx.commit()?;
    Ok(())
}

pub fn delete(c: &mut Connection, id: i64) -> anyhow::Result<()> {
    let tx = c.transaction()?;
    tx.execute("DELETE FROM users WHERE id = ?1", [id])?;
    let admins: i64 =
        tx.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |r| r.get(0))?;
    if admins == 0 {
        anyhow::bail!("the catalog needs at least one enabled admin");
    }
    tx.commit()?;
    Ok(())
}

/// A password nobody knows, for an account that signs in through the institution's sign-in
/// (`sign_in = "proxy"`, signin.rs), where Medorama's passwords are not used.
pub fn unusable_password() -> anyhow::Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("random numbers: {e}"))?;
    Ok(signal_core::text::hex(&b))
}

fn token_hash(token: &str) -> String {
    signal_core::text::hex(&Sha256::digest(token.as_bytes()))
}

/// A new session for the user; returns the token for the cookie.
pub fn new_session(c: &Connection, user: &User, hours: u64) -> anyhow::Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("random numbers: {e}"))?;
    let token = signal_core::text::hex(&b);
    let t = now();
    c.execute("DELETE FROM sessions WHERE expires_at < ?1", [t])?;
    c.execute(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![token_hash(&token), user.id, t, t + hours as f64 * 3600.0],
    )?;
    Ok(token)
}

/// The user of a session, if it is still valid: not expired (`session_hours`), and used within
/// `idle_s` seconds (0: no limit), else it ends.  `active`: this request is the user's (not the
/// page's own checks for new data), and counts as use.
pub fn session_user(c: &Connection, token: &str, idle_s: u64, active: bool) -> anyhow::Result<Option<User>> {
    Ok(session(c, token, idle_s, active)?.map(|(u, _)| u))
}

/// [`session_user`], with the session's Privacy Mode choice (`Some(true)`: on, `Some(false)`:
/// off; `None`: none made, the user's default).  A new session has none.
pub fn session(c: &Connection, token: &str, idle_s: u64, active: bool) -> anyhow::Result<Option<(User, Option<bool>)>> {
    if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let hash = token_hash(token);
    let t = now();
    let found: Option<(i64, f64, Option<String>)> = c
        .query_row(
            "SELECT s.user_id, COALESCE(s.last_seen, s.created_at), s.privacy FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0",
            params![hash, t],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, seen, privacy)) = found else { return Ok(None) };
    let privacy = match privacy.as_deref() {
        Some("on") => Some(true),
        Some("off") => Some(false),
        _ => None,
    };
    if idle_s > 0 && t - seen > idle_s as f64 {
        c.execute("DELETE FROM sessions WHERE token_hash = ?1", [&hash])?;
        return Ok(None);
    }
    // written at most once a minute: reads stay reads; and never waited for (while an import
    // writes, the next request writes it)
    if active && t - seen > 60.0 {
        let _ = c.without_waiting(|c| {
            c.execute("UPDATE sessions SET last_seen = ?1 WHERE token_hash = ?2", params![t, hash])
        });
    }
    Ok(get(c, id)?.map(|u| (u, privacy)))
}

/// Records a session's Privacy Mode choice (signin.rs `set_privacy`); false: no such session.
pub fn set_session_privacy(c: &Connection, token: &str, on: bool) -> anyhow::Result<bool> {
    let n = c.execute(
        "UPDATE sessions SET privacy = ?1 WHERE token_hash = ?2",
        params![if on { "on" } else { "off" }, token_hash(token)],
    )?;
    Ok(n == 1)
}

/// Ends every session of a user (signed out everywhere); returns how many.
pub fn end_sessions(c: &Connection, user: i64) -> anyhow::Result<usize> {
    Ok(c.execute("DELETE FROM sessions WHERE user_id = ?1", [user])?)
}

pub fn end_session(c: &Connection, token: &str) -> anyhow::Result<()> {
    c.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash(token)])?;
    Ok(())
}
