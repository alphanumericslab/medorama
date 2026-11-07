//! WFDB records (docs/wfdb-collection.md): one record (its `.hea` file, single- or
//! multi-segment), a WFDB collection (records described by `index.csv`), or a folder of plain
//! WFDB records, subfolders included, read as one session.
//!
//! Each signal of each record (each segment of a multi-segment record) is a piece.  Start
//! times come from index.csv (UTC) or from the headers (base date and time, taken as UTC);
//! records without one are placed after the data before them and marked "time not stated"
//! (records of the same stretch share that start).  The segments of a multi-segment record
//! follow each other from the record's start; its null segments are breaks of known length.
//! A piece that would overlap another piece of its channel goes into a second lane of that
//! channel (`II (2)`), so overlapping records are never drawn on top of each other.  Vital
//! signs, alarms, breaks and markers come from vitals.csv, events.csv, gaps.csv and the `.mrk`
//! annotation files.
//!
//! Records are read with wfdb-x (`wfdbx`), one library instance per call, so that threads do
//! not share its state. Nothing is assumed that a header does not state: a record without a
//! sampling frequency is left out, an uncalibrated signal (gain 0) is shown in ADC units
//! (`adu`), and signals without units have none.
//!
//! This module is built with the `wfdb` feature (the default); without it, `wfdb_none.rs` takes
//! its place.  What needs no wfdb-x (which paths are records, segment tables, limits) is in
//! `wfdb_files.rs`, shared by both.

use super::wfdb_files::with_ext;
pub use super::wfdb_files::{
    format_label, is_record, is_wfdb_folder, limits, read_csv, segment_table, set_limits, Limits, MAX_FINGERPRINT_FILES,
};
use super::Built;
use crate::common::*;
use indexmap::{IndexMap, IndexSet};
use signal_core::dict;
use signal_core::value::{Dict, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Whether this build reads WFDB records (the `wfdb` feature).
pub const SUPPORTED: bool = true;

/// Samples (frames × signals) decoded at once; at least [`MIN_CHUNK_FRAMES`] frames.
const CHUNK_SAMPLES: u64 = 1 << 20;
const MIN_CHUNK_FRAMES: u64 = 4096;
const SPLIT_REASONS: [&str; 4] = ["channel_absent", "channel_set_change", "label_change", "packet_count_change"];

/// A path as wfdb-x takes it (bytes).
fn wpath(p: &Path) -> Vec<u8> {
    p.to_string_lossy().into_owned().into_bytes()
}

/// A wfdb-x instance that prints nothing (problems come back as errors).
fn quiet() -> wfdbx::Wfdb {
    let mut w = wfdbx::Wfdb::new();
    w.wfdbquiet();
    w
}

fn wfdb_err(record: &Path) -> impl Fn(wfdbx::record::Error) -> signal_core::Error + '_ {
    move |e| signal_core::Error::Wfdb(format!("{}: {e}", record.display()))
}

/// The header of a record (no signal file is opened); an error for a record that states no
/// sampling frequency (or one that is not a finite positive number).  The number of samples is
/// at most what the signal files can hold ([`frames_held`]).
fn header(record: &Path) -> signal_core::Result<wfdbx::Header> {
    let mut h = quiet().read_header(wpath(record), false).map_err(wfdb_err(record))?;
    if !(h.fs > 0.0 && h.fs.is_finite()) {
        return Err(signal_core::Error::Format(format!(
            "{}: the header states no sampling frequency",
            record.display()
        )));
    }
    h.nsamples = h.nsamples.max(0).min(frames_held(record, &h));
    Ok(h)
}

/// Three times the bits a sample of WFDB storage format `fmt` takes, for the formats of fixed
/// size (None for the others: compressed, or unknown).
fn bits3(fmt: i32) -> Option<u128> {
    Some(match fmt {
        8 | 80 => 24,
        16 | 61 | 160 => 48,
        212 => 36,
        310 | 311 => 32,
        24 => 72,
        32 => 96,
        _ => return None,
    })
}

/// The file a signal file name of a header designates, as wfdb-x opens it: `~` expanded, an
/// absolute name as given, any other next to the header.  None for a signal without a file.
fn signal_file(record: &Path, fname: &[u8]) -> Option<PathBuf> {
    let f = String::from_utf8_lossy(fname).to_string();
    if f.is_empty() || f == "-" || f == "~" {
        return None;
    }
    let f = crate::paths::expand(&f);
    if Path::new(&f).is_absolute() {
        return Some(PathBuf::from(f));
    }
    Some(record.parent().map_or_else(|| PathBuf::from(&f), |d| d.join(&f)))
}

