// db-builder's page: the databases of the server's root folder, and for the one open its
// tables, imports, recordings, rules, check, sources and info.  The server decides what may be
// read and written (src/serve/): every path is relative to its root, and refused outside it.

const $ = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => Array.from(root.querySelectorAll(s));
const PAGE = 100;

const esc = (t) => String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const qs = (o) => new URLSearchParams(Object.entries(o).filter(([, v]) => v !== undefined && v !== null && v !== '')).toString();
const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;

const store = {
  get(k, d) { try { const v = localStorage.getItem(`db-builder.${k}`); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(`db-builder.${k}`, JSON.stringify(v)); } catch { /* private mode */ } },
};

/** The page's state: who is signed in, the database open, the tab shown, a table's page. */
const st = { me: null, db: null, tab: 'tables', table: 'persons', offset: 0, total: 0, inputs: [], sources: [] };

async function api(path, { method = 'GET', json, background = false } = {}) {
  const init = { method, headers: {}, credentials: 'same-origin' };
  // every request that changes something says so (the server refuses it otherwise: CSRF)
  if (method !== 'GET') init.headers['X-DB-Builder'] = '1';
  if (background) init.headers['X-Background'] = '1';
  if (json !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(json);
  }
  const r = await fetch(`api/${path}`, init);
  let data = null;
  try { data = await r.json(); } catch { /* empty */ }
  if (r.status === 401 && path !== 'login') {
    showLogin();
    throw new Error('Please sign in.');
  }
  if (!r.ok) throw new Error((data && data.detail) || `${r.status} ${r.statusText}`);
  return data;
}

function toast(msg, isErr = false) {
  const t = $('#toast');
  t.textContent = msg;
  t.classList.toggle('err', isErr);
  t.classList.remove('hidden');
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add('hidden'), isErr ? 7000 : 3500);
}
const fail = (e) => toast(e.message || String(e), true);

