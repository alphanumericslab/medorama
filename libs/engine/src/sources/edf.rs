//! EDF, EDF+ and BDF files (European Data Format, <https://www.edfplus.info/specs/>): one
//! file is one recording (viewers/vitalstream/docs/index-format.md, "EDF, EDF+ and BDF").
//!
//! - Each ordinary signal is a channel (its label and physical dimension), at
//!   `samples per data record / duration of a data record` Hz; physical values are
//!   `pmin + (digital - dmin) * (pmax - pmin) / (dmax - dmin)` (the digital values themselves
//!   when `dmin == dmax`).  BDF and BDF+ files hold 24-bit samples, EDF and EDF+ 16-bit.
//! - Data records follow each other from the header's start time (EDF), or from the onsets the
//!   EDF+ time-keeping annotations state (EDF+C and EDF+D): a record that does not start where
//!   the one before it ends begins a new piece of every channel, with a break between them.
//! - The annotations of EDF+ and BDF+ files (`EDF Annotations`, `BDF Annotations` signals) are
//!   events with their onset, duration and text.
//! - The header's patient and recording identification fields are never part of the index: they
//!   hold the patient's identity.  Only [`identification`] reads them, for Medorama's `identify`
//!   action, which keeps what they say with the catalog's other identifiers.

use super::Built;
use crate::common::{untimed_spans, Channels, Pieces};
use signal_core::dict;
use signal_core::value::{Dict, Value};
use signal_core::{Error, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Records read at once while indexing (at most this many bytes).
const BATCH_BYTES: usize = 1 << 22;
/// Records closer than this (seconds) to where the one before ends follow it without a break.
const CONTIGUOUS_S: f64 = 1e-6;

/// Whether `p` is a file named as an EDF or BDF file (`.edf`, `.bdf`, any case).
pub fn is_edf(p: &Path) -> bool {
    p.is_file()
        && matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("edf" | "bdf"))
}

/// One signal of the header.
#[derive(Clone, Debug)]
pub struct Signal {
    pub label: String,
    pub unit: String,
    pmin: f64,
    pmax: f64,
    dmin: f64,
    dmax: f64,
    /// samples per data record
    pub spr: usize,
    /// an `EDF Annotations` (`BDF Annotations`) signal
    pub annotations: bool,
    /// where its samples start in a data record (bytes)
    offset: usize,
}

impl Signal {
    fn physical(&self, d: i32) -> f64 {
        if self.dmax == self.dmin {
            return d as f64;
        }
        let gain = (self.pmax - self.pmin) / (self.dmax - self.dmin);
        self.pmin + (d as f64 - self.dmin) * gain
    }
}

/// What the header says (the identification fields left out).
#[derive(Clone, Debug)]
pub struct Header {
    /// 24-bit samples (BDF, BDF+)
    pub bdf: bool,
    /// EDF+ or BDF+ (`C` continuous, `D` discontinuous)
    pub plus: Option<char>,
    /// the start (Unix seconds, the header's clock time taken as UTC), if the header states a valid one
    pub start: Option<f64>,
    header_bytes: u64,
    /// data records stated in the header (-1: unknown)
    declared: i64,
    /// data records in the file (whole ones)
    pub records: u64,
    /// bytes after the last whole record
    rest: u64,
    /// seconds per data record
    pub duration: f64,
    pub signals: Vec<Signal>,
    record_bytes: usize,
}

/// A header field: its bytes as Latin-1 text, without the spaces around it.
fn field(b: &[u8]) -> String {
    let s: String = b.iter().map(|&c| c as char).collect();
    signal_core::text::py_strip(&s).to_string()
}

fn number(b: &[u8], what: &str) -> Result<f64> {
    let s = field(b);
    signal_core::text::parse_float(&s)
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::Format(format!("EDF header: {what} is not a number ({s:?})")))
}