/// The most frames the signal files of a record can hold: for each file of signals of a fixed
/// size, `ceil(8 × bytes / bits per frame)`, the least of them (every frame stated when no file
/// can be measured).  A header that states more samples is read up to this many.
fn frames_held(record: &Path, h: &wfdbx::Header) -> i64 {
    let mut per_file: IndexMap<PathBuf, Option<u128>> = IndexMap::new();
    for s in &h.siginfo {
        let Some(f) = signal_file(record, s.fname.as_deref().unwrap_or_default()) else { continue };
        let bits = bits3(s.fmt).map(|b| b * s.spf.max(1) as u128);
        let e = per_file.entry(f).or_insert(Some(0));
        *e = match (*e, bits) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
    }
    let mut held = i64::MAX;
    for (f, bits) in per_file {
        let (Some(bits), Ok(m)) = (bits, f.metadata()) else { continue };
        if bits > 0 {
            let frames = (m.len() as u128 * 24).div_ceil(bits);
            held = held.min(i64::try_from(frames).unwrap_or(i64::MAX));
        }
    }
    held
}

/// Every file reading record `name` (relative to `base`) opens: its header, the signal files
/// its header names, its `.mrk` file, and those of each segment of a multi-segment record.
pub fn files_read(base: &Path, name: &str) -> Vec<PathBuf> {
    let mut out: IndexSet<PathBuf> = IndexSet::new();
    let add = |rec: &Path, out: &mut IndexSet<PathBuf>| {
        out.insert(with_ext(rec, "hea"));
        if let Ok(h) = quiet().read_header(wpath(rec), false) {
            for s in &h.siginfo {
                out.extend(signal_file(rec, s.fname.as_deref().unwrap_or_default()));
            }
        }
        let mrk = with_ext(rec, "mrk");
        if mrk.symlink_metadata().is_ok() {
            out.insert(mrk);
        }
    };
    add(&base.join(name), &mut out);
    for (seg, _) in segment_table(&base.join(name)).unwrap_or_default() {
        if seg != "~" {
            add(&base.join(beside(name, &seg)), &mut out);
        }
    }
    out.into_iter().collect()
}

/// An error naming the first file of record `name` that lies outside the folders the readers
/// may open ([`crate::paths::set_allowed_roots`]).
pub fn check_files(base: &Path, name: &str) -> signal_core::Result<()> {
    match files_read(base, name).into_iter().find(|f| !crate::paths::allowed_file(f)) {
        Some(f) => Err(signal_core::Error::Value(format!(
            "{} lies outside the data roots: the record {name} is not read",
            f.display()
        ))),
        None => Ok(()),
    }
}

/// [`check_files`] for a record's header file (`dir/name.hea`).
pub fn check_record(hea: &Path) -> signal_core::Result<()> {
    let base = hea.parent().map(Path::to_path_buf).unwrap_or_default();
    check_files(&base, &hea.file_stem().unwrap_or_default().to_string_lossy())
}

/// A record name from index.csv that stays inside its folder: relative, without `..`.
fn safe_name(name: &str) -> bool {
    let p = Path::new(name);
    !name.is_empty()
        && !name.starts_with(['/', '\\', '~'])
        && p.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir))
}

/// `name` of a segment, next to its record (`dir/rec` -> `dir/seg`), with `/` between folders.
fn beside(record: &str, name: &str) -> String {
    match record.rfind('/') {
        Some(i) => format!("{}/{name}", &record[..i]),
        None => name.to_string(),
    }
}

/// Start (Unix time, UTC) from a header's base date and time, when it states both.
fn start_unix(h: &wfdbx::Header) -> Option<f64> {
    let ([y, m, d], ms) = (h.base_date?, h.base_time_ms?);
    Some(signal_core::pyfmt::days_from_date(y, m as u32, d as u32) as f64 * 86400.0 + ms as f64 / 1000.0)
}

/// Name and unit of each signal: the description, and the units (`adu` for an uncalibrated
/// signal; empty when none are stated).
fn names_units(h: &wfdbx::Header) -> Vec<(String, String)> {
    h.siginfo
        .iter()
        .map(|s| {
            let name = String::from_utf8_lossy(s.desc.as_deref().unwrap_or_default()).into_owned();
            let unit = if s.gain == 0.0 {
                "adu".to_string()
            } else {
                String::from_utf8_lossy(s.units.as_deref().unwrap_or_default()).into_owned()
            };
            (name, unit)
        })
        .collect()
}

/// A stored sample in physical units: (value - baseline) / gain; ADC units when the signal is
/// uncalibrated; NaN for WFDB's invalid sample.
fn physical(v: i32, s: &wfdbx::Siginfo) -> f64 {
    if v == wfdbx::WFDB_INVALID_SAMPLE {
        f64::NAN
    } else if s.gain == 0.0 {
        v as f64
    } else {
        (v as f64 - s.baseline as f64) / s.gain
    }
}

/// Frames decoded at once for a record of `nsig` signals.
fn chunk_frames(nsig: usize) -> u64 {
    (CHUNK_SAMPLES / nsig.max(1) as u64).max(MIN_CHUNK_FRAMES)
}