function size(n) {
  if (n < 1e3) return `${n} B`;
  if (n < 1e6) return `${Math.round(n / 1e3)} kB`;
  if (n < 1e9) return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB`;
  return `${(n / 1e9).toFixed(1)} GB`;
}

// ------------------------------------------------------------ theme and sign-in

function applyTheme(t) {
  const sys = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  document.documentElement.dataset.theme = t || sys;
}

function showLogin() {
  $('#app').classList.add('hidden');
  $('#login').classList.remove('hidden');
  $('#login-pass').value = '';
  $('#login-pass').focus();
}

async function signIn(e) {
  e.preventDefault();
  $('#login-msg').textContent = '';
  try {
    await api('login', { method: 'POST', json: { username: $('#login-user').value.trim(), password: $('#login-pass').value } });
    $('#login-pass').value = '';
    await start();
  } catch (err) {
    $('#login-msg').textContent = err.message;
  }
}

async function start() {
  st.me = await api('me');
  $('#login').classList.add('hidden');
  $('#app').classList.remove('hidden');
  $('#root-path').textContent = st.me.root;
  $('#enc').classList.toggle('hidden', !st.me.encrypted);
  const f = await api('formats');
  $('#im-format').innerHTML = '<option value="auto">Recognised by its columns</option><option value="none">None: a plain table</option>'
    + f.items.map((x) => `<option value="${esc(x.key)}">${esc(x.label)} (${esc(x.key)})</option>`).join('');
  for (const sel of $$('select.categories')) {
    const first = sel.options[0].outerHTML;
    sel.innerHTML = first + f.categories.map((c) => `<option value="${esc(c.key)}">${esc(c.label)}</option>`).join('');
  }
  await listDatabases();
  const last = store.get('db', null);
  if (last && !st.db) openDb(last).catch(() => store.set('db', null));
}

// ------------------------------------------------------------ the databases

async function listDatabases() {
  const r = await api('databases');
  const ul = $('#db-list');
  ul.innerHTML = r.items.length
    ? r.items.map((d) => `<li role="option" tabindex="0" data-path="${esc(d.path)}" aria-selected="${d.path === st.db}">${esc(d.path)}<span class="size">${size(d.size)}</span></li>`).join('')
    : '<li class="muted" aria-disabled="true">No .sqlite file in the root yet: make one with New…</li>';
}

async function openDb(path) {
  const info = await api(`info?${qs({ db: path })}`);
  st.db = info.path;
  store.set('db', st.db);
  $('#none').classList.add('hidden');
  $('#open').classList.remove('hidden');
  $('#db-title').textContent = info.path;
  $('#db-sub').textContent = `schema ${info.meta.schema} · local times in ${info.meta.timezone}${info.encrypted ? ' · encrypted' : ''}`;
  for (const li of $$('#db-list li')) li.setAttribute('aria-selected', String(li.dataset.path === st.db));
  st.offset = 0;
  await showTab(st.tab);
}

/** A file name in the root, asked in a dialog (a new database, a copy). */
function askName({ title, hint, ok, timezone }) {
  const d = $('#nd');
  $('#nd-title').textContent = title;
  $('#nd-hint').textContent = hint;
  $('#nd-ok').textContent = ok;
  $('#nd-tz-l').classList.toggle('hidden', !timezone);
  $('#nd-msg').textContent = '';
  $('#nd-path').value = '';
  return new Promise((resolve) => {
    d.addEventListener('close', () => resolve(d.returnValue === 'ok' ? { path: $('#nd-path').value.trim(), timezone: $('#nd-tz').value.trim() } : null), { once: true });
    d.returnValue = '';
    d.showModal();
    $('#nd-path').focus();
  });
}

async function newDb() {
  const v = await askName({ title: 'New database', hint: 'An empty database, in the root (a folder of it: data/study.sqlite). Its local times are read in one time zone, chosen now.', ok: 'Make it', timezone: true });
  if (!v || !v.path) return;
  const r = await api('databases', { method: 'POST', json: v });
  toast(`${r.path} made`);
  await listDatabases();
  st.tab = 'import';
  await openDb(r.path);
}

async function openOther() {
  const [p] = (await choose({ title: 'Open a database', mode: 'files', multiple: false, filter: /\.(sqlite3?|db)$/i })) || [];
  if (p) await openDb(p);
}

async function saveCopy() {
  const v = await askName({ title: 'Save a copy', hint: `A copy of ${st.db}, in a new file of the root${st.me.encrypted ? ', encrypted with the same key' : ''}.`, ok: 'Save the copy' });
  if (!v || !v.path) return;
  const r = await api('copy', { method: 'POST', json: { db: st.db, to: v.path } });
  toast(`Saved as ${r.path}`);
  await listDatabases();
}

// ------------------------------------------------------------ the file chooser

/**
 * Paths chosen in the root (relative to it), or null if cancelled.  mode: `files` (files, and
 * folders too with `folders`), or `folder` (the folder shown).
 */
function choose({ title, mode = 'files', multiple = true, folders = false, filter = null }) {
  const d = $('#fc');
  const sel = new Set();
  let here = { dir: '', parent: null };
  $('#fc-title').textContent = title;
  $('#fc-here').classList.toggle('hidden', mode !== 'folder');
  $('#fc-ok').classList.toggle('hidden', mode === 'folder');
  const update = () => {
    $('#fc-ok').disabled = sel.size === 0;
    $('#fc-count').textContent = multiple && sel.size ? `${sel.size} chosen` : '';
    $('#fc-up').disabled = here.parent === null;
  };
  const show = async (dir) => {
    $('#fc-msg').textContent = 'Reading…';
    try {
      const v = await api(`files?${qs({ dir })}`);
      here = v;
      store.set('dir', v.dir);
      $('#fc-dir').textContent = `/${v.dir}`;
      const items = v.items.filter((it) => it.dir || mode === 'files');
      const pick = (it) => `<label><input type="${multiple ? 'checkbox' : 'radio'}" name="fc-pick" data-path="${esc(it.path)}"${sel.has(it.path) ? ' checked' : ''}><span>${esc(it.name)}${it.dir ? '/' : ''}</span>${it.dir ? '' : `<span class="size">${size(it.size)}</span>`}</label>`;
      $('#fc-list').innerHTML = items.map((it) => `<div role="listitem" class="fc-item">${it.dir
        ? `${folders && mode === 'files' ? `<input type="${multiple ? 'checkbox' : 'radio'}" name="fc-pick" aria-label="Choose ${esc(it.name)}" data-path="${esc(it.path)}"${sel.has(it.path) ? ' checked' : ''}>` : ''}<button type="button" class="dir" data-dir="${esc(it.path)}">${esc(it.name)}/</button>`
        : (filter && !filter.test(it.name) ? `<span class="faint">${esc(it.name)}</span>` : pick(it))}</div>`).join('');
      $('#fc-msg').textContent = items.length ? (v.truncated ? `Only the first ${v.items.length} are shown.` : '') : 'Nothing here.';
    } catch (e) {
      if (dir) { show(''); return; }
      $('#fc-msg').textContent = e.message;
    }
    update();
  };
  return new Promise((resolve) => {
    const onClick = (e) => {
      const b = e.target.closest('[data-dir]');
      if (b) { show(b.dataset.dir); return; }
      if (e.target.closest('#fc-up') && here.parent !== null) show(here.parent);
      if (e.target.closest('#fc-here')) { sel.clear(); sel.add(here.dir); d.close('ok'); }
    };
    const onChange = (e) => {
      const p = e.target.dataset?.path;
      if (p === undefined) return;
      if (!multiple) sel.clear();
      if (e.target.checked) sel.add(p); else sel.delete(p);
      update();
    };
    d.addEventListener('click', onClick);
    d.addEventListener('change', onChange);
    d.addEventListener('close', () => {
      d.removeEventListener('click', onClick);
      d.removeEventListener('change', onChange);
      resolve(d.returnValue === 'ok' ? [...sel] : null);
    }, { once: true });
    d.returnValue = '';
    d.showModal();
    show(store.get('dir', ''));
  });
}

// ------------------------------------------------------------ tabs

async function showTab(tab) {
  st.tab = tab;
  for (const b of $$('.tabs [role=tab]')) {
    const on = b.dataset.tab === tab;
    b.setAttribute('aria-selected', String(on));
    b.tabIndex = on ? 0 : -1;
  }
  for (const p of $$('.pane')) p.classList.toggle('hidden', p.id !== `pane-${tab}`);
  if (!st.db) return;
  // a task of this tab still running: followed again
  if (tasks[tab]) follow(tab).catch(fail);
  if (tab === 'tables') await drawTable();
  else if (tab === 'rules') await drawRules();
  else if (tab === 'sources') await drawSources();
  else if (tab === 'info') await drawInfo();
}

function tabKeys(e) {
  const tabs = $$('.tabs [role=tab]');
  const i = tabs.indexOf(e.target);
  if (i < 0) return;
  let j = null;
  if (e.key === 'ArrowRight') j = (i + 1) % tabs.length;
  else if (e.key === 'ArrowLeft') j = (i - 1 + tabs.length) % tabs.length;
  else if (e.key === 'Home') j = 0;
  else if (e.key === 'End') j = tabs.length - 1;
  if (j === null) return;
  e.preventDefault();
  tabs[j].focus();
  showTab(tabs[j].dataset.tab).catch(fail);
}

// ------------------------------------------------------------ Tables

function cell(v) {
  if (v === null) return '<td class="null">NULL</td>';
  if (typeof v === 'number') return `<td class="num">${esc(v)}</td>`;
  return `<td title="${esc(v)}">${esc(v)}</td>`;
}

async function drawTable() {
  const r = await api(`rows?${qs({ db: st.db, table: st.table, offset: st.offset, limit: PAGE, q: $('#t-filter').value.trim() })}`);
  st.total = r.total;
  $('#t-grid').innerHTML = `<thead><tr>${r.columns.map((c) => `<th scope="col">${esc(c)}</th>`).join('')}</tr></thead><tbody>${r.rows.length
    ? r.rows.map((row) => `<tr>${row.map(cell).join('')}</tr>`).join('')
    : `<tr><td colspan="${r.columns.length}" class="muted">No rows.</td></tr>`}</tbody>`;
  const to = Math.min(st.offset + r.rows.length, r.total);
  $('#t-count').textContent = r.total ? `rows ${st.offset + 1}–${to} of ${r.total}` : '';
  $('#t-prev').disabled = st.offset === 0;
  $('#t-next').disabled = to >= r.total;
}

// ------------------------------------------------------------ Import and Recordings: tasks

function options(prefix) {
  const v = (id) => ($(`#${prefix}-${id}`)?.value || '').trim();
  const o = { issuer: v('issuer'), mrn_authority: v('auth'), category: v('category'), day_first: $(`#${prefix}-dayfirst`).checked };
  if (prefix === 'im') {
    o.format = v('format');
    o.mapping = v('mapping');
    o.formats = v('formats').split(',').map((s) => s.trim()).filter(Boolean);
  }
  return o;
}

