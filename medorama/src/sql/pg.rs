//! PostgreSQL: the client, its statement cache, TLS, and values both ways.
//!
//! The client is tokio-postgres, run on a small runtime of its own: each call waits for its
//! answer as a SQLite call does, from any thread (an async task included), so the rest of
//! Medorama calls both databases the same way.
//!
//! Values are SQLite's five kinds (`Value`).  A parameter is written as the type PostgreSQL
//! inferred for it (an integer into a `double precision` or a `boolean`, a number into
//! `numeric`, text into `jsonb` …); a column is read as the nearest kind: integers and booleans
//! as integers, `numeric` as a number, dates as `YYYY-MM-DD` text, `timestamptz` as Unix
//! seconds, `timestamp` (without a time zone: a local time) as `YYYY-MM-DD HH:MM:SS` text, read
//! in the zone of whoever reads it (a connected database's), `json` as text.
//!
//! Each connection has limits unless its URL sets them: 10 seconds to connect, TCP keepalives
//! (a server gone away is noticed), and 5 minutes for a statement.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::future::Future;
use std::sync::{Arc, OnceLock};

use bytes::{BufMut, BytesMut};
use rustls::pki_types::pem::PemObject;
use tokio_postgres::types::{FromSql, IsNull, Kind, ToSql, Type};

use super::{Error, Result, Value};

type BoxError = Box<dyn StdError + Sync + Send>;

/// How connections are encrypted: the certificate authorities trusted (the system's, and
/// `ca_file` when given).  TLS is used as `sslmode` in the URL says; without one, it is
/// required for a server on another host (and `prefer`red on this one).  When used, the
/// server's certificate and name are always verified.
#[derive(Debug, Clone, Default)]
pub struct Tls {
    pub ca_file: Option<std::path::PathBuf>,
}

impl Tls {
    fn connector(&self) -> anyhow::Result<tokio_postgres_rustls::MakeRustlsConnect> {
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(c);
        }
        if let Some(f) = &self.ca_file {
            let pem = std::fs::read(f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
            for c in rustls::pki_types::CertificateDer::pem_slice_iter(&pem) {
                roots.add(c.map_err(|e| anyhow::anyhow!("{}: {e:?}", f.display()))?)?;
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(tokio_postgres_rustls::MakeRustlsConnect::new(cfg))
    }
}

/// The runtime the PostgreSQL connections run on.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("medorama-postgres")
            .enable_all()
            .build()
            .expect("the PostgreSQL runtime")
    })
}

/// Run `f` on that runtime and wait for it.
fn wait<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    runtime().spawn(async move {
        let _ = tx.send(f.await);
    });
    rx.recv().expect("the PostgreSQL runtime stopped")
}

pub type Prepared = tokio_postgres::Statement;

pub struct Client {
    c: Arc<tokio_postgres::Client>,
    cache: RefCell<HashMap<String, Prepared>>,
    pub(super) in_transaction: Cell<bool>,
    /// How it was opened: another connection to the same database ([`Client::again`]).
    opened: (String, Option<String>, Tls, String),
}

/// A statement's time limit, unless the URL or the caller sets one.
const STATEMENT_TIMEOUT_MS: u64 = 5 * 60 * 1000;