/// Frames [from, to) of a record, decoded a chunk at a time: `sink(signals, frames, count)`
/// gets the stored samples of `count` frames (`frames[i * nsig + k]`).  Stops at the end of
/// the data (a chunk read short), whatever the header states.
fn decode(
    record: &Path,
    h: &wfdbx::Header,
    from: u64,
    to: u64,
    sink: &mut dyn FnMut(&[wfdbx::Siginfo], &[i32], usize) -> signal_core::Result<()>,
) -> signal_core::Result<()> {
    let nsig = h.nsig;
    let mut w = quiet();
    let mut si = vec![wfdbx::Siginfo::default(); nsig];
    let fail = || signal_core::Error::Wfdb(format!("{}: cannot read the signals", record.display()));
    if w.isigopen(wpath(record).as_slice(), &mut si, nsig as i32) != nsig as i32 {
        return Err(fail());
    }
    if w.isigsettime(i64::try_from(from).unwrap_or(i64::MAX)) < 0 {
        w.wfdbquit();
        return Err(fail());
    }
    let chunk = chunk_frames(nsig);
    let mut buf = vec![0i32; chunk.min(to.saturating_sub(from)) as usize * nsig];
    let mut a = from;
    let result = loop {
        if a >= to {
            break Ok(());
        }
        let n = chunk.min(to - a) as usize;
        let got = w.getvecs(&mut buf[..n * nsig], n as i64).clamp(0, n as i64) as usize;
        if let Err(e) = sink(&si, &buf[..got * nsig], got) {
            break Err(e);
        }
        if got < n {
            break Ok(());
        }
        a += n as u64;
    };
    w.wfdbquit();
    result
}

/// Frames [from, to) of a record, the signals `want` in physical units (one vector each).
fn read_frames(
    record: &Path,
    h: &wfdbx::Header,
    from: u64,
    to: u64,
    want: &[usize],
) -> signal_core::Result<Vec<Vec<f64>>> {
    let nsig = h.nsig;
    let mut out: Vec<Vec<f64>> = want.iter().map(|_| Vec::new()).collect();
    decode(record, h, from, to, &mut |si, buf, got| {
        for (o, &k) in out.iter_mut().zip(want) {
            if k < nsig {
                o.extend((0..got).map(|i| physical(buf[i * nsig + k], &si[k])));
            }
        }
        Ok(())
    })?;
    Ok(out)
}

fn fl(v: Option<&String>) -> Option<f64> {
    v.and_then(|s| signal_core::text::parse_float(s)).filter(|f| f.is_finite())
}

fn sv(v: Option<&String>) -> Value {
    v.map_or(Value::None, |s| Value::Str(s.clone()))
}

fn int_of(v: Option<&String>) -> Value {
    fl(v).map_or(Value::None, |f| Value::Int(f.trunc() as i128))
}

/// The refusal for a folder over a limit, with what to do instead.
fn over_limit(what: String) -> signal_core::Error {
    signal_core::Error::Value(format!(
        "{what}: open a subfolder, or describe the records in an index.csv (a WFDB collection, docs/wfdb-collection.md)"
    ))
}

/// Header files below `folder`, as record names (relative, without `.hea`, `/` between
/// folders), sorted.  Hidden entries (`.` names) and symbolic links are skipped.  Over a limit
/// (records, depth, or `deadline` for the scan) it is an error.
fn walk(folder: &Path, limits: &Limits, deadline: Option<Instant>) -> signal_core::Result<Vec<String>> {
    fn go(
        dir: &Path,
        rel: &str,
        depth: usize,
        limits: &Limits,
        deadline: Option<Instant>,
        out: &mut Vec<String>,
    ) -> signal_core::Result<()> {
        // reached, not passed: a clock that ticks every 15 ms (Windows) still ends a 0 s limit
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(over_limit(format!("counting the records took more than {} s", limits.max_scan_s)));
        }
        let Ok(it) = std::fs::read_dir(dir) else { return Ok(()) };
        let mut entries: Vec<(String, std::fs::FileType)> = it
            .filter_map(Result::ok)
            .filter_map(|e| Some((e.file_name().to_string_lossy().to_string(), e.file_type().ok()?)))
            .filter(|(n, t)| !n.starts_with('.') && !t.is_symlink())
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, t) in entries {
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if t.is_dir() {
                if depth + 1 > limits.max_depth {
                    return Err(over_limit(format!(
                        "the folder has records nested more than {} levels deep",
                        limits.max_depth
                    )));
                }
                go(&dir.join(&name), &r, depth + 1, limits, deadline, out)?;
            } else if let Some(stem) = r.strip_suffix(".hea") {
                out.push(stem.to_string());
                if out.len() > limits.max_records {
                    return Err(over_limit(format!("the folder holds more than {} WFDB records", limits.max_records)));
                }
            }
        }
        Ok(())
    }
    let mut out = vec![];
    go(folder, "", 0, limits, deadline, &mut out)?;
    out.sort();
    Ok(out)
}

/// The records of a folder that are not segments (or layouts) of a multi-segment record in it.
fn plain_records(folder: &Path, names: Vec<String>) -> Vec<String> {
    let mut claimed: IndexSet<String> = IndexSet::new();
    for n in &names {
        for (seg, _) in segment_table(&folder.join(n)).unwrap_or_default() {
            if seg != "~" {
                claimed.insert(beside(n, &seg));
            }
        }
    }
    names.into_iter().filter(|n| !claimed.contains(n)).collect()
}

