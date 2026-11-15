//! A patient's data, read now from the connected databases the user may view: their entries
//! (as items), their visits, their stays in units, and their recordings, and the report entry
//! each item belongs to.

use super::*;

/// A patient's items as the journey and the report use them.
pub(super) struct It {
    pub(super) id: i64,
    pub(super) patient: i64,
    /// Its visit ([`Visit::id`]), when the database says.
    pub(super) encounter: Option<i64>,
    pub(super) category: String,
    pub(super) name: String,
    pub(super) code: String,
    pub(super) time: Option<f64>,
    pub(super) end: Option<f64>,
    pub(super) num: Option<f64>,
    pub(super) text: String,
    pub(super) unit: String,
    pub(super) level: u8,
    /// Below its range (else above, when abnormal).
    pub(super) flag_low: bool,
    pub(super) author: String,
    pub(super) service: String,
    /// Its file: the recording (its number) whose key is the entry's followed by `:file`.
    pub(super) file: Option<i64>,
    pub(super) flag: String,
    pub(super) ref_low: Option<f64>,
    pub(super) ref_high: Option<f64>,
    pub(super) vocabulary: String,
    pub(super) live: Option<Live>,
    /// Its calendar day in Medorama's time zone (days since 1970-01-01 there): a day's vital
    /// signs are one report entry, the day the page shows.
    pub(super) day: Option<i64>,
}

/// Where an entry comes from, and its token (its key never leaves the server).
#[derive(Clone)]
pub(super) struct Live {
    pub(super) connection: i64,
    pub(super) source: String,
    pub(super) token: String,
}

/// A visit of the patient in a connected database.
#[derive(Debug, Clone, Serialize)]
pub struct Visit {
    /// Its number in this answer (items point to it).
    pub id: i64,
    #[serde(skip)]
    pub database: i64,
    pub source: String,
    /// Its token (`enc:<database>:<token>`): its key never leaves the server.
    pub key: String,
    /// What Privacy Mode names it by (pseudonyms.rs).
    pub pseudonym: String,
    #[serde(skip)]
    pub visit_key: String,
    pub admit: Option<f64>,
    pub discharge: Option<f64>,
    pub unit: String,
    pub icu: bool,
}

/// A stay of the patient in a unit during a visit (a bed stay, OMOP's `visit_detail`).
#[derive(Debug, Clone)]
pub(super) struct Stay {
    /// Its visit ([`Visit::id`]), when the database names one it gave.
    pub(super) visit: Option<i64>,
    pub(super) start: Option<f64>,
    pub(super) end: Option<f64>,
    pub(super) icu: bool,
}

/// Everything the page shows of a patient.
pub(super) struct Data {
    pub(super) items: Vec<It>,
    pub(super) allergies: Vec<It>,
    pub(super) visits: Vec<Visit>,
    /// Their stays in units, when the databases give them (the ICU column draws the
    /// intensive-care ones, else the whole visit).
    pub(super) stays: Vec<Stay>,
    pub(super) recordings: Vec<crate::recordings::Recording>,
    /// The recordings that are an entry's file: shown with their entry, not on their own.
    pub(super) attached: HashSet<i64>,
    /// The databases linked to the patient that did not answer.
    pub(super) unavailable: Vec<String>,
    /// Whether a database the user may view holds allergies at all.
    pub(super) allergies_recorded: bool,
}