function drawInputs() {
  $('#im-inputs').innerHTML = st.inputs.length
    ? st.inputs.map((p, i) => `<li><code>${esc(p)}</code><button class="link" data-drop="${i}" aria-label="Leave ${esc(p)} out">Remove</button></li>`).join('')
    : '<li class="muted">None chosen yet.</li>';
}

function columnText(c) {
  if (c.how === 'field') return `<span class="how-field">${esc(c.field)}</span>`;
  if (c.how === 'measure') return `<span class="how-measure">a measurement: ${esc(c.field)}${c.unit ? ` (${esc(c.unit)})` : ''}</span>`;
  return '<span class="how-extra">extras (kept as it came)</span>';
}

function report(r, dry) {
  let status;
  if (r.already) status = `already added (source ${r.already}): nothing ${dry ? 'would be' : 'was'} done`;
  else if (dry) status = r.replaced.length ? 'changed since it was added: would replace its earlier version' : 'would be added';
  else status = `added as source ${r.source}${r.replaced.length ? ` (replacing source ${r.replaced.join(', ')})` : ''}`;
  let h = `<div class="report"><h3>${esc(r.file)}</h3><p class="muted">${esc(r.kind)}${r.format ? ` · ${esc(r.format)}` : ''} · ${esc(status)}</p>`;
  if (!r.already) {
    const n = [['rows', r.rows], ['persons added', r.persons_added], ['visits added', r.visits_added], ['entries', r.entries], ['recordings', r.recordings], ['left out', r.left_out]];
    h += `<div class="counts">${n.map(([k, x]) => `<span><b>${x}</b> ${k}</span>`).join('')}</div>`;
  }
  if (r.warnings.length) h += `<ul class="warnings">${r.warnings.map((w) => `<li>${esc(w)}</li>`).join('')}</ul>`;
  if (r.columns.length) {
    const sample = r.sample || [];
    h += `<details${dry ? ' open' : ''}><summary>How its ${plural(r.columns.length, 'column')} are read${sample.length ? ', and its first rows' : ''}</summary><div class="table-wrap"><table class="grid">`;
    h += `<thead><tr><th scope="col">Column</th><th scope="col">Read as</th>${sample.map((_, i) => `<th scope="col">Row ${i + 1}</th>`).join('')}</tr></thead><tbody>`;
    h += r.columns.map((c, i) => `<tr><td>${esc(c.header)}</td><td>${columnText(c)}</td>${sample.map((row) => cell(row[i] ?? '')).join('')}</tr>`).join('');
    h += '</tbody></table></div></details>';
  }
  return `${h}</div>`;
}

