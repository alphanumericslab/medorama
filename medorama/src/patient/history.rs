//! The patients each user has looked at (the History tab), latest first.

use super::*;

#[derive(Debug, Serialize)]
pub struct Seen {
    pub patient: i64,
    pub pseudonym: String,
    pub first_seen: f64,
    pub last_seen: f64,
    pub views: i64,
}

/// A user looked at a patient's page.
pub fn seen(c: &Connection, user: &User, patient: i64) -> anyhow::Result<()> {
    if user.id == 0 {
        return Ok(());
    }
    let t = crate::db::now();
    c.execute(
        "INSERT INTO history (user_id, patient, first_seen, last_seen, views) VALUES (?1, ?2, ?3, ?3, 1)
         ON CONFLICT (user_id, patient) DO UPDATE SET last_seen = excluded.last_seen, views = history.views + 1",
        params![user.id, patient, t],
    )?;
    Ok(())
}

/// The patients a user looked at, latest first (those they may still see).
pub fn history(c: &Connection, user: &User, limit: i64) -> anyhow::Result<Vec<Seen>> {
    let mut st = c.prepare(
        "SELECT h.patient, COALESCE(p.pseudonym, ''), h.first_seen, h.last_seen, h.views FROM history h
         JOIN patients p ON p.id = h.patient WHERE h.user_id = ?1 ORDER BY h.last_seen DESC LIMIT ?2",
    )?;
    let rows: Vec<Seen> = st
        .query_map(params![user.id, limit], |r| {
            Ok(Seen {
                patient: r.get(0)?,
                pseudonym: r.get(1)?,
                first_seen: r.get(2)?,
                last_seen: r.get(3)?,
                views: r.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows.into_iter().filter(|s| patient_exists(c, user, s.patient).is_ok()).collect())
}

/// Forget one patient of a user's history, or all of it.
pub fn forget(c: &Connection, user: &User, patient: Option<i64>) -> anyhow::Result<usize> {
    Ok(match patient {
        Some(p) => c.execute("DELETE FROM history WHERE user_id = ?1 AND patient = ?2", [user.id, p])?,
        None => c.execute("DELETE FROM history WHERE user_id = ?1", [user.id])?,
    })
}
