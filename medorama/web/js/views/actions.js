// Actions: run actions (indexing, plug-ins) on selected encounters, a search, a cohort's
// members (#/actions?cohort=…), or everything.

import { $, $$, api, canRun, esc, fail, store, toast } from '../core.js';
import { encState, updateSelection } from './encounters.js';
import { pollJobs } from './jobs.js';

async function load(r) {
  const [data, cohorts] = await Promise.all([api('actions'), api('cohorts')]);
  const want = r?.params.get('cohort') || '';
  $('#act-cohort').innerHTML = cohorts.items.length
    ? cohorts.items.map((c) => `<option value="${c.id}"${String(c.id) === want ? ' selected' : ''}>${esc(c.name)} (${c.members})</option>`).join('')
    : '<option value="">no cohort yet</option>';
  if (want) $$('input[name="act-scope"]').forEach((i) => { i.checked = i.value === 'cohort'; });
  const ticked = store.get('actions', null);
  let html = '<tr><th></th><th>Action</th><th>Version</th><th>Requires</th><th>Description</th></tr>';
  for (const a of data.actions) {
    const on = ticked ? ticked.includes(a.name) : a.default;
    html += `<tr><td><input type="checkbox" data-action="${esc(a.name)}" aria-label="Run ${esc(a.name)}"${on && !a.problem ? ' checked' : ''}${a.problem ? ' disabled' : ''}></td>
      <td class="nowrap">${esc(a.title)}${a.builtin ? ' <span class="pill">built in</span>' : ''}</td><td>${esc(a.version)}</td>
      <td>${esc(a.requires.join(', '))}</td><td>${esc(a.description)}${a.problem ? `<div class="err">${esc(a.problem)}</div>` : ''}</td></tr>`;
  }
  $('#act-table').innerHTML = html;
  $('#act-problems').classList.toggle('hidden', !data.problems.length);
  $('#act-problems').textContent = data.problems.length ? `Not loaded: ${data.problems.join('; ')}` : '';
  $('#act-run').disabled = !canRun();
  updateSelection();
}

async function run() {
  const names = $$('#act-table input[data-action]:checked').map((i) => i.dataset.action);
  store.set('actions', names);
  if (!names.length) return toast('Tick at least one action', true);
  const scope = $$('input[name="act-scope"]').find((i) => i.checked).value;
  let selection;
  if (scope === 'selected') {
    if (!encState.selected.size) return toast('No encounter selected (select some in Patient Catalog › Encounters)', true);
    selection = { encounters: [...encState.selected] };
  } else if (scope === 'search') selection = { search: encState.search };
  else if (scope === 'cohort') {
    if (!$('#act-cohort').value) return toast('No cohort yet: make one in Cohorts', true);
    selection = { cohort: +$('#act-cohort').value };
  }
  else {
    if (!confirm('Run on every recording in the catalog?')) return;
    selection = { all: true };
  }
  const r = await api('runs', { method: 'POST', json: { actions: names, selection, force: $('#act-force').checked } });
  $('#act-report').textContent = `Run ${r.run}: ${r.recordings} recording(s), ${r.queued} job(s) queued; ${r.done_already} already done, ${r.pending_already} already queued, ${r.not_applicable} not applicable.`;
  pollJobs();
}

export default {
  show: (r) => load(r),
  wire() { $('#act-run').addEventListener('click', () => run().catch(fail)); },
};
