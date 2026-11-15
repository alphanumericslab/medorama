//! Adding tables (`db-builder add`): each file a source, its rows written as `persons`,
//! `visits`, `entries`, `recordings` and `extras` (docs/schema.md).  A row may name, in any
//! combination:
//! - **a patient**: `patient_id` (with its issuer) and/or `mrn` (with `mrn_authority`), names,
//!   date of birth, sex: found by its identifiers, else added;
//! - **an encounter**: `encounter_id` (its `visit_key`), admission and discharge, unit, and a bed
//!   stay (`care_unit`, `bed`, `bed_start`, `bed_end`: kept in `extras` as a `bed_stays` row);
//! - **an entry**: a measurement, a drug, a code, a note (`category`, `name`, `time`, `value` ...),
//!   and one more entry for each number of a measurement column (`QTc (ms)`);
//! - **a file**: a recording (a WFDB record by its header, an EDF or BDF file, a file an installed
//!   adapter reads: a `recordings` row), or a file of the entry (kept in `extras`).
//!
//! Tables come in any order: a row naming an encounter that no table has named yet keeps its
//! `visit_key`, and gets its person when the encounter's table is added ([`settle`]).  A column
//! that is none of the above goes to `extras` under its own header, as it came.

use crate::categories;
use crate::db;
use crate::formats::{Choice, Converter, Format};
use crate::tables::{self, column_map, guess_category, guess_mapping, measure_column};
use crate::util::{iso, parse_date, parse_time};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What `add` and `recordings` are told.
#[derive(Clone, Default)]
pub struct Options {
    /// The issuer of the rows' patient IDs when a row names none (`--issuer`).
    pub issuer: String,
    /// The authority of the rows' MRNs when a row names none (`--mrn-authority`).
    pub mrn_authority: String,
    /// The category of the entries when a row names none (`--category`).
    pub category: String,
    /// The columns mapped by hand (`--mapping`): recognised column -> header.
    pub mapping: HashMap<String, String>,
    /// `auto` (or empty), `none`, or a format's key (`--format`).
    pub format: String,
    /// TOML files (or folders of them) describing formats (`--formats`).
    pub formats: Vec<PathBuf>,
    /// Slash dates read day first (`--day-first`).
    pub day_first: bool,
    /// Files' paths and identifiers rewritten as they are read (`--rules`).
    pub rules: crate::rules::Rules,
    /// Only say what would be added: everything is read and written in a transaction that is
    /// then rolled back, so the database is left as it was (`--dry-run`, the page's Preview).
    pub dry_run: bool,
    /// Where the rows read so far are counted, for whoever watches (the web page's progress).
    pub progress: Progress,
}

/// The progress of an `add` or `recordings`: the file being read and the rows read so far,
/// shared with another thread that shows them (the web interface).  The default counts nothing.
#[derive(Clone, Default)]
pub struct Progress(Option<Arc<ProgressState>>);

#[derive(Default)]
pub struct ProgressState {
    file: std::sync::Mutex<String>,
    rows: std::sync::atomic::AtomicUsize,
}

impl Progress {
    /// A progress that counts.
    pub fn counting() -> Progress {
        Progress(Some(Arc::default()))
    }
    /// The file now read.
    pub fn file(&self, f: &str) {
        if let Some(p) = &self.0 {
            *p.file.lock().unwrap() = f.to_string();
        }
    }
    /// One more row (or recording) read.
    pub fn row(&self) {
        if let Some(p) = &self.0 {
            p.rows.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// The file now read, and the rows read so far (of every file).
    pub fn get(&self) -> (String, usize) {
        match &self.0 {
            Some(p) => (p.file.lock().unwrap().clone(), p.rows.load(std::sync::atomic::Ordering::Relaxed)),
            None => (String::new(), 0),
        }
    }
}

/// How one column of an input is read (what a preview shows): its header, and the recognised
/// field it fills (docs/importing.md), or `measure` (a measurement column: one entry per
/// number) or `extra` (kept in `extras` under its header).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ColumnUse {
    pub header: String,
    /// `field`, `measure` or `extra`.
    pub how: String,
    /// The field (`patient_id`, `time` ...), or the measurement's name and unit (`QTc`, `ms`).
    pub field: String,
    pub unit: String,
}

/// What adding one source did (or would do, in a dry run).
#[derive(Debug, Default, serde::Serialize)]
pub struct SourceReport {
    pub file: String,
    pub kind: String,
    /// Its source id (none when it was already added).
    pub source: Option<i64>,
    /// The source it is the same file as (already added: nothing done).
    pub already: Option<i64>,
    /// The sources of an earlier version of the file, removed first.
    pub replaced: Vec<i64>,
    pub rows: usize,
    pub persons_added: usize,
    pub visits_added: usize,
    pub entries: usize,
    pub recordings: usize,
    pub left_out: usize,
    pub warnings: Vec<String>,
    /// The known export it was read as (its label), if any.
    pub format: String,
    /// How its columns are read, in their order (after a known export's conversion).
    pub columns: Vec<ColumnUse>,
    /// Its first rows ([`SAMPLE_ROWS`]), cells in the order of `columns` (the web page's
    /// preview; never printed on the command line: they may hold identifiers).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample: Vec<Vec<String>>,
}

/// The rows of an input a report keeps as its sample.
pub const SAMPLE_ROWS: usize = 5;

const MAX_WARNINGS: usize = 50;

impl SourceReport {
    pub fn warn(&mut self, msg: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(msg);
        } else if self.warnings.len() == MAX_WARNINGS {
            self.warnings.push("(more warnings not shown)".into());
        }
    }
}

