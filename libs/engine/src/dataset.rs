//! A loaded source: answers window, vital-sign and marker queries from its index (the
//! Python viewer's `dataset.py`; responses are the same JSON).

use crate::common::{bucket_size, fmax64, fmin64, LEVEL0};
use crate::index::Loaded;
use crate::sources::Reader;
use indexmap::IndexMap;
use signal_core::dict;
use signal_core::pyfmt::{g, repr};
use signal_core::value::{Dict, Value};

/// Send samples as they are up to this many per pixel.
const RAW_PER_PX: f64 = 2.0;
/// Never decode more samples than this for one window.
const MAX_RAW: i64 = 2_000_000;
/// vitals_at: values older than this are not reported.
const VITAL_MAX_AGE_S: f64 = 600.0;

pub struct Piece {
    pub id: usize,
    pub ch: i64,
    pub fs: f64,
    pub n: i64,
    pub t0: f64,
    pub timed: bool,
    pub env: Vec<(usize, usize)>,
}

impl Piece {
    fn end(&self) -> f64 {
        self.t0 + self.n as f64 / self.fs
    }

    /// Samples [a, b) of the piece that a window [t0, t1] takes: one sample more before, two
    /// after; within [0, n] (any finite times).
    fn span(&self, t0: f64, t1: f64) -> (i64, i64) {
        let n = self.n as f64;
        let a = (((t0 - self.t0) * self.fs).floor() - 1.0).clamp(0.0, n);
        let b = (((t1 - self.t0) * self.fs).ceil() + 2.0).clamp(0.0, n);
        (a as i64, b as i64)
    }
}

/// How a block of a window is made: samples decoded (`per_px` samples a pixel), or the index's
/// envelope.
enum Plan {
    Raw(usize, i64, f64),
    Env(usize, i64, i64, f64),
}

/// The pieces of one channel, for finding those in a window quickly: in order of their start
/// (then id), the latest end of the pieces up to each, and the margin around a piece's span
/// that a window may still take samples from (three samples at its lowest rate).
#[derive(Default)]
struct ChannelPieces {
    order: Vec<usize>,
    max_end: Vec<f64>,
    margin: f64,
}

pub struct Dataset {
    pub meta: Dict,
    pub arrays: Loaded,
    pub pieces: Vec<Piece>,
    by_channel: IndexMap<i64, ChannelPieces>,
    reader: Reader,
    vitals: IndexMap<String, (String, usize, usize)>,
}

fn json_array(x: &[f64], digits: usize) -> String {
    let mut s = String::with_capacity(x.len() * (digits + 2) + 2);
    s.push('[');
    for (i, &v) in x.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        if v.is_finite() {
            s.push_str(&g(v, digits));
        } else {
            s.push_str("null");
        }
    }
    s.push(']');
    s
}

/// Index of the first element > x (`side='right'`) or >= x (`side='left'`).
fn search(v: &[f64], x: f64, right: bool) -> usize {
    if right {
        v.partition_point(|&a| a <= x)
    } else {
        v.partition_point(|&a| a < x)
    }
}

