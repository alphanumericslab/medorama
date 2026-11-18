// Databases: the connected databases a user is granted (admins: every one, to grant it), each
// with its status: a green dot online, a grey one offline (when it was last seen and why), never
// red (red is for alarms).  Online ones first.  Who manages a database checks it, refreshes what
// Medorama keeps of it, sees its variables and identity conflicts, grants it and removes it;
// admins connect one (docs/design/databases-only.md).  #/databases/12 shows one database.

import { $, api, esc, fail, fmtTime, isAdmin, st, toast } from '../core.js';
import { go, href } from '../router.js';
import { checkData } from '../live.js';
import { chooseOnServer } from '../serverfiles.js';

let page = null;      // the database shown in full, or null
let people = null;    // the users a database can be granted to

const PERMS = ['search', 'view', 'phi', 'manage'];
const PERM_TEXT = {
  search: 'its patients found by criteria',
  view: 'and its data on patient pages',
  phi: 'and its identifiers, outside Privacy Mode',
  manage: 'and checking, granting, removing it',
};

function status(d) {
  const on = d.status === 'online';
  const when = d.checked_at ? ` · checked ${fmtTime(d.checked_at)}` : '';
  return `<span class="dbstat ${on ? 'on' : 'off'}" title="${esc(on ? 'Online' : `Offline${d.status_error ? `: ${d.status_error}` : ''}`)}"><span class="dot" aria-hidden="true"></span>${on ? 'online' : esc(d.status === 'unknown' ? 'not checked' : 'offline')}</span><span class="faint small">${esc(when)}</span>`;
}

function row(d) {
  const counts = d.permission
    ? `${d.patients ?? 0} patient${d.patients === 1 ? '' : 's'} · ${d.variables ?? 0} variable${d.variables === 1 ? '' : 's'}${d.recordings ? ` · ${d.recordings} recording${d.recordings === 1 ? '' : 's'} opened` : ''}${d.conflicts ? ` · <span title="Persons whose identifiers name two patients: not linked">${d.conflicts} conflict${d.conflicts === 1 ? '' : 's'}</span>` : ''}`
    : '<span class="faint">not granted to you</span>';
  return `<tr class="${d.status === 'online' ? '' : 'off'}"><td><a href="${esc(href('databases', [String(d.id)]))}"><b>${esc(d.name)}</b></a></td><td>${esc(kindText(d.kind))} · ${esc(d.engine)}</td><td>${status(d)}</td><td>${counts}</td><td>${esc(d.permission || '')}</td></tr>`;
}

const kindText = (k) => ({ dbb: 'db-builder', omop: 'OMOP', views: 'SQL views' })[k] || k;

async function drawList() {
  const r = await api('databases');
  if (!r.items.length) {
    $('#db-list').innerHTML = `<p class="empty">${isAdmin() ? 'No database is connected yet: connect one below.' : 'No database is granted to you yet: an admin grants them.'}</p>`;
    return r.items;
  }
  $('#db-list').innerHTML = `<div class="table-wrap short"><table class="grid"><tr><th>Database</th><th>Kind</th><th>Status</th><th>What it holds</th><th>Yours</th></tr>${r.items.map(row).join('')}</table></div>`;
  return r.items;
}

