//! A GE MUSE XML ECG as Privacy Mode rewrites it: demographics removed, staff replaced, dates moved.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = medorama::deid::muse_xml(&text, &medorama::deid::Settings::default());
});