/// A token for a database's key: keyed with the gate's secret, the same at every request.
pub(super) fn token(secret: &[u8], db: i64, key: &str) -> String {
    let mut h = sha2::Sha256::new();
    h.update(secret);
    h.update(format!("{db}:{key}").as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A patient's data, from every database linked to them that `user` may view (a database that
/// does not answer is left out: `unavailable` says which).
pub(super) fn load(cfg: &Config, c: &Connection, user: &User, patient: i64) -> anyhow::Result<Data> {
    let viewable = crate::databases::granted(c, user, "view")?;
    let secret = crate::gate::secret(c)?;
    let mut d = Data {
        items: vec![],
        allergies: vec![],
        visits: vec![],
        stays: vec![],
        recordings: vec![],
        attached: HashSet::new(),
        unavailable: vec![],
        allergies_recorded: false,
    };
    for (db, keys) in crate::persons::persons_of(cfg, c, patient, Some(&viewable))? {
        let Some(s) = crate::sources::get(c, db)? else { continue };
        d.allergies_recorded |= crate::variables::holds_category(cfg, c, db, "allergy").unwrap_or(false);
        let zone = s.zone(&cfg.timezone);
        let shown = crate::util::tz(&cfg.timezone);
        let (entries, visits) = match s.entries(&keys, zone).and_then(|e| Ok((e, s.visits(&keys, zone)?))) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("medorama: {}: {e}", s.name);
                d.unavailable.push(s.name.clone());
                continue;
            }
        };
        // stays are optional: a database whose stays cannot be read is shown without them
        let stays = s.stays(&keys, zone).unwrap_or_else(|e| {
            eprintln!("medorama: {}: stays: {e}", s.name);
            vec![]
        });
        // an entry's file is the recording keyed `<entry key>:file` (db-builder/docs/schema.md, Keys)
        let recordings = if s.has_recordings() { crate::recordings::of_persons(cfg, c, &s, &keys)? } else { vec![] };
        let file_of: HashMap<&str, i64> =
            recordings.iter().filter_map(|r| Some((r.key.strip_suffix(":file")?, r.id))).collect();
        let mut visit_of: HashMap<String, i64> = HashMap::new();
        let icu_visits: HashSet<&str> = stays.iter().filter(|x| x.icu).map(|x| x.visit.as_str()).collect();
        for v in visits {
            let id = d.visits.len() as i64 + 1;
            let icu = v.icu || categories::is_icu_unit(&v.unit) || icu_visits.contains(v.key.as_str());
            let t = token(&secret, db, &v.key);
            visit_of.insert(v.key.clone(), id);
            d.visits.push(Visit {
                id,
                database: db,
                source: s.name.clone(),
                key: format!("enc:{db}:{t}"),
                pseudonym: crate::pseudonyms::of_visit(c, db, &t)?,
                visit_key: v.key,
                admit: v.start,
                discharge: v.end,
                icu,
                unit: v.unit,
            });
        }
        for x in &stays {
            d.stays.push(Stay { visit: visit_of.get(&x.visit).copied(), start: x.start, end: x.end, icu: x.icu });
        }
        for e in entries {
            let note = e.category == "note";
            let text = match (note, e.text.trim().is_empty()) {
                (true, false) => e.text.clone(),
                (false, false) if !e.value_text.is_empty() => format!("{}\n{}", e.value_text, e.text),
                (false, false) => e.text.clone(),
                _ => e.value_text.clone(),
            };
            let n = d.items.len() + d.allergies.len();
            let it = It {
                id: -(n as i64) - 1,
                patient,
                encounter: visit_of.get(&e.visit).copied(),
                level: level(&e.flag, e.value_num, e.ref_low, e.ref_high),
                flag_low: e.flag.trim().to_lowercase().starts_with('l')
                    || matches!((e.value_num, e.ref_low), (Some(v), Some(l)) if v < l),
                category: e.category.clone(),
                name: e.name,
                code: e.code,
                time: e.time,
                end: e.end,
                num: e.value_num,
                text,
                unit: e.unit,
                author: e.author,
                service: String::new(),
                file: file_of.get(e.key.as_str()).copied(),
                flag: e.flag,
                ref_low: e.ref_low,
                ref_high: e.ref_high,
                vocabulary: e.vocabulary,
                live: Some(Live { connection: db, source: s.name.clone(), token: token(&secret, db, &e.key) }),
                day: e.time.map(|t| day_in(t, shown)),
            };
            d.attached.extend(it.file);
            if e.category == "allergy" {
                d.allergies.push(it);
            } else {
                d.items.push(it);
            }
        }
        for mut r in recordings {
            r.patient = Some(patient);
            d.recordings.push(r);
        }
    }
    d.items.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    d.visits.sort_by(|a, b| b.admit.partial_cmp(&a.admit).unwrap_or(std::cmp::Ordering::Equal));
    d.recordings.sort_by(|a, b| a.t_start.partial_cmp(&b.t_start).unwrap_or(std::cmp::Ordering::Equal));
    Ok(d)
}

/// The calendar day of `t` (Unix seconds) in `zone`, as days since 1970-01-01.
pub(super) fn day_in(t: f64, zone: chrono_tz::Tz) -> i64 {
    use chrono::TimeZone;
    match zone.timestamp_opt(t.floor() as i64, 0).earliest() {
        Some(d) => (d.date_naive() - chrono::NaiveDate::default()).num_days(),
        None => (t / 86400.0).floor() as i64,
    }
}

/// The visit a recording belongs to: the one its database names, else the one it falls in.
pub(super) fn visit_of_recording(d: &Data, r: &crate::recordings::Recording) -> Option<i64> {
    d.visits
        .iter()
        .find(|v| v.database == r.database && !r.visit_key.is_empty() && v.visit_key == r.visit_key)
        .or_else(|| {
            let t = r.t_start?;
            d.visits.iter().find(|v| {
                v.database == r.database && v.admit.is_some_and(|a| a <= t) && v.discharge.is_none_or(|b| t <= b)
            })
        })
        .map(|v| v.id)
}

/// The report entry an item belongs to (the patient's labs at one moment are one panel; vital
/// signs one entry a day, the day in Medorama's time zone).
pub(super) fn entry_key(it: &It) -> String {
    let p = it.patient;
    match (it.category.as_str(), &it.live) {
        ("lab", _) if it.time.is_some() => format!("labs:{p}:{}", it.time.unwrap_or(0.0)),
        ("vit", _) if it.time.is_some() => {
            format!("vit:{p}:{}", it.day.unwrap_or_else(|| (it.time.unwrap_or(0.0) / 86400.0).floor() as i64))
        }
        (_, Some(l)) => format!("live:{}:{}", l.connection, l.token),
        _ => format!("item:{}", it.id),
    }
}

/// Whether the patient is there and the user may see them: linked by a database they may view.
pub(super) fn patient_exists(c: &Connection, user: &User, id: i64) -> anyhow::Result<()> {
    let viewable = crate::databases::granted(c, user, "view")?;
    if crate::persons::databases_of(c, id)?.iter().any(|d| viewable.contains(d)) {
        return Ok(());
    }
    anyhow::bail!("no patient {id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_those_of_the_zone_shown() {
        let ny = crate::util::tz("America/New_York");
        // 2024-03-02 23:30 in New York is 04:30 UTC on 3 March: the 2nd there
        let t = crate::util::parse_time("2024-03-02 23:30", ny).unwrap().unwrap();
        let d = day_in(t, ny);
        assert_eq!(d, day_in(crate::util::parse_time("2024-03-02 00:10", ny).unwrap().unwrap(), ny));
        assert_eq!(d + 1, (t / 86400.0).floor() as i64, "the UTC day is the next");
        assert_eq!(day_in(0.0, crate::util::tz("UTC")), 0);
    }
}