/** The tasks started from a tab (`import`, `recordings`): { task, out, dry, db, polling }. */
const tasks = {};

/** Start a task (`import` or `recordings`) and show its progress, then its reports. */
async function runTask(endpoint, body, out, dry) {
  if (tasks[endpoint]) return;
  const el = $(out);
  const buttons = $$(`#pane-${endpoint} .ftools button`);
  buttons.forEach((b) => { b.disabled = true; });
  el.innerHTML = `<div class="progress"><span class="dot" aria-hidden="true"></span><span>Starting…</span></div>`;
  try {
    const { task } = await api(endpoint, { method: 'POST', json: { ...body, db: st.db, dry_run: dry } });
    tasks[endpoint] = { task, out, dry, db: st.db, polling: false };
  } catch (e) {
    el.innerHTML = `<p class="err">${esc(e.message)}</p>`;
    buttons.forEach((b) => { b.disabled = false; });
    return;
  }
  await follow(endpoint);
}

/**
 * Follow a task while its tab is shown: its progress, then its reports.  Leaving the tab (or
 * opening another database) stops asking; the task goes on on the server, and coming back to
 * the tab follows it again (showTab).
 */
async function follow(endpoint) {
  const job = tasks[endpoint];
  if (!job || job.polling) return;
  job.polling = true;
  const { task, dry } = job;
  const el = $(job.out);
  const buttons = $$(`#pane-${endpoint} .ftools button`);
  buttons.forEach((b) => { b.disabled = true; });
  let ended = false;
  try {
    for (;;) {
      if (st.tab !== endpoint || st.db !== job.db) {
        el.innerHTML = `<div class="progress"><span class="dot" aria-hidden="true"></span><span>Still running on the server: its report shows here when you come back to this tab${st.db !== job.db ? ` with ${esc(job.db)} open` : ''}.</span></div>`;
        return;
      }
      const t = await api(`tasks/${task}`, { background: true });
      const secs = `${t.elapsed_s.toFixed(t.elapsed_s < 10 ? 1 : 0)} s`;
      if (t.state === 'waiting' || t.state === 'running') {
        const what = t.state === 'waiting' ? 'Waiting for another task to end' : `${dry ? 'Reading (a preview: nothing is written)' : 'Adding'}${t.file ? `: ${t.file}` : ''}`;
        el.innerHTML = `<div class="progress"><span class="dot" aria-hidden="true"></span><span>${esc(what)} · ${plural(t.rows, 'row')} read · ${secs}</span></div>`;
        await new Promise((res) => { setTimeout(res, 700); });
        continue;
      }
      if (t.state === 'failed') {
        ended = true;
        el.innerHTML = `<div class="progress failed"><span class="dot" aria-hidden="true"></span><span class="err">${dry ? 'The preview' : 'Nothing was added'}: ${esc(t.error)}</span></div>`;
        return;
      }
      ended = true;
      const head = dry ? `Preview, in ${secs}: nothing was written.` : `Done in ${secs}.`;
      el.innerHTML = `<div class="progress done"><span class="dot" aria-hidden="true"></span><span>${esc(head)}</span></div>${t.reports.map((r) => report(r, dry)).join('')}`;
      if (!dry) {
        toast('Added');
        listDatabases().catch(() => {});
      }
      return;
    }
  } catch (e) {
    ended = true;
    el.innerHTML = `<p class="err">${esc(e.message)}</p>`;
  } finally {
    job.polling = false;
    if (ended) delete tasks[endpoint];
    // the tab's buttons wait while its task runs (on the server, followed or not)
    buttons.forEach((b) => { b.disabled = !ended; });
  }
}