impl Client {
    pub fn connect(url: &str, password: Option<&str>, tls: &Tls, options: &str) -> anyhow::Result<Client> {
        let opened = (url.to_string(), password.map(str::to_string), tls.clone(), options.to_string());
        let mut cfg: tokio_postgres::Config =
            url.parse().map_err(|e| anyhow::anyhow!("the database URL is not valid: {e}"))?;
        if let Some(p) = password {
            cfg.password(p);
        }
        let mut all = format!("{} {options}", cfg.get_options().unwrap_or_default());
        if !all.contains("statement_timeout") {
            all = format!("{all} -c statement_timeout={STATEMENT_TIMEOUT_MS}");
        }
        cfg.options(all.trim());
        // a server that does not answer, or is gone, is an error, not a wait without end
        let lower = url.to_lowercase();
        if cfg.get_connect_timeout().is_none() {
            cfg.connect_timeout(std::time::Duration::from_secs(10));
        }
        if !lower.contains("keepalives") {
            cfg.keepalives(true);
            cfg.keepalives_idle(std::time::Duration::from_secs(60));
        }
        // TLS to any server not on this host, unless the URL says otherwise (libpq's default,
        // `prefer`, would fall back to plain text)
        if !url.to_lowercase().contains("sslmode=") && !cfg.get_hosts().iter().all(local) {
            cfg.ssl_mode(tokio_postgres::config::SslMode::Require);
        }
        if cfg.get_application_name().is_none() {
            cfg.application_name("medorama");
        }
        let tls = tls.connector()?;
        let c = wait(async move {
            let (client, conn) = cfg.connect(tls).await?;
            tokio::spawn(async move {
                let _ = conn.await;
            });
            Ok::<_, tokio_postgres::Error>(client)
        })
        .map_err(|e| anyhow::anyhow!("PostgreSQL: {}", Error::from(e)))?;
        Ok(Client { c: Arc::new(c), cache: RefCell::new(HashMap::new()), in_transaction: Cell::new(false), opened })
    }

    /// The URL it was opened with (no password: that is apart).
    pub fn url(&self) -> &str {
        &self.opened.0
    }

    /// Another connection to the same database, opened the same way.
    pub fn again(&self) -> anyhow::Result<Client> {
        let (url, password, tls, options) = &self.opened;
        Client::connect(url, password.as_deref(), tls, options)
    }

    /// A statement in SQLite's dialect as Medorama writes it ([`super::translate`]d).
    pub fn prepare(&self, sql: &str) -> Result<Prepared> {
        self.prepare_as(sql, true)
    }

    /// A statement in PostgreSQL's own dialect, as written (a connected database's views).
    pub fn prepare_native(&self, sql: &str) -> Result<Prepared> {
        self.prepare_as(sql, false)
    }

    fn prepare_as(&self, sql: &str, translated: bool) -> Result<Prepared> {
        let key = if translated { sql.to_string() } else { format!("\u{0}native\u{0}{sql}") };
        if let Some(s) = self.cache.borrow().get(&key) {
            return Ok(s.clone());
        }
        let (c, t) = (self.c.clone(), if translated { super::translate(sql) } else { sql.to_string() });
        let s = match wait({
            let t = t.clone();
            async move { c.prepare(&t).await }
        }) {
            Ok(s) => s,
            Err(e) => {
                // MEDORAMA_DEBUG_SQL=1: show the statements PostgreSQL refuses (never their values)
                if std::env::var_os("MEDORAMA_DEBUG_SQL").is_some() {
                    let e = Error::from(e);
                    eprintln!("PostgreSQL refused: {e}\n  {t}");
                    return Err(e);
                }
                return Err(e.into());
            }
        };
        let mut cache = self.cache.borrow_mut();
        if cache.len() > 512 {
            cache.clear();
        }
        cache.insert(key, s.clone());
        Ok(s)
    }

    pub fn is_closed(&self) -> bool {
        self.c.is_closed()
    }

    pub fn batch(&self, sql: &str) -> Result<()> {
        self.command(sql)?;
        // a schema change may have changed what cached statements return
        self.cache.borrow_mut().clear();
        Ok(())
    }

    /// A statement without parameters that changes no schema (a lock, a transaction's end).
    pub fn command(&self, sql: &str) -> Result<()> {
        let (c, sql) = (self.c.clone(), sql.to_string());
        wait(async move { c.batch_execute(&sql).await })?;
        Ok(())
    }

    pub fn execute(&self, s: &Prepared, v: &[Value]) -> Result<usize> {
        let (c, s, p) = (self.c.clone(), s.clone(), params(v));
        let n = wait(async move { c.execute(&s, &refs(&p)).await })?;
        Ok(n as usize)
    }

