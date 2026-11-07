//! A table to import (CSV, Parquet, or a spreadsheet: Excel, OpenDocument): read, its columns
//! guessed, inspected with and without the built-in formats.  Never a panic, whatever the bytes.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(init: {
    // Medorama catches the panics of the parquet crate on damaged files (parquet_rows.rs): the
    // standard hook, not the fuzzer's (which aborts at any panic, even one caught); a panic
    // Medorama does not catch still ends the run, and is reported
    let _ = std::panic::take_hook();
}, |data: &[u8]| {
    if let Ok((headers, _rows)) = medorama::tables::read_rows(data) {
        let _ = medorama::tables::guess_mapping(&headers);
        let _ = medorama::tables::guess_category("t.csv", &headers);
    }
    // the built-in formats (GE MUSE, CLIF) recognise and convert what they can
    let cfg = medorama::config::Config::new(std::path::Path::new("/nonexistent"), std::path::Path::new("/nonexistent"));
    let _ = medorama::tables::inspect(&cfg, "t.csv", data, true, &medorama::formats::Choice::Detect);
    let _ = medorama::tables::inspect(&cfg, "t.xlsx", data, false, &medorama::formats::Choice::None);
});
