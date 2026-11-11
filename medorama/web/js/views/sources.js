// Import (admins): add databases (CSV, spreadsheets), and see and remove what each source
// contributes.  Databases are the only way in: a file (a recording, an ECG, an image) enters as
// a row that names its patient and links the file, never on its own.

import { $, api, esc, fail, fmtTime, qs, removalText, toast } from '../core.js';
import { reviewBadge } from './review.js';
import { checkData } from '../live.js';
import { cancel, dismiss, onImports, progressHtml, sizeBars, upload, watch, watched, whenDone } from '../imports.js';
import { chooseOnServer } from '../serverfiles.js';

const imp = { tables: [], mappings: [], formats: [], categories: [], problems: [], kind: 'import', busy: false, uploading: null, ours: new Set() };

const LABELS = {
  patient_id: 'patient ID', mrn: 'MRN', mrn_authority: 'MRN authority', last_name: 'last name', first_name: 'first name', birth_date: 'date of birth',
  sex: 'sex', encounter_id: 'encounter ID', csn: 'encounter number', admit_time: 'admission', discharge_time: 'discharge', admit_unit: 'admitting unit',
  disposition: 'disposition', care_unit: 'unit', bed: 'bed', bed_start: 'in bed from', bed_end: 'in bed until', category: 'category', name: 'name',
  time: 'time', end_time: 'end time', value: 'value', unit: 'unit of measure', ref_low: 'normal from', ref_high: 'normal to', flag: 'flag', code: 'code',
  vocabulary: 'code system', text: 'text (note, report)', author: 'author', service: 'service', domain: 'domain (OMOP)', file: 'file (path)', source: 'source',
};

async function load() {
  const [, m, f] = await Promise.all([loadSources(), api('mappings'), api('formats')]);
  imp.mappings = m.items;
  imp.formats = f.items;
  imp.categories = f.categories;
  imp.problems = f.problems;
  renderChoices();
  renderFormats();
  renderTables();
  await loadConnections();
}

// ------------------------------------------------------------ the kind of database
function showKind(kind) {
  imp.kind = kind;
  $('#db-import').classList.toggle('hidden', kind !== 'import');
  $('#db-connect').classList.toggle('hidden', kind === 'import');
  const server = kind === 'server';
  for (const id of ['cn-url-l', 'cn-pass-l', 'cn-schema-l', 'cn-vschema-l']) $(`#${id}`).classList.toggle('hidden', !server);
  $('#cn-path-l').classList.toggle('hidden', server);
  $('#cn-views').classList.toggle('hidden', $('#cn-kind').value !== 'sql');
  $('#cn-result').textContent = '';
}

// ------------------------------------------------------------ importing tables
/** The kinds of data and the formats for the chosen kind. */
function renderChoices() {
  const cat = $('#tb-cat').value || '';
  $('#tb-cat').innerHTML = `<option value="">Any kind (from the files)</option>${imp.categories.map((c) => `<option value="${esc(c.key)}"${c.key === cat ? ' selected' : ''}>${esc(c.label)}</option>`).join('')}`;
  const fmt = $('#tb-format').value || 'auto';
  const fit = imp.formats.filter((f) => !cat || !f.category || f.category === cat);
  if (fmt !== 'auto' && fmt !== 'none' && !fit.some((f) => f.key === fmt)) $('#tb-format').value = 'auto';
  $('#tb-format').innerHTML = `<option value="auto">Recognise it from its columns</option><option value="none">As the files are (no conversion)</option>
    ${fit.map((f) => `<option value="${esc(f.key)}"${f.key === fmt ? ' selected' : ''}>${esc(f.label)}${f.origin === 'built in' ? '' : ' (this site’s)'}</option>`).join('')}`;
  const chosen = imp.formats.find((f) => f.key === $('#tb-format').value);
  $('#tb-format-about').textContent = chosen ? chosen.about
    : $('#tb-format').value === 'none' ? 'Columns are read by their names, or as you map them below.'
      : 'A known export is converted (its label shows on the file); any other table is read by its column names.';
}

