//! The list of patients (the Patient Catalog): browsing in SQL, text search, the page's details.

use super::*;

// ------------------------------------------------------------------ the list of patients

#[derive(Debug, Serialize)]
pub struct PatientRow {
    pub id: i64,
    pub patient_id: String,
    /// What Privacy Mode names the patient by (pseudonyms.rs).
    pub pseudonym: String,
    pub sex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phi: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<i64>,
    pub encounters: i64,
    pub latest: Option<f64>,
    pub latest_where: String,
    /// Categories of data the patient has (category keys).
    pub kinds: Vec<String>,
    pub items: i64,
    /// The connected databases that hold data of theirs (read when their page opens).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

pub const LIST_FIELDS: &[&str] = &["patient", "encounter", "sex", "unit", "has", "name", "first", "last", "mrn", "dob"];

/// Patients, latest encounter first; `q` uses the search syntax (fields: [`LIST_FIELDS`];
/// `has:icu` …), `has` limits to patients with all these categories (`!icu`: without).
pub fn list(
    c: &Connection,
    user: &User,
    q: &str,
    has: &[String],
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>)> {
    list_in(c, user, q, has, None, limit, offset)
}

/// The age a user is shown: with the PHI permission, from the date of birth; in Privacy Mode
/// too (the date of birth stays on the server; the gate caps the age at 90+); else none.
pub(super) fn shown_age(c: &Connection, user: &User, id: i64, ident: Option<&Identity>) -> anyhow::Result<Option<i64>> {
    Ok(match ident {
        _ if user.privacy => age(&identity(c, id)?.birth_date),
        Some(i) => age(&i.birth_date),
        None => None,
    })
}

/// `shown_age`, from the real identity read already (`real`: with the PHI permission or in
/// Privacy Mode).
pub(super) fn shown_age_of(user: &User, real: Option<&Identity>) -> Option<i64> {
    if user.phi || user.privacy {
        real.and_then(|i| age(&i.birth_date))
    } else {
        None
    }
}

/// The SQL condition "the bed stay `b` is in an intensive care unit", by the one rule
/// (`categories::is_icu_unit`): the units of the bed stays it holds for, written out.
pub(super) fn icu_units(c: &Connection) -> anyhow::Result<String> {
    let units: Vec<String> =
        c.prepare("SELECT DISTINCT unit FROM bed_stays")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let icu: Vec<String> = units
        .into_iter()
        .filter(|u| categories::is_icu_unit(u))
        .map(|u| format!("'{}'", u.replace('\'', "''")))
        .collect();
    Ok(if icu.is_empty() { "1 = 0".into() } else { format!("b.unit IN ({})", icu.join(", ")) })
}

/// What each patient has: category keys, in the journey's order.
pub fn kinds_by_patient(c: &Connection) -> anyhow::Result<HashMap<i64, Vec<String>>> {
    let mut kinds: HashMap<i64, Vec<String>> = HashMap::new();
    let icu = icu_units(c)?;
    {
        let (item, rec, enc, stay) =
            (item_shown(""), recording_shown("r.id"), encounter_shown("e.id"), stay_shown("b.id"));
        let mut st = c.prepare(&format!(
            "SELECT DISTINCT patient, category FROM ehr_items WHERE patient IS NOT NULL AND category NOT IN ('', 'allergy') AND {item}
             UNION SELECT DISTINCT e.patient, r.category FROM recording_links l JOIN encounters e ON e.id = l.encounter
               JOIN recordings r ON r.id = l.recording WHERE {rec} AND {enc}
             UNION SELECT DISTINCT e.patient, 'icu' FROM bed_stays b JOIN encounters e ON e.id = b.encounter
               WHERE {icu} AND {stay} AND {enc}",
        ))?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            kinds.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
    }
    let order: Vec<&str> = categories::journey_keys().collect();
    for v in kinds.values_mut() {
        v.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(99));
    }
    Ok(kinds)
}

/// `list`, limited to the patients in `only` (a cohort, a cohort search's matches).
pub fn list_in(
    c: &Connection,
    user: &User,
    q: &str,
    has: &[String],
    only: Option<&std::collections::HashSet<i64>>,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>)> {
    let (total, rows, _) = list_core(c, user, q, has, only, limit.clamp(1, 1000), offset)?;
    Ok((total, rows))
}