/// The single-segment record holding samples of a record: itself, or one segment of a
/// multi-segment record, `offset` samples after the record's start.
struct Part {
    rec: String,
    offset: i64,
    h: wfdbx::Header,
}

/// A record to read: index.csv's row (or one made from the header), and its parts.
struct Unit {
    /// the record's name, relative to the folder opened
    name: String,
    row: IndexMap<String, String>,
    fs: f64,
    n: i64,
    parts: Vec<Part>,
    /// null segments: (offset, samples)
    nulls: Vec<(i64, i64)>,
    /// the master header of a multi-segment record
    master: Option<wfdbx::Header>,
}

/// The record `name` (relative to `base`), with `row` from index.csv or None.
fn unit(base: &Path, name: &str, row: Option<IndexMap<String, String>>) -> signal_core::Result<Unit> {
    check_files(base, name)?;
    let path = base.join(name);
    let h = header(&path)?;
    let row = row.unwrap_or_else(|| {
        let start = start_unix(&h).map(signal_core::pyfmt::repr).unwrap_or_default();
        IndexMap::from([
            ("record".to_string(), name.to_string()),
            ("modality".to_string(), "wfdb".to_string()),
            ("start_unix".to_string(), start),
        ])
    });
    let Some(segs) = segment_table(&path) else {
        return Ok(Unit {
            name: name.to_string(),
            row,
            fs: h.fs,
            n: h.nsamples.max(0),
            parts: vec![Part { rec: name.to_string(), offset: 0, h: h.clone() }],
            nulls: vec![],
            master: None,
        });
    };
    let (mut parts, mut nulls, mut offset) = (vec![], vec![], 0i64);
    for (seg, n) in segs {
        if seg == "~" {
            if n > 0 {
                nulls.push((offset, n));
            }
        } else if n > 0 {
            let rec = beside(name, &seg);
            let sh = header(&base.join(&rec))?;
            parts.push(Part { rec, offset, h: sh });
        }
        offset = offset.saturating_add(n.max(0)); // a layout segment has no samples
    }
    Ok(Unit { name: name.to_string(), row, fs: h.fs, n: offset, parts, nulls, master: Some(h) })
}

/// What is opened: the records and how they were found.
struct Opened {
    /// the folder record names are relative to
    base: PathBuf,
    units: Vec<Unit>,
    /// index.csv describes the records
    collection: bool,
    /// one record (a `.hea` file)
    single: bool,
    /// records left out, with the reason
    skipped: Vec<(String, String)>,
}

fn open(path: &Path, limits: &Limits, deadline: Option<Instant>) -> signal_core::Result<Opened> {
    if is_record(path) {
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let u = unit(&base, &name, None)?;
        return Ok(Opened { base, units: vec![u], collection: false, single: true, skipped: vec![] });
    }
    let base = path.to_path_buf();
    let rows = read_csv_within(&base.join("index.csv"));
    let (mut units, mut skipped) = (vec![], vec![]);
    let collection = !rows.is_empty();
    if collection {
        // a record that cannot be read is left out, as in a folder of plain records
        for (i, r) in rows.into_iter().enumerate() {
            let Some(name) = r.get("record").cloned() else {
                return Err(signal_core::Error::Format(format!("{}: index.csv has no record column", path.display())));
            };
            if !safe_name(&name) {
                let why = "index.csv names a record outside its folder (an absolute name, or one with ..)";
                skipped.push((if name.is_empty() { format!("row {}", i + 1) } else { name }, why.to_string()));
                continue;
            }
            match unit(&base, &name, Some(r)) {
                Ok(u) => units.push(u),
                Err(e) => skipped.push((name, e.to_string())),
            }
        }
    } else {
        for name in plain_records(&base, walk(&base, limits, deadline)?) {
            match unit(&base, &name, None) {
                Ok(u) => units.push(u),
                Err(e) => skipped.push((name, e.to_string())),
            }
        }
    }
    if units.is_empty() {
        return Err(signal_core::Error::Value(format!("no WFDB records in or below {}", path.display())));
    }
    Ok(Opened { base, units, collection, single: false, skipped })
}

/// [`read_csv`] of a file the readers may open (none otherwise: [`crate::paths::allowed_file`]).
fn read_csv_within(path: &Path) -> Vec<IndexMap<String, String>> {
    if crate::paths::allowed_file(path) {
        read_csv(path)
    } else {
        vec![]
    }
}

fn rec_path(units: &[Unit], name: &str) -> String {
    for u in units {
        let rec = &u.name;
        if rec == name || rec.ends_with(&format!("/{name}")) {
            return rec.clone();
        }
    }
    name.to_string()
}

