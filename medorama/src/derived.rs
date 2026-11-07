//! Series derived from a recording's waveforms, for recordings whose monitor reported none: a
//! Holter recording has ECG leads only, so its heart rate is found from the ECG itself.  They
//! are computed after indexing, kept in the catalog's database (not in the index, whose format
//! the viewers share: docs/index-format.md), and shown with the recording on the patient page.
//!
//! Heart rate: QRS complexes are found on one lead (II if there is one) with a derivative,
//! squaring and moving-window integration detector and an adaptive threshold (after Pan and
//! Tompkins, 1985), read an hour at a time so that a week-long recording needs little memory.
//! Each minute gets the median of its beat-to-beat rates, and the 5th and 95th percentiles as
//! its range; a minute with fewer than 10 plausible beats (30 to 250 bpm) is left out.

use std::path::Path;

use crate::sql::{params, Connection};
use recording_engine::dataset::Dataset;

/// A derived series: per minute, a value and its range.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub name: String,
    pub unit: String,
    pub t: Vec<f64>,
    pub v: Vec<f64>,
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
}

pub const HEART_RATE: &str = "Heart rate (ECG)";

/// Names of ECG leads, as WFDB headers and monitors write them (lower case).
const LEADS: &[&str] = &[
    "ii", "i", "iii", "avr", "avl", "avf", "v1", "v2", "v3", "v4", "v5", "v6", "ecg", "ecg1", "ecg2", "ecg ii", "mlii",
    "ml2", "cm5", "ch1", "ch2",
];

fn is_ecg(label: &str, modality: &str, unit: &str) -> bool {
    let l = label.trim().to_lowercase();
    modality.eq_ignore_ascii_case("ecg")
        || LEADS.contains(&l.as_str())
        || (l.starts_with("ecg") && unit.eq_ignore_ascii_case("mv"))
}

/// The series to derive for an index: the heart rate, when the recording has an ECG lead and
/// no heart rate of its own.  `progress` goes from 0 to 1.
pub fn derive(index_dir: &Path, key: &str, progress: &dyn Fn(f64)) -> anyhow::Result<Vec<Derived>> {
    let Some((meta, arrays)) = recording_engine::index::load_index(index_dir, key) else { return Ok(vec![]) };
    let ds = Dataset::new(meta, arrays);
    let info: serde_json::Value = serde_json::from_str(&ds.info().to_json()).unwrap_or_default();
    let has_hr = info.get("vitals").and_then(|v| v.as_array()).into_iter().flatten().any(|v| {
        let n = v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_lowercase();
        n == "hr" || n.starts_with("hr ") || n.starts_with("heart rate") || n == "pulse"
    });
    if has_hr {
        return Ok(vec![]);
    }
    // the lead: II if there is one, else the first ECG lead
    let mut leads: Vec<(i64, String)> = vec![];
    for ch in info.get("channels").and_then(|v| v.as_array()).into_iter().flatten() {
        let s = |k: &str| ch.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        if let Some(id) = ch.get("id").and_then(|v| v.as_i64()) {
            if is_ecg(&s("label"), &s("modality"), &s("unit")) {
                leads.push((id, s("label").to_lowercase()));
            }
        }
    }
    let Some(lead) = leads.iter().find(|l| l.1 == "ii").or(leads.first()).map(|l| l.0) else { return Ok(vec![]) };
    // untimed pieces too: their beats are placed where the viewer places the data
    let pieces: Vec<usize> = ds.pieces.iter().filter(|p| p.ch == lead && p.fs >= 50.0).map(|p| p.id).collect();
    let total: i64 = pieces.iter().map(|&p| ds.pieces[p].n).sum::<i64>().max(1);
    let mut done = 0i64;
    let mut beats: Vec<f64> = vec![];
    for &p in &pieces {
        let pc = &ds.pieces[p];
        let (fs, n, t0) = (pc.fs, pc.n, pc.t0);
        let hour = (3600.0 * fs) as i64;
        let over = (2.0 * fs) as i64;
        let mut a = 0i64;
        while a < n {
            let b = (a + hour).min(n);
            let lo = (a - over).max(0);
            let x = ds.samples(p, lo, b)?;
            for s in qrs(&x, fs) {
                let k = lo + s as i64;
                if k >= a {
                    beats.push(t0 + k as f64 / fs);
                }
            }
            done += b - a;
            progress(done as f64 / total as f64);
            a = b;
        }
    }
    let hr = per_minute(&beats);
    Ok(if hr.t.is_empty() { vec![] } else { vec![hr] })
}

