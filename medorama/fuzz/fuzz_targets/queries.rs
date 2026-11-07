//! What users type: a cohort expression, a search of the Patient Catalog, a time, a date, a
//! number, a unit; and text whose dates Privacy Mode moves.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let mut resolve = |q: Option<&str>, n: &str| -> anyhow::Result<medorama::cohorts::Criterion> {
        if q == Some("db") || n.is_empty() {
            anyhow::bail!("no such database");
        }
        Ok(medorama::cohorts::Criterion { kind: "column".into(), name: n.into(), import: Some(1), ..Default::default() })
    };
    let _ = medorama::expr::parse(s, &mut resolve);
    let terms = medorama::search::parse(s);
    let _ = medorama::search::check(&terms, &["name", "dob", "unit"], true);
    let _ = medorama::util::parse_time(s, medorama::util::tz("America/New_York"));
    let _ = medorama::util::parse_date(s);
    let _ = medorama::ehr::number(s);
    let _ = medorama::linkage::read_mapping(s);
    let _ = medorama::deid::shift_text(s, 3_159_648_000);
    let _ = medorama::expr::convert(1.0, s, "ms");
    let _ = medorama::tables::measure_column(s);
});