/// `list_in` without pages: every patient in `only`, latest first (exports).
pub fn list_all_in(
    c: &Connection,
    user: &User,
    only: &std::collections::HashSet<i64>,
) -> anyhow::Result<Vec<PatientRow>> {
    Ok(list_core(c, user, "", &[], Some(only), usize::MAX, 0)?.1)
}

/// Every patient the list's search finds (the same search: `q`, `has`, `only`), latest first.
pub fn matching_ids(
    c: &Connection,
    user: &User,
    q: &str,
    has: &[String],
    only: Option<&std::collections::HashSet<i64>>,
) -> anyhow::Result<Vec<i64>> {
    Ok(list_core(c, user, q, has, only, 0, 0)?.2)
}

/// The patients who have data of category `cat`, as [`kinds_by_patient`] finds it: a SQL
/// query of patient numbers, never NULL (`cat` is a category's key, never typed text).  Read
/// as a set (`p.id IN (…)`): one pass over the category's rows, not a probe per patient.
pub(super) fn has_set(cat: &str, icu: &str) -> String {
    let (item, rec, enc, stay) = (item_shown(""), recording_shown("r.id"), encounter_shown("e.id"), stay_shown("b.id"));
    let mut any = vec![
        format!("SELECT patient FROM ehr_items WHERE category = '{cat}' AND patient IS NOT NULL AND {item}"),
        format!(
            "SELECT e.patient FROM recording_links l JOIN encounters e ON e.id = l.encounter
               JOIN recordings r ON r.id = l.recording WHERE r.category = '{cat}' AND {rec} AND {enc}"
        ),
    ];
    if cat == "icu" {
        any.push(format!(
            "SELECT e.patient FROM bed_stays b JOIN encounters e ON e.id = b.encounter WHERE {icu} AND {stay} AND {enc}"
        ));
    }
    any.join(" UNION ")
}

/// The SQL condition "patient `p` has data of category `cat`", probed per patient: SQLite's
/// form, and PostgreSQL's for "has none" (`NOT IN` over a large set is slow there; [`has_set`]
/// otherwise).
pub(super) fn has_sql(cat: &str, icu: &str) -> String {
    let (item, rec, enc, stay) =
        (item_shown("i"), recording_shown("r.id"), encounter_shown("e.id"), stay_shown("b.id"));
    let mut any = vec![
        format!("EXISTS (SELECT 1 FROM ehr_items i WHERE i.patient = p.id AND i.category = '{cat}' AND {item})"),
        format!(
            "EXISTS (SELECT 1 FROM recording_links l JOIN encounters e ON e.id = l.encounter
                      JOIN recordings r ON r.id = l.recording WHERE e.patient = p.id AND r.category = '{cat}' AND {rec} AND {enc})"
        ),
    ];
    if cat == "icu" {
        any.push(format!(
            "EXISTS (SELECT 1 FROM bed_stays b JOIN encounters e ON e.id = b.encounter
                      WHERE e.patient = p.id AND {icu} AND {stay} AND {enc})"
        ));
    }
    format!("({})", any.join(" OR "))
}

/// The categories of data of these patients (as [`kinds_by_patient`], for a page).
pub(super) fn kinds_of(c: &Connection, ids: &[i64]) -> anyhow::Result<HashMap<i64, Vec<String>>> {
    let mut kinds: HashMap<i64, Vec<String>> = HashMap::new();
    let icu = icu_units(c)?;
    let (item, rec, enc, stay) = (item_shown(""), recording_shown("r.id"), encounter_shown("e.id"), stay_shown("b.id"));
    for (p, k) in c.query_in(
        &format!("SELECT DISTINCT patient, category FROM ehr_items WHERE patient IN ({{}}) AND category NOT IN ('', 'allergy') AND {item}
         UNION SELECT DISTINCT e.patient, r.category FROM recording_links l JOIN encounters e ON e.id = l.encounter
           JOIN recordings r ON r.id = l.recording WHERE e.patient IN ({{}}) AND {rec} AND {enc}
         UNION SELECT DISTINCT e.patient, 'icu' FROM bed_stays b JOIN encounters e ON e.id = b.encounter
           WHERE e.patient IN ({{}}) AND {icu} AND {stay} AND {enc}"
        ),
        ids,
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )? {
        let v = kinds.entry(p).or_default();
        if !v.contains(&k) {
            v.push(k);
        }
    }
    let order: Vec<&str> = categories::journey_keys().collect();
    for v in kinds.values_mut() {
        v.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(99));
    }
    Ok(kinds)
}

