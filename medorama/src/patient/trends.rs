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
pub fn trends(c: &Connection, user: &User, id: i64) -> anyhow::Result<Vec<Trend>> {
    let _ = user;
    patient_exists(c, id)?;
    let mut st = c.prepare(&format!(
        "SELECT name, category, unit, ref_low, ref_high, time, value_num, flag FROM ehr_items
         WHERE patient = ?1 AND value_num IS NOT NULL AND time IS NOT NULL AND category IN ('lab', 'vit', 'obs', 'ecg', 'echo')
           AND {}
         ORDER BY time, id",
        item_shown("")
    ))?;
    // a series per name and unit: values in mg/dL and in mmol/L are not one line
    let key = |cat: &str, name: &str, unit: &str| {
        (cat.to_string(), name.to_lowercase(), unit.trim().to_lowercase().replace(['µ', 'μ'], "u"))
    };
    let mut by: BTreeMap<(String, String, String), Trend> = BTreeMap::new();
    let mut rows = st.query([id])?;
    while let Some(r) = rows.next()? {
        let (name, cat, unit, lo, hi, t, v, flag): (
            String,
            String,
            String,
            Option<f64>,
            Option<f64>,
            f64,
            f64,
            String,
        ) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?);
        let e = by.entry(key(&cat, &name, &unit)).or_insert(Trend {
            name,
            category: cat,
            unit,
            ref_low: lo,
            ref_high: hi,
            points: vec![],
            n: 0,
        });
        e.points.push((t, v, level(&flag, Some(v), lo, hi)));
        e.n += 1;
    }
    for it in live_items(c, id)?.0 {
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