/// Start of each record on the time axis, and whether it is stated: records in order; those
/// without a start follow the data before them, after the break gaps.csv states before them
/// (gap_s), if any; records of the same stretch start together.
fn placement(units: &[Unit], gap_rows: &[IndexMap<String, String>]) -> Vec<(f64, bool)> {
    let mut counted: IndexMap<String, f64> = IndexMap::new();
    for g in gap_rows {
        if let Some(gs) = fl(g.get("gap_s")).filter(|_| g.get("scope").map(String::as_str) == Some("all")) {
            for name in g.get("records_after").map(String::as_str).unwrap_or("").split(';') {
                counted.insert(rec_path(units, name), gs);
            }
        }
    }
    let mut out = Vec::with_capacity(units.len());
    let mut last_end: Option<f64> = None;
    let mut untimed_group: IndexMap<String, f64> = IndexMap::new();
    for (i, u) in units.iter().enumerate() {
        let r = &u.row;
        let dur = u.n as f64 / u.fs;
        let s = match fl(r.get("start_unix")) {
            Some(t) => (t, true),
            None => {
                let g = r.get("stretch").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| format!("record {i}"));
                let t = *untimed_group
                    .entry(g)
                    .or_insert_with(|| last_end.unwrap_or(0.0) + counted.get(&u.name).copied().unwrap_or(0.0));
                (t, false)
            }
        };
        out.push(s);
        let end = s.0 + dur;
        last_end = Some(last_end.map_or(end, |l| l.max(end)));
    }
    out
}

/// Lanes of each channel: the time spans taken in each, to put a piece in the first lane it
/// does not overlap.
#[derive(Default)]
struct Lanes {
    taken: IndexMap<String, Vec<Vec<(f64, f64)>>>,
}

impl Lanes {
    /// The lane (1, 2, ...) of a piece of channel `key` over [t0, t1).
    fn place(&mut self, key: &str, t0: f64, t1: f64) -> usize {
        let lanes = self.taken.entry(key.to_string()).or_default();
        for (k, spans) in lanes.iter_mut().enumerate() {
            if spans.iter().all(|&(a, b)| t1 <= a + 1e-9 || t0 >= b - 1e-9) {
                spans.push((t0, t1));
                return k + 1;
            }
        }
        lanes.push(vec![(t0, t1)]);
        lanes.len()
    }
}

