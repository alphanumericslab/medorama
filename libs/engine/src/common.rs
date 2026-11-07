//! Building blocks shared by all sources (the Python viewer's `common.py`).
//!
//! - channel: one signal as the user sees it (label, unit, modality);
//! - piece: a stretch of a channel the source proves contiguous: `n` samples at `fs` from
//!   `t0` (Unix seconds, UTC); `timed = false` when the file states no time for it;
//! - envelope: per piece, min and max of every block of [`LEVEL0`] samples, and of every
//!   [`FANOUT`] blocks above that (a min/max pyramid).

use indexmap::IndexMap;
use signal_core::dict;
use signal_core::value::{Dict, Value};

pub const LEVEL0: usize = 32;
pub const FANOUT: usize = 4;
/// Bump when the cached index layout changes.
pub const INDEX_VERSION: i64 = 2;
pub const CHUNK: usize = 1 << 16;
/// The most pieces a recording may have: a reader refuses a recording with more
/// (viewers/vitalstream/docs/index-format.md, "Limits").
pub const MAX_PIECES: usize = 100_000;
/// The most distinct marker names (`mk_code` is 16-bit): names after the first
/// `MAX_MARKER_NAMES - 1` share the last code, named [`OTHER_MARKERS`].
pub const MAX_MARKER_NAMES: usize = 32_768;
pub const OTHER_MARKERS: &str = "(other markers)";

pub fn bucket_size(level: usize) -> usize {
    LEVEL0 * FANOUT.pow(level as u32)
}

/// NaN-ignoring min (NumPy `fmin`).
#[inline]
pub fn fmin(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a <= b {
        a
    } else {
        b
    }
}

#[inline]
pub fn fmax(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a >= b {
        a
    } else {
        b
    }
}

pub fn fmin64(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a <= b {
        a
    } else {
        b
    }
}

pub fn fmax64(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a >= b {
        a
    } else {
        b
    }
}

/// Streaming min/max pyramid of one piece (NaN only where a whole bucket is NaN).
#[derive(Default)]
pub struct Envelope {
    mins: Vec<f32>,
    maxs: Vec<f32>,
    rest: Vec<f32>,
}

impl Envelope {
    pub fn add(&mut self, x: &[f32]) {
        for &v in x {
            self.rest.push(v);
            if self.rest.len() == LEVEL0 {
                self.flush_bucket();
            }
        }
    }

    fn flush_bucket(&mut self) {
        let (mut lo, mut hi) = (f32::NAN, f32::NAN);
        for &v in &self.rest {
            lo = fmin(lo, v);
            hi = fmax(hi, v);
        }
        self.mins.push(lo);
        self.maxs.push(hi);
        self.rest.clear();
    }

    /// (min, max) per level, level 0 first, down to a single bucket.
    pub fn finish(mut self) -> Vec<(Vec<f32>, Vec<f32>)> {
        if !self.rest.is_empty() {
            self.flush_bucket();
        }
        let mut levels = vec![(self.mins, self.maxs)];
        while levels.last().unwrap().0.len() > 1 {
            let (lo, hi) = levels.last().unwrap();
            let n = lo.len().div_ceil(FANOUT);
            let mut nlo = Vec::with_capacity(n);
            let mut nhi = Vec::with_capacity(n);
            for k in 0..n {
                let (mut a, mut b) = (f32::NAN, f32::NAN);
                for j in k * FANOUT..((k + 1) * FANOUT).min(lo.len()) {
                    a = fmin(a, lo[j]);
                    b = fmax(b, hi[j]);
                }
                nlo.push(a);
                nhi.push(b);
            }
            levels.push((nlo, nhi));
        }
        levels
    }
}

/// Channel registry: one id per key, first-seen order.
#[derive(Default)]
pub struct Channels {
    pub items: Vec<Dict>,
    ids: IndexMap<String, usize>,
}

impl Channels {
    pub fn get(&mut self, key: String, label: &str, unit: &str, modality: &str, extra: Option<(&str, Value)>) -> usize {
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = self.items.len();
        let mut d = dict! { "id" => id, "label" => label, "unit" => unit, "modality" => modality };
        if let Some((k, v)) = extra {
            d.insert(k.into(), v);
        }
        self.items.push(d);
        self.ids.insert(key, id);
        id
    }
}

/// Arrays of an index (see `index.rs` for the files).
#[derive(Default, Debug)]
pub struct Arrays {
    pub env_min: Vec<Vec<f32>>,
    pub env_max: Vec<Vec<f32>>,
    pub mk_piece: Vec<i32>,
    pub mk_sample: Vec<i64>,
    pub mk_code: Vec<i16>,
    pub vit_t: Vec<f64>,
    pub vit_v: Vec<f64>,
}

