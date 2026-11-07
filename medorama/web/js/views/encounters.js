// Encounters: the table of encounters, for selecting them (actions) and removing them; the
// encounter drawer (bed stays, recordings and their links, EHR items).

import { $, $$, PAGE, api, dot, esc, fail, fmtDuration, fmtTime, isAdmin, phi, qs, removalText, statusCounts, toast, zonedDayEnd, zonedMidnight } from '../core.js';
import { recordingButtons, wireRecordingButtons } from './recordings.js';
import { go } from '../router.js';

/** Shared with the Actions screen: the current search and the selected encounters. */
export const encState = { offset: 0, total: 0, items: [], selected: new Set(), search: {}, timer: null };
const drawer = { id: null, domain: '' };

/** What a recording's own file says about the patient (PHI users), ✓ / ⚠ against the databases. */
export function inFile(f) {
  if (!f) return '';
  if (f.state !== 'found') return `<div class="results">In the file: ${f.state === 'none' ? 'no identifiers' : esc(f.state)}</div>`;
  const m = (x) => (x === true ? ' <span class="ok" title="Matches the databases">✓</span>' : x === false ? ' <span class="err" title="Differs from the databases">⚠ differs</span>' : '');
  const name = [f.full_name, [f.last_name, f.first_name].filter(Boolean).join(', ')].filter(Boolean).join(' / ');
  const parts = [];
  if (name) parts.push(`${esc(name)}${m(f.name_matches)}`);
  if (f.patient_id) parts.push(`ID ${esc(f.patient_id)}${m(f.mrn_matches)}`);
  if (f.birth_date) parts.push(`born ${esc(f.birth_date)}${m(f.birth_date_matches)}`);
  if (f.encounter_id) parts.push(`encounter ${esc(f.encounter_id)}`);
  const more = Array.isArray(f.others) && f.others.length ? ` <span class="err">⚠ ${f.others.length} other patient(s) in the file</span>` : '';
  return `<div class="results">In the file: ${parts.join(' · ')}${more}</div>`;
}

function filesCell(f) {
  if (!f) return '';
  const n = f.agree + f.differ + f.unchecked;
  const counts = [[f.agree, 'agree'], [f.differ, 'differ'], [f.unchecked, 'not comparable'], [f.none, 'hold none'], [f.unread, 'not read yet']]
    .filter(([k]) => k).map(([k, w]) => `${k} ${w}`).join(', ');
  const title = esc([...f.found, counts ? `Files: ${counts}` : 'No recordings'].join('\n'));
  if (!n) return `<span class="muted" title="${title}">${f.unread ? 'reading…' : f.none ? 'none' : ''}</span>`;
  const mk = f.differ ? ' <span class="err">⚠</span>' : f.agree ? ' <span class="ok">✓</span>' : '';
  const more = f.found.length > 1 ? ` <span class="muted">+${f.found.length - 1}</span>` : '';
  return `<span class="nowrap" title="${title}">${esc(f.found[0] || '')}${more}${mk}</span>`;
}

function searchOf() {
  return {
    q: $('#enc-q').value.trim(), unit: $('#enc-unit').value.trim(), status: $('#enc-status').value,
    from: zonedMidnight($('#enc-from').value),
    to: zonedDayEnd($('#enc-to').value),
  };
}

async function load(background = false) {
  const data = await api(`encounters?${qs({ ...encState.search, limit: PAGE, offset: encState.offset })}`, { background });
  encState.total = data.total;
  encState.items = data.items;
  render();
  clearTimeout(encState.timer);
  // while recordings are indexed: asked again, only while the tab is shown (and the page seen)
  const shown = () => !$('#view-encounters')?.classList.contains('hidden');
  const poll = () => {
    if (!shown()) return;
    if (document.hidden) { encState.timer = setTimeout(poll, 3000); return; }
    load(true).catch(() => {});
  };
  if (shown() && encState.items.some((e) => e.status === 'queued' || e.status === 'indexing')) encState.timer = setTimeout(poll, 3000);
}

