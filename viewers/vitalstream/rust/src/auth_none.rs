//! A build without password support (`cargo build --no-default-features`): no password can be
//! set, the page never asks for one, and the viewer refuses to start where a password is
//! required (`require_password`, which Medorama sets for the viewer it runs).  The same
//! functions as the `auth` module of the default build, so the server is the same.

use std::path::Path;

pub const LOGIN_DAYS: u64 = 30;
/// Whether this build can protect the viewer with a password.
pub const SUPPORTED: bool = false;

#[derive(Debug, Clone)]
pub struct Auth;

const NONE: &str = "this viewer was built without password support";

pub fn load(_path: &Path) -> Option<Auth> {
    None
}

pub fn set_password(_path: &Path, _password: &str) -> std::io::Result<()> {
    Err(std::io::Error::other(NONE))
}

pub fn clear_password(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(NONE))
}

pub fn revoked(_path: &Path) -> std::collections::HashMap<String, u64> {
    Default::default()
}

pub fn save_revoked(_path: &Path, _all: &std::collections::HashMap<String, u64>) -> std::io::Result<()> {
    Err(std::io::Error::other(NONE))
}

pub fn check_password(_auth: &Auth, _password: &str) -> bool {
    false
}

pub fn make_cookie(_auth: &Auth) -> String {
    String::new()
}

pub fn cookie_ok(_auth: &Auth, _value: Option<&str>) -> bool {
    false
}
