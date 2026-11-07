// Patient Catalog: every patient, latest encounter first; the search line finds a patient by
// name, MRN, ID, date of birth or encounter (identifiers only with the PHI permission); the
// chips filter by kind of data: a click keeps the patients who have it, a second those who have
// none, a third clears it (?has=ecg,!icu); ?cohort=… shows a cohort's members only.  "Add to
// cohort…" adds every patient the search finds to a cohort (the server runs the same search).

import { $, PAGE, ages, api, catClass, esc, fail, fmtDate, phi, privacy, qs, toast } from '../core.js';
import { go, href, setParams } from '../router.js';

// `shown`: the search whose result is on screen ("Add to cohort" adds what it found); `seq`: the
// newest list asked for (an older answer arriving after it is dropped)
const pl = { offset: 0, q: '', has: [], cohort: '', cohortName: '', timer: null, items: [], total: 0, targets: [], shown: null, seq: 0 };

async function load() {
  const asked = { q: pl.q, has: [...pl.has], cohort: pl.cohort, cohortName: pl.cohortName };
  const seq = ++pl.seq;
  const data = await api(`patients?${qs({ q: asked.q, has: asked.has.join(','), cohort: asked.cohort, limit: PAGE, offset: pl.offset })}`);
  if (seq !== pl.seq) return;
  pl.shown = asked;
  $('#pl-crumb').innerHTML = pl.cohort
    ? `<span class="chip">Cohort: ${esc(pl.cohortName)}<button data-uncohort title="Every patient" aria-label="Every patient">✕</button></span>`
    : 'Patient Catalog';
  pl.items = data.items;
  const state = (k) => (pl.has.includes(k) ? 'on' : pl.has.includes(`!${k}`) ? 'out' : '');
  const tip = { '': 'Click: only patients with', on: 'Click again: only patients without', out: 'Click again: no filter on' };
  $('#pl-has').innerHTML = data.categories.map(([k, l]) => `<button class="f ${state(k)}" data-has="${esc(k)}" aria-pressed="${state(k) ? 'true' : 'false'}" title="${esc(`${tip[state(k)]} ${l}`)}"><span class="sw ${catClass(k)}"></span>${esc(l)}</button>`).join('');
  $('#pl-clear').hidden = !pl.has.length;
  const lbl = Object.fromEntries(data.categories);
  const withs = pl.has.filter((h) => !h.startsWith('!')).map((h) => lbl[h] || h);
  const withouts = pl.has.filter((h) => h.startsWith('!')).map((h) => lbl[h.slice(1)] || h.slice(1));
  $('#pl-sum').innerHTML = pl.has.length
    ? `Patients${withs.length ? ` with <b>${withs.map(esc).join('</b> and <b>')}</b>` : ''}${withs.length && withouts.length ? ',' : ''}${withouts.length ? ` without <b>${withouts.map(esc).join('</b> or <b>')}</b>` : ''}`
    : '';
  const p = phi();
  let html = `<tr><th>Patient</th><th>Patient ID</th>${p ? '<th>MRN</th>' : ''}<th>Sex</th>${ages() ? '<th class="num">Age</th>' : ''}${p ? '<th>Born</th>' : ''}<th class="num">Encounters</th><th>Latest encounter</th><th>Where</th><th>Data</th><th class="num">Items</th></tr>`;
  if (!data.items.length) html += `<tr><td colspan="11" class="empty">${asked.q || asked.has.length ? 'No patient matches.' : 'No patients yet: add databases in Import.'}</td></tr>`;
  const labels = Object.fromEntries(data.categories);
  for (const r of data.items) {
    const name = p ? [r.phi?.last_name, r.phi?.first_name].filter(Boolean).join(', ') || '—' : r.patient_id;
    const mrn = p ? (r.phi?.mrns || []).map((m) => m[1]).join(', ') : '';
    // and the connected databases that hold their data (read when their page opens)
    const kinds = [...r.kinds.map((k) => esc(labels[k] || k)), ...(r.sources || []).map((n) => `<span class="pill" title="Data in the connected database ${esc(n)}">${esc(n)}</span>`)].join(' · ');
    const to = esc(href('patient', [String(r.id)]));
    html += `<tr data-href="${to}"><td class="nm"><a href="${to}">${esc(name)}</a></td><td class="mono">${esc(r.patient_id)}</td>${p ? `<td class="mono">${esc(mrn)}</td>` : ''}
      <td>${esc(r.sex)}</td>${ages() ? `<td class="num">${esc(r.age ?? '')}</td>` : ''}${p ? `<td>${esc(r.phi?.birth_date || '')}</td>` : ''}<td class="num">${r.encounters}</td>
      <td>${r.latest ? fmtDate(r.latest) : '<span class="faint">—</span>'}</td><td>${esc(r.latest_where)}</td><td><span class="kinds">${kinds}</span></td><td class="num">${r.items}</td></tr>`;
  }
  $('#pl-table').innerHTML = html;
  const o = pl.offset;
  pl.total = data.total;
  $('#pl-count').textContent = `${data.total} patient${data.total === 1 ? '' : 's'}`;
  if (!$('#pl-addto').classList.contains('hidden')) addToPanel().catch(fail);
  $('#pl-page').textContent = data.total ? `${o + 1}–${o + data.items.length} of ${data.total}` : '';
  $('#pl-prev').disabled = o === 0;
  $('#pl-next').disabled = o + data.items.length >= data.total;
}

