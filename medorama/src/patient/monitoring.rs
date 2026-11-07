//! ICU monitoring read from a recording's index: waveforms, vital signs, gaps.

use super::*;

// ------------------------------------------------------------------ ICU monitoring from the index

pub(super) fn read_f64(path: &Path, offset: u64, n: usize) -> Option<Vec<f64>> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset * 8)).ok()?;
    let mut buf = vec![0u8; n * 8];
    f.read_exact(&mut buf).ok()?;
    Some(buf.as_chunks::<8>().0.iter().map(|b| f64::from_le_bytes(*b)).collect())
}

/// What a recording's index holds: waveform and vital-sign names, the main vital signs over
/// the whole recording (at most 600 points each), its gaps.
pub(super) fn index_overview(c: &Connection, cfg: &Config, r: &crate::recordings::Recording) -> Option<Monitoring> {
    let key = r.index_key.clone().unwrap_or_else(|| r.fingerprint.clone());
    let folder = cfg.index_dir.join(key);
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("meta.json")).ok()?).ok()?;
    let mut waveforms: Vec<String> = vec![];
    for ch in meta.get("channels").and_then(|v| v.as_array()).into_iter().flatten() {
        if let Some(l) = ch.get("label").and_then(|v| v.as_str()) {
            if !waveforms.iter().any(|w| w == l) {
                waveforms.push(l.to_string());
            }
        }
    }
    let vit: Vec<(String, String, u64, usize)> = meta
        .get("vitals")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some((
                v.get("name")?.as_str()?.to_string(),
                v.get("unit").and_then(|u| u.as_str()).unwrap_or("").to_string(),
                v.get("offset")?.as_u64()?,
                v.get("n")?.as_u64()? as usize,
            ))
        })
        .collect();
    // the main vital signs, in this order, by their usual names
    // (GE: HR, AR1-M, SPO2-%, CO2-RR; Philips: HR, ABPm, SpO₂, RESP)
    let wanted: [&[&str]; 4] = [
        &["hr", "heart rate", "pulse"],
        &["ar1-m", "ar2-m", "abpm", "abp-m", "art-m", "map", "abp", "art", "nbp-m", "nbpm"],
        &["spo2", "sao2", "spo₂"],
        &["co2-rr", "rr", "resp", "awrr"],
    ];
    let mut picked: Vec<usize> = vec![];
    for names in wanted {
        let found = vit.iter().enumerate().find(|(i, (n, ..))| {
            let n = n.to_lowercase();
            !picked.contains(i)
                && names.iter().any(|w| n == *w || n.starts_with(&format!("{w} ")) || (w.len() > 3 && n.starts_with(w)))
        });
        if let Some((i, _)) = found {
            picked.push(i);
        }
    }
    for i in 0..vit.len() {
        if picked.len() >= 4 {
            break;
        }
        if !picked.contains(&i) {
            picked.push(i);
        }
    }
    // derived series first: at most 600 points over the whole recording (a week: one per 17 min),
    // each the mean of its minutes, with the lowest and highest of their ranges
    let mut trends = vec![];
    let derived = crate::derived::load(c, r.id).unwrap_or_default();
    for d in &derived {
        let step = (d.t.len() / 600).max(1);
        let mut st =
            Series { name: d.name.clone(), unit: d.unit.clone(), t: vec![], v: vec![], lo: vec![], hi: vec![] };
        for i in (0..d.t.len()).step_by(step) {
            let j = (i + step).min(d.t.len());
            let k = (j - i) as f64;
            st.t.push(d.t[i..j].iter().sum::<f64>() / k);
            st.v.push((d.v[i..j].iter().sum::<f64>() / k * 10.0).round() / 10.0);
            st.lo.push(d.lo[i..j].iter().copied().fold(f64::INFINITY, f64::min));
            st.hi.push(d.hi[i..j].iter().copied().fold(f64::NEG_INFINITY, f64::max));
        }
        trends.push(st);
    }
    picked.truncate(4usize.saturating_sub(trends.len()));
    for i in picked {
        let (name, unit, off, n) = &vit[i];
        let (Some(t), Some(v)) =
            (read_f64(&folder.join("vit_t.f64"), *off, *n), read_f64(&folder.join("vit_v.f64"), *off, *n))
        else {
            continue;
        };
        let pts: Vec<(f64, f64)> = t.into_iter().zip(v).filter(|(a, b)| a.is_finite() && b.is_finite()).collect();
        if pts.is_empty() {
            continue;
        }
        let step = (pts.len() / 600).max(1);
        let mut st = Series { name: name.clone(), unit: unit.clone(), t: vec![], v: vec![], lo: vec![], hi: vec![] };
        for chunk in pts.chunks(step) {
            st.t.push(chunk.iter().map(|p| p.0).sum::<f64>() / chunk.len() as f64);
            st.v.push(chunk.iter().map(|p| p.1).sum::<f64>() / chunk.len() as f64);
        }
        trends.push(st);
    }
    let gaps: Vec<(f64, f64)> = meta
        .get("gaps")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g.get("t0")?.as_f64()?, g.get("t1")?.as_f64()?)))
        .collect();
    let recorded = match (r.t_start, r.t_end) {
        (Some(a), Some(b)) if b > a => {
            Some(1.0 - gaps.iter().map(|(x, y)| (y.min(b) - x.max(a)).max(0.0)).sum::<f64>() / (b - a))
        }
        _ => None,
    };
    Some(Monitoring {
        recording: r.clone(),
        waveforms,
        vitals: vit.into_iter().map(|v| v.0).collect(),
        derived: derived.into_iter().map(|d| d.name).collect(),
        trends,
        gaps,
        recorded,
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn thinned_series_keep_the_shown_point() {
        let series: Vec<(f64, f64)> = (0..1000).map(|i| (i as f64, i as f64 * 0.5)).collect();
        for keep in [0.0, 1.0, 437.0, 999.0] {
            let t = thin(series.clone(), Some(keep), 80);
            assert!(t.len() <= 90 && t.iter().any(|p| p.0 == keep), "{keep}: {}", t.len());
            assert!(t.windows(2).all(|w| w[0].0 < w[1].0), "in time order, once each");
            assert_eq!((t[0].0, t[t.len() - 1].0), (0.0, 999.0), "the ends kept");
        }
        assert_eq!(thin(series[..50].to_vec(), None, 80).len(), 50, "short series as they are");
    }
    use super::*;

    #[test]
    fn levels_and_words() {
        assert_eq!(level("H", Some(5.0), None, None), 1);
        assert_eq!(level("", Some(5.0), Some(1.0), Some(4.0)), 1);
        assert_eq!(level("critical", None, None, None), 2);
        assert_eq!(level("", Some(3.0), Some(1.0), Some(4.0)), 0);
        let (w, c) = search_words("ecg lactate > 4 hb<8 \"st depression\"");
        assert_eq!(w, vec!["ecg", "st depression"]);
        assert_eq!(c, vec![("lactate".into(), ">".into(), 4.0), ("hb".into(), "<".into(), 8.0)]);
        assert_eq!(distance("sepsis", "sepsys"), 1);
        assert_eq!(snippet("No signs of sepsis today.", &["sepsis".into()]).unwrap(), "No signs of sepsis today.");
    }
}