const together = () => imp.tables.length > 1 && document.querySelector('input[name="tb-mode"]:checked')?.value === 'one';

/** A table on the server, as the page holds it beside the browser's files: read where it is. */
const onServer = (path) => ({ name: path.split(/[\\/]/).pop(), path });

async function addFiles(files) {
  const format = $('#tb-format').value;
  for (const file of files) {
    try {
      const ins = file.path
        ? await api('tables/server/inspect', { method: 'POST', json: { path: file.path, format } })
        : await api(`tables/inspect?${qs({ name: file.name, format })}`, { method: 'POST', body: file });
      imp.tables.push({ file, ins, map: { ...ins.mapping }, category: $('#tb-cat').value || ins.category, name: file.name, source: '', report: '' });
    } catch (e) {
      imp.tables.push({ file, ins: null, map: {}, category: '', name: file.name, source: '', report: e.message });
    }
  }
  renderTables();
}

/** One table's card: its columns mapped, its category, name and source. */
function tableCard(t, i, title) {
  if (!t.ins) return `<div class="card tbl"><b>${esc(t.file.name)}</b><pre class="report">${esc(t.report)}</pre></div>`;
  const opts = (sel) => ['', ...t.ins.headers].map((h) => `<option${h === sel ? ' selected' : ''} value="${esc(h)}">${esc(h || '—')}</option>`).join('');
  const grid = t.ins.groups.map(([g, cols]) => `<h4>${esc(g)}</h4>${cols.map((c) => `<label><span>${esc(LABELS[c] || c)}</span><select data-tcol="${i}" data-col="${esc(c)}">${opts(t.map[c] || '')}</select></label>`).join('')}`).join('');
  const cats = `<option value="">from the column, or by name</option>${t.ins.categories.map(([k, l]) => `<option value="${esc(k)}"${k === t.category ? ' selected' : ''}>${esc(l)}</option>`).join('')}`;
  const saved = imp.mappings.filter((m) => m.kind === 'table').map((m) => `<option>${esc(m.name)}</option>`).join('');
  const mapped = Object.values(t.map).filter(Boolean).length;
  const sample = t.ins.sample ? `<details><summary>First rows</summary><div class="table-wrap short"><table class="grid"><tr>${t.ins.headers.map((h) => `<th>${esc(h)}</th>`).join('')}</tr>${t.ins.sample.map((r) => `<tr>${r.map((v) => `<td>${esc(v)}</td>`).join('')}</tr>`).join('')}</table></div></details>` : '';
  const named = together() ? '' : `<div class="ftools wrap"><input data-tname="${i}" value="${esc(t.name)}" placeholder="Name of this database" aria-label="Name of this database"><input data-tsource="${i}" value="${esc(t.source)}" placeholder="Source (default: the name; a source column wins)" aria-label="Source"></div>`;
  return `<div class="card tbl">
    <div class="ftools wrap"><b>${title}</b>${t.ins.format ? `<span class="pill" title="A known export: converted as it is imported (its columns below are those stored)">${esc(t.ins.format)}</span>` : ''}<span class="muted">${t.ins.rows} rows · ${mapped} of ${t.ins.headers.length} columns mapped</span>
      <label class="inline">Items are <select data-tcat="${i}" aria-label="Kind of items">${cats}</select></label>
      <select data-tsaved="${i}" aria-label="Saved mapping"><option value="">saved mapping…</option>${saved}</select>
      <button class="link" data-tsave="${i}">Save mapping as…</button>
      <span class="spacer"></span>${together() ? '' : `<button class="link danger" data-tdrop="${i}" title="Leave this file out" aria-label="Leave this file out">✕</button>`}</div>
    ${named}
    <details${t.report ? '' : ' open'}><summary>Columns</summary><div class="mapgrid">${grid}</div></details>
    ${sample}
    <pre class="report${t.report ? '' : ' hidden'}">${esc(t.report)}</pre>
  </div>`;
}