/// Sample numbers of the R peaks of an ECG stretch.
pub fn qrs(x: &[f64], fs: f64) -> Vec<usize> {
    let n = x.len();
    if n < (fs * 2.0) as usize {
        return vec![];
    }
    // the slope, squared, integrated over 150 ms; NaN (no data) counts as flat
    let d: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = (x[i.saturating_sub(1)], x[(i + 1).min(n - 1)]);
            let s = if a.is_finite() && b.is_finite() { b - a } else { 0.0 };
            s * s
        })
        .collect();
    let w = ((0.15 * fs) as usize).max(1);
    let mut m = vec![0.0; n];
    let mut acc = 0.0;
    for i in 0..n {
        acc += d[i];
        if i >= w {
            acc -= d[i - w];
        }
        m[i] = acc / w as f64;
    }
    // threshold: a third of the 98th percentile of each 10 s (a noise-robust "typical QRS")
    let block = (10.0 * fs) as usize;
    let refractory = (0.25 * fs) as usize;
    let mut peaks = vec![];
    let mut last: Option<usize> = None;
    let mut start = 0;
    while start < n {
        let end = (start + block).min(n);
        let mut s: Vec<f64> = m[start..end].to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        let thr = s[((s.len() - 1) as f64 * 0.98) as usize] / 3.0;
        if thr > 0.0 {
            let mut i = start;
            while i < end {
                if m[i] > thr {
                    // the region above threshold; the R peak is the largest |x| change in it,
                    // found on the signal itself, a little before the integrator's peak
                    let mut j = i;
                    while j < n && m[j] > thr {
                        j += 1;
                    }
                    let a = i.saturating_sub(w);
                    let base = median(&x[a..j.min(n)]);
                    let r = (a..j.min(n))
                        .filter(|&k| x[k].is_finite())
                        .max_by(|&p, &q| (x[p] - base).abs().total_cmp(&(x[q] - base).abs()));
                    if let Some(r) = r {
                        if last.is_none_or(|l| r >= l + refractory) {
                            peaks.push(r);
                            last = Some(r);
                        }
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
        start = end;
    }
    peaks
}

fn median(x: &[f64]) -> f64 {
    let mut v: Vec<f64> = x.iter().copied().filter(|v| v.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Per minute: the median beat-to-beat rate and its 5th–95th percentile range.
fn per_minute(beats: &[f64]) -> Derived {
    let mut out = Derived { name: HEART_RATE.into(), unit: "bpm".into(), t: vec![], v: vec![], lo: vec![], hi: vec![] };
    let mut minute = f64::NAN;
    let mut rates: Vec<f64> = vec![];
    let flush = |minute: f64, rates: &mut Vec<f64>, out: &mut Derived| {
        if rates.len() >= 10 {
            out.t.push(minute * 60.0 + 30.0);
            out.v.push((percentile(rates, 0.5) * 10.0).round() / 10.0);
            out.lo.push((percentile(rates, 0.05) * 10.0).round() / 10.0);
            out.hi.push((percentile(rates, 0.95) * 10.0).round() / 10.0);
        }
        rates.clear();
    };
    for w in beats.windows(2) {
        let rr = w[1] - w[0];
        let m = (w[1] / 60.0).floor();
        if m != minute {
            flush(minute, &mut rates, &mut out);
            minute = m;
        }
        if rr > 0.24 && rr < 2.0 {
            rates.push(60.0 / rr);
        }
    }
    flush(minute, &mut rates, &mut out);
    out
}

fn blob(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn unblob(b: &[u8]) -> Vec<f64> {
    // (by index: `as_chunks` is newer than the Rust version Medorama builds with)
    (0..b.len() / 8).map(|i| f64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap())).collect()
}

/// Replace a recording's derived series.
pub fn store(c: &Connection, recording: i64, series: &[Derived]) -> anyhow::Result<()> {
    c.execute("DELETE FROM recording_series WHERE recording = ?1", [recording])?;
    for s in series {
        c.execute(
            "INSERT INTO recording_series (recording, name, unit, t, v, lo, hi) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![recording, s.name, s.unit, blob(&s.t), blob(&s.v), blob(&s.lo), blob(&s.hi)],
        )?;
    }
    Ok(())
}

/// A recording's derived series.
pub fn load(c: &Connection, recording: i64) -> anyhow::Result<Vec<Derived>> {
    let mut st =
        c.prepare("SELECT name, unit, t, v, lo, hi FROM recording_series WHERE recording = ?1 ORDER BY name")?;
    let v = st
        .query_map([recording], |r| {
            Ok(Derived {
                name: r.get(0)?,
                unit: r.get(1)?,
                t: unblob(&r.get::<_, Vec<u8>>(2)?),
                v: unblob(&r.get::<_, Vec<u8>>(3)?),
                lo: unblob(&r.get::<_, Vec<u8>>(4)?),
                hi: unblob(&r.get::<_, Vec<u8>>(5)?),
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic ECG: narrow R waves at the given rate, a slow baseline and noise.
    fn ecg(fs: f64, seconds: f64, bpm: f64) -> Vec<f64> {
        let n = (fs * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                let ph = (t * bpm / 60.0).fract();
                let r = (-((ph - 0.3) / 0.012).powi(2)).exp() * 1.2 - (-((ph - 0.33) / 0.01).powi(2)).exp() * 0.25;
                let tw = (-((ph - 0.6) / 0.05).powi(2)).exp() * 0.3;
                r + tw + 0.2 * (t * 0.3 * std::f64::consts::TAU).sin() + 0.01 * ((i * 7919 % 13) as f64 - 6.0) / 6.0
            })
            .collect()
    }

    #[test]
    fn heart_rate_from_ecg() {
        for (fs, bpm) in [(128.0, 60.0), (250.0, 75.0), (500.0, 140.0)] {
            let x = ecg(fs, 120.0, bpm);
            let p = qrs(&x, fs);
            let expected = 120.0 * bpm / 60.0;
            assert!((p.len() as f64 - expected).abs() <= 2.0, "{fs} Hz {bpm} bpm: {} beats", p.len());
            let beats: Vec<f64> = p.iter().map(|&s| s as f64 / fs).collect();
            let hr = per_minute(&beats);
            assert_eq!(hr.t.len(), 2);
            assert!(hr.v.iter().all(|v| (v - bpm).abs() < 2.0), "{:?}", hr.v);
        }
        assert!(qrs(&[0.0; 100], 128.0).is_empty());
        assert!(qrs(&vec![0.0; 5000], 128.0).is_empty(), "a flat line has no beats");
    }

    #[test]
    fn ecg_leads() {
        assert!(is_ecg("II", "wfdb", "mV"));
        assert!(is_ecg("V5", "", "mV"));
        assert!(is_ecg("anything", "ecg", ""));
        assert!(!is_ecg("SPO2", "spo2", "adu"));
        assert!(!is_ecg("ABP", "wfdb", "mmHg"));
    }
}
