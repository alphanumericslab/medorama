//! Files attached to items: kinds, viewers, the paths that may be read, folders and WFDB parts.

use super::*;

pub fn file_kind(path: &str) -> &'static str {
    if Path::new(path).is_dir() {
        return "folder";
    }
    let kind = file_kind_by_name(path);
    // DICOM files often have no extension (IM0001): the "DICM" marker says it
    if kind == "other" && is_dicom(Path::new(path)) {
        return "dicom";
    }
    kind
}

/// Whether a file is a DICOM Part 10 file ("DICM" after its 128-byte preamble).
pub fn is_dicom(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 132];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head[128..] == b"DICM"
}

pub(super) fn file_kind_by_name(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") => "image",
        Some("pdf") => "pdf",
        Some("txt") => "text",
        Some("mp4" | "webm" | "mov" | "m4v") => "video",
        Some("hea") => "wfdb",
        Some("edf" | "bdf") => "edf",
        Some("json") => "json",
        Some("dcm" | "dicom") => "dicom",
        Some("xml") => "xml",
        _ => "other",
    }
}

/// Where a viewer reads an attachment: the file under its own name (so that a WFDB header's
/// signal files resolve beside it), or a folder's listing.
pub fn viewer_src(url: &str, kind: &str, name: &str) -> String {
    match kind {
        "folder" => format!("{url}/"),
        _ => format!("{url}/{}", encode_name(name)),
    }
}

/// What a viewer needs that a picture or video does not say, from the columns of the row the
/// item came from: its scale (`mm` per pixel; from `pixel spacing (mm)`, `mm per pixel`,
/// `pixel size (cm)` …) and frame rate (`fps`; from `fps`, `frame rate (fps)`, `frame rate (Hz)`).
pub fn viewer_hints(c: &Connection, item: i64) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    let row: Option<(i64, String)> = c
        .query_row("SELECT import_id, cells FROM import_rows WHERE item = ?1 ORDER BY id LIMIT 1", [item], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let Some((import, cells)) = row else { return Ok(out) };
    let cells: Vec<String> = serde_json::from_str(&cells).unwrap_or_default();
    let mut st = c.prepare("SELECT pos, header FROM import_columns WHERE import_id = ?1")?;
    let cols: Vec<(usize, String)> =
        st.query_map([import], |r| Ok((r.get::<_, i64>(0)? as usize, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (pos, header) in cols {
        let Some(v) = cells.get(pos).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 0.0) else { continue };
        let (name, unit) = crate::tables::measure_column(&header).unwrap_or((header.clone(), String::new()));
        let n: String = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let u = unit.to_lowercase();
        let hint = match n.as_str() {
            "pixelspacing" | "pixelsize" | "spacing" | "mmperpixel" | "mmpixel" | "resolution" => match u.as_str() {
                "mm" | "mm/px" | "mm/pixel" | "" => Some(("mm", v)),
                "cm" | "cm/px" | "cm/pixel" => Some(("mm", v * 10.0)),
                "um" | "µm" => Some(("mm", v / 1000.0)),
                _ => None,
            },
            "fps" | "framerate" | "framespersecond" => match u.as_str() {
                "" | "fps" | "hz" | "/s" | "1/s" => Some(("fps", v)),
                _ => None,
            },
            _ => None,
        };
        if let Some((k, v)) = hint {
            out.entry(k.to_string()).or_insert_with(|| format!("{v}"));
        }
    }
    Ok(out)
}

/// The viewer of a category's files, if one shows that kind of file.
pub fn viewer_for(category: &str, kind: &str) -> Option<&'static str> {
    match (category, kind) {
        ("ecg", "wfdb" | "edf" | "json" | "xml" | "dicom" | "image" | "pdf") => Some("ecg"),
        ("echo" | "cath" | "img", "video" | "dicom" | "folder" | "image" | "pdf") => Some("pacs"),
        _ => None,
    }
}

/// The path of an item's attached file or folder, if it may be read: the patient's, and inside
/// the data roots.
pub fn item_file(c: &Connection, cfg: &Config, patient: i64, item: i64) -> anyhow::Result<PathBuf> {
    let path: String = c
        .query_row(
            &format!("SELECT file_path FROM ehr_items WHERE id = ?1 AND patient = ?2 AND {}", super::item_shown("")),
            [item, patient],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no such file"))?;
    let p = PathBuf::from(&path);
    if path.is_empty() || !p.exists() || !cfg.import_allowed(&p) {
        anyhow::bail!("no such file");
    }
    Ok(p)
}

pub(super) fn mime_of(p: &Path) -> Option<&'static str> {
    Some(match p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("txt" | "hea") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("dcm" | "dicom") => "application/dicom",
        Some("xml") => "application/xml",
        Some("dat" | "edf" | "bdf") => "application/octet-stream",
        // DICOM files often have no extension
        None => "application/octet-stream",
        _ => return None,
    })
}

