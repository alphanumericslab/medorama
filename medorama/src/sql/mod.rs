//! The database interface: one API over SQLite and PostgreSQL (docs/design/sources.md, "The
//! control store").
//!
//! The rest of Medorama writes SQL once, in SQLite's dialect kept to what both databases
//! understand, and calls it through the subset of rusqlite's API it has always used
//! (`Connection`, `Statement`, `Row`, `params!`, `OptionalExtension`).  On SQLite the calls go
//! straight to rusqlite.  On PostgreSQL each statement is translated once (`dialect.rs`:
//! `?1` → `$1`, `INSERT OR IGNORE`, `LIKE`, `group_concat`, SQLite's order of NULLs …),
//! prepared, cached per connection, and its values converted both ways (`pg.rs`).  The few
//! queries that have no common form ask the connection for its [`Dialect`].

mod dialect;
mod pg;

use std::cell::Cell;
use std::fmt;

pub use dialect::translate;
pub use pg::Tls;

/// Which database a connection talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

// ------------------------------------------------------------------ errors

#[derive(Debug)]
pub enum Error {
    /// `query_row` found no row (`OptionalExtension::optional` makes it `None`).
    QueryReturnedNoRows,
    Sqlite(rusqlite::Error),
    Postgres(tokio_postgres::Error),
    /// A value of one type read or written as another.
    Conversion(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::QueryReturnedNoRows => write!(f, "query returned no rows"),
            Error::Sqlite(e) => write!(f, "{e}"),
            Error::Postgres(e) => match e.as_db_error() {
                Some(d) => write!(f, "{}: {}", d.severity(), d.message()),
                None => write!(f, "{e}"),
            },
            Error::Conversion(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Error::QueryReturnedNoRows,
            e => Error::Sqlite(e),
        }
    }
}

impl From<tokio_postgres::Error> for Error {
    fn from(e: tokio_postgres::Error) -> Self {
        Error::Postgres(e)
    }
}

/// `query_row(...).optional()`: no row is `None`, not an error.
pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ------------------------------------------------------------------ values

pub mod types {
    pub use super::{FromSql, ToSql, Value, ValueRef};
}

/// A value as both databases store it: SQLite's five storage classes.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueRef<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a str),
    Blob(&'a [u8]),
}

impl Value {
    pub fn as_ref(&self) -> ValueRef<'_> {
        match self {
            Value::Null => ValueRef::Null,
            Value::Integer(i) => ValueRef::Integer(*i),
            Value::Real(x) => ValueRef::Real(*x),
            Value::Text(s) => ValueRef::Text(s),
            Value::Blob(b) => ValueRef::Blob(b),
        }
    }
    fn to_rusqlite(&self) -> rusqlite::types::Value {
        match self {
            Value::Null => rusqlite::types::Value::Null,
            Value::Integer(i) => rusqlite::types::Value::Integer(*i),
            Value::Real(x) => rusqlite::types::Value::Real(*x),
            Value::Text(s) => rusqlite::types::Value::Text(s.clone()),
            Value::Blob(b) => rusqlite::types::Value::Blob(b.clone()),
        }
    }
}

impl ValueRef<'_> {
    fn kind(&self) -> &'static str {
        match self {
            ValueRef::Null => "NULL",
            ValueRef::Integer(_) => "an integer",
            ValueRef::Real(_) => "a real number",
            ValueRef::Text(_) => "text",
            ValueRef::Blob(_) => "a blob",
        }
    }
    fn from_rusqlite(v: rusqlite::types::ValueRef<'_>) -> Result<ValueRef<'_>> {
        Ok(match v {
            rusqlite::types::ValueRef::Null => ValueRef::Null,
            rusqlite::types::ValueRef::Integer(i) => ValueRef::Integer(i),
            rusqlite::types::ValueRef::Real(x) => ValueRef::Real(x),
            rusqlite::types::ValueRef::Text(t) => {
                ValueRef::Text(std::str::from_utf8(t).map_err(|e| Error::Conversion(e.to_string()))?)
            }
            rusqlite::types::ValueRef::Blob(b) => ValueRef::Blob(b),
        })
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Integer(i)
    }
}
impl From<f64> for Value {
    fn from(x: f64) -> Self {
        Value::Real(x)
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Null, Into::into)
    }
}

/// What can be passed as a parameter.
pub trait ToSql {
    fn to_sql(&self) -> Value;
}