function renderTables() {
  $('#tb-together').classList.toggle('hidden', imp.tables.length < 2);
  $('#tb-dbname').classList.toggle('hidden', !together());
  if (together()) {
    // one database: the first file's columns stand for all (they must be the same)
    const names = imp.tables.map((t) => t.file.name);
    const title = `${imp.tables.length} files: ${esc(names.slice(0, 3).join(', '))}${names.length > 3 ? ` and ${names.length - 3} more` : ''}`;
    $('#tb-list').innerHTML = tableCard(imp.tables[0], 0, title) + (imp.joined ? `<pre class="report">${esc(imp.joined)}</pre>` : '');
  } else {
    $('#tb-list').innerHTML = imp.tables.map((t, i) => tableCard(t, i, esc(t.file.name))).join('');
  }
  renderRunning();
}

function tableReport(r) {
  const head = r.dry_run ? 'Preview: ' : `Import ${r.import_id}: `;
  const parts = [
    [r.patients_added, 'new patient'], [r.encounters_added, 'new encounter'], [r.bed_stays_added, 'bed stay'], [r.items_added, 'item'],
    [r.recordings_added, 'recording'], [r.attachments, 'attached file'],
  ].filter(([n]) => n).map(([n, w]) => `${n} ${w}${n === 1 ? '' : 's'}`);
  const lines = [`${head}${r.rows} rows: ${parts.join(', ') || 'nothing new'}.`];
  if (r.duplicates) lines.push(`${r.duplicates} item(s) already there (now also held by this source).`);
  if (r.conflicts) lines.push(`${r.conflicts} row(s) held back${r.dry_run ? '' : ': decide in Review'}.`);
  if (r.ignored_columns.length) lines.push(`The database's own columns (shown in Databases, searchable in Cohorts): ${r.ignored_columns.join(', ')}`);
  return [...lines, ...r.warnings].join('\n');
}

const mappingOf = (t) => JSON.stringify(Object.fromEntries(Object.entries(t.map).filter(([, h]) => h)));

async function importTables(dryRun) {
  const format = $('#tb-format').value;
  // what to import: one database of all the files, or each file its own (tables describing
  // patients and encounters first, so that items find them at once)
  let runs;
  if (together()) {
    const name = $('#tb-dbname').value.trim();
    if (!name) { $('#tb-dbname').focus(); throw new Error('Give the database a name'); }
    runs = [{ name, files: imp.tables, t: imp.tables[0] }];
  } else {
    const describes = (t) => ['last_name', 'birth_date', 'admit_time', 'bed'].some((c) => t.map[c]);
    runs = imp.tables.filter((t) => t.ins).sort((a, b) => describes(b) - describes(a)).map((t) => ({ name: t.name.trim() || t.file.name, files: [t], t }));
  }
  imp.busy = true;
  imp.joined = '';
  renderTables();
  try {
    for (const run of runs) {
      const { t } = run;
      t.report = '';
      try {
        const fields = { batch: run.name, source: t.source, category: t.category, mapping: mappingOf(t), format, dry_run: dryRun, background: true };
        const served = run.files.filter((x) => x.file.path).length;
        let task;
        if (served && served < run.files.length) throw new Error('Tables on the server and tables from this computer cannot make one database: import them apart');
        if (served) {
          // read on the server where they are: nothing to upload
          task = await api('tables/server', { method: 'POST', json: { ...fields, paths: run.files.map((x) => x.file.path) } });
        } else {
          const form = new FormData();
          for (const x of run.files) form.append('file', x.file, x.file.name);
          imp.uploading = { id: 'upload', label: run.name, state: 'running', fraction: 0, loaded: 0, total: run.files.reduce((n, x) => n + x.file.size, 0) };
          renderRunning();
          task = await upload(`tables/files?${qs(fields)}`, form, (fraction, loaded, total) => {
            Object.assign(imp.uploading, { fraction, loaded, total });
            renderRunning();
          });
        }
        imp.uploading = null;
        imp.ours.add(task.id);
        watch(task);
        const end = await whenDone(task.id);
        dismiss(task.id);
        const text = end.state === 'done' ? tableReport(end.report) : end.error;
        if (together()) imp.joined = text; else t.report = text;
        t.done = end.state === 'done' && !dryRun;
      } catch (e) {
        imp.uploading = null;
        if (together()) imp.joined = e.message; else t.report = e.message;
      }
      renderTables();
    }
  } finally {
    imp.busy = false;
    imp.uploading = null;
    if (!dryRun) {
      imp.tables = imp.tables.filter((t) => !t.done);
      loadSources();
      checkData();
    }
    renderTables();
  }
}

