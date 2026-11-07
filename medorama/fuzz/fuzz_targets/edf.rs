//! An EDF or BDF file: its header, read from the bytes and from the file alike, and its index
//! (recording-engine), the identifiers of its patient and recording fields (`identify`), and
//! the copy Privacy Mode sends (`deid::edf_header`, then
//! `deid::edf_annotations` on every annotation signal of every data record, as the server's
//! `edf_deidentified` does).  An answer or an error, never a panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use recording_engine::sources::edf;

fuzz_target!(|data: &[u8]| {
    // the engine reads files: the input in a file of this process's own
    let path = std::env::temp_dir().join(format!("medorama-fuzz-edf-{}.edf", std::process::id()));
    std::fs::write(&path, data).expect("a temporary file");
    let s = medorama::deid::Settings::default();

    if let Ok((patient, recording)) = edf::identification(&path) {
        let _ = medorama::identify::from_edf(&patient, &recording);
    }
    // the header rewritten whatever its length; an annotation signal's bytes, whatever they are
    let _ = medorama::deid::edf_header(data, &s);
    let _ = medorama::deid::edf_header(data, &s.for_draw(Some(0.5)));
    medorama::deid::edf_annotations(&mut data.to_vec());

    // the header read from the bytes is the one read from the file
    let from_bytes = edf::header_from_bytes(data);
    let from_file = edf::header(&path);
    assert_eq!(from_bytes.is_ok(), from_file.is_ok(), "header_from_bytes and header disagree");
    if let (Ok(a), Ok(b)) = (&from_bytes, &from_file) {
        assert_eq!((a.records, a.signals.len()), (b.records, b.signals.len()), "header_from_bytes and header disagree");
    }

    if let Ok(h) = from_file {
        let head_len = 256 * (h.signals.len() + 1);
        let width = if h.bdf { 3 } else { 2 };
        let record: usize = h.signals.iter().map(|x| x.spr * width).sum();
        assert!(head_len + h.records as usize * record <= data.len(), "records past the end");
        let mut all = data[..head_len + h.records as usize * record].to_vec();
        let head = medorama::deid::edf_header(&all[..head_len], &s);
        assert_eq!(head.len(), head_len, "the rewritten header's length");
        all[..head_len].copy_from_slice(&head);
        for r in 0..h.records as usize {
            let mut at = head_len + r * record;
            for x in &h.signals {
                let n = x.spr * width;
                if x.annotations {
                    medorama::deid::edf_annotations(&mut all[at..at + n]);
                }
                at += n;
            }
        }
        let _ = edf::format_label(&path);
        let _ = edf::build(&path, &|_| {}, &|| false);
    }
    let _ = std::fs::remove_file(&path);
});
