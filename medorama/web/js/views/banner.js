// Whose file a viewer shows (ECG, PACS, VitalStream).  A viewer opened from a patient's page or
// a database's row carries `patient=<id>&entry=<key>` in its address.  From them, this module
// makes the viewer's context (its title in place of the file's name: viewers' host.js): who
// the patient is (as the server sends them: identifiers only with the PHI permission, none in
// Privacy Mode), what the file is, its place among the patient's files of that kind;
// and it acts on the viewer's buttons (back to the patient, the previous or next file), which
// the viewer posts to this page.

import { api, ages, fail, fmtDate, fmtHM } from '../core.js';
import { go, href } from '../router.js';

const cache = { summaries: new Map(), details: new Map(), reports: new Map() };
const get = (map, key, path) => {
  if (!map.has(key)) map.set(key, api(path).catch((e) => { map.delete(key); throw e; }));
  return map.get(key);
};
const detail = (patient, key) => get(cache.details, `${patient}|${key}`, `patients/${patient}/detail?key=${encodeURIComponent(key)}`);

/** Forget what was read (the data changed: an import, a removal). */
export function invalidateBanner() {
  cache.summaries.clear();
  cache.details.clear();
  cache.reports.clear();
}

// the context each kind of viewer shows now: { patient, entry, prev, next }
const shown = { app: null, rec: null };
// how the tabs build their addresses (set by apps.js, to avoid a circular import)
let opener = null;
export function setOpener(f) { opener = f; }

/** The address of a patient's entry in its viewer, with the patient's context; null if none. */
export async function entryHref(patient, key) {
  const d = await detail(patient, key);
  const ctx = { patient, entry: key };
  if (d.monitoring?.recording) {
    const r = d.monitoring.recording;
    return href('viewer', [], { path: r.path, t0: r.t_start ?? undefined, t1: r.t_start != null ? r.t_start + 60 : undefined, ...ctx });
  }
  if (d.attachment?.viewer && opener) return opener(d.attachment, ctx);
  return null;
}

/**
 * The viewer's context for a route (`kind`: 'app' for the ECG and PACS tabs, 'rec' for
 * VitalStream): { who, what, pos, nav } for its address, or null when it has no patient.
 */
export async function hostContext(params, kind) {
  const patient = +params.get('patient');
  const entry = params.get('entry');
  shown[kind] = null;
  if (!patient || !entry) return null;
  let s;
  let d;
  let rep;
  try {
    [s, d, rep] = await Promise.all([
      get(cache.summaries, patient, `patients/${patient}`),
      detail(patient, entry),
      get(cache.reports, patient, `patients/${patient}/report`),
    ]);
  } catch (e) {
    fail(new Error(`The patient of this file cannot be shown: ${e.message}`));
    return null;
  }
  // as the server sent them: the identity, or in Privacy Mode its placeholders
  const name = [s.phi?.last_name, s.phi?.first_name].filter(Boolean).join(', ');
  const mrn = (s.phi?.mrns || []).map((m) => m[1]).join(', ');
  const age = ages() && s.age !== undefined && s.age !== null ? `${s.age} y` : '';
  const who = [name, s.patient_id, mrn && `MRN ${mrn}`, s.sex, age].filter(Boolean).join(' · ');
  const enc = s.encounters?.find((x) => x.id === d.encounter);
  const what = [d.title, d.time ? `${fmtDate(d.time)} ${fmtHM(d.time)}` : '', enc?.encounter_id].filter(Boolean).join(' · ');
  // the patient's other files of this kind, in time order
  const sameKind = (e) => e.category === d.category && (kind === 'rec' ? e.key.startsWith('rec:') : e.attachment);
  const sibs = (rep.entries || []).filter(sameKind).sort((a, b) => (a.time ?? 0) - (b.time ?? 0));
  const i = sibs.findIndex((e) => e.key === entry);
  const prev = i > 0 ? sibs[i - 1].key : null;
  const next = i >= 0 && i < sibs.length - 1 ? sibs[i + 1].key : null;
  shown[kind] = { patient, entry, prev, next };
  return {
    who,
    what,
    pos: sibs.length > 1 && i >= 0 ? `${i + 1} of ${sibs.length}` : '',
    nav: ['back', prev && 'prev', next && 'next'].filter(Boolean).join(','),
  };
}

/** The context's keys added to a viewer's address parameters. */
export function withContext(p, c) {
  if (c) for (const k of ['who', 'what', 'pos', 'nav']) if (c[k]) p.set(k, c[k]);
  return p;
}

/**
 * The viewers' buttons: a frame of this page (same origin) posts `{ viewerHost: action }`.
 * `frames()`: the frames, as [element, kind] pairs.
 */
export function listen(frames) {
  window.addEventListener('message', async (e) => {
    if (e.origin !== location.origin || !e.data || typeof e.data.viewerHost !== 'string') return;
    const kind = frames().find(([f]) => f && f.contentWindow === e.source)?.[1];
    const c = kind && shown[kind];
    if (!c) return;
    const action = e.data.viewerHost;
    try {
      if (action === 'back') go(href('patient', [String(c.patient)], { item: c.entry }));
      else if ((action === 'prev' || action === 'next') && c[action]) {
        const to = await entryHref(c.patient, c[action]);
        if (to) go(to);
      }
    } catch (err) { fail(err); }
  });
}
