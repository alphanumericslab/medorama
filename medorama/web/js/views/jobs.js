// Jobs: the queue (indexing and other actions), refreshed while something runs; the badge in
// the top bar counts unfinished jobs.

import { $, $$, api, canRun, dot, esc, fail, fmtTime, qs } from '../core.js';

const jobs = { timer: null, shown: false };

function badge(counts) {
  const n = (counts.running || 0) + (counts.queued || 0);
  $('#jobs-badge').textContent = n;
  $('#jobs-badge').classList.toggle('hidden', !n);
  return n;
}

async function load(background = false) {
  const data = await api(`jobs?${qs({ state: $('#job-state').value, limit: 500 })}`, { background });
  let html = '<tr><th>Job</th><th>State</th><th>Action</th><th>Recording</th><th>Progress</th><th>Queued</th><th>Finished</th><th></th></tr>';
  if (!data.items.length) html += '<tr><td colspan="8" class="empty">No jobs.</td></tr>';
  for (const j of data.items) {
    const busy = j.state === 'running' || j.state === 'queued';
    const bar = j.state === 'running' ? `<span class="bar"><i data-w="${Math.round(j.progress * 100)}"></i></span> ${Math.round(j.progress * 100)}%` : '';
    html += `<tr><td class="num">${j.id}</td><td>${dot({ running: 'indexing', done: 'ready', cancelled: 'new' }[j.state] || j.state)} ${esc(j.state)}</td>
      <td>${esc(j.action)}</td><td>${esc(j.recording_name)}${j.error ? `<div class="err">${esc(j.error)}</div>` : ''}</td><td class="nowrap">${bar}</td>
      <td class="nowrap">${fmtTime(j.created_at)}</td><td class="nowrap">${fmtTime(j.finished_at)}</td>
      <td>${busy && canRun() ? `<button class="link danger" data-cancel="${j.id}">Cancel</button>` : ''}</td></tr>`;
  }
  $('#job-table').innerHTML = html;
  $$('#job-table .bar > i').forEach((i) => { i.style.width = `${i.dataset.w}%`; });
  $('#job-counts').textContent = Object.entries(data.counts).map(([k, n]) => `${n} ${k}`).join(' · ');
  const n = badge(data.counts);
  clearTimeout(jobs.timer);
  if (n && jobs.shown) jobs.timer = setTimeout(() => load(true).catch(() => {}), 3000);
}

/** Refresh the badge; keep refreshing while jobs are unfinished. */
export async function pollJobs() {
  try {
    // the page's own refresh: it does not keep an idle session alive
    const data = await api('jobs?limit=1', { background: true });
    if (badge(data.counts) && !jobs.shown) {
      clearTimeout(jobs.timer);
      jobs.timer = setTimeout(pollJobs, 5000);
    }
  } catch { /* signed out */ }
}

export default {
  show: () => { jobs.shown = true; return load(); },
  hide: () => { jobs.shown = false; pollJobs(); },
  wire() {
    $('#job-state').addEventListener('change', () => load().catch(fail));
    $('#job-cancel-all').addEventListener('click', () => {
      if (confirm('Cancel every queued and running job?')) api('jobs/cancel', { method: 'POST', json: {} }).then(load).catch(fail);
    });
    $('#job-table').addEventListener('click', (e) => {
      const b = e.target.closest('[data-cancel]');
      if (b) api('jobs/cancel', { method: 'POST', json: { job: +b.dataset.cancel } }).then(load).catch(fail);
    });
  },
};
