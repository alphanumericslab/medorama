//! Small helpers: times in CSV files, a reproducible random number generator.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use std::str::FromStr;

pub fn tz(name: &str) -> Tz {
    Tz::from_str(name).unwrap_or(chrono_tz::UTC)
}

/// The earliest and latest times a cell may give (1700-01-01, 2500-01-01 UTC): anything
/// outside is a mistake (a year of two digits, milliseconds read as seconds).  The range is wide
/// for research datasets whose dates are shifted (MIMIC-IV's run past 2200, MIMIC-III's dates of
/// birth of patients over 89 lie in the 1800s).
const EARLIEST: f64 = -8_520_336_000.0;
const LATEST: f64 = 16_725_225_600.0;

/// How a date written with slashes (`03/04/2024`) is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOrder {
    /// Month/day/year (the United States' exports): the default.
    MonthFirst,
    /// Day/month/year: when Medorama writes its dates so (Admin › Dates and times, `dmy`).
    DayFirst,
}

thread_local! {
    static DATE_ORDER: std::cell::Cell<DateOrder> = const { std::cell::Cell::new(DateOrder::MonthFirst) };
}

/// `f`, with slash dates read in `order` on this thread (an import reads its table so).
pub fn with_date_order<T>(order: DateOrder, f: impl FnOnce() -> T) -> T {
    struct Back(DateOrder);
    impl Drop for Back {
        fn drop(&mut self) {
            DATE_ORDER.with(|o| o.set(self.0));
        }
    }
    let _back = Back(DATE_ORDER.with(|o| o.replace(order)));
    f()
}

/// The order slash dates are read in for Medorama: day first when its dates are shown
/// day first (`dmy`, `dmy_dot`), else month first.
pub fn date_order_of(display_date: &str) -> DateOrder {
    if display_date.starts_with("dmy") {
        DateOrder::DayFirst
    } else {
        DateOrder::MonthFirst
    }
}

/// The year of a date as written: its four digits, or an error (`03/04/24` is no date: which
/// century?).
fn four_digit_year(t: &str, year: i32) -> Result<(), String> {
    if (1000..=9999).contains(&year) && t.contains(&year.to_string()) {
        Ok(())
    } else {
        Err(format!("not a time: {t:?} (write the year with its four digits)"))
    }
}

/// A time whose offset is written `-05` or `-0500` (PostgreSQL's text of a `timestamp with
/// time zone`), with the offset as `-05:00`; anything else as it is.
fn colon_offset(t: &str) -> String {
    // the offset's sign comes after the time's clock (`07:10:00-05`), never in the date
    if let Some(p) = t.rfind(['+', '-']).filter(|&p| p > 10 && t[..p].contains(':')) {
        let (head, off) = (&t[..p + 1], &t[p + 1..]);
        if head[..p].ends_with(|c: char| c.is_ascii_digit()) && off.bytes().all(|b| b.is_ascii_digit()) {
            match off.len() {
                2 => return format!("{head}{off}:00"),
                4 => return format!("{head}{}:{}", &off[..2], &off[2..]),
                _ => {}
            }
        }
    }
    t.to_string()
}