/** The imports running (this page's, the CLIF folder's, one still running after a reload):
 * their progress, Cancel; those ended that no file's card reports, until closed. */
function renderRunning() {
  const box = $('#tb-running');
  if (!box) return;
  const items = watched().filter((t) => t.state === 'running' || !imp.ours.has(t.id));
  let h = imp.uploading ? progressHtml(imp.uploading, { upload: imp.uploading }) : '';
  for (const t of items) {
    if (t.state === 'running') h += progressHtml(t);
    else {
      const text = t.state === 'failed' ? t.error : t.report?.tables ? clifText(t.report) : tableReport(t.report);
      h += `<div class="progress ended"><div class="ftools"><b>${esc(t.label)}</b><span class="muted">${{ failed: 'failed', cancelled: 'stopped' }[t.state] || 'done'}</span><span class="spacer"></span>
        <button class="link" data-task-close="${esc(t.id)}" aria-label="Close">✕</button></div><pre class="report">${esc(text)}</pre></div>`;
    }
  }
  box.innerHTML = h ? `<h4>Imports in progress</h4>${h}` : '';
  sizeBars(box);
  box.classList.toggle('hidden', !h);
  const busy = imp.busy || watched().some((t) => t.state === 'running');
  $('#tb-preview').disabled = $('#tb-import').disabled = busy || !imp.tables.length;
  $('#clif-preview').disabled = $('#clif-import').disabled = busy;
  $('#tb-files').disabled = $('#tb-server').disabled = imp.busy;
  $('#tb-import').textContent = imp.busy ? 'Importing…' : 'Import all';
}

// ------------------------------------------------------------ a CLIF folder on the server
const clifText = (r) => [
  ...r.tables.map((t) => `${t.name} (${t.files} file${t.files === 1 ? '' : 's'}): ${tableReport(t.report).split('\n')[0]}`),
  ...(r.skipped.length ? [`Not CLIF tables (left out): ${r.skipped.join(', ')}`] : []),
].join('\n');

async function importClif(dryRun) {
  const dir = $('#clif-dir').value.trim();
  if (!dir) { $('#clif-dir').focus(); throw new Error('The folder, on the server'); }
  $('#clif-report').classList.add('hidden');
  try {
    const task = await api('tables/clif', { method: 'POST', json: { dir, dry_run: dryRun, background: true } });
    imp.ours.add(task.id);
    watch(task);
    const end = await whenDone(task.id);
    dismiss(task.id);
    $('#clif-report').textContent = end.state === 'done' ? clifText(end.report) : end.error;
    $('#clif-report').classList.remove('hidden');
    if (!dryRun && end.state === 'done') loadSources();
  } catch (e) {
    $('#clif-report').textContent = e.message;
    $('#clif-report').classList.remove('hidden');
  }
}

// ------------------------------------------------------------ formats
function renderFormats() {
  let h = '<tr><th>Format</th><th>Kind of data</th><th>What it is</th><th>From</th></tr>';
  const label = Object.fromEntries(imp.categories.map((c) => [c.key, c.label]));
  for (const f of imp.formats) {
    h += `<tr><td><b>${esc(f.label)}</b><div class="muted mono">${esc(f.key)}</div></td><td>${esc(label[f.category] || f.category || 'any')}</td>
      <td class="muted">${esc(f.about)}</td><td class="path">${esc(f.origin)}</td></tr>`;
  }
  $('#fmt-table').innerHTML = h;
  $('#fmt-problems').textContent = imp.problems.join('\n');
  $('#fmt-problems').classList.toggle('hidden', !imp.problems.length);
}

