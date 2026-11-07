//! The Databases tab (docs/administration.md, "Databases"): every imported database as a table,
//! organised by database rather than by patient.  A table import shows its rows as they were
//! (its own columns, kept at import: import_rows); a folder of recordings shows its
//! recordings.  Each row names the patient it reached and links what it holds to its viewer.
//! Users without the PHI permission see neither the identifying columns (names, MRNs, dates of
//! birth, CSNs) nor the unmapped ones (which may hold anything) but measurement columns
//! (`QT (ms)`), nor the text of notes.

use std::collections::{HashMap, HashSet};

use crate::sql::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::audit;
use crate::users::User;

/// Catalog columns that identify a patient.
const PHI_COLUMNS: &[&str] = &["mrn", "mrn_authority", "last_name", "first_name", "birth_date", "csn"];

#[derive(Debug, Serialize)]
pub struct Database {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub origin: String,
    pub source: String,
    pub username: String,
    pub time: f64,
    pub rows: i64,
    pub patients: i64,
    /// Categories of the items it holds.
    pub categories: Vec<String>,
    /// Rows held back for review, items waiting for their patient.
    pub held: i64,
    pub waiting: i64,
    /// Whether its rows can be shown (table imports since rows were kept; folders of recordings).
    pub viewable: bool,
    /// A connected database: its kind and database (`OMOP · PostgreSQL`); its data are read live.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
}

/// Whether a user sees a database's column: everyone sees the mapped columns that do not
/// identify and the measurement columns; the others need the PHI permission.
pub fn column_visible(user: &User, header: &str, maps_to: &str) -> bool {
    user.phi
        || (!maps_to.is_empty() && !PHI_COLUMNS.contains(&maps_to))
        || (maps_to.is_empty() && crate::tables::measure_column(header).is_some())
}

