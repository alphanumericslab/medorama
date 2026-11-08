//! WFDB records as files, without reading them (no wfdb-x): which paths are records and WFDB
//! folders, the segment table of a multi-segment record, the format labels, the limits on a
//! folder of records, and a folder's files for the cache key.  Shared by the reader
//! ([`super::wfdb_folder`], with the `wfdb` feature) and by a build without it (`wfdb_none.rs`),
//! which re-export it: a build without WFDB support still recognises a WFDB record, and refuses
//! it ([`super::NO_WFDB`]) rather than taking it for something else.

use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The most files a folder's key is computed from (`fingerprint`).
pub const MAX_FINGERPRINT_FILES: usize = 200_000;

/// What a folder of plain records may hold before it is read: records (header files, the
/// segments of multi-segment records included), folder levels below it, and seconds for the
/// scan that counts them (docs/wfdb-collection.md, "Limits").  A collection (`index.csv`)
/// lists its records itself and is not limited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub max_records: usize,
    pub max_depth: usize,
    pub max_scan_s: f64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_records: 5000, max_depth: 20, max_scan_s: 10.0 }
    }
}

static LIMITS: RwLock<Option<Limits>> = RwLock::new(None);

/// Use `limits` for every folder read from now on (the programs' settings).
pub fn set_limits(limits: Limits) {
    *LIMITS.write().unwrap() = Some(limits);
}

/// The limits in force.
pub fn limits() -> Limits {
    LIMITS.read().unwrap().unwrap_or_default()
}

/// A folder written by the converters (index.csv), or holding WFDB records itself (RECORDS, or
/// a .hea file among its first `scan_limit` entries).  Any folder can be opened as records
/// (the reader reads its subfolders too); this tells which folders are recordings when a tree
/// is searched for them.
pub fn is_wfdb_folder(p: &Path, scan_limit: usize) -> bool {
    if !p.is_dir() {
        return false;
    }
    if p.join("index.csv").is_file() || p.join("RECORDS").is_file() {
        return true;
    }
    std::fs::read_dir(p)
        .map(|it| it.take(scan_limit).filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().ends_with(".hea")))
        .unwrap_or(false)
}

/// A WFDB header file (`.hea`): one record.
pub fn is_record(p: &Path) -> bool {
    p.is_file() && p.extension().is_some_and(|e| e == "hea")
}

/// Rows of a CSV file (Python's csv module, excel dialect) as dicts of strings.
pub fn read_csv(path: &Path) -> Vec<IndexMap<String, String>> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let (mut field, mut row, mut quoted, mut any) = (String::new(), Vec::new(), false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                any = true;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                any = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            c => {
                field.push(c);
                any = true;
            }
        }
    }
    if any || !field.is_empty() {
        row.push(field);
        rows.push(row);
    }
    let Some(head) = rows.first().cloned() else { return vec![] };
    rows.into_iter()
        .skip(1)
        .filter(|r| !(r.len() == 1 && r[0].is_empty()))
        .map(|r| head.iter().cloned().zip(r.into_iter().chain(std::iter::repeat(String::new()))).collect())
        .collect()
}

/// The lines of a header file that are neither blank nor comments, trimmed.
pub(crate) fn header_lines(hea: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(hea) else { return vec![] };
    String::from_utf8_lossy(&bytes)
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// A record's file with an extension added (`a.v1` → `a.v1.hea`): a record name may hold a dot,
/// which `Path::with_extension` would take for the start of an extension.
pub(crate) fn with_ext(rec: &Path, ext: &str) -> PathBuf {
    let mut s = rec.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// The segments of a multi-segment record, `(name, samples)` in order (`~` names a null
/// segment); None for a single-segment record.  `record`: the record's path without `.hea`.
pub fn segment_table(record: &Path) -> Option<Vec<(String, i64)>> {
    let lines = header_lines(&with_ext(record, "hea"));
    let first = lines.first()?;
    let (_, nseg) = first.split_whitespace().next()?.split_once('/')?;
    let nseg: usize = nseg.parse().ok()?;
    Some(
        lines[1..]
            .iter()
            .take(nseg)
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some((it.next()?.to_string(), it.next()?.parse().ok()?))
            })
            .collect(),
    )
}

/// The format label of what `path` opens as.
pub fn format_label(path: &Path) -> &'static str {
    if is_record(path) {
        if segment_table(&path.with_extension("")).is_some() {
            "WFDB multi-segment record"
        } else {
            "WFDB record"
        }
    } else if path.join("index.csv").is_file() {
        "WFDB collection"
    } else {
        "WFDB records"
    }
}