pub fn build(path: &Path, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> signal_core::Result<Built> {
    let lim = limits();
    let opened = open(path, &lim, None)?;
    let (base, units) = (&opened.base, &opened.units);
    let mut channels = Channels::default();
    let mut pieces = Pieces::default();
    let mut vitals = Vitals::default();
    let total = units
        .iter()
        .flat_map(|u| &u.parts)
        .map(|p| (p.h.nsamples.max(0) as u64).saturating_mul(p.h.nsig as u64))
        .fold(0u64, u64::saturating_add)
        .max(1) as f64;
    // the CSV files beside the records (a collection's; a folder of plain records may have them too)
    let csv = |name: &str| if opened.single { vec![] } else { read_csv_within(&base.join(name)) };
    let gap_rows = csv("gaps.csv");
    let starts = placement(units, &gap_rows);

    let mut done = 0u64;
    let mut lanes = Lanes::default();
    let mut rec_pieces: IndexMap<String, Vec<usize>> = IndexMap::new();
    let mut piece_info: Vec<(String, usize)> = vec![];
    let mut null_gaps: Vec<Value> = vec![];
    for (u, &(t0, timed)) in units.iter().zip(&starts) {
        let modality = u.row.get("modality").filter(|m| !m.is_empty()).cloned().unwrap_or_else(|| "wfdb".into());
        let mut all = vec![];
        // per part: its pieces, and the index of each of its signal names
        let mut part_pieces: Vec<(Vec<usize>, IndexMap<String, usize>)> = vec![];
        for part in &u.parts {
            let tp = t0 + part.offset as f64 / u.fs;
            let tend = tp + part.h.nsamples.max(0) as f64 / part.h.fs;
            let mut ps = vec![];
            let mut by_name = IndexMap::new();
            pieces.room(part.h.nsig)?;
            for (j, (name, unit)) in names_units(&part.h).into_iter().enumerate() {
                let key = format!("wfdb\u{1}{modality}\u{1}{name}\u{1}{unit}");
                let lane = lanes.place(&key, tp, tend);
                let (key, label) = if lane == 1 {
                    (key, name.clone())
                } else {
                    (format!("{key}\u{1}{lane}"), format!("{name} ({lane})"))
                };
                let cid = channels.get(key, &label, &unit, &modality, None);
                ps.push(pieces.start(cid, part.h.fs, dict! { "t0" => tp, "timed" => timed }));
                piece_info.push((part.rec.clone(), j));
                by_name.entry(name).or_insert(j);
            }
            all.extend(ps.iter().copied());
            let path = base.join(&part.rec);
            let n = part.h.nsamples.max(0) as u64;
            let nsig = part.h.nsig;
            let mut xf: Vec<f32> = vec![];
            decode(&path, &part.h, 0, n, &mut |si, buf, got| {
                if cancelled() {
                    return Err(signal_core::Error::Value("indexing cancelled".into()));
                }
                for (j, &p) in ps.iter().enumerate() {
                    xf.clear();
                    xf.extend((0..got).map(|i| physical(buf[i * nsig + j], &si[j]) as f32));
                    pieces.add(p, &xf);
                }
                done = done.saturating_add(got as u64 * nsig as u64);
                progress(done as f64 / total);
                Ok(())
            })?;
            part_pieces.push((ps, by_name));
        }
        rec_pieces.insert(u.name.clone(), all);
        // markers: the record's `.mrk` annotations (of a multi-segment record: the record's,
        // with sample numbers from its start); a marker's name is its text, else the
        // annotation code's mnemonic
        let rec = base.join(&u.name);
        if with_ext(&rec, "mrk").is_file() {
            let mut w = quiet();
            if let Ok(ann) = w.read_annotations(wpath(&rec), "mrk", None, None, None, false) {
                let master_names = u.master.as_ref().map(names_units);
                for i in 0..ann.len() {
                    let c = ann.chans[i] as usize;
                    let text = ann.aux[i].as_deref().map(|t| String::from_utf8_lossy(t).trim().to_string());
                    let name = text
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| String::from_utf8_lossy(&w.ecgstr(ann.codes[i])).trim().to_string());
                    let s = ann.samples[i];
                    match &master_names {
                        None => {
                            let ps = &part_pieces[0].0;
                            if c < ps.len() {
                                pieces.marker(ps[c], s, &name);
                            }
                        }
                        Some(mn) => {
                            let Some(sig) = mn.get(c).map(|x| &x.0) else { continue };
                            let at = u.parts.iter().position(|p| p.offset <= s && s < p.offset + p.h.nsamples.max(0));
                            let Some(k) = at else { continue };
                            let (ps, by_name) = &part_pieces[k];
                            if let Some(&j) = by_name.get(sig) {
                                pieces.marker(ps[j], s - u.parts[k].offset, &name);
                            }
                        }
                    }
                }
            }
        }
        for &(off, n) in &u.nulls {
            let (a, b) = (t0 + off as f64 / u.fs, t0 + (off + n) as f64 / u.fs);
            null_gaps.push(Value::Dict(dict! {
                "t0" => a, "t1" => b, "reason" => "null segment", "gap_s" => b - a, "exact" => true,
                "measured_by" => Value::None, "notes" => "",
            }));
        }
    }
    let pieces_markers_capped = pieces.markers_capped();
    let (mut items, mut arrays, marker_names) = pieces.finish();
    for (p, (record, sig)) in items.iter_mut().zip(piece_info) {
        p.insert("record".into(), record.into());
        p.insert("sig".into(), sig.into());
    }
    let end_of = |p: &Dict| {
        p.get("t0").and_then(Value::as_f64).unwrap_or(0.0)
            + p.get("n").and_then(Value::as_f64).unwrap_or(0.0) / p.get("fs").and_then(Value::as_f64).unwrap_or(1.0)
    };
    let t_lo = items.iter().filter_map(|p| p.get("t0").and_then(Value::as_f64)).reduce(f64::min);
    let t_hi = items.iter().map(end_of).reduce(f64::max);

    // vital signs (vitals.csv: time, name or label, value, unit; rows with a non-zero
    // validity are left out). A label measured by several sources keeps its name for the first
    // source in the file; the others are named with their source ("HR [import]").
    let vital_rows: Vec<_> =
        csv("vitals.csv").into_iter().filter(|v| v.get("validity").is_none_or(|x| x.is_empty() || x == "0")).collect();
    let label_of = |v: &IndexMap<String, String>| {
        [v.get("label"), v.get("name")].into_iter().flatten().find(|s| !s.is_empty()).cloned().unwrap_or_default()
    };
    let mut sources_of: IndexMap<String, Vec<String>> = IndexMap::new();
    for v in &vital_rows {
        let src = v.get("source").cloned().unwrap_or_default();
        let e = sources_of.entry(label_of(v)).or_default();
        if !e.contains(&src) {
            e.push(src);
        }
    }
    for v in &vital_rows {
        let label = label_of(v);
        let src = v.get("source").map(String::as_str).unwrap_or("");
        let name = if !src.is_empty() && sources_of[&label][0] != src { format!("{label} [{src}]") } else { label };
        let value = v.get("value").or(v.get("value1"));
        vitals.add(&name, &sv(v.get("unit")), &sv(v.get("time")), &sv(value));
    }
    // alarms and other events with a duration (events.csv: start_unix, end_unix, level, text, kind)
    let mut alarms: Vec<Dict> = csv("events.csv")
        .iter()
        .filter_map(|e| {
            let t0 = plausible_time(&sv(e.get("start_unix")), t_lo, t_hi)?;
            let t1 = plausible_time(&sv(e.get("end_unix")), t_lo, t_hi).filter(|t| *t >= t0).unwrap_or(t0);
            let text = e.get("text").map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
            let kind = e.get("kind").filter(|k| !k.is_empty()).cloned().unwrap_or_else(|| "event".into());
            Some(dict! { "t0" => t0, "t1" => t1, "text" => text, "level" => int_of(e.get("level")), "kind" => kind })
        })
        .collect();
    alarms.sort_by(|a, b| t0_of(a).partial_cmp(&t0_of(b)).unwrap_or(std::cmp::Ordering::Equal));

    let mut gaps = vec![];
    let piece_of = |p: usize| &items[p];
    for g in &gap_rows {
        let reason = g.get("reason").cloned().unwrap_or_default();
        if SPLIT_REASONS.contains(&reason.as_str()) {
            continue; // a new record, not a break in the data
        }
        let collect = |k: &str| -> Vec<usize> {
            g.get(k)
                .map(String::as_str)
                .unwrap_or("")
                .split(';')
                .filter(|r| !r.is_empty())
                .flat_map(|r| rec_pieces.get(&rec_path(units, r)).cloned().unwrap_or_default())
                .collect()
        };
        let (before, after) = (collect("records_before"), collect("records_after"));
        let mut t0 = fl(g.get("end_before_unix"));
        let mut t1 = fl(g.get("start_after_unix"));
        if t0.is_none() && !before.is_empty() {
            t0 = before.iter().map(|&p| end_of(piece_of(p))).reduce(f64::max);
        }
        if t1.is_none() && !after.is_empty() {
            t1 = after.iter().filter_map(|&p| piece_of(p).get("t0").and_then(Value::as_f64)).reduce(f64::min);
        }
        if t0.is_none() && t1.is_none() {
            continue;
        }
        let scope = g.get("scope").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "all".into());
        let mut row = dict! {
            "t0" => t0.or(t1), "t1" => t1.or(t0),
            "reason" => if scope == "all" { reason.clone() } else { format!("{scope}: {reason}") },
            "gap_s" => fl(g.get("gap_s")), "exact" => g.get("exact").map(String::as_str) == Some("True"),
            "measured_by" => sv(g.get("measured_by")), "notes" => g.get("notes").cloned().unwrap_or_default(),
        };
        if scope != "all" {
            let mut chs: Vec<i64> = before
                .iter()
                .chain(&after)
                .map(|&p| piece_of(p).get("ch").and_then(Value::as_int).unwrap_or(0) as i64)
                .collect();
            chs.sort();
            chs.dedup();
            row.insert("channels".into(), chs.into());
        }
        gaps.push(Value::Dict(row));
    }
    gaps.extend(null_gaps);

    let (vit_meta, vit_t, vit_v) = vitals.finish();
    arrays.vit_t = vit_t;
    arrays.vit_v = vit_v;
    let mut notes: Vec<Value> = vec![];
    if !opened.collection {
        notes.push("The headers' start times are taken as UTC.".into());
    }
    if items.iter().any(|p| p.get("timed") == Some(&Value::Bool(false))) {
        notes.push("Some records have no start time; they are placed after the data before them and marked \"time not stated\".".into());
    }
    if lanes.taken.values().any(|l| l.len() > 1) {
        notes.push(
            "Some records overlap in time: a signal recorded twice at once is shown in a second lane (\"II (2)\")."
                .into(),
        );
    }
    if pieces_markers_capped {
        notes.push(
            format!(
                "More than {} distinct marker names: the others are shown as \"{OTHER_MARKERS}\".",
                MAX_MARKER_NAMES - 1
            )
            .into(),
        );
    }
    if !opened.skipped.is_empty() {
        notes.push(
            match opened.skipped.len() {
                1 => "1 record could not be read and was left out.".to_string(),
                k => format!("{k} records could not be read and were left out."),
            }
            .into(),
        );
    }
    let skipped: Dict = opened.skipped.iter().map(|(n, why)| (n.clone(), Value::Str(why.clone()))).collect();
    let untimed = untimed_spans(&items);
    let meta = dict! {
        "channels" => Value::List(channels.items.into_iter().map(Value::Dict).collect()),
        "pieces" => Value::List(items.into_iter().map(Value::Dict).collect()),
        "vitals" => Value::List(vit_meta.into_iter().map(Value::Dict).collect()),
        "alarms" => Value::List(alarms.into_iter().map(Value::Dict).collect()),
        "gaps" => Value::List(gaps), "untimed" => Value::List(untimed), "marker_names" => marker_names,
        "skipped" => Value::Dict(skipped), "notes" => Value::List(notes), "time_resolution_s" => Value::None,
        "reader" => Value::Dict(Dict::new()),
    };
    Ok((meta, arrays))
}

