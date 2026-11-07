//! A minimal Excel (.xlsx) writer: sheets of text and numbers, nothing else (no styles, no
//! formulas).  An xlsx file is a zip of XML parts; this one is stored (not compressed), which
//! every spreadsheet program reads.
//!
//! Excel's limits are kept: a sheet of more than 1,048,576 rows goes on in further sheets
//! (`Members (2)` …, each beginning with the first's header row), and a cell longer than
//! 32,767 characters is cut, ending with a mark that says so (the CSV export keeps it whole).

/// The most rows a sheet has (Excel's), and the most characters in a cell.
pub const MAX_ROWS: usize = 1_048_576;
pub const MAX_CELL: usize = 32_767;

/// One sheet: its name and rows of cells; cells that read as numbers are written as numbers.
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            // characters XML 1.0 does not allow
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

fn col_name(mut i: usize) -> String {
    let mut s = vec![];
    loop {
        s.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

fn is_number(s: &str) -> bool {
    let t = s.trim();
    // leading zeros (IDs, codes) stay text
    !t.is_empty()
        && t == s
        && t.parse::<f64>().is_ok_and(f64::is_finite)
        && !(t.len() > 1 && t.starts_with('0') && !t.starts_with("0."))
        && !t.contains(['e', 'E', 'x', 'X'])
        && t.len() <= 15
}

/// A cell's text, cut to Excel's limit with a mark saying how much is left out.
fn fitted(v: &str) -> std::borrow::Cow<'_, str> {
    let n = v.chars().count();
    if n <= MAX_CELL {
        return v.into();
    }
    // the mark's own length depends on the count it gives: its longest form first
    let mark = |left: usize| format!(" … [cut: {left} more characters]");
    let keep = MAX_CELL - mark(n).chars().count();
    format!("{}{}", v.chars().take(keep).collect::<String>(), mark(n - keep)).into()
}

/// Sheets as Excel takes them: one of more rows than [`MAX_ROWS`] goes on in further ones,
/// each beginning with its header row.
fn split(sheets: &[Sheet]) -> Vec<(&str, Vec<&[Vec<String>]>)> {
    let mut out = vec![];
    for s in sheets {
        if s.rows.len() <= MAX_ROWS {
            out.push((s.name.as_str(), vec![&s.rows[..]]));
            continue;
        }
        let (head, body) = s.rows.split_at(1);
        for part in body.chunks(MAX_ROWS - 1) {
            out.push((s.name.as_str(), vec![head, part]));
        }
    }
    out
}

fn sheet_xml(parts: &[&[Vec<String>]]) -> String {
    let rows = parts.iter().flat_map(|p| p.iter());
    let mut x = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (r, row) in rows.enumerate() {
        x.push_str(&format!("<row r=\"{}\">", r + 1));
        for (c, v) in row.iter().enumerate() {
            if v.is_empty() {
                continue;
            }
            let at = format!("{}{}", col_name(c), r + 1);
            let v = fitted(v);
            let v = v.as_ref();
            if is_number(v) {
                x.push_str(&format!("<c r=\"{at}\"><v>{v}</v></c>"));
            } else {
                x.push_str(&format!(
                    "<c r=\"{at}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    esc(v)
                ));
            }
        }
        x.push_str("</row>");
    }
    x.push_str("</sheetData></worksheet>");
    x
}

/// Sheet names as Excel allows them: at most 31 characters, none of []:*?/\, unique.
fn sheet_names(names: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for s in names {
        let base: String = s.chars().filter(|c| !"[]:*?/\\".contains(*c)).take(31).collect();
        let base = if base.trim().is_empty() { "Sheet".to_string() } else { base };
        let mut name = base.clone();
        let mut k = 2;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            let tail = format!(" ({k})");
            name = format!("{}{tail}", base.chars().take(31 - tail.len()).collect::<String>());
            k += 1;
        }
        out.push(name);
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// A stored zip of (name, bytes).
fn zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![];
    let mut central = vec![];
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let n = name.as_bytes();
        let len = data.len() as u32;
        // local header: version 2.0, UTF-8 names (bit 11), stored, fixed time (1980-01-01)
        let local = |sig: u32, v: &mut Vec<u8>| {
            v.extend(sig.to_le_bytes());
        };
        local(0x0403_4b50, &mut out);
        out.extend(20u16.to_le_bytes());
        out.extend(0x0800u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0x0021u16.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend((n.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(n);
        out.extend(data);
        local(0x0201_4b50, &mut central);
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(0x0800u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0x0021u16.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend((n.len() as u16).to_le_bytes());
        central.extend([0u8; 12]); // extra, comment, disk, internal attributes, external (2 + 2 + 2 + 2 + 4)
        central.extend(offset.to_le_bytes());
        central.extend(n);
    }
    let start = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend(size.to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// The workbook's bytes.
pub fn write(sheets: &[Sheet]) -> Vec<u8> {
    let sheets = split(sheets);
    let names = sheet_names(&sheets.iter().map(|s| s.0).collect::<Vec<_>>());
    let mut files: Vec<(String, Vec<u8>)> = vec![];
    let overrides: String = (1..=sheets.len())
        .map(|i| format!(r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#))
        .collect();
    files.push((
        "[Content_Types].xml".into(),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>{overrides}</Types>"#
        )
        .into_bytes(),
    ));
    files.push((
        "_rels/.rels".into(),
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_vec(),
    ));
    let list: String = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!(r#"<sheet name="{}" sheetId="{}" r:id="rId{}"/>"#, esc(n), i + 1, i + 1))
        .collect();
    files.push((
        "xl/workbook.xml".into(),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{list}</sheets></workbook>"#
        )
        .into_bytes(),
    ));
    let rels: String = (1..=sheets.len())
        .map(|i| format!(r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>"#))
        .collect();
    files.push((
        "xl/_rels/workbook.xml.rels".into(),
        format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#).into_bytes(),
    ));
    for (i, s) in sheets.iter().enumerate() {
        files.push((format!("xl/worksheets/sheet{}.xml", i + 1), sheet_xml(&s.1).into_bytes()));
    }
    zip(&files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_back_with_calamine() {
        use calamine::{Reader, Xlsx};
        let bytes = write(&[
            Sheet {
                name: "Members".into(),
                rows: vec![vec!["patient_id".into(), "QT (ms)".into()], vec!["P0001".into(), "412".into()]],
            },
            Sheet { name: "a/b:c?".into(), rows: vec![vec!["x & <y>".into(), "007".into()]] },
            Sheet { name: "Members".into(), rows: vec![] },
        ]);
        let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(wb.sheet_names(), vec!["Members", "abc", "Members (2)"]);
        let r = wb.worksheet_range("Members").unwrap();
        assert_eq!(r.get_value((1, 0)).unwrap().to_string(), "P0001");
        assert_eq!(r.get_value((1, 1)).unwrap(), &calamine::Data::Float(412.0));
        let r = wb.worksheet_range("abc").unwrap();
        assert_eq!(r.get_value((0, 0)).unwrap().to_string(), "x & <y>");
        assert_eq!(r.get_value((0, 1)).unwrap().to_string(), "007");
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(27), "AB");
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn excels_limits() {
        use calamine::{Reader, Xlsx};
        let long = "x".repeat(MAX_CELL + 1000);
        let cut = fitted(&long);
        assert!(cut.chars().count() <= MAX_CELL && cut.chars().count() > MAX_CELL - 3);
        assert!(cut.ends_with("more characters]"));
        assert_eq!(fitted("short"), "short");
        // a sheet too long for Excel goes on in another, its header first
        let mut rows = vec![vec!["h".to_string()]];
        rows.extend((0..MAX_ROWS + 5).map(|i| vec![i.to_string()]));
        let sheets = [Sheet { name: "Rows".into(), rows }];
        let parts = split(&sheets);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1.iter().map(|p| p.len()).sum::<usize>(), MAX_ROWS);
        assert_eq!((parts[1].1[0][0][0].as_str(), parts[1].1[1].len()), ("h", 6));
        let bytes = write(&[Sheet { name: "Long".into(), rows: vec![vec![long]] }]);
        let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(bytes)).unwrap();
        let r = wb.worksheet_range("Long").unwrap();
        assert!(r.get_value((0, 0)).unwrap().to_string().chars().count() <= MAX_CELL);
    }
}
