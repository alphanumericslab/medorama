// Imports running on the server, wherever the user is in the page: started here (the upload
// shows its own progress), they run in the background (`import-tasks`); their progress is asked
// for every second or two, shown on the Import page and, on every other page, on the top bar's
// Import tab (a small bar and "47 %").  An import started in another tab or by the command line is not followed;
// one this page started and the server no longer knows (it restarted) is said to have stopped.

import { $, API, api, esc, toast } from './core.js';
import { checkData } from './live.js';

const KEPT = 'medorama.import-tasks';
const tasks = new Map(); // id → the task as the server last said
const listeners = [];
let waiters = [];
let timer = null;

/** Call `fn()` whenever a watched import moves or ends. */
export function onImports(fn) { listeners.push(fn); }

/** The imports watched: running first, then those ended (newest first). */
export function watched() {
  return [...tasks.values()].sort((a, b) => (a.state === 'running' ? 0 : 1) - (b.state === 'running' ? 0 : 1) || b.started - a.started);
}

function kept() {
  try { return JSON.parse(sessionStorage.getItem(KEPT) || '[]'); } catch { return []; }
}
function keep() {
  try { sessionStorage.setItem(KEPT, JSON.stringify([...tasks.values()].filter((t) => t.state === 'running').map((t) => t.id))); } catch { /* private mode */ }
}

/** Upload `form` to `path` (a background import), `onUpload(fraction, loaded, total)` as it
 * goes; returns the task the server started. */
export function upload(path, form, onUpload) {
  return new Promise((resolve, reject) => {
    const x = new XMLHttpRequest();
    x.open('POST', API + path);
    x.setRequestHeader('X-Medorama', '1');
    x.withCredentials = true;
    x.responseType = 'json';
    x.upload.onprogress = (e) => { if (e.lengthComputable) onUpload(e.loaded / e.total, e.loaded, e.total); };
    x.onload = () => {
      const data = x.response;
      if (x.status >= 200 && x.status < 300) resolve(data);
      else reject(new Error((data && data.detail) || `${x.status} ${x.statusText}`));
    };
    x.onerror = () => reject(new Error('The upload stopped (the connection to the server was lost).'));
    x.send(form);
  });
}

/** Follow an import the server runs (a task as `upload` returned it). */
export function watch(task) {
  tasks.set(task.id, task);
  keep();
  changed();
  poll();
}

/** The import's end (the task, done or failed), once it comes. */
export function whenDone(id) {
  return new Promise((resolve) => {
    const check = () => {
      const t = tasks.get(id);
      if (!t || t.state === 'running') return false;
      resolve(t);
      return true;
    };
    if (!check()) waiters.push(check);
  });
}

/** Ask the server to stop an import (it rolls back at its next chunk of rows). */
export async function cancel(id) {
  const t = await api(`import-tasks/${id}/cancel`, { method: 'POST' });
  tasks.set(id, t);
  changed();
}

/** Forget an ended import (its report closed). */
export function dismiss(id) {
  tasks.delete(id);
  keep();
  changed();
}

/** After signing in or a reload: the imports this tab started that may still run. */
export async function resume() {
  for (const id of kept()) if (!tasks.has(id)) tasks.set(id, { id, label: '', state: 'running', started: 0, progress: { phase: 'rows', done: 0, rows: 0 } });
  if (tasks.size) poll();
}

async function poll() {
  clearTimeout(timer);
  const running = [...tasks.values()].filter((t) => t.state === 'running');
  for (const t of running) {
    try {
      const v = await api(`import-tasks/${t.id}`, { background: true });
      tasks.set(t.id, v);
      if (v.state !== 'running') ended(v);
    } catch (e) {
      if (/no such import/.test(e.message)) {
        tasks.set(t.id, { ...t, state: 'failed', error: 'The import stopped when the server restarted; nothing of it was kept. Import the files again.', progress: { ...t.progress } });
        ended(tasks.get(t.id));
      }
      // else: the server is busy or restarting; ask again
    }
  }
  keep();
  changed();
  if ([...tasks.values()].some((t) => t.state === 'running')) {
    const here = location.hash.startsWith('#/import');
    timer = setTimeout(poll, here ? 1000 : 2000);
  }
}

