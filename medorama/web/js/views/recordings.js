// Recordings: the recordings of the recording databases the user may view, with status and
// actions.  Medorama only reads them: a recording leaves when its database does.

import { $, PAGE, api, canRun, dot, esc, fail, fmtDuration, fmtTime, isAdmin, qs, toast, viewerMode, externalViewerLink } from '../core.js';
import { href } from '../router.js';

const rec = { offset: 0, total: 0, timer: null };

/** View (opens the Viewer screen), Copy path, Index now. */
export function viewButton(r, t0, t1) {
  const mode = viewerMode();
  if (r.status === 'missing') return '<button class="link" disabled title="The recording is missing">View</button>';
  if (mode === 'embedded') return `<a class="btnlink" href="${esc(href('viewer', [], { path: r.path, t0, t1 }))}">View</a>`;
  if (mode === 'link') return `<a href="${esc(externalViewerLink(r.path, t0, t1))}" target="vitalstream-viewer" rel="noopener noreferrer">View</a>`;
  return '<button class="link" disabled>View</button>';
}

export function recordingButtons(r) {
  const out = [viewButton(r), `<button class="link" data-copy="${esc(r.path)}" title="Copy the recording's path">Copy path</button>`];
  if (canRun() && r.status !== 'ready' && r.status !== 'missing') out.push(`<button class="link" data-index="${r.id}">Index now</button>`);
  return out.join(' ');
}

export async function indexNow(id) {
  const r = await api('runs', { method: 'POST', json: { actions: ['index'], selection: { recordings: [id] }, priority: 10 } });
  toast(r.queued ? 'Indexing queued' : 'Already indexed or queued');
}

async function load(background = false) {
  const data = await api(`recordings?${qs({ q: $('#rec-q').value.trim(), limit: PAGE, offset: rec.offset })}`, { background });
  rec.total = data.total;
  let html = '<tr><th></th><th>Recording</th><th>Kind</th><th>Start</th><th>Length</th><th>Patient</th><th>Database</th><th>Path</th><th></th></tr>';
  if (!data.items.length) html += `<tr><td colspan="9" class="empty">No recordings.${isAdmin() ? ' Connect a recording database in Databases.' : ''}</td></tr>`;
  for (const r of data.items) {
    const len = r.t_end && r.t_start ? r.t_end - r.t_start : null;
    const who = r.patient ? `<a href="${esc(href('patient', [String(r.patient)]))}">patient</a>` : '<span class="faint">—</span>';
    html += `<tr><td>${dot(r.status, r.progress)}</td><td>${esc(r.name)}${r.error ? `<div class="err">${esc(r.error)}</div>` : ''}</td>
      <td class="nowrap">${esc(r.kind)}${r.format ? ` · ${esc(r.format)}` : ''}</td><td class="nowrap">${fmtTime(r.t_start)}</td>
      <td class="nowrap">${fmtDuration(len)}</td><td class="nowrap">${who}</td><td class="muted">${esc(r.source)}</td>
      <td class="path">${esc(r.path)}</td><td class="nowrap">${recordingButtons(r)}</td></tr>`;
  }
  $('#rec-table').innerHTML = html;
  const o = rec.offset;
  $('#rec-count').textContent = data.total ? `${o + 1}–${o + data.items.length} of ${data.total} recordings` : '';
  $('#rec-prev').disabled = o === 0;
  $('#rec-next').disabled = o + data.items.length >= data.total;
  clearTimeout(rec.timer);
  if (data.items.some((r) => r.status === 'queued' || r.status === 'indexing')) rec.timer = setTimeout(() => load(true).catch(() => {}), 3000);
}

/** Buttons shared by every table showing recordings (Copy path, Index now). */
export function wireRecordingButtons(root, after) {
  root.addEventListener('click', (e) => {
    const b = e.target.closest('[data-index],[data-copy]');
    if (!b) return;
    const d = b.dataset;
    if (d.index) indexNow(+d.index).then(after).catch(fail);
    else if (d.copy) navigator.clipboard.writeText(d.copy).then(() => toast('Path copied'), () => prompt('Path:', d.copy));
  });
}

export default {
  nav: 'databases',
  show: () => load(),
  hide: () => clearTimeout(rec.timer),
  wire() {
    $('#rec-search').addEventListener('submit', (e) => { e.preventDefault(); rec.offset = 0; load().catch(fail); });
    $('#rec-prev').addEventListener('click', () => { rec.offset = Math.max(0, rec.offset - PAGE); load().catch(fail); });
    $('#rec-next').addEventListener('click', () => { rec.offset += PAGE; load().catch(fail); });
    $('#chk-run').addEventListener('click', () => {
      api('check', { method: 'POST' })
        .then((r) => { toast(`${r.checked} checked: ${r.missing} missing, ${r.returned} back, ${r.changed} changed, ${r.index_lost} index folders gone`); return load(); })
        .catch(fail);
    });
    wireRecordingButtons($('#view-recordings'), () => load().catch(() => {}));
  },
};