macro_rules! to_sql_int {
    ($($t:ty),*) => {$(
        impl ToSql for $t {
            fn to_sql(&self) -> Value {
                Value::Integer(*self as i64)
            }
        }
    )*};
}
to_sql_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl ToSql for bool {
    fn to_sql(&self) -> Value {
        Value::Integer(*self as i64)
    }
}
impl ToSql for f64 {
    fn to_sql(&self) -> Value {
        Value::Real(*self)
    }
}
impl ToSql for f32 {
    fn to_sql(&self) -> Value {
        Value::Real(*self as f64)
    }
}
impl ToSql for str {
    fn to_sql(&self) -> Value {
        Value::Text(self.to_string())
    }
}
impl ToSql for String {
    fn to_sql(&self) -> Value {
        Value::Text(self.clone())
    }
}
impl ToSql for std::borrow::Cow<'_, str> {
    fn to_sql(&self) -> Value {
        Value::Text(self.to_string())
    }
}
impl ToSql for [u8] {
    fn to_sql(&self) -> Value {
        Value::Blob(self.to_vec())
    }
}
impl ToSql for Vec<u8> {
    fn to_sql(&self) -> Value {
        Value::Blob(self.clone())
    }
}
impl ToSql for Value {
    fn to_sql(&self) -> Value {
        self.clone()
    }
}
impl<T: ToSql> ToSql for Option<T> {
    fn to_sql(&self) -> Value {
        self.as_ref().map_or(Value::Null, ToSql::to_sql)
    }
}
impl<T: ToSql + ?Sized> ToSql for &T {
    fn to_sql(&self) -> Value {
        (**self).to_sql()
    }
}
impl<T: ToSql + ?Sized> ToSql for Box<T> {
    fn to_sql(&self) -> Value {
        (**self).to_sql()
    }
}

/// What a column can be read as.  The conversions are rusqlite's (a real is not read as an
/// integer, nor a number as text), and an integer is read as a boolean, as SQLite stores them.
pub trait FromSql: Sized {
    fn column_result(v: ValueRef<'_>) -> Result<Self>;
}

fn mismatch<T>(v: ValueRef<'_>, want: &str) -> Result<T> {
    Err(Error::Conversion(format!("{} cannot be read as {want}", v.kind())))
}

impl FromSql for i64 {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        match v {
            ValueRef::Integer(i) => Ok(i),
            v => mismatch(v, "an integer"),
        }
    }
}

macro_rules! from_sql_int {
    ($($t:ty),*) => {$(
        impl FromSql for $t {
            fn column_result(v: ValueRef<'_>) -> Result<Self> {
                let i = i64::column_result(v)?;
                <$t>::try_from(i).map_err(|_| Error::Conversion(format!("{i} is out of range for {}", stringify!($t))))
            }
        }
    )*};
}
from_sql_int!(i32, u32, u64, usize, i16, u16, u8);

impl FromSql for bool {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        match v {
            ValueRef::Integer(i) => Ok(i != 0),
            v => mismatch(v, "a boolean"),
        }
    }
}
impl FromSql for f64 {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        match v {
            ValueRef::Real(x) => Ok(x),
            ValueRef::Integer(i) => Ok(i as f64),
            v => mismatch(v, "a number"),
        }
    }
}
impl FromSql for String {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        match v {
            ValueRef::Text(s) => Ok(s.to_string()),
            v => mismatch(v, "text"),
        }
    }
}
impl FromSql for Vec<u8> {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        match v {
            ValueRef::Blob(b) => Ok(b.to_vec()),
            ValueRef::Text(s) => Ok(s.as_bytes().to_vec()),
            v => mismatch(v, "a blob"),
        }
    }
}
impl FromSql for Value {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        Ok(match v {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(i) => Value::Integer(i),
            ValueRef::Real(x) => Value::Real(x),
            ValueRef::Text(s) => Value::Text(s.to_string()),
            ValueRef::Blob(b) => Value::Blob(b.to_vec()),
        })
    }
}
impl<T: FromSql> FromSql for Option<T> {
    fn column_result(v: ValueRef<'_>) -> Result<Self> {
        match v {
            ValueRef::Null => Ok(None),
            v => T::column_result(v).map(Some),
        }
    }
}

// ------------------------------------------------------------------ parameters

/// A statement's parameters: `[]`, `[a]`, `[a, b]` (all of one type), `params![a, b, …]`,
/// `params_from_iter(…)`.
pub trait Params {
    fn into_values(self) -> Vec<Value>;
}

impl Params for [&dyn ToSql; 0] {
    fn into_values(self) -> Vec<Value> {
        vec![]
    }
}

impl Params for &[&dyn ToSql] {
    fn into_values(self) -> Vec<Value> {
        self.iter().map(|p| p.to_sql()).collect()
    }
}

impl Params for Vec<Value> {
    fn into_values(self) -> Vec<Value> {
        self
    }
}

