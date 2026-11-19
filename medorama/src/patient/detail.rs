//! An entry's detail: panel rows, fields, text, attachment, notes around it.

use super::*;

// ------------------------------------------------------------------ an entry's detail

#[derive(Debug, Serialize)]
pub struct Row {
    pub item: i64,
    pub name: String,
    pub value: String,
    pub num: Option<f64>,
    pub unit: String,
    pub ref_low: Option<f64>,
    pub ref_high: Option<f64>,
    pub flag: String,
    pub level: u8,
    pub previous: Option<String>,
    /// The test over its encounter: (time, value).
    pub series: Vec<(f64, f64)>,
    pub time: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct NoteBrief {
    pub key: String,
    pub time: Option<f64>,
    pub title: String,
    pub author: String,
    pub service: String,
    /// Only with the PHI permission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Attachment {
    pub name: String,
    /// image, pdf, text, video, wfdb, json, dicom, folder, other
    pub kind: String,
    pub url: String,
    /// The viewer that shows it (`ecg`, `echo`, `pacs`: `viewers/<name>/`), with the address it
    /// reads it from: the file under its own name, so that the files next to it (a WFDB
    /// record's signal files) resolve beside it; a folder's address ends with `/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<String>,
    pub src: String,
    /// What the viewer needs to know that the file does not say, from the item's row: `mm`
    /// (millimetres per pixel of a picture or video), `fps` (frames per second of pictures).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub hints: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct Series {
    pub name: String,
    pub unit: String,
    pub t: Vec<f64>,
    pub v: Vec<f64>,
    /// A range around each value (derived series: the beat-to-beat spread of a minute).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lo: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hi: Vec<f64>,
}

#[derive(Debug, Serialize)]
pub struct Monitoring {
    pub recording: crate::recordings::Recording,
    pub waveforms: Vec<String>,
    pub vitals: Vec<String>,
    /// Series found from the waveforms (a Holter's heart rate), shown first in `trends`.
    pub derived: Vec<String>,
    pub trends: Vec<Series>,
    pub gaps: Vec<(f64, f64)>,
    /// Share of the recording's span with data (gaps excluded).
    pub recorded: Option<f64>,
}

/// An ECG's measurement: its value, the change since the patient's previous ECG, and its
/// values over every ECG of the patient.
#[derive(Debug, Serialize)]
pub struct Measure {
    pub name: String,
    pub value: f64,
    pub unit: String,
    /// This value less the previous ECG's (none: the previous ECG has none).
    pub change: Option<f64>,
    /// (time, value) over the patient's ECGs, oldest first.
    pub series: Vec<(f64, f64)>,
}

/// An ECG: its measurements, its statements (those new since the previous ECG marked), and
/// the previous ECG's statements it no longer has.
#[derive(Debug, Serialize)]
pub struct Ecg {
    pub measures: Vec<Measure>,
    /// (statement, new since the previous ECG); none when its text is withheld.
    pub statements: Vec<(String, bool)>,
    pub gone: Vec<String>,
    /// The previous ECG's time: what each change is since.
    pub previous: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Detail {
    pub key: String,
    pub category: String,
    pub title: String,
    pub time: Option<f64>,
    pub end: Option<f64>,
    pub encounter: Option<i64>,
    pub sources: Vec<String>,
    pub rows: Vec<Row>,
    /// One item's own fields (code, value, text, author, …).
    pub fields: Vec<(String, String)>,
    /// A note's or report's text (notes: PHI only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitoring: Option<Monitoring>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecg: Option<Ecg>,
    /// Notes of the same encounter around this moment (or during the stay).
    pub notes: Vec<NoteBrief>,
    /// A recording's signals, as its database lists them (`recording_signals`): by group.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<crate::sources::Signal>,
}

/// Whether an entry's text is free text that "the text of notes and reports" covers (Admin ›
/// Privacy Mode): a note's or a report's, and any long text (a result's comment, a value
/// written out), whatever the entry's category.
pub(super) fn is_free_text(it: &It) -> bool {
    it.category == "note"
        || it.category == "report"
        || it.text.chars().count() > FREE_TEXT_CHARS
        || it.text.contains('\n')
}

/// The length from which an entry's text counts as free text, whatever its category.
pub(super) const FREE_TEXT_CHARS: usize = 40;

/// The notes of the same visit around a moment (or during the stay), latest first.
pub(super) fn notes_near(
    items: &[It],
    encounter: Option<i64>,
    t0: f64,
    t1: f64,
    read_notes: bool,
    except: i64,
) -> Vec<NoteBrief> {
    let mut v: Vec<NoteBrief> = items
        .iter()
        .filter(|i| i.category == "note" && i.id != except && (encounter.is_none() || i.encounter == encounter))
        .filter(|i| i.time.is_some_and(|t| t >= t0 && t <= t1))
        .map(|i| NoteBrief {
            key: entry_key(i),
            time: i.time,
            title: i.name.clone(),
            author: i.author.clone(),
            service: i.service.clone(),
            text: read_notes.then(|| i.text.clone()),
        })
        .collect();
    v.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(12);
    v
}

/// A recording shown as a file (an ECG, images, a letter), by a viewer that shows it;
/// `restricted`: its database is shown de-identified (Privacy Mode, or granted without `phi`).
fn attachment_of(restricted: bool, patient: i64, r: &crate::recordings::Recording) -> Attachment {
    let fkind = file_kind(&r.path);
    let real = r.name.clone();
    // de-identified, a file is named by its token: its name may identify (gate.rs)
    let name = if restricted { crate::gate::file_token(0, &real) } else { real };
    let url = format!("patients/{patient}/file/{}", r.id);
    Attachment {
        src: viewer_src(&url, fkind, &name),
        name,
        kind: fkind.into(),
        viewer: viewer_for(&r.category, fkind).map(String::from),
        url,
        hints: Default::default(),
    }
}

pub fn detail(c: &Connection, cfg: &Config, user: &User, id: i64, key: &str) -> anyhow::Result<Detail> {
    patient_exists(c, user, id)?;
    // notes' text: with the PHI permission, or in a de-identified session that lets notes through
    let read_notes = user.reads_notes();
    let (kind, rest) = key.split_once(':').ok_or_else(|| anyhow::anyhow!("no entry {key}"))?;
    let data = load(cfg, c, user, id)?;
    // the databases shown de-identified outside Privacy Mode (granted without `phi`)
    let per_db = PerDatabase::of_patient(c, cfg, user, id)?;
    let restricted = |db: i64| user.privacy || per_db.as_ref().is_some_and(|p| p.dbs.contains(&db));
    if kind == "rec" {
        let rid: i64 = rest.parse()?;
        let r =
            data.recordings.iter().find(|r| r.id == rid).cloned().ok_or_else(|| anyhow::anyhow!("no entry {key}"))?;
        let enc = visit_of_recording(&data, &r);
        let (t0, t1) = (r.t_start, r.t_end);
        let notes = match t0 {
            Some(a) => notes_near(&data.items, enc, a - 12.0 * 3600.0, t1.unwrap_or(a) + 12.0 * 3600.0, read_notes, 0),
            None => vec![],
        };
        // read with the recording's real times (its index's are), then as shown
        let monitoring = match &per_db {
            Some(p) if p.dbs.contains(&r.database) => {
                let real = crate::recordings::get(cfg, c, r.id)?.ok_or_else(|| anyhow::anyhow!("no entry {key}"))?;
                index_overview(c, cfg, &real).map(|mut m| {
                    p.monitoring(&mut m);
                    m
                })
            }
            _ => index_overview(c, cfg, &r),
        };
        // a file (an ECG, images, a letter): opened from the recording, by a viewer that shows
        // it; a waveform recording is opened in VitalStream instead
        let waveform = matches!(r.category.as_str(), "icu" | "holter" | "eeg" | "sleep");
        let attachment = (!waveform).then(|| attachment_of(restricted(r.database), id, &r));
        let monitoring = if attachment.is_some() {
            monitoring
        } else {
            let mut shown = r.clone();
            if !user.privacy && restricted(r.database) {
                // its path and name never leave (the answer is not gated again outside Privacy Mode)
                shown.path = crate::gate::recording_token(r.id);
                shown.name = crate::gate::RECORDING.into();
            }
            Some(monitoring.unwrap_or(Monitoring {
                recording: shown,
                waveforms: vec![],
                vitals: vec![],
                derived: vec![],
                trends: vec![],
                gaps: vec![],
                recorded: None,
            }))
        };
        // the signals its database lists (names, not values); none when it cannot be asked now
        let signals = crate::sources::get(c, r.database)
            .ok()
            .flatten()
            .and_then(|s| s.signals(std::slice::from_ref(&r.key)).ok())
            .unwrap_or_default();
        return Ok(Detail {
            key: key.into(),
            title: recording_title(&r.category).into(),
            category: r.category.clone(),
            time: t0,
            end: t1,
            encounter: enc,
            sources: vec![r.source.clone()],
            rows: vec![],
            fields: vec![],
            text: None,
            attachment,
            monitoring,
            ecg: None,
            notes,
            signals,
        });
    }
    if kind == "enc" {
        let v = data.visits.iter().find(|v| v.key == key).ok_or_else(|| anyhow::anyhow!("no entry {key}"))?;
        let mut fields = vec![("Database".to_string(), v.source.clone())];
        if !v.unit.is_empty() {
            fields.push(("Unit".into(), v.unit.clone()));
        }
        let notes = match v.admit {
            Some(a) => notes_near(&data.items, Some(v.id), a, v.discharge.unwrap_or(a) + 86400.0, read_notes, 0),
            None => vec![],
        };
        return Ok(Detail {
            key: key.into(),
            category: if v.icu { "icu".into() } else { "enc".into() },
            title: if v.icu { "Intensive-care stay".into() } else { "Visit".into() },
            time: v.admit,
            end: v.discharge,
            encounter: Some(v.id),
            sources: vec![v.source.clone()],
            rows: vec![],
            fields,
            text: None,
            attachment: None,
            monitoring: None,
            ecg: None,
            notes,
            signals: vec![],
        });
    }
    let items = &data.items;
    let group: Vec<&It> = items.iter().filter(|i| entry_key(i) == key).collect();
    if group.is_empty() {
        anyhow::bail!("no entry {key}");
    }
    let first = group[0];
    let t = first.time;
    let mut d = Detail {
        key: key.into(),
        category: first.category.clone(),
        title: first.name.clone(),
        time: t,
        end: group.iter().filter_map(|i| i.end).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.max(x)))),
        encounter: first.encounter,
        sources: {
            let mut v: Vec<String> = vec![];
            for l in group.iter().filter_map(|i| i.live.as_ref()) {
                if !v.contains(&l.source) {
                    v.push(l.source.clone());
                }
            }
            v
        },
        rows: vec![],
        fields: vec![],
        text: None,
        attachment: None,
        monitoring: None,
        ecg: None,
        notes: vec![],
        signals: vec![],
    };
    if kind == "labs" || kind == "vit" {
        d.title = if kind == "vit" {
            "Vital signs".into()
        } else if group.len() == 1 {
            first.name.clone()
        } else {
            "Laboratory".into()
        };
        // vital signs: each sign once (in the entry's order), its last reading of the entry:
        // the one shown and marked on its curve
        let mut last: HashMap<String, i64> = HashMap::new();
        for it in &group {
            let k = it.name.to_lowercase();
            let later =
                last.get(&k).and_then(|id| group.iter().find(|o| o.id == *id)).is_none_or(|o| it.time >= o.time);
            if later {
                last.insert(k, it.id);
            }
        }
        let mut seen: HashSet<String> = HashSet::new();
        for it in &group {
            if kind == "vit"
                && (last.get(&it.name.to_lowercase()) != Some(&it.id) || !seen.insert(it.name.to_lowercase()))
            {
                continue;
            }
            let (flag, lo, hi) = (it.flag.clone(), it.ref_low, it.ref_high);
            // the same test over the encounter (else over the patient's data)
            let same: Vec<&It> = items
                .iter()
                .filter(|o| {
                    o.name.eq_ignore_ascii_case(&it.name)
                        && o.num.is_some()
                        && o.time.is_some()
                        && (it.encounter.is_none() || o.encounter == it.encounter)
                })
                .collect();
            let mut series: Vec<(f64, f64)> =
                same.iter().map(|o| (o.time.unwrap_or(0.0), o.num.unwrap_or(0.0))).collect();
            series.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            // vital signs: the 12 hours on each side of this reading, so that stepping from one
            // entry to the next moves the window with it (results: the whole stay)
            if kind == "vit" {
                let t = it.time.unwrap_or(0.0);
                series.retain(|p| (p.0 - t).abs() <= 12.0 * 3600.0);
            }
            let previous = items
                .iter()
                .filter(|o| o.name.eq_ignore_ascii_case(&it.name) && o.time.is_some() && o.time < it.time)
                .max_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal))
                .map(|o| shown_value(o, read_notes));
            let text = if read_notes || !is_free_text(it) { it.text.clone() } else { crate::gate::WITHHELD.into() };
            let (value, num) = (it.num.map(fmt_num).unwrap_or(text), it.num);
            d.rows.push(Row {
                item: it.id,
                name: it.name.clone(),
                value,
                num,
                unit: it.unit.clone(),
                ref_low: lo,
                ref_high: hi,
                level: if kind == "vit" { 0 } else { level(&flag, it.num, lo, hi) },
                flag,
                previous: if kind == "vit" { None } else { previous },
                series: thin(series, it.time, 80),
                time: it.time,
            });
        }
    } else if kind == "ecg" {
        let head = *group.iter().find(|i| is_ecg_head(i, &group)).unwrap_or(&first);
        d.title = head.name.clone();
        d.ecg = Some(ecg_of(items, &group, head, read_notes));
        let file = group.iter().find_map(|i| i.file).and_then(|rid| data.recordings.iter().find(|r| r.id == rid));
        d.attachment = file.map(|r| attachment_of(restricted(r.database), id, r));
    } else {
        let (vocab, flag, lo, hi) = (first.vocabulary.clone(), first.flag.clone(), first.ref_low, first.ref_high);
        let mut f = vec![];
        if first.num.is_some() {
            f.push(("Value".to_string(), shown_value(first, read_notes)));
        }
        if !first.code.is_empty() {
            f.push((
                "Code".into(),
                [vocab.as_str(), first.code.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
        if lo.is_some() || hi.is_some() {
            f.push((
                "Reference".into(),
                format!("{} – {}", lo.map(fmt_num).unwrap_or_default(), hi.map(fmt_num).unwrap_or_default()),
            ));
        }
        if !flag.is_empty() {
            f.push(("Flag".into(), flag));
        }
        if !first.author.is_empty() {
            // staff names are withheld in Privacy Mode
            f.push(("Author".into(), if user.privacy { crate::gate::STAFF_NAME.into() } else { first.author.clone() }));
        }
        if !first.service.is_empty() {
            f.push(("Service".into(), first.service.clone()));
        }
        d.fields = f;
        // free text (a note's, a report's, any entry's): as the session may read it
        if !first.text.is_empty() && (read_notes || !is_free_text(first)) {
            d.text = Some(first.text.clone());
        }
        // the entry's file (items.rs)
        let file = first.file.and_then(|rid| data.recordings.iter().find(|r| r.id == rid));
        d.attachment = file.map(|r| attachment_of(restricted(r.database), id, r));
    }
    if let Some(t) = t {
        d.notes = notes_near(
            items,
            first.encounter,
            t - 24.0 * 3600.0,
            t + 24.0 * 3600.0,
            read_notes,
            if first.category == "note" { first.id } else { 0 },
        );
    }
    Ok(d)
}

/// An ECG's statements, one a line, as the session may read them (none: withheld).
fn statements_of(it: &It, read_notes: bool) -> Option<Vec<String>> {
    if !read_notes && is_free_text(it) {
        return None;
    }
    Some(it.text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// An ECG's detail: its measurements against the patient's previous ECG and over all of them,
/// its statements against the previous ECG's (compared as written).
fn ecg_of(items: &[It], group: &[&It], head: &It, read_notes: bool) -> Ecg {
    let ecg_items = || items.iter().filter(|i| i.ecg.is_some());
    // the previous ECG: the latest ECG entry before this one
    let previous_key = ecg_items()
        .filter(|i| i.time.is_some() && i.time < head.time)
        .max_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal))
        .and_then(|i| i.ecg.clone());
    let previous: Vec<&It> = ecg_items().filter(|i| previous_key.is_some() && i.ecg == previous_key).collect();
    let mut measures = vec![];
    for it in group.iter().filter(|i| i.num.is_some()) {
        let same = |o: &&It| o.num.is_some() && o.name.eq_ignore_ascii_case(&it.name) && o.unit == it.unit;
        let mut series: Vec<(f64, f64)> = ecg_items().filter(same).filter_map(|o| Some((o.time?, o.num?))).collect();
        series.sort_by(|a, b| a.0.total_cmp(&b.0));
        let before = previous.iter().copied().find(same).and_then(|o| o.num);
        let value = it.num.unwrap_or_default();
        measures.push(Measure {
            name: it.name.clone(),
            value,
            unit: it.unit.clone(),
            change: before.map(|b| value - b),
            series: thin(series, it.time, 80),
        });
    }
    let mine = statements_of(head, read_notes);
    let theirs = previous.iter().find(|i| is_ecg_head(i, &previous)).and_then(|p| statements_of(p, read_notes));
    let (statements, gone) = match (mine, theirs) {
        (Some(m), Some(t)) => (
            m.iter().map(|x| (x.clone(), !t.contains(x))).collect(),
            t.into_iter().filter(|x| !m.contains(x)).collect(),
        ),
        (Some(m), None) => (m.into_iter().map(|x| (x, false)).collect(), vec![]),
        _ => (vec![], vec![]),
    };
    Ecg { measures, statements, gone, previous: previous.first().and_then(|p| p.time) }
}