// ------------------------------------------------------------ connected databases
async function loadConnections() {
  const r = await api('connections');
  let h = '<tr><th>Name</th><th>Layout</th><th>Where</th><th class="num">Persons linked</th><th>Connected</th><th></th></tr>';
  if (!r.items.length) h += '<tr><td colspan="6" class="empty">No database connected.</td></tr>';
  for (const x of r.items) {
    h += `<tr><td><b>${esc(x.name)}</b></td><td>${esc(x.kind === 'omop' ? 'OMOP' : 'SQL views')}</td><td class="path">${esc(x.url)} <span class="muted">(${esc(x.database)})</span></td>
      <td class="num">${x.persons}</td><td class="nowrap">${fmtTime(x.created_at)}</td>
      <td class="nowrap"><button class="link" data-cn-check="${x.id}">Check</button> <button class="link" data-cn-sync="${x.id}">Link all</button>
        <button class="link danger" data-cn-rm="${x.id}" data-name="${esc(x.name)}">Disconnect…</button></td></tr>`;
  }
  $('#cn-table').innerHTML = h;
}

async function connect() {
  const v = (id) => $(`#${id}`).value.trim();
  const server = imp.kind === 'server';
  const url = server ? v('cn-url') : `sqlite:${v('cn-path')}`;
  const settings = {
    schema: server ? v('cn-schema') : '', vocabulary_schema: server ? v('cn-vschema') : '', identifier: v('cn-ident'),
    mrn_authority: v('cn-auth'), timezone: v('cn-tz'),
  };
  if (v('cn-kind') === 'sql') Object.assign(settings, { persons: v('cn-v-persons'), visits: v('cn-v-visits'), entries: v('cn-v-entries'), version: v('cn-v-version') });
  $('#cn-result').textContent = 'Connecting and checking…';
  try {
    const r = await api('connections', { method: 'POST', json: { name: v('cn-name'), kind: v('cn-kind'), url, password: server ? $('#cn-pass').value : '', settings } });
    $('#cn-pass').value = '';
    $('#cn-result').textContent = `Connected: views ${r.views.join(', ')}${r.version ? `; version ${r.version}` : ''}.`;
    toast(`${r.name} connected`);
    await loadConnections();
    checkData();
  } catch (e) {
    $('#cn-result').textContent = '';
    throw e;
  }
}

// ------------------------------------------------------------ sources and imports
async function loadSources() {
  const [src, batches] = await Promise.all([api('sources'), api('imports')]);
  reviewBadge().catch(() => {});
  let html = '<tr><th>Source</th><th class="num">Recordings</th><th class="num">Patients</th><th class="num">Encounters</th><th class="num">Items</th><th class="num">Imports</th><th></th></tr>';
  if (!src.items.length) html += '<tr><td colspan="7" class="empty">Nothing imported yet.</td></tr>';
  for (const x of src.items) {
    html += `<tr><td>${esc(x.source || '(none)')}</td><td class="num">${x.recordings}</td><td class="num">${x.patients}</td><td class="num">${x.encounters}</td>
      <td class="num">${x.ehr_items}</td><td class="num">${x.imports}</td><td><button class="link danger" data-rm-source="${esc(x.source)}">Remove…</button></td></tr>`;
  }
  $('#src-table').innerHTML = html;
  $('#src-waiting').textContent = src.waiting ? `${src.waiting} item(s) wait for a database that names their patient or encounter.` : '';
  html = '<tr><th>Import</th><th>Time</th><th>Kind</th><th>Name</th><th>Source</th><th>By</th><th>Still contributes</th><th></th></tr>';
  for (const t of watched().filter((x) => x.state === 'running')) {
    html += `<tr class="running"><td class="num">…</td><td class="nowrap">${t.started ? fmtTime(t.started) : ''}</td><td>database</td><td>${esc(t.label)}</td><td></td><td></td>
      <td class="muted">importing: ${Math.floor((t.progress?.done || 0) * 100)} %</td><td></td></tr>`;
  }
  for (const b of batches.items) {
    const left = [[b.recordings, 'recording'], [b.encounters, 'encounter'], [b.ehr_items, 'item']].filter(([n]) => n).map(([n, w]) => `${n} ${w}${n === 1 ? '' : 's'}`).join(', ');
    html += `<tr><td class="num">${b.id}</td><td class="nowrap">${fmtTime(b.time)}</td><td>${esc(b.kind === 'table' ? 'database' : b.kind)}</td><td>${esc(b.name)}</td><td>${esc(b.source)}</td><td>${esc(b.username || '(command line)')}</td>
      <td class="muted">${b.state === 'removed' ? 'removed' : esc(left || 'nothing')}</td>
      <td>${b.state === 'active' ? `<button class="link danger" data-rm-import="${b.id}">Remove…</button>` : ''}</td></tr>`;
  }
  $('#imports-table').innerHTML = html;
}

