// Databases: every imported database, and each one as a table: its rows as they were imported,
// the patient each reached, and what it holds linked to its viewer (an ECG, an image, an echo
// loop, a recording).  The same data as the patient pages, organised by database.  In the list
// a ▸ unfolds a database's table in place (the open ones are remembered in this browser);
// #/databases/12 shows one on a page of its own.  Identifying and unmapped columns are not sent
// to users without the PHI permission.  Admins import (in Import), export (CSV, as imported) and
// delete databases here; a deletion shows at once wherever the data appeared (live.js).

import { $, api, API, catClass, privacy, esc, externalViewerLink, fail, fmtDate, fmtTime, isAdmin, PAGE, phi, qs, removalText, store, toast, viewerMode } from '../core.js';
import { go, href, setParams } from '../router.js';
import { checkData } from '../live.js';
import { APPS, appHref } from './apps.js';

/** Per open table (by database id): its search, filters, order and page. */
const tables = new Map();
const state = (id) => {
  if (!tables.has(id)) tables.set(id, { id, q: '', sort: null, desc: false, offset: 0, filters: {}, timer: null });
  return tables.get(id);
};
let page = null;                      // the database shown on a page of its own, or null (the list)
let open = new Set(store.get('dbOpen', []));
let labels = {};
const kindLabel = (k) => labels[k] || { allergy: 'Allergies' }[k] || k;
const kinds = (cats) => cats.map((k) => `<span class="kind"><span class="sw ${catClass(k)}"></span>${esc(kindLabel(k))}</span>`).join('');

const STATUS = {
  identity: 'patient, encounter', item: '', duplicate: 'duplicate', recording: '', held: 'held back', invalid: 'not read', skipped: 'nothing to read',
};

// ------------------------------------------------------------ a table
function linkHtml(l, row) {
  if (!l) return '';
  // whose file it is, for the viewer's banner (a row that reached its patient)
  const ctx = row?.patient && row?.entry ? { patient: row.patient, entry: row.entry } : {};
  if (l.viewer === 'vitalstream') {
    const t0 = l.t0;
    const t1 = t0 !== null && t0 !== undefined ? t0 + 60 : undefined;
    if (viewerMode() === 'embedded') return `<a class="vlink" href="${esc(href('viewer', [], { path: l.path, t0, t1, ...ctx }))}">VitalStream ↗</a>`;
    if (viewerMode() === 'link') return `<a class="vlink" href="${esc(externalViewerLink(l.path, t0, t1))}" target="vitalstream-viewer" rel="noopener">VitalStream ↗</a>`;
    return '<span class="faint">recording</span>';
  }
  if (l.viewer === 'file') return `<a class="vlink" href="${esc(API + l.src)}" target="_blank" rel="noopener">Open ↗</a>`;
  return `<a class="vlink" href="${esc(appHref(l.viewer, l, API, ctx))}">${esc(APPS[l.viewer])} ↗</a>`;
}

