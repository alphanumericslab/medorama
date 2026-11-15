//! Every numeric series of a patient.

use super::*;

// ------------------------------------------------------------------ trends

#[derive(Debug, Serialize)]
pub struct Trend {
    pub name: String,
    pub category: String,
    pub unit: String,
    pub ref_low: Option<f64>,
    pub ref_high: Option<f64>,
    /// (time, value, level), at most 300 points.
    pub points: Vec<(f64, f64, u8)>,
    pub n: usize,
}

/// Every numeric series of a patient (laboratory, vital signs, other measurements), most
/// measured first.
pub fn trends(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<Vec<Trend>> {
    patient_exists(c, user, id)?;
    // a series per name and unit: values in mg/dL and in mmol/L are not one line
    let key = |cat: &str, name: &str, unit: &str| {
        (cat.to_string(), name.to_lowercase(), unit.trim().to_lowercase().replace(['µ', 'μ'], "u"))
    };
    let mut by: BTreeMap<(String, String, String), Trend> = BTreeMap::new();
    for it in load(cfg, c, user, id)?.items {
        let (Some(t), Some(v)) = (it.time, it.num) else { continue };
        if !["lab", "vit", "obs", "ecg", "echo"].contains(&it.category.as_str()) {
            continue;
        }
        let e = by.entry(key(&it.category, &it.name, &it.unit)).or_insert(Trend {
            name: it.name.clone(),
            category: it.category.clone(),
            unit: it.unit.clone(),
            ref_low: it.ref_low,
            ref_high: it.ref_high,
            points: vec![],
            n: 0,
        });
        e.points.push((t, v, it.level));
        e.n += 1;
    }
    for t in by.values_mut() {
        t.points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    let mut out: Vec<Trend> = by.into_values().collect();
    for t in out.iter_mut() {
        if t.points.len() > 300 {
            let step = t.points.len() / 300 + 1;
            let last = *t.points.last().unwrap();
            t.points = t.points.iter().step_by(step).copied().collect();
            t.points.push(last);
        }
    }
    out.sort_by(|a, b| b.n.cmp(&a.n).then(a.name.cmp(&b.name)));
    Ok(out)
}