macro_rules! array_params {
    ($($n:literal),*) => {$(
        impl<T: ToSql> Params for [T; $n] {
            fn into_values(self) -> Vec<Value> {
                self.iter().map(ToSql::to_sql).collect()
            }
        }
        impl<T: ToSql> Params for &[T; $n] {
            fn into_values(self) -> Vec<Value> {
                self.iter().map(ToSql::to_sql).collect()
            }
        }
    )*};
}
array_params!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16);

pub struct ParamsFromIter<I>(I);

/// Parameters from an iterator of values.
pub fn params_from_iter<I>(iter: I) -> ParamsFromIter<I>
where
    I: IntoIterator,
    I::Item: ToSql,
{
    ParamsFromIter(iter)
}

impl<I> Params for ParamsFromIter<I>
where
    I: IntoIterator,
    I::Item: ToSql,
{
    fn into_values(self) -> Vec<Value> {
        self.0.into_iter().map(|p| p.to_sql()).collect()
    }
}

/// Parameters of different types: `params![id, name, now()]`.
#[macro_export]
macro_rules! params {
    () => {
        &[] as &[&dyn $crate::sql::ToSql]
    };
    ($($p:expr),+ $(,)?) => {
        &[$(&$p as &dyn $crate::sql::ToSql),+] as &[&dyn $crate::sql::ToSql]
    };
}
pub use crate::params;

// ------------------------------------------------------------------ rows

/// One row of a result.
pub struct Row<'a>(RowInner<'a>);

enum RowInner<'a> {
    Sqlite(&'a rusqlite::Row<'a>),
    Owned(&'a [Value]),
}

/// A column, by its position.
pub trait RowIndex {
    fn index(&self) -> usize;
}
impl RowIndex for usize {
    fn index(&self) -> usize {
        *self
    }
}
impl RowIndex for i32 {
    fn index(&self) -> usize {
        *self as usize
    }
}

impl Row<'_> {
    pub fn get_ref<I: RowIndex>(&self, idx: I) -> Result<ValueRef<'_>> {
        match &self.0 {
            RowInner::Sqlite(r) => ValueRef::from_rusqlite(r.get_ref(idx.index())?),
            RowInner::Owned(v) => v
                .get(idx.index())
                .map(Value::as_ref)
                .ok_or_else(|| Error::Conversion(format!("no column {}", idx.index()))),
        }
    }

    pub fn get<I: RowIndex, T: FromSql>(&self, idx: I) -> Result<T> {
        let i = idx.index();
        T::column_result(self.get_ref(i)?).map_err(|e| match e {
            Error::Conversion(m) => Error::Conversion(format!("column {i}: {m}")),
            e => e,
        })
    }
}

/// The rows of a query, one at a time.
pub struct Rows<'s>(RowsInner<'s>);

enum RowsInner<'s> {
    Sqlite(rusqlite::Rows<'s>),
    Owned(std::vec::IntoIter<Vec<Value>>, Option<Vec<Value>>),
}

impl Rows<'_> {
    #[allow(clippy::should_implement_trait)] // as rusqlite's: it lends each row
    pub fn next(&mut self) -> Result<Option<Row<'_>>> {
        match &mut self.0 {
            RowsInner::Sqlite(r) => Ok(r.next()?.map(|r| Row(RowInner::Sqlite(r)))),
            RowsInner::Owned(it, cur) => {
                *cur = it.next();
                Ok(cur.as_deref().map(|v| Row(RowInner::Owned(v))))
            }
        }
    }
}

/// A query's results, mapped (they are read in full when the query runs).
pub struct MappedRows<T>(std::vec::IntoIter<Result<T>>);

impl<T> Iterator for MappedRows<T> {
    type Item = Result<T>;
    fn next(&mut self) -> Option<Result<T>> {
        self.0.next()
    }
}

// ------------------------------------------------------------------ connections

/// A connection to the database, either kind.
pub struct Connection {
    b: Backend,
}

enum Backend {
    Sqlite(rusqlite::Connection),
    Postgres(pg::Client),
}

impl Connection {
    pub fn from_sqlite(c: rusqlite::Connection) -> Connection {
        Connection { b: Backend::Sqlite(c) }
    }

    /// A SQLite database in memory (tests, scratch work).
    pub fn open_in_memory() -> Result<Connection> {
        Ok(Connection::from_sqlite(rusqlite::Connection::open_in_memory()?))
    }

    /// Connect to a PostgreSQL database: `url` as libpq takes it (`postgres://user@host/db?…`),
    /// the password apart (so that it is in no settings file).
    pub fn postgres(url: &str, password: Option<&str>, tls: &Tls) -> anyhow::Result<Connection> {
        Self::postgres_with(url, password, tls, "")
    }

