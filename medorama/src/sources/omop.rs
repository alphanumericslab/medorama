//! The OMOP CDM (5.3, 5.4) as the canonical views (docs/design/sources.md, "OMOP").
//!
//! Tables read: `person`, `visit_occurrence`, `visit_detail`, `care_site`, `measurement`,
//! `observation`, `condition_occurrence`, `drug_exposure`, `procedure_occurrence`, `note`,
//! `provider`, `concept` (in the vocabulary's schema), `cdm_source` (the version).  Times are
//! given as text (the database's local times, read in the connection's time zone); on SQLite,
//! where OMOP's tools may store dates as Unix seconds, those are turned into text first.

use super::{Settings, Views};
use crate::sql::Dialect;

/// The OMOP concepts read: female, male, an intensive-care stay.
const FEMALE: i64 = 8532;
const MALE: i64 = 8507;
const INTENSIVE_CARE: i64 = 32037;

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
    // a date or a time as text
    let dt = |x: &str| match d {
        Dialect::Postgres => format!("CAST({x} AS TEXT)"),
        Dialect::Sqlite => {
            format!("CASE WHEN typeof({x}) IN ('integer', 'real') THEN datetime({x}, 'unixepoch') ELSE {x} END")
        }
    };
    let birth = match d {
        Dialect::Postgres => "CASE WHEN p.birth_datetime IS NOT NULL THEN CAST(CAST(p.birth_datetime AS DATE) AS TEXT)
                 WHEN p.year_of_birth IS NOT NULL AND p.month_of_birth IS NOT NULL AND p.day_of_birth IS NOT NULL
                 THEN CAST(make_date(p.year_of_birth, p.month_of_birth, p.day_of_birth) AS TEXT) END"
            .to_string(),
        Dialect::Sqlite => format!(
            "CASE WHEN p.birth_datetime IS NOT NULL THEN substr({}, 1, 10)
                  WHEN p.year_of_birth IS NOT NULL AND p.month_of_birth IS NOT NULL AND p.day_of_birth IS NOT NULL
                  THEN printf('%04d-%02d-%02d', p.year_of_birth, p.month_of_birth, p.day_of_birth) END",
            dt("p.birth_datetime")
        ),
    };
    let (patient_id, mrn) = match st.identifier.as_str() {
        "patient_id" => ("p.person_source_value", "NULL"),
        "none" => ("NULL", "NULL"),
        _ => ("NULL", "p.person_source_value"),
    };
    let persons = format!(
        "SELECT p.person_id AS person_key, {patient_id} AS patient_id, {mrn} AS mrn, '' AS mrn_authority,
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
         LEFT JOIN {v}concept vc ON vc.concept_id = vo.visit_concept_id",
        start = dt("COALESCE(vo.visit_start_datetime, vo.visit_start_date)"),
        end = dt("COALESCE(vo.visit_end_datetime, vo.visit_end_date)"),
    );
    // entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary,
    // value_num, value_text, unit, ref_low, ref_high, flag, author, text
    let measurement = format!(
        "SELECT 'm' || m.measurement_id AS entry_key, m.person_id AS person_key, m.visit_occurrence_id AS visit_key,
                'lab' AS category, {t} AS time, NULL AS end_time,
                COALESCE(c.concept_name, m.measurement_source_value, '') AS name, COALESCE(c.concept_code, '') AS code,
                COALESCE(c.vocabulary_id, '') AS vocabulary, m.value_as_number AS value_num,
                CASE WHEN m.value_as_number IS NULL THEN COALESCE(vc.concept_name, m.value_source_value, '') ELSE '' END AS value_text,
                COALESCE(u.concept_code, m.unit_source_value, '') AS unit, m.range_low AS ref_low, m.range_high AS ref_high,
                '' AS flag, '' AS author, '' AS text
         FROM {s}measurement m
         LEFT JOIN {v}concept c ON c.concept_id = m.measurement_concept_id
         LEFT JOIN {v}concept u ON u.concept_id = m.unit_concept_id
         LEFT JOIN {v}concept vc ON vc.concept_id = m.value_as_concept_id",
        t = dt("COALESCE(m.measurement_datetime, m.measurement_date)"),
    );
    let observation = format!(
        "SELECT 'o' || o.observation_id, o.person_id, o.visit_occurrence_id, 'obs',
                {t}, NULL, COALESCE(c.concept_name, o.observation_source_value, ''), COALESCE(c.concept_code, ''),
                COALESCE(c.vocabulary_id, ''), o.value_as_number,
                CASE WHEN o.value_as_number IS NULL THEN COALESCE(o.value_as_string, vc.concept_name, '') ELSE '' END,
                COALESCE(u.concept_code, o.unit_source_value, ''), NULL, NULL, '', '', ''
         FROM {s}observation o
         LEFT JOIN {v}concept c ON c.concept_id = o.observation_concept_id
         LEFT JOIN {v}concept u ON u.concept_id = o.unit_concept_id
         LEFT JOIN {v}concept vc ON vc.concept_id = o.value_as_concept_id",
        t = dt("COALESCE(o.observation_datetime, o.observation_date)"),
    );
    let condition = format!(
        "SELECT 'c' || co.condition_occurrence_id, co.person_id, co.visit_occurrence_id, 'dx',
                {t}, {e}, COALESCE(c.concept_name, co.condition_source_value, ''),
                COALESCE(sc.concept_code, co.condition_source_value, ''), COALESCE(sc.vocabulary_id, ''),
                NULL, '', '', NULL, NULL, '', '', ''
         FROM {s}condition_occurrence co
         LEFT JOIN {v}concept c ON c.concept_id = co.condition_concept_id
         LEFT JOIN {v}concept sc ON sc.concept_id = co.condition_source_concept_id",
        t = dt("COALESCE(co.condition_start_datetime, co.condition_start_date)"),
        e = dt("COALESCE(co.condition_end_datetime, co.condition_end_date)"),
    );
    let drug = format!(
        "SELECT 'd' || de.drug_exposure_id, de.person_id, de.visit_occurrence_id, 'med',
                {t}, {e}, COALESCE(c.concept_name, de.drug_source_value, ''), COALESCE(c.concept_code, ''),
                COALESCE(c.vocabulary_id, ''), NULL, '', COALESCE(de.dose_unit_source_value, ''), NULL, NULL, '', '', ''
         FROM {s}drug_exposure de
         LEFT JOIN {v}concept c ON c.concept_id = de.drug_concept_id",
        t = dt("COALESCE(de.drug_exposure_start_datetime, de.drug_exposure_start_date)"),
        e = dt("COALESCE(de.drug_exposure_end_datetime, de.drug_exposure_end_date)"),
    );
    let procedure = format!(
        "SELECT 'p' || po.procedure_occurrence_id, po.person_id, po.visit_occurrence_id, 'proc',
                {t}, NULL, COALESCE(c.concept_name, po.procedure_source_value, ''), COALESCE(c.concept_code, ''),
                COALESCE(c.vocabulary_id, ''), NULL, '', '', NULL, NULL, '', '', ''
         FROM {s}procedure_occurrence po
         LEFT JOIN {v}concept c ON c.concept_id = po.procedure_concept_id",
        t = dt("COALESCE(po.procedure_datetime, po.procedure_date)"),
    );
    let note = format!(
        "SELECT 'n' || n.note_id, n.person_id, n.visit_occurrence_id, 'note',
                {t}, NULL, COALESCE(NULLIF(n.note_title, ''), tc.concept_name, 'Note'), '', '', NULL, '', '',
                NULL, NULL, '', COALESCE(pr.provider_name, ''), COALESCE(n.note_text, '')
         FROM {s}note n
         LEFT JOIN {v}concept tc ON tc.concept_id = n.note_type_concept_id
         LEFT JOIN {s}provider pr ON pr.provider_id = n.provider_id",
        t = dt("COALESCE(n.note_datetime, n.note_date)"),
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
        dt("source_release_date"),
        dt("cdm_release_date")
    );
    Views { persons, visits, entries, version }
}