async function drawOne(d) {
  const el = $('#db-one');
  const manage = d.permission === 'manage';
  let h = `<div class="ftools"><h2>${esc(d.name)}</h2>${status(d)}<span class="spacer"></span><a href="#/databases" class="link">All databases</a></div>`;
  h += `<p class="muted">${esc(kindText(d.kind))} · ${esc(d.engine)}${d.url ? ` · <span class="mono">${esc(d.url)}</span>` : ''}${d.version ? ` · version ${esc(d.version)}` : ''} · connected ${fmtTime(d.created_at)}</p>`;
  if (d.status_error) h += `<p class="err">${esc(d.status_error)}</p>`;
  if (d.permission) {
    h += '<div class="ftools"><button data-act="check">Check now</button>';
    if (manage) {
      h += '<button data-act="refresh" title="Read its catalog (the names and units of its variables), its persons and their encounters again">Refresh</button>';
      h += '<button data-act="count" title="Count how many entries and persons hold each variable: one pass over its entries, which takes a while on a large database">Count now</button>';
      if (d.has_recordings) h += '<button data-act="index" title="Read every recording it lists, check its files and index them (a patient\'s recordings are otherwise read and indexed when they are opened)">Index all recordings</button>';
      h += '<button data-act="remove" class="danger">Remove…</button>';
    }
    h += '</div><h3>Variables</h3><div id="db-vars" class="table-wrap short"><p class="muted">Reading…</p></div>';
  }
  if (manage) {
    h += '<h3>Who may use it</h3><div id="db-grants"></div>';
    h += `<form id="db-grant" class="ftools wrap"><select id="db-grant-user" aria-label="User"></select><select id="db-grant-perm" aria-label="Permission">${PERMS.map((p) => `<option value="${p}">${p}: ${esc(PERM_TEXT[p])}</option>`).join('')}</select><button>Grant</button></form>`;
    if (d.conflicts) h += '<h3>Identity conflicts</h3><p class="muted small">Persons whose identifiers name two different patients: not linked until the database is corrected (Medorama does not merge patients).</p><div id="db-conflicts"></div>';
  } else if (isAdmin() && !d.permission) {
    h += '<p class="muted">An admin is not granted a database by being an admin: grant it to yourself (it is audited).</p>';
    h += `<form id="db-grant" class="ftools wrap"><input type="hidden" id="db-grant-self" value="1"><select id="db-grant-perm" aria-label="Permission">${PERMS.map((p) => `<option value="${p}">${p}: ${esc(PERM_TEXT[p])}</option>`).join('')}</select><button>Grant it to me</button></form>`;
  }
  el.innerHTML = h;
  el.dataset.id = d.id;
  el.dataset.name = d.name;
  // an answer for a database no longer shown (another one opened meanwhile): dropped
  const still = () => el.dataset.id === String(d.id) && !el.classList.contains('hidden');
  if (d.permission) {
    api(`databases/${d.id}/variables`).then((v) => {
      if (!still()) return;
      const n = (x) => (x === null || x === undefined ? '<span class="faint">—</span>' : esc(x));
      const rows = v.items.slice(0, 500).map((x) => `<tr><td>${esc(x.category)}</td><td>${esc(x.name)}</td><td>${esc(x.unit)}</td><td>${esc([x.vocabulary, x.code].filter(Boolean).join(' '))}</td><td class="num">${n(x.entries)}</td><td class="num">${n(x.persons)}</td></tr>`).join('');
      const when = v.counted_at
        ? `<p class="muted small">Counted ${fmtTime(v.counted_at)}; counts below 11 are shown as “&lt; 11”.</p>`
        : '<p class="muted small">Not counted: the names and units only (Count now counts them).</p>';
      $('#db-vars').innerHTML = v.items.length
        ? `${when}<table class="grid"><tr><th>Category</th><th>Name</th><th>Unit</th><th>Code</th><th class="num">Entries</th><th class="num">Persons</th></tr>${rows}</table>${v.items.length > 500 ? `<p class="muted small">The first 500 of ${v.items.length}.</p>` : ''}`
        : '<p class="muted">No variables.</p>';
    }).catch((e) => { if (still()) $('#db-vars').innerHTML = `<p class="err">${esc(e.message)}</p>`; });
  }
  if (manage) {
    const [g, users] = await Promise.all([api(`databases/${d.id}/grants`), people || api('cohorts/people').then((r) => { people = r.items; return people; })]);
    if (!still()) return;
    const by = new Map();
    for (const x of g.items) by.set(x.username, [...(by.get(x.username) || []), x]);
    $('#db-grants').innerHTML = by.size
      ? `<table class="grid"><tr><th>User</th><th>Permissions</th><th></th></tr>${[...by].map(([u, xs]) => `<tr><td>${esc(u)}</td><td>${xs.map((x) => esc(x.permission)).join(', ')}</td><td>${xs[0].user_id === st.user?.id ? '<span class="faint">you</span>' : `<button class="link" data-revoke="${xs[0].user_id}" data-perm="search">Revoke all</button>`}</td></tr>`).join('')}</table>`
      : '<p class="muted">Nobody else.</p>';
    $('#db-grant-user').innerHTML = users.map((u) => `<option value="${u.id}">${esc(u.display_name || u.username)}</option>`).join('');
    if (d.conflicts) {
      const c = await api(`databases/${d.id}/conflicts`);
      if (!still()) return;
      $('#db-conflicts').innerHTML = `<table class="grid"><tr><th>Person (the database's key)</th><th>What is wrong</th></tr>${c.items.map((x) => `<tr><td class="mono">${esc(x.person_key)}</td><td>${esc(x.detail)}</td></tr>`).join('')}</table>`;
    }
  }
}