/// `[name, size, mtime ns]` of each file, for the cache key: every file below a folder
/// (hidden entries and links to folders skipped; a link to a file counts with its target's
/// size and time), or the files of one record (`record_files(base, name)`); `name` relative to the
/// folder (the record's folder), with `/` between components on every system
/// (viewers/vitalstream/docs/index-format.md).  A folder's walk visits names in order, enters
/// at most `max_depth` levels of subfolders ([`limits`]) and stops after
/// [`MAX_FINGERPRINT_FILES`] files.
pub(crate) fn fingerprint(path: &Path, record_files: fn(&Path, &str) -> Vec<String>) -> Vec<(String, u64, i128)> {
    fn stat(p: &Path) -> Option<(u64, i128)> {
        let m = p.metadata().ok()?;
        let mt = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i128);
        Some((m.len(), mt))
    }
    let mut out = vec![];
    if is_record(path) {
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        for f in record_files(&base, &name) {
            if let Some((size, mt)) = stat(&base.join(&f)) {
                out.push((f, size, mt));
            }
        }
        return out;
    }
    fn walk(dir: &Path, rel: &str, depth: usize, max_depth: usize, out: &mut Vec<(String, u64, i128)>) {
        let Ok(it) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, PathBuf, std::fs::FileType)> = it
            .filter_map(Result::ok)
            .filter_map(|e| Some((e.file_name().to_string_lossy().to_string(), e.path(), e.file_type().ok()?)))
            .filter(|(n, _, _)| !n.starts_with('.'))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, full, t) in entries {
            if out.len() >= MAX_FINGERPRINT_FILES {
                return;
            }
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if t.is_dir() {
                if depth < max_depth {
                    walk(&full, &r, depth + 1, max_depth, out);
                }
            } else if t.is_symlink() && !full.is_file() {
                // a link to a folder, or a broken link
            } else if let Some((size, mt)) = stat(&full) {
                out.push((r, size, mt));
            }
        }
    }
    walk(path, "", 0, limits().max_depth, &mut out);
    out.sort();
    out
}

// ------------------------------------------------------------------ signal files

/// Whether `p` is named as a WFDB signal file is (`.dat`, or `.mat`, a MATLAB file a header
/// names): not a record of its own, but read with the header that names it.
pub fn is_signal_file(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dat") || e.eq_ignore_ascii_case("mat"))
}

/// For a WFDB signal file (`.dat`, `.mat`) opened or added as a recording: what to open in its
/// place, its header (with the name of the header next to it that names it, when there is one).
/// None for any other path.
pub fn signal_file_note(p: &Path) -> Option<String> {
    if !p.is_file() || !is_signal_file(p) {
        return None;
    }
    let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
    let hea = p.with_extension("hea");
    let names_it = header_signals(&hea).iter().any(|s| {
        signal_path(&hea, &s.file).is_some_and(|f| f.file_name() == p.file_name() && f.parent() == hea.parent())
    });
    Some(if names_it {
        format!(
            "{name} is a WFDB signal file: open its header, {}",
            hea.file_name().unwrap_or_default().to_string_lossy()
        )
    } else {
        format!("{name} is a WFDB signal file: open the header (.hea) that names it")
    })
}

/// One signal line of a header: its file name, storage format, samples per frame and byte
/// offset (`file fmt[xspf][:skew][+offset] ...`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SignalLine {
    pub file: String,
    pub fmt: i64,
    pub spf: u64,
    pub offset: u64,
}

/// The record line's number of samples per signal (0: not stated), and the signal lines of a
/// single-segment header (none for a multi-segment one, or a header that cannot be read).
fn header_record(hea: &Path) -> (u64, Vec<SignalLine>) {
    let lines = header_lines(hea);
    let Some(first) = lines.first() else { return (0, vec![]) };
    let rec: Vec<&str> = first.split_whitespace().collect();
    if rec.first().is_none_or(|n| n.contains('/')) {
        return (0, vec![]);
    }
    let nsig: usize = rec.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
    let nsamp: u64 = rec.get(3).and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut out = vec![];
    for l in lines.iter().skip(1).take(nsig) {
        let mut it = l.split_whitespace();
        let (Some(file), Some(f)) = (it.next(), it.next()) else { break };
        let num = |s: &str| -> String { s.chars().take_while(char::is_ascii_digit).collect() };
        let after = |c: char| f.split_once(c).map(|(_, r)| num(r)).and_then(|n| n.parse().ok());
        let Ok(fmt) = num(f).parse() else { break };
        out.push(SignalLine {
            file: file.into(),
            fmt,
            spf: after('x').unwrap_or(1).max(1),
            offset: after('+').unwrap_or(0),
        });
    }
    (nsamp, out)
}