/// The list without a text search: filtered and ordered in the database, the page's details
/// read for the page only, so that it costs the same with a thousand patients or a million.
pub(super) fn list_browse(
    c: &Connection,
    user: &User,
    has: &[String],
    only: Option<&std::collections::HashSet<i64>>,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>, Vec<i64>)> {
    let phi = user.phi;
    // `ecg`: has ECGs; `!ecg`: has none; an unknown category: nobody
    let pg = c.dialect() == crate::sql::Dialect::Postgres;
    // (a patient only an import still loading holds is not listed yet)
    let mut cond = format!(" AND {}", patient_shown("p.id"));
    let icu = if has.iter().any(|h| h.trim_start_matches('!') == "icu") { icu_units(c)? } else { "1 = 0".into() };
    for h in has {
        let (want, name) = h.strip_prefix('!').map_or((true, h.as_str()), |n| (false, n));
        cond += &match categories::named(name).filter(|k| *k != "allergy") {
            // PostgreSQL reads the category's patients as one set; SQLite probes each patient
            // (a set there is a temporary table, which costs more when many users ask at once)
            Some(k) if want && pg => format!(" AND p.id IN ({})", has_set(k, &icu)),
            Some(k) if want => format!(" AND {}", has_sql(k, &icu)),
            Some(k) => format!(" AND NOT {}", has_sql(k, &icu)),
            None => " AND 1 = 0".into(),
        };
    }
    let enc = encounter_shown("e.id");
    let latest = format!("(SELECT MAX(e.admit_time) FROM encounters e WHERE e.patient = p.id AND {enc})");
    let row = |r: &crate::sql::Row| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?));
    let mut rows: Vec<(i64, Option<f64>)> = match only {
        Some(set) => {
            let ids: Vec<i64> = set.iter().copied().collect();
            c.query_in(&format!("SELECT p.id, {latest} FROM patients p WHERE p.id IN ({{}}){cond}"), &ids, row)?
        }
        None => {
            // every patient's latest encounter: on PostgreSQL in one grouped read; on SQLite by
            // the index, patient by patient (a grouped read builds a temporary table each time,
            // which costs more when many users ask at once)
            let sql = if pg {
                format!(
                    "SELECT p.id, m.latest FROM patients p
                     LEFT JOIN (SELECT patient, MAX(admit_time) AS latest FROM encounters e WHERE {enc} GROUP BY patient) m
                       ON m.patient = p.id
                     WHERE 1 = 1{cond}"
                )
            } else {
                format!("SELECT p.id, {latest} FROM patients p WHERE 1 = 1{cond}")
            };
            let mut st = c.prepare(&sql)?;
            st.query_map([], row)?.collect::<Result<_, _>>()?
        }
    };
    // latest encounter first, patients without one last; then the newest patient first
    sort_latest(c, user, &mut rows)?;
    let total = rows.len();
    let all: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let page: Vec<(i64, Option<f64>)> = rows.into_iter().skip(offset).take(limit).collect();
    let ids: Vec<i64> = page.iter().map(|p| p.0).collect();
    if ids.is_empty() {
        return Ok((total, vec![], all));
    }
    // the page's details
    let base: HashMap<i64, (String, String, String)> = c
        .query_in("SELECT id, patient_id, sex, COALESCE(pseudonym, '') FROM patients WHERE id IN ({})", &ids, |r| {
            Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
        })?
        .into_iter()
        .collect();
    let mut encs: HashMap<i64, Vec<(Option<f64>, String)>> = HashMap::new();
    for (p, t, unit) in c.query_in(
        &format!(
            "SELECT e.patient, e.admit_time,
                    COALESCE((SELECT unit FROM bed_stays b WHERE b.encounter = e.id AND unit != '' AND {}
                              ORDER BY start_time DESC, id DESC LIMIT 1), e.admit_unit)
             FROM encounters e WHERE e.patient IN ({{}}) AND {enc}",
            stay_shown("b.id")
        ),
        &ids,
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, String>(2)?)),
    )? {
        encs.entry(p).or_default().push((t, unit));
    }
    let items: HashMap<i64, i64> = c
        .query_in(
            &format!(
                "SELECT patient, COUNT(*) FROM ehr_items WHERE patient IN ({{}}) AND {} GROUP BY patient",
                item_shown("")
            ),
            &ids,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect();
    let mut kinds = kinds_of(c, &ids)?;
    let mut linked: HashMap<i64, Vec<String>> = HashMap::new();
    for (p, name) in c.query_in(
        "SELECT s.patient, c.name FROM source_persons s JOIN connections c ON c.id = s.connection
         WHERE s.patient IN ({}) GROUP BY s.patient, c.name ORDER BY s.patient, c.name",
        &ids,
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )? {
        linked.entry(p).or_default().push(name);
    }
    let mut real = if phi || user.privacy { identities(c, &ids)? } else { HashMap::new() };
    let mut out = vec![];
    for (id, latest) in page {
        let Some((patient_id, sex, pseudonym)) = base.get(&id).cloned() else { continue };
        let e = encs.remove(&id).unwrap_or_default();
        let latest_where = e.iter().filter(|x| x.0 == latest).map(|x| x.1.clone()).next().unwrap_or_default();
        let ident = real.remove(&id);
        out.push(PatientRow {
            id,
            patient_id,
            pseudonym,
            sex,
            age: shown_age_of(user, ident.as_ref()),
            phi: if phi { ident } else { None },
            encounters: e.len() as i64,
            latest,
            latest_where,
            kinds: kinds.remove(&id).unwrap_or_default(),
            items: items.get(&id).copied().unwrap_or(0),
            sources: linked.remove(&id).unwrap_or_default(),
        });
    }
    Ok((total, out, all))
}

