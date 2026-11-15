//! Exports: members (CSV), the definition (JSON), the members' data (CSV, Excel).

use super::*;

/// The members as CSV, one row per patient.  For users with the PHI permission, their
/// identifiers too (MRN, names, date of birth, age).  In Privacy Mode (`gate`), the same columns,
/// the identifiers as placeholders, patients by their pseudonyms, dates moved back, ages above
/// 89 as 90+.
pub fn export_csv(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    gate: Option<&crate::gate::Restricted>,
) -> anyhow::Result<(String, String)> {
    let co = load(c, cfg, user, id)?;
    let rows = all_members(c, cfg, user, id)?;
    let mut w = csv::Writer::from_writer(vec![]);
    let identified = user.phi && gate.is_none();
    let mut head = vec!["patient_id", "sex", "encounters", "latest_encounter", "data", "how", "added_at"];
    if identified || gate.is_some() {
        head.extend(["mrn", "last_name", "first_name", "birth_date", "age"]);
    }
    w.write_record(&head)?;
    for r in &rows {
        // each patient's dates by their own offset
        let own = match (gate, r["id"].as_i64()) {
            (Some(g), Some(p)) => Some(g.for_patient(p)),
            _ => None,
        };
        let gate = own.as_ref().or(gate);
        let date = |v: &serde_json::Value| {
            v.as_f64().map(|t| utc(gate.map_or(t, |g| g.settings.shift(t)))).unwrap_or_default()
        };
        let s = |k: &str| {
            r.get(k).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_default()
        };
        let kinds: Vec<String> =
            r["kinds"].as_array().into_iter().flatten().filter_map(|k| k.as_str()).map(String::from).collect();
        let patient = match (gate, r["id"].as_i64()) {
            (Some(g), Some(p)) => g.pseudonym(p),
            _ if identified => s("patient_id"),
            _ => s("pseudonym"),
        };
        let mut rec = vec![
            patient,
            s("sex"),
            s("encounters"),
            date(&r["latest"]),
            kinds.join(" "),
            s("how"),
            date(&r["added_at"]),
        ];
        if let Some(g) = gate {
            let age = r["age"].as_i64().map(|a| g.settings.age(a)).unwrap_or_default();
            rec.extend([
                crate::gate::MRN.into(),
                crate::gate::PATIENT_NAME.into(),
                crate::gate::PATIENT_NAME.into(),
                crate::gate::BIRTH_DATE.into(),
                age,
            ]);
        } else if identified {
            let p = &r["phi"];
            let mrn = p["mrns"]
                .as_array()
                .and_then(|m| m.first())
                .and_then(|m| m.get(1))
                .and_then(|m| m.as_str())
                .unwrap_or("");
            rec.extend([
                mrn.to_string(),
                p["last_name"].as_str().unwrap_or("").into(),
                p["first_name"].as_str().unwrap_or("").into(),
                p["birth_date"].as_str().unwrap_or("").into(),
                r["age"].as_i64().map(|a| a.to_string()).unwrap_or_default(),
            ]);
        }
        w.write_record(rec.iter().map(|x| crate::databases::csv_cell(x).into_owned()))?;
    }
    if identified {
        audit(c, Some(user), "phi_export", "cohort", &id.to_string());
    }
    Ok((
        format!("cohort-{}{}.csv", file_stem(&co.name), if gate.is_some() { "-privacy" } else { "" }),
        String::from_utf8(w.into_inner()?)?,
    ))
}

