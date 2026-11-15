//! A patient's data from the databases a user sees de-identified outside Privacy Mode
//! (docs/security.md, "Privacy Mode and the privacy gate"): the PHI permission shows the
//! identifiers of the databases granted `phi` only.  A user with it, Privacy Mode off, who
//! holds only `view` on another database linked to the same patient, gets that database's data
//! as Privacy Mode shows them, for that patient: dates moved back by the patient's offset, free
//! text through the gate (or withheld), authors as `[Staff Name]`, files named by tokens and
//! de-identified (server.rs `file_restricted`).  In Privacy Mode every answer passes the gate
//! anyway (server.rs `privacy`): nothing is done here then.

use super::*;

/// The databases linked to a patient that `user` may view, but not see identified, outside
/// Privacy Mode, with the gate for that patient.
pub struct PerDatabase {
    pub dbs: HashSet<i64>,
    pub gate: crate::gate::Restricted,
    names: Vec<String>,
    birth: String,
}

/// The databases `user` may view without `phi` (all of them for a session in Privacy Mode).
pub fn without_phi(c: &Connection, user: &User) -> anyhow::Result<HashSet<i64>> {
    let viewable = crate::databases::granted(c, user, "view")?;
    if user.privacy || !user.phi {
        return Ok(viewable.into_iter().collect());
    }
    let phi = crate::databases::granted(c, user, "phi")?;
    Ok(viewable.into_iter().filter(|d| !phi.contains(d)).collect())
}

impl PerDatabase {
    /// For a session outside Privacy Mode: the patient's databases it sees de-identified, if any.
    pub fn of_patient(c: &Connection, cfg: &Config, user: &User, patient: i64) -> anyhow::Result<Option<PerDatabase>> {
        if user.privacy {
            return Ok(None);
        }
        let without = without_phi(c, user)?;
        let dbs: HashSet<i64> =
            crate::persons::databases_of(c, patient)?.into_iter().filter(|d| without.contains(d)).collect();
        if dbs.is_empty() {
            return Ok(None);
        }
        let gate = crate::search::build_gate(c, cfg)?.for_patient(patient);
        let (names, birth) = crate::gate::Restricted::identity_words(c, cfg, patient);
        Ok(Some(PerDatabase { dbs, gate, names, birth }))
    }

    /// Text of an entry as Privacy Mode writes it (a name, a unit, a service).
    fn words(&self, s: &str) -> String {
        if s.is_empty() {
            return String::new();
        }
        self.gate.note(s, &self.names, &self.birth)
    }

    fn time(&self, t: Option<f64>) -> Option<f64> {
        t.map(|t| self.gate.settings.shift(t))
    }

    fn item(&self, it: &mut It) {
        it.name = self.words(&it.name);
        it.code = self.words(&it.code);
        it.unit = self.words(&it.unit);
        it.service = self.words(&it.service);
        it.flag = self.words(&it.flag);
        // free text withheld when Privacy Mode withholds it; a short value through the gate
        it.text = if is_free_text(it) {
            self.gate.entry_text(&it.text, &self.names, &self.birth)
        } else {
            self.words(&it.text)
        };
        if !it.author.trim().is_empty() {
            it.author = crate::gate::STAFF_NAME.into();
        }
        it.time = self.time(it.time);
        it.end = self.time(it.end);
    }

    /// The patient's data of these databases as Privacy Mode shows them (their recordings keep
    /// their paths, which never leave: `files` and `detail` name them by tokens).
    pub(super) fn apply(&self, d: &mut Data) {
        let mine = |it: &It| it.live.as_ref().is_some_and(|l| self.dbs.contains(&l.connection));
        for it in d.items.iter_mut().chain(d.allergies.iter_mut()).filter(|i| mine(i)) {
            self.item(it);
        }
        for v in d.visits.iter_mut().filter(|v| self.dbs.contains(&v.database)) {
            v.admit = self.time(v.admit);
            v.discharge = self.time(v.discharge);
            v.unit = self.words(&v.unit);
        }
        for r in d.recordings.iter_mut().filter(|r| self.dbs.contains(&r.database)) {
            r.t_start = self.time(r.t_start);
            r.t_end = self.time(r.t_end);
            r.label = self.words(&r.label);
            r.error = self.gate.dict.scrub(&r.error).unwrap_or_else(|| r.error.clone());
        }
        // in time order again, as shown
        d.items.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        d.visits.sort_by(|a, b| b.admit.partial_cmp(&a.admit).unwrap_or(std::cmp::Ordering::Equal));
        d.recordings.sort_by(|a, b| a.t_start.partial_cmp(&b.t_start).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// A recording's monitoring overview (read from its index with its real times) as Privacy
    /// Mode shows it: every time moved back, the recording named by its token.
    pub(super) fn monitoring(&self, m: &mut Monitoring) {
        let shift = |t: &mut f64| *t = self.gate.settings.shift(*t);
        for s in m.trends.iter_mut() {
            s.t.iter_mut().for_each(shift);
        }
        for g in m.gaps.iter_mut() {
            shift(&mut g.0);
            shift(&mut g.1);
        }
        let r = &mut m.recording;
        r.t_start = self.time(r.t_start);
        r.t_end = self.time(r.t_end);
        r.added_at = self.gate.settings.shift(r.added_at);
        r.path = crate::gate::recording_token(r.id);
        r.name = crate::gate::RECORDING.into();
        r.label = self.words(&r.label);
        r.error = self.gate.dict.scrub(&r.error).unwrap_or_else(|| r.error.clone());
    }
}

/// A patient's data, from every database linked to them that `user` may view (items.rs
/// `load`), those of the databases they see de-identified as Privacy Mode shows them.  Every
/// page of a patient reads them through here.
pub(super) fn load(cfg: &Config, c: &Connection, user: &User, patient: i64) -> anyhow::Result<Data> {
    let mut d = super::items::load(cfg, c, user, patient)?;
    if let Some(p) = PerDatabase::of_patient(c, cfg, user, patient)? {
        p.apply(&mut d);
    }
    Ok(d)
}