/// A time from a CSV cell, as Unix seconds: an ISO 8601 / RFC 3339 time with an offset
/// (`2024-03-02T07:10:00-05:00`, `...Z`, `2024-03-02 12:10:00+00:00`), a local time without one
/// (`2024-03-02 07:10[:00]`, in `zone`), a date (`2024-03-02`, local midnight; `03/02/2024`,
/// month first unless [`with_date_order`] says day first; `02.03.2024`, day first) or a number
/// of Unix seconds (above 10^8 only: smaller numbers are not times; above 10^11 refused: a
/// number of milliseconds).  Years have four digits, and a time is between 1700 and 2500.
/// An offset of hours alone or without its colon (`-05`, `+0530`, as PostgreSQL writes a
/// `timestamp with time zone` as text) is read as `-05:00`, `+05:30`.
/// Empty -> Ok(None).
pub fn parse_time(text: &str, zone: Tz) -> Result<Option<f64>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let in_range = |x: f64| -> Result<Option<f64>, String> {
        if (EARLIEST..LATEST).contains(&x) {
            Ok(Some(x))
        } else {
            Err(format!("not a time: {t:?} (before 1700 or after 2500)"))
        }
    };
    if let Ok(x) = t.parse::<f64>() {
        if x.is_finite() && x >= 1e11 {
            return Err(format!("{t} is too large for Unix seconds: if it counts milliseconds, divide it by 1000"));
        }
        if x.is_finite() && x > 1e8 {
            return Ok(Some(x));
        }
    }
    let colon = colon_offset(t);
    let t = colon.as_str();
    if let Ok(d) = DateTime::parse_from_rfc3339(t) {
        four_digit_year(t, chrono::Datelike::year(&d))?;
        return in_range(d.timestamp_micros() as f64 / 1e6);
    }
    if let Ok(d) = DateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f%:z") {
        four_digit_year(t, chrono::Datelike::year(&d))?;
        return in_range(d.timestamp_micros() as f64 / 1e6);
    }
    let slash = match DATE_ORDER.with(|o| o.get()) {
        DateOrder::MonthFirst => "%m/%d/%Y",
        DateOrder::DayFirst => "%d/%m/%Y",
    };
    let with_time = |f: &str| [format!("{f} %H:%M:%S"), format!("{f} %H:%M")];
    let mut forms: Vec<String> = ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"]
        .iter()
        .map(|f| f.to_string())
        .collect();
    forms.extend(with_time(slash));
    forms.extend(with_time("%d.%m.%Y"));
    let naive = forms.iter().find_map(|f| NaiveDateTime::parse_from_str(t, f).ok()).or_else(|| {
        ["%Y-%m-%d", slash, "%d.%m.%Y"]
            .iter()
            .find_map(|f| NaiveDate::parse_from_str(t, f).ok())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    });
    match naive {
        Some(n) => {
            four_digit_year(t, chrono::Datelike::year(&n))?;
            match zone.from_local_datetime(&n).earliest() {
                Some(d) => in_range(d.timestamp_micros() as f64 / 1e6),
                // a local time skipped by a daylight-saving change: read with the offset after the
                // change (02:30 on 2024-03-10 in New York is the moment shown as 01:30)
                None => match zone.from_local_datetime(&(n + chrono::Duration::hours(1))).earliest() {
                    Some(d) => in_range(d.timestamp_micros() as f64 / 1e6 - 3600.0),
                    None => Ok(None),
                },
            }
        }
        None => Err(format!("not a time: {t:?}")),
    }
}

/// A date of birth as 'YYYY-MM-DD' (also accepts MM/DD/YYYY, or DD/MM/YYYY as
/// [`with_date_order`] says, DD.MM.YYYY and YYYYMMDD): a year of four digits, from 1700 to
/// today.
pub fn parse_date(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    let slash = match DATE_ORDER.with(|o| o.get()) {
        DateOrder::MonthFirst => "%m/%d/%Y",
        DateOrder::DayFirst => "%d/%m/%Y",
    };
    let d = ["%Y-%m-%d", slash, "%d.%m.%Y", "%Y%m%d"]
        .iter()
        .find_map(|f| NaiveDate::parse_from_str(t, f).ok())
        .ok_or_else(|| format!("not a date: {t:?}"))?;
    if !(1000..=9999).contains(&chrono::Datelike::year(&d)) || !t.contains(&chrono::Datelike::year(&d).to_string()) {
        return Err(format!("not a date: {t:?} (write the year with its four digits)"));
    }
    let today = chrono::Utc::now().date_naive();
    if chrono::Datelike::year(&d) < 1700 || d > today {
        return Err(format!("not a date of birth: {t:?} (before 1700, or in the future)"));
    }
    Ok(d.format("%Y-%m-%d").to_string())
}

/// `t` (Unix seconds) as local `YYYY-MM-DD HH:MM:SS` in `zone`.
pub fn fmt_local(t: f64, zone: Tz) -> String {
    match DateTime::from_timestamp(t.floor() as i64, 0) {
        Some(d) => d.with_timezone(&zone).format("%Y-%m-%d %H:%M:%S").to_string(),
        None => String::new(),
    }
}