/// The database's time zone (`meta.timezone`).
pub fn zone_of(c: &Connection) -> anyhow::Result<Tz> {
    crate::util::tz(&db::meta(c, "timezone")?.unwrap_or_else(|| db::DEFAULT_TIMEZONE.into()))
}

/// Add these inputs (tables, or CLIF folders) to the database at `db_path`, in one transaction:
/// all of them, or nothing.  With `opts.dry_run`, the transaction is rolled back: the reports
/// say what would be added, and the database is not changed.
pub fn add(
    c: &mut Connection,
    db_path: &Path,
    inputs: &[PathBuf],
    opts: &Options,
) -> anyhow::Result<Vec<SourceReport>> {
    let (known, problems) = crate::formats::all(&opts.formats);
    if let Some(p) = problems.first() {
        anyhow::bail!("{p}");
    }
    let choice = Choice::parse(&opts.format, &known)?;
    let zone = zone_of(c)?;
    let db_dir = db::folder_of(db_path);
    let order = if opts.day_first { crate::util::DateOrder::DayFirst } else { crate::util::DateOrder::MonthFirst };
    crate::util::with_date_order(order, || {
        let tx = c.transaction()?;
        let mut out = vec![];
        for input in inputs {
            let abs = db::absolute(input);
            if abs.is_dir() {
                if db::absolute(db_path).starts_with(&abs) {
                    anyhow::bail!("{}: the database would be written into the folder it reads", abs.display());
                }
                out.extend(add_clif_folder(&tx, &abs, &known, zone, &db_dir, opts)?);
            } else {
                out.push(add_table(&tx, &abs, &choice, &known, zone, &db_dir, opts)?);
            }
        }
        // (nothing written when every file was added already: the version stays; nothing at
        // all in a dry run: the transaction is dropped, rolled back)
        if !opts.dry_run && out.iter().any(|r| r.source.is_some()) {
            settle(&tx)?;
            db::bump(&tx)?;
            tx.commit()?;
        }
        Ok(out)
    })
}

/// A CLIF folder: its tables (CSV or Parquet files) recognised by their columns, each file a
/// source, in order (patients, hospitalizations, ADT, then the rest; the susceptibilities last,
/// each on the culture of the folder its `organism_id` names).
fn add_clif_folder(
    tx: &Connection,
    dir: &Path,
    known: &[Arc<dyn Format>],
    zone: Tz,
    db_dir: &Path,
    opts: &Options,
) -> anyhow::Result<Vec<SourceReport>> {
    use crate::formats::clif;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("csv") || x.eq_ignore_ascii_case("parquet")))
        // (`db-builder serve`: a link out of its root is not read)
        .filter(|p| recording_engine::paths::allowed_file(p))
        .collect();
    files.sort();
    let mut found: Vec<(String, PathBuf)> = vec![];
    let mut skipped = vec![];
    for f in files {
        let (cols, _) = tables::rows_of(&f)?;
        match known.iter().find(|x| x.key().starts_with("clif-") && x.detect(&cols)) {
            Some(fmt) => found.push((fmt.key().to_string(), f)),
            None => skipped.push(f),
        }
    }
    if found.is_empty() {
        anyhow::bail!("{}: no CLIF table (CSV or Parquet) found", dir.display());
    }
    found.sort_by_key(|(k, f)| (clif::rank(k), k.clone(), f.clone()));
    let culture_files: Vec<PathBuf> =
        found.iter().filter(|(k, _)| k == "clif-microbiology-culture").map(|(_, f)| f.clone()).collect();
    let mut out = vec![];
    for (key, f) in found {
        let fmt: Arc<dyn Format> = if key == clif::SUSCEPTIBILITY {
            Arc::new(clif::Clif::susceptibility(clif::cultures(&culture_files)?))
        } else {
            known.iter().find(|x| x.key() == key).cloned().expect("a known format")
        };
        let mut r = add_table(tx, &f, &Choice::Named(fmt), known, zone, db_dir, opts)?;
        if out.is_empty() {
            for s in &skipped {
                r.warn(format!("{}: not a CLIF table, left out", s.display()));
            }
        }
        out.push(r);
    }
    Ok(out)
}

/// Whether the source of this file is already in the database: the same file (path and
/// fingerprint) is not added twice; an earlier version of it (another fingerprint) is removed
/// first.  Returns the report so far, and whether to go on.
fn begin_source(tx: &Connection, file: &str, kind: &str, fingerprint: &str) -> anyhow::Result<(SourceReport, bool)> {
    let mut rep = SourceReport { file: file.into(), kind: kind.into(), ..Default::default() };
    let before = db::source_of(tx, file)?;
    if let Some((id, _)) = before.iter().find(|(_, f)| f == fingerprint) {
        rep.already = Some(*id);
        return Ok((rep, false));
    }
    for (id, _) in before {
        db::remove_source(tx, id)?;
        rep.replaced.push(id);
    }
    rep.source = Some(db::add_source(tx, file, kind, fingerprint)?);
    Ok((rep, true))
}