function render() {
  const { items, total, offset } = encState;
  const p = phi() ? '<th>MRN</th><th>Name</th><th class="nowrap">Birth date</th><th>In the files</th>' : '';
  let html = `<tr><th><input type="checkbox" id="enc-all" title="Select this page"></th><th></th><th>Encounter</th><th>Patient</th>${p}
    <th>Admit</th><th>Discharge</th><th>Units</th><th class="num">Recordings</th><th>Source</th></tr>`;
  if (!items.length) html += `<tr><td colspan="${phi() ? 13 : 9}" class="empty">No encounters.</td></tr>`;
  for (const e of items) {
    const ph = phi()
      ? `<td>${esc(e.phi?.mrn)}</td><td class="nowrap">${esc([e.phi?.last_name, e.phi?.first_name].filter(Boolean).join(', '))}</td><td class="nowrap">${esc(e.phi?.birth_date)}</td><td>${filesCell(e.in_files)}</td>`
      : '';
    const sel = encState.selected.has(e.id);
    html += `<tr class="row${sel ? ' sel' : ''}" data-enc="${e.id}">
      <td><input type="checkbox" data-sel="${e.id}"${sel ? ' checked' : ''}></td>
      <td title="${esc(statusCounts(e.status_counts))}">${dot(e.status)}</td>
      <td class="nowrap">${esc(e.encounter_id)}</td><td class="nowrap">${esc(e.patient_id)}</td>${ph}
      <td class="nowrap">${fmtTime(e.admit_time)}</td><td class="nowrap">${fmtTime(e.discharge_time)}</td>
      <td>${esc(e.units.join(', '))}</td><td class="num">${e.recordings}</td><td class="muted">${esc((e.sources || []).join(', '))}</td></tr>`;
  }
  $('#enc-table').innerHTML = html;
  $('#enc-count').textContent = total ? `${offset + 1}–${offset + items.length} of ${total} encounters` : '';
  $('#enc-prev').disabled = offset === 0;
  $('#enc-next').disabled = offset + items.length >= total;
  updateSelection();
}

export function updateSelection() {
  const n = encState.selected.size;
  $('#enc-selected').textContent = n ? `${n} selected` : '';
  $('#enc-run').disabled = !n;
  $('#enc-remove').disabled = !n;
  const l = $('#act-sel-label');
  if (l) l.textContent = `The selected encounters (${n})`;
}

function searchHelp() {
  const fields = ['encounter', 'patient', 'source', 'unit', 'bed', 'admit', 'discharge', 'disposition', 'sex', 'status', 'file']
    .concat(phi() ? ['name', 'first', 'last', 'dob', 'mrn', 'csn', 'file-name', 'file-id', 'file-dob'] : []);
  return `Words: each must appear in some field.   field:value: only in that field.   Quotes for spaces: source:"ICU pilot".
Never case-sensitive; a value matches anywhere in the field (name:smi finds Smith).
Fields: ${fields.join(', ')}`;
}

async function removeMatching() {
  const search = encState.search;
  const p = await api('encounters/remove-matching', { method: 'POST', json: { search, dry_run: true } });
  if (!p.encounters) return toast('The search finds no encounter', true);
  if (!confirm(`Remove what the search "${search.q || '(everything)'}" finds?\n\n${removalText(p)}\n\nRecordings stay in the catalog (unlinked); no file is touched.`)) return;
  const r = await api('encounters/remove-matching', { method: 'POST', json: { search } });
  toast(`Removed: ${removalText(r).split('\n')[0]}`);
  encState.selected.clear();
  load();
}

async function removeSelected() {
  const n = encState.selected.size;
  if (!confirm(`Remove ${n} encounter(s), their items and links from the catalog?\nRecordings stay in the catalog (unlinked); no file is touched.`)) return;
  const r = await api('encounters/remove', { method: 'POST', json: { ids: [...encState.selected] } });
  encState.selected.clear();
  toast(`${r.removed} encounter(s) removed`);
  load();
}

// ------------------------------------------------------------ the drawer
export async function openEncounter(id) {
  drawer.id = id;
  drawer.domain = '';
  const d = await api(`encounters/${id}`);
  renderDrawer(d);
  $('#drawer').classList.remove('hidden');
  loadEhr().catch(fail);
}
export function closeDrawer() {
  drawer.id = null;
  $('#drawer').classList.add('hidden');
}