function importBody() {
  if (!st.inputs.length) throw new Error('Choose the tables, exports or CLIF folders to add first.');
  return { inputs: st.inputs, options: options('im') };
}

function recordingsBody() {
  const v = (id) => $(`#re-${id}`).value.trim();
  if (!v('folder')) throw new Error('Choose the folder of recordings first.');
  return { folder: v('folder'), patients: v('patients'), pattern: v('pattern'), options: options('re') };
}

// ------------------------------------------------------------ Rules

async function drawRules() {
  const r = await api(`rules?${qs({ db: st.db })}`);
  $('#ru-file').textContent = r.file;
  $('#ru-text').value = r.text;
  $('#ru-msg').textContent = r.exists ? '' : 'No rules yet: rows are written as they are read.';
  $('#ru-rw').textContent = '';
}

async function saveRules() {
  const r = await api('rules', { method: 'PUT', json: { db: st.db, text: $('#ru-text').value } });
  $('#ru-msg').textContent = `Saved in ${r.file}${r.empty ? ' (no rule: nothing is changed)' : ''}.`;
}

async function rewrite(dry) {
  if (!dry && !confirm(`Rewrite the paths ${st.db} lists by its [[path]] rules?\n\nOnly the paths change; the files are not read again.`)) return;
  const r = await api('rewrite', { method: 'POST', json: { db: st.db, dry_run: dry } });
  $('#ru-rw').textContent = `${dry ? 'Would rewrite' : 'Rewritten'}: ${plural(r.recordings, 'recording')}, ${r.files} entries' files, ${plural(r.sources, 'source')}' inputs.`;
}