    /// `postgres`, with settings for the session (`-c name=value …`, added to the URL's).
    pub fn postgres_with(url: &str, password: Option<&str>, tls: &Tls, options: &str) -> anyhow::Result<Connection> {
        Ok(Connection { b: Backend::Postgres(pg::Client::connect(url, password, tls, options)?) })
    }

    /// `f`, without waiting for another writer (an import holding SQLite's one write lock): a
    /// write that can wait for another time (a session's last use) fails at once rather than
    /// after the busy timeout.  On PostgreSQL, `f` as it is.
    pub fn without_waiting<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let Backend::Sqlite(c) = &self.b else { return f(self) };
        c.busy_timeout(std::time::Duration::ZERO)?;
        let r = f(self);
        c.busy_timeout(std::time::Duration::from_secs(30))?;
        r
    }

    pub fn dialect(&self) -> Dialect {
        match &self.b {
            Backend::Sqlite(_) => Dialect::Sqlite,
            Backend::Postgres(_) => Dialect::Postgres,
        }
    }

    /// The SQLite connection itself (settings, backups); `None` on PostgreSQL.
    pub fn sqlite(&self) -> Option<&rusqlite::Connection> {
        match &self.b {
            Backend::Sqlite(c) => Some(c),
            Backend::Postgres(_) => None,
        }
    }

    /// The condition "the row `alias` of item_text holds `text`" (in any case, any part of a
    /// word: SQLite's FTS5 trigram index, PostgreSQL's ILIKE over a trigram index), for
    /// parameter `?n`, and that parameter.
    pub fn text_contains(&self, alias: &str, n: usize, text: &str) -> (String, String) {
        match self.dialect() {
            Dialect::Sqlite => (format!("item_text MATCH ?{n}"), format!("\"{}\"", text.replace('"', "\"\""))),
            Dialect::Postgres => (
                format!("{alias}.body ILIKE ?{n}"),
                format!("%{}%", text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")),
            ),
        }
    }

    /// `n` new ids for rows of `table` (its `id` column), to insert many rows at once with their
    /// ids known: on PostgreSQL drawn from its sequence; on SQLite, whose one writer is this
    /// transaction, the next ones after every id given so far.
    pub fn next_ids(&self, table: &str, n: usize) -> Result<Vec<i64>> {
        if n == 0 {
            return Ok(vec![]);
        }
        match self.dialect() {
            Dialect::Sqlite => {
                let base: i64 = self.query_row(
                    &format!(
                        "SELECT MAX(COALESCE((SELECT seq FROM sqlite_sequence WHERE name = '{table}'), 0),
                                    COALESCE((SELECT MAX(id) FROM {table}), 0))"
                    ),
                    [],
                    |r| r.get(0),
                )?;
                Ok((1..=n as i64).map(|k| base + k).collect())
            }
            Dialect::Postgres => {
                let mut st = self.prepare(&format!(
                    "SELECT nextval(pg_get_serial_sequence('{table}', 'id')) FROM generate_series(1, {n})"
                ))?;
                st.query_map([], |r| r.get(0))?.collect()
            }
        }
    }

    /// Whether the database has this table.
    pub fn table_exists(&self, table: &str) -> Result<bool> {
        match self.dialect() {
            Dialect::Sqlite => self.query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |r| r.get(0),
            ),
            Dialect::Postgres => {
                self.query_row("SELECT to_regclass(CAST(?1 AS TEXT)) IS NOT NULL", [table], |r| r.get(0))
            }
        }
    }

    /// Rows inserted many to a statement: `head` is `INSERT INTO t (a, b, …)`, `tail` what
    /// follows the values (`ON CONFLICT …`, or nothing); each row has a value for each column.
    pub fn insert_rows(&self, head: &str, tail: &str, rows: &[Vec<Value>]) -> Result<()> {
        let width = rows.first().map_or(1, |r| r.len().max(1));
        for chunk in rows.chunks((30_000 / width).clamp(1, 500)) {
            let (list, args) = values_list(chunk, &[]);
            self.execute(&format!("{head} VALUES {list} {tail}"), params_from_iter(args))?;
        }
        Ok(())
    }

    /// The text of cell `pos` of `col`, a JSON array of texts (an imported row's cells).
    pub fn json_cell(&self, col: &str, pos: usize) -> String {
        match self.dialect() {
            Dialect::Sqlite => format!("json_extract({col}, '$[{pos}]')"),
            Dialect::Postgres => format!("(CAST({col} AS jsonb) ->> {pos})"),
        }
    }

    /// `expr` (text) read as a number: on PostgreSQL NULL when it is not written as one (a cast
    /// would fail), on SQLite its leading number (0 when none).  For filtering rows before a
    /// test that reads the text itself.
    pub fn as_number(&self, expr: &str) -> String {
        match self.dialect() {
            Dialect::Sqlite => format!("CAST({expr} AS REAL)"),
            Dialect::Postgres => format!(
                "(CASE WHEN {expr} ~ '^\\s*[-+]?([0-9]+\\.?[0-9]*|\\.[0-9]+)([eE][-+]?[0-9]+)?\\s*$' THEN CAST({expr} AS DOUBLE PRECISION) END)"
            ),
        }
    }

    /// `sql` for many values at once: `{}` in it is an IN list of the values (as `?1, ?2, …`),
    /// run for 500 of them at a time (both databases limit a statement's parameters); every
    /// row, mapped by `f`.  One query instead of one per value.
    pub fn query_in<T, V: ToSql>(
        &self,
        sql: &str,
        values: &[V],
        mut f: impl FnMut(&Row<'_>) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut out = vec![];
        for chunk in values.chunks(500) {
            let list: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let mut st = self.prepare(&sql.replace("{}", &list.join(", ")))?;
            for r in st.query_map(params_from_iter(chunk.iter()), &mut f)? {
                out.push(r?);
            }
        }
        Ok(out)
    }

    pub fn execute<P: Params>(&self, sql: &str, p: P) -> Result<usize> {
        self.prepare(sql)?.execute(p)
    }

    /// Statements without parameters, separated by `;`.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        match &self.b {
            Backend::Sqlite(c) => Ok(c.execute_batch(sql)?),
            Backend::Postgres(c) => c.batch(&translate(sql)),
        }
    }

    /// Statements in the database's own dialect, as written (schema migrations).
    pub fn execute_batch_native(&self, sql: &str) -> Result<()> {
        match &self.b {
            Backend::Sqlite(c) => Ok(c.execute_batch(sql)?),
            Backend::Postgres(c) => c.batch(sql),
        }
    }

    /// Whether the connection is lost (PostgreSQL: the server closed it).
    pub fn is_closed(&self) -> bool {
        match &self.b {
            Backend::Sqlite(_) => false,
            Backend::Postgres(c) => c.is_closed(),
        }
    }

    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        Ok(Statement(match &self.b {
            Backend::Sqlite(c) => StmtInner::Sqlite(c.prepare_cached(sql)?),
            Backend::Postgres(c) => StmtInner::Postgres(c, c.prepare(sql)?),
        }))
    }

    pub fn query_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(&self, sql: &str, p: P, f: F) -> Result<T> {
        self.prepare(sql)?.query_row(p, f)
    }

    /// A statement in the database's own dialect, as written, not translated: a connected
    /// database's queries, where SQL its users wrote is put in (Medorama's parts written for
    /// either dialect, their parameters numbered by [`Connection::param`]).
    pub fn prepare_native(&self, sql: &str) -> Result<Statement<'_>> {
        Ok(Statement(match &self.b {
            Backend::Sqlite(c) => StmtInner::Sqlite(c.prepare_cached(sql)?),
            Backend::Postgres(c) => StmtInner::Postgres(c, c.prepare_native(sql)?),
        }))
    }

    pub fn query_row_native<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(&self, sql: &str, p: P, f: F) -> Result<T> {
        self.prepare_native(sql)?.query_row(p, f)
    }

    /// Parameter `n` (from 1) as the database writes it: `?n` (SQLite), `$n` (PostgreSQL).
    pub fn param(&self, n: usize) -> String {
        match self.dialect() {
            Dialect::Sqlite => format!("?{n}"),
            Dialect::Postgres => format!("${n}"),
        }
    }

    /// [`Connection::query_in`] for a statement in the database's own dialect
    /// ([`Connection::prepare_native`]).
    pub fn query_in_native<T, V: ToSql>(
        &self,
        sql: &str,
        values: &[V],
        mut f: impl FnMut(&Row<'_>) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut out = vec![];
        for chunk in values.chunks(500) {
            let list: Vec<String> = (1..=chunk.len()).map(|i| self.param(i)).collect();
            let mut st = self.prepare_native(&sql.replace("{}", &list.join(", ")))?;
            for r in st.query_map(params_from_iter(chunk.iter()), &mut f)? {
                out.push(r?);
            }
        }
        Ok(out)
    }

    /// An insert into a table with an `id` column (INTEGER PRIMARY KEY), returning the new id.
    pub fn insert<P: Params>(&self, sql: &str, p: P) -> Result<i64> {
        match &self.b {
            Backend::Sqlite(c) => {
                self.execute(sql, p)?;
                Ok(c.last_insert_rowid())
            }
            Backend::Postgres(_) => self.query_row(&format!("{sql} RETURNING id"), p, |r| r.get(0)),
        }
    }

    /// Within the open transaction, wait until no other server holds `lock` (PostgreSQL: an
    /// advisory lock, released at commit).  SQLite, which has one writer at a time, needs none.
    pub fn lock_in_transaction(&self, lock: Lock) -> Result<()> {
        match &self.b {
            Backend::Sqlite(_) => Ok(()),
            Backend::Postgres(c) => c.command(&format!("SELECT pg_advisory_xact_lock({})", lock as i64)),
        }
    }

    /// Run `f` while no other server runs the work of `lock` (PostgreSQL: an advisory lock on
    /// this connection, given back however `f` ends, a panic included; SQLite: as is).
    pub fn exclusive<T>(&self, lock: Lock, f: impl FnOnce() -> T) -> Result<T> {
        match &self.b {
            Backend::Sqlite(_) => Ok(f()),
            Backend::Postgres(c) => {
                struct Unlock<'a>(&'a pg::Client, i64);
                impl Drop for Unlock<'_> {
                    fn drop(&mut self) {
                        let _ = self.0.command(&format!("SELECT pg_advisory_unlock({})", self.1));
                    }
                }
                c.command(&format!("SELECT pg_advisory_lock({})", lock as i64))?;
                let _unlock = Unlock(c, lock as i64);
                Ok(f())
            }
        }
    }

    /// Wait until no other import runs (on any server, or in any `medorama` command), and hold
    /// that until the guard is dropped: imports run one at a time, in the order they asked
    /// (within a process).  `waiting` is called about every second while another runs (to say
    /// so, and to stop: an error from it is returned).  The lock is held by a connection of its
    /// own (SQLite: an exclusive lock on the file `<database>-import.lock`; PostgreSQL: a
    /// session's advisory lock), so a process that stops gives it back.  The same thread may
    /// take it again while holding it (a folder of tables, imported one after another).
    pub fn import_lock(&self, mut waiting: impl FnMut() -> anyhow::Result<()>) -> anyhow::Result<ImportLock> {
        if IMPORT_DEPTH.with(|d| d.get()) > 0 {
            IMPORT_DEPTH.with(|d| d.set(d.get() + 1));
            return Ok(ImportLock { held: None, ticket: None, top: false });
        }
        // this process's imports into this database, in turn
        let key = match &self.b {
            Backend::Sqlite(c) => c.path().unwrap_or("").to_string(),
            Backend::Postgres(c) => c.url().to_string(),
        };
        let ticket = {
            let (q, _) = import_queue();
            let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
            q.1 += 1;
            let t = q.1;
            q.0.push_back((t, key.clone()));
            t
        };
        let mut lock = ImportLock { held: None, ticket: Some(ticket), top: false };
        {
            let (q, cv) = import_queue();
            let first = |g: &(std::collections::VecDeque<(u64, String)>, u64)| {
                g.0.iter().find(|(_, k)| *k == key).map(|(t, _)| *t)
            };
            let mut g = q.lock().unwrap_or_else(|e| e.into_inner());
            while first(&g) != Some(ticket) {
                let (next, _) =
                    cv.wait_timeout(g, std::time::Duration::from_secs(1)).unwrap_or_else(|e| e.into_inner());
                g = next;
                if first(&g) != Some(ticket) {
                    drop(g);
                    waiting()?; // an error drops `lock`: the ticket leaves the queue
                    g = q.lock().unwrap_or_else(|e| e.into_inner());
                }
            }
        }
        // then the other processes' and servers'
        let held = match &self.b {
            Backend::Sqlite(c) => match c.path().filter(|p| !p.is_empty()) {
                None => None, // a database in memory: this connection's alone
                Some(p) => {
                    let l = rusqlite::Connection::open(format!("{p}-import.lock"))?;
                    l.busy_timeout(std::time::Duration::from_millis(1000))?;
                    loop {
                        match l.execute_batch("BEGIN EXCLUSIVE") {
                            Ok(()) => break,
                            Err(rusqlite::Error::SqliteFailure(e, _))
                                if matches!(
                                    e.code,
                                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                                ) =>
                            {
                                waiting()?
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    Some(Held::Sqlite(l))
                }
            },
            Backend::Postgres(c) => {
                let l = Connection { b: Backend::Postgres(c.again()?) };
                while !l.query_row("SELECT pg_try_advisory_lock(?1)", [Lock::Import as i64], |r| r.get::<_, bool>(0))? {
                    waiting()?;
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                Some(Held::Postgres(l))
            }
        };
        lock.held = held;
        lock.top = true;
        IMPORT_DEPTH.with(|d| d.set(1));
        Ok(lock)
    }

    /// Whether no import holds [`Connection::import_lock`] now (none runs): then the imports
    /// left loading are those a stop or a crash interrupted.  Returns the lock, held, if so.
    pub fn import_lock_if_free(&self) -> anyhow::Result<Option<ImportLock>> {
        let mut busy = false;
        let r = self.import_lock(|| {
            busy = true;
            anyhow::bail!("an import runs")
        });
        match r {
            Ok(l) => Ok(Some(l)),
            Err(_) if busy => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether no transaction is open.
    pub fn is_autocommit(&self) -> bool {
        match &self.b {
            Backend::Sqlite(c) => c.is_autocommit(),
            Backend::Postgres(c) => !c.in_transaction.get(),
        }
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.transaction_with_behavior(TransactionBehavior::Deferred)
    }

    /// A transaction.  `Immediate` takes the write lock at once (SQLite); on PostgreSQL,
    /// where writers lock rows, both are the same.
    pub fn transaction_with_behavior(&mut self, b: TransactionBehavior) -> Result<Transaction<'_>> {
        match &self.b {
            Backend::Sqlite(c) => c.execute_batch(match b {
                TransactionBehavior::Deferred => "BEGIN DEFERRED",
                TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
            })?,
            Backend::Postgres(c) => {
                c.batch("BEGIN")?;
                c.in_transaction.set(true);
            }
        }
        Ok(Transaction { conn: self, done: Cell::new(false) })
    }

    /// A transaction begun through a shared reference (rusqlite's name): the caller makes sure
    /// none is open already.
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        match &self.b {
            Backend::Sqlite(c) => c.execute_batch("BEGIN DEFERRED")?,
            Backend::Postgres(c) => {
                c.batch("BEGIN")?;
                c.in_transaction.set(true);
            }
        }
        Ok(Transaction { conn: self, done: Cell::new(false) })
    }

    fn end(&self, commit: bool) -> Result<()> {
        let sql = if commit { "COMMIT" } else { "ROLLBACK" };
        match &self.b {
            Backend::Sqlite(c) => Ok(c.execute_batch(sql)?),
            Backend::Postgres(c) => {
                c.in_transaction.set(false);
                c.command(sql)
            }
        }
    }
}

/// Work that one server at a time does, when several share a PostgreSQL database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Lock {
    /// Bringing the schema up to date.
    Migrate = 7310,
    /// Taking the next job from the queue.
    ClaimJob = 7311,
    /// Queuing automatic indexing.
    Schedule = 7312,
    /// Sealing the audit log.
    Audit = 7313,
    /// Writing patients and encounters (their identity): read, decided and written by one
    /// transaction at a time, so that one MRN never names two patients.
    Identity = 7314,
    /// Running an import ([`Connection::import_lock`]).
    Import = 7315,
}

thread_local! {
    /// How many times this thread holds the import lock.
    static IMPORT_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// This process's imports waiting for the import lock, in order (tickets, with their
/// database), and the last ticket given.
type ImportQueue = (std::sync::Mutex<(std::collections::VecDeque<(u64, String)>, u64)>, std::sync::Condvar);

fn import_queue() -> &'static ImportQueue {
    static Q: std::sync::OnceLock<ImportQueue> = std::sync::OnceLock::new();
    Q.get_or_init(Default::default)
}

enum Held {
    Sqlite(#[allow(dead_code)] rusqlite::Connection),
    Postgres(Connection),
}

/// The import lock, held until dropped ([`Connection::import_lock`]).
pub struct ImportLock {
    held: Option<Held>,
    /// This process's turn; `None` when taken again by the thread holding it.
    ticket: Option<u64>,
    /// Taken (not only waited for).
    top: bool,
}

impl ImportLock {
    /// Whether this is the import lock taken (not taken again by the thread that holds it).
    pub fn first(&self) -> bool {
        self.top
    }
}

impl Drop for ImportLock {
    fn drop(&mut self) {
        let Some(ticket) = self.ticket else {
            IMPORT_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
            return;
        };
        if self.top {
            IMPORT_DEPTH.with(|d| d.set(0));
        }
        match self.held.take() {
            Some(Held::Postgres(c)) => {
                let _ = c.query_row("SELECT pg_advisory_unlock(?1)", [Lock::Import as i64], |r| r.get::<_, bool>(0));
            }
            Some(Held::Sqlite(c)) => drop(c), // closing it ends its transaction and lock
            None => {}
        }
        let (q, cv) = import_queue();
        let mut g = q.lock().unwrap_or_else(|e| e.into_inner());
        g.0.retain(|(t, _)| *t != ticket);
        cv.notify_all();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionBehavior {
    Deferred,
    Immediate,
}

/// An open transaction: rolled back when dropped unless committed.
pub struct Transaction<'c> {
    conn: &'c Connection,
    done: Cell<bool>,
}

impl Transaction<'_> {
    pub fn commit(self) -> Result<()> {
        self.done.set(true);
        self.conn.end(true)
    }
    pub fn rollback(self) -> Result<()> {
        self.done.set(true);
        self.conn.end(false)
    }
}

impl std::ops::Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.done.get() {
            let _ = self.conn.end(false);
        }
    }
}

// ------------------------------------------------------------------ statements

pub struct Statement<'c>(StmtInner<'c>);