function linkButtons(r, eid) {
  if (!isAdmin()) return '';
  return r.link === 'manual'
    ? `<button class="link" data-decide="" data-rec="${r.id}" data-eid="${esc(eid)}" title="Forget this manual link: automatic linking resumes">Forget manual link</button>`
    : `<button class="link danger" data-decide="never" data-rec="${r.id}" data-eid="${esc(eid)}" title="Unlink, and never link it to this encounter automatically again">Unlink</button>`;
}

export async function decideLink(rec, eid, decision) {
  await api(`recordings/${rec}/decision`, { method: 'POST', json: { encounter_id: eid, decision: decision || null } });
  toast(decision === 'never' ? 'Unlinked; it will not be linked to this encounter again' : decision === 'link' ? 'Link kept' : 'Manual link forgotten');
  if (drawer.id !== null) openEncounter(drawer.id).catch(fail);
}

function renderDrawer(d) {
  $('#drawer-title').textContent = `Encounter ${d.encounter_id}`;
  const rows = [['Patient', `<a href="#/patient/${d.patient}">${esc(d.patient_id)}</a>${d.sex ? ` · ${esc(d.sex)}` : ''}`]];
  if (d.phi) {
    rows.push(['Name', esc([d.phi.last_name, d.phi.first_name].filter(Boolean).join(', ') || '—')]);
    rows.push(['MRN', esc(d.phi.mrn || '—')], ['Birth date', esc(d.phi.birth_date || '—')], ['Encounter no.', esc(d.phi.csn || '—')]);
  }
  rows.push(['Admit', fmtTime(d.admit_time)], ['Discharge', fmtTime(d.discharge_time)]);
  if ((d.sources || []).length) rows.push(['Source', esc(d.sources.join(', '))]);
  if (d.disposition) rows.push(['Disposition', esc(d.disposition)]);
  let html = `<dl class="kv">${rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('')}</dl>`;
  html += '<h3>Bed stays</h3>';
  if (!d.bed_stays.length) html += '<p class="muted">None.</p>';
  else {
    html += '<table class="grid"><tr><th>Unit</th><th>Bed</th><th>From</th><th>To</th></tr>';
    for (const b of d.bed_stays) html += `<tr><td>${esc(b.unit || '—')}</td><td>${esc(b.bed || '—')}</td><td class="nowrap">${fmtTime(b.start_time)}</td><td class="nowrap">${fmtTime(b.end_time)}</td></tr>`;
    html += '</table>';
  }
  html += `<h3>Recordings (${d.recording_list.length})</h3>`;
  if (!d.recording_list.length) html += '<p class="muted">No recording linked to this encounter.</p>';
  else {
    html += '<table class="grid"><tr><th></th><th>Recording</th><th>Start</th><th>Length</th><th></th></tr>';
    for (const r of d.recording_list) {
      const len = r.t_end && r.t_start ? r.t_end - r.t_start : null;
      html += `<tr><td>${dot(r.status, r.progress)}</td>
        <td><div>${esc(r.name)} <span class="pill">${esc(r.kind)}${r.format ? ` · ${esc(r.format)}` : ''}</span> <span class="pill" title="How it was linked">${esc(r.link)}</span></div>
          ${r.error ? `<div class="err">${esc(r.error)}</div>` : ''}${inFile(r.in_file)}</td>
        <td class="nowrap">${fmtTime(r.t_start ?? r.name_time)}</td><td class="nowrap">${fmtDuration(len)}</td>
        <td class="nowrap">${recordingButtons(r)} ${linkButtons(r, d.encounter_id)}</td></tr>`;
    }
    html += '</table>';
  }
  html += '<h3>Items</h3>';
  if (!d.ehr_counts.length && !d.patient_level_items) html += '<p class="muted">None.</p>';
  else {
    const total = d.ehr_counts.reduce((a, [, n]) => a + n, 0);
    const pl = d.patient_level_items ? `<button data-domain="@patient">outside encounters (${d.patient_level_items})</button>` : '';
    html += `<div class="chips"><button data-domain="" class="on">All (${total})</button>${d.ehr_counts.map(([k, n]) => `<button data-domain="${esc(k)}">${esc(k)} (${n})</button>`).join('')}${pl}</div>
      <div class="table-wrap short"><table id="ehr-table" class="grid"></table></div>`;
  }
  if (isAdmin()) html += `<h3>Catalog</h3><button class="danger" data-remove-enc="${d.id}">Remove this encounter from the catalog</button>`;
  $('#drawer-body').innerHTML = html;
}