/// Pieces with their envelopes and markers.
#[derive(Default)]
pub struct Pieces {
    pub items: Vec<Dict>,
    envelopes: Vec<Envelope>,
    markers: Vec<(i32, i64, i16)>,
    pub marker_names: Vec<String>,
    names: IndexMap<String, usize>,
}

impl Pieces {
    /// A new piece of channel `ch` at `fs`; `info`: source-specific fields.  A reader checks
    /// [`Pieces::room`] first ([`MAX_PIECES`]).
    pub fn start(&mut self, ch: usize, fs: f64, info: Dict) -> usize {
        let id = self.items.len();
        let mut d = dict! { "id" => id, "ch" => ch, "fs" => fs, "n" => 0i64 };
        d.extend(info);
        self.items.push(d);
        self.envelopes.push(Envelope::default());
        id
    }

    /// An error unless `more` pieces still fit within [`MAX_PIECES`].
    pub fn room(&self, more: usize) -> signal_core::Result<()> {
        if self.items.len().saturating_add(more) > MAX_PIECES {
            return Err(signal_core::Error::Value(format!(
                "the recording has more than {MAX_PIECES} pieces (stretches of one signal): it is not indexed"
            )));
        }
        Ok(())
    }

    pub fn n(&self, p: usize) -> i64 {
        self.items[p].get("n").and_then(Value::as_int).unwrap_or(0) as i64
    }

    pub fn add(&mut self, p: usize, values: &[f32]) {
        self.envelopes[p].add(values);
        let n = self.n(p) + values.len() as i64;
        self.items[p].insert("n".into(), n.into());
    }

    pub fn marker(&mut self, p: usize, sample: i64, name: &str) {
        let i = match self.names.get(name) {
            Some(&i) => i,
            // every code taken: the rest share the last one
            None if self.marker_names.len() == MAX_MARKER_NAMES - 1 => {
                self.marker_names.push(OTHER_MARKERS.to_string());
                self.names.insert(OTHER_MARKERS.to_string(), MAX_MARKER_NAMES - 1);
                MAX_MARKER_NAMES - 1
            }
            None if self.marker_names.len() == MAX_MARKER_NAMES => MAX_MARKER_NAMES - 1,
            None => {
                self.marker_names.push(name.to_string());
                self.names.insert(name.to_string(), self.marker_names.len() - 1);
                self.marker_names.len() - 1
            }
        };
        // pieces are fewer than MAX_PIECES, far below i32::MAX
        self.markers.push((p as i32, sample, i as i16));
    }

    /// Whether more marker names were found than codes exist (the rest share one name).
    pub fn markers_capped(&self) -> bool {
        self.marker_names.len() == MAX_MARKER_NAMES
    }

    /// Envelope offsets into the pieces, and the arrays.
    pub fn finish(self) -> (Vec<Dict>, Arrays, Vec<String>) {
        let mut items = self.items;
        let mut a = Arrays::default();
        for (p, env) in self.envelopes.into_iter().enumerate() {
            let levels = env.finish();
            let mut offs = Vec::new();
            for (lvl, (lo, hi)) in levels.into_iter().enumerate() {
                if a.env_min.len() <= lvl {
                    a.env_min.push(vec![]);
                    a.env_max.push(vec![]);
                }
                offs.push(Value::List(vec![a.env_min[lvl].len().into(), lo.len().into()]));
                a.env_min[lvl].extend(lo);
                a.env_max[lvl].extend(hi);
            }
            items[p].insert("env".into(), Value::List(offs));
        }
        let mut mk = self.markers;
        mk.sort_by_key(|m| (m.0, m.1)); // stable: ties keep their order
        a.mk_piece = mk.iter().map(|m| m.0).collect();
        a.mk_sample = mk.iter().map(|m| m.1).collect();
        a.mk_code = mk.iter().map(|m| m.2).collect();
        (items, a, self.marker_names)
    }
}

/// Python `float(x)` for values from a file: numbers and numeric strings.
pub fn to_float(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        Value::Bool(b) => Some(*b as i64 as f64),
        Value::Str(s) => signal_core::text::parse_float(s),
        _ => None,
    }
}

/// Vital-sign series: name -> times and values (finite values only).
#[derive(Default)]
pub struct Vitals {
    series: IndexMap<String, (String, Vec<f64>, Vec<f64>)>,
}

impl Vitals {
    pub fn add(&mut self, name: &str, unit: &Value, t: &Value, value: &Value) {
        let (Some(t), Some(v)) = (to_float(t), to_float(value)) else { return };
        if !(t.is_finite() && v.is_finite()) {
            return;
        }
        let s = self.series.entry(name.to_string()).or_insert_with(|| {
            let u = if unit.truthy() { unit.py_str() } else { String::new() };
            (u, vec![], vec![])
        });
        s.1.push(t);
        s.2.push(v);
    }