function ended(t) {
  if (t.state === 'done') {
    const r = t.report || {};
    const what = r.tables ? `${t.label} imported` : `${t.label}: ${r.dry_run ? 'preview ready' : `${(r.rows || 0).toLocaleString()} rows imported`}`;
    toast(what);
    checkData();
  } else if (t.state === 'cancelled') {
    toast(`${t.label}: stopped; nothing of it was kept`);
  } else if (t.state === 'failed') {
    toast(`${t.label || 'The import'}: ${t.error}`, true);
  }
}

function changed() {
  showPill();
  waiters = waiters.filter((w) => !w());
  for (const fn of listeners) fn();
}

/** The Import tab's indicator, on every page: the running imports' progress. */
function showPill() {
  const pill = $('#import-pill');
  if (!pill) return;
  const running = [...tasks.values()].filter((t) => t.state === 'running');
  pill.classList.toggle('hidden', !running.length);
  if (!running.length) return;
  const done = running.reduce((s, t) => s + (t.progress?.done || 0), 0) / running.length;
  const pct = Math.floor(done * 100);
  pill.querySelector('.lbl').textContent = `${pct} %`;
  pill.setAttribute('aria-label', running.length > 1 ? `${running.length} imports running, ${pct} %` : `Importing, ${pct} %`);
  pill.querySelector('.bar > i').style.width = `${Math.max(2, pct)}%`;
  pill.title = running.map((t) => `${t.label}: ${describe(t.progress)}`).join('\n');
}

/** A task's progress in words: what it does, how many rows, how fast, how long still. */
export function describe(p) {
  if (!p) return '';
  if (p.cancelling) return 'Stopping (rolling back)…';
  const what = { waiting: 'Waiting for another import to end', rows: 'Reading and linking rows', index: 'Building the search index', match: 'Matching waiting items and recordings', done: 'Done' }[p.phase] || '';
  const bits = [what];
  if (p.rows) bits.push(`${p.rows.toLocaleString()} rows`);
  if (p.phase === 'rows' && p.rate) bits.push(`${p.rate.toLocaleString()} rows/s`);
  if (p.left_s != null) bits.push(`about ${duration(p.left_s)} left`);
  return bits.join(' · ');
}

/** Seconds as a person reads them. */
export function duration(s) {
  s = Math.max(0, Math.round(s));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`;
}

/** Bytes as a person reads them. */
export function size(n) {
  if (n < 1e6) return `${Math.max(1, Math.round(n / 1e3))} kB`;
  if (n < 1e9) return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB`;
  return `${(n / 1e9).toFixed(1)} GB`;
}

/** A progress block (the Import page's): label, bar, words, and Cancel while it runs. */
export function progressHtml(t, { upload: up } = {}) {
  const p = t.progress || {};
  const running = t.state === 'running';
  const pct = up ? Math.floor(up.fraction * 100) : Math.floor((p.done || 0) * 100);
  const words = up ? `Uploading ${size(up.loaded)} of ${size(up.total)}` : running ? describe(p) : '';
  return `<div class="progress${running || up ? '' : ' ended'}" role="group" aria-label="${esc(t.label)}">
    <div class="ftools"><b>${esc(t.label)}</b><span class="muted">${up ? 'uploading' : running ? (p.cancelling ? 'stopping' : 'importing') : ''}</span>
      <span class="spacer"></span>${running && !p.cancelling && !up ? `<button class="small" data-task-cancel="${esc(t.id)}">Cancel</button>` : ''}</div>
    <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"><i data-w="${Math.max(1, pct)}"></i></div>
    <div class="muted small">${pct} %${words ? ` · ${esc(words)}` : ''}</div>
  </div>`;
}

/** The bars `progressHtml` drew, at their widths (the page's policy allows no inline style). */
export function sizeBars(root) {
  for (const i of root.querySelectorAll('.bar > i[data-w]')) i.style.width = `${i.dataset.w}%`;
}
