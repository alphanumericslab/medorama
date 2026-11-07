//! A DICOM file's header as Privacy Mode rewrites it (PS3.15 Basic Profile): an answer or an
//! error, never a panic or a read past the end.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = |s: &str| s.replace("Test", "[Patient Name]");
    let r = medorama::dicom::Rewrite {
        pseudonym: "MO-TEST-0000",
        offset_s: 86_400,
        cap_ages: true,
        uid_key: &[7; 32],
        text: &text,
    };
    if let Ok(w) = medorama::dicom::deidentify_bytes(data, &r) {
        assert!(w.rest_from as usize <= data.len(), "the rest begins inside the file");
    }
});