/// The patients a database holds data about.
pub fn patients_of(c: &Connection, import: i64) -> anyhow::Result<HashSet<i64>> {
    let mut st = c.prepare(
        "SELECT i.patient FROM ehr_provenance v JOIN ehr_items i ON i.id = v.item WHERE v.import_id = ?1 AND i.patient IS NOT NULL
         UNION SELECT row_id FROM provenance WHERE import_id = ?1 AND kind = 'patient'
         UNION SELECT e.patient FROM provenance v JOIN encounters e ON e.id = v.row_id WHERE v.import_id = ?1 AND v.kind = 'encounter'
         UNION SELECT e.patient FROM provenance v JOIN recording_links l ON l.recording = v.row_id
               JOIN encounters e ON e.id = l.encounter WHERE v.import_id = ?1 AND v.kind = 'recording'",
    )?;
    let ids = st.query_map([import], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// The databases in use (removed ones are gone), newest first.
pub fn list(c: &Connection) -> anyhow::Result<Vec<Database>> {
    let mut st = c.prepare(
        "SELECT i.id, i.kind, i.name, i.origin, i.source, COALESCE(u.username, ''), i.time,
                (SELECT COUNT(*) FROM import_rows r WHERE r.import_id = i.id),
                (SELECT COUNT(*) FROM provenance p WHERE p.import_id = i.id AND p.kind = 'recording'),
                (SELECT COUNT(*) FROM held_rows h WHERE h.import_id = i.id AND h.state = 'open'),
                (SELECT COUNT(*) FROM ehr_provenance v JOIN ehr_items e ON e.id = v.item WHERE v.import_id = i.id AND e.patient IS NULL),
                (SELECT group_concat(DISTINCT e.category) FROM ehr_provenance v JOIN ehr_items e ON e.id = v.item WHERE v.import_id = i.id)
         FROM imports i LEFT JOIN users u ON u.id = i.user_id WHERE i.state = 'active' ORDER BY i.id DESC",
    )?;
    let rows: Vec<(i64, String, String, String, String, String, f64, i64, i64, i64, i64, Option<String>)> = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut out = vec![];
    for (id, kind, name, origin, source, username, time, kept, recs, held, waiting, cats) in rows {
        let folder = kind == "recordings";
        let mut categories: Vec<String> =
            cats.unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(String::from).collect();
        if folder || recs > 0 {
            let mut st = c.prepare(
                "SELECT DISTINCT r.category FROM provenance p JOIN recordings r ON r.id = p.row_id WHERE p.import_id = ?1 AND p.kind = 'recording'",
            )?;
            for k in st.query_map([id], |r| r.get::<_, String>(0))? {
                let k = k?;
                if !categories.contains(&k) {
                    categories.push(k);
                }
            }
        }
        let order: Vec<&str> = crate::categories::journey_keys().collect();
        categories.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(99));
        let connection = match kind.as_str() {
            "connection" => crate::sources::of_import(c, id)?.map(|s| {
                format!(
                    "{} · {}",
                    if s.kind == "omop" { "OMOP" } else { "SQL" },
                    if s.dialect() == crate::sql::Dialect::Sqlite { "SQLite" } else { "PostgreSQL" }
                )
            }),
            _ => None,
        };
        out.push(Database {
            id,
            name: if name.is_empty() { origin.rsplit(['/', '\\']).next().unwrap_or(&origin).to_string() } else { name },
            kind,
            origin,
            source,
            username,
            time,
            rows: if folder { recs } else { kept },
            patients: patients_of(c, id)?.len() as i64,
            categories,
            held,
            waiting,
            viewable: folder || kept > 0,
            connection,
        });
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Column {
    pub pos: usize,
    pub header: String,
    /// The catalog's column it was mapped to ("" when none).
    pub maps_to: String,
}

/// What a row's file opens in.
#[derive(Debug, Serialize)]
pub struct Link {
    /// `ecg`, `echo`, `pacs`, `vitalstream`, or `file` (shown by the page itself)
    pub viewer: String,
    pub name: String,
    /// For the ECG, echo and imaging viewers: the API address of the file, and its kind.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub src: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// What the viewer needs that the file does not say (patient.rs `viewer_hints`).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub hints: std::collections::BTreeMap<String, String>,
    /// For VitalStream: the recording's path and start.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t0: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Row {
    pub line: i64,
    /// One per visible column (hidden ones are left out, as the columns are).
    pub cells: Vec<String>,
    pub status: String,
    pub patient: Option<i64>,
    pub patient_id: String,
    /// The patient page's entry the row became (`item:…`, `rec:…`).
    pub entry: String,
    pub link: Option<Link>,
    /// The position (in `cells`) of the cell the link belongs to: the file's column.
    pub link_cell: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub database: Database,
    pub columns: Vec<Column>,
    /// Columns not shown to this user (identifying or unmapped), by header.
    pub hidden: Vec<String>,
    pub total: usize,
    pub rows: Vec<Row>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Query {
    /// Words every shown row contains (in any visible cell, ignoring case).
    pub q: String,
    /// Per column (its `pos`): text the cell contains.
    pub filters: Vec<(usize, String)>,
    /// A column's `pos` to sort by, and the direction.
    pub sort: Option<usize>,
    pub desc: bool,
    pub limit: usize,
    pub offset: usize,
}

/// One database in use, as a table page needs it: its name, kind and whether its rows can be
/// shown (not the counts `list` computes for every database).
fn one(c: &Connection, id: i64) -> anyhow::Result<Database> {
    let (kind, name, origin, source, username, time, kept): (String, String, String, String, String, f64, bool) = c
        .query_row(
            "SELECT i.kind, i.name, i.origin, i.source, COALESCE(u.username, ''), i.time,
                    EXISTS (SELECT 1 FROM import_rows r WHERE r.import_id = i.id)
             FROM imports i LEFT JOIN users u ON u.id = i.user_id WHERE i.id = ?1 AND i.state = 'active'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no database {id}"))?;
    let folder = kind == "recordings";
    Ok(Database {
        id,
        name: if name.is_empty() { origin.rsplit(['/', '\\']).next().unwrap_or(&origin).to_string() } else { name },
        kind,
        origin,
        source,
        username,
        time,
        rows: 0,
        patients: 0,
        categories: vec![],
        held: 0,
        waiting: 0,
        viewable: folder || kept,
        connection: None,
    })
}

/// A database's rows, one page.  In Privacy Mode (`gate`), every column is listed, the cells of
/// those that identify replaced by placeholders (gate.rs `column_mask`); searches, filters and
/// sorting use the cells as the session is shown them (through the gate: pseudonyms, each
/// patient's dates moved back, notes without the patient's names), so that they cannot reveal
/// what is withheld.  Elsewhere they run in the database, over the columns the user sees.
pub fn rows(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    id: i64,
    q: &Query,
) -> anyhow::Result<Page> {
    let db = one(c, id)?;
    if db.kind == "recordings" {
        return recording_rows(c, user, gate, db, q);
    }
    if db.kind == "connection" {
        anyhow::bail!(
            "a connected database: its data are read from it when a patient's page opens or a cohort is searched"
        );
    }
    if !db.viewable {
        anyhow::bail!("this database was imported before its rows were kept: import it again to see them here");
    }
    let mut st = c.prepare("SELECT pos, header, maps_to FROM import_columns WHERE import_id = ?1 ORDER BY pos")?;
    let all: Vec<Column> = st
        .query_map([id], |r| Ok(Column { pos: r.get::<_, i64>(0)? as usize, header: r.get(1)?, maps_to: r.get(2)? }))?
        .collect::<Result<_, _>>()?;
    let mask = |col: &Column| crate::gate::column_mask(user, gate, &col.header, &col.maps_to);
    let hidden: Vec<String> = if gate.is_some() {
        vec![]
    } else {
        all.iter().filter(|col| mask(col).is_some()).map(|col| col.header.clone()).collect()
    };
    let columns: Vec<Column> =
        if gate.is_some() { all } else { all.into_iter().filter(|col| mask(col).is_none()).collect() };
    let masks: Vec<Option<&'static str>> = columns.iter().map(mask).collect();
    let text_pos = columns.iter().position(|col| col.maps_to == "text");
    let file_pos = columns.iter().position(|col| col.maps_to == "file");
    // the filter and the order, over shown columns only (a search must not reveal what is
    // withheld: nor notes' text, for users without the PHI permission)
    let searchable = |col: &Column| mask(col).is_none() && (user.reads_notes() || col.maps_to != "text");
    // a column's filter: on a column searched (in Privacy Mode, any column listed: a placeholder
    // is what its cells are shown as)
    let filters: Vec<(usize, String)> = q
        .filters
        .iter()
        .filter(|(pos, v)| {
            columns.iter().any(|col| col.pos == *pos && (gate.is_some() || searchable(col))) && !v.trim().is_empty()
        })
        .map(|(pos, v)| (*pos, v.trim().to_lowercase()))
        .collect();
    let sort = q.sort.filter(|p| columns.iter().any(|col| col.pos == *p && mask(col).is_none()));
    let words: Vec<String> = q.q.split_whitespace().map(str::to_lowercase).collect();
    // in Privacy Mode, a search or an order reads the rows as shown (here, not in the database)
    let shown_mode = gate.is_some() && (!words.is_empty() || !filters.is_empty() || sort.is_some());
    let pg = c.dialect() == crate::sql::Dialect::Postgres;
    let cell = |pos: usize| {
        if pg {
            format!("COALESCE(CAST(r.cells AS jsonb) ->> {pos}, '')")
        } else {
            format!("COALESCE(json_extract(r.cells, '$[{pos}]'), '')")
        }
    };
    let mut wh = vec!["r.import_id = ?1".to_string()];
    let mut args: Vec<String> = vec![];
    if !shown_mode {
        for w in &words {
            args.push(like_any(w));
            let any: Vec<String> = columns
                .iter()
                .filter(|col| searchable(col))
                .map(|col| format!("lower({}) LIKE ?{} ESCAPE '\\'", cell(col.pos), args.len() + 1))
                .collect();
            wh.push(if any.is_empty() { "1 = 0".into() } else { format!("({})", any.join(" OR ")) });
        }
        for (pos, v) in &filters {
            args.push(like_any(v));
            wh.push(format!("lower({}) LIKE ?{} ESCAPE '\\'", cell(*pos), args.len() + 1));
        }
    }
    let filter = wh.join(" AND ");
    let mut values: Vec<crate::sql::types::Value> = vec![id.into()];
    values.extend(args.iter().cloned().map(crate::sql::types::Value::from));
    let limit = q.limit.clamp(1, 500);
    let total: i64 = if shown_mode {
        0
    } else {
        c.query_row(
            &format!("SELECT COUNT(*) FROM import_rows r WHERE {filter}"),
            crate::sql::params_from_iter(values.iter()),
            |r| r.get(0),
        )?
    };
    // an order by a column: the one rule (numbers as numbers, then text; empty cells last) on
    // either database, here: the lines of the page found from the column's cells alone
    let mut lines: Option<Vec<i64>> = None;
    if let (Some(p), false) = (sort, shown_mode) {
        let mut st = c.prepare(&format!("SELECT r.line, {} FROM import_rows r WHERE {filter}", cell(p)))?;
        let mut all: Vec<(i64, String)> = st
            .query_map(crate::sql::params_from_iter(values.iter()), |r| {
                Ok((r.get(0)?, r.get::<_, String>(1)?.to_lowercase()))
            })?
            .collect::<Result<_, _>>()?;
        all.sort_by(|a, b| cell_order(&a.1, &b.1, q.desc).then(a.0.cmp(&b.0)));
        lines = Some(all.into_iter().skip(q.offset).take(limit).map(|x| x.0).collect());
    }
    let page = match (&lines, shown_mode) {
        (Some(l), _) if l.is_empty() => " AND 1 = 0 ORDER BY r.line".to_string(),
        (Some(l), _) => {
            format!(
                " AND r.line IN ({}) ORDER BY r.line",
                l.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")
            )
        }
        (None, true) => " ORDER BY r.line".into(),
        (None, false) => format!(" ORDER BY r.line LIMIT {limit} OFFSET {}", q.offset),
    };
    let sql = format!(
        "SELECT r.line, r.cells, r.status, r.key_patient, r.key_mrn, r.key_encounter, r.item, r.recording,
                i.patient, i.category, i.file_path
         FROM import_rows r LEFT JOIN ehr_items i ON i.id = r.item WHERE {filter}{page}"
    );
    let mut st = c.prepare(&sql)?;
    let raw: Vec<RawRow> = st
        .query_map(crate::sql::params_from_iter(values.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    // in the order found
    let raw: Vec<RawRow> = match &lines {
        Some(l) => {
            let mut by: HashMap<i64, RawRow> = raw.into_iter().map(|r| (r.0, r)).collect();
            l.iter().filter_map(|x| by.remove(x)).collect()
        }
        None => raw,
    };
    let patients = row_patients(c, &raw)?;
    let ids: Vec<i64> = patients.iter().flatten().copied().collect::<HashSet<_>>().into_iter().collect();
    let patient_ids: HashMap<i64, String> = c
        .query_in("SELECT id, patient_id FROM patients WHERE id IN ({})", &ids, |r| Ok((r.get(0)?, r.get(1)?)))?
        .into_iter()
        .collect();
    // each row's cells as this handler sends them (the gate, in Privacy Mode, after)
    let mut identities: HashMap<i64, (Vec<String>, String)> = HashMap::new();
    let mut cells_of = |r: &RawRow, patient: Option<i64>| -> Vec<String> {
        let cells: Vec<String> = serde_json::from_str(&r.1).unwrap_or_default();
        let mut shown: Vec<String> = columns
            .iter()
            .zip(&masks)
            .map(|(col, m)| crate::gate::cell(None, *m, &cells.get(col.pos).cloned().unwrap_or_default()))
            .collect();
        let note = r.9.as_deref() == Some("note");
        if note && !user.reads_notes() {
            if let Some(t) = text_pos {
                shown[t] = if shown[t].is_empty() {
                    String::new()
                } else {
                    "(the text of notes needs the PHI permission)".into()
                };
            }
        }
        // a note's text, in Privacy Mode: the patient's own names and date of birth replaced
        // (identifiers and dates then go through the gate with the rest of the answer)
        if let (Some(_), Some(t), Some(p)) = (gate, text_pos, patient) {
            if masks[t].is_none() && !shown[t].is_empty() {
                let (names, birth) =
                    identities.entry(p).or_insert_with(|| crate::gate::Restricted::identity_words(c, p));
                let mut v = serde_json::Value::String(std::mem::take(&mut shown[t]));
                crate::gate::scrub_names(&mut v, names, birth);
                shown[t] = v.as_str().unwrap_or_default().to_string();
            }
        }
        shown
    };
    let mut prepared: Vec<(RawRow, Option<i64>, Vec<String>)> = raw
        .into_iter()
        .zip(patients)
        .map(|(r, p)| {
            let cells = cells_of(&r, p);
            (r, p, cells)
        })
        .collect();
    let total = if let (true, Some(g)) = (shown_mode, gate) {
        // the cells as the session sees them: through the gate, by the row's patient
        let mut kept: Vec<(Vec<String>, (RawRow, Option<i64>, Vec<String>))> = vec![];
        for row in prepared {
            let v = crate::search::shown(g, &serde_json::json!({ "patient": row.1, "cells": row.2 }));
            let seen: Vec<String> = crate::search::strings(&v["cells"]).into_iter().map(|x| x.to_lowercase()).collect();
            let hit = |pos: usize, w: &str| {
                columns.iter().position(|col| col.pos == pos).and_then(|i| seen.get(i)).is_some_and(|x| x.contains(w))
            };
            let any = |w: &str| columns.iter().filter(|col| searchable(col)).any(|col| hit(col.pos, w));
            if words.iter().all(|w| any(w)) && filters.iter().all(|(pos, v)| hit(*pos, v)) {
                kept.push((seen, row));
            }
        }
        if let Some(p) = sort.and_then(|p| columns.iter().position(|col| col.pos == p)) {
            kept.sort_by(|a, b| {
                let o = cell_order(&a.0[p], &b.0[p], q.desc);
                o.then(a.1 .0 .0.cmp(&b.1 .0 .0))
            });
        }
        let total = kept.len();
        prepared = kept.into_iter().skip(q.offset).take(limit).map(|k| k.1).collect();
        total as i64
    } else {
        total
    };
    let mut out = vec![];
    for ((line, _, status, _, _, _, item, rec, _, icat, ifile), patient, shown) in prepared {
        let patient_id: String = patient.and_then(|p| patient_ids.get(&p).cloned()).unwrap_or_default();
        let mut link = None;
        let mut entry = String::new();
        if let Some(i) = item {
            entry = format!("item:{i}");
            let file = ifile.unwrap_or_default();
            if let (false, Some(p)) = (file.is_empty(), patient) {
                let kind = crate::patient::file_kind(&file);
                let real = std::path::Path::new(&file)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                // in Privacy Mode a file is named by its token (its name may identify)
                let name = if gate.is_some() { crate::gate::file_token(0, &real) } else { real };
                let url = format!("patients/{p}/file/{i}");
                let viewer = crate::patient::viewer_for(icat.as_deref().unwrap_or(""), kind).unwrap_or("file");
                link = Some(Link {
                    viewer: viewer.into(),
                    src: if viewer == "file" { url } else { crate::patient::viewer_src(&url, kind, &name) },
                    kind: kind.into(),
                    hints: if viewer == "file" { Default::default() } else { crate::patient::viewer_hints(c, i)? },
                    name,
                    path: String::new(),
                    t0: None,
                });
            }
        } else if let Some(r) = rec {
            entry = format!("rec:{r}");
            if let Some(x) = crate::recordings::get(c, r)? {
                link = Some(Link {
                    viewer: "vitalstream".into(),
                    name: x.name.clone(),
                    src: String::new(),
                    kind: x.kind.clone(),
                    hints: Default::default(),
                    path: x.path.clone(),
                    t0: x.t_start.or(x.name_time),
                });
            }
        }
        out.push(Row {
            line,
            cells: shown,
            status,
            patient,
            patient_id,
            entry,
            link_cell: link.as_ref().and(file_pos),
            link,
        });
    }
    if user.phi {
        audit(c, Some(user), "view_database", &id.to_string(), &q.q);
    } else if user.privacy && (!words.is_empty() || !filters.is_empty()) {
        audit(c, Some(user), "privacy_search", &format!("database {id}"), &q.q);
    }
    Ok(Page { database: db, columns, hidden, total: total as usize, rows: out })
}

/// A cell as a CSV export writes it: text a spreadsheet would run as a formula (`=`, `+`, `-`,
/// `@`, a tab or a return first) behind a `'`, so that it opens as text; numbers as they are.
pub(crate) fn csv_cell(v: &str) -> std::borrow::Cow<'_, str> {
    let formula = v.starts_with(['=', '+', '-', '@', '\t', '\r']);
    if formula && v.trim().parse::<f64>().is_err() {
        std::borrow::Cow::Owned(format!("'{v}"))
    } else {
        std::borrow::Cow::Borrowed(v)
    }
}

/// A row of `import_rows` as [`rows`] reads it: line, cells, status, its keys (patient, MRN,
/// encounter), its item and recording, the item's patient, category and file.
type RawRow = (
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// Each row's patient: its item's, its recording's, else by its keys.
fn row_patients(c: &Connection, raw: &[RawRow]) -> anyhow::Result<Vec<Option<i64>>> {
    let recs: Vec<i64> = raw.iter().filter(|r| r.8.is_none()).filter_map(|r| r.7).collect();
    let rec_patient: HashMap<i64, i64> = c
        .query_in(
            "SELECT l.recording, MIN(e.patient) FROM recording_links l JOIN encounters e ON e.id = l.encounter
             WHERE l.recording IN ({}) GROUP BY l.recording",
            &recs,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect();
    let keys: Vec<(String, String, String)> = raw
        .iter()
        .filter(|r| r.8.is_none() && r.7.is_none())
        .map(|r| (r.3.clone(), r.4.clone(), r.5.clone()))
        .collect();
    let mut by_keys = resolve_patients(c, &keys)?.into_iter();
    Ok(raw
        .iter()
        .map(|r| match (r.8, r.7) {
            (Some(p), _) => Some(p),
            (None, Some(rec)) => rec_patient.get(&rec).copied(),
            (None, None) => by_keys.next().flatten(),
        })
        .collect())
}

/// `%text%` for LIKE, its `%`, `_` and `\` taken as written.
fn like_any(s: &str) -> String {
    format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// The order of two cells (lower-cased) as a table sorts them: numbers as numbers, before text;
/// empty cells last, whichever the direction.
fn cell_order(a: &str, b: &str, desc: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    let o = match (a.parse::<f64>().ok().filter(|x| x.is_finite()), b.parse::<f64>().ok().filter(|x| x.is_finite())) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    };
    if desc {
        o.reverse()
    } else {
        o
    }
}

/// A database as CSV: its columns under their imported names, its rows as imported.  For users
/// with the PHI permission, everything; in Privacy Mode (`gate`), the same shape, the cells of
/// identifying columns as placeholders and the others through the gate (pseudonyms, dates moved
/// back).  `(file name, text)`.
pub fn export_csv(
    c: &Connection,
    user: &User,
    id: i64,
    gate: Option<&crate::gate::Restricted>,
) -> anyhow::Result<(String, String)> {
    let db = one(c, id)?;
    let shown_as = User { phi: user.phi && gate.is_none(), ..user.clone() };
    let user = &shown_as;
    if db.kind == "recordings" || !db.viewable {
        anyhow::bail!("this database has no rows to export (imported before rows were kept)");
    }
    let mut st = c.prepare("SELECT pos, header, maps_to FROM import_columns WHERE import_id = ?1 ORDER BY pos")?;
    let cols: Vec<Column> = st
        .query_map([id], |r| Ok(Column { pos: r.get::<_, i64>(0)? as usize, header: r.get(1)?, maps_to: r.get(2)? }))?
        .collect::<Result<_, _>>()?;
    let masked: Vec<(&Column, Option<&'static str>)> = cols
        .iter()
        .map(|col| (col, crate::gate::column_mask(user, gate, &col.header, &col.maps_to)))
        .filter(|(_, m)| gate.is_some() || m.is_none())
        .collect();
    let mut w = csv::Writer::from_writer(vec![]);
    w.write_record(masked.iter().map(|(col, _)| col.header.as_str()))?;
    let mut st = c.prepare(
        "SELECT r.cells, COALESCE(i.category, ''), i.patient FROM import_rows r LEFT JOIN ehr_items i ON i.id = r.item
         WHERE r.import_id = ?1 ORDER BY r.line",
    )?;
    for row in
        st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)))?
    {
        let (cells, cat, patient) = row?;
        let cells: Vec<String> = serde_json::from_str(&cells).unwrap_or_default();
        // each patient's dates by their own offset
        let own = gate.zip(patient).map(|(g, p)| g.for_patient(p));
        let gate = own.as_ref().or(gate);
        w.write_record(masked.iter().map(|(col, m)| {
            let v = cells.get(col.pos).cloned().unwrap_or_default();
            if !user.reads_notes() && cat == "note" && col.maps_to == "text" {
                return String::new();
            }
            let out = match (gate, patient) {
                (Some(g), Some(p)) if m.is_none() && cat == "note" && col.maps_to == "text" => {
                    let (names, birth) = crate::gate::Restricted::identity_words(c, p);
                    g.note(&v, &names, &birth)
                }
                _ => crate::gate::cell(gate, *m, &v),
            };
            csv_cell(&out).into_owned()
        }))?;
    }
    audit(c, Some(user), if user.phi { "phi_export" } else { "export_database" }, &id.to_string(), &db.name);
    let stem = db.name.rsplit_once('.').map_or(db.name.as_str(), |(a, _)| a).to_string();
    Ok((format!("{stem}{}.csv", if gate.is_some() { "-privacy" } else { "" }), String::from_utf8(w.into_inner()?)?))
}

/// The patient a row's keys name (encounter, patient ID, MRN), if the catalog knows it (and
/// shows it: not a patient or an encounter only an import still loading holds).
pub fn resolve_patient(c: &Connection, patient_id: &str, mrn: &str, encounter: &str) -> anyhow::Result<Option<i64>> {
    let patient = crate::patient::patient_shown("patient");
    if !encounter.is_empty() {
        if let Some(p) = c
            .query_row(
                &format!(
                    "SELECT patient FROM encounters WHERE encounter_id = ?1 AND {} AND {patient}",
                    crate::patient::encounter_shown("id")
                ),
                [encounter],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(Some(p));
        }
    }
    if !patient_id.is_empty() {
        if let Some(p) = c
            .query_row(
                &format!("SELECT id FROM patients WHERE patient_id = ?1 AND {}", crate::patient::patient_shown("id")),
                [patient_id],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(Some(p));
        }
    }
    if !mrn.is_empty() {
        return Ok(c
            .query_row(
                &format!("SELECT patient FROM patient_mrns WHERE mrn_key = lower(trim(?1)) AND {patient} LIMIT 1"),
                [mrn],
                |r| r.get(0),
            )
            .optional()?);
    }
    Ok(None)
}

/// The patients of many rows at once, as `resolve_patient` finds each: (patient ID, MRN,
/// encounter ID) per row.
pub fn resolve_patients(c: &Connection, keys: &[(String, String, String)]) -> anyhow::Result<Vec<Option<i64>>> {
    let distinct = |f: &dyn Fn(&(String, String, String)) -> String| -> Vec<String> {
        let s: std::collections::BTreeSet<String> = keys.iter().map(f).filter(|k| !k.is_empty()).collect();
        s.into_iter().collect()
    };
    let map = |sql: &str, values: Vec<String>| -> anyhow::Result<HashMap<String, i64>> {
        Ok(c.query_in(sql, &values, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.into_iter().collect())
    };
    // (not a patient or an encounter only an import still loading holds)
    let patient = crate::patient::patient_shown("patient");
    let by_enc = map(
        &format!(
            "SELECT encounter_id, patient FROM encounters WHERE encounter_id IN ({{}}) AND {} AND {patient}",
            crate::patient::encounter_shown("id")
        ),
        distinct(&|k| k.2.clone()),
    )?;
    let by_pid = map(
        &format!(
            "SELECT patient_id, id FROM patients WHERE patient_id IN ({{}}) AND {}",
            crate::patient::patient_shown("id")
        ),
        distinct(&|k| k.0.clone()),
    )?;
    let by_mrn = map(
        &format!(
            "SELECT mrn_key, MIN(patient) FROM patient_mrns WHERE mrn_key IN ({{}}) AND {patient} GROUP BY mrn_key"
        ),
        distinct(&|k| crate::identity::mrn_key(&k.1)),
    )?;
    Ok(keys
        .iter()
        .map(|(pid, mrn, enc)| {
            by_enc.get(enc).or_else(|| by_pid.get(pid)).or_else(|| by_mrn.get(&crate::identity::mrn_key(mrn))).copied()
        })
        .collect())
}

/// A folder of recordings as a table: one row per recording.  In Privacy Mode (`gate`), searched,
/// filtered and sorted as the session is shown the cells (through the gate).
fn recording_rows(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    db: Database,
    q: &Query,
) -> anyhow::Result<Page> {
    let heads = ["Recording", "Kind", "Category", "Start", "End", "Unit", "Bed", "Status", "Encounter"];
    let columns: Vec<Column> = heads
        .iter()
        .enumerate()
        .map(|(i, h)| Column { pos: i, header: h.to_string(), maps_to: String::new() })
        .collect();
    let mut st = c.prepare(
        "SELECT r.id FROM provenance p JOIN recordings r ON r.id = p.row_id WHERE p.import_id = ?1 AND p.kind = 'recording' ORDER BY r.id",
    )?;
    let ids: Vec<i64> = st.query_map([db.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let zone = crate::util::tz("UTC");
    let mut all = vec![];
    for id in ids {
        let Some(x) = crate::recordings::get(c, id)? else { continue };
        let (enc, patient): (Option<String>, Option<i64>) = c
            .query_row(
                "SELECT e.encounter_id, e.patient FROM recording_links l JOIN encounters e ON e.id = l.encounter WHERE l.recording = ?1 LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((None, None));
        let category: String = c.query_row("SELECT category FROM recordings WHERE id = ?1", [id], |r| r.get(0))?;
        let t = |v: Option<f64>| v.map(|t| crate::util::fmt_local(t, zone)).unwrap_or_default();
        let cells = vec![
            x.name.clone(),
            x.kind.clone(),
            crate::categories::label(&category).to_string(),
            t(x.t_start.or(x.name_time)),
            t(x.t_end),
            x.unit.clone(),
            x.bed.clone(),
            serde_json::to_value(x.status).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
            enc.unwrap_or_default(),
        ];
        let patient_id: String = match patient {
            Some(p) => c.query_row("SELECT patient_id FROM patients WHERE id = ?1", [p], |r| r.get(0))?,
            None => String::new(),
        };
        // what the session sees: in Privacy Mode, the cells through the gate, as the row leaves
        let seen: Vec<String> = match gate {
            Some(g) => crate::search::strings(
                &crate::search::shown(g, &serde_json::json!({ "patient": patient, "cells": cells }))["cells"],
            ),
            None => cells.clone(),
        };
        let seen: Vec<String> = seen.iter().map(|x| x.to_lowercase()).collect();
        let words = q.q.to_lowercase();
        if !words.split_whitespace().all(|w| seen.iter().any(|c| c.contains(w))) {
            continue;
        }
        if !q.filters.iter().all(|(p, v)| seen.get(*p).is_some_and(|c| c.contains(&v.trim().to_lowercase()))) {
            continue;
        }
        all.push((
            seen,
            Row {
                line: id,
                cells,
                status: "recording".into(),
                patient,
                patient_id,
                entry: format!("rec:{id}"),
                link: Some(Link {
                    viewer: "vitalstream".into(),
                    name: x.name.clone(),
                    src: String::new(),
                    kind: x.kind.clone(),
                    hints: Default::default(),
                    path: x.path.clone(),
                    t0: x.t_start.or(x.name_time),
                }),
                link_cell: Some(0),
            },
        ));
    }
    if let Some(p) = q.sort.filter(|p| *p < heads.len()) {
        all.sort_by(|a, b| a.0[p].cmp(&b.0[p]));
        if q.desc {
            all.reverse();
        }
    }
    if user.privacy && !user.phi && (!q.q.trim().is_empty() || !q.filters.is_empty()) {
        audit(c, Some(user), "privacy_search", &format!("database {}", db.id), &q.q);
    }
    let total = all.len();
    let rows = all.into_iter().skip(q.offset).take(q.limit.clamp(1, 500)).map(|r| r.1).collect();
    Ok(Page { database: db, columns, hidden: vec![], total, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_write_no_formula() {
        assert_eq!(csv_cell("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
        assert_eq!(csv_cell("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_cell("+1+2"), "'+1+2");
        assert_eq!(csv_cell("-30"), "-30", "a number stays a number");
        assert_eq!(csv_cell("Sinus rhythm"), "Sinus rhythm");
        let mut seen = vec![];
        for (a, b) in [("", "1"), ("2", "10"), ("x", "2"), ("", "")] {
            seen.push(cell_order(a, b, false));
        }
        use std::cmp::Ordering::*;
        assert_eq!(seen, [Greater, Less, Greater, Equal], "empty cells last, numbers as numbers, before text");
    }
}