    pub fn query(&self, s: &Prepared, v: &[Value]) -> Result<Vec<Vec<Value>>> {
        let (c, s, p) = (self.c.clone(), s.clone(), params(v));
        let rows = wait(async move { c.query(&s, &refs(&p)).await })?;
        rows.iter()
            .map(|r| (0..r.len()).map(|i| r.try_get::<_, Read>(i).map(|x| x.0).map_err(Error::from)).collect())
            .collect()
    }
}

/// A host on this machine: a Unix socket, or a loopback address.
fn local(h: &tokio_postgres::config::Host) -> bool {
    match h {
        tokio_postgres::config::Host::Tcp(name) => {
            name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback())
        }
        #[cfg(unix)]
        tokio_postgres::config::Host::Unix(_) => true,
    }
}

fn params(v: &[Value]) -> Vec<Param> {
    v.iter().cloned().map(Param).collect()
}

fn refs(p: &[Param]) -> Vec<&(dyn ToSql + Sync)> {
    p.iter().map(|x| x as &(dyn ToSql + Sync)).collect()
}

// ------------------------------------------------------------------ reading

/// A column read as a `Value`.
struct Read(Value);

/// Days and microseconds in PostgreSQL's dates and timestamps count from 2000-01-01.
const PG_EPOCH: i64 = 946_684_800;