async function removeImport(kind, id) {
  const url = kind === 'source' ? 'sources/remove' : `imports/${id}/remove`;
  const base = kind === 'source' ? { source: id } : {};
  const p = await api(url, { method: 'POST', json: { ...base, dry_run: true } });
  const what = kind === 'source' ? `the source "${id}"` : `import ${id}`;
  if (!confirm(`Remove ${what}?\n\nThis removes: ${removalText(p)}\n\nFiles are never touched.`)) return;
  const purge = p.recordings ? confirm("Also delete the catalog's indexes and results of the removed recordings (in the results folder only)?") : false;
  const r = await api(url, { method: 'POST', json: { ...base, purge } });
  toast(`Removed: ${removalText(r).split('\n')[0]}`);
  await loadSources();
  checkData();
}

export default {
  section: 'sources',
  nav: 'import',
  show: () => load(),
  wire() {
    $('#tb-files').addEventListener('change', (e) => { addFiles([...e.target.files]).catch(fail); e.target.value = ''; });
    $('#tb-server').addEventListener('click', async () => {
      const paths = await chooseOnServer({ kind: 'tables' });
      if (paths?.length) addFiles(paths.map(onServer)).catch(fail);
    });
    // a folder or a database file on the server, chosen rather than typed
    for (const [button, input, kind] of [['#clif-pick', '#clif-dir', 'folders'], ['#cn-path-pick', '#cn-path', 'databases']]) {
      $(button).addEventListener('click', async () => {
        const [path] = (await chooseOnServer({ kind, multiple: false })) || [];
        if (path) $(input).value = path;
      });
    }
    // files dropped on the zone
    const drop = $('#tb-drop');
    for (const ev of ['dragenter', 'dragover']) drop.addEventListener(ev, (e) => { e.preventDefault(); if (!imp.busy) drop.classList.add('over'); });
    for (const ev of ['dragleave', 'drop']) drop.addEventListener(ev, () => drop.classList.remove('over'));
    drop.addEventListener('drop', (e) => {
      e.preventDefault();
      if (imp.busy) return;
      const files = [...(e.dataTransfer?.files || [])];
      if (files.length) addFiles(files).catch(fail);
    });
    for (const r of document.querySelectorAll('input[name="db-kind"]')) r.addEventListener('change', () => showKind(r.value));
    $('#cn-kind').addEventListener('change', () => showKind(imp.kind));
    $('#tb-cat').addEventListener('change', renderChoices);
    $('#tb-format').addEventListener('change', () => {
      renderChoices();
      // the files read again as the chosen format reads them
      const files = imp.tables.map((t) => t.file);
      imp.tables = [];
      if (files.length) addFiles(files).catch(fail);
    });
    for (const r of document.querySelectorAll('input[name="tb-mode"]')) r.addEventListener('change', () => { imp.joined = ''; renderTables(); });
    $('#db-connect').addEventListener('submit', (e) => { e.preventDefault(); connect().catch(fail); });
    $('#clif-preview').addEventListener('click', () => importClif(true).catch(fail));
    $('#clif-import').addEventListener('click', () => importClif(false).catch(fail));
    $('#cn-table').addEventListener('click', async (e) => {
      const b = e.target.closest('[data-cn-check],[data-cn-sync],[data-cn-rm]');
      if (!b) return;
      const d = b.dataset;
      try {
        if (d.cnCheck) {
          const r = await api(`connections/${d.cnCheck}/check`, { method: 'POST' });
          toast(r.ok ? `Answers in ${r.ms} ms: views ${r.views.join(', ')}` : `Does not answer: ${r.error}`, !r.ok);
        } else if (d.cnSync) {
          toast('Linking its persons…');
          const r = await api(`connections/${d.cnSync}/sync`, { method: 'POST' });
          toast(`${r.persons} person(s); ${r.linked} linked to the catalog's patients`);
          await loadConnections();
          checkData();
        } else if (d.cnRm && confirm(`Disconnect ${d.name}?\n\nIts links and what it added to the catalog go; the database itself is not touched.`)) {
          await api(`connections/${d.cnRm}`, { method: 'DELETE' });
          toast(`${d.name} disconnected`);
          await loadConnections();
          checkData();
        }
      } catch (err) { fail(err); }
    });
    $('#tb-preview').addEventListener('click', () => importTables(true).catch(fail));
    $('#tb-import').addEventListener('click', () => importTables(false).catch(fail));
    const view = $('#view-sources');
    view.addEventListener('change', (e) => {
      const t = e.target;
      const d = t.dataset;
      if (d.tcol !== undefined) {
        imp.tables[+d.tcol].map[d.col] = t.value;
      } else if (d.tcat !== undefined) {
        imp.tables[+d.tcat].category = t.value;
      } else if (d.tsaved !== undefined && t.value) {
        const m = imp.mappings.find((x) => x.name === t.value);
        if (m) Object.assign(imp.tables[+d.tsaved].map, m.columns);
        renderTables();
      }
    });
    view.addEventListener('input', (e) => {
      const d = e.target.dataset;
      if (d.tname !== undefined) imp.tables[+d.tname].name = e.target.value;
      if (d.tsource !== undefined) imp.tables[+d.tsource].source = e.target.value;
    });
    onImports(() => { if (!$('#view-sources').classList.contains('hidden')) renderRunning(); });
    view.addEventListener('click', (e) => {
      const c = e.target.closest('[data-task-cancel],[data-task-close]');
      if (c) {
        if (c.dataset.taskClose) dismiss(c.dataset.taskClose);
        else if (confirm('Stop this import?\n\nNothing of it is kept: the catalog stays as it was before it.')) cancel(c.dataset.taskCancel).catch(fail);
        return;
      }
      const b = e.target.closest('[data-rm-source],[data-rm-import],[data-tsave],[data-tdrop]');
      if (!b) return;
      const d = b.dataset;
      if (d.rmSource !== undefined) removeImport('source', d.rmSource).catch(fail);
      else if (d.rmImport !== undefined) removeImport('import', +d.rmImport).catch(fail);
      else if (d.tdrop !== undefined) { imp.tables.splice(+d.tdrop, 1); renderTables(); }
      else if (d.tsave !== undefined) {
        const t = imp.tables[+d.tsave];
        const name = prompt('Save this column mapping as (e.g. "Epic lab export"):');
        if (!name) return;
        const columns = Object.fromEntries(Object.entries(t.map).filter(([, h]) => h));
        api('mappings', { method: 'POST', json: { name, kind: 'table', columns } })
          .then(() => api('mappings')).then((m) => { imp.mappings = m.items; renderTables(); toast('Mapping saved'); }).catch(fail);
      }
    });
  },
};