/// What opening `path` would read, from the headers alone (no signal file is opened), within
/// `limits` (the scan's time limit too): an error over a limit (docs/wfdb-collection.md, "Limits").
pub fn scan(path: &Path, limits: &Limits) -> signal_core::Result<Dict> {
    let t_start = Instant::now();
    // no deadline for a limit too large to state (an infinite one: no time limit)
    let deadline = Duration::try_from_secs_f64(limits.max_scan_s.max(0.0)).ok().and_then(|d| t_start.checked_add(d));
    let opened = open(path, limits, deadline)?;
    let starts = placement(&opened.units, &[]);
    let (mut signals, mut segments, mut samples_s) = (IndexSet::new(), 0usize, 0.0f64);
    let (mut lo, mut hi): (Option<f64>, Option<f64>) = (None, None);
    let mut untimed = 0usize;
    let mut files: IndexSet<String> = IndexSet::new();
    for (u, &(t0, timed)) in opened.units.iter().zip(&starts) {
        if u.master.is_some() {
            segments += u.parts.len();
        }
        let dur = u.n as f64 / u.fs;
        samples_s += dur;
        if timed {
            lo = Some(lo.map_or(t0, |l: f64| l.min(t0)));
            hi = Some(hi.map_or(t0 + dur, |h: f64| h.max(t0 + dur)));
        } else {
            untimed += 1;
        }
        for p in &u.parts {
            for (name, _) in names_units(&p.h) {
                signals.insert(name);
            }
        }
        for f in record_files(&opened.base, &u.name) {
            files.insert(f);
        }
    }
    let bytes: u64 = files.iter().filter_map(|f| opened.base.join(f).metadata().ok()).map(|m| m.len()).sum();
    Ok(dict! {
        "path" => path.to_string_lossy().to_string(), "format" => format_label(path),
        "records" => opened.units.len(), "segments" => segments,
        "signals" => Value::List(signals.into_iter().map(Value::from).collect()),
        "duration_s" => samples_s, "t_range" => match (lo, hi) { (Some(a), Some(b)) => Value::List(vec![a.into(), b.into()]), _ => Value::None },
        "untimed" => untimed, "bytes" => bytes, "skipped" => opened.skipped.len(),
        "scan_s" => signal_core::pyfmt::round_to(t_start.elapsed().as_secs_f64(), 2),
    })
}