enum StmtInner<'c> {
    Sqlite(rusqlite::CachedStatement<'c>),
    Postgres(&'c pg::Client, pg::Prepared),
}

fn sqlite_params(v: &[Value]) -> Vec<rusqlite::types::Value> {
    v.iter().map(Value::to_rusqlite).collect()
}

impl Statement<'_> {
    pub fn execute<P: Params>(&mut self, p: P) -> Result<usize> {
        let v = p.into_values();
        match &mut self.0 {
            StmtInner::Sqlite(s) => Ok(s.execute(rusqlite::params_from_iter(sqlite_params(&v)))?),
            StmtInner::Postgres(c, s) => c.execute(s, &v),
        }
    }

    pub fn query<P: Params>(&mut self, p: P) -> Result<Rows<'_>> {
        let v = p.into_values();
        Ok(Rows(match &mut self.0 {
            StmtInner::Sqlite(s) => RowsInner::Sqlite(s.query(rusqlite::params_from_iter(sqlite_params(&v)))?),
            StmtInner::Postgres(c, s) => RowsInner::Owned(c.query(s, &v)?.into_iter(), None),
        }))
    }

    pub fn query_map<T, P: Params, F: FnMut(&Row<'_>) -> Result<T>>(
        &mut self,
        p: P,
        mut f: F,
    ) -> Result<MappedRows<T>> {
        let mut out = vec![];
        let mut rows = self.query(p)?;
        loop {
            match rows.next() {
                Ok(Some(r)) => out.push(f(&r)),
                Ok(None) => break,
                Err(e) => {
                    out.push(Err(e));
                    break;
                }
            }
        }
        Ok(MappedRows(out.into_iter()))
    }

    pub fn query_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(&mut self, p: P, f: F) -> Result<T> {
        let mut rows = self.query(p)?;
        match rows.next()? {
            Some(r) => f(&r),
            None => Err(Error::QueryReturnedNoRows),
        }
    }

    pub fn exists<P: Params>(&mut self, p: P) -> Result<bool> {
        Ok(self.query(p)?.next()?.is_some())
    }
}

