//! A folder of recordings (`db-builder recordings`): one `recordings` row per recording found in
//! it and its subfolders (a WFDB record by its header, an EDF or BDF file, a file an installed
//! adapter reads), with its patient from a patients table (its file, its patient) or from a
//! pattern of the files' names (`{patient}_*.edf`), and its start and end from its header.  The
//! folder is one source: added again unchanged, nothing is done; changed (a file added, removed
//! or changed, another table or pattern), it replaces the earlier one.

use crate::add::{Line, Options, Recording, Row, SourceReport, Writer};
use crate::db;
use crate::tables::{self, column_map, guess_mapping};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the patients of a folder's recordings come from.
#[derive(Clone, Default)]
pub struct Patients {
    /// A table of file and patient (`--patients`).
    pub table: Option<PathBuf>,
    /// A pattern of the files' names (`--pattern`).
    pub pattern: Option<String>,
}

/// The recordings in `dir` and its subfolders, in order: a WFDB collection by its folder (one
/// with an `index.csv`: docs/wfdb-collection.md), a WFDB record by its header (the segments of a
/// multi-segment record are not records of their own), an EDF or BDF file, a file an installed
/// adapter reads.  Hidden entries are skipped, and symbolic links not followed.
pub fn find(dir: &Path, rep: &mut SourceReport) -> Vec<PathBuf> {
    let mut out = vec![];
    walk(dir, &mut out, rep);
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, rep: &mut SourceReport) {
    use recording_engine::sources::wfdb_folder;
    // a collection is one recording, its records parts of it
    if dir.join("index.csv").is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(it) = std::fs::read_dir(dir) else {
        rep.warn(format!("{}: cannot read the folder", dir.display()));
        return;
    };
    let mut entries: Vec<PathBuf> = it
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    entries.sort();
    // the segments of the multi-segment records here
    let segments: Vec<String> = entries
        .iter()
        .filter(|p| wfdb_folder::is_record(p))
        .filter_map(|p| wfdb_folder::segment_table(&p.with_extension("")))
        .flatten()
        .map(|(name, _)| name)
        .collect();
    let mut subfolders = vec![];
    for e in entries {
        if e.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(true) {
            rep.warn(format!("{}: a symbolic link, not followed", e.display()));
        } else if e.is_dir() {
            subfolders.push(e);
        } else if wfdb_folder::is_record(&e) {
            let stem = e.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if !segments.contains(&stem) {
                out.push(e);
            }
        } else if crate::add::is_recording(&e) {
            out.push(e);
        }
    }
    for d in subfolders {
        walk(&d, out, rep);
    }
}

/// A path as a key to match a table's file names by: `/` between folders, lower case.
fn norm(s: &str) -> String {
    let s = s.trim().replace('\\', "/").to_lowercase();
    s.strip_prefix("./").unwrap_or(&s).to_string()
}

/// The keys a recording is looked up by in a table: its path below the folder, its absolute
/// path, its file name, each with and without its extension.
fn keys_of(p: &Path, dir: &Path) -> Vec<String> {
    let rel = db::stored_path(p, dir);
    let abs = p.to_string_lossy().to_string();
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut out = vec![];
    for k in [rel, abs, name] {
        let n = norm(&k);
        if let Some((stem, _)) = n.rsplit_once('.').filter(|(s, _)| !s.is_empty() && !s.ends_with('/')) {
            let stem = stem.to_string();
            out.push(n);
            out.push(stem);
        } else {
            out.push(n);
        }
    }
    out
}

/// A pattern of file names: `*` any text, `{patient}` the patient ID (any text but `/`), other
/// characters themselves (letter case ignored).  Matched against the file's name, or against its
/// path below the folder when the pattern holds a `/`.
pub struct Pattern {
    parts: Vec<Part>,
    whole_path: bool,
}

enum Part {
    Text(String),
    Any,
    Patient,
}