impl<'a> FromSql<'a> for Read {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> std::result::Result<Self, BoxError> {
        let v = match *ty {
            Type::BOOL => Value::Integer(bool::from_sql(ty, raw)? as i64),
            Type::INT2 => Value::Integer(i16::from_sql(ty, raw)? as i64),
            Type::INT4 => Value::Integer(i32::from_sql(ty, raw)? as i64),
            Type::INT8 => Value::Integer(i64::from_sql(ty, raw)?),
            Type::OID => Value::Integer(u32::from_sql(ty, raw)? as i64),
            Type::FLOAT4 => Value::Real(f32::from_sql(ty, raw)? as f64),
            Type::FLOAT8 => Value::Real(f64::from_sql(ty, raw)?),
            Type::NUMERIC => numeric_read(raw)?,
            Type::BYTEA => Value::Blob(raw.to_vec()),
            Type::JSONB => Value::Text(String::from_utf8(raw.get(1..).unwrap_or_default().to_vec())?),
            Type::DATE => {
                let d = i32::from_sql(ty, raw)? as i64;
                let t = chrono::DateTime::from_timestamp(PG_EPOCH + d * 86400, 0).ok_or("date out of range")?;
                Value::Text(t.format("%Y-%m-%d").to_string())
            }
            Type::TIMESTAMPTZ => {
                let us = i64::from_sql(ty, raw)?;
                Value::Real(PG_EPOCH as f64 + us as f64 / 1e6)
            }
            // a local time, of no zone: as text, for its reader to place in its zone
            Type::TIMESTAMP => {
                let us = i64::from_sql(ty, raw)?;
                let t = chrono::DateTime::from_timestamp_micros((PG_EPOCH * 1_000_000).saturating_add(us))
                    .ok_or("timestamp out of range")?
                    .naive_utc();
                Value::Text(if us % 1_000_000 == 0 {
                    t.format("%Y-%m-%d %H:%M:%S").to_string()
                } else {
                    t.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
                })
            }
            Type::UUID if raw.len() == 16 => {
                let h: String = raw.iter().map(|b| format!("{b:02x}")).collect();
                Value::Text(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
            }
            _ if text_like(ty) => Value::Text(std::str::from_utf8(raw)?.to_string()),
            _ => return Err(format!("columns of type {} are not read (cast them in the query)", ty.name()).into()),
        };
        Ok(Read(v))
    }
    fn from_sql_null(_: &Type) -> std::result::Result<Self, BoxError> {
        Ok(Read(Value::Null))
    }
    fn accepts(_: &Type) -> bool {
        true
    }
}

/// Types whose binary form is their text: text, varchar, char, name, json, unknown, citext.
fn text_like(ty: &Type) -> bool {
    matches!(*ty, Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::JSON | Type::UNKNOWN)
        || (matches!(ty.kind(), Kind::Simple) && ty.name() == "citext")
}

/// PostgreSQL's binary `numeric`: digit count, weight, sign, scale, then base-10000 digits.
fn numeric_read(raw: &[u8]) -> std::result::Result<Value, BoxError> {
    let word = |i: usize| -> std::result::Result<u16, BoxError> {
        raw.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or_else(|| "a numeric too short".into())
    };
    let (n, weight, sign, scale) = (word(0)? as usize, word(2)? as i16, word(4)?, word(6)?);
    if sign == 0xC000 {
        return Ok(Value::Real(f64::NAN));
    }
    // the digit group of weight p (10000^p), zero outside those given
    let (weight, n) = (weight as i32, n as i32);
    let group = |p: i32| -> std::result::Result<String, BoxError> {
        let i = weight - p;
        Ok(if (0..n).contains(&i) { format!("{:04}", word(8 + 2 * i as usize)?) } else { "0000".into() })
    };
    let mut int = String::new();
    for p in (0..=weight).rev() {
        int.push_str(&group(p)?);
    }
    let mut frac = String::new();
    for p in (weight - (n - 1)).min(0)..0 {
        frac.insert_str(0, &group(p)?);
    }
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let sign = if sign == 0x4000 { "-" } else { "" };
    if scale == 0 || frac.trim_end_matches('0').is_empty() {
        if let Ok(i) = format!("{sign}{int}").parse::<i64>() {
            if scale == 0 {
                return Ok(Value::Integer(i));
            }
        }
    }
    Ok(Value::Real(format!("{sign}{int}.{frac}0").parse()?))
}

/// A number as PostgreSQL's binary `numeric`, from its decimal text.
fn numeric_write(text: &str, out: &mut BytesMut) -> std::result::Result<(), BoxError> {
    let t = text.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    if int.is_empty() && frac.is_empty() || !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) {
        return Err(format!("{text:?} is not a number").into());
    }
    let int = int.trim_start_matches('0');
    // pad to whole groups of four digits on both sides of the point
    let ipad = (4 - int.len() % 4) % 4;
    let int = format!("{}{int}", "0".repeat(ipad));
    let fpad = (4 - frac.len() % 4) % 4;
    let frac_p = format!("{frac}{}", "0".repeat(fpad));
    let mut digits: Vec<u16> = int
        .as_bytes()
        .chunks(4)
        .chain(frac_p.as_bytes().chunks(4))
        .map(|c| std::str::from_utf8(c).unwrap().parse().unwrap())
        .collect();
    let mut weight = int.len() as i32 / 4 - 1;
    while digits.first() == Some(&0) {
        digits.remove(0);
        weight -= 1;
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.is_empty() {
        weight = 0;
    }
    out.put_u16(digits.len() as u16);
    out.put_i16(weight as i16);
    out.put_u16(if neg && !digits.is_empty() { 0x4000 } else { 0 });
    out.put_u16(frac.len() as u16);
    for d in digits {
        out.put_u16(d);
    }
    Ok(())
}

// ------------------------------------------------------------------ writing

/// A parameter, written as the type the statement expects.
#[derive(Debug)]
struct Param(Value);

impl ToSql for Param {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> std::result::Result<IsNull, BoxError> {
        let bad = || -> BoxError { format!("{:?} cannot be written as {}", self.0, ty.name()).into() };
        match &self.0 {
            Value::Null => return Ok(IsNull::Yes),
            Value::Integer(i) => match *ty {
                Type::BOOL => (*i != 0).to_sql(ty, out)?,
                Type::INT2 => i16::try_from(*i)?.to_sql(ty, out)?,
                Type::INT4 => i32::try_from(*i)?.to_sql(ty, out)?,
                Type::INT8 => i.to_sql(ty, out)?,
                Type::OID => u32::try_from(*i)?.to_sql(ty, out)?,
                Type::FLOAT4 => (*i as f32).to_sql(ty, out)?,
                Type::FLOAT8 => (*i as f64).to_sql(ty, out)?,
                Type::NUMERIC => {
                    numeric_write(&i.to_string(), out)?;
                    IsNull::No
                }
                Type::TIMESTAMP | Type::TIMESTAMPTZ => ((*i - PG_EPOCH) * 1_000_000).to_sql(&Type::INT8, out)?,
                _ if text_like(ty) => i.to_string().as_str().to_sql(&Type::TEXT, out)?,
                _ => return Err(bad()),
            },
            Value::Real(x) => match *ty {
                Type::FLOAT8 => x.to_sql(ty, out)?,
                Type::FLOAT4 => (*x as f32).to_sql(ty, out)?,
                Type::INT8 | Type::INT4 | Type::INT2 if x.fract() == 0.0 => {
                    return Param(Value::Integer(*x as i64)).to_sql(ty, out)
                }
                Type::NUMERIC if x.is_finite() => {
                    numeric_write(&x.to_string(), out)?;
                    IsNull::No
                }
                Type::TIMESTAMP | Type::TIMESTAMPTZ => {
                    (((x - PG_EPOCH as f64) * 1e6).round() as i64).to_sql(&Type::INT8, out)?
                }
                _ if text_like(ty) => x.to_string().as_str().to_sql(&Type::TEXT, out)?,
                _ => return Err(bad()),
            },
            Value::Text(s) => match *ty {
                Type::JSONB => {
                    out.put_u8(1);
                    out.put_slice(s.as_bytes());
                    IsNull::No
                }
                Type::BYTEA => s.as_bytes().to_sql(ty, out)?,
                Type::INT8 | Type::INT4 | Type::INT2 | Type::OID => {
                    return Param(Value::Integer(s.trim().parse().map_err(|_| bad())?)).to_sql(ty, out)
                }
                Type::FLOAT8 | Type::FLOAT4 => {
                    return Param(Value::Real(s.trim().parse().map_err(|_| bad())?)).to_sql(ty, out)
                }
                Type::NUMERIC => {
                    numeric_write(s, out)?;
                    IsNull::No
                }
                Type::BOOL => matches!(s.trim().to_lowercase().as_str(), "1" | "t" | "true" | "yes").to_sql(ty, out)?,
                _ if text_like(ty) => s.as_str().to_sql(&Type::TEXT, out)?,
                _ => return Err(bad()),
            },
            Value::Blob(b) => match *ty {
                Type::BYTEA => b.as_slice().to_sql(ty, out)?,
                _ if text_like(ty) => std::str::from_utf8(b)?.to_sql(&Type::TEXT, out)?,
                _ => return Err(bad()),
            },
        };
        Ok(IsNull::No)
    }
    fn accepts(_: &Type) -> bool {
        true
    }
    tokio_postgres::types::to_sql_checked!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(text: &str) -> Value {
        let mut b = BytesMut::new();
        numeric_write(text, &mut b).unwrap();
        numeric_read(&b).unwrap()
    }

    #[test]
    fn local_hosts() {
        use tokio_postgres::config::Host;
        assert!(
            local(&Host::Tcp("localhost".into()))
                && local(&Host::Tcp("127.0.0.1".into()))
                && local(&Host::Tcp("::1".into()))
        );
        assert!(!local(&Host::Tcp("db.example.org".into())) && !local(&Host::Tcp("10.0.0.5".into())));
    }

    #[test]
    fn numerics_both_ways() {
        assert_eq!(round("0"), Value::Integer(0));
        assert_eq!(round("12345678"), Value::Integer(12345678));
        assert_eq!(round("-20000"), Value::Integer(-20000));
        assert_eq!(round("4.1"), Value::Real(4.1));
        assert_eq!(round("-0.00012"), Value::Real(-0.00012));
        assert_eq!(round("1700000000.25"), Value::Real(1700000000.25));
        assert!(numeric_write("1e5", &mut BytesMut::new()).is_err());
    }
}