/// Patients (id, latest encounter), latest first, those without one last, then the newest
/// patient first.  In Privacy Mode, by the latest encounter as shown (each patient's own
/// offset): an order by real times would tell them.
fn sort_latest(c: &Connection, user: &User, rows: &mut [(i64, Option<f64>)]) -> anyhow::Result<()> {
    if user.privacy && !user.phi {
        let o = crate::search::Offsets::load(c)?;
        for r in rows.iter_mut() {
            r.1 = r.1.map(|t| o.shift(Some(r.0), t));
        }
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.0.cmp(&a.0)));
        // the real times back (the gate moves them as it sends them)
        for r in rows.iter_mut() {
            r.1 = r.1.map(|t| t + o.seconds(Some(r.0)));
        }
        return Ok(());
    }
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.0.cmp(&a.0)));
    Ok(())
}

/// The search behind `list_in` and `matching_ids`: the total, a page of rows, every id found.
pub(super) fn list_core(
    c: &Connection,
    user: &User,
    q: &str,
    has: &[String],
    only: Option<&std::collections::HashSet<i64>>,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>, Vec<i64>)> {
    let phi = user.phi;
    let terms = parse(q);
    crate::search::check(&terms, LIST_FIELDS, phi).map_err(anyhow::Error::msg)?;
    if terms.is_empty() {
        return list_browse(c, user, has, only, limit, offset);
    }
    let kinds = kinds_by_patient(c)?;
    let mut encs: HashMap<i64, Vec<(String, Option<f64>, String)>> = HashMap::new();
    {
        let mut st = c.prepare(&format!(
            "SELECT e.patient, CASE WHEN ?1 THEN e.encounter_id ELSE COALESCE(e.pseudonym, '') END, e.admit_time,
                    COALESCE((SELECT unit FROM bed_stays b WHERE b.encounter = e.id AND unit != '' AND {}
                              ORDER BY start_time DESC LIMIT 1), e.admit_unit)
             FROM encounters e WHERE {}",
            stay_shown("b.id"),
            encounter_shown("e.id")
        ))?;
        // without the PHI permission, encounters are searched by their pseudonyms only
        let mut rows = st.query([phi])?;
        while let Some(r) = rows.next()? {
            encs.entry(r.get(0)?).or_default().push((r.get(1)?, r.get(2)?, r.get(3)?));
        }
    }
    let mut items: HashMap<i64, i64> = HashMap::new();
    {
        let mut st = c.prepare(&format!(
            "SELECT patient, COUNT(*) FROM ehr_items WHERE patient IS NOT NULL AND {} GROUP BY patient",
            item_shown("")
        ))?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            items.insert(r.get(0)?, r.get(1)?);
        }
    }
    let mut st = c.prepare(&format!(
        "SELECT p.id, p.patient_id, p.sex, COALESCE(i.last_name, ''), COALESCE(i.first_name, ''), COALESCE(i.birth_date, ''),
                COALESCE(p.pseudonym, '')
         FROM patients p LEFT JOIN patient_identifiers i ON i.patient = p.id WHERE {}",
        patient_shown("p.id")
    ))?;
    let base: Vec<(i64, String, String, String, String, String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .collect::<Result<_, _>>()?;
    let mrns: HashMap<i64, Vec<String>> = {
        let mut out: HashMap<i64, Vec<String>> = HashMap::new();
        let mut st = c.prepare("SELECT DISTINCT patient, mrn FROM patient_mrns")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            out.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
        out
    };
    let mut rows: Vec<(i64, Option<f64>)> = vec![];
    for (id, pid, sex, last, first, dob, pseudonym) in &base {
        if only.is_some_and(|o| !o.contains(id)) {
            continue;
        }
        let k = kinds.get(id).cloned().unwrap_or_default();
        // `ecg`: has ECGs; `!ecg`: has none
        if !has.iter().all(|h| {
            let (want, name) = h.strip_prefix('!').map_or((true, h.as_str()), |n| (false, n));
            categories::named(name).is_some_and(|c| k.iter().any(|x| x == c) == want)
        }) {
            continue;
        }
        let e = encs.get(id).cloned().unwrap_or_default();
        if !terms.is_empty() {
            let mut d = Doc::default();
            // the imported patient ID may be an MRN: searched with the PHI permission only
            if phi {
                d.add("patient", pid);
            }
            d.add("patient", pseudonym);
            d.add("sex", sex);
            for (eid, _, unit) in &e {
                d.add("encounter", eid);
                d.add("unit", unit);
            }
            for x in &k {
                d.add("has", x);
                d.add("has", categories::label(x));
            }
            if phi {
                d.add("name", &format!("{last}, {first}"));
                d.add("name", &format!("{first} {last}"));
                d.add("first", first);
                d.add("last", last);
                d.add("dob", dob);
                for m in mrns.get(id).into_iter().flatten() {
                    d.add("mrn", m);
                }
            }
            if !d.matches(&terms, phi) {
                continue;
            }
        }
        rows.push((*id, e.iter().filter_map(|x| x.1).fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.max(t))))));
    }
    if phi && !q.trim().is_empty() {
        audit(c, Some(user), "phi_search", "patients", q);
    }
    sort_latest(c, user, &mut rows)?;
    let total = rows.len();
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let base: HashMap<i64, &(i64, String, String, String, String, String, String)> =
        base.iter().map(|b| (b.0, b)).collect();
    let mut out = vec![];
    let page: Vec<(i64, Option<f64>)> = rows.into_iter().skip(offset).take(limit).collect();
    // identities, for the page, at once: shown with the PHI permission; ages in Privacy Mode too
    let mut real = if phi || user.privacy {
        identities(c, &page.iter().map(|p| p.0).collect::<Vec<_>>())?
    } else {
        HashMap::new()
    };
    let mut linked: HashMap<i64, Vec<String>> = HashMap::new();
    for (p, name) in c.query_in(
        "SELECT s.patient, c.name FROM source_persons s JOIN connections c ON c.id = s.connection
         WHERE s.patient IN ({}) GROUP BY s.patient, c.name ORDER BY s.patient, c.name",
        &page.iter().map(|p| p.0).collect::<Vec<_>>(),
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )? {
        linked.entry(p).or_default().push(name);
    }
    for (id, latest) in page {
        let b = base[&id];
        let e = encs.get(&id).cloned().unwrap_or_default();
        let latest_where = e.iter().filter(|x| x.1 == latest).map(|x| x.2.clone()).next().unwrap_or_default();
        let ident = real.remove(&id);
        out.push(PatientRow {
            id,
            patient_id: b.1.clone(),
            pseudonym: b.6.clone(),
            sex: b.2.clone(),
            age: shown_age_of(user, ident.as_ref()),
            phi: if phi { ident } else { None },
            encounters: e.len() as i64,
            latest,
            latest_where,
            kinds: kinds.get(&id).cloned().unwrap_or_default(),
            items: items.get(&id).copied().unwrap_or(0),
            sources: linked.remove(&id).unwrap_or_default(),
        });
    }
    Ok((total, out, ids))
}
