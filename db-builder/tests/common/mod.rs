//! Helpers shared by the tests: invented tables, spreadsheets, Parquet files and recordings
//! (no real patient's data anywhere), and the database they make.
#![allow(dead_code)]

use db_builder::add::{Options, SourceReport};
use db_builder::db;
use rusqlite::Connection;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A folder for a test, and its database's path in it (`out.sqlite`).
pub struct Fx {
    pub dir: tempfile::TempDir,
}

impl Fx {
    pub fn new() -> Fx {
        Fx { dir: tempfile::tempdir().unwrap() }
    }
    pub fn root(&self) -> &Path {
        self.dir.path()
    }
    pub fn db(&self) -> PathBuf {
        self.root().join("out.sqlite")
    }
    /// A file of the test (its folders made).
    pub fn write(&self, rel: &str, data: impl AsRef<[u8]>) -> PathBuf {
        let p = self.root().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, data).unwrap();
        p
    }
    /// The database, made in `timezone` when it does not exist.
    pub fn open(&self, timezone: &str) -> Connection {
        db::open_or_create(&self.db(), &db::Opening::default(), Some(timezone)).unwrap()
    }
    /// `db-builder add`, with these options.
    pub fn add(&self, inputs: &[PathBuf], opts: &Options) -> Vec<SourceReport> {
        let mut c = self.open("UTC");
        db_builder::add::add(&mut c, &self.db(), inputs, opts).unwrap()
    }
}

pub fn opts(issuer: &str) -> Options {
    Options { issuer: issuer.into(), mrn_authority: "HOSP".into(), ..Default::default() }
}

/// The rows of a query, each column as text (NULL as `NULL`).
pub fn rows(c: &Connection, sql: &str) -> Vec<Vec<String>> {
    let mut st = c.prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| match r.get_ref(i).unwrap() {
                rusqlite::types::ValueRef::Null => "NULL".to_string(),
                rusqlite::types::ValueRef::Integer(x) => x.to_string(),
                rusqlite::types::ValueRef::Real(x) => x.to_string(),
                rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).to_string(),
                rusqlite::types::ValueRef::Blob(_) => "BLOB".to_string(),
            })
            .collect())
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

pub fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 2024-01-01 12:00:00 (as a recording header's clock time)
pub const T0: i64 = 1_704_110_400;

/// A WFDB record `name` (one ECG lead, 250 Hz, 20 s) in folder `dir`, its header's clock at
/// T0 + `offset_s`.
pub fn wfdb_record(dir: &Path, name: &str, offset_s: i64) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let t = chrono::DateTime::from_timestamp(T0 + offset_s, 0).unwrap();
    let n = 5000;
    let samples: Vec<i16> = (0..n).map(|i| (100.0 * (i as f64 / 25.0).sin()) as i16).collect();
    // WFDB's checksum: the 16-bit sum of the samples (WFDB readers check it)
    let checksum = samples.iter().fold(0i16, |a, &x| a.wrapping_add(x));
    let hea = dir.join(format!("{name}.hea"));
    std::fs::write(
        &hea,
        format!(
            "{name} 1 250 {n} {}\n{name}.dat 16 200 12 0 {} {checksum} 0 II\n",
            t.format("%H:%M:%S %d/%m/%Y"),
            samples[0]
        ),
    )
    .unwrap();
    let mut b = Vec::with_capacity(n * 2);
    for x in &samples {
        b.extend(x.to_le_bytes());
    }
    std::fs::write(dir.join(format!("{name}.dat")), b).unwrap();
    hea
}

/// An EDF file: two signals, one data record of 1 s, starting 02.01.24 08.10.00 (its header's
/// identification fields hold invented text only).
pub fn edf_file() -> Vec<u8> {
    let field = |t: &str, w: usize| format!("{t:<w$}").into_bytes();
    let signals: &[(&str, &str, usize)] = &[("I", "mV", 500usize), ("II", "mV", 500)];
    let mut h = field("0", 8);
    h.extend(field("X X X X", 80));
    h.extend(field("Startdate X X X X", 80));
    h.extend(field("02.01.24", 8));
    h.extend(field("08.10.00", 8));
    h.extend(field(&(256 * (signals.len() + 1)).to_string(), 8));
    h.extend(field("", 44));
    h.extend(field("1", 8));
    h.extend(field("1", 8));
    h.extend(field(&signals.len().to_string(), 4));
    let each = |h: &mut Vec<u8>, w: usize, f: &dyn Fn(&(&str, &str, usize)) -> String| {
        for s in signals {
            h.extend(field(&f(s), w));
        }
    };
    each(&mut h, 16, &|s| s.0.into());
    each(&mut h, 80, &|_| String::new());
    each(&mut h, 8, &|s| s.1.into());
    each(&mut h, 8, &|_| "-5".into());
    each(&mut h, 8, &|_| "5".into());
    each(&mut h, 8, &|_| "-32768".into());
    each(&mut h, 8, &|_| "32767".into());
    each(&mut h, 80, &|_| String::new());
    each(&mut h, 8, &|s| s.2.to_string());
    each(&mut h, 32, &|_| String::new());
    h.extend(vec![0u8; 2 * 500 * 2]);
    h
}

