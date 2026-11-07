//! How the page writes dates and times (Admin › Dates and times): the order of a date's parts
//! and the clock, for the whole catalog.  The page applies it to every date it shows; data
//! (exports, the API) and the server's own sentences keep ISO 8601, which every program reads
//! and Privacy Mode recognises when it moves dates written in text.

use crate::db::{audit, now};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use serde::{Deserialize, Serialize};

/// (key, example, label)
pub const DATES: &[(&str, &str, &str)] = &[
    ("ymd", "2024-01-31", "Year-month-day (ISO 8601)"),
    ("mdy", "01/31/2024", "Month/day/year (United States)"),
    ("dmy", "31/01/2024", "Day/month/year (United Kingdom, France, …)"),
    ("dmy_dot", "31.01.2024", "Day.month.year (Germany, …)"),
];
pub const CLOCKS: &[(&str, &str, &str)] = &[("24h", "14:05", "24-hour"), ("12h", "2:05 PM", "12-hour")];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Display {
    pub date: String,
    pub clock: String,
}

impl Default for Display {
    fn default() -> Self {
        Display { date: "ymd".into(), clock: "24h".into() }
    }
}

pub fn load(c: &Connection) -> anyhow::Result<Display> {
    let v: Option<String> =
        c.query_row("SELECT value FROM settings WHERE key = 'display'", [], |r| r.get(0)).optional()?;
    Ok(v.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default())
}

/// Admins only; audited.
pub fn save(c: &Connection, user: &User, d: &Display) -> anyhow::Result<Display> {
    if !user.is_admin() {
        anyhow::bail!("only admins change how dates and times are shown");
    }
    if !DATES.iter().any(|x| x.0 == d.date) {
        anyhow::bail!("date: one of {}", DATES.iter().map(|x| x.0).collect::<Vec<_>>().join(", "));
    }
    if !CLOCKS.iter().any(|x| x.0 == d.clock) {
        anyhow::bail!("clock: 24h or 12h");
    }
    let text = serde_json::to_string(d)?;
    c.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ('display', ?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![text, (user.id > 0).then_some(user.id), now()],
    )?;
    audit(c, Some(user), "display_changed", &d.date, &d.clock);
    Ok(d.clone())
}

/// The setting and its choices, for the page.
pub fn describe(d: &Display) -> serde_json::Value {
    let list = |xs: &[(&str, &str, &str)]| {
        xs.iter().map(|(k, e, l)| serde_json::json!({"key": k, "example": e, "label": l})).collect::<Vec<_>>()
    };
    serde_json::json!({ "date": d.date, "clock": d.clock, "dates": list(DATES), "clocks": list(CLOCKS) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_checked() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_by INTEGER, updated_at REAL);
             CREATE TABLE audit (id INTEGER PRIMARY KEY, time REAL, user_id INTEGER, username TEXT, event TEXT, target TEXT, detail TEXT);",
        )
        .unwrap();
        assert_eq!(load(&c).unwrap(), Display::default());
        let admin = User::local();
        let d = Display { date: "dmy".into(), clock: "12h".into() };
        save(&c, &admin, &d).unwrap();
        assert_eq!(load(&c).unwrap(), d);
        assert!(save(&c, &admin, &Display { date: "yyyy".into(), clock: "24h".into() }).is_err());
        assert!(save(&c, &admin, &Display { date: "ymd".into(), clock: "25h".into() }).is_err());
    }
}