// ------------------------------------------------------------ add to a cohort
/** What the search is, in words, for the panel. */
function searchWords() {
  const s = pl.shown || pl;
  const parts = [];
  if (s.q) parts.push(`“${esc(s.q)}”`);
  if (s.has.length) parts.push('the filters');
  if (s.cohort) parts.push(`among “${esc(s.cohortName)}”`);
  return parts.length ? `this search (${parts.join(', ')}) finds` : 'the catalog holds (no search)';
}

async function addToPanel() {
  pl.targets = (await api('cohorts')).items.filter((c) => c.can_edit && String(c.id) !== pl.cohort);
  const n = pl.total;
  $('#pl-addto').innerHTML = pl.targets.length
    ? `<b>Add to a cohort</b>
      <p class="muted small">The ${n} patient${n === 1 ? '' : 's'} ${searchWords()}. Those the cohort's definition does not select are listed in it as added manually.</p>
      <div class="ftools"><select id="pl-addto-co" aria-label="Cohort">${pl.targets.map((c) => `<option value="${c.id}">${esc(c.name)} (${c.members})</option>`).join('')}</select>
        <button class="btn primary" id="pl-addto-go"${n ? '' : ' disabled'}>Add ${n} patient${n === 1 ? '' : 's'}</button><button class="btn" id="pl-addto-close">Cancel</button></div>`
    : `<b>Add to a cohort</b><p class="muted small">There is no cohort you may change. <a href="#/cohorts">Make one in Cohorts</a>.</p>
      <div class="ftools"><button class="btn" id="pl-addto-close">Close</button></div>`;
  $('#pl-addto').classList.remove('hidden');
}

async function addTo() {
  const target = pl.targets.find((c) => String(c.id) === $('#pl-addto-co').value);
  if (!target) return;
  // the search whose result the panel counted, not a newer one being typed
  const s = pl.shown || pl;
  const c = await api(`cohorts/${target.id}`, {
    method: 'PATCH',
    json: { add_search: { q: s.q, has: s.has, cohort: s.cohort ? +s.cohort : null } },
  });
  const added = c.members - target.members;
  const already = pl.total - added;
  toast(`${added} patient${added === 1 ? '' : 's'} added to “${c.name}”${already > 0 ? ` (${already} already in it)` : ''}`);
  $('#pl-addto').classList.add('hidden');
}

export default {
  async show(r) {
    pl.q = r.params.get('q') || '';
    pl.has = (r.params.get('has') || '').split(',').filter(Boolean);
    pl.offset = +(r.params.get('o') || 0);
    pl.cohort = r.params.get('cohort') || '';
    if (pl.cohort) pl.cohortName = (await api(`cohorts/${pl.cohort}`).catch(() => ({ name: pl.cohort }))).name;
    $('#pl-q').value = pl.q;
    $('#pl-q').placeholder = privacy.on
      ? 'Search: pseudonym (MO-…), encounter, unit, sex… (Privacy Mode: no identifiers)'
      : 'Search: name, MRN, patient ID, date of birth, encounter…';
    return load();
  },
  wire() {
    $('#pl-q').addEventListener('input', (e) => {
      clearTimeout(pl.timer);
      pl.timer = setTimeout(() => {
        pl.q = e.target.value.trim();
        pl.offset = 0;
        setParams({ q: pl.q, o: '' });
        load().catch(fail);
      }, 220);
    });
    $('#pl-q').addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && pl.items.length === 1) go(href('patient', [String(pl.items[0].id)])).catch(fail);
    });
    $('#pl-crumb').addEventListener('click', (e) => {
      if (!e.target.closest('[data-uncohort]')) return;
      pl.cohort = '';
      pl.offset = 0;
      setParams({ cohort: '', o: '' });
      load().catch(fail);
    });
    $('#pl-has').addEventListener('click', (e) => {
      const b = e.target.closest('[data-has]');
      if (!b) return;
      // off → with → without → off
      const k = b.dataset.has;
      if (pl.has.includes(k)) pl.has = pl.has.map((x) => (x === k ? `!${k}` : x));
      else if (pl.has.includes(`!${k}`)) pl.has = pl.has.filter((x) => x !== `!${k}`);
      else pl.has = [...pl.has, k];
      pl.offset = 0;
      setParams({ has: pl.has.join(','), o: '' });
      load().catch(fail);
    });
    $('#pl-clear').addEventListener('click', () => {
      pl.has = [];
      pl.offset = 0;
      setParams({ has: '', o: '' });
      load().catch(fail);
    });
    $('#pl-addto-open').addEventListener('click', () => addToPanel().catch(fail));
    $('#pl-addto').addEventListener('click', (e) => {
      if (e.target.id === 'pl-addto-close') $('#pl-addto').classList.add('hidden');
      if (e.target.id === 'pl-addto-go') addTo().catch(fail);
    });
    $('#pl-table').addEventListener('click', (e) => {
      // the name is a link (keyboard, a new tab); the rest of the row opens it too
      if (e.target.closest('a[href]')) return;
      const row = e.target.closest('tr[data-href]');
      if (row) go(row.dataset.href).catch(fail);
    });
    $('#pl-prev').addEventListener('click', () => { pl.offset = Math.max(0, pl.offset - PAGE); setParams({ o: pl.offset || '' }); load().catch(fail); });
    $('#pl-next').addEventListener('click', () => { pl.offset += PAGE; setParams({ o: pl.offset }); load().catch(fail); });
    document.addEventListener('keydown', (e) => {
      if (e.key !== '/' || ['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName)) return;
      const box = !$('#view-patients').classList.contains('hidden') ? $('#pl-q') : !$('#view-patient').classList.contains('hidden') ? $('#pp-find') : null;
      if (box) { e.preventDefault(); box.focus(); }
    });
  },
};
