// History: the patients this user opened, most recent first (only this user sees it).

import { $, api, esc, fail, fmtTime } from '../core.js';
import { go, href } from '../router.js';

async function load() {
  const data = await api('history');
  let html = '<tr><th>Patient</th><th>First opened</th><th>Last opened</th><th class="num">Times</th><th></th></tr>';
  if (!data.items.length) html += '<tr><td colspan="5" class="empty">Your history is empty.</td></tr>';
  for (const h of data.items) {
    html += `<tr class="row" data-patient="${h.patient}"><td class="mono"><a href="${esc(href('patient', [String(h.patient)]))}">${esc(h.pseudonym)}</a></td>
      <td class="nowrap">${fmtTime(h.first_seen)}</td><td class="nowrap">${fmtTime(h.last_seen)}</td><td class="num">${h.views}</td>
      <td><button class="link" data-forget="${h.patient}" title="Remove from your history">✕</button></td></tr>`;
  }
  $('#hist-table').innerHTML = html;
}

export default {
  show: () => load(),
  wire() {
    $('#hist-table').addEventListener('click', (e) => {
      const f = e.target.closest('[data-forget]');
      if (f) return api(`history/${f.dataset.forget}`, { method: 'DELETE' }).then(load).catch(fail);
      const row = e.target.closest('tr[data-patient]');
      if (row && !e.target.closest('a')) go(href('patient', [row.dataset.patient])).catch(fail);
    });
    $('#hist-clear').addEventListener('click', () => {
      if (confirm('Clear your history?')) api('history', { method: 'DELETE' }).then(load).catch(fail);
    });
  },
};