/** Draw database `s.id`'s table into `el` (a `.dbtable` holding data-db). */
async function drawTable(el, s, { header = false } = {}) {
  const params = { q: s.q, sort: s.sort ?? '', desc: s.desc ? 1 : '', limit: PAGE, offset: s.offset };
  for (const [k, v] of Object.entries(s.filters)) if (v) params[`f${k}`] = v;
  let p;
  try {
    p = await api(`databases/${s.id}/rows?${qs(params)}`);
  } catch (e) {
    el.innerHTML = `<p class="empty">${esc(e.message)}</p>`;
    return null;
  }
  const d = p.database;
  const hidden = p.hidden.length
    ? `<span class="faint" title="${esc(p.hidden.join(', '))}">${p.hidden.length} column${p.hidden.length === 1 ? '' : 's'} not shown${phi() ? '' : ' (identifying, or not mapped: they need the PHI permission)'}</span>`
    : '';
  const head = p.columns.map((c) => {
    const on = s.sort === c.pos;
    return `<th><button class="sort${on ? ' on' : ''}" data-sort="${c.pos}" title="${esc(c.maps_to ? `→ ${c.maps_to}` : 'not mapped')}">${esc(c.header)}${on ? (s.desc ? ' ↓' : ' ↑') : ''}</button></th>`;
  }).join('');
  const filt = p.columns.map((c) => `<th><input class="colf" data-f="${c.pos}" value="${esc(s.filters[c.pos] || '')}" placeholder="filter" aria-label="Filter ${esc(c.header)}"></th>`).join('');
  let body = '';
  for (const r of p.rows) {
    const cells = r.cells.map((v, i) => {
      if (i === r.link_cell && r.link) return `<td class="file"><span class="fname" title="${esc(v)}">${esc(v.split(/[\\/]/).pop())}</span> ${linkHtml(r.link, r)}</td>`;
      return `<td title="${esc(v.length > 40 ? v : '')}">${esc(v.length > 80 ? `${v.slice(0, 80)}…` : v)}</td>`;
    }).join('');
    const extra = r.link && r.link_cell === null ? linkHtml(r.link, r) : '';
    const pat = r.patient
      ? `<a class="mono" href="${esc(href('patient', [String(r.patient)], r.entry ? { item: r.entry } : {}))}">${esc(r.patient_id)}</a>`
      : '<span class="faint" title="No database names this patient yet">—</span>';
    const st = STATUS[r.status] ?? r.status;
    body += `<tr${r.status === 'held' || r.status === 'invalid' ? ' class="off"' : ''}><td class="line">${r.line}</td><td>${pat}</td>${cells}<td>${extra}${st ? ` <span class="faint">${esc(st)}</span>` : ''}</td></tr>`;
  }
  if (!p.rows.length) body = `<tr><td colspan="${p.columns.length + 3}" class="empty">No row matches.</td></tr>`;
  const info = `<span class="muted">${d.rows} rows · ${d.patients} patients · ${esc(d.source || d.origin)} · imported ${fmtTime(d.time)}${d.username ? ` by ${esc(d.username)}` : ''}</span>`;
  el.innerHTML = `<div class="ftools">${header ? `<span class="kinds">${kinds(d.categories)}</span>${info}` : `<label class="sbox"><span class="glass">⌕</span><input data-q value="${esc(s.q)}" placeholder="Search these rows" aria-label="Search ${esc(d.name)}"></label>`}<span class="spacer"></span>${hidden}${header ? '' : `<a class="btn" href="${esc(href('databases', [String(d.id)]))}" title="This table on a page of its own">Full page ↗</a>`}${adminTools(d)}</div>
    <div class="table-wrap dbt"><table class="grid dbgrid"><thead><tr><th class="line">Line</th><th>Patient</th>${head}<th></th></tr><tr class="filters"><th></th><th></th>${filt}<th></th></tr></thead><tbody>${body}</tbody></table></div>
    <div class="pager"><span class="muted">${p.total ? `${s.offset + 1}–${s.offset + p.rows.length} of ${p.total}` : ''}</span><span><button data-page="-1"${s.offset === 0 ? ' disabled' : ''}>‹ Previous</button> <button data-page="1"${s.offset + p.rows.length >= p.total ? ' disabled' : ''}>Next ›</button></span></div>`;
  return p;
}

/** Redraw a table after a change, keeping the focus in the box being typed in. */
async function redraw(el, s) {
  const focus = document.activeElement && el.contains(document.activeElement) ? document.activeElement : null;
  const key = focus?.dataset.f ?? (focus?.dataset.q !== undefined ? 'q' : null);
  const caret = focus?.selectionStart;
  await drawTable(el, s, { header: el.dataset.page === '1' });
  if (page && el.dataset.page === '1') syncParams(s);
  if (key !== null) {
    const again = key === 'q' ? el.querySelector('input[data-q]') : el.querySelector(`input[data-f="${key}"]`);
    if (again) { again.focus(); if (caret !== undefined) again.setSelectionRange(caret, caret); }
  }
}

function syncParams(s) {
  const f = Object.fromEntries(Object.entries(s.filters).filter(([, v]) => v).map(([k, v]) => [`f${k}`, v]));
  setParams({ q: s.q, sort: s.sort ?? '', desc: s.desc ? 1 : '', o: s.offset || '', ...f });
}

