//! What names in criteria text refer to: a database's column, or its results of that name, or a connected database's measurement.

use super::*;

/// The database's name as people call it: its name, else its file's, without the extension.
pub(super) fn db_name(name: &str, origin: &str) -> String {
    let n = if name.trim().is_empty() { origin } else { name.trim() };
    let f = n.rsplit(['/', '\\']).next().unwrap_or(n);
    match f.rsplit_once('.') {
        Some((a, ext))
            if !a.is_empty() && ["csv", "tsv", "txt", "xlsx", "xls"].contains(&ext.to_lowercase().as_str()) =>
        {
            a.to_string()
        }
        _ => f.to_string(),
    }
}

/// A database row that reached a patient: (patient, time, cells, line, its item's category).
pub(super) type DbRow = (i64, Option<f64>, Vec<String>, i64, String);

/// The rows of a database that `filter` keeps (a condition on `r`, the row; its parameters
/// `?2`, `?3`, … in `args`; "" for every row), each with the patient it reached, its time and
/// one of its cells (column `pos`, the only one in its cells): the filter runs in the
/// database, not over every row in Medorama.
pub(super) fn db_cell_where(
    c: &Connection,
    import: i64,
    pos: usize,
    filter: &str,
    args: Vec<crate::sql::Value>,
) -> anyhow::Result<Vec<DbRow>> {
    rows_resolved(c, import, Some(pos), filter, args)
}

/// A database's rows of these patients only (a cohort's members): the rows that reached an item read
/// by the items' patients (`ehr_items` by patient, `import_rows` by item), not the whole table;
/// the others (patients, encounters, recordings, waiting items) resolved as [`db_cell_where`] does.
pub(super) fn db_rows_of(c: &Connection, import: i64, patients: &[i64]) -> anyhow::Result<Vec<DbRow>> {
    // from the items to their rows: SQLite takes the tables in the order written with CROSS JOIN
    // (else it may walk the whole database for each batch of patients); PostgreSQL plans it so
    let join = if c.dialect() == crate::sql::Dialect::Sqlite { "CROSS JOIN" } else { "JOIN" };
    let mut out: Vec<DbRow> = c
        .query_in(
            &format!(
                "SELECT i.patient, COALESCE(i.time, (SELECT t_start FROM recordings WHERE id = r.recording)), r.cells, r.line,
                        COALESCE(i.category, '')
                 FROM ehr_items i {join} import_rows r ON r.item = i.id
                 WHERE i.patient IN ({{}}) AND r.import_id = {import} AND r.status NOT IN ('held', 'invalid')"
            ),
            patients,
            |r| {
                let cells: String = r.get(2)?;
                Ok((r.get(0)?, r.get(1)?, serde_json::from_str(&cells).unwrap_or_default(), r.get(3)?, r.get(4)?))
            },
        )?;
    let want: std::collections::HashSet<i64> = patients.iter().copied().collect();
    out.extend(
        rows_resolved(
            c,
            import,
            None,
            " AND (r.item IS NULL OR r.item IN (SELECT id FROM ehr_items WHERE patient IS NULL))",
            vec![],
        )?
        .into_iter()
        .filter(|r| want.contains(&r.0)),
    );
    out.sort_by_key(|r| r.3);
    Ok(out)
}

/// A database's rows (those `filter` keeps) and the patient each is about; with `cell`, only
/// that cell of each.
fn rows_resolved(
    c: &Connection,
    import: i64,
    cell: Option<usize>,
    filter: &str,
    args: Vec<crate::sql::Value>,
) -> anyhow::Result<Vec<DbRow>> {
    let cells = match cell {
        Some(pos) => format!("COALESCE({}, '')", c.json_cell("r.cells", pos)),
        None => "r.cells".to_string(),
    };
    let mut st = c.prepare(&format!(
        "SELECT {cells}, r.key_patient, r.key_mrn, r.key_encounter,
                (SELECT patient FROM ehr_items WHERE id = r.item),
                (SELECT e.patient FROM recording_links l JOIN encounters e ON e.id = l.encounter WHERE l.recording = r.recording LIMIT 1),
                COALESCE((SELECT time FROM ehr_items WHERE id = r.item), (SELECT t_start FROM recordings WHERE id = r.recording)),
                r.line, COALESCE((SELECT category FROM ehr_items WHERE id = r.item), '')
         FROM import_rows r WHERE r.import_id = ?1 AND r.status NOT IN ('held', 'invalid'){filter} ORDER BY r.line"
    ))?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, String, Option<i64>, Option<i64>, Option<f64>, i64, String)> = st
        .query_map(
            crate::sql::params_from_iter(std::iter::once(crate::sql::Value::Integer(import)).chain(args)),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
        )?
        .collect::<Result<_, _>>()?;
    // rows that reached no item nor recording (patients, encounters): their keys, all at once
    let keys: Vec<(String, String, String)> =
        rows.iter().filter(|r| r.4.or(r.5).is_none()).map(|r| (r.1.clone(), r.2.clone(), r.3.clone())).collect();
    let mut found = crate::databases::resolve_patients(c, &keys)?.into_iter();
    let mut out = vec![];
    for (cells, _, _, _, ip, rp, t, line, cat) in rows {
        let p = match ip.or(rp) {
            Some(p) => Some(p),
            None => found.next().flatten(),
        };
        if let Some(p) = p {
            let cells = if cell.is_some() { vec![cells] } else { serde_json::from_str(&cells).unwrap_or_default() };
            out.push((p, t, cells, line, cat));
        }
    }
    Ok(out)
}

