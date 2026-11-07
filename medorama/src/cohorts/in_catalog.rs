//! A criterion in the catalog: SQL over the imported databases.

use super::*;

/// The patients the catalog says meet one criterion (before `not`), each with a reason.
pub(super) fn meets_catalog(
    c: &Connection,
    user: &User,
    timezone: &str,
    k: &Criterion,
    cache: &Cache,
) -> anyhow::Result<HashMap<i64, String>> {
    let zone = tz(timezone);
    let mut out: HashMap<i64, String> = HashMap::new();
    let mut add = |p: i64, why: String| {
        out.entry(p).or_insert(why);
    };
    let dates = Dates::of(c, k, zone)?;
    let (from, to) = dates.wide();
    match k.kind.as_str() {
        "age" => {
            if !user.phi {
                anyhow::bail!("age comes from the date of birth: it needs the PHI permission");
            }
            let mut st = c.prepare("SELECT patient, birth_date FROM patient_identifiers WHERE birth_date != ''")?;
            for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
                let (p, b) = r?;
                if let Some(a) = patient::age(&b) {
                    if test_value(k, a as f64, "")? {
                        add(p, format!("age {a}"));
                    }
                }
            }
        }
        "sex" => {
            let mut st = c.prepare("SELECT id, sex FROM patients WHERE upper(sex) = upper(?1)")?;
            for r in st.query_map([k.text.trim()], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
                let (p, s) = r?;
                add(p, format!("sex {s}"));
            }
        }
        "has" => {
            let cat = categories::named(&k.category).ok_or_else(|| anyhow::anyhow!("no category {:?}", k.category))?;
            for (p, ks) in patient::kinds_by_patient(c)? {
                if ks.iter().any(|x| x == cat) {
                    add(p, format!("has {}", categories::label(cat)));
                }
            }
        }
        "encounter" => {
            let mut st = c.prepare(&format!(
                "SELECT e.patient, e.encounter_id, e.admit_time, e.discharge_time, e.admit_unit,
                        COALESCE((SELECT group_concat(unit, '|') FROM bed_stays b WHERE b.encounter = e.id AND {}), '')
                 FROM encounters e WHERE {}",
                patient::stay_shown("b.id"),
                patient::encounter_shown("e.id")
            ))?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?;
            for r in rows {
                let (p, eid, a, b, unit, stays) = r?;
                let units: Vec<&str> =
                    std::iter::once(unit.as_str()).chain(stays.split('|')).filter(|u| !u.is_empty()).collect();
                // an encounter without a discharge goes on: until now
                let (a2, b2) =
                    (a.unwrap_or(f64::NEG_INFINITY), b.unwrap_or_else(|| a.map_or(f64::INFINITY, |a| now().max(a))));
                if !(dates.after(p, b2) && dates.before(p, a2)) {
                    continue;
                }
                let t = k.text.trim().to_lowercase();
                if !t.is_empty() && !units.iter().any(|u| u.to_lowercase().contains(&t)) {
                    continue;
                }
                if let Some(want) = k.icu {
                    if units.iter().any(|u| categories::is_icu_unit(u)) != want {
                        continue;
                    }
                }
                add(p, format!("encounter {eid}{}", units.last().map(|u| format!(" ({u})")).unwrap_or_default()));
            }
        }
        "diagnosis" => {
            let t = k.text.trim();
            let mut st = c.prepare(&format!(
                "SELECT patient, code, name FROM ehr_items WHERE patient IS NOT NULL AND category = 'dx'
                   AND (code LIKE ?1 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\') AND {} ORDER BY time, id",
                patient::item_shown("")
            ))?;
            let prefix = format!("{}%", t.replace('%', "\\%").replace('_', "\\_"));
            for r in st.query_map(params![prefix, like(t)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })? {
                let (p, code, name) = r?;
                add(p, format!("{} {name}", code).trim().to_string());
            }
        }
        "measurement" => {
            let rows = measurement_rows(c, k, &dates)?;
            let units: Vec<String> = rows.iter().map(|r| r.3.clone()).collect();
            one_unit(k, &units)?;
            let mut skipped = Skipped::default();
            for (p, name, v, unit, t) in rows {
                if !skipped.compares(k, &unit) {
                    continue;
                }
                if has_value_test(k) && !test_value(k, v, &unit)? {
                    continue;
                }
                let when = t
                    .map(|t| format!(" on {}", crate::util::fmt_local(t, zone).get(..10).unwrap_or("")))
                    .unwrap_or_default();
                add(p, format!("{name} {} {unit}{when}", fmt_num(v)).replace("  ", " "));
            }
            skipped.done(k, cache)?;
        }
        "medication" => {
            let mut st = c.prepare(&format!(
                "SELECT patient, name, COALESCE(end_time, time), time FROM ehr_items WHERE patient IS NOT NULL AND category IN ('med', 'blood')
                   AND name LIKE ?1 ESCAPE '\\' AND (CAST(?2 AS REAL) IS NULL OR COALESCE(end_time, time) >= ?2) AND (CAST(?3 AS REAL) IS NULL OR time < ?3)
                   AND {}",
                patient::item_shown("")
            ))?;
            for r in st.query_map(params![like(&k.text), from, to], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                ))
            })? {
                let (p, name, end, start) = r?;
                let within = dates.from.is_none_or(|_| end.is_some_and(|t| dates.after(p, t)))
                    && dates.to.is_none_or(|_| start.is_some_and(|t| dates.before(p, t)));
                if dates.per_patient() && !within {
                    continue;
                }
                add(p, name);
            }
        }
        "text" => {
            let t = k.text.trim();
            if t.chars().count() < 3 {
                anyhow::bail!("text: at least 3 characters");
            }
            let (cond, phrase) = c.text_contains("x", 1, t);
            let mut st = c.prepare(&format!(
                "SELECT i.patient, i.name, i.category, i.code, i.vocabulary, i.value_text, i.unit, i.service
                 FROM item_text x JOIN ehr_items i ON i.id = x.rowid
                 WHERE {cond} AND i.patient IS NOT NULL AND (?2 OR i.category != 'note') AND {}",
                patient::item_shown("i")
            ))?;
            // in Privacy Mode, the text as the session is shown it holds the words (not the
            // patient's names, identifiers or staff, which it does not show)
            let gate = if user.privacy { Some(crate::search::build_gate(c)?) } else { None };
            let want = t.to_lowercase();
            for r in st.query_map(params![phrase, user.reads_notes()], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    [
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                    ],
                ))
            })? {
                let (p, name, cat, [code, vocabulary, value, unit, service]) = r?;
                if let Some(g) = &gate {
                    let own = g.for_patient(p);
                    let value = if cat == "note" {
                        let (names, birth) = crate::gate::Restricted::identity_words(c, p);
                        own.note(&value, &names, &birth)
                    } else {
                        own.text(&value)
                    };
                    let body = [
                        own.text(&name),
                        own.text(&code),
                        own.text(&vocabulary),
                        value,
                        own.text(&unit),
                        own.text(&service),
                    ]
                    .join(" ")
                    .to_lowercase();
                    if !body.contains(&want) {
                        continue;
                    }
                }
                add(p, format!("“{t}” in {} ({name})", categories::label(&cat).to_lowercase()));
            }
        }
        "column" => {
            let id = k.import.ok_or_else(|| anyhow::anyhow!("column: of which database?"))?;
            let Some((_, dname, cols)) = db_columns(c)?.into_iter().find(|d| d.0 == id) else {
                return Ok(out); // a database removed since: nobody
            };
            let Some((pos, header, maps_to)) = cols
                .iter()
                .find(|(_, h, _)| h.trim().eq_ignore_ascii_case(k.name.trim()))
                .cloned()
                .or_else(|| cols.iter().find(|(_, h, _)| names_column(h, &k.name)).cloned())
            else {
                anyhow::bail!("{dname} has no column {:?}", k.name);
            };
            if !crate::databases::column_visible(user, &header, &maps_to) {
                anyhow::bail!("the column {header:?} of {dname} needs the PHI permission");
            }
            let unit = crate::tables::measure_column(&header).map(|(_, u)| u).unwrap_or_default();
            // the text of notes is searched only with the PHI permission
            let hide_notes = !user.reads_notes() && maps_to == "text";
            // in Privacy Mode, the cells are tested as the session is shown them (gate.rs)
            let gate = if user.privacy { Some(crate::search::build_gate(c)?) } else { None };
            let shown_as = user.clone();
            // only the rows whose cell may pass, read from the database: a little more than the
            // test below lets through (it stays the test), never less
            let cell = c.json_cell("r.cells", pos);
            let mut filter = String::new();
            let mut args: Vec<crate::sql::Value> = vec![];
            let named = Criterion { name: header.clone(), ..k.clone() };
            if k.latest || k.cmp == "!=" {
                // every row: the latest is chosen among all; "not" needs all
            } else if has_value_test(k) {
                if let Ok((lo, hi)) = value_bounds(&named, &unit) {
                    // the database's numbers (typed.rs), by the column's index; else its cells
                    let typed = crate::typed::exists(c, id)?;
                    if typed {
                        crate::typed::index(c, id, pos);
                    }
                    let num = if typed { format!("t.c{pos}") } else { c.as_number(&cell) };
                    let mut conds = String::new();
                    for (b, op) in [(lo, ">="), (hi, "<=")] {
                        if let Some(b) = b {
                            args.push(b.into());
                            conds += &format!(" AND {num} {op} ?{}", args.len() + 1);
                        }
                    }
                    if typed && !conds.is_empty() {
                        filter += &format!(
                            " AND r.line IN (SELECT t.line FROM {} t WHERE 1 = 1{conds})",
                            crate::typed::name(id)
                        );
                    } else {
                        filter += &conds;
                    }
                }
            } else if !k.text.trim().is_empty() && k.cmp != "=" && k.text.is_ascii() && !user.privacy {
                // (SQLite's LIKE ignores the case of ASCII letters only)
                args.push(like(&k.text).into());
                filter += &format!(" AND {cell} LIKE ?{} ESCAPE '\\'", args.len() + 1);
            }
            // the column's cell only (not every cell of each row)
            let mut rows: Vec<(i64, Option<f64>, String)> = db_cell_where(c, id, pos, &filter, args)?
                .into_iter()
                .filter(|(p, t, ..)| dates.holds(*p, *t))
                .filter(|(.., cat)| !(hide_notes && cat == "note"))
                .map(|(p, t, cells, _, cat)| {
                    let v = cells.first().cloned().unwrap_or_default();
                    // in Privacy Mode, the cell as the session is shown it
                    let v = match &gate {
                        Some(g) => shown_cell(c, g, &shown_as, &header, &maps_to, p, &cat, &v),
                        None => v,
                    };
                    (p, t, v.trim().to_string())
                })
                .filter(|(_, _, v)| !v.is_empty())
                .collect();
            if k.latest {
                // each patient's latest row (by time, then by line)
                let mut last: HashMap<i64, (Option<f64>, usize)> = HashMap::new();
                for (i, (p, t, _)) in rows.iter().enumerate() {
                    let e = last.entry(*p).or_insert((*t, i));
                    if t.unwrap_or(f64::NEG_INFINITY) >= e.0.unwrap_or(f64::NEG_INFINITY) {
                        *e = (*t, i);
                    }
                }
                let keep: HashSet<usize> = last.into_values().map(|(_, i)| i).collect();
                rows = rows.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, r)| r).collect();
            }
            let short = crate::tables::measure_column(&header).map(|(n, _)| n).unwrap_or(header.clone());
            let mut skipped = Skipped::default();
            for (p, t, v) in rows {
                let ok = if has_value_test(k) {
                    match v.parse::<f64>() {
                        Ok(_) if !skipped.compares(&named, &unit) => false,
                        Ok(x) => test_value(&named, x, &unit)?,
                        Err(_) => false,
                    }
                } else if !k.text.trim().is_empty() {
                    let hit = v.to_lowercase().contains(&k.text.trim().to_lowercase());
                    if k.cmp == "=" {
                        v.eq_ignore_ascii_case(k.text.trim())
                    } else if k.cmp == "!=" {
                        !v.eq_ignore_ascii_case(k.text.trim())
                    } else {
                        hit
                    }
                } else {
                    true
                };
                if ok {
                    let when = t
                        .map(|t| format!(" on {}", crate::util::fmt_local(t, zone).get(..10).unwrap_or("")))
                        .unwrap_or_default();
                    let shown: String = if v.chars().count() > 40 {
                        format!("{}…", v.chars().take(40).collect::<String>())
                    } else {
                        v.clone()
                    };
                    add(
                        p,
                        format!(
                            "{short} {shown}{} ({dname}){when}",
                            if unit.is_empty() || v.parse::<f64>().is_err() {
                                String::new()
                            } else {
                                format!(" {unit}")
                            }
                        ),
                    );
                }
            }
            skipped.done(&Criterion { name: short.clone(), ..k.clone() }, cache)?;
        }
        "expression" => {
            // as saved; the text read again only to say when it now means something else
            let now = parse_text(c, user, &k.text);
            let e = match (&k.resolved, now) {
                (Some(saved), Ok(now)) => {
                    if **saved != now {
                        cache.warn(Some(format!(
                            "“{}” now reads as {}: the criteria keep what it meant when saved ({})",
                            k.text,
                            describe_expr(&now),
                            describe_expr(saved)
                        )));
                    }
                    (**saved).clone()
                }
                (Some(saved), Err(why)) => {
                    cache.warn(Some(format!(
                        "“{}” would not be read now ({why}): the criteria keep what it meant when saved",
                        k.text
                    )));
                    (**saved).clone()
                }
                (None, now) => now?,
            };
            let q = Criteria { include: Some(e), ..Default::default() };
            for (p, why) in evaluate_twice(c, user, timezone, &q, cache)?.0 {
                add(p, why.join(", "));
            }
        }
        "database" => {
            let id = k.import.ok_or_else(|| anyhow::anyhow!("database: which one?"))?;
            // a database removed since the criteria were written: nobody is in it any more
            let Some(name): Option<String> = c
                .query_row(
                    "SELECT CASE WHEN name != '' THEN name ELSE origin END FROM imports WHERE id = ?1 AND state = 'active'",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
            else {
                return Ok(out);
            };
            for p in crate::databases::patients_of(c, id)? {
                add(p, format!("in {name}"));
            }
        }
        other => anyhow::bail!("unknown criterion {other:?}"),
    }
    Ok(out)
}

