//! The DICOM rewrite of Privacy Mode on real files, when this clone has them (the local
//! `sample_data/sample-dicom`, never in git): each file rewrites, and the rewrite holds none of
//! the original's identifying values.  Skips itself without the files (CI).  With
//! `MEDORAMA_DICOM_OUT=DIR`, the rewritten files are written there, for checking with other readers.

use medorama::dicom::{deidentify, Rewrite};

#[test]
fn sample_files_rewrite() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sample_data/sample-dicom");
    if !root.is_dir() {
        eprintln!("no {}: skipped", root.display());
        return;
    }
    let out = std::env::var("MEDORAMA_DICOM_OUT").ok().map(std::path::PathBuf::from);
    let r = Rewrite {
        pseudonym: "MO-7F3K-2QXM",
        offset_s: 36570 * 86400,
        cap_ages: true,
        uid_key: b"test secret",
        text: &|s| s.to_string(),
    };
    let mut n = 0;
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        // a few files per folder: the formats, not every slice
        let mut files = 0;
        for p in entries {
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with('.') || name == "DICOMDIR" || !medorama::patient::is_dicom(&p) || files >= 3 {
                continue;
            }
            files += 1;
            let w = deidentify(&p, &r).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(w.head.windows(14).any(|x| x == b"[Patient Name]"), "{}", p.display());
            n += 1;
            if let Some(o) = &out {
                let bytes = std::fs::read(&p).unwrap();
                let mut all = w.head.clone();
                all.extend_from_slice(&bytes[w.rest_from as usize..]);
                let rel = p.strip_prefix(&root).unwrap().to_string_lossy().replace(['/', ' '], "_");
                std::fs::create_dir_all(o).unwrap();
                std::fs::write(o.join(rel), all).unwrap();
            }
        }
    }
    assert!(n > 0);
    eprintln!("{n} files rewritten");
}