/** "Create a database…": db-builder's own page (its own sign-in), in a tab named db-builder (a
 * second click shows the same tab); hidden when Medorama runs and links to none. */
function drawCreate() {
  const d = st.cfg?.db_builder;
  const on = isAdmin() && d && d.mode !== 'off' && d.url;
  $('#db-create').classList.toggle('hidden', !on);
  if (!on) return;
  $('#db-create-link').href = d.url;
  $('#db-create-note').textContent = d.reason
    ? `db-builder is not running: ${d.reason}`
    : 'db-builder opens in a tab of its own, with its own sign-in; connect here what it builds.';
  $('#db-create-note').classList.toggle('err', !!d.reason);
}

async function show(r) {
  page = r.parts[0] ? Number(r.parts[0]) : null;
  drawCreate();
  const items = await drawList();
  $('#cn-result').textContent = '';
  const one = page !== null && items.find((d) => d.id === page);
  const el = $('#db-one');
  el.classList.toggle('hidden', page === null);
  if (one) await drawOne(one);
  else if (page !== null) {
    // an address of a database not here (removed, or not granted): said, not hidden
    el.dataset.id = '';
    el.dataset.name = '';
    el.innerHTML = `<div class="ftools"><h2>No database ${esc(String(r.parts[0]))}</h2><span class="spacer"></span><a href="#/databases" class="link">All databases</a></div>
      <p class="muted">No database ${esc(String(r.parts[0]))} here (removed, or not granted to you).</p>`;
  }
}

function showWhere() {
  const kind = $('#cn-kind').value;
  // a db-builder database is a SQLite file
  if (kind === 'dbb') $('#cn-where').value = 'file';
  $('#cn-where').disabled = kind === 'dbb';
  const server = $('#cn-where').value === 'server';
  $('#cn-url-l').classList.toggle('hidden', !server);
  // a file's password: its SQLCipher key, when it is encrypted (db-builder --encrypt)
  $('#cn-pass-t').textContent = server ? 'Password' : 'Key, if encrypted';
  $('#cn-pass-n').textContent = server
    ? 'Kept wrapped by the master key; never shown again.'
    : 'Only for an encrypted file (db-builder --encrypt): its 64 hex digits, or its passphrase. Kept wrapped by the master key.';
  for (const id of ['cn-schema-l', 'cn-vschema-l', 'cn-ident-l']) $(`#${id}`).classList.toggle('hidden', kind !== 'omop');
  $('#cn-path-l').classList.toggle('hidden', server);
  // where it is, required as it is given: a file's path, or a server's address
  $('#cn-path').required = !server;
  $('#cn-url').required = server;
  $('#cn-views').classList.toggle('hidden', kind !== 'views');
}

let connecting = false;

/** The form while a database is being connected: not sent twice, said to be busy. */
function busy(on) {
  connecting = on;
  const f = $('#db-connect');
  f.setAttribute('aria-busy', String(on));
  for (const x of f.querySelectorAll('input, select, textarea, button')) {
    if (on) { x.dataset.was = x.disabled ? '1' : ''; x.disabled = true; } else { x.disabled = x.dataset.was === '1'; delete x.dataset.was; }
  }
}