fn integer(b: &[u8], what: &str) -> Result<i64> {
    let s = field(b);
    s.parse::<i64>().map_err(|_| Error::Format(format!("EDF header: {what} is not a whole number ({s:?})")))
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Three two-digit numbers `aa?bb?cc` (any separators), as EDF writes dates and times; the
/// third may be anything when `loose` (the year of an EDF+ date after 2084 is `yy`).
fn three(s: &str, loose: bool) -> Option<(i64, i64, Option<i64>)> {
    let b = s.as_bytes();
    let digits = |i: usize| b[i].is_ascii_digit() && b[i + 1].is_ascii_digit();
    if b.len() != 8 || !digits(0) || !digits(3) || b[2].is_ascii_digit() || b[5].is_ascii_digit() {
        return None;
    }
    let n = |i: usize| ((b[i] - b'0') * 10 + (b[i + 1] - b'0')) as i64;
    match digits(6) {
        true => Some((n(0), n(3), Some(n(6)))),
        false if loose => Some((n(0), n(3), None)),
        false => None,
    }
}

/// The four-digit year of an EDF+ recording field (`Startdate 02-MAR-2002 ...`), if it states one.
fn plus_year(recording: &str) -> Option<i64> {
    let mut words = recording.split(' ');
    if words.next() != Some("Startdate") {
        return None;
    }
    let date = words.next()?;
    let y = date.get(7..)?;
    (date.len() == 11 && y.bytes().all(|c| c.is_ascii_digit())).then(|| y.parse().ok())?
}

/// The start of a file: its header's date (`dd.mm.yy`: 85-99 are 1985-1999, the others 2000-2084;
/// an EDF+ recording field's `Startdate dd-MMM-yyyy` gives the year in full) and time (`hh.mm.ss`).
fn start_of(date: &str, time: &str, recording: Option<&str>) -> Option<f64> {
    let full = recording.and_then(plus_year);
    let (d, m, yy) = three(date, full.is_some())?;
    let (hh, mi, ss) = three(time, false)?;
    let ss = ss?;
    let year = match (full, yy) {
        (Some(y), _) => y,
        (None, Some(yy)) => {
            if yy >= 85 {
                1900 + yy
            } else {
                2000 + yy
            }
        }
        (None, None) => return None,
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mi > 59 || ss > 59 {
        return None;
    }
    Some((days_from_civil(year, m, d) * 86400 + hh * 3600 + mi * 60 + ss) as f64)
}

/// The header of the EDF or BDF file at `path`.
pub fn header(path: &Path) -> Result<Header> {
    let mut f = std::fs::File::open(path).map_err(Error::io(path))?;
    let size = f.metadata().map_err(Error::io(path))?.len();
    let mut b = vec![0u8; 256];
    f.read_exact(&mut b).map_err(|_| Error::Format(format!("not an EDF or BDF file: {}", path.display())))?;
    // the signal fields, when the number of signals is one
    if let Ok(ns) = field(&b[252..256]).parse::<usize>() {
        if (1..=9999).contains(&ns) {
            b.resize(256 * (ns + 1), 0);
            f.read_exact(&mut b[256..])
                .map_err(|_| Error::Format(format!("EDF header cut short: {}", path.display())))?;
        }
    }
    parse_header(&b, size, &path.display().to_string())
}

/// The header of an EDF or BDF file from its first bytes (at least the whole header), the
/// file being `data.len()` bytes long: [`header`] without a file (for fuzzing and tests).
pub fn header_from_bytes(data: &[u8]) -> Result<Header> {
    parse_header(data, data.len() as u64, "the data")
}

/// The header in `b` (the file's first bytes) of a file of `size` bytes, named `name` in errors.
fn parse_header(b: &[u8], size: u64, name: &str) -> Result<Header> {
    let not_edf = || Error::Format(format!("not an EDF or BDF file: {name}"));
    let fixed: &[u8; 256] = b.get(..256).and_then(|x| x.try_into().ok()).ok_or_else(not_edf)?;
    let bdf = fixed[0] == 0xFF && &fixed[1..8] == b"BIOSEMI";
    if !bdf && field(&fixed[0..8]) != "0" {
        return Err(not_edf());
    }
    let reserved = field(&fixed[192..236]);
    let plus = ["EDF+C", "EDF+D", "BDF+C", "BDF+D"]
        .iter()
        .find(|p| reserved.starts_with(*p))
        .map(|p| p.chars().last().unwrap_or('C'));
    let recording = field(&fixed[88..168]);
    let start = start_of(&field(&fixed[168..176]), &field(&fixed[176..184]), plus.map(|_| recording.as_str()));
    let header_bytes = integer(&fixed[184..192], "the number of header bytes")?;
    let declared = integer(&fixed[236..244], "the number of data records")?;
    let duration = number(&fixed[244..252], "the duration of a data record")?;
    let ns = integer(&fixed[252..256], "the number of signals")?;
    if !(1..=9999).contains(&ns) || header_bytes != 256 * (ns + 1) {
        return Err(Error::Format(format!(
            "EDF header: {ns} signals do not fit its {header_bytes} header bytes: {name}"
        )));
    }
    let ns = ns as usize;
    let rest = b.get(256..256 * (ns + 1)).ok_or_else(|| Error::Format(format!("EDF header cut short: {name}")))?;
    // the signal fields, each for every signal in turn
    let widths = [16usize, 80, 8, 8, 8, 8, 8, 80, 8, 32];
    let mut at = 0;
    let mut cols: Vec<Vec<&[u8]>> = vec![];
    for w in widths {
        cols.push((0..ns).map(|k| &rest[at + k * w..at + (k + 1) * w]).collect());
        at += w * ns;
    }
    let width = if bdf { 3 } else { 2 };
    let mut signals = Vec::with_capacity(ns);
    let mut offset = 0;
    let col = |c: usize, k: usize| cols[c][k];
    for k in 0..ns {
        let label = field(col(0, k));
        let spr = integer(col(8, k), "a number of samples per data record")?;
        if spr < 0 {
            return Err(Error::Format(format!("EDF header: signal {} has {spr} samples per data record", k + 1)));
        }
        let s = Signal {
            annotations: label == "EDF Annotations" || label == "BDF Annotations",
            label,
            unit: field(col(2, k)),
            pmin: number(col(3, k), "a physical minimum")?,
            pmax: number(col(4, k), "a physical maximum")?,
            dmin: number(col(5, k), "a digital minimum")?,
            dmax: number(col(6, k), "a digital maximum")?,
            spr: spr as usize,
            offset,
        };
        offset += s.spr * width;
        signals.push(s);
    }
    let record_bytes = offset;
    let body = size.saturating_sub(header_bytes as u64);
    let (records, rest) =
        if record_bytes == 0 { (0, body) } else { (body / record_bytes as u64, body % record_bytes as u64) };
    let records = if declared >= 0 { records.min(declared as u64) } else { records };
    if !(duration > 0.0) && signals.iter().any(|s| !s.annotations && s.spr > 0) {
        return Err(Error::Format(format!("EDF header: a data record lasts {duration} s: {name}")));
    }
    // a sampling rate that is not a finite number (a data record of 1e-320 s)
    if let Some(s) = signals.iter().find(|s| !s.annotations && s.spr > 0 && !(s.spr as f64 / duration).is_finite()) {
        return Err(Error::Format(format!(
            "EDF header: signal {} has no finite sampling rate ({} samples in {duration} s): {name}",
            s.label, s.spr
        )));
    }
    Ok(Header {
        bdf,
        plus,
        start,
        header_bytes: header_bytes as u64,
        declared,
        records,
        rest,
        duration,
        signals,
        record_bytes,
    })
}

/// The header's local patient and local recording identification fields, as they are written:
/// identifiers (PHI), for Medorama's `identify` action only.  EDF+ structures them as
/// `code sex birthdate name ...` and `Startdate dd-MMM-yyyy admincode technician equipment ...`.
pub fn identification(path: &Path) -> Result<(String, String)> {
    let mut f = std::fs::File::open(path).map_err(Error::io(path))?;
    let mut fixed = [0u8; 168];
    f.read_exact(&mut fixed).map_err(|_| Error::Format(format!("not an EDF or BDF file: {}", path.display())))?;
    Ok((field(&fixed[8..88]), field(&fixed[88..168])))
}

/// The format label of an EDF or BDF file: `EDF`, `EDF+`, `BDF`, `BDF+`.
pub fn format_label(path: &Path) -> &'static str {
    match header(path) {
        Ok(h) => match (h.bdf, h.plus.is_some()) {
            (false, false) => "EDF",
            (false, true) => "EDF+",
            (true, false) => "BDF",
            (true, true) => "BDF+",
        },
        Err(_) => "EDF",
    }
}

/// Python's `' '.join(text.split())`.
fn words(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The time-stamped annotation lists of an annotation signal's bytes:
/// `(onset, duration, texts)`, each `+onset[\x15duration]\x14text\x14...\x14\x00`.
fn tals(bytes: &[u8]) -> Vec<(f64, Option<f64>, Vec<String>)> {
    let mut out = vec![];
    for tal in bytes.split(|&c| c == 0).filter(|t| !t.is_empty()) {
        let mut parts = tal.split(|&c| c == 0x14);
        let Some(time) = parts.next() else { continue };
        let mut t = time.split(|&c| c == 0x15);
        let num = |b: Option<&[u8]>| {
            b.and_then(|b| std::str::from_utf8(b).ok())
                .and_then(signal_core::text::parse_float)
                .filter(|v| v.is_finite())
        };
        let Some(onset) = num(t.next()) else { continue };
        let duration = num(t.next());
        let texts = parts.map(|p| words(&String::from_utf8_lossy(p))).collect();
        out.push((onset, duration, texts));
    }
    out
}

/// A record's samples of one signal, as digital values.
fn digital(rec: &[u8], s: &Signal, bdf: bool) -> Vec<i32> {
    let b = &rec[s.offset..];
    if bdf {
        (0..s.spr).map(|i| i32::from_le_bytes([b[3 * i], b[3 * i + 1], b[3 * i + 2], 0]) << 8 >> 8).collect()
    } else {
        (0..s.spr).map(|i| i16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as i32).collect()
    }
}

fn read_records(f: &mut std::fs::File, h: &Header, first: u64, count: u64, path: &Path) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; count as usize * h.record_bytes];
    f.seek(SeekFrom::Start(h.header_bytes + first * h.record_bytes as u64)).map_err(Error::io(path))?;
    f.read_exact(&mut buf).map_err(Error::io(path))?;
    Ok(buf)
}