/// Rows as a `VALUES` list (`(?1, ?2), (?3, ?4)`) and its values, in order.  `types`, when
/// given, are each column's (`INTEGER`, `REAL`, `TEXT`): the values are cast to them, as a list
/// used as a table (`WITH v (a, b) AS (VALUES …)`) needs on PostgreSQL, where a parameter has no
/// type of its own.
pub fn values_list(rows: &[Vec<Value>], types: &[&str]) -> (String, Vec<Value>) {
    let mut args = Vec::with_capacity(rows.iter().map(Vec::len).sum());
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, v)| {
                args.push(v.clone());
                match types.get(i) {
                    Some(t) => format!("CAST(?{} AS {t})", args.len()),
                    None => format!("?{}", args.len()),
                }
            })
            .collect();
        out.push(format!("({})", cells.join(", ")));
    }
    (out.join(", "), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn values_go_both_ways() {
        let mut c = memory();
        c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, n INTEGER, x REAL, s TEXT, b BLOB)")
            .unwrap();
        let id = c.insert("INSERT INTO t (n, x, s, b) VALUES (?1, ?2, ?3, ?4)", params![true, 1.5, "a", vec![1u8, 2]]);
        assert_eq!(id.unwrap(), 1);
        let (n, x, s, b): (bool, f64, String, Vec<u8>) =
            c.query_row("SELECT n, x, s, b FROM t", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!((n, x, s.as_str(), b), (true, 1.5, "a", vec![1, 2]));
        let none: Option<i64> = c.query_row("SELECT n FROM t WHERE id = 9", [], |r| r.get(0)).optional().unwrap();
        assert_eq!(none, None);
        assert!(c.query_row("SELECT s FROM t", [], |r| r.get::<_, i64>(0)).is_err());
        {
            let tx = c.transaction().unwrap();
            tx.execute("DELETE FROM t", []).unwrap();
        } // dropped: rolled back
        assert_eq!(c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert!(c.is_autocommit());
        let mut st = c.prepare("SELECT id FROM t WHERE id IN (?1, ?2)").unwrap();
        let ids: Vec<i64> =
            st.query_map(params_from_iter([1, 2]), |r| r.get(0)).unwrap().collect::<Result<_>>().unwrap();
        assert_eq!(ids, vec![1]);
    }
}
