// History: the encounters this user opened, most recent first (only this user sees it).

import { $, api, dot, esc, fail, fmtTime, phi } from '../core.js';
import { openEncounter } from './encounters.js';

async function load() {
  const data = await api('history');
  const p = phi() ? '<th>Name</th>' : '';
  let html = `<tr><th></th><th>Encounter</th><th>Patient</th>${p}<th>Admit</th><th>Last opened</th><th class="num">Times</th><th></th></tr>`;
  if (!data.items.length) html += `<tr><td colspan="${phi() ? 8 : 7}" class="empty">Your history is empty.</td></tr>`;
  for (const h of data.items) {
    const name = phi() ? `<td>${esc([h.phi?.last_name, h.phi?.first_name].filter(Boolean).join(', '))}</td>` : '';
    html += `<tr class="row" data-enc="${h.id}"><td>${dot(h.status)}</td><td>${esc(h.encounter_id)}</td><td>${esc(h.patient_id)}</td>${name}
      <td class="nowrap">${fmtTime(h.admit_time)}</td><td class="nowrap">${fmtTime(h.last_seen)}</td><td class="num">${h.views}</td>
      <td><button class="link" data-forget="${h.id}" title="Remove from your history">✕</button></td></tr>`;
  }
  $('#hist-table').innerHTML = html;
}

export default {
  show: () => load(),
  wire() {
    $('#hist-table').addEventListener('click', (e) => {
      const f = e.target.closest('[data-forget]');
      if (f) return api(`history/${f.dataset.forget}`, { method: 'DELETE' }).then(load).catch(fail);
      const row = e.target.closest('tr[data-enc]');
      if (row) openEncounter(+row.dataset.enc).catch(fail);
    });
    $('#hist-clear').addEventListener('click', () => {
      if (confirm('Clear your history?')) api('history', { method: 'DELETE' }).then(load).catch(fail);
    });
  },
};