fn header_signals(hea: &Path) -> Vec<SignalLine> {
    header_record(hea).1
}

/// The file a header's signal file name designates, as wfdb-x opens it: `~` expanded, an
/// absolute name as given, any other next to the header.  None for a signal without a file.
fn signal_path(hea: &Path, f: &str) -> Option<PathBuf> {
    if f.is_empty() || f == "-" || f == "~" {
        return None;
    }
    let f = crate::paths::expand(f);
    if Path::new(&f).is_absolute() {
        return Some(PathBuf::from(f));
    }
    Some(hea.parent().map_or_else(|| PathBuf::from(&f), |d| d.join(&f)))
}

/// How a MAT file stores its matrix: element type, byte order, size and where the elements
/// start.
#[cfg_attr(not(feature = "wfdb"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct MatLayout {
    elem: MatElem,
    big_endian: bool,
    rows: u64,
    cols: u64,
    start: u64,
    /// The bytes of the elements a level 5 file states (a level 4 file states none).
    bytes: Option<u64>,
    /// Where the matrix's name lies (its bytes, without a final NUL).
    name: (usize, usize),
    /// Where the first matrix ends (the elements of a level 4 file; the element of a level 5
    /// file), as the file states it.
    end: u64,
}

#[cfg_attr(not(feature = "wfdb"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum MatElem {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl MatElem {
    fn size(self) -> u64 {
        match self {
            MatElem::I8 | MatElem::U8 => 1,
            MatElem::I16 | MatElem::U16 => 2,
            MatElem::I32 | MatElem::U32 | MatElem::F32 => 4,
            MatElem::I64 | MatElem::U64 | MatElem::F64 => 8,
        }
    }
    #[cfg(feature = "wfdb")]
    fn text(self) -> &'static str {
        match self {
            MatElem::I8 => "8-bit integers",
            MatElem::U8 => "8-bit unsigned integers",
            MatElem::I16 => "16-bit integers",
            MatElem::U16 => "16-bit unsigned integers",
            MatElem::I32 => "32-bit integers",
            MatElem::U32 => "32-bit unsigned integers",
            MatElem::I64 => "64-bit integers",
            MatElem::U64 => "64-bit unsigned integers",
            MatElem::F32 => "single-precision numbers",
            MatElem::F64 => "double-precision numbers",
        }
    }
}

/// The elements a WFDB storage format reads from a MAT file, and their byte order (None: either,
/// for one byte); None for a format no MAT matrix holds.
#[cfg(feature = "wfdb")]
fn mat_elem_of(fmt: i64) -> Option<(MatElem, Option<bool>)> {
    Some(match fmt {
        80 => (MatElem::U8, None),
        16 => (MatElem::I16, Some(false)),
        61 => (MatElem::I16, Some(true)),
        160 => (MatElem::U16, Some(false)),
        32 => (MatElem::I32, Some(false)),
        _ => return None,
    })
}

/// The layout of the first matrix of a MAT file (level 4, or level 5 uncompressed), from its
/// first bytes; an error saying why it is not one WFDB can read.
fn mat_layout(head: &[u8]) -> Result<MatLayout, String> {
    // level 5: "If any of the first four bytes is zero, it is a level 4 file" (the MAT-file format)
    if head.len() >= 4 && head[..4].iter().all(|&b| b != 0) {
        return mat5_layout(head);
    }
    if head.len() < 20 {
        return Err("it is too short for a MAT file".into());
    }
    let word = |i: usize, be: bool| {
        let b: [u8; 4] = head[i..i + 4].try_into().unwrap();
        if be {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        }
    };
    // the type MOPT: M the byte order (0 little-endian, 1 big-endian), O 0, P the element, T the kind
    let valid = |t: u32, m: u32| t < 5000 && t / 1000 == m && (t / 100).is_multiple_of(10) && (t / 10) % 10 <= 5;
    let be = if valid(word(0, false), 0) {
        false
    } else if valid(word(0, true), 1) {
        true
    } else {
        return Err("it is not a MAT file (level 4 or 5) that WFDB reads".into());
    };
    let t = word(0, be);
    if t % 10 != 0 {
        return Err(if t % 10 == 1 { "it holds text, not a numeric matrix" } else { "it holds a sparse matrix" }.into());
    }
    if word(12, be) != 0 {
        return Err("it holds a complex matrix".into());
    }
    let elem =
        [MatElem::F64, MatElem::F32, MatElem::I32, MatElem::I16, MatElem::U16, MatElem::U8][((t / 10) % 10) as usize];
    let (rows, cols, namlen) = (word(4, be) as u64, word(8, be) as u64, word(16, be) as usize);
    let start = 20 + namlen as u64;
    Ok(MatLayout {
        elem,
        big_endian: be,
        rows,
        cols,
        start,
        bytes: None,
        name: (20, namlen.saturating_sub(1)),
        end: start.saturating_add(rows.saturating_mul(cols).saturating_mul(elem.size())),
    })
}