/// One table file, a source of its own: read row by row (converted first when it is a known
/// export), each row written by [`Writer::row`].
fn add_table(
    tx: &Connection,
    path: &Path,
    choice: &Choice,
    known: &[Arc<dyn Format>],
    zone: Tz,
    db_dir: &Path,
    opts: &Options,
) -> anyhow::Result<SourceReport> {
    let file = path.to_string_lossy().to_string();
    let fingerprint = db::fingerprint(path)?;
    let (raw_headers, rows) = tables::rows_of(path)?;
    let format = choice.resolve(&raw_headers, known);
    let mut convert: Option<Box<dyn Converter>> = match &format {
        Some(f) => Some(f.start(&raw_headers).map_err(|e| anyhow::anyhow!("{file}: {e}"))?),
        None => None,
    };
    let kind = match &format {
        Some(f) => f.key().to_string(),
        None => tables::kind_of(path)?.to_string(),
    };
    opts.progress.file(&file);
    let headers = convert.as_ref().map_or_else(|| raw_headers.clone(), |c| c.headers());
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // no category chosen: the format's, else the one the file name or headers suggest
    let category = if !opts.category.trim().is_empty() {
        opts.category.trim().to_string()
    } else {
        format
            .as_ref()
            .filter(|f| !f.category().is_empty())
            .map_or_else(|| guess_category(&name, &headers), |f| f.category().to_string())
    };
    let mapping = if opts.mapping.is_empty() { guess_mapping(&headers) } else { opts.mapping.clone() };
    let mapping: HashMap<String, String> = mapping.into_iter().filter(|(_, h)| !h.is_empty()).collect();
    let (maps, doubled) = column_map(&headers, &mapping, &tables::columns());
    // the columns that are none of the recognised ones: a measurement, else an extra
    let measures: Vec<Option<(String, String)>> =
        headers.iter().zip(&maps).map(|(h, k)| if k.is_empty() { measure_column(h) } else { None }).collect();
    let (mut rep, go) = begin_source(tx, &file, &kind, &fingerprint)?;
    rep.format = format.as_ref().map_or_else(String::new, |f| f.label().to_string());
    rep.columns = column_uses(&headers, &maps, &measures);
    if !go {
        return Ok(rep);
    }
    let source = rep.source.expect("a new source");
    for w in doubled {
        rep.warn(format!("{name}: {w}"));
    }
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut w = Writer::new(tx, source, zone, opts, &category, &base, db_dir);
    // rows a format leaves out, by why (counted: rows hold identifiers)
    let mut left_out: BTreeMap<String, usize> = Default::default();
    let mut n = 0usize;
    for record in rows {
        let (line, record) = record.map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
        if record.iter().all(|v| v.trim().is_empty()) {
            continue;
        }
        // a format may make several rows of one (a ventilator's settings, each an entry)
        let converted = match convert.as_mut() {
            Some(conv) => match conv.rows(&record) {
                Ok(r) => r,
                Err(why) => {
                    *left_out.entry(why).or_default() += 1;
                    continue;
                }
            },
            None => vec![record],
        };
        for cells in converted {
            n += 1;
            opts.progress.row();
            if rep.sample.len() < SAMPLE_ROWS {
                rep.sample.push(cells.iter().map(|c| c.trim().to_string()).collect());
            }
            let mut row = Row { line: Line { file: &name, line }, number: n, ..Default::default() };
            for (i, cell) in cells.iter().enumerate() {
                let v = cell.trim();
                let Some(h) = headers.get(i) else { continue };
                if !maps[i].is_empty() {
                    row.fields.insert(maps[i], v.to_string());
                } else if v.is_empty() {
                } else if let Some((m, unit)) = &measures[i] {
                    row.measures.push(Measure {
                        col: i,
                        header: h.clone(),
                        name: m.clone(),
                        unit: unit.clone(),
                        value: v.to_string(),
                    });
                } else {
                    row.extras.push((h.clone(), v.to_string()));
                }
            }
            opts.rules.apply(&mut row.fields);
            w.row(&row, &mut rep)?;
        }
    }
    for (why, k) in left_out.iter().rev() {
        rep.left_out += k;
        rep.warnings.insert(0, format!("{k} {why}: left out ({})", format.as_ref().map_or("", |f| f.label())));
    }
    rep.rows = n;
    tx.execute("UPDATE sources SET rows = ?1 WHERE id = ?2", params![n as i64, source])?;
    w.finish(&mut rep);
    Ok(rep)
}

/// How each column is read: a recognised field, a measurement, or an extra.
pub fn column_uses(headers: &[String], maps: &[&'static str], measures: &[Option<(String, String)>]) -> Vec<ColumnUse> {
    headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let field = maps.get(i).copied().unwrap_or("");
            let (how, field, unit) = match measures.get(i).and_then(|m| m.as_ref()) {
                _ if !field.is_empty() => ("field", field.to_string(), String::new()),
                Some((m, u)) => ("measure", m.clone(), u.clone()),
                None => ("extra", String::new(), String::new()),
            };
            ColumnUse { header: h.clone(), how: how.into(), field, unit }
        })
        .collect()
}

/// Where a row is, as warnings name it: `labs.csv, line 12`.
#[derive(Clone, Copy, Default)]
pub struct Line<'a> {
    pub file: &'a str,
    pub line: u64,
}

impl std::fmt::Display for Line<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.file)
        } else {
            write!(f, "{}, line {}", self.file, self.line)
        }
    }
}

/// A cell of a measurement column: its column's position and header, the measurement's name
/// and unit, the value.
pub struct Measure {
    pub col: usize,
    pub header: String,
    pub name: String,
    pub unit: String,
    pub value: String,
}

/// A row as read: its recognised cells, its measurement cells, the others (header, value),
/// where it is and its number in its source (its keys').
#[derive(Default)]
pub struct Row<'a> {
    pub fields: HashMap<&'static str, String>,
    pub measures: Vec<Measure>,
    pub extras: Vec<(String, String)>,
    pub line: Line<'a>,
    pub number: usize,
}

impl Row<'_> {
    pub fn get(&self, k: &str) -> &str {
        self.fields.get(k).map(|s| s.trim()).unwrap_or("")
    }
}

/// The fields that describe a bed stay (kept as a `bed_stays` row of `extras`).
const BED_STAY: &[&str] = &["care_unit", "bed", "bed_start", "bed_end", "recording_file"];
/// The fields that describe an encounter.
const VISIT: &[&str] = &["csn", "admit_time", "discharge_time", "admit_unit", "disposition"];
/// The fields that make a row an entry.
const ITEM: &[&str] = &["name", "value", "text", "file"];