/// An item's attached file, or a folder's listing.
pub enum Attached {
    File(PathBuf, &'static str),
    Listing(Vec<String>),
}

pub fn attachment(c: &Connection, cfg: &Config, patient: i64, item: i64) -> anyhow::Result<Attached> {
    let p = item_file(c, cfg, patient, item)?;
    if p.is_dir() {
        return Ok(Attached::Listing(folder_files(&p)));
    }
    let mime = mime_of(&p).ok_or_else(|| anyhow::anyhow!("this kind of file is not shown"))?;
    Ok(Attached::File(p, mime))
}

/// The files of an item's attachment, in the order Privacy Mode numbers them (gate.rs
/// `file_token`): a folder's files; else the file itself, then a WFDB header's signal files.
pub fn attachment_names(c: &Connection, cfg: &Config, patient: i64, item: i64) -> anyhow::Result<Vec<String>> {
    let p = item_file(c, cfg, patient, item)?;
    if p.is_dir() {
        return Ok(folder_files(&p));
    }
    let mut names = vec![p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()];
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hea")) {
        for n in wfdb_files(&p) {
            if !names.contains(&n) {
                names.push(n); // signals often share one file
            }
        }
    }
    Ok(names)
}

/// The files of a folder (not its subfolders, not hidden files, not symbolic links, which may
/// point anywhere), sorted.
pub(super) fn folder_files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.file_type().is_file()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

/// A file that belongs to an item's attachment, by name: the file itself; a WFDB header's
/// signal files (those it names, in its folder); a file of an attached folder.  Nothing else
/// of the folder it lies in, which may hold other patients' files.
pub fn attachment_part(
    c: &Connection,
    cfg: &Config,
    patient: i64,
    item: i64,
    name: &str,
) -> anyhow::Result<(PathBuf, &'static str)> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        anyhow::bail!("no such file");
    }
    let p = item_file(c, cfg, patient, item)?;
    let allowed = if p.is_dir() {
        folder_files(&p).iter().any(|n| n == name)
    } else if p.file_name().is_some_and(|n| n.to_string_lossy() == name) {
        true
    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hea")) {
        wfdb_files(&p).iter().any(|n| n == name)
    } else {
        false
    };
    let file = if p.is_dir() { p.join(name) } else { p.with_file_name(name) };
    if !allowed || !file.is_file() || !cfg.import_allowed(&file) {
        anyhow::bail!("no such file");
    }
    // a part lies, really, in the attachment's folder (a symbolic link may point anywhere)
    let folder = if p.is_dir() { Some(p.as_path()) } else { p.parent() };
    let at_home =
        file == p || folder.is_some_and(|d| crate::config::real(&file).parent() == Some(&*crate::config::real(d)));
    if !at_home {
        anyhow::bail!("no such file");
    }
    let mime = mime_of(&file).ok_or_else(|| anyhow::anyhow!("this kind of file is not shown"))?;
    Ok((file, mime))
}

/// The signal files the WFDB headers of an attachment name: its header's, or those of the
/// headers in an attached folder.  Privacy Mode sends a `.dat` file only when one names it.
pub fn signal_names(p: &Path) -> Vec<String> {
    if p.is_dir() {
        let mut v: Vec<String> = folder_files(p)
            .iter()
            .filter(|n| Path::new(n).extension().is_some_and(|e| e.eq_ignore_ascii_case("hea")))
            .flat_map(|n| wfdb_files(&p.join(n)))
            .collect();
        v.sort();
        v.dedup();
        v
    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hea")) {
        wfdb_files(p)
    } else {
        vec![]
    }
}

/// The signal files a WFDB header names (plain names, in its folder).
pub(super) fn wfdb_files(hea: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(hea) else { return vec![] };
    let mut lines = text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty());
    let n: usize = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse().ok()).unwrap_or(0);
    lines
        .take(n)
        .filter_map(|l| l.split_whitespace().next())
        .filter(|f| !f.contains(['/', '\\']) && *f != "-")
        .map(String::from)
        .collect()
}

/// A file name in a URL path.
pub(super) fn encode_name(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
