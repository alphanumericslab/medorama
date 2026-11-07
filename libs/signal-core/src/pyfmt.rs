//! Text formatting identical to the Python implementation's, so that the converters write
//! the same bytes: `repr(float)`, `'%.Ng' % x`, `round(x, n)`, `f'{x:.Nf}'`, the shortest
//! float32 text, UTC ISO times and CSV rows (Python's `csv` module, excel dialect).

/// Python's `'%.{precision}g' % x`.
pub fn g(x: f64, precision: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    c_g(x, precision)
}

/// C's `printf("%.{precision}g", x)`.
fn c_g(x: f64, precision: usize) -> String {
    if x.is_nan() {
        return if x.is_sign_negative() { "-nan".into() } else { "nan".into() };
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let p = precision.max(1);
    // Exponent after rounding to p significant digits.
    let e_text = format!("{:.*e}", p - 1, x);
    let (mant, exp) = e_text.split_once('e').expect("LowerExp has an exponent");
    let exp: i32 = exp.parse().expect("integer exponent");
    if exp < -4 || exp >= p as i32 {
        let mant = strip_zeros(mant);
        format!("{}e{}{:02}", mant, if exp < 0 { '-' } else { '+' }, exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{:.*}", decimals, x)).to_string()
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// Shortest round-trip text of `x`, in the style of Python's `repr(float)` (fixed notation
/// with at least one decimal for exponents -4..16, else `1e-05`, `1.5e+16`).
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let (neg, digits, exp) = shortest_digits(x);
    let decpt = exp + 1; // value = 0.d1d2... x 10^decpt
    let mut out = String::with_capacity(digits.len() + 8);
    if neg {
        out.push('-');
    }
    if -4 < decpt && decpt <= 16 {
        let n = digits.len() as i32;
        if decpt <= 0 {
            out.push_str("0.");
            for _ in 0..-decpt {
                out.push('0');
            }
            out.push_str(&digits);
        } else if decpt >= n {
            out.push_str(&digits);
            for _ in 0..decpt - n {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..decpt as usize]);
            out.push('.');
            out.push_str(&digits[decpt as usize..]);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", e.abs()));
    }
    out
}

/// (negative, significant digits without trailing zeros, decimal exponent of the first digit).
pub fn shortest_digits(x: f64) -> (bool, String, i32) {
    let mut s = format!("{:e}", x); // shortest round-trip digits, e.g. "-4.098360655737705e2"
                                    // Of the strings with that many digits that read back as x, use the correctly
                                    // rounded one (ties to even), as Python and C do; the shortest-digits algorithm may
                                    // round a tie in the last digit up.
    let n = s.trim_start_matches('-').split('e').next().map_or(1, |m| m.chars().filter(char::is_ascii_digit).count());
    let rounded = format!("{:.*e}", n.saturating_sub(1), x);
    if rounded.parse::<f64>().ok() == Some(x) {
        s = rounded;
    }
    let (mant, exp) = s.split_once('e').expect("LowerExp output has an exponent");
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    (neg, digits.to_string(), exp.parse().expect("integer exponent"))
}

/// `x` rounded to `ndigits` decimals, correctly rounded from its exact binary value.
pub fn round_to(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{:.*}", ndigits, x).parse().unwrap_or(x)
}

/// Python's `f'{x:.{decimals}f}'`.
pub fn fixed(x: f64, decimals: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    format!("{:.*}", decimals, x)
}

/// Shortest text (6 to 9 significant digits) that reads back as the same float32 (the
/// Python XML writer's `_f32`).
pub fn f32_short(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "INF".into() } else { "-INF".into() };
    }
    let target = (x as f32).to_bits();
    for p in 6..10 {
        let s = g(x, p);
        if let Ok(v) = s.parse::<f64>() {
            if (v as f32).to_bits() == target {
                return s;
            }
        }
    }
    repr(x)
}

/// `datetime.fromtimestamp(t, timezone.utc).isoformat()`: microsecond resolution (half to
/// even), `+00:00` suffix.
pub fn utc_iso(t: f64) -> String {
    let mut secs = t.trunc();
    let mut us = ((t - secs) * 1e6).round_ties_even();
    if us >= 1e6 {
        us -= 1e6;
        secs += 1.0;
    } else if us < 0.0 {
        us += 1e6;
        secs -= 1.0;
    }
    let secs = secs as i64;
    let d = date_from_days(secs.div_euclid(86400));
    let sod = secs.rem_euclid(86400);
    let mut out =
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", d.year, d.month, d.day, sod / 3600, sod / 60 % 60, sod % 60);
    if us != 0.0 {
        out.push_str(&format!(".{:06}", us as i64));
    }
    out.push_str("+00:00");
    out
}

/// One CSV field as Python's `csv` writer quotes it (QUOTE_MINIMAL).
pub fn csv_field(s: &str, out: &mut String) {
    if s.contains([',', '"', '\r', '\n']) {
        out.push('"');
        out.push_str(&s.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(s);
    }
}

/// A CSV row (fields already converted to text), with Python's `\r\n` line end.
pub fn csv_row<S: AsRef<str>>(fields: &[S]) -> String {
    let mut out = String::new();
    if fields.len() == 1 && fields[0].as_ref().is_empty() {
        return "\"\"\r\n".into();
    }
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        csv_field(f.as_ref(), &mut out);
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_python() {
        assert_eq!(g(87.0, 10), "87");
        assert_eq!(g(-0.3, 10), "-0.3");
        assert_eq!(g(1100000000.375, 15), "1100000000.375");
        assert_eq!(fixed(1100000000.04999, 1), "1100000000.0");
        assert_eq!(fixed(0.25, 1), "0.2"); // half to even on exact ties, as Python
        assert_eq!(f32_short(98.19999694824219), "98.2");
        assert_eq!(utc_iso(1100000000.0), "2004-11-09T11:33:20+00:00");
        assert_eq!(utc_iso(1100008800.375), "2004-11-09T14:00:00.375000+00:00");
        assert_eq!(csv_row(&["a", "b,c", "d\"e", ""]), "a,\"b,c\",\"d\"\"e\",\r\n");
        assert_eq!(round_to(2.0000004999, 6), 2.0);
    }
}

/// A date of the proleptic Gregorian calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// The date `days` days after 1970-01-01.
pub fn date_from_days(days: i64) -> CivilDate {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + (month <= 2) as i64) as i32;
    CivilDate { year, month, day }
}

/// Days from 1970-01-01 to a date.
pub fn days_from_date(year: i32, month: u32, day: u32) -> i64 {
    let (y, m, d) = (year as i64, month as i64, day as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