/// The active table databases and their columns: (id, name, [(pos, header, maps_to)]).
pub(super) fn db_columns(c: &Connection) -> anyhow::Result<Vec<(i64, String, Vec<(usize, String, String)>)>> {
    let mut st = c.prepare("SELECT id, name, origin FROM imports WHERE state = 'active' ORDER BY id")?;
    let dbs: Vec<(i64, String, String)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut st = c.prepare("SELECT pos, header, maps_to FROM import_columns WHERE import_id = ?1 ORDER BY pos")?;
    let mut out = vec![];
    for (id, name, origin) in dbs {
        let cols = st
            .query_map([id], |r| Ok((r.get::<_, i64>(0)? as usize, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        out.push((id, db_name(&name, &origin), cols));
    }
    Ok(out)
}

/// Whether a column's header is `name`: the header itself, or its name without the unit.
pub(super) fn names_column(header: &str, name: &str) -> bool {
    header.trim().eq_ignore_ascii_case(name.trim())
        || crate::tables::measure_column(header).is_some_and(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
}

/// Whether a column's name begins with `name` as whole words (`LAD` for `LAD stenosis (%)`).
pub(super) fn starts_column(header: &str, name: &str) -> bool {
    let full = crate::tables::measure_column(header)
        .map(|(n, _)| n)
        .unwrap_or_else(|| header.trim().to_string())
        .to_lowercase();
    let n = name.trim().to_lowercase();
    !n.is_empty() && full.len() > n.len() && full.starts_with(&n) && full[n.len()..].starts_with(' ')
}

/// What names in criteria text refer to (expr.rs): a database's column, or EHR items of that
/// name in one database; never several databases at once.  The databases' columns and the
/// connected databases' names are read once per resolver (one text parsed), not per name.
pub fn resolver<'a>(
    c: &'a Connection,
    user: &User,
) -> impl FnMut(Option<&str>, &str) -> anyhow::Result<Criterion> + 'a {
    let user = user.clone();
    let mut columns: Option<Vec<(i64, String, Vec<(usize, String, String)>)>> = None;
    let mut sources: Option<Vec<crate::sources::Source>> = None;
    let mut names: HashMap<i64, Result<std::sync::Arc<Vec<(String, String, bool)>>, String>> = HashMap::new();
    move |qual: Option<&str>, name: &str| {
        if columns.is_none() {
            columns = Some(db_columns(c)?);
        }
        if sources.is_none() {
            sources = Some(crate::sources::list(c)?);
        }
        let dbs = columns.as_ref().unwrap();
        let pick: Vec<&(i64, String, Vec<(usize, String, String)>)> = match qual {
            Some(q) => {
                let q = q.trim().to_lowercase();
                let exact: Vec<_> = dbs
                    .iter()
                    .filter(|d| d.1.to_lowercase() == q || d.0.to_string() == q.trim_start_matches('#'))
                    .collect();
                if !exact.is_empty() {
                    exact
                } else {
                    dbs.iter().filter(|d| d.1.to_lowercase().starts_with(&q)).collect()
                }
            }
            None => dbs.iter().collect(),
        };
        if let Some(q) = qual {
            if pick.is_empty() {
                anyhow::bail!(
                    "no database {q:?} (databases: {})",
                    dbs.iter().map(|d| d.1.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
            if name.is_empty() {
                if pick.len() > 1 {
                    anyhow::bail!(
                        "{q:?} names several databases: {}",
                        pick.iter().map(|d| d.1.as_str()).collect::<Vec<_>>().join(", ")
                    );
                }
                return Ok(Criterion { kind: "database".into(), import: Some(pick[0].0), ..Default::default() });
            }
        }
        // a column of a database
        let mut hits: Vec<(i64, String, String)> = vec![];
        for (id, dname, cols) in &pick {
            if let Some((_, h, m)) = cols
                .iter()
                .find(|(_, h, m)| m.is_empty() && names_column(h, name))
                .or_else(|| cols.iter().find(|(_, h, m)| !m.is_empty() && h.trim().eq_ignore_ascii_case(name.trim())))
                .or_else(|| {
                    // the first words of exactly one column's name: `LAD` for `LAD stenosis (%)`
                    let mut starts = cols.iter().filter(|(_, h, m)| m.is_empty() && starts_column(h, name));
                    let first = starts.next();
                    if starts.next().is_none() {
                        first
                    } else {
                        None
                    }
                })
            {
                if !crate::databases::column_visible(&user, h, m) {
                    anyhow::bail!("the column {h:?} of {dname} needs the PHI permission");
                }
                hits.push((*id, dname.clone(), h.clone()));
            }
        }
        if hits.len() > 1 {
            anyhow::bail!(
                "{name:?} is a column of several databases ({}): write which, e.g. {}.{}",
                hits.iter().map(|h| h.1.as_str()).collect::<Vec<_>>().join(", "),
                hits[0].1,
                if name.contains(' ') { format!("\"{name}\"") } else { name.to_string() }
            );
        }
        if let Some((id, _, h)) = hits.pop() {
            return Ok(Criterion { kind: "column".into(), import: Some(id), name: h, ..Default::default() });
        }
        // EHR items of that name, in one database
        let mut st = c.prepare(
            "SELECT DISTINCT v.import_id FROM ehr_items i JOIN ehr_provenance v ON v.item = i.id
             JOIN imports m ON m.id = v.import_id AND m.state = 'active'
             WHERE lower(i.name) = lower(?1) AND i.value_num IS NOT NULL",
        )?;
        let ids: Vec<i64> = st.query_map([name.trim()], |r| r.get(0))?.collect::<Result<_, _>>()?;
        // (database, the measurement's name there)
        let mut hits: Vec<(i64, String)> =
            ids.into_iter().filter(|i| pick.iter().any(|d| d.0 == *i)).map(|i| (i, name.trim().to_string())).collect();
        // … and in the connected databases: a name they hold, else the one name containing it
        let mut unread = vec![];
        for src in sources.as_ref().unwrap().iter().filter(|x| pick.iter().any(|d| d.0 == x.import_id)) {
            let names = match names.entry(src.import_id).or_insert_with(|| src.names().map_err(|e| e.to_string())) {
                Ok(n) => n.clone(),
                Err(e) => {
                    unread.push(format!("{}: {e}", src.name));
                    continue;
                }
            };
            let w = name.trim().to_lowercase();
            let numeric: Vec<&String> = names.iter().filter(|n| n.2).map(|n| &n.1).collect();
            if let Some(n) = numeric.iter().find(|n| n.to_lowercase() == w) {
                hits.push((src.import_id, n.to_string()));
                continue;
            }
            let mut like: Vec<&String> = numeric.into_iter().filter(|n| n.to_lowercase().contains(&w)).collect();
            like.dedup();
            match like.len() {
                0 => {}
                1 => hits.push((src.import_id, like[0].clone())),
                n => anyhow::bail!(
                    "{name:?} names {n} measurements in {}: {}{}; write which, e.g. {}.\"{}\"",
                    src.name,
                    like.iter().take(6).map(|x| format!("“{x}”")).collect::<Vec<_>>().join(", "),
                    if n > 6 { " …" } else { "" },
                    src.name,
                    like[0]
                ),
            }
        }
        let label = |i: i64| dbs.iter().find(|d| d.0 == i).map(|d| d.1.clone()).unwrap_or_default();
        match hits.len() {
            0 => anyhow::bail!(
                "nothing is named {name:?}{}{}",
                qual.map(|q| format!(" in {q}")).unwrap_or_default(),
                if unread.is_empty() { String::new() } else { format!(" ({} could not be read)", unread.join("; ")) }
            ),
            1 => Ok(Criterion {
                kind: "measurement".into(),
                name: hits[0].1.clone(),
                import: Some(hits[0].0),
                ..Default::default()
            }),
            _ => anyhow::bail!(
                "{name:?} is in several databases ({}): write which, e.g. {}.{name}",
                hits.iter().map(|h| label(h.0)).collect::<Vec<_>>().join(", "),
                label(hits[0].0)
            ),
        }
    }
}

/// Criteria text as the structured query (POST cohorts/parse shows it while it is written).
pub fn parse_text(c: &Connection, user: &User, text: &str) -> anyhow::Result<Expr> {
    let mut r = resolver(c, user);
    crate::expr::parse(text, &mut r)
}
