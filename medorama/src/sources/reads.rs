//! A criterion's reads of a connected database (docs/performance.md, "What a criterion
//! reads"): the numbers of a measurement, the persons who have data of a category, the entries
//! a filter finds.  Each answer is kept, in memory, while the database's data version stays
//! the same (its `version` view): a search repeated, or a cohort edited one criterion at a
//! time, reads only what changed.  A database without a version is read each time.  What is
//! kept is the database's own answer, the same for every user: linking, dates as shown,
//! Privacy Mode and grants are applied to it afterwards, by each search.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::{Entry, Find, Source, ValueRow};

/// What a criterion asks a database for.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    /// A measurement's numbers (`Source::find_values`).
    Values(Find),
    /// The persons who have data of a category (`Source::persons_with`).
    Persons(String),
    /// The entries a filter finds, at most `max_rows` (`Source::find`).
    Entries(Find),
}

/// A database's answer to a [`Read`].
#[derive(Debug, Clone)]
pub enum Answer {
    Values(Arc<Vec<ValueRow>>),
    Persons(Arc<Vec<String>>),
    Entries(Arc<Vec<Entry>>),
}

impl Answer {
    fn rows(&self) -> usize {
        match self {
            Answer::Values(v) => v.len(),
            Answer::Persons(v) => v.len(),
            // (an entry holds its texts: as several numbers)
            Answer::Entries(v) => v.len() * 4,
        }
    }
}

/// The rows kept in all (the answers least recently used go first); an answer larger than
/// this is not kept.
pub const KEPT_ROWS: usize = 4_000_000;

struct Kept {
    map: Mutex<HashMap<String, (u64, Answer)>>,
    tick: AtomicU64,
}

fn kept() -> &'static Kept {
    static K: OnceLock<Kept> = OnceLock::new();
    K.get_or_init(|| Kept { map: Mutex::new(HashMap::new()), tick: AtomicU64::new(0) })
}

/// Forget what was kept of a source (removed, or its settings changed): its keys begin with
/// its id, as its pools' do.
pub(super) fn forget(prefix: &str) {
    kept().map.lock().unwrap().retain(|k, _| !k.starts_with(prefix));
}

impl Source {
    /// A read's key: the database (as connected), its time zone and the read.
    pub fn read_key(&self, r: &Read, zone: chrono_tz::Tz) -> String {
        format!("{}\u{1}{zone}\u{1}{r:?}", self.pool_key())
    }

    /// The database's answer to a read: as kept while its data version is the same, else read
    /// now (and kept, when it has a version).
    pub fn read(&self, r: &Read, zone: chrono_tz::Tz) -> anyhow::Result<Answer> {
        self.read_at(r, zone, &self.version()?)
    }

    /// [`Source::read`], the data's version read already (a search reads it once).
    pub fn read_at(&self, r: &Read, zone: chrono_tz::Tz, version: &str) -> anyhow::Result<Answer> {
        let key = format!("{}\u{1}{version}", self.read_key(r, zone));
        let k = kept();
        if !version.is_empty() {
            let mut map = k.map.lock().unwrap();
            if let Some((t, a)) = map.get_mut(&key) {
                *t = k.tick.fetch_add(1, Ordering::Relaxed);
                return Ok(a.clone());
            }
        }
        let a = self.read_now(r, zone)?;
        if !version.is_empty() && a.rows() <= KEPT_ROWS {
            let mut map = k.map.lock().unwrap();
            // a version newer than a kept answer's: that answer is no longer the data's
            let stale = format!("{}\u{1}", self.read_key(r, zone));
            map.retain(|x, _| !x.starts_with(&stale));
            let mut total: usize = map.values().map(|(_, a)| a.rows()).sum::<usize>() + a.rows();
            while total > KEPT_ROWS {
                let Some(oldest) = map.iter().min_by_key(|(_, (t, _))| *t).map(|(x, _)| x.clone()) else { break };
                total -= map.remove(&oldest).map(|(_, a)| a.rows()).unwrap_or(0);
            }
            map.insert(key, (k.tick.fetch_add(1, Ordering::Relaxed), a.clone()));
        }
        Ok(a)
    }

    fn read_now(&self, r: &Read, zone: chrono_tz::Tz) -> anyhow::Result<Answer> {
        Ok(match r {
            Read::Values(f) => {
                let mut v = vec![];
                self.find_values(f, zone, |row| {
                    v.push(row);
                    Ok(())
                })?;
                Answer::Values(Arc::new(v))
            }
            Read::Persons(category) => Answer::Persons(Arc::new(self.persons_with(category, zone)?)),
            Read::Entries(f) => Answer::Entries(Arc::new(self.find(f, zone)?)),
        })
    }
}