/// A number, or nothing: an error when the cell is something else.
fn number(s: &str) -> Result<Option<f64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<f64>().ok().filter(|x| x.is_finite()).map(Some).ok_or_else(|| format!("not a number: {s:?}"))
}

/// `F`, `M`, or empty (any other value is kept in `extras`).
fn sex(s: &str) -> Option<&'static str> {
    match s.trim().to_lowercase().as_str() {
        "" => Some(""),
        "f" | "female" => Some("F"),
        "m" | "male" => Some("M"),
        _ => None,
    }
}

/// What a recording is, as its header says (no signal is read).
pub struct RecordingInfo {
    pub format: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub size: i64,
    pub fingerprint: String,
}

/// A recording's format, start and end (the header's clock time read in `zone`; a BedMaster
/// name's Unix time otherwise), size and fingerprint.
pub fn recording_info(p: &Path) -> anyhow::Result<RecordingInfo> {
    use recording_engine::sources::{edf, format_label, wfdb_folder};
    let (mut start, mut end) = (None, None);
    let collection = p.join("index.csv").is_file();
    if wfdb_folder::is_record(p) || collection {
        if let Ok(meta) = wfdb_folder::scan(p, &wfdb_folder::limits()) {
            if let Some(r) = meta.get("t_range").and_then(|v| v.as_list()) {
                // a header's clock time as the engine reads it (as UTC), so that it agrees with
                // the recording's index
                start = r.first().and_then(|v| v.as_f64());
                end = r.get(1).and_then(|v| v.as_f64());
            }
        }
    } else if edf::is_edf(p) {
        if let Ok(h) = edf::header(p) {
            if let Some(t) = h.start {
                start = Some(t);
                end = Some(t + h.records as f64 * h.duration);
            }
        }
    }
    if start.is_none() {
        // BedMaster's `UNIT_BED-<Unix time>` names
        start = bed_name_time(p);
    }
    Ok(RecordingInfo {
        format: format_label(p),
        start,
        end,
        // a collection: the size of its files
        size: if collection {
            wfdb_folder::fingerprint(p).iter().map(|f| f.1 as i64).sum()
        } else {
            p.metadata().map_or(0, |m| m.len() as i64)
        },
        fingerprint: db::fingerprint(p)?,
    })
}

/// The Unix time of a name `UNIT_BED-<Unix time>` (9 to 11 digits), if it is one.
fn bed_name_time(p: &Path) -> Option<f64> {
    let stem = p.file_stem()?.to_string_lossy();
    let (label, time) = stem.rsplit_once('-')?;
    ((9..=11).contains(&time.len()) && time.bytes().all(|b| b.is_ascii_digit()) && !label.is_empty())
        .then(|| time.parse::<f64>().ok())?
}

/// Whether a file is a recording the engine recognises: a WFDB record (its header), an EDF or
/// BDF file, a file an installed adapter reads.
pub fn is_recording(p: &Path) -> bool {
    p.is_file() && recording_engine::sources::kind_of(p).is_some()
}

/// Writes the rows of one source.
pub struct Writer<'a> {
    pub tx: &'a Connection,
    pub source: i64,
    pub zone: Tz,
    pub opts: &'a Options,
    /// The category of the source's entries when a row names none.
    pub category: String,
    /// The folder relative paths in the rows are read from (the table's).
    pub base: PathBuf,
    pub db_dir: PathBuf,
    persons_added: usize,
    visits_added: usize,
    entries: usize,
    recordings: usize,
    left_out: usize,
    no_issuer: usize,
}

