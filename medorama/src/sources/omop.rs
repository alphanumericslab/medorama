//! The OMOP CDM (5.3, 5.4) as the canonical views (docs/design/sources.md, "OMOP").
//!
//! Tables read: `person`, `visit_occurrence`, `visit_detail`, `care_site`, `measurement`,
//! `observation`, `condition_occurrence`, `drug_exposure`, `procedure_occurrence`, `note`,
//! `provider`, `concept` (in the vocabulary's schema), `cdm_source` (the version).  Times are
//! given as text (the database's local times, read in the connection's time zone); on SQLite,
//! where OMOP's tools may store them as Unix seconds, a date stored so is turned into its date
//! (local midnight) and a date and time is given as the moment it is (Unix seconds are UTC).
//!
//! Every concept is joined on `NULLIF(id, 0)`: concept 0 ("No matching concept") names nothing,
//! so the source value is read instead.

use super::{Settings, Views};
use crate::sql::Dialect;

/// The OMOP concepts read: female, male, an intensive-care stay.
const FEMALE: i64 = 8532;
const MALE: i64 = 8507;
const INTENSIVE_CARE: i64 = 32037;

/// The measurement operators (`operator_concept_id`), when the vocabulary lacks their concepts:
/// `<`, `<=`, `>`, `>=` (`=` is no operator: the value is the value).
const OPERATORS: &[(i64, &str)] = &[(4171756, "<"), (4171754, "<="), (4172704, ">"), (4171755, ">=")];

/// A schema's prefix: `"cdm".`, or nothing for the default one.
fn prefix(schema: &str) -> String {
    if schema.is_empty() {
        String::new()
    } else {
        format!("\"{}\".", schema.replace('"', ""))
    }
}