fn zip_of(files: &[(&str, String)], stored_first: bool) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut out);
        for (i, (name, text)) in files.iter().enumerate() {
            let o = if stored_first && i == 0 {
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
            } else {
                zip::write::SimpleFileOptions::default()
            };
            z.start_file(*name, o).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    out.into_inner()
}

/// A one-sheet Excel spreadsheet of text cells (inline strings).
pub fn xlsx(rows: &[&[&str]]) -> Vec<u8> {
    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (r, row) in rows.iter().enumerate() {
        sheet.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (i, v) in row.iter().enumerate() {
            sheet.push_str(&format!(
                r#"<c r="{}{}" t="inlineStr"><is><t>{v}</t></is></c>"#,
                (b'A' + i as u8) as char,
                r + 1
            ));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    zip_of(
        &[
            ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
            ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
            ("xl/workbook.xml", r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string()),
            ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string()),
            ("xl/worksheets/sheet1.xml", sheet),
        ],
        false,
    )
}

/// A one-sheet OpenDocument spreadsheet of text cells.
pub fn ods(rows: &[&[&str]]) -> Vec<u8> {
    let mut table = String::new();
    for row in rows {
        table.push_str("<table:table-row>");
        for v in *row {
            table.push_str(&format!(
                r#"<table:table-cell office:value-type="string"><text:p>{v}</text:p></table:table-cell>"#
            ));
        }
        table.push_str("</table:table-row>");
    }
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2"><office:body><office:spreadsheet><table:table table:name="S">{table}</table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    zip_of(
        &[
            ("mimetype", "application/vnd.oasis.opendocument.spreadsheet".to_string()),
            ("content.xml", content),
            ("META-INF/manifest.xml", r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#.to_string()),
        ],
        true,
    )
}

/// A Parquet file of text columns, and one timestamp column (`ts`, microseconds, UTC) when
/// `times` is given.
pub fn parquet(cols: &[&str], rows: &[&[&str]], times: Option<&[i64]>) -> Vec<u8> {
    use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
    use parquet::file::writer::SerializedFileWriter;
    let mut schema = String::from("message m {");
    for c in cols {
        schema.push_str(&format!(" required binary {c} (UTF8);"));
    }
    if times.is_some() {
        schema.push_str(" required int64 ts (TIMESTAMP(MICROS,true));");
    }
    schema.push_str(" }");
    let schema = std::sync::Arc::new(parquet::schema::parser::parse_message_type(&schema).unwrap());
    let mut buf = vec![];
    {
        let mut w = SerializedFileWriter::new(&mut buf, schema, Default::default()).unwrap();
        let mut rg = w.next_row_group().unwrap();
        for i in 0..cols.len() {
            let mut col = rg.next_column().unwrap().unwrap();
            let values: Vec<ByteArray> = rows.iter().map(|r| ByteArray::from(r[i])).collect();
            col.typed::<ByteArrayType>().write_batch(&values, None, None).unwrap();
            col.close().unwrap();
        }
        if let Some(t) = times {
            let mut col = rg.next_column().unwrap().unwrap();
            col.typed::<Int64Type>().write_batch(t, None, None).unwrap();
            col.close().unwrap();
        }
        rg.close().unwrap();
        w.close().unwrap();
    }
    buf
}

/// The `db-builder` command, with `DB_BUILDER_KEY` set to `key` (or unset): its exit code, its
/// output and its errors.
pub fn cli(args: &[&str], key: Option<&str>) -> (i32, String, String) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_db-builder"));
    cmd.args(args);
    match key {
        Some(k) => cmd.env(db::KEY_VAR, k),
        None => cmd.env_remove(db::KEY_VAR),
    };
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}