// ------------------------------------------------------------ Check, Sources, Info

async function runCheck() {
  $('#ck-out').innerHTML = '<p class="muted">Checking…</p>';
  const r = await api('check', { method: 'POST', json: { db: st.db } });
  $('#ck-out').innerHTML = r.problems.length
    ? r.problems.map((p) => `<div class="problem"><b>${p.count}</b> ${esc(p.what)}<ul>${p.examples.map((x) => `<li><code>${esc(x)}</code></li>`).join('')}${p.count > p.examples.length ? '<li class="muted">…</li>' : ''}</ul></div>`).join('')
    : '<p class="ok">No problem found.</p>';
}

async function drawSources() {
  const info = await api(`info?${qs({ db: st.db })}`);
  st.sources = info.sources;
  $('#so-grid').innerHTML = `<thead><tr><th scope="col"><span class="sr-only">Choose</span></th><th scope="col" class="num">Id</th><th scope="col">Kind</th><th scope="col" class="num">Rows</th><th scope="col">Added</th><th scope="col">File</th></tr></thead><tbody>${info.sources.length
    ? info.sources.map((s) => `<tr><td><input type="checkbox" data-source="${s.id}" aria-label="Source ${s.id}"></td><td class="num">${s.id}</td><td>${esc(s.kind)}</td><td class="num">${s.rows}</td><td>${esc(s.added_at)}</td><td title="${esc(s.file)}">${esc(s.file)}</td></tr>`).join('')
    : '<tr><td colspan="6" class="muted">No source yet: add some in Import or Recordings.</td></tr>'}</tbody>`;
  $('#so-remove').disabled = true;
  $('#so-msg').textContent = '';
}

async function removeSources() {
  const ids = $$('#so-grid input[data-source]:checked').map((x) => Number(x.dataset.source));
  if (!ids.length) return;
  const list = ids.map((id) => { const s = st.sources.find((x) => x.id === id); return `  ${id}: ${s ? s.file : ''}`; }).join('\n');
  if (!confirm(`Remove ${plural(ids.length, 'source')} from ${st.db}?\n\n${list}\n\nTheir entries, recordings and extras are taken out, and the persons and visits nothing else names. This cannot be undone (keep a copy first: Save a copy…).`)) return;
  const r = await api('remove-source', { method: 'POST', json: { db: st.db, ids } });
  await drawSources();
  $('#so-msg').textContent = r.removed.map((x) => `source ${x.id}: ${x.entries} entries, ${x.recordings} recordings, ${x.visits} visits, ${x.persons} persons`).join('; ');
}

async function drawInfo() {
  const i = await api(`info?${qs({ db: st.db })}`);
  const kv = (rows) => `<dl class="kv">${rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join('')}</dl>`;
  $('#in-out').innerHTML = `<h3>The database</h3>${kv([['file', i.path], ['size', size(i.size)], ['encrypted', i.encrypted ? 'yes' : 'no'],
    ['schema', i.meta.schema], ['version', i.meta.version], ['time zone', i.meta.timezone], ['made by', i.meta.created_by], ['rules file', i.rules_file]])}`
    + `<h3>Its tables</h3>${kv(Object.entries(i.counts).map(([t, n]) => [t, plural(n, 'row')]))}`;
}

// ------------------------------------------------------------ wiring