async function loadEhr() {
  const t = $('#ehr-table');
  if (!t || drawer.id === null) return;
  const pl = drawer.domain === '@patient';
  const data = await api(`encounters/${drawer.id}/ehr?${qs({ domain: pl ? '' : drawer.domain, patient_level: pl, limit: 2000 })}`);
  let html = '<tr><th>Time</th><th>Domain</th><th>Name</th><th>Value</th><th>Code</th><th>Source</th></tr>';
  for (const i of data.items) {
    const v = i.value === '' || i.value === null ? '' : `${esc(i.value)} ${esc(i.unit)}`;
    html += `<tr><td class="nowrap">${fmtTime(i.time)}</td><td>${esc(i.domain)}</td><td>${esc(i.name)}</td><td>${v}</td>
      <td class="nowrap muted">${esc(i.vocabulary)} ${esc(i.code)}</td><td class="muted">${esc(i.source)}</td></tr>`;
  }
  if (data.total > data.items.length) html += `<tr><td colspan="6" class="muted">${data.total - data.items.length} more not shown</td></tr>`;
  t.innerHTML = html;
}

export default {
  nav: 'patients',
  show: () => load(),
  hide: () => { clearTimeout(encState.timer); closeDrawer(); },
  wire() {
    $('#enc-search').addEventListener('submit', (e) => { e.preventDefault(); encState.search = searchOf(); encState.offset = 0; load().catch(fail); });
    for (const id of ['#enc-unit', '#enc-from', '#enc-to', '#enc-status']) {
      $(id).addEventListener('change', () => { encState.search = searchOf(); encState.offset = 0; load().catch(fail); });
    }
    $('#enc-prev').addEventListener('click', () => { encState.offset = Math.max(0, encState.offset - PAGE); load().catch(fail); });
    $('#enc-next').addEventListener('click', () => { encState.offset += PAGE; load().catch(fail); });
    $('#enc-run').addEventListener('click', () => go('#/actions'));
    $('#enc-remove').addEventListener('click', () => removeSelected().catch(fail));
    $('#enc-remove-matching').addEventListener('click', () => removeMatching().catch(fail));
    $('#enc-help').addEventListener('click', () => { const el = $('#enc-help-text'); el.textContent = searchHelp(); el.classList.toggle('hidden'); });
    $('#enc-table').addEventListener('click', (e) => {
      const all = e.target.closest('#enc-all');
      if (all) {
        for (const i of encState.items) all.checked ? encState.selected.add(i.id) : encState.selected.delete(i.id);
        render();
        return;
      }
      const sel = e.target.closest('input[data-sel]');
      if (sel) {
        const id = +sel.dataset.sel;
        sel.checked ? encState.selected.add(id) : encState.selected.delete(id);
        sel.closest('tr').classList.toggle('sel', sel.checked);
        updateSelection();
        return;
      }
      const row = e.target.closest('tr[data-enc]');
      if (row) openEncounter(+row.dataset.enc).catch(fail);
    });
    // the drawer
    $('#drawer-close').addEventListener('click', closeDrawer);
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeDrawer(); });
    wireRecordingButtons($('#drawer'), () => drawer.id !== null && openEncounter(drawer.id).catch(() => {}));
    $('#drawer').addEventListener('click', (e) => {
      const b = e.target.closest('[data-domain],[data-remove-enc],[data-decide]');
      if (!b) return;
      const d = b.dataset;
      if (d.domain !== undefined) {
        drawer.domain = d.domain;
        $$('#drawer [data-domain]').forEach((x) => x.classList.toggle('on', x === b));
        loadEhr().catch(fail);
      } else if (d.removeEnc) {
        if (confirm('Remove this encounter, its items and links from the catalog? Recordings stay (unlinked); no file is touched.')) {
          api('encounters/remove', { method: 'POST', json: { ids: [+d.removeEnc] } })
            .then(() => { closeDrawer(); toast('Encounter removed'); load().catch(() => {}); }).catch(fail);
        }
      } else if (d.decide !== undefined) {
        if (d.decide !== 'never' || confirm('Unlink this recording from the encounter? It will not be linked to it automatically again.')) decideLink(+d.rec, d.eid, d.decide).catch(fail);
      }
    });
  },
};