/// A time as exports write it: in UTC, said so (ISO 8601).
pub(super) fn utc(t: f64) -> String {
    chrono::DateTime::from_timestamp(t.floor() as i64, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// A file name made of a cohort's name.
pub(super) fn file_stem(name: &str) -> String {
    name.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' { ch } else { '_' }).collect()
}

/// The definition as a JSON file: the query and the source as stored, and the whole
/// definition in words (with the patients added and removed manually).
pub fn export_criteria(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<(String, String)> {
    let co = load(c, cfg, user, id)?;
    let v = serde_json::json!({
        "cohort": co.name, "description": co.description, "members": co.members,
        "criteria": co.criteria, "in_words": criteria_words(&co.criteria), "databases": co.definition.databases,
        "source": co.source, "definition": co.definition,
        "exported_at": utc(now()),
    });
    Ok((format!("cohort-{}-criteria.json", file_stem(&co.name)), serde_json::to_string_pretty(&v)?))
}

/// The criteria in words: what includes, and each exclusion.
pub fn criteria_words(q: &Criteria) -> serde_json::Value {
    let include: Vec<String> = q.include.iter().map(describe_expr).collect();
    serde_json::json!({
        "include": include.join(" AND "),
        "exclude": q.exclude.iter().map(describe_expr).collect::<Vec<_>>(),
    })
}

/// One database's entries about the cohort's members: its name, headers and rows (the patient
/// first, then the entry).
pub struct MemberData {
    pub database: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn member_data(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    gate: Option<&crate::gate::Restricted>,
) -> anyhow::Result<Vec<MemberData>> {
    let members = member_ids(c, cfg, user, id)?;
    let mut ids: Vec<i64> = members.iter().copied().collect();
    ids.sort_unstable();
    let shown_as = User { phi: user.phi && gate.is_none(), ..user.clone() };
    let viewable = crate::databases::granted(c, user, "view")?;
    // outside Privacy Mode, the databases granted without `phi` go through the gate too
    // (patient/restricted.rs): one built here when none is given
    let deidentified = if gate.is_none() { crate::patient::without_phi(c, user)? } else { Default::default() };
    let built = if gate.is_none() && viewable.iter().any(|d| deidentified.contains(d)) {
        Some(crate::search::build_gate(c, cfg)?)
    } else {
        None
    };
    let mut out = vec![];
    let mut identified = false;
    for s in crate::sources::list(c)? {
        if !viewable.contains(&s.id) {
            continue;
        }
        let db_gate = gate.or(built.as_ref().filter(|_| deidentified.contains(&s.id)));
        identified |= db_gate.is_none() && shown_as.phi;
        let zone = s.zone(&cfg.timezone);
        let mut rows = vec![];
        for p in &ids {
            let persons: Vec<String> =
                crate::persons::persons_of(cfg, c, *p, Some(&[s.id]))?.into_iter().flat_map(|(_, keys)| keys).collect();
            if persons.is_empty() {
                continue;
            }
            let shown_pid = match db_gate {
                Some(g) => g.pseudonym(*p),
                None => crate::patient::pseudonym_of(c, *p)?,
            };
            let own = db_gate.map(|g| g.for_patient(*p));
            let g = own.as_ref();
            let (names, birth) = crate::gate::Restricted::identity_words(c, cfg, *p);
            // every cell of text as the gate writes it (the patient's names, identifiers, contact
            // details and dates replaced); free text withheld when it must be
            let cell = |t: &str| g.map_or_else(|| t.to_string(), |g| g.note(t, &names, &birth));
            let free = |t: &str| {
                if t.trim().is_empty() {
                    String::new()
                } else if !shown_as.reads_notes() {
                    "(free text needs the PHI permission, or Privacy Mode letting it through)".to_string()
                } else {
                    g.map_or_else(|| t.to_string(), |g| g.entry_text(t, &names, &birth))
                }
            };
            for e in s.entries(&persons, zone)? {
                let time = e.time.map(|t| utc(g.map_or(t, |g| g.settings.shift(t)))).unwrap_or_default();
                let value = match e.value_num {
                    Some(v) => fmt_num(v),
                    None if crate::cohorts::is_long_text(&e.value_text) => free(&e.value_text),
                    None => cell(&e.value_text),
                };
                rows.push(vec![
                    shown_pid.clone(),
                    time,
                    e.category.clone(),
                    cell(&e.name),
                    cell(&e.code),
                    value,
                    cell(&e.unit),
                    cell(&e.flag),
                    free(&e.text),
                ]);
            }
        }
        if rows.is_empty() {
            continue;
        }
        let headers =
            ["patient", "time", "category", "name", "code", "value", "unit", "flag", "text"].map(String::from).to_vec();
        out.push(MemberData { database: s.name.clone(), headers, rows });
    }
    audit(c, Some(user), if identified { "phi_export" } else { "export" }, "cohort_data", &id.to_string());
    Ok(out)
}

/// The members' data as one CSV: a line per entry (database, patient, time, the entry).
pub fn export_data_csv(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    gate: Option<&crate::gate::Restricted>,
) -> anyhow::Result<(String, String)> {
    let co = load(c, cfg, user, id)?;
    let mut w = csv::Writer::from_writer(vec![]);
    let mut head = false;
    for d in member_data(c, cfg, user, id, gate)? {
        if !head {
            let mut h = vec!["database".to_string()];
            h.extend(d.headers.iter().cloned());
            w.write_record(&h)?;
            head = true;
        }
        for r in &d.rows {
            let mut rec = vec![d.database.clone()];
            rec.extend(r.iter().cloned());
            w.write_record(rec.iter().map(|x| crate::databases::csv_cell(x).into_owned()))?;
        }
    }
    Ok((
        format!("cohort-{}-data{}.csv", file_stem(&co.name), if gate.is_some() { "-privacy" } else { "" }),
        String::from_utf8(w.into_inner()?)?,
    ))
}

/// The cohort as an Excel workbook: its members, its criteria, and a sheet per database.
pub fn export_xlsx(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    gate: Option<&crate::gate::Restricted>,
) -> anyhow::Result<(String, Vec<u8>)> {
    let co = load(c, cfg, user, id)?;
    let (_, members) = export_csv(c, cfg, user, id, gate)?;
    let mut rd = csv::ReaderBuilder::new().has_headers(false).from_reader(members.as_bytes());
    let mrows: Vec<Vec<String>> =
        rd.records().filter_map(|r| r.ok()).map(|r| r.iter().map(String::from).collect()).collect();
    let d = &co.definition;
    let mut crit = vec![
        vec!["Cohort".to_string(), co.name.clone()],
        vec!["Description".into(), co.description.clone()],
        vec!["Members".into(), co.members.to_string()],
    ];
    let dbs = d.databases.join(", ");
    for (label, value) in [("Made", &d.origin), ("From", &d.from), ("Databases", &dbs), ("Include", &d.include)] {
        if !value.is_empty() {
            crit.push(vec![label.into(), value.clone()]);
        }
    }
    for x in &d.exclude {
        crit.push(vec!["Exclude".into(), x.clone()]);
    }
    if !d.added.is_empty() {
        crit.push(vec!["Added manually".into(), d.added.join(", ")]);
    }
    if !d.removed.is_empty() {
        crit.push(vec!["Removed manually".into(), d.removed.join(", ")]);
    }
    crit.push(vec!["Criteria (JSON)".into(), serde_json::to_string(&co.criteria)?]);
    if let Some(s) = &co.source {
        crit.push(vec!["Source (JSON)".into(), serde_json::to_string(s)?]);
    }
    if let Some(g) = gate {
        // the definition names patients by their IDs: their pseudonyms here
        for row in crit.iter_mut() {
            for cell in row.iter_mut().skip(1) {
                *cell = g.text(cell);
            }
        }
        let d = &g.settings;
        crit.push(vec![
            "Privacy Mode".into(),
            format!(
                "dates moved back {} days, and each patient's by 0 to {} days more of their own; withheld: {}",
                d.base_seconds() / 86400,
                d.spread_days,
                d.fields.join(", ")
            ),
        ]);
    }
    let mut sheets = vec![
        crate::xlsx::Sheet { name: "Members".into(), rows: mrows },
        crate::xlsx::Sheet { name: "Definition".into(), rows: crit },
    ];
    for d in member_data(c, cfg, user, id, gate)? {
        let mut rows = vec![d.headers];
        rows.extend(d.rows);
        sheets.push(crate::xlsx::Sheet { name: d.database, rows });
    }
    Ok((
        format!("cohort-{}{}.xlsx", file_stem(&co.name), if gate.is_some() { "-privacy" } else { "" }),
        crate::xlsx::write(&sheets),
    ))
}