/// Whether a schema's name is plain (letters, digits, `_`): nothing else is written into SQL.
pub fn valid_schema(s: &str) -> bool {
    s.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub fn views(st: &Settings, d: Dialect) -> Views {
    let s = prefix(&st.schema);
    let v = prefix(if st.vocabulary_schema.is_empty() { &st.schema } else { &st.vocabulary_schema });
    // a date and time, else its date, as the views give times: text (local), or on SQLite Unix
    // seconds as they are (a moment); a date stored as Unix seconds is that date
    let dt = |datetime: &str, date: &str| {
        match d {
        Dialect::Postgres => format!("CAST(COALESCE({datetime}, {date}) AS TEXT)"),
        Dialect::Sqlite => format!(
            "COALESCE({datetime}, CASE WHEN typeof({date}) IN ('integer', 'real') THEN date({date}, 'unixepoch') ELSE {date} END)"
        ),
    }
    };
    // a value as text (the version)
    let text = |x: &str| match d {
        Dialect::Postgres => format!("CAST({x} AS TEXT)"),
        Dialect::Sqlite => {
            format!("CASE WHEN typeof({x}) IN ('integer', 'real') THEN datetime({x}, 'unixepoch') ELSE {x} END")
        }
    };
    // a concept of an id: none for 0 ("No matching concept")
    let concept =
        |alias: &str, id: &str| format!("LEFT JOIN {v}concept {alias} ON {alias}.concept_id = NULLIF({id}, 0)");
    // the date of birth: birth_datetime's date, else the year, month and day; a month without its
    // day is its 15th, a year alone its 1 July (such ages are within six months)
    let birth = match d {
        Dialect::Postgres => "CASE WHEN p.birth_datetime IS NOT NULL THEN CAST(CAST(p.birth_datetime AS DATE) AS TEXT)
                 WHEN p.year_of_birth IS NOT NULL
                 THEN CAST(make_date(p.year_of_birth, COALESCE(p.month_of_birth, 7),
                                     COALESCE(p.day_of_birth, CASE WHEN p.month_of_birth IS NULL THEN 1 ELSE 15 END)) AS TEXT) END"
            .to_string(),
        Dialect::Sqlite => "CASE WHEN p.birth_datetime IS NOT NULL
                  THEN substr(CASE WHEN typeof(p.birth_datetime) IN ('integer', 'real')
                                   THEN datetime(p.birth_datetime, 'unixepoch') ELSE p.birth_datetime END, 1, 10)
                  WHEN p.year_of_birth IS NOT NULL
                  THEN printf('%04d-%02d-%02d', p.year_of_birth, COALESCE(p.month_of_birth, 7),
                              COALESCE(p.day_of_birth, CASE WHEN p.month_of_birth IS NULL THEN 1 ELSE 15 END)) END"
            .to_string(),
    };
    let (patient_id, mrn) = match st.identifier.as_str() {
        "patient_id" => ("p.person_source_value", "NULL"),
        "none" => ("NULL", "NULL"),
        _ => ("NULL", "p.person_source_value"),
    };
    let persons = format!(
        "SELECT p.person_id AS person_key, {patient_id} AS patient_id, '' AS patient_id_issuer, {mrn} AS mrn,
                '' AS mrn_authority,
                '' AS last_name, '' AS first_name, {birth} AS birth_date,
                CASE p.gender_concept_id WHEN {FEMALE} THEN 'F' WHEN {MALE} THEN 'M' ELSE '' END AS sex
         FROM {s}person p"
    );
    let visits = format!(
        "SELECT vo.visit_occurrence_id AS visit_key, vo.person_id AS person_key,
                {start} AS start_time, {end} AS end_time,
                COALESCE(cs.care_site_name, vc.concept_name, '') AS unit,
                CASE WHEN EXISTS (SELECT 1 FROM {s}visit_detail vd WHERE vd.visit_occurrence_id = vo.visit_occurrence_id
                                  AND vd.visit_detail_concept_id = {INTENSIVE_CARE}) THEN 1 ELSE 0 END AS icu
         FROM {s}visit_occurrence vo
         LEFT JOIN {s}care_site cs ON cs.care_site_id = vo.care_site_id
         {vc}",
        start = dt("vo.visit_start_datetime", "vo.visit_start_date"),
        end = dt("vo.visit_end_datetime", "vo.visit_end_date"),
        vc = concept("vc", "vo.visit_concept_id"),
    );
    // a visit's details: the stays in its units (an intensive-care one by its concept, or by its
    // unit's name, which Medorama tests)
    let stays = format!(
        "SELECT vd.visit_detail_id AS stay_key, vd.person_id AS person_key, vd.visit_occurrence_id AS visit_key,
                {start} AS start_time, {end} AS end_time,
                COALESCE(cs.care_site_name, dc.concept_name, '') AS unit,
                CASE WHEN vd.visit_detail_concept_id = {INTENSIVE_CARE} THEN 1 ELSE 0 END AS icu
         FROM {s}visit_detail vd
         LEFT JOIN {s}care_site cs ON cs.care_site_id = vd.care_site_id
         {dc}",
        start = dt("vd.visit_detail_start_datetime", "vd.visit_detail_start_date"),
        end = dt("vd.visit_detail_end_datetime", "vd.visit_detail_end_date"),
        dc = concept("dc", "vd.visit_detail_concept_id"),
    );
    // a measurement's operator (`<`, `>` …), when it has one other than `=`: the value is then
    // not a number but text, `<5` (as db-builder keeps "<0.5")
    let known: Vec<String> = OPERATORS.iter().map(|(id, sym)| format!("WHEN {id} THEN '{sym}'")).collect();
    let op = format!("COALESCE(NULLIF(op.concept_name, '='), CASE m.operator_concept_id {} END)", known.join(" "));
    let number = match d {
        Dialect::Postgres => "CAST(m.value_as_number AS TEXT)".to_string(),
        Dialect::Sqlite => "CASE WHEN m.value_as_number = CAST(m.value_as_number AS INTEGER)
                 THEN CAST(CAST(m.value_as_number AS INTEGER) AS TEXT) ELSE CAST(m.value_as_number AS TEXT) END"
            .to_string(),
    };
    // entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary,
    // value_num, value_text, unit, ref_low, ref_high, flag, author, text
    let measurement = format!(
        "SELECT 'm' || m.measurement_id AS entry_key, m.person_id AS person_key, m.visit_occurrence_id AS visit_key,
                'lab' AS category, {t} AS time, NULL AS end_time,
                COALESCE(c.concept_name, m.measurement_source_value, '') AS name, COALESCE(c.concept_code, '') AS code,
                COALESCE(c.vocabulary_id, '') AS vocabulary,
                CASE WHEN {op} IS NULL THEN m.value_as_number END AS value_num,
                CASE WHEN m.value_as_number IS NULL THEN COALESCE(vc.concept_name, m.value_source_value, '')
                     WHEN {op} IS NOT NULL THEN {op} || {number} ELSE '' END AS value_text,
                COALESCE(u.concept_code, m.unit_source_value, '') AS unit, m.range_low AS ref_low, m.range_high AS ref_high,
                '' AS flag, '' AS author, '' AS text
         FROM {s}measurement m
         {c}
         {u}
         {vc}
         {opc}",
        t = dt("m.measurement_datetime", "m.measurement_date"),
        c = concept("c", "m.measurement_concept_id"),
        u = concept("u", "m.unit_concept_id"),
        vc = concept("vc", "m.value_as_concept_id"),
        opc = concept("op", "m.operator_concept_id"),
    );
    let observation = format!(
        "SELECT 'o' || o.observation_id, o.person_id, o.visit_occurrence_id, 'obs',
                {t}, NULL, COALESCE(c.concept_name, o.observation_source_value, ''), COALESCE(c.concept_code, ''),
                COALESCE(c.vocabulary_id, ''), o.value_as_number,
                CASE WHEN o.value_as_number IS NULL THEN COALESCE(o.value_as_string, vc.concept_name, '') ELSE '' END,
                COALESCE(u.concept_code, o.unit_source_value, ''), NULL, NULL, '', '', ''
         FROM {s}observation o
         {c}
         {u}
         {vc}",
        t = dt("o.observation_datetime", "o.observation_date"),
        c = concept("c", "o.observation_concept_id"),
        u = concept("u", "o.unit_concept_id"),
        vc = concept("vc", "o.value_as_concept_id"),
    );
    let condition = format!(
        "SELECT 'c' || co.condition_occurrence_id, co.person_id, co.visit_occurrence_id, 'dx',
                {t}, {e}, COALESCE(c.concept_name, co.condition_source_value, ''),
                COALESCE(sc.concept_code, co.condition_source_value, ''), COALESCE(sc.vocabulary_id, ''),
                NULL, '', '', NULL, NULL, '', '', ''
         FROM {s}condition_occurrence co
         {c}
         {sc}",
        t = dt("co.condition_start_datetime", "co.condition_start_date"),
        e = dt("co.condition_end_datetime", "co.condition_end_date"),
        c = concept("c", "co.condition_concept_id"),
        sc = concept("sc", "co.condition_source_concept_id"),
    );
    let drug = format!(
        "SELECT 'd' || de.drug_exposure_id, de.person_id, de.visit_occurrence_id, 'med',
                {t}, {e}, COALESCE(c.concept_name, de.drug_source_value, ''), COALESCE(c.concept_code, ''),
                COALESCE(c.vocabulary_id, ''), NULL, '', COALESCE(de.dose_unit_source_value, ''), NULL, NULL, '', '', ''
         FROM {s}drug_exposure de
         {c}",
        t = dt("de.drug_exposure_start_datetime", "de.drug_exposure_start_date"),
        e = dt("de.drug_exposure_end_datetime", "de.drug_exposure_end_date"),
        c = concept("c", "de.drug_concept_id"),
    );
    let procedure = format!(
        "SELECT 'p' || po.procedure_occurrence_id, po.person_id, po.visit_occurrence_id, 'proc',
                {t}, NULL, COALESCE(c.concept_name, po.procedure_source_value, ''), COALESCE(c.concept_code, ''),
                COALESCE(c.vocabulary_id, ''), NULL, '', '', NULL, NULL, '', '', ''
         FROM {s}procedure_occurrence po
         {c}",
        t = dt("po.procedure_datetime", "po.procedure_date"),
        c = concept("c", "po.procedure_concept_id"),
    );
    let note = format!(
        "SELECT 'n' || n.note_id, n.person_id, n.visit_occurrence_id, 'note',
                {t}, NULL, COALESCE(NULLIF(n.note_title, ''), tc.concept_name, 'Note'), '', '', NULL, '', '',
                NULL, NULL, '', COALESCE(pr.provider_name, ''), COALESCE(n.note_text, '')
         FROM {s}note n
         {tc}
         LEFT JOIN {s}provider pr ON pr.provider_id = n.provider_id",
        t = dt("n.note_datetime", "n.note_date"),
        tc = concept("tc", "n.note_type_concept_id"),
    );
    // the columns are named by the first part
    let entries = format!(
        "{measurement}
         UNION ALL {observation}
         UNION ALL {condition}
         UNION ALL {drug}
         UNION ALL {procedure}
         UNION ALL {note}"
    );
    let version = format!(
        "SELECT COALESCE(MAX({}), '') || ' / ' || COALESCE(MAX({}), '') FROM {s}cdm_source",
        text("source_release_date"),
        text("cdm_release_date")
    );
    Views { persons, visits, entries, recordings: String::new(), stays, version, variables: String::new() }
}