/// xorshift64*: reproducible pseudo-random numbers for synthetic data (not for security).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        let ny = tz("America/New_York");
        assert_eq!(parse_time("2024-03-02T12:00:00Z", ny).unwrap(), Some(1709380800.0));
        assert_eq!(parse_time("2024-03-02 07:00:00", ny).unwrap(), Some(1709380800.0));
        assert_eq!(parse_time("2024-03-02 07:00", ny).unwrap(), Some(1709380800.0));
        assert_eq!(parse_time("2024-03-02T07:00:00-05:00", ny).unwrap(), Some(1709380800.0));
        assert_eq!(parse_time("1709380800", ny).unwrap(), Some(1709380800.0));
        // PostgreSQL's text of a timestamp with time zone: hours alone, or no colon
        assert_eq!(parse_time("2024-03-02 07:00:00-05", ny).unwrap(), Some(1709380800.0));
        assert_eq!(parse_time("2024-03-02 12:30:00+0530", ny).unwrap(), Some(1709362800.0));
        assert_eq!(parse_time("2024-03-02 07:00:00.250-05", ny).unwrap(), Some(1709380800.25));
        assert_eq!(parse_time("2024-03-02T12:00:00+00", ny).unwrap(), Some(1709380800.0));
        // a local time keeps its date's dashes
        assert_eq!(parse_time("2024-03-02 07:00", ny).unwrap(), Some(1709380800.0));
        assert_eq!(parse_time("", ny).unwrap(), None);
        assert!(parse_time("yesterday", ny).is_err());
        assert_eq!(fmt_local(1709380800.0, ny), "2024-03-02 07:00:00");
        assert_eq!(parse_date("03/04/1950").unwrap(), "1950-03-04");
    }

    #[test]
    fn times_that_are_mistakes_are_refused() {
        let ny = tz("America/New_York");
        // a year of two digits: which century?
        assert!(parse_time("03/04/24", ny).unwrap_err().contains("four digits"));
        assert!(parse_time("03/04/24 10:00", ny).is_err());
        assert!(parse_date("03/04/24").unwrap_err().contains("four digits"));
        // milliseconds are not seconds; absurd numbers are no times
        assert!(parse_time("1709380800000", ny).unwrap_err().contains("milliseconds"));
        assert!(parse_time("1e20", ny).is_err());
        // outside 1700 to 2500
        assert!(parse_time("1650-01-01", ny).is_err() && parse_time("2600-01-01T00:00:00Z", ny).is_err());
        assert!(parse_time("1700-01-02", ny).is_ok() && parse_time("2499-12-31", ny).is_ok());
        // shifted research dates: MIMIC-IV's years past 2200, MIMIC-III's births in the 1800s
        assert!(parse_time("2211-05-04 10:00", ny).is_ok() && parse_time("1850-01-01", ny).is_ok());
        // dates of birth: from 1700 to today
        assert!(parse_date("1690-05-01").is_err() && parse_date("2999-01-01").is_err());
        assert_eq!(parse_date("1820-07-01").unwrap(), "1820-07-01");
        assert_eq!(parse_date("19500304").unwrap(), "1950-03-04");
    }

    #[test]
    fn slash_dates_in_medoramas_order() {
        let utc = tz("UTC");
        let april = parse_time("2024-04-03", utc).unwrap();
        assert_eq!(parse_time("03/04/2024", utc).unwrap(), parse_time("2024-03-04", utc).unwrap());
        with_date_order(DateOrder::DayFirst, || {
            assert_eq!(parse_time("03/04/2024", utc).unwrap(), april);
            assert_eq!(parse_date("03/04/1950").unwrap(), "1950-04-03");
        });
        // dots are always day first; the order comes back after
        assert_eq!(parse_time("03.04.2024", utc).unwrap(), april);
        assert_eq!(parse_date("03/04/1950").unwrap(), "1950-03-04");
        assert_eq!(date_order_of("dmy_dot"), DateOrder::DayFirst);
        assert_eq!(date_order_of("ymd"), DateOrder::MonthFirst);
    }
}
