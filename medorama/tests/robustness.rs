//! Every function that reads what users type or import must answer, never panic: random and
//! malformed input (truncated quotes, stray operators, multi-byte text, garbage files) thrown at
//! the parsers of searches, criteria, dates, numbers, tables and the de-identified view.

use medorama::cohorts::Criterion;
use medorama::{categories, deid, ehr, expr, linkage, patient, search, tables, util, xlsx};

/// A small deterministic generator (no dependency): xorshift.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

#[rustfmt::skip]
const PIECES: &[&str] = &[
    "age", ">", "<", ">=", "=", "!=", "18", "-3.5", "1e3", "400ms", "0.4 s", "mV", "µV", "AND", "OR", "NOT",
    "and", "&&", "||", "!", "(", ")", "\"", "'", "“", "female", "male", "has", "icu", "ecg", "dx", "I21",
    "medication", "text", "in", "latest", "QT", "ecg_index.QT", "db.", "\"R axis\"", "contains", "~", "name:",
    "dob:1950", "mrn:", ":", "é", "日本", "🙂", "\\", "%", "_", ".", "2024-01-02", "2024-13-45", "01/02/2024",
    // (a bedside-monitor recording name comes from joining "ICU7_12" and "-1700000000")
    "12:00", "T", "-1700000000", "ICU7_12", " ", "\t", "\n", "",
];

fn garbage(r: &mut Rng, n: usize) -> String {
    (0..n).map(|_| r.pick(PIECES)).collect::<Vec<_>>().join(if r.next().is_multiple_of(3) { "" } else { " " })
}

#[test]
fn parsers_never_panic() {
    let mut r = Rng(0x9e3779b97f4a7c15);
    let mut resolve = |q: Option<&str>, n: &str| -> anyhow::Result<Criterion> {
        if q == Some("db") || n.is_empty() {
            anyhow::bail!("no such database");
        }
        Ok(Criterion { kind: "column".into(), name: n.into(), import: Some(1), ..Default::default() })
    };
    let zone = util::tz("America/New_York");
    for i in 0..20_000 {
        let s = garbage(&mut r, 1 + i % 12);
        let _ = expr::parse(&s, &mut resolve);
        let terms = search::parse(&s);
        let _ = search::check(&terms, &["name", "dob", "unit"], i % 2 == 0);
        let _ = util::parse_time(&s, zone);
        let _ = util::parse_date(&s);
        let _ = ehr::number(&s);
        let _ = patient::age(&s);
        let _ = categories::named(&s);
        let _ = tables::measure_column(&s);
        let _ = linkage::read_mapping(&s);
        let _ = deid::shift_text(&s, (r.next() % 4_000_000_000) as i64 - 2_000_000_000);
        let _ = expr::convert(1.0, &s, "ms");
    }
    // extreme times and zones
    for t in [f64::NAN, f64::INFINITY, -1e18, 1e18, 0.0, -0.5] {
        let _ = util::fmt_local(t, zone);
    }
    let _ = util::tz("Not/AZone");
}

#[test]
fn tables_never_panic() {
    let mut r = Rng(42);
    let samples: Vec<Vec<u8>> = vec![
        vec![],
        b"\xff\xfe\x00garbage".to_vec(),
        b"a,b\n\"unclosed,1\n".to_vec(),
        b"patient_id;time\nP1;2024-01-02\n".to_vec(),
        b"PK\x03\x04 not really a zip".to_vec(),
        "mrn,time,QT (ms),note\nÉ1,2024-01-02,410,\"a\nb\"\n".as_bytes().to_vec(),
        xlsx::write(&[xlsx::Sheet {
            name: "S".into(),
            rows: vec![vec!["patient_id".into(), "QT (ms)".into()], vec!["P1".into(), "412".into()]],
        }]),
    ];
    for s in &samples {
        let _ = tables::read_rows(s);
        let _ = tables::guess_mapping(&tables::read_rows(s).map(|x| x.0).unwrap_or_default());
    }
    for _ in 0..500 {
        let n = (r.next() % 300) as usize;
        let bytes: Vec<u8> = (0..n).map(|_| (r.next() % 256) as u8).collect();
        let _ = tables::read_rows(&bytes);
        // a zip's header then garbage: the Excel reader must give up cleanly
        let mut z = b"PK\x03\x04".to_vec();
        z.extend(&bytes);
        let _ = tables::read_rows(&z);
    }
}

/// A damaged Parquet file (one the fuzzer found to panic the parquet crate): an error, not a
/// panic, whether read whole (an upload) or row by row (a command).
#[test]
fn a_damaged_parquet_file_is_an_error() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/damaged.parquet");
    let data = std::fs::read(path).unwrap();
    let e = tables::read_rows(&data).unwrap_err().to_string();
    assert!(e.contains("damaged") || e.to_lowercase().contains("parquet"), "{e}");
    // its columns may be readable (the damage is in its rows): an answer either way, no panic
    let part = tables::Part { name: "damaged.parquet".into(), data: tables::PartData::File(path.into()) };
    let _ = tables::columns_of(&part);
}

/// A truncated or damaged old-Excel (`.xls`, an OLE2 compound file) file: an error, never a
/// panic or an allocation without bound (RUSTSEC-2026-0436, fixed in calamine 0.38).
#[test]
fn a_damaged_xls_file_is_an_error() {
    // the compound file header: its signature, then sector counts and positions out of range
    let mut cfb = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    cfb.extend([0u8; 16]);
    cfb.extend([0x3E, 0x00, 0x03, 0x00, 0xFE, 0xFF, 0x09, 0x00, 0x06, 0x00]);
    cfb.extend([0u8; 6]);
    cfb.extend([0xFF, 0xFF, 0xFF, 0x7F]); // directory sectors: absurd
    cfb.extend([0xFF, 0xFF, 0xFF, 0x7F]); // FAT sectors: absurd
    cfb.extend([0x01, 0, 0, 0]);
    cfb.resize(600, 0xFF);
    for n in [8, 64, 76, 512, 600] {
        let e = tables::read_rows(&cfb[..n]);
        assert!(e.is_err(), "{n} bytes");
    }
}

/// Text files that are not comma-separated UTF-8 are refused with what they are.
#[test]
fn text_files_say_what_is_wrong_with_them() {
    let err = |b: &[u8]| tables::read_rows(b).unwrap_err().to_string();
    assert!(err(b"patient_id;time\nP1;2024-01-02\n").contains("semicolons"));
    assert!(err(b"patient_id\ttime\nP1\t2024-01-02\n").contains("tabs"));
    let utf16: Vec<u8> =
        [0xFF, 0xFE].into_iter().chain("a,b\n1,2\n".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
    assert!(err(&utf16).contains("UTF-16"));
    let latin1 = b"name,city\nJos\xe9,Montr\xe9al\n";
    let e = err(latin1);
    assert!(e.contains("not UTF-8") && e.contains("Latin-1") && e.contains("line 2"), "{e}");
    // a file of one column is no file of semicolons
    assert_eq!(tables::read_rows(b"patient_id\nP1\n").unwrap().1, [vec!["P1".to_string()]]);
}