impl<'a> Writer<'a> {
    pub fn new(
        tx: &'a Connection,
        source: i64,
        zone: Tz,
        opts: &'a Options,
        category: &str,
        base: &Path,
        db_dir: &Path,
    ) -> Writer<'a> {
        Writer {
            tx,
            source,
            zone,
            opts,
            category: category.to_string(),
            base: base.to_path_buf(),
            db_dir: db_dir.to_path_buf(),
            persons_added: 0,
            visits_added: 0,
            entries: 0,
            recordings: 0,
            left_out: 0,
            no_issuer: 0,
        }
    }

    /// The counts into the report.
    pub fn finish(self, rep: &mut SourceReport) {
        rep.persons_added += self.persons_added;
        rep.visits_added += self.visits_added;
        rep.entries += self.entries;
        rep.recordings += self.recordings;
        rep.left_out += self.left_out;
        if self.no_issuer > 0 {
            rep.warnings.insert(
                0,
                format!(
                    "{} row(s) name a patient ID without its issuer (give --issuer, or a patient_id_issuer column)",
                    self.no_issuer
                ),
            );
        }
    }

    fn key(&self, row: &Row) -> String {
        format!("{}:{}", self.source, row.number)
    }

    fn time(&self, s: &str) -> Result<Option<String>, String> {
        Ok(parse_time(s, self.zone)?.map(|t| iso(t, self.zone)))
    }

    fn extra(&self, table: &str, key: &str, name: &str, value: &str) -> rusqlite::Result<()> {
        if value.trim().is_empty() {
            return Ok(());
        }
        self.tx
            .prepare_cached(
                "INSERT INTO extras (table_name, row_key, name, value, source_id)
                 SELECT ?1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (SELECT 1 FROM extras
                   WHERE table_name = ?1 AND row_key = ?2 AND name = ?3 AND value = ?4 AND source_id = ?5)",
            )?
            .execute(params![table, key, name, value.trim(), self.source])?;
        Ok(())
    }

    fn named_by(&self, table: &str, key: &str) -> rusqlite::Result<()> {
        self.tx
            .prepare_cached("INSERT OR IGNORE INTO row_sources (table_name, row_key, source_id) VALUES (?1, ?2, ?3)")?
            .execute(params![table, key, self.source])?;
        Ok(())
    }

    /// One row: its patient, its encounter, then its entry or recording; what is none of them
    /// goes to `extras`.  A row that cannot be read is left out, with a warning.
    pub fn row(&mut self, row: &Row, rep: &mut SourceReport) -> anyhow::Result<()> {
        let line = row.line;
        let mut extras: Vec<(String, String)> = row.extras.clone();
        // a measurement that is no number: kept as it came
        let numbers: Vec<(&Measure, f64)> = row
            .measures
            .iter()
            .filter_map(|m| match m.value.parse::<f64>().ok().filter(|x| x.is_finite()) {
                Some(x) => Some((m, x)),
                None => {
                    extras.push((m.header.clone(), m.value.clone()));
                    None
                }
            })
            .collect();
        // the patient
        let person = match self.person(row, &mut extras, rep)? {
            Ok(p) => p,
            Err(e) => {
                rep.warn(format!("{line}: {e}"));
                self.left_out += 1;
                return Ok(());
            }
        };
        // the encounter
        let eid = row.get("encounter_id").to_string();
        let (person, visit, bed_stay) = if eid.is_empty() {
            for k in VISIT.iter().chain(BED_STAY) {
                if !row.get(k).is_empty() && *k != "csn" {
                    extras.push((k.to_string(), row.get(k).to_string()));
                }
            }
            (person, None, false)
        } else if person.is_none() && !VISIT.iter().chain(BED_STAY).any(|k| !row.get(k).is_empty()) {
            // a row that names its encounter only: a reference (the encounter is another row's)
            (self.person_of_visit(&eid)?, Some(eid.clone()), false)
        } else {
            match self.visit(row, &eid, person.as_deref(), rep)? {
                Ok((p, bed)) => (p, Some(eid.clone()), bed),
                Err(e) => {
                    rep.warn(format!("{line}: {e}"));
                    self.left_out += 1;
                    return Ok(());
                }
            }
        };
        let csn = row.get("csn");
        let has_item = ITEM.iter().any(|k| !row.get(k).is_empty()) || !numbers.is_empty();
        if !has_item {
            // a row describing its patient or encounter only: its extras with the most specific
            let (table, key) = if bed_stay {
                ("bed_stays".to_string(), self.key(row))
            } else if let Some(v) = &visit {
                ("visits".to_string(), v.clone())
            } else if let Some(p) = &person {
                ("persons".to_string(), p.clone())
            } else {
                rep.warn(format!("{line}: names no patient, encounter or entry: left out"));
                self.left_out += 1;
                return Ok(());
            };
            for (k, v) in &extras {
                self.extra(&table, &key, k, v)?;
            }
            if let Some(s) = row.fields.get("source").filter(|s| !s.trim().is_empty()) {
                self.extra(&table, &key, "source", s)?;
            }
            return Ok(());
        }
        if person.is_none() && visit.is_none() && csn.is_empty() {
            rep.warn(format!("{line}: no patient_id, mrn, encounter_id or csn: left out"));
            self.left_out += 1;
            return Ok(());
        }
        // a visit named by its account number (csn) only
        let visit = match visit {
            Some(v) => Some(v),
            None if !csn.is_empty() => self.visit_by_csn(csn)?,
            None => None,
        };
        let person = match (&person, &visit) {
            (None, Some(v)) => self.person_of_visit(v)?,
            _ => person,
        };
        // a file: a recording, or the entry's
        let mut file = String::new();
        let category = categories::named(row.get("category")).or_else(|| categories::named(&self.category));
        if !row.get("file").is_empty() {
            let given = Path::new(row.get("file"));
            let abs = db::absolute(&if given.is_relative() { self.base.join(given) } else { given.to_path_buf() });
            if !abs.exists() {
                rep.warn(format!("{line}: no such file: {}", row.get("file")));
            } else if !recording_engine::paths::allowed_file(&abs) {
                // `db-builder serve`: outside its root
                rep.warn(format!("{line}: {}: outside the folder db-builder may read: left out", row.get("file")));
            } else if let Some(note) = recording_engine::sources::wfdb_folder::signal_file_note(&abs) {
                rep.warn(format!("{line}: {note} (name it in the file column): the file left out"));
            } else if (is_recording(&abs) || abs.is_dir()) && category.is_none_or(categories::is_recording) {
                let found = if abs.is_dir() { crate::recordings::find(&abs, rep) } else { vec![abs.clone()] };
                if found.is_empty() {
                    rep.warn(format!("{line}: {}: no recording in this folder", row.get("file")));
                }
                for (i, p) in found.iter().enumerate() {
                    // (a collection the row names is one recording: the row's)
                    let key = if *p != abs { format!("{}:{}", self.key(row), i + 1) } else { self.key(row) };
                    let rec = Recording {
                        key,
                        person: person.clone(),
                        visit: visit.clone(),
                        label: row.get("name").to_string(),
                        category: category.unwrap_or("icu").to_string(),
                        time: row.get("time").to_string(),
                        end_time: row.get("end_time").to_string(),
                    };
                    if let Err(e) = self.recording(p, &rec, &extras, rep)? {
                        rep.warn(format!("{line}: {e}"));
                    }
                }
                return Ok(());
            } else {
                // any other file (a 12-lead ECG, a picture, a PDF): the entry's, and a row of
                // `recordings` of the row's category, so that a reader finds every file in one place
                file = db::stored_path(&abs, &self.db_dir);
                let cat = categories::resolve(
                    row.get("category"),
                    &row.get("domain").to_lowercase(),
                    row.get("name"),
                    &self.category,
                );
                let rec = Recording {
                    key: format!("{}:file", self.key(row)),
                    person: person.clone(),
                    visit: visit.clone(),
                    label: row.get("name").to_string(),
                    category: cat,
                    time: row.get("time").to_string(),
                    end_time: row.get("end_time").to_string(),
                };
                if let Err(e) = self.recording(&abs, &rec, &[], rep)? {
                    rep.warn(format!("{line}: {e}"));
                }
            }
        }
        // the entry
        let key = self.key(row);
        let parsed = (
            self.time(row.get("time")),
            self.time(row.get("end_time")),
            number(row.get("ref_low")),
            number(row.get("ref_high")),
        );
        let (time, end, ref_low, ref_high) = match parsed {
            (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
            (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => {
                rep.warn(format!("{line}: {e}: left out"));
                self.left_out += 1;
                return Ok(());
            }
        };
        let row_domain = row.get("domain").to_lowercase();
        let cat = categories::resolve(row.get("category"), &row_domain, row.get("name"), &self.category);
        // (a row whose file was left out, and that holds nothing else: no entry)
        let main = ["name", "value", "text"].iter().any(|k| !row.get(k).is_empty()) || !file.is_empty();
        if !main && numbers.is_empty() {
            self.left_out += 1;
            return Ok(());
        }
        if main {
            let name = if !row.get("name").is_empty() {
                row.get("name").to_string()
            } else if !file.is_empty() {
                Path::new(&file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            } else {
                categories::label(&cat).to_string()
            };
            let value = row.get("value");
            let value_num = value.parse::<f64>().ok().filter(|x| x.is_finite());
            let value_text = if value_num.is_some() { "" } else { value };
            self.entry(&Entry {
                key: &key,
                person: person.as_deref(),
                visit: visit.as_deref(),
                category: &cat,
                time: time.as_deref(),
                end_time: end.as_deref(),
                name: &name,
                code: row.get("code"),
                vocabulary: row.get("vocabulary"),
                value_num,
                value_text,
                unit: row.get("unit"),
                ref_low,
                ref_high,
                flag: row.get("flag"),
                author: row.get("author"),
                text: row.get("text"),
            })?;
            for (k, v) in [
                ("file", file.as_str()),
                ("service", row.get("service")),
                ("domain", row.get("domain")),
                ("source", row.get("source")),
            ] {
                self.extra("entries", &key, k, v)?;
            }
            if visit.is_none() {
                self.extra("entries", &key, "csn", csn)?;
            }
            for (k, v) in &extras {
                self.extra("entries", &key, k, v)?;
            }
        }
        // each number of a measurement column, an entry of its own
        for (m, x) in &numbers {
            let mkey = format!("{key}:{}", m.col + 1);
            self.entry(&Entry {
                key: &mkey,
                person: person.as_deref(),
                visit: visit.as_deref(),
                category: &cat,
                time: time.as_deref(),
                end_time: end.as_deref(),
                name: &m.name,
                value_num: Some(*x),
                unit: &m.unit,
                ..Default::default()
            })?;
            if !main && m.col == numbers[0].0.col {
                // (a row of measurements only: its other columns with its first)
                for (k, v) in &extras {
                    self.extra("entries", &mkey, k, v)?;
                }
                if visit.is_none() {
                    self.extra("entries", &mkey, "csn", csn)?;
                }
            }
        }
        Ok(())
    }

    fn entry(&mut self, e: &Entry) -> rusqlite::Result<()> {
        self.tx
            .prepare_cached(
                "INSERT INTO entries (entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary,
                   value_num, value_text, unit, ref_low, ref_high, flag, author, text, source_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            )?
            .execute(params![
                e.key,
                e.person,
                e.visit,
                e.category,
                e.time,
                e.end_time,
                e.name,
                e.code,
                e.vocabulary,
                e.value_num,
                e.value_text,
                e.unit,
                e.ref_low,
                e.ref_high,
                e.flag,
                e.author,
                e.text,
                self.source
            ])?;
        self.entries += 1;
        Ok(())
    }

    /// The row's patient: found by its identifiers, else added (the key is the patient ID when no
    /// other person has it, else `<source>:<row>`).  What the row says of them fills what is
    /// empty; a different value is not written over (a warning).  `Err` inside: the row cannot
    /// be read (a date of birth that is no date).
    pub fn person(
        &mut self,
        row: &Row,
        extras: &mut Vec<(String, String)>,
        rep: &mut SourceReport,
    ) -> anyhow::Result<Result<Option<String>, String>> {
        let (pid, mrn) = (row.get("patient_id"), row.get("mrn"));
        if pid.is_empty() && mrn.is_empty() {
            for k in ["patient_id_issuer", "mrn_authority", "last_name", "first_name", "birth_date", "sex"] {
                if !row.get(k).is_empty() {
                    extras.push((k.to_string(), row.get(k).to_string()));
                }
            }
            return Ok(Ok(None));
        }
        let or = |a: &str, b: &str| if a.is_empty() { b.trim().to_string() } else { a.to_string() };
        let issuer = if pid.is_empty() { String::new() } else { or(row.get("patient_id_issuer"), &self.opts.issuer) };
        let authority =
            if mrn.is_empty() { String::new() } else { or(row.get("mrn_authority"), &self.opts.mrn_authority) };
        if !pid.is_empty() && issuer.is_empty() {
            self.no_issuer += 1;
        }
        let birth = match parse_date(row.get("birth_date")) {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        let sx = match sex(row.get("sex")) {
            Some(s) => s,
            None => {
                extras.push(("sex".into(), row.get("sex").to_string()));
                ""
            }
        };
        let tx = self.tx;
        let by_pid = |pid: &str, issuer: &str| -> rusqlite::Result<Option<String>> {
            tx.prepare_cached(
                "SELECT person_key FROM persons WHERE patient_id = ?1 AND patient_id_issuer = ?2 LIMIT 1",
            )?
            .query_row([pid, issuer], |r| r.get(0))
            .optional()
        };
        // MRNs are compared without their surrounding spaces, ignoring letter case
        let by_mrn = |mrn: &str, auth: &str| -> rusqlite::Result<Option<(String, String)>> {
            tx.prepare_cached(
                "SELECT person_key, patient_id FROM persons WHERE lower(mrn) = lower(?1) AND mrn_authority = ?2
                 ORDER BY patient_id = '' DESC LIMIT 1",
            )?
            .query_row([mrn, auth], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
        };
        let mut found = if pid.is_empty() { None } else { by_pid(pid, &issuer)? };
        if found.is_none() && !mrn.is_empty() {
            match by_mrn(mrn, &authority)? {
                // a person known by the MRN only: this is its patient ID
                Some((k, p)) if p.is_empty() && !pid.is_empty() => {
                    tx.execute(
                        "UPDATE persons SET patient_id = ?1, patient_id_issuer = ?2 WHERE person_key = ?3",
                        params![pid, issuer, k],
                    )?;
                    found = Some(k);
                }
                Some((k, _)) if pid.is_empty() => found = Some(k),
                Some(_) => {
                    rep.warn(format!(
                        "{}: its MRN is another person's too (db-builder check lists such MRNs)",
                        row.line
                    ));
                }
                None => {}
            }
        }
        let key = match found {
            Some(k) => {
                let have: [String; 6] = tx
                    .prepare_cached(
                        "SELECT mrn, mrn_authority, last_name, first_name, birth_date, sex FROM persons WHERE person_key = ?1",
                    )?
                    .query_row([&k], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?]))?;
                let given = [mrn, authority.as_str(), row.get("last_name"), row.get("first_name"), birth.as_str(), sx];
                let cols = ["mrn", "mrn_authority", "last_name", "first_name", "birth_date", "sex"];
                for i in 0..6 {
                    if given[i].is_empty() {
                        continue;
                    }
                    if have[i].is_empty() {
                        // an MRN and its authority go together
                        if i == 1 && !have[0].is_empty() && !mrn.eq_ignore_ascii_case(&have[0]) {
                            continue;
                        }
                        tx.execute(
                            &format!("UPDATE persons SET {} = ?1 WHERE person_key = ?2", cols[i]),
                            params![given[i], k],
                        )?;
                    } else if !have[i].eq_ignore_ascii_case(given[i]) {
                        rep.warn(format!(
                            "{}: the person's {} differs from an earlier row's: the earlier one is kept",
                            row.line, cols[i]
                        ));
                    }
                }
                k
            }
            None => {
                let taken = |k: &str| -> rusqlite::Result<bool> {
                    tx.prepare_cached("SELECT 1 FROM persons WHERE person_key = ?1")?.exists([k])
                };
                let k = if !pid.is_empty() && !taken(pid)? { pid.to_string() } else { self.key(row) };
                tx.prepare_cached(
                    "INSERT INTO persons (person_key, patient_id, patient_id_issuer, mrn, mrn_authority, last_name, first_name, birth_date, sex)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?
                .execute(params![k, pid, issuer, mrn, authority, row.get("last_name"), row.get("first_name"), birth, sx])?;
                self.persons_added += 1;
                k
            }
        };
        self.named_by("persons", &key)?;
        Ok(Ok(Some(key)))
    }

    /// The row's encounter (`visit_key` = its `encounter_id`): added, or what is empty of it
    /// filled; its account number and disposition in `extras`, and the row's bed stay as a
    /// `bed_stays` row.  Returns the visit's person and whether the row was a bed stay.
    fn visit(
        &mut self,
        row: &Row,
        eid: &str,
        person: Option<&str>,
        rep: &mut SourceReport,
    ) -> anyhow::Result<Result<(Option<String>, bool), String>> {
        let times = (
            self.time(row.get("admit_time")),
            self.time(row.get("discharge_time")),
            self.time(row.get("bed_start")),
            self.time(row.get("bed_end")),
        );
        let (start, end, bed_start, bed_end) = match times {
            (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
            (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => return Ok(Err(e)),
        };
        let (admit_unit, care_unit) = (row.get("admit_unit"), row.get("care_unit"));
        let unit = if admit_unit.is_empty() { care_unit } else { admit_unit };
        let icu = categories::is_icu_unit(admit_unit) || categories::is_icu_unit(care_unit);
        let tx = self.tx;
        let have: Option<Option<String>> = tx
            .prepare_cached("SELECT person_key FROM visits WHERE visit_key = ?1")?
            .query_row([eid], |r| r.get(0))
            .optional()?;
        let owner = match have {
            None => {
                tx.prepare_cached(
                    "INSERT INTO visits (visit_key, person_key, start_time, end_time, unit, icu, source_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?
                .execute(params![eid, person, start, end, unit, icu as i64, self.source])?;
                self.visits_added += 1;
                person.map(str::to_string)
            }
            Some(had) => {
                if let (Some(h), Some(p)) = (&had, person) {
                    if h != p {
                        rep.warn(format!(
                            "{}: encounter {eid} is another person's: its entry is this row's person's",
                            row.line
                        ));
                    }
                }
                tx.prepare_cached(
                    "UPDATE visits SET person_key = COALESCE(person_key, ?2), start_time = COALESCE(start_time, ?3),
                       end_time = COALESCE(end_time, ?4), unit = CASE WHEN unit = '' THEN ?5 ELSE unit END,
                       icu = MAX(icu, ?6) WHERE visit_key = ?1",
                )?
                .execute(params![eid, person, start, end, unit, icu as i64])?;
                person.map(str::to_string).or(had)
            }
        };
        self.named_by("visits", eid)?;
        self.extra("visits", eid, "csn", row.get("csn"))?;
        self.extra("visits", eid, "disposition", row.get("disposition"))?;
        if !admit_unit.is_empty() && !care_unit.is_empty() {
            self.extra("visits", eid, "admit_unit", admit_unit)?;
        }
        let bed_stay = BED_STAY.iter().any(|k| !row.get(k).is_empty());
        if bed_stay {
            let key = self.key(row);
            self.extra("bed_stays", &key, "visit_key", eid)?;
            for (k, v) in [
                ("care_unit", care_unit),
                ("bed", row.get("bed")),
                ("bed_start", bed_start.as_deref().unwrap_or("")),
                ("bed_end", bed_end.as_deref().unwrap_or("")),
                ("recording_file", row.get("recording_file")),
            ] {
                self.extra("bed_stays", &key, k, v)?;
            }
        }
        Ok(Ok((owner, bed_stay)))
    }

    fn visit_by_csn(&self, csn: &str) -> rusqlite::Result<Option<String>> {
        self.tx
            .prepare_cached(
                "SELECT row_key FROM extras WHERE table_name = 'visits' AND name = 'csn' AND value = ?1 LIMIT 1",
            )?
            .query_row([csn], |r| r.get(0))
            .optional()
    }

    fn person_of_visit(&self, visit: &str) -> rusqlite::Result<Option<String>> {
        Ok(self
            .tx
            .prepare_cached("SELECT person_key FROM visits WHERE visit_key = ?1")?
            .query_row([visit], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    }

    /// A recording row (`Err` inside: why it is not added).
    pub fn recording(
        &mut self,
        p: &Path,
        rec: &Recording,
        extras: &[(String, String)],
        rep: &mut SourceReport,
    ) -> anyhow::Result<Result<(), String>> {
        let file = db::stored_path(p, &self.db_dir);
        let already: Option<String> = self
            .tx
            .prepare_cached("SELECT recording_key FROM recordings WHERE file = ?1 LIMIT 1")?
            .query_row([&file], |r| r.get(0))
            .optional()?;
        if let Some(k) = already {
            return Ok(Err(format!("{file} is in the database already (recording {k}): not added again")));
        }
        let info = recording_info(p)?;
        let given = (self.time(&rec.time), self.time(&rec.end_time));
        let (start, end) = match (info.start, info.end) {
            (Some(a), b) => (Some(iso(a, self.zone)), b.map(|b| iso(b, self.zone))),
            (None, _) => match given {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => return Ok(Err(e)),
            },
        };
        if wfdb_unsupported(p) && info.start.is_none() {
            rep.warn(format!("{file}: {}: its start and end not read", recording_engine::sources::NO_WFDB));
        }
        let label = if rec.label.is_empty() {
            p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            rec.label.clone()
        };
        self.tx
            .prepare_cached(
                "INSERT INTO recordings (recording_key, person_key, visit_key, file, format, start_time, end_time, label,
                   category, size, fingerprint, source_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?
            .execute(params![
                rec.key,
                rec.person,
                rec.visit,
                file,
                info.format,
                start,
                end,
                label,
                rec.category,
                info.size,
                info.fingerprint,
                self.source
            ])?;
        for (k, v) in extras {
            self.extra("recordings", &rec.key, k, v)?;
        }
        self.recordings += 1;
        Ok(Ok(()))
    }
}

/// Whether this build cannot read a WFDB record's header times (built without `wfdb`).
fn wfdb_unsupported(p: &Path) -> bool {
    recording_engine::sources::unreadable(p).is_some()
}

/// A recording to write: its key, person, visit, label, category, and the times a table gives
/// (used when its header gives none).
pub struct Recording {
    pub key: String,
    pub person: Option<String>,
    pub visit: Option<String>,
    pub label: String,
    pub category: String,
    pub time: String,
    pub end_time: String,
}

/// An entry to write.
#[derive(Default)]
struct Entry<'a> {
    key: &'a str,
    person: Option<&'a str>,
    visit: Option<&'a str>,
    category: &'a str,
    time: Option<&'a str>,
    end_time: Option<&'a str>,
    name: &'a str,
    code: &'a str,
    vocabulary: &'a str,
    value_num: Option<f64>,
    value_text: &'a str,
    unit: &'a str,
    ref_low: Option<f64>,
    ref_high: Option<f64>,
    flag: &'a str,
    author: &'a str,
    text: &'a str,
}

/// After every write: rows that name their visit only get its person, and entries that name a
/// visit by its account number (csn) get that visit, once the visit is known.
pub fn settle(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "UPDATE entries SET visit_key = (
             SELECT v.row_key FROM extras v JOIN extras e ON e.value = v.value
             WHERE v.table_name = 'visits' AND v.name = 'csn' AND e.table_name = 'entries' AND e.name = 'csn'
               AND e.row_key = entries.entry_key LIMIT 1)
         WHERE visit_key IS NULL AND entry_key IN (SELECT row_key FROM extras WHERE table_name = 'entries' AND name = 'csn');
         UPDATE entries SET person_key = (SELECT v.person_key FROM visits v WHERE v.visit_key = entries.visit_key)
         WHERE person_key IS NULL AND visit_key IS NOT NULL;
         UPDATE recordings SET person_key = (SELECT v.person_key FROM visits v WHERE v.visit_key = recordings.visit_key)
         WHERE person_key IS NULL AND visit_key IS NOT NULL;",
    )
}