/// The files of record `name` (relative to `base`, `/` between folders): its header, the
/// signal files its header names, its `.mrk` file if there is one, and for a multi-segment
/// record those of each segment; sorted, each once.
pub fn record_files(base: &Path, name: &str) -> Vec<String> {
    let mut out: IndexSet<String> = IndexSet::new();
    fn add(base: &Path, name: &str, out: &mut IndexSet<String>) {
        let dir = match name.rfind('/') {
            Some(i) => &name[..=i],
            None => "",
        };
        out.insert(format!("{name}.hea"));
        if let Ok(h) = quiet().read_header(wpath(&base.join(name)), false) {
            for s in &h.siginfo {
                let f = String::from_utf8_lossy(s.fname.as_deref().unwrap_or_default()).to_string();
                if !f.is_empty() && f != "-" && f != "~" {
                    out.insert(format!("{dir}{f}"));
                }
            }
        }
        if base.join(format!("{name}.mrk")).is_file() {
            out.insert(format!("{name}.mrk"));
        }
    }
    add(base, name, &mut out);
    for (seg, _) in segment_table(&base.join(name)).unwrap_or_default() {
        if seg != "~" {
            add(base, &beside(name, &seg), &mut out);
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

pub struct Reader {
    folder: PathBuf,
    /// per piece: record, signal
    pieces: Vec<(String, usize)>,
}

impl Reader {
    /// The reader of the records at `path` (a folder, or a record's `.hea` file).
    pub fn new(path: &Path, meta: &Dict) -> Reader {
        let pieces = meta
            .get("pieces")
            .and_then(Value::as_list)
            .map(|l| {
                l.iter()
                    .map(|p| {
                        (
                            p.get("record").and_then(Value::as_str).unwrap_or("").to_string(),
                            p.get("sig").and_then(Value::as_int).unwrap_or(0) as usize,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let folder =
            if is_record(path) { path.parent().map(Path::to_path_buf).unwrap_or_default() } else { path.to_path_buf() };
        Reader { folder, pieces }
    }

    pub fn read(&self, piece: usize, a: i64, b: i64) -> signal_core::Result<Vec<f64>> {
        Ok(self.read_many(&[(piece, a, b)])?.swap_remove(0))
    }

    /// [`Reader::read`] of several `(piece, a, b)` at once: the signals of one record over the
    /// same samples are decoded together, once.
    pub fn read_many(&self, wanted: &[(usize, i64, i64)]) -> signal_core::Result<Vec<Vec<f64>>> {
        let mut groups: IndexMap<(&str, i64, i64), Vec<(usize, usize)>> = IndexMap::new();
        for (k, &(piece, a, b)) in wanted.iter().enumerate() {
            let (rec, sig) = self
                .pieces
                .get(piece)
                .ok_or_else(|| signal_core::Error::Value(format!("no piece {piece} in this recording")))?;
            groups.entry((rec.as_str(), a.max(0), b.max(0))).or_default().push((k, *sig));
        }
        let mut out: Vec<Vec<f64>> = vec![vec![]; wanted.len()];
        for ((rec, a, b), items) in groups {
            check_files(&self.folder, rec)?;
            let path = self.folder.join(rec);
            let h = header(&path)?;
            let (a, b) = (a as u64, (b as u64).min(h.nsamples.max(0) as u64));
            let sigs: Vec<usize> = items.iter().map(|&(_, s)| s).collect();
            let x = read_frames(&path, &h, a, b.max(a), &sigs)?;
            for ((k, _), y) in items.into_iter().zip(x) {
                out[k] = y;
            }
        }
        Ok(out)
    }
}

/// `[name, size, mtime ns]` of each file, for the cache key: every file below a folder, or the
/// files of one record ([`record_files`]); see `wfdb_files.rs`.
pub fn fingerprint(path: &Path) -> Vec<(String, u64, i128)> {
    super::wfdb_files::fingerprint(path, record_files)
}