/** An admin's export and delete for a database. */
function adminTools(d) {
  if (!isAdmin()) return '';
  return `<a class="btn" href="${esc(`${API}databases/${d.id}/export`)}" download title="${privacy.on ? 'As Privacy Mode shows it: every column, identifiers as placeholders, patients by their pseudonyms, dates moved back' : 'The rows as imported, as CSV'}">Export CSV${privacy.on ? ' (Privacy Mode)' : ''}</a>
    <button class="btn danger" data-delete="${d.id}" data-name="${esc(d.name)}">Delete…</button>`;
}

/** Delete a database (admins): preview what goes, confirm, remove; every page follows. */
async function remove(id, name) {
  const p = await api(`imports/${id}/remove`, { method: 'POST', json: { dry_run: true } });
  if (!confirm(`Delete the database “${name}”?\n\nThis removes: ${removalText(p)}\n\nFrom every patient's page, the cohorts and the viewers. Files are never touched.`)) return;
  const purge = p.recordings ? confirm("Also delete the catalog's indexes and results of the removed recordings (in the results folder only)?") : false;
  const r = await api(`imports/${id}/remove`, { method: 'POST', json: { purge } });
  toast(`Deleted “${name}”: ${removalText(r).split('\n')[0]}`);
  open.delete(id);
  store.set('dbOpen', [...open]);
  tables.delete(id);
  if (page === id) await go('#/databases');
  await checkData();
  if (page === null) await list();
}

// ------------------------------------------------------------ the list
async function list() {
  page = null;
  $('#db-list').classList.remove('hidden');
  $('#db-one').classList.add('hidden');
  $('#db-crumb').innerHTML = '<span class="crumb now">Databases</span>';
  $('#db-q').classList.add('hidden');
  const { items } = await api('databases');
  if (!items.length) {
    $('#db-list').innerHTML = `<p class="empty">No database yet${isAdmin() ? ': <a href="#/import">import some</a>' : ''}.</p>`;
    return;
  }
  const ids = new Set(items.map((d) => d.id));
  open = new Set([...open].filter((id) => ids.has(id)));
  $('#db-list').innerHTML = `${isAdmin() ? '<div class="ftools"><span class="spacer"></span><a class="btn primary" href="#/import">Import databases…</a></div>' : ''}<div class="dbfold">
    <div class="dbhead"><span></span><span>Database</span><span>Holds</span><span class="num">Rows</span><span class="num">Patients</span><span>Source</span><span>Imported</span><span></span></div>
    ${items.map((d) => {
      const notes = [d.held ? `<span class="badge">${d.held} held back</span>` : '', d.waiting ? `<span class="pill">${d.waiting} waiting</span>` : ''].join(' ');
      const can = d.viewable;
      const on = can && open.has(d.id);
      const live = d.kind === 'connection';
      const off = live
        ? `A connected database (${d.connection || ''}): its data are read from it when a patient's page opens or a cohort is searched`
        : 'Imported before rows were kept: import it again to see it here';
      return `<div class="dbitem${on ? ' open' : ''}${can || live ? '' : ' off'}" data-id="${d.id}">
        <button class="dbrow" data-toggle="${d.id}"${can ? ` aria-expanded="${on}"` : ` disabled title="${esc(off)}"`}>
          <span class="tri" aria-hidden="true">${can ? '<svg width="12" height="12" viewBox="0 0 12 12"><path d="M4 2l4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>' : ''}</span><span class="nm">${esc(d.name)}${d.kind === 'recordings' ? ' <span class="pill">folder</span>' : ''}${live ? ` <span class="pill">connected · ${esc(d.connection || '')}</span>` : ''}</span>
          <span class="kinds">${kinds(d.categories) || '<span class="faint">—</span>'}</span><span class="num">${live ? '<span class="faint">live</span>' : d.rows}</span><span class="num">${d.patients}</span>
          <span>${d.source && d.source !== d.name ? esc(d.source) : '<span class="faint">—</span>'}</span><span>${fmtDate(d.time)} <span class="faint">${esc(d.username)}</span></span><span>${notes}</span>
        </button>
        <div class="dbtable${on ? '' : ' hidden'}" data-db="${d.id}"></div>
      </div>`;
    }).join('')}</div>`;
  await Promise.all([...open].map((id) => drawTable($(`#db-list .dbtable[data-db="${id}"]`), state(id))));
}

