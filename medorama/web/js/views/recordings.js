// Recordings: every recording in the catalog, with status, links and actions.

import { $, PAGE, api, canRun, dot, esc, fail, fmtDuration, fmtTime, isAdmin, qs, toast, viewerMode, externalViewerLink } from '../core.js';
import { href } from '../router.js';
import { inFile } from './encounters.js';

const rec = { offset: 0, total: 0, timer: null };

/** View (opens the Viewer screen), Copy path, Index now, Remove. */
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
  if (isAdmin()) out.push(`<button class="link danger" data-remove-rec="${r.id}">Remove</button>`);
  return out.join(' ');
}

export async function indexNow(id) {
  const r = await api('runs', { method: 'POST', json: { actions: ['index'], selection: { recordings: [id] }, priority: 10 } });
  toast(r.queued ? 'Indexing queued' : 'Already indexed or queued');
}

async function removeRecording(id) {
  if (!confirm('Remove this recording from the catalog?\nThe file itself is not touched.')) return;
  const purge = confirm("Also delete the catalog's index and results for it (in the results folder only)?");
  const r = await api('recordings/remove', { method: 'POST', json: { ids: [id], purge } });
  toast(`${r.removed} recording removed from the catalog`);
}

async function load(background = false) {
  const data = await api(`recordings?${qs({ q: $('#rec-q').value.trim(), unlinked: $('#rec-unlinked').checked, limit: PAGE, offset: rec.offset })}`, { background });
  rec.total = data.total;
  let html = '<tr><th></th><th>Recording</th><th>Kind</th><th>Start</th><th>Length</th><th>Encounters</th><th>Source</th><th>Path</th><th></th></tr>';
  if (!data.items.length) html += `<tr><td colspan="9" class="empty">No recordings.${isAdmin() ? ' Add some in Import.' : ''}</td></tr>`;
  for (const r of data.items) {
    const len = r.t_end && r.t_start ? r.t_end - r.t_start : null;
    html += `<tr><td>${dot(r.status, r.progress)}</td><td>${esc(r.name)}${r.error ? `<div class="err">${esc(r.error)}</div>` : ''}${inFile(r.in_file)}</td>
      <td class="nowrap">${esc(r.kind)}${r.format ? ` · ${esc(r.format)}` : ''}</td><td class="nowrap">${fmtTime(r.t_start ?? r.name_time)}</td>
      <td class="nowrap">${fmtDuration(len)}</td><td class="nowrap">${esc((r.encounters || []).join(', '))}</td><td class="muted">${esc((r.sources || []).join(', '))}</td>
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

async function removeMatching() {
  const body = { q: $('#rec-q').value.trim(), unlinked: $('#rec-unlinked').checked };
  const p = await api('recordings/remove-matching', { method: 'POST', json: { ...body, dry_run: true } });
  if (!p.recordings) return toast('The search finds no recording', true);
  if (!confirm(`Remove the ${p.recordings} recording(s) the search finds from the catalog?\nThe files are not touched.`)) return;
  const purge = confirm("Also delete the catalog's indexes and results for them (in the results folder only)?");
  const r = await api('recordings/remove-matching', { method: 'POST', json: { ...body, purge } });
  toast(`${r.recordings} recording(s) removed from the catalog`);
  load();
}

/** Buttons shared by every table showing recordings (Copy path, Index now, Remove). */
export function wireRecordingButtons(root, after) {
  root.addEventListener('click', (e) => {
    const b = e.target.closest('[data-index],[data-remove-rec],[data-copy]');
    if (!b) return;
    const d = b.dataset;
    if (d.index) indexNow(+d.index).then(after).catch(fail);
    else if (d.removeRec) removeRecording(+d.removeRec).then(after).catch(fail);
    else if (d.copy) navigator.clipboard.writeText(d.copy).then(() => toast('Path copied'), () => prompt('Path:', d.copy));
  });
}

export default {
  nav: 'import',
  show: () => load(),
  hide: () => clearTimeout(rec.timer),
  wire() {
    $('#rec-search').addEventListener('submit', (e) => { e.preventDefault(); rec.offset = 0; load().catch(fail); });
    $('#rec-unlinked').addEventListener('change', () => { rec.offset = 0; load().catch(fail); });
    $('#rec-prev').addEventListener('click', () => { rec.offset = Math.max(0, rec.offset - PAGE); load().catch(fail); });
    $('#rec-next').addEventListener('click', () => { rec.offset += PAGE; load().catch(fail); });
    $('#rec-remove-matching').addEventListener('click', () => removeMatching().catch(fail));
    $('#chk-run').addEventListener('click', () => {
      api('check', { method: 'POST' })
        .then((r) => { toast(`${r.checked} checked: ${r.missing} missing, ${r.returned} back, ${r.changed} changed, ${r.index_lost} index folders gone`); return load(); })
        .catch(fail);
    });
    wireRecordingButtons($('#view-recordings'), () => load().catch(() => {}));
  },
};
