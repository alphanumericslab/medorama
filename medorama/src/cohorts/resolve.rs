//! What names in criteria text refer to: a connected database, or a measurement of one, read
//! from the variables Medorama keeps of each database (variables.rs), offline ones included.

use super::*;

/// What names in criteria text refer to (expr.rs): a database (`omop.` alone), or a
/// measurement of that name in one database; never several databases at once.  The databases
/// are those the user may search; their variables are read once per resolver (one text
/// parsed), not per name.
pub fn resolver<'a>(
    c: &'a Connection,
    cfg: &'a Config,
    user: &User,
) -> impl FnMut(Option<&str>, &str) -> anyhow::Result<Criterion> + 'a {
    let user = user.clone();
    let mut dbs: Option<Vec<(i64, String)>> = None;
    let mut names: HashMap<i64, Result<Vec<(String, String, bool)>, String>> = HashMap::new();
    move |qual: Option<&str>, name: &str| {
        if dbs.is_none() {
            let granted = crate::databases::granted(c, &user, "search")?;
            dbs = Some(
                crate::sources::list(c)?
                    .into_iter()
                    .filter(|s| granted.contains(&s.id))
                    .map(|s| (s.id, s.name))
                    .collect(),
            );
        }
        let dbs = dbs.as_ref().unwrap();
        // an action's result (`hrv.rmssd`): its name holds the dot, and no database is named so
        let whole;
        let (qual, name) = match qual {
            Some(q)
                if !name.is_empty()
                    && !dbs.iter().any(|d| d.1.to_lowercase().starts_with(&q.trim().to_lowercase())) =>
            {
                whole = format!("{}.{name}", q.trim());
                (None, whole.as_str())
            }
            _ => (qual, name),
        };
        let pick: Vec<&(i64, String)> = match qual {
            Some(q) => {
                let q = q.trim().to_lowercase();
                let exact: Vec<_> = dbs
                    .iter()
                    .filter(|d| d.1.to_lowercase() == q || d.0.to_string() == q.trim_start_matches('#'))
                    .collect();
                if !exact.is_empty() {
                    exact
                } else {
                    dbs.iter().filter(|d| d.1.to_lowercase().starts_with(&q)).collect()
                }
            }
            None => dbs.iter().collect(),
        };
        if let Some(q) = qual {
            if pick.is_empty() {
                anyhow::bail!(
                    "no database {q:?} (databases: {})",
                    dbs.iter().map(|d| d.1.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
            if name.is_empty() {
                if pick.len() > 1 {
                    anyhow::bail!(
                        "{q:?} names several databases: {}",
                        pick.iter().map(|d| d.1.as_str()).collect::<Vec<_>>().join(", ")
                    );
                }
                return Ok(Criterion { kind: "database".into(), database: Some(pick[0].0), ..Default::default() });
            }
        }
        // a measurement: a name a database holds, else the one name containing it
        let mut hits: Vec<(i64, String)> = vec![];
        let mut unread = vec![];
        for (id, dname) in &pick {
            let held = match names
                .entry(*id)
                .or_insert_with(|| crate::variables::names(cfg, c, *id).map_err(|e| e.to_string()))
            {
                Ok(n) => n.clone(),
                Err(e) => {
                    unread.push(format!("{dname}: {e}"));
                    continue;
                }
            };
            let w = name.trim().to_lowercase();
            let numeric: Vec<&String> = held.iter().filter(|n| n.2).map(|n| &n.1).collect();
            if let Some(n) = numeric.iter().find(|n| n.to_lowercase() == w) {
                hits.push((*id, n.to_string()));
                continue;
            }
            let mut like: Vec<&String> = numeric.into_iter().filter(|n| n.to_lowercase().contains(&w)).collect();
            like.dedup();
            match like.len() {
                0 => {}
                1 => hits.push((*id, like[0].clone())),
                n => anyhow::bail!(
                    "{name:?} names {n} measurements in {dname}: {}{}; write which, e.g. {dname}.\"{}\"",
                    like.iter().take(6).map(|x| format!("“{x}”")).collect::<Vec<_>>().join(", "),
                    if n > 6 { " …" } else { "" },
                    like[0]
                ),
            }
        }
        let label = |i: i64| dbs.iter().find(|d| d.0 == i).map(|d| d.1.clone()).unwrap_or_default();
        match hits.len() {
            0 => anyhow::bail!(
                "nothing is named {name:?}{}{}",
                qual.map(|q| format!(" in {q}")).unwrap_or_default(),
                if unread.is_empty() { String::new() } else { format!(" ({} could not be read)", unread.join("; ")) }
            ),
            1 => Ok(Criterion {
                kind: "measurement".into(),
                name: hits[0].1.clone(),
                database: Some(hits[0].0),
                ..Default::default()
            }),
            _ => anyhow::bail!(
                "{name:?} is in several databases ({}): write which, e.g. {}.{name}",
                hits.iter().map(|h| label(h.0)).collect::<Vec<_>>().join(", "),
                label(hits[0].0)
            ),
        }
    }
}

/// Criteria text as the structured query (POST cohorts/parse shows it while it is written).
pub fn parse_text(c: &Connection, cfg: &Config, user: &User, text: &str) -> anyhow::Result<Expr> {
    let mut r = resolver(c, cfg, user);
    crate::expr::parse(text, &mut r)
}
