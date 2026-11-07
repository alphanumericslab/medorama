//! A WFDB header (`.hea`) as Privacy Mode rewrites it: identifying comments removed, dates moved.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let s = medorama::deid::Settings::default();
    let _ = medorama::deid::wfdb_header(&text, &s);
    let _ = medorama::deid::wfdb_header(&text, &s.for_draw(Some(0.5)));
});