async function toggle(id) {
  const item = $(`#db-list .dbitem[data-id="${id}"]`);
  const el = item.querySelector('.dbtable');
  const on = !open.has(id);
  if (on) open.add(id); else open.delete(id);
  store.set('dbOpen', [...open]);
  item.classList.toggle('open', on);
  item.querySelector('.dbrow').setAttribute('aria-expanded', String(on));
  el.classList.toggle('hidden', !on);
  if (on) {
    el.innerHTML = '<p class="muted">Loading…</p>';
    await drawTable(el, state(id));
  } else {
    el.innerHTML = '';
  }
}

// ------------------------------------------------------------ one database on its own page
async function one(r) {
  const id = +r.parts[0];
  const s = state(id);
  s.q = r.params.get('q') || '';
  s.sort = r.params.get('sort') !== null && r.params.get('sort') !== '' ? +r.params.get('sort') : null;
  s.desc = r.params.get('desc') === '1';
  s.offset = +(r.params.get('o') || 0);
  s.filters = {};
  for (const [k, v] of r.params) if (/^f\d+$/.test(k)) s.filters[k.slice(1)] = v;
  page = id;
  $('#db-list').classList.add('hidden');
  const el = $('#db-one');
  el.classList.remove('hidden');
  el.dataset.db = String(id);
  el.dataset.page = '1';
  $('#db-q').classList.remove('hidden');
  $('#db-q input').value = s.q;
  const p = await drawTable(el, s, { header: true });
  const name = p ? p.database.name : `#${id}`;
  $('#db-crumb').innerHTML = `<a class="crumb" href="#/databases">Databases</a><span class="sep">›</span><span class="crumb now">${esc(name)}</span>`;
}

// ------------------------------------------------------------ events (both places)
function tableOf(t) {
  const el = t.closest('[data-db]');
  return el ? { el, s: state(+el.dataset.db) } : null;
}

function later(s, f) {
  clearTimeout(s.timer);
  s.timer = setTimeout(() => f().catch(fail), 300);
}

export default {
  async show(r) {
    if (!Object.keys(labels).length) {
      const pl = await api('patients?limit=1');
      labels = Object.fromEntries(pl.categories);
    }
    return r.parts[0] ? one(r) : list();
  },
  wire() {
    const view = $('#view-databases');
    view.addEventListener('click', (e) => {
      const t = e.target;
      const del = t.closest('[data-delete]');
      if (del) { remove(+del.dataset.delete, del.dataset.name).catch(fail); return; }
      const tog = t.closest('[data-toggle]');
      if (tog && !tog.disabled) { toggle(+tog.dataset.toggle).catch(fail); return; }
      const x = tableOf(t);
      if (!x) return;
      const sort = t.closest('button[data-sort]');
      if (sort) {
        const pos = +sort.dataset.sort;
        if (x.s.sort === pos) x.s.desc = !x.s.desc; else { x.s.sort = pos; x.s.desc = false; }
        x.s.offset = 0;
        redraw(x.el, x.s).catch(fail);
      }
      const pg = t.closest('button[data-page]');
      if (pg) {
        x.s.offset = Math.max(0, x.s.offset + +pg.dataset.page * PAGE);
        redraw(x.el, x.s).catch(fail);
      }
    });
    view.addEventListener('input', (e) => {
      const t = e.target;
      if (t.closest('#db-q')) {
        const s = state(page);
        later(s, async () => { s.q = t.value.trim(); s.offset = 0; await redraw($('#db-one'), s); });
        return;
      }
      const x = tableOf(t);
      if (!x) return;
      if (t.dataset.f !== undefined) later(x.s, async () => { x.s.filters[t.dataset.f] = t.value.trim(); x.s.offset = 0; await redraw(x.el, x.s); });
      if (t.dataset.q !== undefined) later(x.s, async () => { x.s.q = t.value.trim(); x.s.offset = 0; await redraw(x.el, x.s); });
    });
  },
};