async function connect() {
  if (connecting) return;
  const v = (id) => $(`#${id}`).value.trim();
  const server = $('#cn-where').value === 'server';
  const kind = v('cn-kind');
  const settings = {
    issuer: v('cn-issuer'), mrn_authority: v('cn-auth'), timezone: v('cn-tz'),
    ...(kind === 'omop' ? { schema: v('cn-schema'), vocabulary_schema: v('cn-vschema'), identifier: v('cn-ident') } : {}),
    ...(kind === 'views' ? { persons: v('cn-v-persons'), visits: v('cn-v-visits'), entries: v('cn-v-entries'), ...(v('cn-v-stays') ? { stays: v('cn-v-stays') } : {}), ...(v('cn-v-has') ? { has: v('cn-v-has') } : {}), recordings: v('cn-v-recordings'), version: v('cn-v-version') } : {}),
  };
  if (!(server ? v('cn-url') : v('cn-path'))) {
    $(server ? '#cn-url' : '#cn-path').focus();
    throw new Error(server ? "Give the server's address" : "Give the file's path on this server");
  }
  $('#cn-result').textContent = 'Connecting, checking and reading it…';
  busy(true);
  try {
    const r = await api('databases', {
      method: 'POST',
      json: { name: v('cn-name'), kind, url: server ? v('cn-url') : `sqlite:${v('cn-path')}`, password: $('#cn-pass').value, settings },
    });
    $('#cn-pass').value = '';
    $('#cn-result').textContent = 'Connected.';
    checkData();
    await go(href('databases', [String(r.id)]));
  } catch (e) {
    $('#cn-result').textContent = '';
    throw e;
  } finally {
    busy(false);
  }
}

async function act(what) {
  const el = $('#db-one');
  const { id, name } = el.dataset;
  if (what === 'check') {
    const r = await api(`databases/${id}/check`, { method: 'POST' });
    toast(r.online ? `${name} is online` : `${name} does not answer`, !r.online);
  } else if (what === 'refresh') {
    toast(`Reading ${name} again…`);
    await api(`databases/${id}/refresh`, { method: 'POST' });
    toast(`${name} refreshed`);
    checkData();
  } else if (what === 'count') {
    toast(`Counting ${name}'s variables…`);
    const r = await api(`databases/${id}/count`, { method: 'POST' });
    toast(`${r.counted} variable${r.counted === 1 ? '' : 's'} counted`);
  } else if (what === 'index') {
    toast(`Reading ${name}'s recordings…`);
    const r = await api(`databases/${id}/index`, { method: 'POST' });
    toast(`${r.recordings} recording${r.recordings === 1 ? '' : 's'} read, ${r.missing} missing; ${r.queued} queued for indexing`);
  } else if (what === 'remove') {
    const typed = prompt(`Remove ${name} and every trace of it from Medorama?\n\nIts key is destroyed, then its workspace folder (its persons, indexes, results); its patients leave every cohort. The database itself is not touched. This cannot be undone.\n\nType its name to remove it:`);
    if (typed === null) return;
    if (typed.trim() !== name) { toast('Not removed: the name typed differs', true); return; }
    await api(`databases/${id}`, { method: 'DELETE' });
    toast(`${name} removed`);
    checkData();
    await go('#/databases');
    return;
  }
  await show({ parts: [id] });
}

export default {
  section: 'databases',
  show,
  invalidate() { people = null; },
  wire() {
    $('#cn-kind').addEventListener('change', showWhere);
    $('#cn-where').addEventListener('change', showWhere);
    showWhere();
    $('#cn-path-pick').addEventListener('click', async () => {
      const [path] = (await chooseOnServer({ kind: 'databases', multiple: false })) || [];
      if (path) $('#cn-path').value = path;
    });
    $('#db-connect').addEventListener('submit', (e) => { e.preventDefault(); connect().catch(fail); });
    $('#db-one').addEventListener('click', (e) => {
      const b = e.target.closest('[data-act]');
      if (b) { act(b.dataset.act).catch(fail); return; }
      const rv = e.target.closest('[data-revoke]');
      if (rv) {
        api(`databases/${$('#db-one').dataset.id}/grants`, { method: 'DELETE', json: { user_id: Number(rv.dataset.revoke), permission: rv.dataset.perm } })
          .then(() => show({ parts: [$('#db-one').dataset.id] })).catch(fail);
      }
    });
    $('#db-one').addEventListener('submit', (e) => {
      if (e.target.id !== 'db-grant') return;
      e.preventDefault();
      // to oneself (an admin not granted it yet): no user named
      const json = { permission: $('#db-grant-perm').value };
      if (!$('#db-grant-self')) json.user_id = Number($('#db-grant-user').value);
      api(`databases/${$('#db-one').dataset.id}/grants`, { method: 'POST', json })
        .then(() => show({ parts: [$('#db-one').dataset.id] })).catch(fail);
    });
  },
};
