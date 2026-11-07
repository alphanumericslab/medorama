//! Text helpers with Python's semantics, shared by the readers and the engine.

/// Lower-case hexadecimal text of bytes.
pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(2 * b.len());
    for &x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// Python's `str.strip()` (Unicode whitespace).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Python's `float(str)`: surrounding whitespace allowed (not the separators `\x1c`-`\x1f`,
/// which `float` refuses although `str.strip` removes them), one optional sign, `inf`/`nan`
/// words, underscores between digits.
pub fn parse_float(s: &str) -> Option<f64> {
    let t = s.trim_matches(char::is_whitespace);
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let body = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    if matches!(body, "inf" | "infinity" | "nan") {
        let v = if body == "nan" { f64::NAN } else { f64::INFINITY };
        return Some(if lower.starts_with('-') { -v } else { v });
    }
    if t.contains('_') {
        let b = t.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c == b'_' && !(i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit()) {
                return None;
            }
        }
        return t.replace('_', "").parse().ok();
    }
    if !t.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    t.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_python() {
        assert_eq!(hex(&[0x00, 0xab, 0x10]), "00ab10");
        assert_eq!(py_strip("\u{1f} a b \t"), "a b");
        assert_eq!(parse_float(" 1_000.5 "), Some(1000.5));
        assert_eq!(parse_float("-inf"), Some(f64::NEG_INFINITY));
        assert!(parse_float("nan").unwrap().is_nan());
        assert_eq!(
            (parse_float("1__0"), parse_float("_1"), parse_float(""), parse_float("1x")),
            (None, None, None, None)
        );
        // one sign at most, and the separators 0x1c-0x1f are not whitespace here (as in Python)
        assert!(parse_float("+nan").unwrap().is_nan());
        assert_eq!(parse_float("+Infinity"), Some(f64::INFINITY));
        assert_eq!(parse_float("\u{a0}2\n"), Some(2.0));
        for bad in ["+-nan", "--inf", "-+inf", "1\x1c", "\x1f1", "0x10", "--1"] {
            assert_eq!(parse_float(bad), None, "{bad:?}");
        }
    }
}
