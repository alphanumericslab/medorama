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

/// The viewer of a category's files, if one shows that kind of file.
pub fn viewer_for(category: &str, kind: &str) -> Option<&'static str> {
    match (category, kind) {
        ("ecg", "wfdb" | "edf" | "json" | "xml" | "dicom" | "image" | "pdf") => Some("ecg"),
        ("echo" | "cath" | "img", "video" | "dicom" | "folder" | "image" | "pdf") => Some("pacs"),
        _ => None,
    }
}

/// The path of a recording's file or folder, if it may be read: a recording of the patient's,
/// in a database `user` may view, present, inside the data roots.
pub fn item_file(c: &Connection, cfg: &Config, user: &User, patient: i64, recording: i64) -> anyhow::Result<PathBuf> {
    let r = crate::recordings::get(cfg, c, recording)?.ok_or_else(|| anyhow::anyhow!("no such file"))?;
    if r.patient != Some(patient) || !r.present || !crate::grants::can(c, user, &format!("db:{}", r.database), "view")?
    {
        anyhow::bail!("no such file");
    }
    let p = PathBuf::from(&r.path);
    if !p.exists() || !cfg.in_data_roots(&p) {
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
        Some("dat" | "mat" | "edf" | "bdf") => "application/octet-stream",
        // DICOM files often have no extension
        None => "application/octet-stream",
        _ => return None,
    })
}

/// A recording's file, or a folder's listing (the viewers of ECGs and images read them).
pub enum Attached {
    File(PathBuf, &'static str),
    Listing(Vec<String>),
}

pub fn attachment(c: &Connection, cfg: &Config, user: &User, patient: i64, item: i64) -> anyhow::Result<Attached> {
    let p = item_file(c, cfg, user, patient, item)?;
    if p.is_dir() {
        return Ok(Attached::Listing(folder_files(&p)));
    }
    let mime = mime_of(&p).ok_or_else(|| anyhow::anyhow!("this kind of file is not shown"))?;
    Ok(Attached::File(p, mime))
}

/// The files of an item's attachment, in the order Privacy Mode numbers them (gate.rs
/// `file_token`): a folder's files; else the file itself, then a WFDB header's signal files.
pub fn attachment_names(
    c: &Connection,
    cfg: &Config,
    user: &User,
    patient: i64,
    item: i64,
) -> anyhow::Result<Vec<String>> {
    let p = item_file(c, cfg, user, patient, item)?;
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
    user: &User,
    patient: i64,
    item: i64,
    name: &str,
) -> anyhow::Result<(PathBuf, &'static str)> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        anyhow::bail!("no such file");
    }
    let p = item_file(c, cfg, user, patient, item)?;
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
    if !allowed || !file.is_file() || !cfg.in_data_roots(&file) {
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