    pub fn finish(self) -> (Vec<Dict>, Vec<f64>, Vec<f64>) {
        let mut names: Vec<&String> = self.series.keys().collect();
        names.sort_by(|a, b| self.series[*b].1.len().cmp(&self.series[*a].1.len()).then(a.cmp(b)));
        let (mut meta, mut ts, mut vs) = (vec![], vec![], vec![]);
        for name in names {
            let (unit, t, v) = &self.series[name];
            let mut order: Vec<usize> = (0..t.len()).collect();
            order.sort_by(|&a, &b| t[a].partial_cmp(&t[b]).unwrap_or(std::cmp::Ordering::Equal));
            meta.push(dict! { "name" => name.clone(), "unit" => unit.clone(), "offset" => ts.len(), "n" => t.len() });
            ts.extend(order.iter().map(|&i| t[i]));
            vs.extend(order.iter().map(|&i| v[i]));
        }
        (meta, ts, vs)
    }
}

pub fn t0_of(d: &Dict) -> f64 {
    d.get("t0").and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// `t` if it lies within one day of [lo, hi], else None (e.g. .NET zero dates).
pub fn plausible_time(t: &Value, lo: Option<f64>, hi: Option<f64>) -> Option<f64> {
    let t = to_float(t)?;
    let (lo, hi) = (lo?, hi?);
    (t.is_finite() && lo - 86400.0 <= t && t <= hi + 86400.0).then_some(t)
}

/// Merged [t0, t1] spans covered by pieces whose time is not stated.
pub fn untimed_spans(pieces: &[Dict]) -> Vec<Value> {
    let mut spans: Vec<(f64, f64)> = pieces
        .iter()
        .filter(|p| p.get("timed") == Some(&Value::Bool(false)))
        .map(|p| {
            let t0 = p.get("t0").and_then(Value::as_f64).unwrap_or(0.0);
            let n = p.get("n").and_then(Value::as_f64).unwrap_or(0.0);
            let fs = p.get("fs").and_then(Value::as_f64).unwrap_or(f64::NAN);
            (t0, t0 + n / fs)
        })
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(l) if a <= l.1 + 1e-9 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out.into_iter().map(|(a, b)| Value::Dict(dict! { "t0" => a, "t1" => b })).collect()
}

/// JSON-safe copy: NaN and infinities become null.
pub fn clean(v: Value) -> Value {
    match v {
        Value::Float(f) if !f.is_finite() => Value::None,
        Value::List(l) => Value::List(l.into_iter().map(clean).collect()),
        Value::Dict(d) => Value::Dict(d.into_iter().map(|(k, x)| (k, clean(x))).collect()),
        x => x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_levels() {
        let x: Vec<f32> = (0..10_007).map(|i| ((i * 7919) % 1000) as f32 - 500.0).collect();
        let mut x = x;
        for v in &mut x[100..140] {
            *v = f32::NAN;
        }
        let mut env = Envelope::default();
        for c in x.chunks(999) {
            env.add(c);
        }
        let levels = env.finish();
        assert_eq!(levels[0].0.len(), x.len().div_ceil(LEVEL0));
        for (i, b) in x.chunks(LEVEL0).enumerate() {
            let valid: Vec<f32> = b.iter().copied().filter(|v| !v.is_nan()).collect();
            if valid.is_empty() {
                assert!(levels[0].0[i].is_nan());
            } else {
                assert_eq!(levels[0].0[i], valid.iter().copied().fold(f32::INFINITY, f32::min));
            }
        }
        assert_eq!(levels.last().unwrap().0.len(), 1);
    }

    #[test]
    fn marker_names_and_pieces_are_bounded() {
        let mut p = Pieces::default();
        let id = p.start(0, 1.0, Dict::new());
        for k in 0..MAX_MARKER_NAMES + 10 {
            p.marker(id, k as i64, &format!("m{k}"));
        }
        assert!(p.markers_capped());
        let (_, a, names) = p.finish();
        assert_eq!(names.len(), MAX_MARKER_NAMES);
        assert_eq!(names[MAX_MARKER_NAMES - 1], OTHER_MARKERS);
        assert_eq!(a.mk_code[MAX_MARKER_NAMES - 2], (MAX_MARKER_NAMES - 2) as i16);
        assert!(a.mk_code[MAX_MARKER_NAMES - 1..].iter().all(|&c| c == i16::MAX));
        let p = Pieces::default();
        assert!(p.room(MAX_PIECES).is_ok() && p.room(MAX_PIECES + 1).is_err());
    }

    #[test]
    fn untimed_spans_with_nan() {
        let piece = |t0: f64, fs: f64| dict! { "t0" => t0, "n" => 10i64, "fs" => fs, "timed" => false };
        // a NaN rate (an index written before rates were checked) does not panic the sort
        let spans = untimed_spans(&[piece(5.0, f64::NAN), piece(0.0, 1.0), piece(f64::NAN, 1.0)]);
        assert!(!spans.is_empty());
    }
}