impl Dataset {
    pub fn new(meta: Dict, arrays: Loaded) -> Dataset {
        let pieces: Vec<Piece> = meta
            .get("pieces")
            .and_then(Value::as_list)
            .map(|l| {
                l.iter()
                    .map(|p| Piece {
                        id: p.get("id").and_then(Value::as_int).unwrap_or(0) as usize,
                        ch: p.get("ch").and_then(Value::as_int).unwrap_or(0) as i64,
                        // load_index checks that every piece states its rate and start
                        fs: p.get("fs").and_then(Value::as_f64).unwrap_or(f64::NAN),
                        n: p.get("n").and_then(Value::as_int).unwrap_or(0) as i64,
                        t0: p.get("t0").and_then(Value::as_f64).unwrap_or(0.0),
                        timed: p.get("timed") != Some(&Value::Bool(false)),
                        env: p.get("env").and_then(Value::as_list).map_or(vec![], |e| {
                            e.iter()
                                .filter_map(|x| {
                                    let x = x.as_list()?;
                                    Some((x[0].as_int()? as usize, x[1].as_int()? as usize))
                                })
                                .collect()
                        }),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut by_channel: IndexMap<i64, ChannelPieces> = IndexMap::new();
        for (i, p) in pieces.iter().enumerate() {
            by_channel.entry(p.ch).or_default().order.push(i);
        }
        for c in by_channel.values_mut() {
            c.order.sort_by(|&a, &b| pieces[a].t0.total_cmp(&pieces[b].t0).then(a.cmp(&b)));
            let mut latest = f64::NEG_INFINITY;
            c.max_end = c
                .order
                .iter()
                .map(|&i| {
                    latest = latest.max(pieces[i].end());
                    latest
                })
                .collect();
            c.margin = c.order.iter().map(|&i| 3.0 / pieces[i].fs).fold(0.0, f64::max);
        }
        let vitals = meta
            .get("vitals")
            .and_then(Value::as_list)
            .map(|l| {
                l.iter()
                    .map(|v| {
                        let s = |k: &str| v.get(k).map(Value::py_str).unwrap_or_default();
                        let i = |k: &str| v.get(k).and_then(Value::as_int).unwrap_or(0) as usize;
                        (s("name"), (s("unit"), i("offset"), i("n")))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let reader = Reader::new(&meta, &arrays.dir);
        Dataset { meta, arrays, pieces, by_channel, reader, vitals }
    }

    /// The number of channels (their ids are 0 to this, excluded).
    pub fn channel_count(&self) -> usize {
        self.meta.get("channels").and_then(Value::as_list).map_or(0, |l| l.len())
    }

    fn m(&self, k: &str) -> Value {
        self.meta.get(k).cloned().unwrap_or(Value::None)
    }

    /// The dataset description (`GET datasets/{key}`).
    pub fn info(&self) -> Value {
        let mut starts: Vec<f64> = self.pieces.iter().map(|p| p.t0).collect();
        let mut ends: Vec<f64> = self.pieces.iter().map(|p| p.t0 + p.n as f64 / p.fs).collect();
        let vt = self.arrays.vit_t.as_slice();
        if !vt.is_empty() {
            starts.push(vt.iter().copied().fold(f64::INFINITY, f64::min));
            ends.push(vt.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        }
        let t_lo = starts.iter().copied().reduce(f64::min);
        let t_hi = ends.iter().copied().reduce(f64::max);
        let pieces: Vec<Value> = self
            .pieces
            .iter()
            .map(|p| {
                Value::Dict(
                    dict! { "id" => p.id, "ch" => p.ch, "t0" => p.t0, "fs" => p.fs, "n" => p.n, "timed" => p.timed },
                )
            })
            .collect();
        let vitals: Vec<Value> = self
            .vitals
            .iter()
            .map(|(k, (u, _, n))| Value::Dict(dict! { "name" => k.clone(), "unit" => u.clone(), "n" => *n }))
            .collect();
        Value::Dict(dict! {
            "key" => self.m("key"), "name" => self.m("name"), "path" => self.m("path"), "kind" => self.m("kind"),
            "format" => self.m("format"), "description" => self.m("description"), "t_range" => Value::List(vec![t_lo.into(), t_hi.into()]),
            "time_resolution_s" => self.m("time_resolution_s"), "channels" => self.m("channels"),
            "pieces" => Value::List(pieces), "vitals" => Value::List(vitals), "alarms" => self.m("alarms"),
            "gaps" => self.m("gaps"), "untimed" => self.m("untimed"),
            "late_blocks" => self.meta.get("late_blocks").cloned().unwrap_or(Value::List(vec![])),
            "markers" => Value::Dict(dict! { "names" => self.m("marker_names"), "count" => self.arrays.mk_piece.as_slice().len() }),
            "skipped" => self.meta.get("skipped").cloned().unwrap_or(Value::Dict(Dict::new())),
            "notes" => self.meta.get("notes").cloned().unwrap_or(Value::List(vec![])),
            "indexed_s" => self.m("indexed_s"),
        })
    }

    /// The channels' pieces that lie in [t0, t1], in order of id.  With more than `px` of a
    /// channel, only the first (by start, then id) to start in each pixel column.
    fn pieces_in(&self, ch: i64, t0: f64, t1: f64, px: f64) -> Vec<usize> {
        let Some(c) = self.by_channel.get(&ch) else { return vec![] };
        let hi = c.order.partition_point(|&i| self.pieces[i].t0 < t1 + c.margin);
        let lo = c.max_end[..hi].partition_point(|&e| e + c.margin <= t0);
        let mut found: Vec<usize> = c.order[lo..hi]
            .iter()
            .copied()
            .filter(|&i| {
                let (a, b) = self.pieces[i].span(t0, t1);
                b > a
            })
            .collect();
        if found.len() as f64 > px {
            let mut last = None;
            found.retain(|&i| {
                let col = ((self.pieces[i].t0.max(t0) - t0) / (t1 - t0) * px).floor() as i64;
                let keep = last != Some(col);
                last = Some(col);
                keep
            });
        }
        found.sort_unstable();
        found
    }

    /// Per channel, blocks of samples (`raw`) or min/max (`env`) covering [t0, t1] at
    /// about `px` points across (JSON text).  `t0 < t1`, both finite.
    pub fn window_json(&self, channels: &[i64], t0: f64, t1: f64, px: i64) -> signal_core::Result<String> {
        let px = px.clamp(16, 8000) as f64;
        let span = t1 - t0;
        let mut plans: Vec<Vec<Plan>> = vec![];
        let mut raw: Vec<(usize, i64, i64)> = vec![];
        for &ch in channels {
            let mut blocks = vec![];
            for i in self.pieces_in(ch, t0, t1, px) {
                let p = &self.pieces[i];
                let (a, b) = p.span(t0, t1);
                let per_px = p.fs * span / px;
                if per_px < LEVEL0 as f64 && b - a <= MAX_RAW {
                    blocks.push(Plan::Raw(i, a, per_px));
                    raw.push((p.id, a, b));
                } else {
                    blocks.push(Plan::Env(i, a, b, per_px));
                }
            }
            plans.push(blocks);
        }
        // the samples of every raw block, read together
        let mut samples = self.reader.read_many(&raw)?.into_iter();
        let mut parts = Vec::new();
        for (ch, blocks) in channels.iter().zip(plans) {
            let mut out = Vec::with_capacity(blocks.len());
            for plan in blocks {
                out.push(match plan {
                    Plan::Raw(i, a, per_px) => {
                        let y = samples.next().unwrap_or_default();
                        self.raw_block(&self.pieces[i], a, &y, per_px)
                    }
                    Plan::Env(i, a, b, per_px) => self.env_block(&self.pieces[i], a, b, per_px),
                });
            }
            parts.push(format!("\"{ch}\":[{}]", out.join(",")));
        }
        Ok(format!("{{\"t0\":{},\"t1\":{},\"channels\":{{{}}}}}", repr(t0), repr(t1), parts.join(",")))
    }

    /// A block of samples `y` from sample `a`: as they are, or min/max of every `per_px`.
    fn raw_block(&self, p: &Piece, a: i64, y: &[f64], per_px: f64) -> String {
        let fs = p.fs;
        if per_px <= RAW_PER_PX {
            return format!(
                "{{\"piece\":{},\"kind\":\"raw\",\"t0\":{},\"dt\":{},\"y\":{}}}",
                p.id,
                repr(p.t0 + a as f64 / fs),
                repr(1.0 / fs),
                json_array(y, 8)
            );
        }
        let step = (per_px as usize).max(1);
        let (mut lo, mut hi) = (vec![], vec![]);
        for c in y.chunks(step) {
            lo.push(c.iter().copied().fold(f64::NAN, fmin64));
            hi.push(c.iter().copied().fold(f64::NAN, fmax64));
        }
        format!(
            "{{\"piece\":{},\"kind\":\"env\",\"t0\":{},\"dt\":{},\"min\":{},\"max\":{}}}",
            p.id,
            repr(p.t0 + a as f64 / fs),
            repr(step as f64 / fs),
            json_array(&lo, 7),
            json_array(&hi, 7)
        )
    }

    /// A block of the index's envelope over samples [a, b), at the coarsest level whose buckets
    /// hold at most `per_px` samples.
    fn env_block(&self, p: &Piece, a: i64, b: i64, per_px: f64) -> String {
        let fs = p.fs;
        let mut lvl = 0;
        while lvl + 1 < p.env.len() && bucket_size(lvl + 1) as f64 <= per_px {
            lvl += 1;
        }
        let size = bucket_size(lvl) as i64;
        let (off, length) = p.env.get(lvl).copied().unwrap_or((0, 0));
        let i0 = (a / size) as usize;
        let i1 = (length as i64).min((b + size - 1) / size) as usize;
        let (lo, hi) = if i1 > i0 {
            let mn = self.arrays.env_min[lvl].as_slice();
            let mx = self.arrays.env_max[lvl].as_slice();
            (
                mn[off + i0..off + i1].iter().map(|&x| x as f64).collect::<Vec<_>>(),
                mx[off + i0..off + i1].iter().map(|&x| x as f64).collect::<Vec<_>>(),
            )
        } else {
            (vec![], vec![])
        };
        format!(
            "{{\"piece\":{},\"kind\":\"env\",\"t0\":{},\"dt\":{},\"min\":{},\"max\":{}}}",
            p.id,
            repr(p.t0 + (i0 as i64 * size) as f64 / fs),
            repr(size as f64 / fs),
            json_array(&lo, 7),
            json_array(&hi, 7)
        )
    }

    /// Physical samples [a, b) of a piece.
    pub fn samples(&self, piece: usize, a: i64, b: i64) -> signal_core::Result<Vec<f64>> {
        self.reader.read(piece, a, b)
    }

    fn series(&self, name: &str) -> Option<(&[f64], &[f64], &str)> {
        let (unit, off, n) = self.vitals.get(name)?;
        let t = &self.arrays.vit_t.as_slice()[*off..off + n];
        let v = &self.arrays.vit_v.as_slice()[*off..off + n];
        Some((t, v, unit))
    }

    /// Per series, its points in [t0, t1]; with more than 3 per pixel, the min and max of
    /// each pixel column (at the column's first time).
    pub fn vitals_window(&self, names: &[String], t0: f64, t1: f64, px: i64) -> Value {
        let mut out = Dict::new();
        for name in names {
            let Some((t, v, unit)) = self.series(name) else { continue };
            let i0 = search(t, t0, false).saturating_sub(1);
            let i1 = (search(t, t1, true) + 1).min(t.len());
            let (t, v) = (&t[i0..i1.max(i0)], &v[i0..i1.max(i0)]);
            let pxf = px as f64;
            if t.len() as f64 > 3.0 * pxf && t1 > t0 {
                let (mut ts, mut lo, mut hi) = (vec![], vec![], vec![]);
                let mut last_col: Option<i64> = None;
                for (k, (&tk, &vk)) in t.iter().zip(v).enumerate() {
                    let col = ((tk - t0) / (t1 - t0) * pxf).floor() as i64;
                    if last_col != Some(col) || k == 0 {
                        ts.push(tk);
                        lo.push(vk);
                        hi.push(vk);
                        last_col = Some(col);
                    } else {
                        let j = lo.len() - 1;
                        lo[j] = if vk < lo[j] || vk.is_nan() { vk } else { lo[j] };
                        hi[j] = if vk > hi[j] || vk.is_nan() { vk } else { hi[j] };
                    }
                }
                out.insert(
                    name.clone(),
                    Value::Dict(dict! { "unit" => unit, "kind" => "env", "t" => ts, "min" => lo, "max" => hi }),
                );
            } else {
                out.insert(
                    name.clone(),
                    Value::Dict(dict! { "unit" => unit, "kind" => "points", "t" => t.to_vec(), "v" => v.to_vec() }),
                );
            }
        }
        Value::Dict(out)
    }

    /// Latest value of every series at time t (not older than 10 minutes).
    pub fn vitals_at(&self, t: f64) -> Value {
        let mut out = vec![];
        for (name, (unit, _, _)) in &self.vitals {
            let Some((ts, vs, _)) = self.series(name) else { continue };
            let i = search(ts, t, true);
            if i > 0 && t - ts[i - 1] <= VITAL_MAX_AGE_S {
                out.push(Value::Dict(
                    dict! { "name" => name.clone(), "unit" => unit.clone(), "value" => vs[i - 1], "t" => ts[i - 1] },
                ));
            }
        }
        Value::List(out)
    }

    /// Markers in [t0, t1] on the given channels: [channel, time, name index].
    pub fn markers(&self, channels: &[i64], t0: f64, t1: f64) -> Value {
        const LIMIT: usize = 20000;
        let mk_p = self.arrays.mk_piece.as_slice();
        let mk_s = self.arrays.mk_sample.as_slice();
        let mk_c = self.arrays.mk_code.as_slice();
        let mut items = vec![];
        let names = self.m("marker_names");
        for p in &self.pieces {
            if !channels.contains(&p.ch) {
                continue;
            }
            // saturating: any finite times
            let a = ((t0 - p.t0) * p.fs).floor() as i64;
            let b = ((t1 - p.t0) * p.fs).ceil() as i64;
            if b < 0 || a > p.n {
                continue;
            }
            let lo = mk_p.partition_point(|&x| (x as i64) < p.id as i64);
            let hi = mk_p.partition_point(|&x| (x as i64) <= p.id as i64);
            let s = &mk_s[lo..hi];
            let j0 = lo + s.partition_point(|&x| x < a);
            let j1 = lo + s.partition_point(|&x| x <= b);
            for k in j0..j1 {
                items.push(Value::List(vec![p.ch.into(), (p.t0 + mk_s[k] as f64 / p.fs).into(), mk_c[k].into()]));
                if items.len() >= LIMIT {
                    return Value::Dict(dict! { "names" => names, "items" => Value::List(items), "truncated" => true });
                }
            }
        }
        Value::Dict(dict! { "names" => names, "items" => Value::List(items), "truncated" => false })
    }
}
