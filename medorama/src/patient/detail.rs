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
    /// Notes of the same encounter around this moment (or during the stay).
    pub notes: Vec<NoteBrief>,
}

pub(super) fn notes_near(
    c: &Connection,
    patient: i64,
    encounter: Option<i64>,
    t0: f64,
    t1: f64,
    read_notes: bool,
    except: i64,
) -> anyhow::Result<Vec<NoteBrief>> {
    let mut st = c.prepare(&format!(
        "SELECT id, time, name, author, service, value_text FROM ehr_items
         WHERE patient = ?1 AND category = 'note' AND id != ?2 AND (CAST(?3 AS INTEGER) IS NULL OR encounter = ?3) AND time BETWEEN ?4 AND ?5
           AND {}
         ORDER BY time DESC, id DESC LIMIT 12",
        item_shown("")
    ))?;
    let v = st
        .query_map(params![patient, except, encounter, t0, t1], |r| {
            let text: String = r.get(5)?;
            Ok(NoteBrief {
                key: format!("item:{}", r.get::<_, i64>(0)?),
                time: r.get(1)?,
                title: r.get(2)?,
                author: r.get(3)?,
                service: r.get(4)?,
                text: read_notes.then_some(text),
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

pub(super) fn sources_of_items(c: &Connection, ids: &[i64]) -> anyhow::Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for id in ids {
        let mut st = c.prepare("SELECT DISTINCT source FROM ehr_provenance WHERE item = ?1")?;
        for s in st.query_map([id], |r| r.get::<_, String>(0))? {
            let s = s?;
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    Ok(out)
}

pub fn detail(c: &Connection, cfg: &Config, user: &User, id: i64, key: &str) -> anyhow::Result<Detail> {
    patient_exists(c, id)?;
    // notes' text: with the PHI permission, or in a de-identified session that lets notes through
    let read_notes = user.reads_notes();
    let (kind, rest) = key.split_once(':').ok_or_else(|| anyhow::anyhow!("no entry {key}"))?;
    if kind == "rec" {
        let rid: i64 = rest.parse()?;
        let Some(cat) = recordings_of(c, id)?.into_iter().find(|r| r.0 == rid).map(|r| r.4) else {
            anyhow::bail!("no entry {key}");
        };
        let r = crate::recordings::get(c, rid)?.ok_or_else(|| anyhow::anyhow!("no entry {key}"))?;
        let enc: Option<i64> = c
            .query_row(
                &format!(
                    "SELECT l.encounter FROM recording_links l JOIN encounters e ON e.id = l.encounter
                     WHERE l.recording = ?1 AND e.patient = ?2 AND {} LIMIT 1",
                    encounter_shown("e.id")
                ),
                [rid, id],
                |x| x.get(0),
            )
            .optional()?;
        let (t0, t1) = (r.t_start.or(r.name_time), r.t_end);
        let notes = match t0 {
            Some(a) => notes_near(c, id, enc, a - 12.0 * 3600.0, t1.unwrap_or(a) + 12.0 * 3600.0, read_notes, -1)?,
            None => vec![],
        };
        let monitoring = index_overview(c, cfg, &r);
        let sources = crate::imports::sources_of(c, "recording", rid)?;
        return Ok(Detail {
            key: key.into(),
            title: recording_title(&cat).into(),
            category: cat,
            time: t0,
            end: t1,
            encounter: enc,
            sources,
            rows: vec![],
            fields: vec![],
            text: None,
            attachment: None,
            monitoring: Some(monitoring.unwrap_or(Monitoring {
                recording: r,
                waveforms: vec![],
                vitals: vec![],
                derived: vec![],
                trends: vec![],
                gaps: vec![],
                recorded: None,
            })),
            notes,
        });
    }
    let items = items_of(c, id)?;
    let group: Vec<&It> = items.iter().filter(|i| entry_key(i) == key).collect();
    if group.is_empty() {
        anyhow::bail!("no entry {key}");
    }
    let first = group[0];
    let t = first.time;
    let ids: Vec<i64> = group.iter().map(|i| i.id).collect();
    let mut d = Detail {
        key: key.into(),
        category: first.category.clone(),
        title: first.name.clone(),
        time: t,
        end: group.iter().filter_map(|i| i.end).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.max(x)))),
        encounter: first.encounter,
        sources: {
            let mut v = sources_of_items(c, &ids.iter().copied().filter(|i| *i > 0).collect::<Vec<_>>())?;
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
        notes: vec![],
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
                .map(value_text);
            let (value, num) = (it.num.map(fmt_num).unwrap_or_else(|| it.text.clone()), it.num);
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
    } else {
        let (vocab, flag, lo, hi, file): (String, String, Option<f64>, Option<f64>, String) = match &first.live {
            Some(_) => (first.vocabulary.clone(), first.flag.clone(), first.ref_low, first.ref_high, String::new()),
            None => c.query_row(
                "SELECT vocabulary, flag, ref_low, ref_high, file_path FROM ehr_items WHERE id = ?1",
                [first.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?,
        };
        let mut f = vec![];
        if first.num.is_some() {
            f.push(("Value".to_string(), value_text(first)));
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
        if !first.text.is_empty() && (read_notes || first.category != "note") {
            d.text = Some(first.text.clone());
        }
        if !file.is_empty() {
            let real = Path::new(&file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // in Privacy Mode a file is named by its token: its name may identify (gate.rs)
            let name = if user.privacy { crate::gate::file_token(0, &real) } else { real };
            let kind = file_kind(&file);
            let url = format!("patients/{id}/file/{}", first.id);
            let src = viewer_src(&url, kind, &name);
            d.attachment = Some(Attachment {
                name,
                kind: kind.into(),
                viewer: viewer_for(&first.category, kind).map(String::from),
                url,
                src,
                hints: viewer_hints(c, first.id)?,
            });
        }
    }
    if let Some(t) = t {
        d.notes = notes_near(
            c,
            id,
            first.encounter,
            t - 24.0 * 3600.0,
            t + 24.0 * 3600.0,
            read_notes,
            if first.category == "note" { first.id } else { -1 },
        )?;
    }
    Ok(d)
}