fn mat5_layout(head: &[u8]) -> Result<MatLayout, String> {
    let be = match head.get(126..128) {
        Some(b"IM") => false,
        Some(b"MI") => true,
        _ => return Err("it is not a MAT file (level 4 or 5) that WFDB reads".into()),
    };
    let u16_at = |i: usize| {
        let b = [head[i], head[i + 1]];
        if be {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        }
    };
    match u16_at(124) {
        0x0100 => {}
        0x0200 => return Err("it is a MATLAB 7.3 (HDF5) file: save it with save(..., '-v6') or '-v4'".into()),
        v => return Err(format!("it is a MAT file of an unknown version ({v:#06x})")),
    }
    let short = || "it ends inside its first matrix".to_string();
    let u32_at = |i: usize| -> Result<u32, String> {
        let b: [u8; 4] = head.get(i..i + 4).ok_or_else(short)?.try_into().unwrap();
        Ok(if be { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };
    // a data element: its tag (type, bytes) and where its data start; a small element packs
    // both in 4 bytes, with its data in the next 4
    let tag = |i: usize| -> Result<(u32, u64, usize), String> {
        let w = u32_at(i)?;
        if w >> 16 != 0 {
            Ok((w & 0xffff, (w >> 16) as u64, i + 4))
        } else {
            Ok((w, u32_at(i + 4)? as u64, i + 8))
        }
    };
    let next =
        |data: usize, bytes: u64, small: bool| if small { data + 4 } else { data + (bytes as usize).div_ceil(8) * 8 };
    let (kind, total, mut at) = tag(128)?;
    match kind {
        14 => {}
        15 => {
            return Err("it is compressed (MATLAB saves so by default): save it with save(..., '-v6') or '-v4'".into())
        }
        _ => return Err("its first variable is not a matrix".into()),
    }
    // array flags: the class in the low byte, the complex flag (0x08) in the next
    let (_, n, data) = tag(at)?;
    let flags = u32_at(data)?;
    let class = flags & 0xff;
    if (flags >> 8) & 0x08 != 0 {
        return Err("it holds a complex matrix".into());
    }
    match class {
        5 => return Err("it holds a sparse matrix".into()),
        6..=15 => {}
        _ => return Err("its first variable is not a numeric matrix".into()),
    }
    at = next(data, n, data == at + 4);
    // dimensions: two 32-bit integers
    let (_, n, data) = tag(at)?;
    if n != 8 {
        return Err(format!("its first variable has {} dimensions, not 2", n / 4));
    }
    let (rows, cols) = (u32_at(data)? as u64, u32_at(data + 4)? as u64);
    at = next(data, n, data == at + 4);
    // the name
    let (_, n, data) = tag(at)?;
    let name = (data, n as usize);
    at = next(data, n, data == at + 4);
    // the real part
    let (t, n, data) = tag(at)?;
    let elem = match t {
        1 => MatElem::I8,
        2 => MatElem::U8,
        3 => MatElem::I16,
        4 => MatElem::U16,
        5 => MatElem::I32,
        6 => MatElem::U32,
        7 => MatElem::F32,
        9 => MatElem::F64,
        12 => MatElem::I64,
        13 => MatElem::U64,
        _ => return Err(format!("its elements are of an unknown type ({t})")),
    };
    Ok(MatLayout { elem, big_endian: be, rows, cols, start: data as u64, bytes: Some(n), name, end: 136 + total })
}

/// Why a MAT signal file does not match the header that names it: the matrix must hold the
/// elements of the header's format, one row per signal of the file (one per sample of a frame),
/// one column per frame, starting at the header's byte offset.  `signals`: the header's
/// signals in this file; `nsamp`: the record line's samples per signal (0: not stated).
#[cfg(feature = "wfdb")]
fn mat_mismatch(head: &[u8], size: u64, signals: &[&SignalLine], nsamp: u64) -> Option<String> {
    let l = match mat_layout(head) {
        Ok(l) => l,
        Err(e) => return Some(e),
    };
    let s = signals[0];
    if let Some(o) = signals.iter().find(|o| o.fmt != s.fmt) {
        return Some(format!("the header gives it two storage formats ({} and {})", s.fmt, o.fmt));
    }
    let Some((elem, order)) = mat_elem_of(s.fmt) else {
        return Some(format!(
            "the header gives format {}, which no MAT matrix holds (8-bit: 80, 16-bit: 16 or 61, 32-bit: 32)",
            s.fmt
        ));
    };
    let endian = |be: bool| if be { "big-endian" } else { "little-endian" };
    if l.elem != elem || order.is_some_and(|be| be != l.big_endian) {
        let got = if l.elem.size() > 1 {
            format!("{} {}", endian(l.big_endian), l.elem.text())
        } else {
            l.elem.text().into()
        };
        let want = match order {
            Some(be) => format!("{} {}", endian(be), elem.text()),
            None => elem.text().into(),
        };
        return Some(format!("it holds {got}; format {} reads {want}", s.fmt));
    }
    let rows: u64 = signals.iter().map(|s| s.spf).sum();
    if l.rows != rows {
        let hint = if l.cols == rows { " (it is transposed: one column per signal)" } else { "" };
        return Some(format!(
            "its matrix has {} rows; the header gives {rows} signals in it, one row each{hint}",
            l.rows
        ));
    }
    if l.start != s.offset {
        return Some(format!("its samples start at byte {}; the header gives +{}", l.start, s.offset));
    }
    if nsamp > 0 && l.cols != nsamp {
        return Some(format!("it holds {} samples per signal; the header gives {nsamp}", l.cols));
    }
    let need = l.rows * l.cols * l.elem.size();
    if l.bytes.is_some_and(|b| b != need) || size < l.start + need {
        return Some(format!("it is shorter than its {} × {} matrix", l.rows, l.cols));
    }
    None
}

/// An error for the first MAT signal file (`.mat`) of a single-segment header that does not
/// match it ([`mat_mismatch`]): a MAT file is read as WFDB reads a signal file, its matrix as
/// frames from the header's byte offset, so a file of another layout (compressed, of doubles,
/// transposed) would be read as noise.  Files that cannot be opened, or lie outside the data
/// roots, are not checked here (reading them fails).
#[cfg(feature = "wfdb")]
pub fn check_mat_files(hea: &Path) -> Result<(), String> {
    let (nsamp, signals) = header_record(hea);
    let mut files: IndexMap<String, Vec<&SignalLine>> = IndexMap::new();
    for s in &signals {
        if Path::new(&s.file).extension().is_some_and(|e| e.eq_ignore_ascii_case("mat")) {
            files.entry(s.file.clone()).or_default().push(s);
        }
    }
    for (file, sigs) in files {
        let Some(p) = signal_path(hea, &file) else { continue };
        if !crate::paths::allowed_file(&p) {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(&p) else { continue };
        let size = f.metadata().map_or(0, |m| m.len());
        let mut head = Vec::with_capacity(512);
        if std::io::Read::read_to_end(&mut std::io::Read::take(&mut f, 512), &mut head).is_err() {
            continue;
        }
        if let Some(why) = mat_mismatch(&head, size, &sigs, nsamp) {
            let h = hea.file_name().unwrap_or_default().to_string_lossy();
            return Err(format!("{file} does not match its header {h}: {why}"));
        }
    }
    Ok(())
}

/// A MAT signal file as Privacy Mode sends it: its first matrix only (a MAT file may hold other
/// variables after the samples WFDB reads), its descriptive text (the program, the computer, the
/// time it was written) and the matrix's name blanked.  The bytes before the samples, rewritten,
/// and where in the file the samples start and the matrix ends: the file is sent as these bytes
/// then its bytes `[from, until)`.  `head`: the file's first bytes (512 hold every header WFDB
/// files have); `size`: its length.  An error for a file that is not a MAT file WFDB reads.
pub fn mat_deidentified(head: &[u8], size: u64) -> Result<(Vec<u8>, u64, u64), String> {
    let l = mat_layout(head)?;
    let from = l.start;
    if from as usize > head.len() || from > size || l.name.0 + l.name.1 > from as usize {
        return Err("its header is not one WFDB files have".into());
    }
    let mut out = head[..from as usize].to_vec();
    if l.bytes.is_some() {
        // level 5: 116 bytes of text, then the offset of subsystem data (none)
        let text = b"MATLAB 5.0 MAT-file";
        out[..116].fill(b' ');
        out[..text.len()].copy_from_slice(text);
        out[116..124].fill(0);
    }
    out[l.name.0..l.name.0 + l.name.1].fill(b'x');
    Ok((out, from, l.end.clamp(from, size)))
}