/// A measurement's results and vital signs in the catalog, each (patient, name, value, unit,
/// time), oldest first, in the criterion's dates; with a threshold, only the values that may
/// pass are read (`value_bounds`, a little wider than `test_value`, which stays the test);
/// `latest`: each patient's latest only, of the name written in full.
pub(super) fn measurement_rows(
    c: &Connection,
    k: &Criterion,
    dates: &Dates,
) -> anyhow::Result<Vec<(i64, String, f64, String, Option<f64>)>> {
    let (from, to) = dates.wide();
    // a name written in full is that name (a database named, or the latest value: QT is not
    // QTc); else names containing it (v1)
    // (the exact name uses the index on lower(name))
    let (named, pattern) = if k.import.is_some() || k.latest {
        ("lower(name) = lower(?1)", k.name.trim().to_string())
    } else {
        ("name LIKE ?1 ESCAPE '\\'", like(&k.name))
    };
    let rows_of = "FROM ehr_items i
         WHERE {named} AND patient IS NOT NULL AND value_num IS NOT NULL
           AND (CAST(?2 AS REAL) IS NULL OR time >= ?2) AND (CAST(?3 AS REAL) IS NULL OR time < ?3)
           AND (CAST(?4 AS INTEGER) IS NULL OR EXISTS (SELECT 1 FROM ehr_provenance v WHERE v.item = i.id AND v.import_id = ?4))"
        .replace("{named}", named)
        + " AND "
        + &patient::item_shown("i");
    let mut args: Vec<crate::sql::Value> = vec![
        pattern.clone().into(),
        from.into(),
        to.into(),
        k.import.map_or(crate::sql::Value::Null, crate::sql::Value::Integer),
    ];
    // a threshold: only the values that may pass are read, by unit (the threshold in each unit
    // the results have; the units it does not compare with read whole, to be counted)
    let mut only = String::new();
    if has_value_test(k) && !k.latest && k.cmp != "!=" {
        let mut st = c.prepare(&format!("SELECT DISTINCT unit {rows_of}"))?;
        let units: Vec<String> =
            st.query_map(crate::sql::params_from_iter(args.clone()), |r| r.get(0))?.collect::<Result<_, _>>()?;
        one_unit(k, &units)?;
        let mut any = vec![];
        for unit in units {
            let bounds = if comparable(k, &unit) { Some(value_bounds(k, &unit)?) } else { None };
            args.push(unit.into());
            let mut cond = format!("unit = ?{}", args.len());
            if let Some((lo, hi)) = bounds {
                if let Some(lo) = lo {
                    args.push(lo.into());
                    cond += &format!(" AND value_num >= ?{}", args.len());
                }
                if let Some(hi) = hi {
                    args.push(hi.into());
                    cond += &format!(" AND value_num <= ?{}", args.len());
                }
            }
            any.push(format!("({cond})"));
        }
        only = if any.is_empty() { " AND 1 = 0".into() } else { format!(" AND ({})", any.join(" OR ")) };
    }
    let mut st =
        c.prepare(&format!("SELECT patient, name, value_num, unit, time {rows_of}{only} ORDER BY time, id"))?;
    let rows = st.query_map(crate::sql::params_from_iter(args), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, f64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<f64>>(4)?,
        ))
    })?;
    let mut rows: Vec<(i64, String, f64, String, Option<f64>)> = rows.collect::<Result<_, _>>()?;
    if dates.per_patient() {
        rows.retain(|r| dates.holds(r.0, r.4));
    }
    if k.latest {
        let mut last: HashMap<i64, usize> = HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            last.insert(r.0, i);
        }
        let keep: HashSet<usize> = last.into_values().collect();
        rows = rows.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, r)| r).collect();
    }
    Ok(rows)
}

/// A database's cell as a session in Privacy Mode is shown it: its placeholder, or its text
/// through the gate by its patient's settings (a note's without the patient's names).
#[allow(clippy::too_many_arguments)]
pub(super) fn shown_cell(
    c: &Connection,
    g: &crate::gate::Restricted,
    user: &User,
    header: &str,
    maps_to: &str,
    patient: i64,
    category: &str,
    v: &str,
) -> String {
    let own = g.for_patient(patient);
    match crate::gate::column_mask(user, Some(g), header, maps_to) {
        None if maps_to == "text" && category == "note" => {
            let (names, birth) = crate::gate::Restricted::identity_words(c, patient);
            own.note(v, &names, &birth)
        }
        m => crate::gate::cell(Some(&own), m, v),
    }
}
