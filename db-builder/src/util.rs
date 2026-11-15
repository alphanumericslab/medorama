//! Small helpers: times and dates in tables, read in the database's time zone and written as ISO
//! 8601 with their offset (docs/schema.md, "Times").

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use std::str::FromStr;

/// A time zone by its IANA name (`America/New_York`, `UTC`), or an error.
pub fn tz(name: &str) -> anyhow::Result<Tz> {
    Tz::from_str(name.trim())
        .map_err(|_| anyhow::anyhow!("{name:?} is not a time zone (an IANA name: America/New_York, Europe/Paris, UTC)"))
}

/// Unix seconds as ISO 8601 in `zone`, with its offset (`2024-03-02T07:10:00-05:00`); a fraction
/// of a second only when there is one.
pub fn iso(t: f64, zone: Tz) -> String {
    let micros = (t * 1e6).round() as i64;
    match DateTime::from_timestamp_micros(micros) {
        Some(d) => {
            let d = d.with_timezone(&zone);
            if micros % 1_000_000 == 0 {
                d.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
            } else {
                d.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()
            }
        }
        None => String::new(),
    }
}

/// A clock time read as if it were UTC (as recording headers' times are read by the engine), as
/// that clock time in `zone`: Unix seconds.
pub fn clock_in(t: f64, zone: Tz) -> Option<f64> {
    let whole = t.floor();
    let n = DateTime::from_timestamp(whole as i64, 0)?.naive_utc();
    let d = match zone.from_local_datetime(&n).earliest() {
        Some(d) => d,
        None => zone.from_local_datetime(&(n + chrono::Duration::hours(1))).earliest()? - chrono::Duration::hours(1),
    };
    Some(d.timestamp() as f64 + (t - whole))
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
    /// Day/month/year (`--day-first`).
    DayFirst,
}

thread_local! {
    static DATE_ORDER: std::cell::Cell<DateOrder> = const { std::cell::Cell::new(DateOrder::MonthFirst) };
}

/// `f`, with slash dates read in `order` on this thread (`add` reads its tables so).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        let ny = tz("America/New_York").unwrap();
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
        assert_eq!(iso(1709380800.0, ny), "2024-03-02T07:00:00-05:00");
        assert_eq!(iso(1709380800.5, tz("UTC").unwrap()), "2024-03-02T12:00:00.500+00:00");
        assert_eq!(clock_in(1709380800.0, ny), Some(1709380800.0 + 5.0 * 3600.0));
        assert!(tz("Mars/Olympus").is_err());
        assert_eq!(parse_date("03/04/1950").unwrap(), "1950-03-04");
    }

    #[test]
    fn times_that_are_mistakes_are_refused() {
        let ny = tz("America/New_York").unwrap();
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
    fn slash_dates_in_the_order_chosen() {
        let utc = tz("UTC").unwrap();
        let april = parse_time("2024-04-03", utc).unwrap();
        assert_eq!(parse_time("03/04/2024", utc).unwrap(), parse_time("2024-03-04", utc).unwrap());
        with_date_order(DateOrder::DayFirst, || {
            assert_eq!(parse_time("03/04/2024", utc).unwrap(), april);
            assert_eq!(parse_date("03/04/1950").unwrap(), "1950-04-03");
        });
        // dots are always day first; the order comes back after
        assert_eq!(parse_time("03.04.2024", utc).unwrap(), april);
        assert_eq!(parse_date("03/04/1950").unwrap(), "1950-03-04");
    }
}
