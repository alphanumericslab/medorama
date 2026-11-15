//! The de-identified view (docs/user-guide.md, "Privacy Mode"): what it hides and how
//! far back it moves dates, as the admin sets them (the `settings` table, key `deid`).  The page
//! applies it to what it shows; exports apply it on the server (`deid=1`).
//!
//! Dates move back by Medorama's offset and by each patient's own whole days (0 to
//! `spread_days`, drawn once and stored: [`Settings::for_draw`]), so the time between any two of
//! a patient's events (their encounters, their ECGs) is kept exactly.  The offset is written in
//! years, months, days and hours; it is counted on the calendar from 2000-01-01 00:00 UTC, which
//! makes it a fixed number of seconds (36570 days by default).

use crate::sql::{params, Connection, OptionalExtension};
use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::db::{audit, now};
use crate::users::User;

/// What an administrator may choose to hide: key, what it covers, whether HIPAA's Safe Harbor
/// lists it, whether it is hidden by default.  Free text is shown by default, marked: it may
/// name the patient, and no rule removes every name from it.  Patients' names, medical record
/// numbers, dates of birth and account numbers are always withheld (the privacy gate, gate.rs):
/// they are not a choice, and a saved setting that lists them is read without them.
pub const FIELDS: &[(&str, &str, bool, bool)] = &[
    ("dates", "Real dates and times: moved back by the offset", true, true),
    ("age_over_89", "Ages above 89: shown as 90+", true, true),
    (
        "notes",
        "The text of notes, reports and every other free text (else shown, marked: it may name the patient)",
        true,
        false,
    ),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Offset {
    pub years: i64,
    pub months: i64,
    pub days: i64,
    pub hours: i64,
}

impl Default for Offset {
    fn default() -> Self {
        Offset { years: 0, months: 0, days: 36570, hours: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// The keys of FIELDS that the view hides.
    pub fields: Vec<String>,
    /// Every date moves back by this offset …
    pub offset: Offset,
    /// … and by each patient's own whole number of days, from 0 to this (their stored draw ×
    /// this): learning one patient's real date gives the others' only to within this many days.
    /// 0: one offset for everyone.
    pub spread_days: i64,
    /// The patient's own part of the offset, in seconds, for settings narrowed to one patient
    /// ([`Settings::for_draw`]); 0 otherwise.
    #[serde(skip)]
    pub extra: i64,
    /// Dates written `01/02/2024` in text are read day first (Medorama's display setting
    /// writes dates day first: display.rs), else month first.
    #[serde(skip)]
    pub day_first: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            fields: FIELDS.iter().filter(|f| f.3).map(|f| f.0.to_string()).collect(),
            offset: Offset::default(),
            spread_days: 365,
            extra: 0,
            day_first: false,
        }
    }
}

impl Settings {
    pub fn hides(&self, field: &str) -> bool {
        self.fields.iter().any(|f| f == field)
    }

    /// The settings for one patient: their dates move back by the offset and by their own
    /// days (`draw`, their stored draw in [0, 1); `None`: none drawn, the offset alone).
    pub fn for_draw(&self, draw: Option<f64>) -> Settings {
        let days = draw.map_or(0, |d| {
            ((d.clamp(0.0, 1.0) * (self.spread_days.max(0) + 1) as f64).floor() as i64).min(self.spread_days.max(0))
        });
        Settings { extra: days * 86400, ..self.clone() }
    }

    /// The settings for one patient (their stored draw).
    pub fn for_patient(&self, c: &Connection, patient: i64) -> anyhow::Result<Settings> {
        let draw: Option<f64> =
            c.query_row("SELECT date_draw FROM patients WHERE id = ?1", [patient], |r| r.get(0)).optional()?.flatten();
        Ok(self.for_draw(draw))
    }

    /// The offset in seconds (dates move back by it): the configured one, and the patient's own
    /// days when the settings are narrowed to a patient.
    pub fn offset_seconds(&self) -> i64 {
        self.base_seconds() + self.extra
    }

    /// The configured offset alone, in seconds.
    pub fn base_seconds(&self) -> i64 {
        let o = &self.offset;
        let reference = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
        let months = o.years * 12 + o.months;
        let back = if months >= 0 {
            reference.checked_sub_months(Months::new(months as u32))
        } else {
            reference.checked_add_months(Months::new((-months) as u32))
        };
        let back = back.unwrap_or(reference) - Duration::days(o.days) - Duration::hours(o.hours);
        (reference - back).num_seconds()
    }

    /// A time (Unix seconds) as the view shows it.
    pub fn shift(&self, t: f64) -> f64 {
        if self.hides("dates") {
            t - self.offset_seconds() as f64
        } else {
            t
        }
    }

    /// An age as the view shows it.
    pub fn age(&self, a: i64) -> String {
        if self.hides("age_over_89") && a > 89 {
            "90+".into()
        } else {
            a.to_string()
        }
    }

    /// Text with its dates (in the forms [`shift_text`] reads) moved back, and the Unix times of
    /// bedside-monitor recording names (UNIT BED - 1700000000).
    pub fn shift_text(&self, s: &str) -> String {
        if !self.hides("dates") {
            return s.to_string();
        }
        shift_text_read(s, self.offset_seconds(), self.day_first)
    }

    /// Whether a database's column, by the canonical column it maps to, is hidden (identifiers
    /// always).
    pub fn hides_column(&self, maps_to: &str) -> bool {
        match maps_to {
            "last_name" | "first_name" | "birth_date" | "mrn" | "mrn_authority" | "csn" => true,
            "text" => self.hides("notes"),
            _ => false,
        }
    }

    /// Whether these settings withhold less than `before`: a choice unticked, fewer days of each
    /// patient's own, or a smaller offset (only an administrator with the PHI permission may).
    pub fn weaker_than(&self, before: &Settings) -> bool {
        before.fields.iter().any(|f| FIELDS.iter().any(|x| x.0 == f) && !self.hides(f))
            || self.spread_days < before.spread_days
            || self.base_seconds() < before.base_seconds()
    }
}

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A month's name at `at` (`Jan`, `JAN`, `Jan.`, `January`, `Sept`, `Sept.`; not followed by
/// a letter): its number (0-11), its length (with its dot), and whether it is written in full.
fn month_at(s: &str, at: usize) -> Option<(usize, usize, bool)> {
    let rest = s.get(at..)?;
    let letters = rest.chars().take_while(char::is_ascii_alphabetic).count();
    let word = &rest[..letters];
    let (m, full) = match MONTH_NAMES.iter().position(|n| n.eq_ignore_ascii_case(word)) {
        Some(m) => (m, word.len() > 3),
        None if letters == 3 => (MONTHS.iter().position(|n| n.eq_ignore_ascii_case(word))?, false),
        None if word.eq_ignore_ascii_case("sept") => (8, false),
        None => return None,
    };
    // an abbreviation's dot (`Mar. 2, 2024`)
    let dot = (!full && rest[letters..].starts_with('.')) as usize;
    Some((m, letters + dot, full))
}

/// A month's name written as `like` was (in full or abbreviated, with its dot, its case).
fn month_like(m: usize, like: &str) -> String {
    let base = like.trim_end_matches('.');
    let full = base.len() > 3
        && !base.eq_ignore_ascii_case("sept")
        && MONTH_NAMES.iter().any(|n| n.eq_ignore_ascii_case(base));
    let mut name = if full {
        MONTH_NAMES[m].to_string()
    } else if base.eq_ignore_ascii_case("sept") && m == 8 {
        "Sept".to_string()
    } else {
        MONTH_NAMES[m][..3].to_string()
    };
    if base.chars().all(|c| c.is_ascii_uppercase()) {
        name = name.to_uppercase();
    }
    if like.ends_with('.') && !full {
        name.push('.');
    }
    name
}

/// A two-digit year as a year: in this century unless that would be more than a year ahead.
fn full_year(yy: i32) -> i32 {
    let this = chrono::Utc::now().year();
    if 2000 + yy <= this + 1 {
        2000 + yy
    } else {
        1900 + yy
    }
}

/// The English ordinal suffix of a day (`1st`, `2nd`, `23rd`, `11th`).
fn ordinal(d: u32) -> &'static str {
    match (d % 10, d % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// An ordinal suffix at `at` (`st`, `nd`, `rd`, `th`, any case): its length, 2, else 0.
fn ordinal_at(s: &str, at: usize) -> usize {
    let w = s.get(at..at + 2).unwrap_or("").to_ascii_lowercase();
    // (`get`: an index inside a character, such as the ° of "37.1 °C", is no suffix)
    let next = s.get((at + 2).min(s.len())..).and_then(|t| t.chars().next());
    if ["st", "nd", "rd", "th"].contains(&w.as_str()) && !next.is_some_and(|c| c.is_ascii_alphanumeric()) {
        2
    } else {
        0
    }
}

/// A run of 1 or 2 digits at `at` (not more): its value and length.
fn small(b: &[u8], at: usize) -> Option<(u32, usize)> {
    let n = (0..3).take_while(|k| b.get(at + k).is_some_and(u8::is_ascii_digit)).count();
    if !(1..=2).contains(&n) {
        return None;
    }
    Some((b[at..at + n].iter().fold(0, |v, d| v * 10 + (d - b'0') as u32), n))
}

/// A time after a date (` 08:00`, ` 08:00:05`) at `at`: hours, minutes, seconds (if written),
/// and its end.
fn time_after(b: &[u8], at: usize) -> Option<(u32, u32, Option<u32>, usize)> {
    if b.get(at) != Some(&b' ') || !digits(b, at + 1, 2) || b.get(at + 3) != Some(&b':') || !digits(b, at + 4, 2) {
        return None;
    }
    let num = |i: usize| (b[i] - b'0') as u32 * 10 + (b[i + 1] - b'0') as u32;
    let (h, m) = (num(at + 1), num(at + 4));
    if b.get(at + 6) == Some(&b':') && digits(b, at + 7, 2) {
        return Some((h, m, Some(num(at + 7)), at + 9));
    }
    Some((h, m, None, at + 6))
}

fn digits(b: &[u8], at: usize, n: usize) -> bool {
    at + n <= b.len() && b[at..at + n].iter().all(u8::is_ascii_digit)
}

/// `s` with each date (and time) moved back by `offset` seconds, written as it was: ISO 8601
/// (`2024-01-02`, with ` 08:00` or `T08:00:05`), `2024/01/02`, `2024.01.02`, `01/02/2024` (month
/// first, else day first if the month would be over 12), `2.1.2024` (day first), `02-JAN-2024`,
/// `Jan 2, 2024`, `Jan. 2, 2024`, `Sept 2, 2024`, `January 2nd, 2024`, `2 January 2024`,
/// `2nd January 2024`, month and year (`January 2024`, `01/2024`), `20240102` (1900-2100), HL7
/// and DICOM date-times (`202401020830`, `20240102083000`, with a fraction), and the Unix time
/// of a bedside-monitor recording's name (`UNIT_BED-<Unix time>`; one before 1970 is written
/// `UNIT_BED--<seconds before 1970>`).  Two-digit years (`01/02/24`, `2.1.24`, `02-JAN-24`) are
/// read in this century unless that is ahead, and written in full once moved: two digits would
/// read as the real century.  A Unix time on its own is a number, and is kept.
pub fn shift_text(s: &str, offset: i64) -> String {
    shift_text_read(s, offset, false)
}

/// [`shift_text`], dates written `01/02/2024` read day first when `day_first` (unless the day
/// would be over 12 then).
pub fn shift_text_read(s: &str, offset: i64, day_first: bool) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        // YYYY-MM-DD[ T]HH:MM[:SS]
        if boundary
            && digits(b, i, 4)
            && b.get(i + 4) == Some(&b'-')
            && digits(b, i + 5, 2)
            && b.get(i + 7) == Some(&b'-')
            && digits(b, i + 8, 2)
        {
            let mut end = i + 10;
            let mut fmt = "%Y-%m-%d";
            let sep = b.get(end).copied();
            if matches!(sep, Some(b' ') | Some(b'T'))
                && digits(b, end + 1, 2)
                && b.get(end + 3) == Some(&b':')
                && digits(b, end + 4, 2)
            {
                end += 6;
                fmt = if sep == Some(b'T') { "%Y-%m-%dT%H:%M" } else { "%Y-%m-%d %H:%M" };
                if b.get(end) == Some(&b':') && digits(b, end + 1, 2) {
                    end += 3;
                    fmt = if sep == Some(b'T') { "%Y-%m-%dT%H:%M:%S" } else { "%Y-%m-%d %H:%M:%S" };
                }
            }
            if !(end < b.len() && b[end].is_ascii_digit()) {
                let text = &s[i..end];
                let parsed = if fmt == "%Y-%m-%d" {
                    NaiveDate::parse_from_str(text, fmt).ok().map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                } else {
                    NaiveDateTime::parse_from_str(text, fmt).ok()
                };
                if let Some(t) = parsed {
                    let moved = t - Duration::seconds(offset);
                    out.push_str(&s[last..i]);
                    out.push_str(&moved.format(fmt).to_string());
                    i = end;
                    last = end;
                    continue;
                }
            }
        }
        // DD-MON-YYYY[ HH:MM] (MUSE's statements: "When compared with ECG of 12-OCT-2023 08:50")
        if boundary && b[i].is_ascii_digit() {
            let d = if digits(b, i, 2) { 2 } else { 1 };
            let m0 = i + d + 1;
            // the year in full, or two digits (`11-MAR-24`)
            let yn = if digits(b, m0 + 4, 4) && !digits(b, m0 + 8, 1) {
                4
            } else if digits(b, m0 + 4, 2) && !digits(b, m0 + 6, 1) {
                2
            } else {
                0
            };
            if b.get(i + d) == Some(&b'-') && b.get(m0 + 3) == Some(&b'-') && yn > 0 {
                if let Some(mon) = s.get(m0..m0 + 3).and_then(|m| MONTHS.iter().position(|x| x.eq_ignore_ascii_case(m)))
                {
                    let mut end = m0 + 4 + yn;
                    let mut time = (0u32, 0u32);
                    let timed = b.get(end) == Some(&b' ')
                        && digits(b, end + 1, 2)
                        && b.get(end + 3) == Some(&b':')
                        && digits(b, end + 4, 2);
                    if timed {
                        time = (s[end + 1..end + 3].parse().unwrap_or(0), s[end + 4..end + 6].parse().unwrap_or(0));
                        end += 6;
                    }
                    let day: u32 = s[i..i + d].parse().unwrap_or(0);
                    let year: i32 = s[m0 + 4..m0 + 4 + yn].parse().unwrap_or(0);
                    let year = if yn == 2 { full_year(year) } else { year };
                    if let Some(t) = NaiveDate::from_ymd_opt(year, mon as u32 + 1, day)
                        .and_then(|x| x.and_hms_opt(time.0, time.1, 0))
                    {
                        let m = t - Duration::seconds(offset);
                        out.push_str(&s[last..i]);
                        out.push_str(&format!("{:02}-{}-{}", m.day(), MONTHS[m.month0() as usize], m.year()));
                        if timed {
                            out.push_str(&format!(" {:02}:{:02}", m.hour(), m.minute()));
                        }
                        i = end;
                        last = end;
                        continue;
                    }
                }
            }
        }
        // other forms: (where it ends, the date and time moved, written as it was)
        if boundary && b[i].is_ascii_alphanumeric() && !(i > 0 && matches!(b[i - 1], b'.' | b'/')) {
            if let Some((end, text)) = other_date(s, i, offset, day_first) {
                out.push_str(&s[last..i]);
                out.push_str(&text);
                i = end;
                last = end;
                continue;
            }
        }
        // HL7 and DICOM date-times: YYYYMMDDHHMM, YYYYMMDDHHMMSS[.ffffff] (1900-2100)
        if boundary && !b[i..].starts_with(b"0") && digits(b, i, 12) {
            let n = if digits(b, i, 14) && !digits(b, i + 14, 1) {
                14
            } else if !digits(b, i + 12, 1) {
                12
            } else {
                0
            };
            let fmt = if n == 14 { "%Y%m%d%H%M%S" } else { "%Y%m%d%H%M" };
            if let Some(t) = (n > 0).then(|| NaiveDateTime::parse_from_str(&s[i..i + n], fmt).ok()).flatten() {
                if (1900..=2100).contains(&t.year()) {
                    let m = t - Duration::seconds(offset);
                    out.push_str(&s[last..i]);
                    out.push_str(&m.format(fmt).to_string());
                    i += n;
                    last = i;
                    continue;
                }
            }
        }
        // YYYYMMDD, alone (file names: "muse_20240102"): a valid date between 1900 and 2100
        if boundary
            && !b[i..].starts_with(b"0")
            && digits(b, i, 8)
            && !digits(b, i + 8, 1)
            && (i + 8 >= b.len() || !b[i + 8].is_ascii_alphabetic() || b[i + 8] == b'T')
        {
            if let Ok(d) = NaiveDate::parse_from_str(&s[i..i + 8], "%Y%m%d") {
                if (1900..=2100).contains(&d.year()) {
                    let m = d.and_hms_opt(0, 0, 0).unwrap() - Duration::seconds(offset);
                    out.push_str(&s[last..i]);
                    out.push_str(&m.format("%Y%m%d").to_string());
                    i += 8;
                    last = i;
                    continue;
                }
            }
        }
        // a bedside-monitor recording name's Unix time: -1XXXXXXXXX
        if b[i] == b'-'
            && i > 0
            && b[i - 1].is_ascii_alphanumeric()
            && b.get(i + 1) == Some(&b'1')
            && digits(b, i + 1, 10)
            && !digits(b, i + 11, 1)
        {
            if let Ok(t) = s[i + 1..i + 11].parse::<i64>() {
                out.push_str(&s[last..i + 1]);
                out.push_str(&(t - offset).to_string());
                i += 11;
                last = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

/// The forms of dates [`shift_text`] reads besides ISO 8601, `DD-MON-YYYY` and `YYYYMMDD`, at
/// `i`: where the date (and its time) ends, and it moved back, written as it was (a two-digit
/// year in full).
fn other_date(s: &str, i: usize, offset: i64, day_first: bool) -> Option<(usize, String)> {
    let b = s.as_bytes();
    let not_digit_at = |at: usize| !b.get(at).is_some_and(u8::is_ascii_digit);
    let pad = |v: u32, n: usize| if n == 2 { format!("{v:02}") } else { v.to_string() };
    let year = |at: usize| digits(b, at, 4).then(|| s[at..at + 4].parse::<i32>().ok()).flatten();
    // a year at `at`, in full or in two digits, not inside a longer number: (year, its length)
    let any_year = |at: usize| {
        if let Some(y) = year(at).filter(|_| not_digit_at(at + 4)) {
            return Some((y, 4));
        }
        (digits(b, at, 2) && not_digit_at(at + 2)).then(|| Some((full_year(s[at..at + 2].parse().ok()?), 2))).flatten()
    };
    // (year, month, day, end, writer of the moved date, whether a day is written)
    type Write = Box<dyn Fn(&NaiveDateTime) -> String>;
    let found: Option<(i32, u32, u32, usize, Write, bool)> = (|| {
        if b[i].is_ascii_digit() {
            // YYYY/MM/DD, YYYY.MM.DD
            if let Some(y) = year(i).filter(|_| matches!(b.get(i + 4), Some(b'/' | b'.'))) {
                let sep = b[i + 4];
                let (m, mn) = small(b, i + 5)?;
                (b.get(i + 5 + mn) == Some(&sep)).then_some(())?;
                let (d, dn) = small(b, i + 6 + mn)?;
                let end = i + 6 + mn + dn;
                (not_digit_at(end) && !(b.get(end) == Some(&sep) && !not_digit_at(end + 1))).then_some(())?;
                let sep = sep as char;
                return Some((
                    y,
                    m,
                    d,
                    end,
                    Box::new(move |t: &NaiveDateTime| {
                        format!("{}{sep}{}{sep}{}", t.year(), pad(t.month(), mn), pad(t.day(), dn))
                    }) as Write,
                    true,
                ));
            }
            let (a, an) = small(b, i)?;
            let sep = *b.get(i + an)?;
            // MM/YYYY: a month and its year
            if sep == b'/' && (1..=12).contains(&a) {
                if let Some(y) =
                    year(i + an + 1).filter(|_| not_digit_at(i + an + 5) && b.get(i + an + 5) != Some(&b'/'))
                {
                    return Some((
                        y,
                        a,
                        15,
                        i + an + 5,
                        Box::new(move |t: &NaiveDateTime| format!("{}/{}", pad(t.month(), an), t.year())) as Write,
                        false,
                    ));
                }
            }
            if sep == b'/' || sep == b'.' {
                // A/B/YYYY, A.B.YYYY, A/B/YY, A.B.YY
                let (c, cn) = small(b, i + an + 1)?;
                (b.get(i + an + 1 + cn) == Some(&sep)).then_some(())?;
                let (y, yn) = any_year(i + an + 2 + cn)?;
                let end = i + an + 2 + cn + yn;
                // not inside a longer run of numbers ("1.2.2024.5")
                (not_digit_at(end) && !(b.get(end) == Some(&sep) && !not_digit_at(end + 1))).then_some(())?;
                let first_is_day = sep == b'.' || a > 12 || (day_first && c <= 12);
                let (d, m, dn, mn) = if first_is_day { (a, c, an, cn) } else { (c, a, cn, an) };
                let sep = sep as char;
                return Some((
                    y,
                    m,
                    d,
                    end,
                    Box::new(move |t: &NaiveDateTime| {
                        let (d, m) = (pad(t.day(), dn), pad(t.month(), mn));
                        if first_is_day {
                            format!("{d}{sep}{m}{sep}{}", t.year())
                        } else {
                            format!("{m}{sep}{d}{sep}{}", t.year())
                        }
                    }) as Write,
                    true,
                ));
            }
            // D Month YYYY, Dth Month YYYY
            let suffix = ordinal_at(s, i + an);
            if b.get(i + an + suffix) == Some(&b' ') {
                let at = i + an + suffix + 1;
                let (m, ml, _) = month_at(s, at)?;
                let like = s[at..at + ml].to_string();
                let at = at + ml;
                (b.get(at) == Some(&b' ')).then_some(())?;
                let y = year(at + 1)?;
                not_digit_at(at + 5).then_some(())?;
                return Some((
                    y,
                    m as u32 + 1,
                    a,
                    at + 5,
                    Box::new(move |t: &NaiveDateTime| {
                        let th = if suffix > 0 { ordinal(t.day()) } else { "" };
                        format!("{}{th} {} {}", pad(t.day(), an), month_like(t.month0() as usize, &like), t.year())
                    }) as Write,
                    true,
                ));
            }
            return None;
        }
        // Month D, YYYY; Month Dth, YYYY; Month YYYY
        let (m, ml, _) = month_at(s, i)?;
        let like = s[i..i + ml].to_string();
        (b.get(i + ml) == Some(&b' ')).then_some(())?;
        if let Some(y) = year(i + ml + 1).filter(|_| not_digit_at(i + ml + 5)) {
            return Some((
                y,
                m as u32 + 1,
                15,
                i + ml + 5,
                Box::new(move |t: &NaiveDateTime| format!("{} {}", month_like(t.month0() as usize, &like), t.year()))
                    as Write,
                false,
            ));
        }
        let (d, dn) = small(b, i + ml + 1)?;
        let suffix = ordinal_at(s, i + ml + 1 + dn);
        let at = i + ml + 1 + dn + suffix;
        let comma = b.get(at) == Some(&b',');
        let at = at + comma as usize;
        (b.get(at) == Some(&b' ')).then_some(())?;
        let y = year(at + 1)?;
        not_digit_at(at + 5).then_some(())?;
        Some((
            y,
            m as u32 + 1,
            d,
            at + 5,
            Box::new(move |t: &NaiveDateTime| {
                format!(
                    "{} {}{}{} {}",
                    month_like(t.month0() as usize, &like),
                    pad(t.day(), dn),
                    if suffix > 0 { ordinal(t.day()) } else { "" },
                    if comma { "," } else { "" },
                    t.year()
                )
            }) as Write,
            true,
        ))
    })();
    let (y, m, d, end, write, has_day) = found?;
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    // a time after it moves with it
    let timed = if has_day { time_after(b, end) } else { None };
    let (time, end, seconds) = match timed {
        Some((h, mi, sec, e)) => (chrono::NaiveTime::from_hms_opt(h, mi, sec.unwrap_or(0))?, e, Some(sec.is_some())),
        None => (chrono::NaiveTime::MIN, end, None),
    };
    let moved = date.and_time(time) - Duration::seconds(offset);
    let mut text = write(&moved);
    match seconds {
        Some(true) => text.push_str(&moved.format(" %H:%M:%S").to_string()),
        Some(false) => text.push_str(&moved.format(" %H:%M").to_string()),
        None => {}
    }
    Some((end, text))
}

pub fn load(c: &Connection) -> anyhow::Result<Settings> {
    let v: Option<String> =
        c.query_row("SELECT value FROM settings WHERE key = 'deid'", [], |r| r.get(0)).optional()?;
    let v = match v {
        Some(v) => Some(v),
        None => draw_offset(c)?,
    };
    let mut s: Settings = v.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    s.fields.retain(|f| FIELDS.iter().any(|x| x.0 == f));
    // dates in text read as Medorama writes them
    s.day_first = crate::display::load(c).is_ok_and(|d| d.date.starts_with("dmy"));
    Ok(s)
}

/// A new installation's offset, drawn at random (80 to 120 years and 0 to 364 days, from the
/// system's secure generator) and kept, the first time the settings are read before any patient
/// is linked: the documented default ([`Offset::default`]) would let anyone re-date what Privacy
/// Mode shows.  An installation with patients already keeps the default (its dates as shown do
/// not move): `serve` warns about it ([`start_warning`]).  The settings kept, if drawn.
pub fn draw_offset(c: &Connection) -> anyhow::Result<Option<String>> {
    let patients: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0))?;
    if patients > 0 {
        return Ok(None);
    }
    let mut r = [0u8; 4];
    getrandom::fill(&mut r).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
    let (a, b) = (u16::from_le_bytes([r[0], r[1]]), u16::from_le_bytes([r[2], r[3]]));
    let s = Settings {
        offset: Offset { years: 80 + (a % 41) as i64, months: 0, days: (b % 365) as i64, hours: 0 },
        ..Default::default()
    };
    c.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ('deid', ?1, NULL, ?2) ON CONFLICT(key) DO NOTHING",
        params![serde_json::to_string(&s)?, now()],
    )?;
    // another connection may have drawn first: what is kept
    let kept: String = c.query_row("SELECT value FROM settings WHERE key = 'deid'", [], |r| r.get(0))?;
    if kept == serde_json::to_string(&s)? {
        // the offset itself stays out of the log
        audit(c, None, "deid_offset_drawn", "deid", "");
    }
    Ok(Some(kept))
}

/// Admins only.
/// The fewest days each patient's dates may move by on top of Medorama's offset (`spread_days`).
pub const MIN_SPREAD_DAYS: i64 = 30;

/// What `serve` warns about at start: fewer days than [`MIN_SPREAD_DAYS`], saved before that
/// minimum, and the documented default offset still in use (an installation made before
/// offsets were drawn: [`draw_offset`]).  Changing either moves every patient's dates as shown,
/// so it is left to an administrator.
pub fn start_warning(s: &Settings) -> Option<String> {
    let mut w = vec![];
    if s.spread_days < MIN_SPREAD_DAYS {
        w.push(format!(
            "each patient's own days in Privacy Mode (spread_days) are {}, under the minimum of {MIN_SPREAD_DAYS}: \
             one real date gives others too closely; raise it in Admin › Privacy Mode \
             (docs/protecting-patient-data.md).",
            s.spread_days
        ));
    }
    if s.offset == Offset::default() {
        w.push(
            "Privacy Mode's offset is the documented default (36,570 days): anyone who reads the documentation \
             can re-date what it shows; set another in Admin › Privacy Mode (docs/protecting-patient-data.md)."
                .to_string(),
        );
    }
    (!w.is_empty()).then(|| w.join(" "))
}

pub fn save(c: &Connection, user: &User, s: &Settings) -> anyhow::Result<Settings> {
    if !user.is_admin() {
        anyhow::bail!("only admins change what the de-identified view hides");
    }
    let mut s = s.clone();
    s.fields.retain(|f| FIELDS.iter().any(|x| x.0 == f));
    s.fields.sort();
    s.fields.dedup();
    // withholding less is for who may see what it withholds
    if !user.phi_allowed && s.weaker_than(&load(c)?) {
        anyhow::bail!(
            "only an admin with the PHI permission may withhold less (untick a choice, fewer days of each \
             patient's own, a smaller offset)"
        );
    }
    let o = &s.offset;
    if [o.years, o.months, o.days, o.hours].iter().any(|x| *x < 0) {
        anyhow::bail!("the offset moves dates back: years, months, days and hours are 0 or more");
    }
    if o.years > 1000 || o.months > 12_000 || o.days > 400_000 || o.hours > 10_000_000 {
        anyhow::bail!("the offset is too large");
    }
    // fewer days would let one known real date give every other patient's almost exactly
    if !(MIN_SPREAD_DAYS..=3650).contains(&s.spread_days) {
        anyhow::bail!("each patient's own days: from {MIN_SPREAD_DAYS} to 3650");
    }
    s.extra = 0;
    c.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ('deid', ?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![serde_json::to_string(&s)?, (user.id > 0).then_some(user.id), now()],
    )?;
    audit(c, Some(user), "deid_settings", "deid", &serde_json::to_string(&s)?);
    Ok(s)
}

// ------------------------------------------------------------------ answers and files

/// Keys whose numbers are times (Unix seconds) in the API's answers.
const TIME_KEYS: &[&str] = &[
    "time",
    "t",
    "end",
    "t0",
    "t1",
    "start",
    "end_time",
    "start_time",
    "admit",
    "discharge",
    "admit_time",
    "discharge_time",
    "t_start",
    "t_end",
    "name_time",
    "first",
    "last",
    "latest",
    "created_at",
    "updated_at",
    "refreshed_at",
    "added_at",
    "started_at",
    "finished_at",
    "queued_at",
    "decided_at",
    "linked_at",
    "first_seen",
    "last_seen",
    "recorded",
    "at",
];
/// Keys whose strings are addresses or keys: never changed (a link must still lead to its file).
const KEEP: &[&str] = &[
    "url",
    "src",
    "href",
    "path",
    "paths",
    "file",
    "files",
    "link",
    "key",
    "entry",
    "hash",
    "origin",
    "id",
    "patient_id",
    "encounter_id",
    "kind",
    "status",
    "category",
    "mode",
    "role",
    "username",
    "mime",
    "sop",
    "op",
    "cmp",
    "unit",
    "maps_to",
    "header",
    "headers",
    "format",
    "flavor",
    "domain",
    "vocabulary",
    "code",
    "fingerprint",
    "source",
    "viewer",
];

/// Whether a number is a time: under a time key, and like one (beyond 1973 or before 1967 in
/// Unix seconds), so that counts and offsets that share a key's name are never moved.
fn is_time(key: &str, x: f64) -> bool {
    TIME_KEYS.contains(&key) && x.abs() > 1e8
}

/// An answer of the API as a de-identified session gets it: times and dates moved back, ages
/// above 89 as "90+".  (Identifiers are not in it: the session is answered as without the PHI
/// permission.)
pub fn answer(v: &mut serde_json::Value, s: &Settings) {
    answer_with(v, s, &|_| None)
}

/// `answer`, each object that names a patient (`patient_of` gives that patient's settings)
/// moved by that patient's own offset, with everything in it.
pub fn answer_with(
    v: &mut serde_json::Value,
    s: &Settings,
    patient_of: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Option<Settings>,
) {
    use serde_json::Value;
    type Of<'a> = &'a dyn Fn(&serde_json::Map<String, Value>) -> Option<Settings>;
    fn shift_num(v: &mut Value, s: &Settings) {
        if let Some(x) = v.as_f64() {
            if x.abs() > 1e8 {
                *v = serde_json::json!(s.shift(x));
            }
        }
    }
    fn walk(v: &mut Value, key: &str, s: &Settings, of: Of) {
        match v {
            Value::Number(n) => {
                let x = n.as_f64().unwrap_or(0.0);
                if is_time(key, x) {
                    *v = serde_json::json!(s.shift(x));
                } else if key == "age" && x > 89.0 && s.hides("age_over_89") {
                    *v = Value::String("90+".into());
                }
            }
            Value::String(t) if !KEEP.contains(&key) => {
                let moved = s.shift_text(t);
                if moved != *t {
                    *t = moved;
                }
            }
            Value::Array(items) => match key {
                // series: a time per sample; journey cells (lane, sub, time, n, level); trend
                // points (time, value, level); gaps (from, to)
                "t" => items.iter_mut().for_each(|x| shift_num(x, s)),
                "cells" | "points" | "series" | "gaps" => {
                    let at: &[usize] = match key {
                        "cells" => &[2],
                        "gaps" => &[0, 1],
                        _ => &[0],
                    };
                    for it in items.iter_mut() {
                        if let Value::Array(t) = it {
                            for &k in at {
                                if let Some(x) = t.get_mut(k) {
                                    shift_num(x, s);
                                }
                            }
                        } else {
                            walk(it, key, s, of);
                        }
                    }
                }
                _ => items.iter_mut().for_each(|x| walk(x, key, s, of)),
            },
            Value::Object(m) => {
                let own = of(m);
                let s = own.as_ref().unwrap_or(s);
                for (k, x) in m.iter_mut() {
                    walk(x, k, s, of);
                }
            }
            _ => {}
        }
    }
    if s.hides("dates") || s.hides("age_over_89") {
        walk(v, "", s, patient_of);
    }
}

/// Numbers that look like Unix times (2000 to 2100, in seconds or milliseconds) under keys
/// [`answer_with`] does not know, moved back as it moves times: a plug-in's summary has keys of
/// its own.  Keys it knows are left to it (they would move twice).
pub fn shift_unknown_times(v: &mut serde_json::Value, s: &Settings) {
    use serde_json::Value;
    if !s.hides("dates") {
        return;
    }
    let off = s.offset_seconds() as f64;
    fn walk(v: &mut Value, key: &str, off: f64) {
        match v {
            Value::Number(n) if !TIME_KEYS.contains(&key) => {
                let x = n.as_f64().unwrap_or(0.0);
                if (9.4e8..4.11e9).contains(&x) {
                    *v = serde_json::json!(x - off);
                } else if (9.4e11..4.11e12).contains(&x) {
                    *v = serde_json::json!(x - off * 1000.0);
                }
            }
            Value::Array(items)
                if !["t", "cells", "points", "series", "gaps"].contains(&key) && !TIME_KEYS.contains(&key) =>
            {
                items.iter_mut().for_each(|x| walk(x, key, off))
            }
            Value::Object(m) => m.iter_mut().for_each(|(k, x)| walk(x, k, off)),
            _ => {}
        }
    }
    walk(v, "", off);
}

/// An SVG picture without the texts it shows or describes (`<text>`, `<title>`, `<desc>`,
/// `<metadata>`, comments), which may name the patient: its drawing as it is.
pub fn svg(x: &str) -> String {
    let lower = x.to_ascii_lowercase();
    let mut out = String::with_capacity(x.len());
    let mut i = 0;
    while i < x.len() {
        let rest = &lower[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |e| e + 3);
            continue;
        }
        let tag = ["text", "title", "desc", "metadata"].into_iter().find(|t| {
            rest.starts_with(&format!("<{t}"))
                && rest[t.len() + 1..].starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        });
        if let Some(t) = tag {
            let open_end = rest.find('>').map_or(rest.len(), |e| e + 1);
            if rest[..open_end].ends_with("/>") {
                i += open_end;
                continue;
            }
            let close = format!("</{t}");
            i += rest.find(&close).map_or(rest.len(), |e| e + rest[e..].find('>').map_or(rest.len() - e, |g| g + 1));
            continue;
        }
        let ch = x[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// A PDF document's descriptive fields blanked in place (its offsets kept: the file still
/// opens): the document information dictionary's `/Author`, `/Title`, `/Subject`, `/Keywords`,
/// `/Creator` and `/Producer` strings, and the XMP metadata's authors, titles, descriptions,
/// subjects and keywords where they are written uncompressed.  What the pages show is not
/// changed (it may still identify: the file stays marked).
pub fn pdf_info(b: &mut [u8]) {
    fn find(b: &[u8], what: &[u8], from: usize) -> Option<usize> {
        b.get(from..)?.windows(what.len()).position(|w| w == what).map(|p| p + from)
    }
    for key in [&b"/Author"[..], b"/Title", b"/Subject", b"/Keywords", b"/Creator", b"/Producer"] {
        let mut at = 0;
        while let Some(k) = find(b, key, at) {
            let mut i = k + key.len();
            at = i;
            if b.get(i).is_some_and(|c| c.is_ascii_alphanumeric()) {
                continue; // a longer name (`/Titles`)
            }
            while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                i += 1;
            }
            match b.get(i) {
                // a literal string: balanced parentheses, backslash escapes
                Some(b'(') => {
                    let (mut depth, mut j) = (1, i + 1);
                    while j < b.len() && depth > 0 {
                        match b[j] {
                            b'\\' => {
                                b[j] = b' ';
                                if j + 1 < b.len() {
                                    b[j + 1] = b' ';
                                }
                                j += 2;
                                continue;
                            }
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            b[j] = b' ';
                        }
                        j += 1;
                    }
                    at = j;
                }
                // a hexadecimal string (not a dictionary): spaces
                Some(b'<') if b.get(i + 1) != Some(&b'<') => {
                    let mut j = i + 1;
                    while j < b.len() && b[j] != b'>' {
                        if b[j].is_ascii_hexdigit() {
                            b[j] = if (j - i) % 2 == 1 { b'2' } else { b'0' };
                        }
                        j += 1;
                    }
                    at = j;
                }
                _ => {}
            }
        }
    }
    // XMP: the texts inside these elements, their tags kept
    for tag in ["dc:creator", "dc:title", "dc:description", "dc:subject", "pdf:Keywords", "pdf:Author"] {
        let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
        let mut at = 0;
        while let Some(o) = find(b, open.as_bytes(), at) {
            let Some(e) = find(b, close.as_bytes(), o) else { break };
            let mut in_tag = false;
            for c in &mut b[o..e] {
                match *c {
                    b'<' => in_tag = true,
                    b'>' => in_tag = false,
                    _ if !in_tag && !c.is_ascii_whitespace() => *c = b' ',
                    _ => {}
                }
            }
            at = e + close.len();
        }
    }
}

/// A comment's key as compared: lower case, letters and digits only (`Patient_Name`,
/// `patient name`, `Patient-Name` are one key).
fn compact(key: &str) -> String {
    key.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Header keys of a WFDB record's comments that name staff (who confirmed, read, recorded it).
fn staff(key: &str) -> bool {
    [
        "confirmedby",
        "overreadby",
        "overreader",
        "readby",
        "reader",
        "editedby",
        "editor",
        "signedby",
        "technician",
        "tech",
        "operator",
        "physician",
        "referringphysician",
        "orderingphysician",
        "attending",
    ]
    .contains(&compact(key).as_str())
}

/// Header keys of a WFDB record's comments that identify the patient (as the ECG viewer drops
/// them): any key that begins with one of these (`Patient name`, `Birthdate`, `Name_last`), or
/// is `id`.
fn identifying(key: &str) -> bool {
    let k = compact(key);
    k == "id"
        || [
            "patient",
            "name",
            "firstname",
            "lastname",
            "familyname",
            "givenname",
            "surname",
            "mrn",
            "medicalrecord",
            "dob",
            "birth",
            "dateofbirth",
            "ssn",
            "socialsecurity",
            "address",
            "phone",
            "email",
        ]
        .iter()
        .any(|w| k.starts_with(w))
}

/// A WFDB comment's key and value: `key: value`, `key=value`, or `key - value`.
fn comment_pair(c: &str) -> Option<(&str, &str)> {
    let at = c.find([':', '=']).or_else(|| c.find(" - ").map(|i| i + 1))?;
    let sep = if c[at..].starts_with('-') { 1 } else { c[at..].chars().next()?.len_utf8() };
    Some((&c[..at], &c[at + sep..]))
}

/// A WFDB header as a de-identified session gets it: identifying comments left out, the base
/// date and the dates of comments moved back, ages above 89 as 90+.
pub fn wfdb_header(text: &str, s: &Settings) -> String {
    let mut out = vec![];
    let mut first = true;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(c) = t.strip_prefix('#') {
            if let Some((k, v)) = comment_pair(c) {
                if identifying(k) {
                    continue;
                }
                if staff(k) && !v.trim().is_empty() {
                    out.push(format!("#{}: {}", k.trim_end(), crate::gate::STAFF_NAME));
                    continue;
                }
                if compact(k) == "age" && s.hides("age_over_89") && v.trim().parse::<f64>().is_ok_and(|a| a > 89.0) {
                    out.push(format!("#{}: 90+", k.trim_end()));
                    continue;
                }
            }
            out.push(s.shift_text(line));
            continue;
        }
        if first && !t.is_empty() {
            first = false;
            out.push(shift_wfdb_base(line, s));
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n") + "\n"
}

/// The record line's base time and date (`HH:MM:SS DD/MM/YYYY`) moved back.
fn shift_wfdb_base(line: &str, s: &Settings) -> String {
    let f: Vec<&str> = line.split_whitespace().collect();
    if !s.hides("dates") || f.len() < 6 {
        return line.to_string();
    }
    let (tm, dt) = (f[f.len() - 2], f[f.len() - 1]);
    let Ok(t) =
        NaiveDateTime::parse_from_str(&format!("{dt} {}", tm.split('.').next().unwrap_or(tm)), "%d/%m/%Y %H:%M:%S")
    else {
        return line.to_string();
    };
    let m = t - Duration::seconds(s.offset_seconds());
    let mut g: Vec<String> = f.iter().map(|x| x.to_string()).collect();
    let n = g.len();
    g[n - 2] = m.format("%H:%M:%S").to_string();
    g[n - 1] = m.format("%d/%m/%Y").to_string();
    g.join(" ")
}

/// An EDF or BDF file's header (its 256 bytes and its signals' 256 each) as a de-identified
/// session gets it (docs/edf.md, "Patient identity in EDF files"): the patient field `X X X X`
/// (EDF+'s anonymous form); the recording field `Startdate dd-MMM-yyyy X X X` in EDF+ and BDF+
/// (the date moved back), blank in plain EDF, whose field is free text; the start date and time
/// moved back.  EDF writes a two-digit year for 1985-2084 only: an EDF+ date outside them is
/// written `yy`, with the year in the recording field, as EDF+ prescribes; a plain EDF file's is
/// left out (`  .  .  `), so that the file states no start rather than a wrong one.
pub fn edf_header(head: &[u8], s: &Settings) -> Vec<u8> {
    let mut out = head.to_vec();
    if out.len() < 256 {
        return out;
    }
    let put = |out: &mut Vec<u8>, at: usize, width: usize, text: &str| {
        let b = text.as_bytes();
        for i in 0..width {
            out[at + i] = *b.get(i).unwrap_or(&b' ');
        }
    };
    let field = |a: usize, b: usize| String::from_utf8_lossy(&head[a..b]).trim().to_string();
    let reserved = field(192, 236);
    let plus = ["EDF+C", "EDF+D", "BDF+C", "BDF+D"].iter().any(|p| reserved.starts_with(p));
    put(&mut out, 8, 80, "X X X X");
    // the start, as the file states it (the EDF+ recording field's year in full)
    let (date, time) = (field(168, 176), field(176, 184));
    let full_year = field(88, 168)
        .strip_prefix("Startdate ")
        .and_then(|r| r.get(7..11))
        .and_then(|y| y.parse::<i32>().ok())
        .filter(|_| plus);
    let num = |t: &str, i: usize| t.get(i..i + 2).and_then(|x| x.parse::<u32>().ok());
    let start = (|| {
        let (d, m) = (num(&date, 0)?, num(&date, 3)?);
        let y =
            full_year.or_else(|| num(&date, 6).map(|yy| if yy >= 85 { 1900 + yy as i32 } else { 2000 + yy as i32 }))?;
        NaiveDate::from_ymd_opt(y, m, d)?.and_hms_opt(num(&time, 0)?, num(&time, 3)?, num(&time, 6)?)
    })();
    let moved = match start {
        Some(t) if s.hides("dates") => Some(t - Duration::seconds(s.offset_seconds())),
        other => other,
    };
    match moved {
        Some(t) => {
            let yy = t.format("%y").to_string();
            let two = (1985..=2084).contains(&t.year());
            put(&mut out, 168, 8, &t.format(&format!("%d.%m.{}", if two { yy.as_str() } else { "yy" })).to_string());
            put(&mut out, 176, 8, &t.format("%H.%M.%S").to_string());
            if plus {
                let day = t.format("%d").to_string();
                let mon = MONTHS[t.month0() as usize];
                put(&mut out, 88, 80, &format!("Startdate {day}-{mon}-{} X X X", t.year()));
            } else {
                put(&mut out, 88, 80, "");
                if !two {
                    put(&mut out, 168, 16, "  .  .    .  .  ");
                }
            }
        }
        None => {
            put(&mut out, 88, 80, if plus { "Startdate X X X X" } else { "" });
            if s.hides("dates") {
                put(&mut out, 168, 16, "  .  .    .  .  ");
            }
        }
    }
    out
}

/// An EDF+ annotation signal's bytes in one data record with the texts of its annotations left
/// out: each record's time-keeping annotation (its onset) is kept, so that the file's timing is
/// whole; every other annotation is dropped, since free text may identify.
pub fn edf_annotations(bytes: &mut [u8]) {
    let first = bytes.split(|&c| c == 0).find(|t| !t.is_empty()).map(<[u8]>::to_vec);
    bytes.fill(0);
    if let Some(tal) = first {
        // its onset (and duration), then an empty annotation: `+onset\x14\x14\x00`
        let time = tal.split(|&c| c == 0x14).next().unwrap_or(&[]);
        let mut keep = time.to_vec();
        keep.extend_from_slice(b"\x14\x14");
        if keep.len() < bytes.len() {
            bytes[..keep.len()].copy_from_slice(&keep);
        }
    }
}

/// A GE MUSE XML file as a de-identified session gets it: the patient's identity (name, ID,
/// date of birth) and the order left out, the age capped, the dates (MM-DD-YYYY with their
/// times) and the statements' dates moved back.  Not a general XML de-identifier: MUSE's
/// elements by name (their start tags may hold attributes and spaces).  Fails (the file is not
/// sent) when anything of the patient's identity is left: a patient's name, ID or date of
/// birth anywhere, or demographics it could not rewrite.
pub fn muse_xml(text: &str, s: &Settings) -> anyhow::Result<String> {
    let mut x = text.to_string();
    // whole elements that identify: the order (accounts, physicians' IDs), and within the
    // patient's demographics everything but age, its units and sex
    while let Some(y) = drop_element(&x, "Order") {
        x = y;
    }
    let mut from = 0;
    while let Some((a, inner_at)) = open_tag(&x, "PatientDemographics", from) {
        if x[..inner_at].ends_with("/>") {
            from = inner_at;
            continue;
        }
        let Some((b, after)) = close_tag(&x, "PatientDemographics", inner_at) else { break };
        let inner = &x[inner_at..b];
        let mut kept = String::new();
        for tag in ["PatientAge", "AgeUnits", "Gender"] {
            if let Some(v) = element_text(inner, tag) {
                let v =
                    if tag == "PatientAge" && s.hides("age_over_89") && v.trim().parse::<f64>().is_ok_and(|a| a > 89.0)
                    {
                        "90+".to_string()
                    } else {
                        v
                    };
                kept.push_str(&format!("<{tag}>{v}</{tag}>"));
            }
        }
        let rebuilt = format!("<PatientDemographics>{kept}</PatientDemographics>");
        x = format!("{}{rebuilt}{}", &x[..a], &x[after..]);
        from = a + rebuilt.len();
    }
    x = muse_staff(&x);
    if s.hides("dates") {
        x = shift_muse_dates(&x, s.offset_seconds());
        x = s.shift_text(&x);
    }
    if let Some(left) = muse_identity_left(&x) {
        anyhow::bail!("the file still holds the patient's identity ({left}) after de-identification");
    }
    Ok(x)
}

/// What of the patient's identity a rewritten MUSE file still holds, if anything: an element
/// of the demographics other than age, units and sex; a patient's name not replaced; a
/// patient ID or date of birth (compared ignoring case).
fn muse_identity_left(x: &str) -> Option<String> {
    let mut in_demographics = false;
    let mut rest = x;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let closing = rest.starts_with('/');
        let name: String =
            rest.trim_start_matches('/').chars().take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/').collect();
        let lower = name.to_ascii_lowercase();
        let value = rest.find('>').map_or("", |e| &rest[e + 1..]);
        let value = value[..value.find('<').unwrap_or(value.len())].trim();
        if lower == "patientdemographics" {
            in_demographics = !closing && !rest[..rest.find('>').unwrap_or(0)].ends_with('/');
            continue;
        }
        if closing || name.is_empty() || name.starts_with(['?', '!']) {
            continue;
        }
        if in_demographics && !["PatientAge", "AgeUnits", "Gender"].contains(&name.as_str()) {
            return Some(name);
        }
        let patient_name = lower.starts_with("patient") && lower.ends_with("name");
        if patient_name && !value.is_empty() && value != crate::gate::PATIENT_NAME {
            return Some(name);
        }
        if ["patientid", "dateofbirth", "birthdate", "patientbirthdate"].contains(&lower.as_str()) && !value.is_empty()
        {
            return Some(name);
        }
    }
    None
}

/// MUSE's staff (overreader, editor, technician, referring physician): their names replaced by
/// the placeholder, their IDs emptied, and "Confirmed by …" in the statements likewise.
fn muse_staff(x: &str) -> String {
    let mut out = String::with_capacity(x.len());
    let mut rest = x;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[1..close];
        let name = tag.trim_end_matches('/');
        let name = name.split_whitespace().next().unwrap_or("");
        let any_name = ["LastName", "FirstName", "MiddleName"].iter().any(|n| name.ends_with(n));
        // a patient's name outside the demographics: replaced by its own placeholder
        let patient = any_name && name.starts_with("Patient");
        let staff_name = any_name;
        let staff_id =
            ["OverreaderID", "EditorID", "AcquisitionTechID", "ReferringMDID", "OrderingMDID", "AttendingMDID"]
                .contains(&name);
        if (staff_name || staff_id) && !tag.ends_with('/') {
            let end = format!("</{name}>");
            if let Some(e) = rest.find(&end) {
                let value = &rest[close + 1..e];
                out.push_str(&rest[..close + 1]);
                // one placeholder per person: in the last name; first and middle names emptied
                if staff_name && name.ends_with("LastName") && !value.trim().is_empty() {
                    out.push_str(if patient { crate::gate::PATIENT_NAME } else { crate::gate::STAFF_NAME });
                }
                out.push_str(&end);
                rest = &rest[e + end.len()..];
                continue;
            }
        }
        out.push_str(&rest[..close + 1]);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    staff_phrases(&out).unwrap_or(out)
}

/// "Confirmed by DOE, JOHN (12) on 01/02/2024" and the like: who confirmed, signed, overread or
/// edited, up to the end of the line or element, replaced by the placeholder.  `None`: none.
pub fn staff_phrases(text: &str) -> Option<String> {
    const BY: [&str; 4] = ["confirmed by ", "signed by ", "overread by ", "edited by "];
    // ASCII lower case: the same byte offsets as the text
    let lower = text.to_ascii_lowercase();
    if !BY.iter().any(|m| lower.contains(m)) {
        return None;
    }
    let mut r = String::with_capacity(text.len());
    let mut at = 0;
    while let Some((p, n)) = BY.iter().filter_map(|m| lower[at..].find(m).map(|p| (at + p, m.len()))).min() {
        let stop = text[p + n..].find(['<', '\n']).map_or(text.len(), |e| p + n + e);
        r.push_str(&text[at..p + n]);
        r.push_str(crate::gate::STAFF_NAME);
        at = stop;
    }
    r.push_str(&text[at..]);
    Some(r)
}

/// MUSE's dates (`<AcquisitionDate>MM-DD-YYYY</AcquisitionDate>`, each with its
/// `<AcquisitionTime>HH:MM:SS</AcquisitionTime>` when there is one) moved back `off` seconds.
fn shift_muse_dates(x: &str, off: i64) -> String {
    // (start, end, replacement) of each value to change
    let mut edits: Vec<(usize, usize, String)> = vec![];
    let mut pos = 0;
    while let Some(rel) = x[pos..].find("Date>") {
        let after = pos + rel + 5;
        pos = after;
        let Some(open) = x[..after - 5].rfind('<') else { continue };
        let name = &x[open + 1..after - 5];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue; // a closing tag, or not an element's name
        }
        let Some(close) = x[after..].find('<').map(|c| after + c) else { continue };
        let Ok(d) = NaiveDate::parse_from_str(x[after..close].trim(), "%m-%d-%Y") else { continue };
        let tag = format!("<{name}Time>");
        let time = x.find(&tag).map(|p| p + tag.len()).and_then(|t0| {
            let t1 = t0 + x[t0..].find('<')?;
            Some((t0, t1, chrono::NaiveTime::parse_from_str(x[t0..t1].trim(), "%H:%M:%S").ok()?))
        });
        let moved = d.and_time(time.map(|t| t.2).unwrap_or_default()) - Duration::seconds(off);
        edits.push((after, close, moved.format("%m-%d-%Y").to_string()));
        if let Some((t0, t1, _)) = time {
            edits.push((t0, t1, moved.format("%H:%M:%S").to_string()));
        }
    }
    edits.sort_by_key(|e| e.0);
    edits.dedup_by_key(|e| e.0);
    let mut out = String::with_capacity(x.len());
    let mut last = 0;
    for (a, b, r) in edits {
        if a < last {
            continue;
        }
        out.push_str(&x[last..a]);
        out.push_str(&r);
        last = b;
    }
    out.push_str(&x[last..]);
    out
}

/// An element's start tag `<tag>` from `from` on, attributes and spaces allowed (`<tag a="1">`):
/// where it starts, and where its content starts.
fn open_tag(xml: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let pat = format!("<{tag}");
    let mut at = from;
    while let Some(rel) = xml.get(at..)?.find(&pat) {
        let a = at + rel;
        let after = a + pat.len();
        match xml[after..].chars().next() {
            Some('>') => return Some((a, after + 1)),
            Some(c) if c.is_whitespace() || c == '/' => return Some((a, after + xml[after..].find('>')? + 1)),
            _ => at = after,
        }
    }
    None
}

/// An element's end tag `</tag>` (or `</tag >`) from `from` on: where it starts and ends.
fn close_tag(xml: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let pat = format!("</{tag}");
    let mut at = from;
    while let Some(rel) = xml.get(at..)?.find(&pat) {
        let a = at + rel;
        let after = a + pat.len();
        let gap = xml[after..].chars().take_while(|c| c.is_whitespace()).map(char::len_utf8).sum::<usize>();
        if xml[after + gap..].starts_with('>') {
            return Some((a, after + gap + 1));
        }
        at = after;
    }
    None
}

fn element_text(xml: &str, tag: &str) -> Option<String> {
    let (_, a) = open_tag(xml, tag, 0)?;
    if xml[..a].ends_with("/>") {
        return None;
    }
    let (b, _) = close_tag(xml, tag, a)?;
    Some(xml[a..b].to_string())
}

/// `xml` without its first element `tag`; `None` when it has none.
fn drop_element(xml: &str, tag: &str) -> Option<String> {
    let (a, inner) = open_tag(xml, tag, 0)?;
    let end = if xml[..inner].ends_with("/>") { inner } else { close_tag(xml, tag, inner)?.1 };
    Some(format!("{}{}", &xml[..a], &xml[end..]))
}

/// The settings as the page takes them: with the offset in seconds and the list of fields.
pub fn describe(s: &Settings) -> serde_json::Value {
    serde_json::json!({
        "fields": s.fields, "offset": s.offset, "offset_seconds": s.base_seconds(), "spread_days": s.spread_days,
        "choices": FIELDS.iter().map(|(k, l, hipaa, on)| serde_json::json!({ "key": k, "label": l, "hipaa": hipaa, "default": on })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_patients_days_have_a_floor() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config::new(&dir.path().join("home"), &dir.path().join("results"));
        let db = crate::db::Db::open(&cfg, true).unwrap();
        let c = db.get().unwrap();
        let admin = User::local();
        for days in [0, MIN_SPREAD_DAYS - 1, 3651] {
            let s = Settings { spread_days: days, ..Default::default() };
            assert!(save(&c, &admin, &s).is_err(), "{days}");
        }
        let offset = Offset { years: 97, months: 0, days: 11, hours: 0 };
        let s = Settings { spread_days: MIN_SPREAD_DAYS, offset, ..Default::default() };
        assert_eq!(save(&c, &admin, &s).unwrap().spread_days, MIN_SPREAD_DAYS);
        assert!(start_warning(&s).is_none());
        assert!(start_warning(&Settings { spread_days: 0, ..Default::default() }).is_some());
    }

    #[test]
    fn edf_files() {
        let edf = |name: &str| {
            let p = format!("{}/../viewers/vitalstream/testdata/edf/{name}", env!("CARGO_MANIFEST_DIR"));
            let b = std::fs::read(p).unwrap();
            let ns: usize = String::from_utf8_lossy(&b[252..256]).trim().parse().unwrap();
            b[..256 * (ns + 1)].to_vec()
        };
        let text = |b: &[u8], a: usize, n: usize| String::from_utf8_lossy(&b[a..a + n]).trim_end().to_string();
        // EDF+, a day back: the identity gone, the date moved in the header and the recording field
        let day = Settings { offset: Offset { years: 0, months: 0, days: 1, hours: 0 }, ..Default::default() };
        let h = edf_header(&edf("psg_plus.edf"), &day);
        assert_eq!(text(&h, 8, 80), "X X X X");
        assert_eq!(text(&h, 88, 80), "Startdate 29-FEB-2024 X X X");
        assert_eq!((text(&h, 168, 8), text(&h, 176, 8)), ("29.02.24".into(), "13.00.00".into()));
        assert_eq!(&h[184..], &edf("psg_plus.edf")[184..], "the rest of the header as it was");
        // the default offset (a century back): EDF+ writes the year `yy` and states it in full
        let h = edf_header(&edf("psg_plus.edf"), &Settings::default());
        assert_eq!(text(&h, 168, 8), "16.01.yy");
        assert_eq!(text(&h, 88, 80), "Startdate 16-JAN-1924 X X X");
        // plain EDF: its free-text recording field blank; a date a two-digit year cannot hold left out
        let h = edf_header(&edf("plain.edf"), &Settings::default());
        assert_eq!((text(&h, 8, 80), text(&h, 88, 80)), ("X X X X".into(), String::new()));
        assert_eq!(text(&h, 168, 16), "  .  .    .  .");
        let h = edf_header(&edf("plain.edf"), &day);
        assert_eq!(text(&h, 168, 8), "01.03.99");
        // annotations: the time keeping kept, the texts gone
        let mut a = b"+0.25\x14\x14\x00+0.25\x1510\x14Sleep stage W\x14\x00+0.5\x14Lights off\x14\x00\x00\x00".to_vec();
        edf_annotations(&mut a);
        assert!(a.starts_with(b"+0.25\x14\x14\x00") && a[8..].iter().all(|&c| c == 0));
    }

    #[test]
    fn muse_and_wfdb_files() {
        let s = Settings { offset: Offset { years: 0, months: 0, days: 1, hours: 0 }, ..Default::default() };
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../viewers/ecg/tests/fixtures/synthetic_muse.xml");
        let raw = std::fs::read(path).unwrap();
        let x = muse_xml(&String::from_utf8_lossy(&raw), &s).unwrap();
        for gone in ["SYN000123", "TESTPATIENT", "<PatientFirstName>", "01-02-1970", "<Order>", "Chest pain"] {
            assert!(!x.contains(gone), "{gone} is still there");
        }
        assert!(x.contains("<PatientAge>47</PatientAge>") && x.contains("<Gender>FEMALE</Gender>"));
        assert!(
            x.contains("<AcquisitionDate>03-13-2020</AcquisitionDate>")
                && x.contains("<AcquisitionTime>08:15:30</AcquisitionTime>")
        );
        assert!(x.contains("<EditDate>03-14-2020</EditDate>"));
        assert!(x.contains("When compared with ECG of 31-DEC-2018"), "the statements' dates too");
        // with hours: the date and its time move together
        let h = Settings { offset: Offset { years: 0, months: 0, days: 0, hours: 9 }, ..Default::default() };
        let y = muse_xml(&String::from_utf8_lossy(&raw), &h).unwrap();
        assert!(
            y.contains("<AcquisitionDate>03-13-2020</AcquisitionDate>")
                && y.contains("<AcquisitionTime>23:15:30</AcquisitionTime>"),
            "{}",
            &y[..y.find("</TestDemographics>").unwrap_or(0)]
        );
        let hea =
            "e1 2 500 1000 08:00:00 02/01/2024\n# Patient: Doe\n# Age: 93\n# QT: 400 ms\ne1.dat 16 200 16 0 0 0 0 I\n";
        let w = wfdb_header(hea, &s);
        assert!(
            w.starts_with("e1 2 500 1000 08:00:00 01/01/2024\n")
                && !w.contains("Doe")
                && w.contains("# Age: 90+")
                && w.contains("# QT: 400 ms"),
            "{w}"
        );
        // answers: times under time keys (and only those), dates in text, ages
        let mut v = serde_json::json!({"time": 1_704_153_600.0, "n": 1_704_153_600u64, "start": 12, "age": 93, "label": "on 2024-01-02",
            "path": "/data/2024-01-02/x.dcm", "cells": [[0, 1, 1_704_153_600.0, 3, 0]], "t": [1_704_153_600.0]});
        answer(&mut v, &s);
        assert_eq!(v["time"], 1_704_067_200.0);
        assert_eq!(v["n"], 1_704_153_600u64, "not a time key");
        assert_eq!(v["start"], 12, "a small number under a time key is not a time");
        assert_eq!(
            (v["age"].clone(), v["label"].clone()),
            (serde_json::json!("90+"), serde_json::json!("on 2024-01-01"))
        );
        assert_eq!(v["path"], "/data/2024-01-02/x.dcm", "addresses are kept");
        assert_eq!(
            (v["cells"][0][2].clone(), v["t"][0].clone()),
            (serde_json::json!(1_704_067_200.0), serde_json::json!(1_704_067_200.0))
        );
    }

    #[test]
    fn dates_in_other_forms() {
        let day = 86400;
        // month first (as written in the United States), else day first when the month would be over 12
        assert_eq!(shift_text("seen 01/02/2024 at 08:00", day), "seen 01/01/2024 at 08:00");
        assert_eq!(shift_text("seen 1/2/2024 08:00", day + 3600), "seen 1/1/2024 07:00");
        assert_eq!(shift_text("on 25/12/2024", day), "on 24/12/2024");
        assert_eq!(shift_text_read("01/02/2024", day, true), "31/01/2024", "day first, as Medorama writes dates");
        assert_eq!(shift_text("2.1.2024", day), "1.1.2024");
        assert_eq!(shift_text("on 2024/01/02", day), "on 2024/01/01");
        assert_eq!(shift_text("Jan 2, 2024", day), "Jan 1, 2024");
        assert_eq!(shift_text("JANUARY 2 2024", day), "JANUARY 1 2024");
        assert_eq!(shift_text("2 January 2024 08:30", day), "1 January 2024 08:30");
        assert_eq!(shift_text("1 Mar 2024", day), "29 Feb 2024");
        // a century back: before 1970 too, in a recording's name
        let off = 36570 * day;
        assert_eq!(shift_text("2 January 2024", off), "18 November 1923");
        let name = |t: &str| format!("ICU7_12-{t}");
        let moved = shift_text(&name("1704182400"), off);
        assert_eq!(moved, name(&(1_704_182_400 - off).to_string()));
        assert!(moved.contains("--"), "{moved}");
        assert_eq!(shift_text(&moved, off), moved, "never twice");
        // not dates: versions, numbers, a Unix time on its own
        for kept in ["v1.2.2024.5", "1.2.3", "1704182400", "13/13/2024", "May be 2, 2024x", "Jan 2"] {
            assert_eq!(shift_text(kept, day), kept);
        }
    }

    #[test]
    fn no_text_breaks_the_date_reader() {
        // every string of up to four pieces among digits, date words and separators, with
        // characters of two to four bytes beside them: none may cut a character in two
        let pieces = [
            "1",
            "11",
            "2024",
            "24",
            "202403110830",
            "03",
            "/",
            ".",
            "-",
            " ",
            ":",
            "Mar",
            "Mar.",
            "March",
            "Sept",
            "2nd",
            "st",
            "°",
            "é",
            "µg",
            "€",
            "😀",
            "Ü",
            ",",
            "T",
        ];
        let n = pieces.len();
        let mut text = String::new();
        for a in 0..n {
            for b in 0..n {
                for c in 0..n {
                    for d in 0..n {
                        text.clear();
                        for k in [a, b, c, d] {
                            text.push_str(pieces[k]);
                        }
                        let _ = shift_text_read(&text, 86400, false);
                        let _ = shift_text_read(&text, 86400, true);
                    }
                }
            }
        }
        // the case that broke a page: a temperature after a date
        assert_eq!(shift_text_read("on 2024-03-11 37.1 °C", 86400, false), "on 2024-03-10 37.1 °C");
    }

    #[test]
    fn dates_in_more_forms() {
        let day = 86400;
        let both = |t: &str, month_first: &str, day_first: &str| {
            assert_eq!(shift_text_read(t, day, false), month_first, "{t}, month first");
            assert_eq!(shift_text_read(t, day, true), day_first, "{t}, day first");
        };
        // two-digit years: read in this century, written in full once moved
        both("seen 03/11/24.", "seen 03/10/2024.", "seen 02/11/2024.");
        both("11.03.24", "10.03.2024", "10.03.2024");
        both("11-MAR-24 08:50", "10-MAR-2024 08:50", "10-MAR-2024 08:50");
        // HL7 and DICOM date-times
        both("OBR|202403110830|", "OBR|202403100830|", "OBR|202403100830|");
        both("20240311083000.123+0100", "20240310083000.123+0100", "20240310083000.123+0100");
        // ordinals, abbreviations with a dot, September's four letters, a year first with dots
        both("on 2nd March 2024", "on 1st March 2024", "on 1st March 2024");
        both("March 2nd, 2024", "March 1st, 2024", "March 1st, 2024");
        both("Mar. 2, 2024", "Mar. 1, 2024", "Mar. 1, 2024");
        both("Sept 2, 2024 and Sept. 1, 2024", "Sept 1, 2024 and Aug. 31, 2024", "Sept 1, 2024 and Aug. 31, 2024");
        both("2024.03.11", "2024.03.10", "2024.03.10");
        // a month and its year, moved by a month
        let month = 31 * day;
        for (t, moved) in [("March 2024", "February 2024"), ("Mar 2024", "Feb 2024"), ("03/2024", "02/2024")] {
            assert_eq!(shift_text_read(t, month, false), moved, "{t}");
            assert_eq!(shift_text_read(t, month, true), moved, "{t}");
        }
        // a century back: no form keeps the real year
        let off = Settings::default().offset_seconds();
        for t in ["03/11/24", "11.03.24", "11-MAR-24", "202403110830", "2nd March 2024", "March 2024", "03/2024"] {
            let moved = shift_text(t, off);
            assert!(!moved.contains("2024") && moved.contains("192"), "{t}: {moved}");
        }
        // still not dates
        for kept in ["1.2.3", "v1.2.24", "120/80", "May be 2, 2024x", "13/2024", "1/3/245", "2024.5"] {
            assert_eq!(shift_text(kept, day), kept);
        }
    }

    #[test]
    fn identifiers_are_no_choice_and_withholding_less_is_weaker() {
        let s = Settings { fields: vec!["names".into(), "dates".into()], ..Default::default() };
        assert!(s.hides_column("first_name") && s.hides_column("mrn") && s.hides_column("birth_date"));
        assert!(!FIELDS.iter().any(|f| ["names", "mrn", "birth_date", "account_numbers"].contains(&f.0)));
        let before = Settings::default();
        assert!(!before.weaker_than(&before));
        let untick = Settings { fields: vec!["dates".into()], ..Default::default() };
        assert!(untick.weaker_than(&before), "age_over_89 unticked");
        assert!(Settings { spread_days: 100, ..Default::default() }.weaker_than(&before));
        let zero = Settings { offset: Offset { years: 0, months: 0, days: 0, hours: 0 }, ..Default::default() };
        assert!(zero.weaker_than(&before));
        let more = Settings {
            fields: vec!["dates".into(), "age_over_89".into(), "notes".into()],
            offset: Offset { years: 110, months: 0, days: 0, hours: 0 },
            ..Default::default()
        };
        assert!(!more.weaker_than(&before), "withholding more, a larger offset");
        // the documented offset is warned about
        assert!(start_warning(&before).is_some_and(|w| w.contains("default")));
        assert!(start_warning(&more).is_none());
    }

    #[test]
    fn a_new_installation_draws_its_offset() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config::new(&dir.path().join("home"), &dir.path().join("results"));
        let db = crate::db::Db::open(&cfg, true).unwrap();
        let c = db.get().unwrap();
        let s = load(&c).unwrap();
        assert!((80..=120).contains(&s.offset.years) && s.offset != Offset::default(), "{:?}", s.offset);
        assert_eq!(load(&c).unwrap().offset, s.offset, "drawn once, kept");
        // an admin without the PHI permission may not withhold less
        let admin = User { phi: false, phi_allowed: false, ..User::local() };
        let weaker = Settings { offset: Offset { years: 1, months: 0, days: 0, hours: 0 }, ..s.clone() };
        assert!(save(&c, &admin, &weaker).is_err());
        assert!(save(&c, &User::local(), &weaker).is_ok());
    }

    #[test]
    fn pictures_and_documents_lose_their_texts() {
        let x = "<svg><title>Zyxwortha</title><!-- MRN83651927 --><desc>for Q.</desc><rect width=\"3\"/>\
                 <text x=\"1\">Zyxwortha <tspan>Q</tspan></text><metadata><a>b</a></metadata><title/></svg>";
        assert_eq!(svg(x), "<svg><rect width=\"3\"/></svg>");
        let mut pdf = b"%PDF-1.4\n1 0 obj << /Author (Zyx \\(Q\\) wortha) /Title <5A79> /Titles (kept) >> endobj\n\
            <x:xmpmeta><dc:creator><rdf:li>Zyxwortha</rdf:li></dc:creator></x:xmpmeta>"
            .to_vec();
        let len = pdf.len();
        pdf_info(&mut pdf);
        let t = String::from_utf8_lossy(&pdf).to_string();
        assert_eq!(pdf.len(), len, "offsets kept");
        assert!(!t.contains("Zyx") && !t.contains("wortha") && !t.contains("5A79"), "{t}");
        assert!(t.contains("/Titles (kept)") && t.contains("<rdf:li>") && t.contains("<2020>"), "{t}");
    }

    #[test]
    fn times_under_a_plug_ins_own_keys_move() {
        let s = Settings::default();
        let off = s.offset_seconds() as f64;
        let mut v = serde_json::json!({"onset": 1_704_110_400.0, "ms": 1_704_110_400_000.0, "time": 1_704_110_400.0,
                                       "beats": 5_000_000_000_000.0, "n": 72, "nested": [{"peak_at": 1_704_110_401.0}]});
        shift_unknown_times(&mut v, &s);
        assert_eq!(v["onset"], 1_704_110_400.0 - off);
        assert_eq!(v["ms"], 1_704_110_400_000.0 - off * 1000.0);
        assert_eq!(v["nested"][0]["peak_at"], 1_704_110_401.0 - off);
        assert_eq!(v["time"], 1_704_110_400.0, "a key the answers' gate knows: moved there, once");
        assert_eq!(
            (v["n"].clone(), v["beats"].clone()),
            (serde_json::json!(72), serde_json::json!(5_000_000_000_000.0))
        );
    }

    #[test]
    fn muse_variants_and_wfdb_keys() {
        let s = Settings::default();
        for open in ["<PatientDemographics>", "<PatientDemographics >", "<PatientDemographics xmlns=\"x\">"] {
            let x = format!("<?xml version=\"1.0\"?><RestingECG>{open}<PatientID>MRN83651927</PatientID><PatientLastName>ZYXWORTHA</PatientLastName><DateofBirth>05-17-1931</DateofBirth><PatientAge>92</PatientAge></PatientDemographics></RestingECG>");
            let out = muse_xml(&x, &s).unwrap();
            for gone in ["ZYXWORTHA", "MRN83651927", "1931"] {
                assert!(!out.contains(gone), "{open}: {gone}: {out}");
            }
            assert!(out.contains("<PatientAge>90+</PatientAge>"), "{out}");
        }
        // a patient's name outside the demographics: replaced
        let out =
            muse_xml("<RestingECG><Other><PatientLastName>ZYX</PatientLastName></Other></RestingECG>", &s).unwrap();
        assert!(!out.contains("ZYX") && out.contains("[Patient Name]"), "{out}");
        // what cannot be rewritten is refused, not sent
        for bad in [
            "<RestingECG><patientdemographics><PatientID>X1</PatientID></patientdemographics></RestingECG>",
            "<RestingECG><PatientDemographics><PatientID>X1</PatientID></RestingECG>",
            "<RestingECG><Extra><PatientID>X1</PatientID></Extra></RestingECG>",
        ] {
            assert!(muse_xml(bad, &s).is_err(), "{bad}");
        }
        let hea = "rec 1 250 5000 08:00:00 02/01/2024\nrec.dat 16 200 12 0 0 0 0 II\n# Name=Zyxwortha\n# Patient name - Quilbert\n# Patient_Name: Zyxwortha\n# Birthdate: 1931-05-17\n# Date-of-Birth: 1931-05-17\n# Confirmed_By: Osei\n# QT: 400 ms\n";
        let out = wfdb_header(hea, &s);
        for gone in ["Zyxwortha", "Quilbert", "1931", "Osei"] {
            assert!(!out.contains(gone), "{gone}: {out}");
        }
        assert!(out.contains("# Confirmed_By: [Staff Name]") && out.contains("# QT: 400 ms"), "{out}");
    }

    #[test]
    fn offset_and_text() {
        let s = Settings::default();
        assert_eq!(s.offset_seconds(), 36570 * 86400);
        let o = Settings { offset: Offset { years: 1, months: 1, days: 1, hours: 1 }, ..Default::default() };
        // 2000-01-01 back 1 year 1 month: 1998-12-01 (396 days), 1 day, 1 hour
        assert_eq!(o.offset_seconds(), (397 * 24 + 1) * 3600);
        assert_eq!(
            shift_text("on 2024-03-12, 2024-03-12 14:00 and 2024-03-12T14:00:05Z", 86400 + 3600),
            "on 2024-03-10, 2024-03-11 13:00 and 2024-03-11T13:00:05Z"
        );
        // leap days are counted
        assert_eq!(shift_text("2024-03-01", 2 * 86400), "2024-02-28");
        // a bedside-monitor recording name, built here (the repository holds no real-looking recording names)
        let name = |t: i64| format!("ICU7_12-{t}");
        assert_eq!(shift_text(&name(1_703_851_200), 100), name(1_703_851_100));
        assert_eq!(shift_text("12024-03-01 v2024-03-011 2024-13-01", 86400), "12024-03-01 v2024-03-011 2024-13-01");
        assert_eq!(s.age(90), "90+");
        assert_eq!(s.age(89), "89");
        assert!(
            s.hides_column("first_name") && !s.hides_column("care_unit") && !s.hides_column("text"),
            "free text shown by default"
        );
        let none = Settings { fields: vec![], ..Default::default() };
        assert_eq!(none.shift_text("2024-03-01"), "2024-03-01");
        assert_eq!(none.shift(10.0), 10.0);
    }
}