function wire() {
  $('#login-form').addEventListener('submit', signIn);
  // a dialog's ✕ and Cancel: plain buttons that close it (Enter in its form never cancels)
  document.addEventListener('click', (e) => { const b = e.target.closest('dialog [data-close]'); if (b) b.closest('dialog').close(''); });
  $('#btn-theme').addEventListener('click', () => {
    const t = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
    store.set('theme', t);
    applyTheme(t);
  });
  $('#btn-logout').addEventListener('click', () => { api('logout', { method: 'POST' }).catch(() => {}).finally(showLogin); });
  $('#db-refresh').addEventListener('click', () => listDatabases().catch(fail));
  $('#db-new').addEventListener('click', () => newDb().catch(fail));
  $('#db-open').addEventListener('click', () => openOther().catch(fail));
  $('#db-copy').addEventListener('click', () => saveCopy().catch(fail));
  const pick = (li) => li?.dataset.path && openDb(li.dataset.path).catch(fail);
  $('#db-list').addEventListener('click', (e) => pick(e.target.closest('li')));
  $('#db-list').addEventListener('keydown', (e) => {
    const li = e.target.closest('li');
    if (!li) return;
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); pick(li); }
    if (e.key === 'ArrowDown') { e.preventDefault(); li.nextElementSibling?.focus(); }
    if (e.key === 'ArrowUp') { e.preventDefault(); li.previousElementSibling?.focus(); }
  });
  $('.tabs').addEventListener('click', (e) => { const b = e.target.closest('[role=tab]'); if (b) showTab(b.dataset.tab).catch(fail); });
  $('.tabs').addEventListener('keydown', tabKeys);
  // Tables
  $('#t-table').addEventListener('change', (e) => { st.table = e.target.value; st.offset = 0; drawTable().catch(fail); });
  let typing = null;
  $('#t-filter').addEventListener('input', () => { clearTimeout(typing); typing = setTimeout(() => { st.offset = 0; drawTable().catch(fail); }, 300); });
  $('#t-prev').addEventListener('click', () => { st.offset = Math.max(0, st.offset - PAGE); drawTable().catch(fail); });
  $('#t-next').addEventListener('click', () => { st.offset += PAGE; drawTable().catch(fail); });
  // Import
  drawInputs();
  $('#im-choose').addEventListener('click', async () => {
    const got = await choose({ title: 'Choose tables, exports or CLIF folders', mode: 'files', multiple: true, folders: true });
    if (got) { st.inputs = [...new Set([...st.inputs, ...got])]; drawInputs(); }
  });
  $('#im-clear').addEventListener('click', () => { st.inputs = []; drawInputs(); });
  $('#im-inputs').addEventListener('click', (e) => { const b = e.target.closest('[data-drop]'); if (b) { st.inputs.splice(Number(b.dataset.drop), 1); drawInputs(); } });
  $('#im-mapping-pick').addEventListener('click', async () => { const [p] = (await choose({ title: 'Choose a mapping file (TOML or JSON)', multiple: false })) || []; if (p) $('#im-mapping').value = p; });
  $('#im-formats-pick').addEventListener('click', async () => { const got = await choose({ title: 'Choose declared formats (TOML files, or a folder of them)', multiple: true, folders: true }); if (got) $('#im-formats').value = got.join(', '); });
  $('#im-preview').addEventListener('click', () => { try { runTask('import', importBody(), '#im-task', true); } catch (e) { fail(e); } });
  $('#im-add').addEventListener('click', () => { try { runTask('import', importBody(), '#im-task', false); } catch (e) { fail(e); } });
  // Recordings
  $('#re-folder-pick').addEventListener('click', async () => { const [p] = (await choose({ title: 'Choose the folder of recordings', mode: 'folder' })) || []; if (p !== undefined) $('#re-folder').value = p; });
  $('#re-patients-pick').addEventListener('click', async () => { const [p] = (await choose({ title: 'Choose the patients table', multiple: false })) || []; if (p) $('#re-patients').value = p; });
  $('#re-preview').addEventListener('click', () => { try { runTask('recordings', recordingsBody(), '#re-task', true); } catch (e) { fail(e); } });
  $('#re-add').addEventListener('click', () => { try { runTask('recordings', recordingsBody(), '#re-task', false); } catch (e) { fail(e); } });
  // Rules, Check, Sources
  $('#ru-save').addEventListener('click', () => saveRules().catch((e) => { $('#ru-msg').textContent = ''; fail(e); }));
  $('#ru-dry').addEventListener('click', () => rewrite(true).catch(fail));
  $('#ru-rewrite').addEventListener('click', () => rewrite(false).catch(fail));
  $('#ck-run').addEventListener('click', () => runCheck().catch(fail));
  $('#so-grid').addEventListener('change', () => { $('#so-remove').disabled = !$$('#so-grid input[data-source]:checked').length; });
  $('#so-remove').addEventListener('click', () => removeSources().catch(fail));
}

applyTheme(store.get('theme', null));
wire();
start().catch(() => showLogin());
