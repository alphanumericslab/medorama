//! Grants (docs/design/databases-only.md, "Access: grants"): who may use each connected
//! database and each cohort.
//!
//! A grant is (user, object, permission), the object `db:ID` or `cohort:ID`.  On a database:
//! `search` (criteria find its patients), `view` (its data on patient pages; implies search),
//! `phi` (its identifiers, outside Privacy Mode; implies view), `manage` (refresh, grant,
//! remove; implies phi).  On a cohort: `view`, `edit` (implies view), `manage` (share, remove;
//! implies edit).  The server unwraps a database's key only for a request that holds a grant on
//! it.  The person at the terminal (`User::local`) holds every grant.

use crate::db::{audit, now};
use crate::sql::{params, Connection};
use crate::users::User;

/// The permissions of a database, weakest first: each implies the ones before it.
pub const DATABASE: &[&str] = &["search", "view", "phi", "manage"];
/// The permissions of a cohort, weakest first.
pub const COHORT: &[&str] = &["view", "edit", "manage"];

fn chain(object: &str) -> &'static [&'static str] {
    if object.starts_with("cohort:") {
        COHORT
    } else {
        DATABASE
    }
}

/// The permissions that imply `permission` on `object` (itself and the stronger ones).
fn implying(object: &str, permission: &str) -> Vec<&'static str> {
    let c = chain(object);
    match c.iter().position(|p| *p == permission) {
        Some(i) => c[i..].to_vec(),
        None => vec![],
    }
}

fn quoted(perms: &[&str]) -> String {
    perms.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(", ")
}

/// Whether `user` may do `permission` on `object`.
pub fn can(c: &Connection, user: &User, object: &str, permission: &str) -> anyhow::Result<bool> {
    if user.id == 0 {
        return Ok(true);
    }
    let perms = implying(object, permission);
    if perms.is_empty() {
        return Ok(false);
    }
    // (the permissions are this module's own words, never typed text)
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM grants WHERE user_id = ?1 AND object = ?2 AND permission IN ({}))",
        quoted(&perms)
    );
    Ok(c.query_row(&sql, params![user.id, object], |r| r.get::<_, bool>(0))?)
}

/// [`can`], or an error naming what is missing.
pub fn require(c: &Connection, user: &User, object: &str, permission: &str) -> anyhow::Result<()> {
    if can(c, user, object, permission)? {
        return Ok(());
    }
    anyhow::bail!("no {permission} permission on {object}")
}

/// The ids of the objects of a kind (`db`, `cohort`) `user` may `permission`, or none for every
/// one (the person at the terminal).
pub fn objects(c: &Connection, user: &User, kind: &str, permission: &str) -> anyhow::Result<Option<Vec<i64>>> {
    if user.id == 0 {
        return Ok(None);
    }
    let prefix = format!("{kind}:");
    let perms = implying(&prefix, permission);
    if perms.is_empty() {
        return Ok(Some(vec![]));
    }
    let mut st = c.prepare(&format!(
        "SELECT DISTINCT object FROM grants WHERE user_id = ?1 AND permission IN ({})",
        quoted(&perms)
    ))?;
    let objs: Vec<String> = st.query_map([user.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut ids: Vec<i64> =
        objs.iter().filter_map(|o| o.strip_prefix(&prefix)).filter_map(|id| id.parse().ok()).collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(Some(ids))
}

/// The databases `user` may `permission`, of those given.
pub fn databases(c: &Connection, user: &User, permission: &str, of: &[i64]) -> anyhow::Result<Vec<i64>> {
    Ok(match objects(c, user, "db", permission)? {
        None => of.to_vec(),
        Some(ids) => of.iter().copied().filter(|i| ids.contains(i)).collect(),
    })
}

/// Grant `permission` on `object` to a user (who manages the object).
pub fn grant(c: &Connection, by: &User, user_id: i64, object: &str, permission: &str) -> anyhow::Result<()> {
    if !chain(object).contains(&permission) {
        anyhow::bail!("{permission}: not a permission of {object} ({})", chain(object).join(", "));
    }
    require(c, by, object, "manage")?;
    c.execute(
        "INSERT INTO grants (user_id, object, permission, granted_by, granted_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT DO NOTHING",
        params![user_id, object, permission, (by.id > 0).then_some(by.id), now()],
    )?;
    let name = crate::users::get(c, user_id)?.map(|u| u.username).unwrap_or_else(|| format!("user {user_id}"));
    audit(c, Some(by), "grant", object, &format!("{permission} to {name}"));
    Ok(())
}

/// Grant every permission of `object` to its maker (connecting a database, making a cohort).
pub fn grant_all(c: &Connection, user: &User, object: &str) -> anyhow::Result<()> {
    if user.id == 0 {
        return Ok(());
    }
    for p in chain(object) {
        c.execute(
            "INSERT INTO grants (user_id, object, permission, granted_by, granted_at) VALUES (?1, ?2, ?3, ?1, ?4)
             ON CONFLICT DO NOTHING",
            params![user.id, object, p, now()],
        )?;
    }
    Ok(())
}

/// Revoke a permission (and the stronger ones it would leave meaningless) on `object`.
pub fn revoke(c: &Connection, by: &User, user_id: i64, object: &str, permission: &str) -> anyhow::Result<()> {
    require(c, by, object, "manage")?;
    if by.id == user_id && implying(object, permission).contains(&"manage") {
        anyhow::bail!("you cannot revoke your own manage permission");
    }
    for p in implying(object, permission) {
        c.execute(
            "DELETE FROM grants WHERE user_id = ?1 AND object = ?2 AND permission = ?3",
            params![user_id, object, p],
        )?;
    }
    let name = crate::users::get(c, user_id)?.map(|u| u.username).unwrap_or_else(|| format!("user {user_id}"));
    audit(c, Some(by), "revoke", object, &format!("{permission} from {name}"));
    Ok(())
}

/// Every grant on an object, for whoever manages it: (user id, username, permission).
pub fn of_object(c: &Connection, object: &str) -> anyhow::Result<Vec<(i64, String, String)>> {
    let mut st = c.prepare(
        "SELECT g.user_id, u.username, g.permission FROM grants g JOIN users u ON u.id = g.user_id
         WHERE g.object = ?1 ORDER BY u.username, g.permission",
    )?;
    let v = st.query_map([object], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// The strongest permission a user holds on an object, if any.
pub fn strongest(c: &Connection, user: &User, object: &str) -> anyhow::Result<Option<&'static str>> {
    for p in chain(object).iter().rev() {
        if can(c, user, object, p)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Remove every grant on an object (it was removed).
pub fn remove_object(c: &Connection, object: &str) -> anyhow::Result<()> {
    c.execute("DELETE FROM grants WHERE object = ?1", [object])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_permissions_imply_weaker() {
        assert_eq!(implying("db:1", "search"), vec!["search", "view", "phi", "manage"]);
        assert_eq!(implying("db:1", "phi"), vec!["phi", "manage"]);
        assert_eq!(implying("cohort:2", "view"), vec!["view", "edit", "manage"]);
        assert!(implying("cohort:2", "phi").is_empty());
    }
}