impl Pattern {
    pub fn parse(s: &str) -> anyhow::Result<Pattern> {
        let mut parts = vec![];
        let mut rest = s;
        let mut text = String::new();
        let mut patient = false;
        while let Some(c) = rest.chars().next() {
            if rest.starts_with("{patient}") {
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                parts.push(Part::Patient);
                patient = true;
                rest = &rest["{patient}".len()..];
                continue;
            }
            if c == '*' {
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                parts.push(Part::Any);
            } else {
                text.extend(c.to_lowercase());
            }
            rest = &rest[c.len_utf8()..];
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        if !patient {
            anyhow::bail!("the pattern {s:?} has no {{patient}} (for example {{patient}}_*.edf)");
        }
        Ok(Pattern { parts, whole_path: s.contains('/') })
    }

    /// The patient ID a recording's name gives, if it matches.
    pub fn patient(&self, p: &Path, dir: &Path) -> Option<String> {
        let target = if self.whole_path {
            db::stored_path(p, dir)
        } else {
            p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        };
        let chars: Vec<char> = target.chars().collect();
        let lower: Vec<char> = target.to_lowercase().chars().collect();
        if chars.len() != lower.len() {
            return None;
        }
        let mut found = None;
        if self.matches(0, &lower, 0, &mut found) {
            found.map(|(a, b): (usize, usize)| chars[a..b].iter().collect::<String>()).filter(|s| !s.is_empty())
        } else {
            None
        }
    }

    fn matches(&self, i: usize, s: &[char], at: usize, found: &mut Option<(usize, usize)>) -> bool {
        let Some(part) = self.parts.get(i) else { return at == s.len() };
        match part {
            Part::Text(t) => {
                let t: Vec<char> = t.chars().collect();
                s.len() >= at + t.len() && s[at..at + t.len()] == t[..] && self.matches(i + 1, s, at + t.len(), found)
            }
            Part::Any | Part::Patient => {
                for end in at..=s.len() {
                    if matches!(part, Part::Patient) && s[at..end].contains(&'/') {
                        break;
                    }
                    if self.matches(i + 1, s, end, found) {
                        if matches!(part, Part::Patient) {
                            *found = Some((at, end));
                        }
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// Add the recordings in `folder` to the database, in one transaction (rolled back with
/// `opts.dry_run`: the report says what would be added).
pub fn add_folder(
    c: &mut Connection,
    db_path: &Path,
    folder: &Path,
    patients: &Patients,
    opts: &Options,
) -> anyhow::Result<SourceReport> {
    let dir = db::absolute(folder);
    if !dir.is_dir() {
        anyhow::bail!("{}: not a folder", folder.display());
    }
    if db::absolute(db_path).starts_with(&dir) {
        anyhow::bail!("{}: the database would be written into the folder it reads", dir.display());
    }
    if patients.table.is_none() && patients.pattern.is_none() {
        anyhow::bail!("say where the recordings' patients are: --patients TABLE, or --pattern PATTERN");
    }
    let pattern = patients.pattern.as_deref().map(Pattern::parse).transpose()?;
    let zone = crate::add::zone_of(c)?;
    let db_dir = db::folder_of(db_path);
    let order = if opts.day_first { crate::util::DateOrder::DayFirst } else { crate::util::DateOrder::MonthFirst };
    crate::util::with_date_order(order, || {
        let mut rep =
            SourceReport { file: dir.to_string_lossy().to_string(), kind: "recordings".into(), ..Default::default() };
        opts.progress.file(&rep.file);
        let found = find(&dir, &mut rep);
        // what a preview shows of each recording: its file, and the patient and encounter found
        rep.columns = ["file", "patient_id", "encounter_id"]
            .iter()
            .map(|f| crate::add::ColumnUse {
                header: f.to_string(),
                how: "field".into(),
                field: f.to_string(),
                unit: "".into(),
            })
            .collect();
        // the table: its rows by file
        let table = match &patients.table {
            Some(t) => Some(read_patients(&db::absolute(t), opts)?),
            None => None,
        };
        // the source's fingerprint: the files, the table and the pattern
        let mut h = Sha256::new();
        for p in &found {
            h.update(db::stored_path(p, &dir).as_bytes());
            h.update(db::fingerprint(p)?.as_bytes());
        }
        if let Some(t) = &patients.table {
            h.update(db::fingerprint(&db::absolute(t))?.as_bytes());
        }
        h.update(patients.pattern.as_deref().unwrap_or("").as_bytes());
        let fingerprint = db::hex(&h.finalize());
        let tx = c.transaction()?;
        let before = db::source_of(&tx, &rep.file)?;
        if let Some((id, _)) = before.iter().find(|(_, f)| *f == fingerprint) {
            rep.already = Some(*id);
            return Ok(rep);
        }
        for (id, _) in before {
            db::remove_source(&tx, id)?;
            rep.replaced.push(id);
        }
        let source = db::add_source(&tx, &rep.file, "recordings", &fingerprint)?;
        rep.source = Some(source);
        let base =
            patients.table.as_ref().and_then(|t| db::absolute(t).parent().map(Path::to_path_buf)).unwrap_or_default();
        let mut w = Writer::new(&tx, source, zone, opts, &opts.category, &base, &db_dir);
        let table_name = patients.table.as_ref().and_then(|t| t.file_name()).map(|n| n.to_string_lossy().to_string());
        let mut used = vec![false; table.as_ref().map_or(0, |t| t.rows.len())];
        let (mut without, mut n) = (0usize, 0usize);
        for p in &found {
            n += 1;
            opts.progress.row();
            let name = db::stored_path(p, &dir);
            // the row of the table that names it, else the pattern
            let at = table.as_ref().and_then(|t| keys_of(p, &dir).iter().find_map(|k| t.by_file.get(k).copied()));
            let mut row = Row { line: Line { file: &name, line: 0 }, number: n, ..Default::default() };
            if let (Some(i), Some(t)) = (at, &table) {
                used[i] = true;
                row.fields = t.rows[i].0.clone();
                row.extras = t.rows[i].1.clone();
                row.line = Line { file: table_name.as_deref().unwrap_or(""), line: t.rows[i].2 };
            } else if let Some(pid) = pattern.as_ref().and_then(|pat| pat.patient(p, &dir)) {
                row.fields.insert("patient_id", pid);
            }
            opts.rules.apply(&mut row.fields);
            let mut extras = row.extras.clone();
            let person = match w.person(&row, &mut extras, &mut rep)? {
                Ok(p) => p,
                Err(e) => {
                    rep.warn(format!("{}: {e}: its patient left out", row.line));
                    None
                }
            };
            let visit = Some(row.get("encounter_id").to_string()).filter(|e| !e.is_empty());
            if person.is_none() && visit.is_none() {
                without += 1;
            }
            if rep.sample.len() < crate::add::SAMPLE_ROWS {
                let pid = [row.get("patient_id"), row.get("mrn")].into_iter().find(|s| !s.is_empty()).unwrap_or("");
                rep.sample.push(vec![name.clone(), pid.to_string(), visit.clone().unwrap_or_default()]);
            }
            let rec = Recording {
                key: format!("{source}:{n}"),
                person,
                visit,
                label: row.get("name").to_string(),
                category: crate::categories::named(row.get("category"))
                    .or_else(|| crate::categories::named(&opts.category))
                    .unwrap_or("icu")
                    .to_string(),
                time: row.get("time").to_string(),
                end_time: row.get("end_time").to_string(),
            };
            if let Err(e) = w.recording(p, &rec, &extras, &mut rep)? {
                rep.warn(format!("{name}: {e}"));
            }
        }
        if without > 0 {
            rep.warnings.insert(
                0,
                format!("{without} recording(s) without a patient: no row of the table or the pattern names them"),
            );
        }
        let unused = used.iter().filter(|u| !**u).count();
        if unused > 0 {
            rep.warnings.insert(0, format!("{unused} row(s) of the patients table name no recording of the folder"));
        }
        if let Some(t) = &table {
            for w in &t.warnings {
                rep.warn(w.clone());
            }
        }
        rep.rows = n;
        tx.execute("UPDATE sources SET rows = ?1 WHERE id = ?2", rusqlite::params![n as i64, source])?;
        w.finish(&mut rep);
        if !opts.dry_run {
            crate::add::settle(&tx)?;
            db::bump(&tx)?;
            tx.commit()?;
        }
        Ok(rep)
    })
}

/// A row of a patients table: its recognised cells, the others (header, value), its line.
type TableRow = (HashMap<&'static str, String>, Vec<(String, String)>, u64);

/// A patients table: its rows (recognised cells, the others, line), and each row by the keys
/// of its file.
struct PatientsTable {
    rows: Vec<TableRow>,
    by_file: HashMap<String, usize>,
    warnings: Vec<String>,
}

fn read_patients(path: &Path, opts: &Options) -> anyhow::Result<PatientsTable> {
    let (headers, rows) = tables::rows_of(path)?;
    let mapping = if opts.mapping.is_empty() { guess_mapping(&headers) } else { opts.mapping.clone() };
    let mapping: HashMap<String, String> = mapping.into_iter().filter(|(_, h)| !h.is_empty()).collect();
    let (maps, mut warnings) = column_map(&headers, &mapping, &tables::columns());
    let file_col = ["file", "recording_file"].iter().find_map(|k| maps.iter().position(|m| m == k));
    let Some(file_col) = file_col else {
        anyhow::bail!(
            "{}: the patients table has no file column (file, path, filename, recording_file ...)",
            path.display()
        );
    };
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut out = PatientsTable { rows: vec![], by_file: HashMap::new(), warnings: vec![] };
    for r in rows {
        let (line, cells) = r?;
        if cells.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        // as the path rules make it (a table written before the data were moved)
        let file = opts.rules.path(cells.get(file_col).map(|s| s.trim()).unwrap_or(""));
        let file = file.as_str();
        if file.is_empty() {
            warnings.push(format!("line {line}: no file"));
            continue;
        }
        let mut fields = HashMap::new();
        let mut extras = vec![];
        for (i, c) in cells.iter().enumerate() {
            if i == file_col || c.trim().is_empty() {
                continue;
            }
            match maps.get(i) {
                Some(k) if !k.is_empty() => {
                    fields.insert(*k, c.trim().to_string());
                }
                _ => extras.push((headers.get(i).cloned().unwrap_or_default(), c.trim().to_string())),
            }
        }
        let i = out.rows.len();
        out.rows.push((fields, extras, line));
        let mut keys = vec![norm(file)];
        // a path relative to the table's folder
        let given = Path::new(file);
        let abs = db::absolute(&if given.is_relative() { base.join(given) } else { given.to_path_buf() });
        keys.push(norm(&abs.to_string_lossy()));
        for k in keys {
            if let Some(j) = out.by_file.insert(k.clone(), i) {
                if j != i {
                    warnings.push(format!("line {line}: {file} is named by an earlier row too: this row is used"));
                }
            }
        }
    }
    out.warnings = warnings;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let p = Pattern::parse("{patient}_*.edf").unwrap();
        let dir = Path::new("/data");
        assert_eq!(p.patient(Path::new("/data/P0001_night1.EDF"), dir).as_deref(), Some("P0001"));
        assert_eq!(p.patient(Path::new("/data/P0001.hea"), dir), None);
        let p = Pattern::parse("{patient}/*").unwrap();
        assert_eq!(p.patient(Path::new("/data/S12/rec.hea"), dir).as_deref(), Some("S12"));
        assert!(Pattern::parse("*.edf").is_err());
    }
}