pub fn build(path: &Path, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<Built> {
    let h = header(path)?;
    let data: Vec<usize> =
        (0..h.signals.len()).filter(|&k| !h.signals[k].annotations && h.signals[k].spr > 0).collect();
    let notes_sig: Vec<usize> = (0..h.signals.len()).filter(|&k| h.signals[k].annotations).collect();
    if data.is_empty() {
        return Err(Error::Value(format!("{} holds no signals, only annotations", path.display())));
    }
    if h.records == 0 {
        return Err(Error::Value(format!("{} holds no data records", path.display())));
    }
    let start = h.start.unwrap_or(0.0);
    let timed = h.start.is_some();

    // channels, in the order of the signals; a label repeated gets its count ("ECG (2)")
    let mut channels = Channels::default();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let ch: Vec<usize> = data
        .iter()
        .map(|&k| {
            let s = &h.signals[k];
            let n = seen.entry(s.label.clone()).or_insert(0);
            *n += 1;
            let label = if *n == 1 { s.label.clone() } else { format!("{} ({n})", s.label) };
            channels.get(format!("edf\u{1}{k}"), &label, &s.unit, "edf", None)
        })
        .collect();

    let mut pieces = Pieces::default();
    let mut info: Vec<(usize, u64)> = vec![]; // per piece: signal, first data record
    let mut current: Vec<usize> = vec![]; // the pieces of the current run
    let mut runs: Vec<(f64, f64)> = vec![]; // [start, end) of each run, from the file's start
    let mut events: Vec<(f64, f64, String)> = vec![];
    let mut file = std::fs::File::open(path).map_err(Error::io(path))?;
    let batch = (BATCH_BYTES / h.record_bytes.max(1)).max(1) as u64;
    let mut k = 0u64;
    while k < h.records {
        if cancelled() {
            return Err(Error::Value("indexing cancelled".into()));
        }
        let count = batch.min(h.records - k);
        let buf = read_records(&mut file, &h, k, count, path)?;
        for (j, rec) in buf.chunks(h.record_bytes).enumerate() {
            let r = k + j as u64;
            // the record's onset: its time-keeping annotation (EDF+), else where it follows on
            let mut onset = None;
            for (i, &a) in notes_sig.iter().enumerate() {
                let s = &h.signals[a];
                let width = if h.bdf { 3 } else { 2 };
                for (n, (t, d, texts)) in tals(&rec[s.offset..s.offset + s.spr * width]).into_iter().enumerate() {
                    if i == 0 && n == 0 && h.plus.is_some() {
                        onset = Some(t);
                    }
                    for text in texts.into_iter().filter(|x| !x.is_empty()) {
                        events.push((t, d.filter(|d| *d > 0.0).unwrap_or(0.0), text));
                    }
                }
            }
            let follows = runs.last().map(|&(_, end)| end);
            let onset = onset.unwrap_or_else(|| follows.unwrap_or(0.0));
            if follows.is_none_or(|end| (onset - end).abs() > CONTIGUOUS_S) {
                pieces.room(data.len())?;
                runs.push((onset, onset));
                current = data
                    .iter()
                    .zip(&ch)
                    .map(|(&s, &c)| {
                        info.push((s, r));
                        pieces.start(
                            c,
                            h.signals[s].spr as f64 / h.duration,
                            dict! { "t0" => start + onset, "timed" => timed },
                        )
                    })
                    .collect();
            }
            let run = runs.last_mut().unwrap();
            run.1 = onset + h.duration;
            for (&s, &p) in data.iter().zip(&current) {
                let sig = &h.signals[s];
                let x: Vec<f32> = digital(rec, sig, h.bdf).into_iter().map(|d| sig.physical(d) as f32).collect();
                pieces.add(p, &x);
            }
        }
        k += count;
        progress(k as f64 / h.records as f64);
    }

    let (mut items, arrays, marker_names) = pieces.finish();
    for (p, (sig, first)) in items.iter_mut().zip(info) {
        p.insert("sig".into(), sig.into());
        p.insert("data_record".into(), (first as i64).into());
    }
    let mut gaps = vec![];
    for w in runs.windows(2) {
        let (a, b) = (start + w[0].1, start + w[1].0);
        if b > a {
            gaps.push(Value::Dict(dict! {
                "t0" => a, "t1" => b, "reason" => "not recorded", "gap_s" => b - a, "exact" => true,
                "measured_by" => Value::None, "notes" => "",
            }));
        }
    }
    let mut alarms: Vec<Dict> = events
        .into_iter()
        .map(|(t, d, text)| {
            dict! { "t0" => start + t, "t1" => start + t + d, "text" => text, "level" => Value::None, "kind" => "annotation" }
        })
        .collect();
    alarms.sort_by(|a, b| {
        crate::common::t0_of(a).partial_cmp(&crate::common::t0_of(b)).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut notes: Vec<Value> = vec![];
    notes.push(if timed {
        "The file's start time is taken as UTC.".into()
    } else {
        "The file states no valid start time: times are counted from its start, and marked \"time not stated\".".into()
    });
    if h.declared >= 0 && (h.declared as u64) > h.records {
        notes.push(format!("The header states {} data records; the file holds {}.", h.declared, h.records).into());
    } else if h.rest > 0 {
        notes.push(format!("The file ends inside a data record: its last {} bytes are left out.", h.rest).into());
    }
    let untimed = untimed_spans(&items);
    let meta = dict! {
        "channels" => Value::List(channels.items.into_iter().map(Value::Dict).collect()),
        "pieces" => Value::List(items.into_iter().map(Value::Dict).collect()),
        "vitals" => Value::List(vec![]),
        "alarms" => Value::List(alarms.into_iter().map(Value::Dict).collect()),
        "gaps" => Value::List(gaps), "untimed" => Value::List(untimed), "marker_names" => marker_names,
        "skipped" => Value::Dict(Dict::new()), "notes" => Value::List(notes), "time_resolution_s" => Value::None,
        "reader" => Value::Dict(Dict::new()),
    };
    Ok((meta, arrays))
}

/// Physical samples of an indexed EDF or BDF file's pieces.
pub struct Reader {
    path: std::path::PathBuf,
    /// per piece: signal, first data record, samples
    pieces: Vec<(usize, u64, i64)>,
}

impl Reader {
    pub fn new(path: &Path, meta: &Dict) -> Reader {
        let int = |p: &Value, k: &str| p.get(k).and_then(Value::as_int).unwrap_or(0) as i64;
        let pieces = meta
            .get("pieces")
            .and_then(Value::as_list)
            .map(|l| l.iter().map(|p| (int(p, "sig") as usize, int(p, "data_record") as u64, int(p, "n"))).collect())
            .unwrap_or_default();
        Reader { path: path.to_path_buf(), pieces }
    }

    /// Physical samples `[a, b)` of a piece.
    pub fn read(&self, piece: usize, a: i64, b: i64) -> Result<Vec<f64>> {
        let &(sig, first, n) =
            self.pieces.get(piece).ok_or_else(|| Error::Value(format!("no piece {piece} in this recording")))?;
        let (a, b) = (a.clamp(0, n), b.clamp(0, n));
        if b <= a {
            return Ok(vec![]);
        }
        let h = header(&self.path)?;
        // a signal without samples now (the file was rewritten in place) would divide by zero
        let s = h
            .signals
            .get(sig)
            .filter(|s| s.spr > 0)
            .ok_or_else(|| Error::Value("the file has changed since it was indexed".into()))?;
        let spr = s.spr as i64;
        let (r0, r1) = (a / spr, (b - 1) / spr);
        if first + r1 as u64 >= h.records {
            return Err(Error::Value("the file has changed since it was indexed".into()));
        }
        let mut f = std::fs::File::open(&self.path).map_err(Error::io(&self.path))?;
        let buf = read_records(&mut f, &h, first + r0 as u64, (r1 - r0 + 1) as u64, &self.path)?;
        let mut out = Vec::with_capacity((b - a) as usize);
        for (j, rec) in buf.chunks(h.record_bytes).enumerate() {
            let base = (r0 + j as i64) * spr;
            for (i, d) in digital(rec, s, h.bdf).into_iter().enumerate() {
                let at = base + i as i64;
                if at >= a && at < b {
                    out.push(s.physical(d));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_annotations() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 3, 1), 19783);
        assert_eq!(days_from_civil(1999, 12, 31), 10956);
        // two-digit years: 85-99 are 19xx, the rest 20xx; EDF+ states the year in full
        assert_eq!(start_of("02.03.99", "10.00.00", None), Some(920368800.0));
        assert_eq!(start_of("01.03.24", "13.00.00", None), Some(1709298000.0));
        assert_eq!(start_of("01.03.yy", "13.00.00", Some("Startdate 01-MAR-2091 X X X")), Some(3823592400.0));
        assert_eq!(start_of("01.03.yy", "13.00.00", None), None);
        assert_eq!(start_of("01.03.91", "13.00.00", Some("Startdate 01-MAR-2091 X X X")), Some(3823592400.0));
        assert_eq!(start_of("31.02.24", "25.00.00", None), None);
        let t = tals(b"+0.25\x14\x14\x00+1.5\x152\x14Arousal\x14 two  words \x14\x00\x00\x00");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].0, t[0].1, t[0].2.clone()), (0.25, None, vec![String::new(), String::new()]));
        assert_eq!(
            (t[1].0, t[1].1, t[1].2.clone()),
            (1.5, Some(2.0), vec!["Arousal".into(), "two words".into(), String::new()])
        );
    }
}
