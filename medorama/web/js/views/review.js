// Review (admins): held-back rows, possible duplicate patients, recordings whose files disagree,
// items waiting for a patient.

import { $, api, esc, fail, phi, toast } from '../core.js';
import { decideLink, inFile, openEncounter } from './encounters.js';

const PROBLEMS = {
  two_patients: 'Patient ID and MRN name two patients',
  mrn_of_other_patient: 'The MRN belongs to another patient ID',
  other_mrn: 'The patient has another MRN',
  encounter_of_other_patient: 'The encounter belongs to another patient',
};
let last = null;

function person(p) {
  const name = [p.last_name, p.first_name].filter(Boolean).join(', ');
  return `<b>${esc(p.patient_id)}</b> · ${esc(name)} · ${esc(p.birth_date)}${p.sex ? ` · ${esc(p.sex)}` : ''}
    <div class="muted">${p.mrns.length ? `MRN ${esc(p.mrns.join(', '))} · ` : ''}${p.encounters} encounter(s)</div>`;
}

export async function reviewBadge() {
  const r = await api('review');
  $('#review-badge').textContent = r.open;
  $('#review-badge').classList.toggle('hidden', !r.open);
  return r;
}

async function load() {
  const r = await reviewBadge();
  last = r;
  let h = '<tr><th>Import</th><th class="num">Line</th><th>Problem</th><th>Row</th><th></th></tr>';
  if (!r.held.length) h += '<tr><td colspan="5" class="empty">Nothing held back.</td></tr>';
  for (const x of r.held) {
    const row = Object.entries(x.row).map(([k, v]) => `${esc(k)}: ${esc(v)}`).join(' · ');
    h += `<tr><td>${esc(x.batch)}<div class="muted">${esc(x.source)}</div></td><td class="num">${x.line}</td>
      <td>${esc(PROBLEMS[x.problem] || x.problem)}<div class="muted">${esc(x.detail)}</div></td><td class="muted">${row}</td>
      <td class="nowrap"><button class="link" data-held="${x.id}" data-act="apply" title="${esc(x.apply)}">Apply…</button>
        <button class="link danger" data-held="${x.id}" data-act="dismiss">Dismiss</button></td></tr>`;
  }
  $('#rv-held').innerHTML = h;
  if (r.duplicates) {
    h = '<tr><th>Patient</th><th>Patient</th><th>Why</th><th></th></tr>';
    if (!r.duplicates.length) h += '<tr><td colspan="4" class="empty">No possible duplicates.</td></tr>';
    r.duplicates.forEach((x, i) => {
      h += `<tr><td>${person(x.a)}</td><td>${person(x.b)}</td><td>${esc(x.reason)}</td>
        <td class="nowrap"><button class="link" data-merge="${i}" data-keep="a">Merge into ${esc(x.a.patient_id)}</button>
          <button class="link" data-merge="${i}" data-keep="b">Merge into ${esc(x.b.patient_id)}</button>
          <button class="link" data-distinct="${i}">Different people</button></td></tr>`;
    });
    $('#rv-dups').innerHTML = h;
  }
  if (r.mismatches) {
    h = '<tr><th>Recording</th><th>Linked to</th><th>The databases say</th><th>The file says</th><th></th></tr>';
    if (!r.mismatches.length) h += '<tr><td colspan="5" class="empty">No disagreement.</td></tr>';
    for (const x of r.mismatches) {
      const ours = [[x.last_name, x.first_name].filter(Boolean).join(', '), x.mrns.length ? `MRN ${x.mrns.join(', ')}` : '', x.birth_date].filter(Boolean).join(' · ');
      h += `<tr><td>${esc(x.name)}</td><td><button class="link" data-open-enc="${x.encounter}">${esc(x.encounter_id)}</button>
        <div class="muted">${esc(x.patient_id)} · ${esc(x.link)}</div></td><td>${esc(ours)}</td><td>${inFile(x.in_file).replace(/^<div class="results">In the file: /, '<div>')}</td>
        <td class="nowrap"><button class="link" data-decide="link" data-rec="${x.recording}" data-eid="${esc(x.encounter_id)}">Keep the link</button>
          <button class="link danger" data-decide="never" data-rec="${x.recording}" data-eid="${esc(x.encounter_id)}">Unlink</button></td></tr>`;
    }
    $('#rv-files').innerHTML = h;
  }
  h = `<tr><th>Encounter ID</th><th>Patient ID</th>${phi() ? '<th>MRN</th><th>CSN</th>' : ''}<th class="num">Items</th><th>Sources</th></tr>`;
  if (!r.waiting.length) h += `<tr><td colspan="${phi() ? 6 : 4}" class="empty">No item waiting.</td></tr>`;
  for (const x of r.waiting) {
    h += `<tr><td>${esc(x.encounter_id)}</td><td>${esc(x.patient_id)}</td>${phi() ? `<td>${esc(x.mrn)}</td><td>${esc(x.csn)}</td>` : ''}
      <td class="num">${x.items}</td><td class="muted">${esc(x.sources)}</td></tr>`;
  }
  $('#rv-waiting').innerHTML = h;
}

async function decideHeld(id, act) {
  const x = last.held.find((y) => y.id === id);
  if (act === 'apply' && !confirm(`${x.apply}?\n\n${x.detail}`)) return;
  if (act === 'dismiss' && !confirm('Dismiss this row? It stays out of the catalog.')) return;
  await api(`review/held/${id}`, { method: 'POST', json: { action: act } });
  toast(act === 'apply' ? 'Row applied' : 'Row dismissed');
  load();
}

async function mergePair(i, keep) {
  const x = last.duplicates[i];
  const [into, from] = keep === 'a' ? [x.a, x.b] : [x.b, x.a];
  if (!confirm(`Merge ${from.patient_id} into ${into.patient_id}?\n\nIts encounters, MRNs, items and sources go to ${into.patient_id}; later rows naming ${from.patient_id} go there too. This cannot be undone.`)) return;
  await api('review/merge', { method: 'POST', json: { a: from.patient_id, b: into.patient_id } });
  toast(`${from.patient_id} merged into ${into.patient_id}`);
  load();
}

export default {
  show: () => load(),
  wire() {
    $('#view-review').addEventListener('click', (e) => {
      const b = e.target.closest('[data-held],[data-merge],[data-distinct],[data-decide],[data-open-enc]');
      if (!b) return;
      const d = b.dataset;
      if (d.held) decideHeld(+d.held, d.act).catch(fail);
      else if (d.merge !== undefined) mergePair(+d.merge, d.keep).catch(fail);
      else if (d.distinct !== undefined) {
        const x = last.duplicates[+d.distinct];
        api('review/distinct', { method: 'POST', json: { a: x.a.patient_id, b: x.b.patient_id } }).then(() => { toast('Marked as different people'); load(); }).catch(fail);
      } else if (d.decide !== undefined) {
        if (d.decide !== 'never' || confirm('Unlink this recording from the encounter?')) decideLink(+d.rec, d.eid, d.decide).then(load).catch(fail);
      } else if (d.openEnc) openEncounter(+d.openEnc).catch(fail);
    });
  },
};
