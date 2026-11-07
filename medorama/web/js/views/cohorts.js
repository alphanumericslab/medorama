// Cohorts: find patients across every database by criteria, and keep them as named cohorts.
//   #/cohorts                  a new cohort: the criteria builder and its search
//   #/cohorts/new?within=7     a new subcohort of cohort 7: the search looks among its members
//   #/cohorts/7                a cohort: its definition, members (added by search, removed with
//                              ✕), refresh, clone, merge, share, delete, exports, history
//   #/cohorts/7/edit           its name, description and criteria
// The criteria are a structured query (cohorts.rs), the same a language engine can write later:
// patients who meet any group of inclusion conditions (all of a group's), and none of the
// exclusions (in their whole history).  A condition may be an expression written as text
// (expr.rs), checked as it is typed; the search runs as they change.  A cohort's definition
// (the server's words) is its source (a parent, cohorts merged), its criteria, and the patients
// added and removed manually.

import { $, API, PAGE, ages, api, catClass, privacy, esc, fail, fmtDate, fmtTime, isAdmin, phi, qs, toast } from '../core.js';
import { focusLost, go, href, setParams } from '../router.js';
import { dataVersion } from '../live.js';

const co = {
  list: [], cats: [], dbs: [], people: [], columns: {},
  editing: null,                // the cohort being edited, or
  parent: null,                 // the parent of a new subcohort (neither: a new cohort)
  mode: '',                     // which builder is shown (a new search starts empty)
  inc: [], exc: [], kept: null, // inclusion groups (each: conditions), exclusions; a query kept as it is
  result: null, selected: new Set(), offset: 0, timer: null, seq: 0, searching: null, words: {},
  cohort: null, moffset: 0,     // the cohort shown, its members' page
  found: null, fseq: 0, ftimer: null, // patients found to add to it
};

// ------------------------------------------------------------ criteria
const KINDS = [
  ['expression', 'Expression'], ['measurement', 'Lab or vital sign'], ['column', "A database's column"],
  ['diagnosis', 'Diagnosis'], ['medication', 'Medication'], ['has', 'Has data'],
  ['encounter', 'Encounter'], ['sex', 'Sex'], ['age', 'Age'], ['text', 'Text in notes and reports'], ['database', 'In a database'],
];
const CMPS = [['', 'between'], ['>', '>'], ['>=', '≥'], ['<', '<'], ['<=', '≤'], ['=', '='], ['!=', '≠']];
const EXAMPLE = '(age > 18 AND age < 85 AND female) OR (age > 18 AND age < 65 AND male AND QT > 400ms)';
const blank = (kind) => ({ kind, not: false });
const dbName = (id) => co.dbs.find((d) => d.id === id)?.name?.replace(/\.(csv|tsv|txt|xlsx|xls)$/i, '') || '?';
const num = (x) => x !== undefined && x !== '' && x !== null && isFinite(+x);

function describe(k) {
  const range = (a, b) => (num(a) && num(b) ? ` ${a}–${b}` : num(a) ? ` ≥ ${a}` : num(b) ? ` ≤ ${b}` : '');
  const value = () => (k.cmp && num(k.value) ? ` ${k.cmp} ${k.value}${k.unit ? ` ${k.unit}` : ''}` : range(k.min, k.max));
  const dates = k.from || k.to ? ` (${k.from || '…'} → ${k.to || '…'})` : '';
  const lbl = Object.fromEntries(co.cats);
  const s = {
    age: `age${value()}`, sex: `sex ${k.text || '?'}`, has: `has ${lbl[k.category] || k.category || '?'}`,
    encounter: `encounter${k.text ? ` in ${k.text}` : ''}${k.icu === true ? ', with ICU stay' : k.icu === false ? ', without ICU stay' : ''}${dates}`,
    diagnosis: `diagnosis ${k.text || '?'}`,
    measurement: `${k.latest ? 'latest ' : ''}${k.name || '?'}${value()}${k.import ? ` (${dbName(k.import)})` : ''}${dates}`,
    column: `${k.latest ? 'latest ' : ''}${dbName(k.import)}.${k.name || '?'}${k.text ? ` contains “${k.text}”` : value()}${dates}`,
    medication: `medication ${k.text || '?'}${dates}`, text: `“${k.text || '?'}”`,
    database: `in ${dbName(k.import)}`, expression: k.text || '?',
  }[k.kind] || k.kind;
  return `${k.not ? 'not ' : ''}${s}`;
}

/** An expression of the API in words (for kept queries and the cohort's page). */
function words(e) {
  if (!e) return '';
  if (!e.op) return describe(e);
  const parts = e.of.map((x) => (x.op && x.op !== e.op && x.of.length > 1 ? `(${words(x)})` : words(x)));
  return e.op === 'not' ? `not (${parts.join(', ')})` : parts.join(` ${e.op.toUpperCase()} `);
}

const at = (g, i) => (g === 'x' ? co.exc[i] : co.inc[g][i]);

function field(g, i, key, label, type = 'text', extra = '') {
  const v = at(g, i)[key] ?? '';
  return `<label class="cf"><span>${esc(label)}</span><input data-g="${g}" data-i="${i}" data-k="${key}" type="${type}" value="${esc(v)}"${extra}></label>`;
}

function editor(k, g, i) {
  const n = (key, label) => field(g, i, key, label, 'number', ' step="any" class="n"');
  const dates = `${field(g, i, 'from', 'from', 'date')}${field(g, i, 'to', 'to', 'date')}`;
  const sel = (key, opts, label) => `<label class="cf"><span>${esc(label)}</span><select data-g="${g}" data-i="${i}" data-k="${key}">${opts.map(([v, l]) => `<option value="${esc(v)}"${String(k[key] ?? '') === String(v) ? ' selected' : ''}>${esc(l)}</option>`).join('')}</select></label>`;
  const value = (units) => `${sel('cmp', CMPS, '')}${k.cmp ? `${n('value', '')}${units ? field(g, i, 'unit', '', 'text', ' class="u" placeholder="unit" list="co-units"') : ''}` : `${n('min', 'from')}${n('max', 'to')}`}`;
  const latest = `<label class="cf"><input type="checkbox" data-g="${g}" data-i="${i}" data-k="latest"${k.latest ? ' checked' : ''}><span>latest only</span></label>`;
  const tables = co.dbs.filter((d) => d.kind !== 'recordings');
  switch (k.kind) {
    case 'expression': {
      const w = co.words[k.text || ''];
      return `<label class="cf expr"><input data-g="${g}" data-i="${i}" data-k="text" value="${esc(k.text || '')}" placeholder="${esc(EXAMPLE)}" spellcheck="false" aria-label="Expression"></label>
        <span class="ewords ${w?.ok === false ? 'bad' : ''}" data-words="${g}:${i}">${w ? esc(w.ok ? `= ${w.words}` : w.error) : ''}</span>`;
    }
    case 'measurement': return `${field(g, i, 'name', 'name', 'text', ' placeholder="lactate, heart rate…"')}${value(true)}${latest}${sel('import', [['', 'any database'], ...tables.map((d) => [d.id, dbName(d.id)])], 'in')}${dates}`;
    case 'column': {
      const cols = co.columns[k.import] || [];
      return `${sel('import', [['', 'choose…'], ...tables.map((d) => [d.id, dbName(d.id)])], 'database')}
        ${k.import ? sel('name', [['', cols.length ? 'choose…' : 'loading…'], ...cols.map((c) => [c.header, c.header])], 'column') : ''}
        ${k.name ? `${sel('mode', [['num', 'value'], ['text', 'contains']], '')}${k.mode === 'text' ? field(g, i, 'text', '', 'text', ' placeholder="text"') : `${value(true)}${latest}`}` : ''}`;
    }
    case 'diagnosis': return field(g, i, 'text', 'code or name', 'text', ' placeholder="I21, sepsis…"');
    case 'medication': return `${field(g, i, 'text', 'name', 'text', ' placeholder="heparin…"')}${dates}`;
    case 'has': return sel('category', [['', 'choose…'], ...co.cats], 'data');
    case 'encounter': return `${field(g, i, 'text', 'unit', 'text', ' placeholder="ICU7, cardiology…"')}${sel('icu', [['', 'any'], ['true', 'with an ICU stay'], ['false', 'without an ICU stay']], 'ICU')}${dates}`;
    case 'sex': return sel('text', [['', 'choose…'], ['F', 'F'], ['M', 'M']], 'sex');
    case 'age': return value(false);
    case 'text': return field(g, i, 'text', 'contains', 'text', ' placeholder="at least 3 letters"');
    case 'database': return sel('import', [['', 'choose…'], ...co.dbs.map((d) => [d.id, dbName(d.id)])], 'database');
    default: return '';
  }
}

// the server searches by age only for a user with the PHI permission out of Privacy Mode
const NO_AGE = 'Age comes from the date of birth: it needs the PHI permission, out of Privacy Mode';

function line(k, g, i) {
  if (k.kind === 'group') {
    // a group of conditions made through the API: kept as it is, removable, not editable here
    return `<div class="crit wide kept"><span class="muted">A group made elsewhere, kept as it is (not editable here):</span> <b>${esc(words(k.expr))}</b>
    <button class="icon x" data-del="${g}:${i}" title="Remove this exclusion" aria-label="Remove">✕</button></div>`;
  }
  const kinds = KINDS.filter(([v]) => v !== 'age' || ages());
  const off = (v) => (v === 'age' && !phi() && v !== k.kind ? ` disabled title="${esc(NO_AGE)}"` : '');
  return `<div class="crit${k.kind === 'expression' ? ' wide' : ''}">
    ${g === 'x' ? '' : `<select data-g="${g}" data-i="${i}" data-k="not" class="inc" aria-label="Has or has not"><option value="0"${k.not ? '' : ' selected'}>with</option><option value="1"${k.not ? ' selected' : ''}>without</option></select>`}
    <select data-g="${g}" data-i="${i}" data-k="kind" class="kind" aria-label="Kind of criterion">${kinds.map(([v, l]) => `<option value="${v}"${v === k.kind ? ' selected' : ''}${off(v)}>${esc(l)}</option>`).join('')}</select>
    <span class="cfs">${editor(k, g, i)}</span>
    <button class="icon x" data-del="${g}:${i}" title="Remove this condition" aria-label="Remove">✕</button></div>`;
}

function renderCriteria() {
  // the field that has the focus is drawn again: the focus goes back to it
  const a = document.activeElement;
  const had = a?.closest?.('#co-builder') && a.dataset.k !== undefined ? `[data-g="${a.dataset.g}"][data-i="${a.dataset.i}"][data-k="${a.dataset.k}"]` : null;
  let h = '';
  if (co.kept) {
    h += `<div class="crit kept"><span class="muted">Criteria made elsewhere, kept as they are:</span> <b>${esc(words(co.kept))}</b>
      <button class="btn small" id="co-unkeep">Replace them</button></div>`;
  } else {
    h += co.inc.map((grp, g) => `${g ? '<div class="orsep"><span>or</span></div>' : ''}<div class="cgroup">
        ${grp.map((k, i) => `${i ? '<div class="andsep">and</div>' : ''}${line(k, g, i)}`).join('')}
        <div class="gtools"><button class="link" data-addto="${g}">+ and…</button><button class="link" data-addexpr="${g}">+ and an expression</button></div></div>`).join('');
    if (!co.inc.length) h += '<p class="muted">No inclusion condition: every patient is included. Add one below.</p>';
    h += `<div class="gtools top"><button class="btn" id="co-addgroup">${co.inc.length ? '+ or another group' : '+ Add a condition'}</button><button class="btn" id="co-addexpr">+ Write an expression</button></div>`;
  }
  $('#co-criteria').innerHTML = h;
  $('#co-exclude').innerHTML = `${co.exc.map((k, i) => line(k, 'x', i)).join('')}
    ${co.exc.length ? '' : '<p class="muted small">No exclusion. An exclusion takes out every included patient who ever had it.</p>'}
    <div class="gtools top"><button class="btn" id="co-addexc">+ Add an exclusion</button></div>`;
  for (const k of [...co.inc.flat(), ...co.exc]) if (k.kind === 'column' && k.import && !co.columns[k.import]) loadColumns(k.import);
  if (had && focusLost()) document.querySelector(had)?.focus();
}

/** The focus on a condition just added (its field, or the choice of its kind). */
function focusCondition(crit) {
  if (crit) (crit.querySelector('input[data-k="text"]') || crit.querySelector('select.kind'))?.focus();
}

async function loadColumns(id) {
  co.columns[id] = [];
  try {
    const r = await api(`databases/${id}/rows?${qs({ limit: 1 })}`);
    co.columns[id] = r.columns.filter((c) => !['patient_id', 'mrn', 'encounter_id', 'csn', 'last_name', 'first_name', 'birth_date', 'file'].includes(c.maps_to));
  } catch (e) { co.columns[id] = []; }
  renderCriteria();
}

/** A condition as the API takes it, or null while it is still being written. */
function toApi(k) {
  if (k.kind === 'group') return k.expr;
  const o = { kind: k.kind };
  if (k.not) o.not = true;
  for (const key of ['text', 'name', 'category', 'from', 'to', 'cmp', 'unit']) if (k[key]) o[key] = String(k[key]).trim();
  // dates typed in Privacy Mode are sent as typed: the server reads them as shown, per patient;
  // dates once written so (privacy_dates) keep that meaning
  if (k.privacy_dates && (o.from || o.to)) o.privacy_dates = true;
  for (const key of ['import']) if (num(k[key])) o[key] = +k[key];
  if (k.cmp) { if (num(k.value)) o.value = +k.value; } else for (const key of ['min', 'max']) if (num(k[key])) o[key] = +k[key];
  if (!k.cmp) delete o.unit;
  if (k.latest) o.latest = true;
  if (k.icu === 'true' || k.icu === true) o.icu = true;
  if (k.icu === 'false' || k.icu === false) o.icu = false;
  if (k.kind === 'column' && k.mode === 'text') { delete o.cmp; delete o.value; delete o.min; delete o.max; delete o.unit; delete o.latest; }
  if (k.kind === 'column' && k.mode !== 'text') delete o.text;
  const hasValue = o.value !== undefined || o.min !== undefined || o.max !== undefined;
  const need = {
    measurement: o.name, diagnosis: o.text, medication: o.text, has: o.category, sex: o.text, text: o.text && o.text.length >= 3,
    database: o.import, age: hasValue, encounter: true, column: o.import && o.name && (hasValue || o.text || k.mode !== 'text'),
    expression: o.text && co.words[o.text]?.ok,
  };
  return need[o.kind] ? o : null;
}

/** The query as the API takes it (version 2). */
function query() {
  if (co.kept) return { version: 1, include: co.kept, exclude: co.exc.map(toApi).filter(Boolean) };
  const groups = co.inc.map((g) => g.map(toApi).filter(Boolean)).filter((g) => g.length);
  const and = (g) => (g.length === 1 ? g[0] : { op: 'and', of: g });
  const q = { version: 1 };
  if (groups.length) q.include = groups.length === 1 ? and(groups[0]) : { op: 'or', of: groups.map(and) };
  const exc = co.exc.map(toApi).filter(Boolean);
  if (exc.length) q.exclude = exc;
  return q;
}

/** A stored query as groups of conditions, or null when it is not of that form. */
function toGroups(q) {
  const leaf = (e) => !e.op;
  const grp = (e) => (leaf(e) ? [e] : e.op === 'and' && e.of.every(leaf) ? e.of : null);
  const prep = (k) => ({ ...k, mode: k.kind === 'column' && k.text ? 'text' : undefined });
  if (q.all?.length && !q.include) return [q.all.map(prep)];
  if (!q.include) return [];
  if (q.include.op === 'or') {
    const gs = q.include.of.map(grp);
    return gs.every(Boolean) ? gs.map((g) => g.map(prep)) : null;
  }
  const g = grp(q.include);
  return g ? [g.map(prep)] : null;
}

/** Check the expressions being written (the server parses them), then search. */
async function checkWords() {
  const texts = [...co.inc.flat(), ...co.exc].filter((k) => k.kind === 'expression' && k.text?.trim()).map((k) => k.text);
  await Promise.all(texts.filter((t) => !co.words[t]).map(async (t) => {
    try { co.words[t] = await api('cohorts/parse', { method: 'POST', json: { text: t } }); } catch (e) { co.words[t] = { ok: false, error: e.message }; }
  }));
  for (const el of document.querySelectorAll('[data-words]')) {
    const [g, i] = el.dataset.words.split(':');
    const k = at(g === 'x' ? 'x' : +g, +i);
    const w = k && co.words[k.text || ''];
    el.textContent = w ? (w.ok ? `= ${w.words}` : w.error) : '';
    el.classList.toggle('bad', w?.ok === false);
  }
}

// ------------------------------------------------------------ search
/** Where the builder's search looks: a new subcohort's parent, or an edited cohort's source. */
function scope() {
  if (co.parent) return { source: { op: 'within', of: [co.parent.id] } };
  if (co.editing?.source) return { within_source_of: co.editing.id };
  return {};
}

async function search() {
  await checkWords();
  const seq = ++co.seq;
  // the search before this one, if it still runs, is not waited for
  co.searching?.abort();
  const ctl = new AbortController();
  co.searching = ctl;
  $('#co-count').textContent = 'Searching…';
  let r;
  try {
    r = await api('cohorts/search', { method: 'POST', json: { criteria: query(), ...scope(), limit: PAGE, offset: co.offset }, signal: ctl.signal });
  } catch (e) {
    if (seq === co.seq) { $('#co-count').textContent = ''; $('#co-results').innerHTML = `<p class="err">${esc(e.message)}</p>`; }
    return;
  } finally {
    if (co.searching === ctl) co.searching = null;
  }
  if (seq !== co.seq) return;
  co.result = r;
  co.selected = new Set([...co.selected].filter((id) => r.ids.includes(id)));
  renderResults();
}

function patientCells(p) {
  const P = phi();
  const name = P ? [p.phi?.last_name, p.phi?.first_name].filter(Boolean).join(', ') || '—' : p.patient_id;
  const lbl = Object.fromEntries(co.cats);
  return `<td class="nm"><a href="${esc(href('patient', [String(p.id)]))}">${esc(name)}</a></td><td class="mono">${esc(p.patient_id)}</td><td>${esc(p.sex)}</td>${ages() ? `<td class="num">${p.age ?? ''}</td>` : ''}
    <td><span class="kinds">${p.kinds.map((k) => `<span class="sw ${catClass(k)}" title="${esc(lbl[k] || k)}"></span>`).join('')}</span></td>`;
}

function renderResults() {
  const r = co.result;
  const n = r.total;
  $('#co-count').innerHTML = `<b>${n}</b> patient${n === 1 ? '' : 's'} match${n === 1 ? 'es' : ''}${r.excluded_total ? ` · ${r.excluded_total} excluded` : ''}${co.selected.size ? ` · ${co.selected.size} ticked` : ''}`;
  const tick = !co.editing;
  let h = `<table class="list"><tr>${tick ? '<th><input type="checkbox" id="co-all" title="Tick this page"></th>' : ''}<th>Patient</th><th>Patient ID</th><th>Sex</th>${ages() ? '<th class="num">Age</th>' : ''}<th>Data</th><th>Why</th></tr>`;
  for (const p of r.items) {
    h += `<tr>${tick ? `<td><input type="checkbox" data-pick="${p.id}"${co.selected.has(p.id) ? ' checked' : ''} aria-label="Tick"></td>` : ''}${patientCells(p)}
      <td class="why">${p.reasons.map((x) => `<span>${esc(x)}</span>`).join('')}</td></tr>`;
  }
  if (!r.items.length) h += `<tr><td colspan="7" class="empty">No patient meets the criteria.</td></tr>`;
  h += '</table>';
  // what the criteria could not test (values in units that do not compare), and saved text
  // criteria that read otherwise now
  if (r.warnings?.length) h = `<div class="notice" role="status">${r.warnings.map((w) => `<span>${esc(w)}</span>`).join('<br>')}</div>${h}`;
  if (r.excluded_total) {
    h += `<details class="excluded"><summary>${r.excluded_total} patient${r.excluded_total === 1 ? '' : 's'} excluded</summary>
      <table class="list"><tr><th>Patient</th><th>Patient ID</th><th>Sex</th>${ages() ? '<th class="num">Age</th>' : ''}<th>Data</th><th>Excluded for</th></tr>
      ${r.excluded.map((p) => `<tr>${patientCells(p)}<td class="why">${p.reasons.map((x) => `<span>${esc(x)}</span>`).join('')}</td></tr>`).join('')}
      ${r.excluded_total > r.excluded.length ? `<tr><td colspan="6" class="muted">and ${r.excluded_total - r.excluded.length} more</td></tr>` : ''}</table></details>`;
  }
  h += `<div class="pager"><span class="muted">${n ? `${co.offset + 1}–${co.offset + r.items.length} of ${n}` : ''}</span><span><button id="co-prev"${co.offset ? '' : ' disabled'}>‹ Previous</button> <button id="co-next"${co.offset + r.items.length >= n ? ' disabled' : ''}>Next ›</button></span></div>`;
  $('#co-results').innerHTML = h;
  renderSave();
}

function renderSave() {
  const n = co.result?.total || 0;
  const k = co.selected.size;
  const which = k ? `the ${k} ticked` : `all ${n}`;
  if (co.editing) {
    const d = co.editing.definition;
    const manual = [d.added.length && `${d.added.length} added`, d.removed.length && `${d.removed.length} removed`].filter(Boolean).join(' and ');
    $('#co-save').innerHTML = `<button class="btn primary" id="co-save-changes">Save the changes</button>
      <a class="btn" href="${esc(href('cohorts', [String(co.editing.id)]))}">Cancel</a>
      <span class="muted small">These are the patients the criteria select${manual ? `; the patients ${manual} manually stay so` : ''}. Saved criteria apply when the members are refreshed: you will see who joins and who leaves first.</span>`;
    return;
  }
  if (co.parent) {
    $('#co-save').innerHTML = `<input id="co-name" placeholder="The subcohort's name" aria-label="Name">
      <button class="btn primary" id="co-create"${n ? '' : ' disabled'}>Save ${esc(which)} as a subcohort of “${esc(co.parent.name)}”</button>`;
    return;
  }
  const others = co.list.filter((c) => c.can_edit);
  $('#co-save').innerHTML = `<input id="co-name" placeholder="New cohort's name" aria-label="Name">
    <button class="btn primary" id="co-create"${n ? '' : ' disabled'}>Save ${esc(which)} as a new cohort</button>
    ${others.length ? `<span class="muted">or add them to</span><select id="co-target" aria-label="Cohort">${others.map((c) => `<option value="${c.id}">${esc(c.name)}</option>`).join('')}</select><button class="btn" id="co-add"${n ? '' : ' disabled'}>Add</button>` : ''}`;
}

const chosen = () => (co.selected.size ? [...co.selected] : co.result.ids);

function scheduleSearch() {
  clearTimeout(co.timer);
  co.offset = 0;
  co.timer = setTimeout(() => search().catch(fail), 350);
}

// ------------------------------------------------------------ the list of cohorts
/** The cohorts in list order, each subcohort under its parent: [cohort, depth]. */
function tree(list) {
  const ids = new Set(list.map((c) => c.id));
  const kids = new Map();
  const roots = [];
  for (const c of list) {
    if (c.parent && ids.has(c.parent)) kids.set(c.parent, [...(kids.get(c.parent) || []), c]);
    else roots.push(c);
  }
  const out = [];
  const walk = (c, depth) => { out.push([c, depth]); for (const k of kids.get(c.id) || []) walk(k, depth + 1); };
  roots.forEach((c) => walk(c, 0));
  return out;
}

/** The "New / Edit cohort" button: a new cohort, or a menu of what to do with the one shown. */
function newEdit() {
  const c = co.cohort || co.editing;
  if (!c) return '<a class="btn primary new" href="#/cohorts">New / Edit cohort</a>';
  const n = esc(c.name);
  const items = [
    '<a role="menuitem" href="#/cohorts">New cohort</a>',
    c.can_edit ? `<a role="menuitem" href="${esc(href('cohorts', [String(c.id), 'edit']))}">Edit “${n}”</a>` : '',
    '<hr>',
    `<button role="menuitem" data-act="clone">Clone “${n}”…</button>`,
    `<a role="menuitem" href="${esc(href('cohorts', ['new'], { within: c.id }))}">New subcohort of “${n}”</a>`,
    `<button role="menuitem" data-act="merge">Merge “${n}” with others…</button>`,
    deletable(c) ? `<hr><button role="menuitem" data-act="delete" class="danger">Delete “${n}”…</button>` : '',
  ];
  return `<div class="nebox"><button class="btn primary new" id="co-ne" aria-haspopup="menu" aria-expanded="false">New / Edit cohort ▾</button>
    <div class="nemenu hidden" role="menu">${items.join('')}</div></div>`;
}

/** Its owner deletes a cohort; any admin the demo's (and the demo with it). */
const deletable = (c) => c.is_owner || (c.demo && isAdmin());

async function loadList() {
  co.list = (await api('cohorts')).items;
  const cur = co.cohort?.id ?? co.editing?.id ?? co.parent?.id;
  $('#co-list').innerHTML = `${newEdit()}${tree(co.list).map(([c, d]) => `<a class="co${c.id === cur ? ' on' : ''}${d ? ` depth${Math.min(d, 3)}` : ''}" href="${esc(href('cohorts', [String(c.id)]))}">
      <b>${esc(c.name)}</b><small>${c.members} patient${c.members === 1 ? '' : 's'}${c.demo ? ' · the demo' : c.is_owner ? (c.shares?.length ? ' · shared' : '') : ` · from ${esc(c.owner_name || '?')}`}</small></a>`).join('')}
    ${co.list.length ? '' : '<p class="muted small">Cohorts you make or that others share with you appear here.</p>'}`;
}

async function references() {
  if (co.cats.length) return;
  const [pl, dbs] = await Promise.all([api('patients?limit=1'), api('databases')]);
  co.cats = pl.categories;
  co.dbs = dbs.items;
}

// ------------------------------------------------------------ a cohort's definition
/** Names joined: “A”, “B” or “C”. */
function joined(parts, last) {
  return parts.length < 2 ? parts.join('') : `${parts.slice(0, -1).join(', ')} ${last} ${parts[parts.length - 1]}`;
}

/** Where a cohort's members come from, with links to the cohorts it names. */
function sourceHtml(d) {
  const one = (s) => (s.deleted
    ? `“${esc(s.name)}” <span class="faint">(deleted: its last members are kept)</span>`
    : `“<a href="${esc(href('cohorts', [String(s.id)]))}">${esc(s.name)}</a>”`);
  const all = d.sources.map(one);
  return {
    within: `Subcohort of ${all[0]}`,
    union: `In ${joined(all, 'or')}`,
    intersection: `In ${joined(all, 'and')}`,
    difference: `In ${all[0]} but not in ${joined(all.slice(1), 'or')}`,
  }[d.op] || '';
}

/** Patient IDs as chips; a long list folds after the first ones. */
function idChips(ids) {
  const SHOWN = 40;
  const chip = (x) => `<span class="idchip">${esc(x)}</span>`;
  if (ids.length <= SHOWN) return ids.map(chip).join('');
  return `${ids.slice(0, SHOWN).map(chip).join('')}<button class="link" data-more>and ${ids.length - SHOWN} more</button><span class="idmore hidden">${ids.slice(SHOWN).map(chip).join('')}</span>`;
}

function definitionHtml(c) {
  const d = c.definition;
  const rows = [];
  if (d.origin) rows.push(['Made', esc(d.origin)]);
  if (d.op) rows.push(['From', `<span>${sourceHtml(d)}</span>`]);
  const crit = `${d.include ? `<span class="pill">${esc(d.include)}</span>` : ''}${d.exclude.map((x) => `<span class="pill out">never: ${esc(x)}</span>`).join('')}`;
  rows.push(['Criteria', crit || `<span class="faint">${d.op ? 'none: every member of its source' : 'none: its members are added manually'}</span>`]);
  if (d.added.length) rows.push(['Added manually', idChips(d.added)]);
  if (d.removed.length) rows.push(['Removed manually', idChips(d.removed)]);
  return `<dl class="codef">${rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('')}</dl>`;
}

// ------------------------------------------------------------ a cohort
async function showCohort(id, params) {
  $('#co-builder').classList.add('hidden');
  $('#co-one').classList.remove('hidden');
  co.editing = null;
  co.parent = null;
  const [cohort, version] = await Promise.all([api(`cohorts/${id}`), api('version', { background: true }).catch(() => null)]);
  co.cohort = cohort;
  co.version = version?.data ?? dataVersion();
  await loadList();
  const c = co.cohort;
  const parent = c.parent && co.list.find((x) => x.id === c.parent);
  $('#co-crumb').innerHTML = `<a class="crumb" href="#/cohorts">Cohorts</a><span class="sep">›</span>${parent ? `<a class="crumb" href="${esc(href('cohorts', [String(parent.id)]))}">${esc(parent.name)}</a><span class="sep">›</span>` : ''}<span class="crumb now">${esc(c.name)}</span>`;
  // in Privacy Mode the server exports through its gate
  const ex = (what, title, label) => `<a class="btn" href="${esc(`${API}cohorts/${c.id}/export${what ? `?${qs({ what })}` : ''}`)}" download title="${title}">${label}</a>`;
  $('#co-head').innerHTML = `<div class="cohead"><h2>${esc(c.name)}</h2>
      <div class="sub">${c.members} patient${c.members === 1 ? '' : 's'} · ${c.demo ? 'the demo, for every user' : c.is_owner ? 'yours' : `${esc(c.owner_name)}'s, shared with you${c.can_edit ? ' to edit' : ' to read'}`} · made ${fmtDate(c.created_at)}${c.refreshed_at ? ` · members found ${fmtTime(c.refreshed_at)}` : ''}${Object.entries(c.source_versions || {}).filter(([k]) => k !== 'catalog').map(([k, v]) => ` · <span title="The version of the connected database ${esc(k)} when its members were found">${esc(k)} ${esc(v)}</span>`).join('')}</div>
      ${c.description ? `<p>${esc(c.description)}</p>` : ''}</div>
    <div class="ftools coacts">
      ${c.can_edit ? `<a class="btn" href="${esc(href('cohorts', [String(c.id), 'edit']))}">Edit</a>` : ''}
      <button class="btn" data-act="clone">Clone…</button>
      <a class="btn" href="${esc(href('cohorts', ['new'], { within: c.id }))}">Subcohort…</a>
      <button class="btn" data-act="merge">Merge…</button>
      ${c.is_owner && !c.demo ? '<button class="btn" data-act="share">Share…</button>' : ''}
      <span class="spacer"></span>
      ${deletable(c) ? '<button class="btn danger" data-act="delete">Delete…</button>' : ''}
    </div>
    ${definitionHtml(c)}
    ${newData(c) ? `<div class="notice" role="status"><span><b>New data since the members were found</b> (${fmtTime(c.refreshed_at)}): the definition may select new patients.</span>
      ${c.can_edit ? '<button class="btn" data-act="refresh">See who would join…</button>' : ''}</div>` : ''}
    <div class="ftools">
      ${c.can_edit ? '<button class="btn" data-act="refresh">Refresh the members…</button>' : ''}
      <a class="btn" href="${esc(href('patients', [], { cohort: c.id }))}">Open in Patient Catalog</a>
      <a class="btn" href="${esc(href('actions', [], { cohort: c.id }))}">Run actions…</a>
      <span class="exports"><span class="muted">Export${privacy.on ? ' (Privacy Mode)' : ''}</span>
        ${ex('', 'The members: one row per patient', 'Members (CSV)')}
        ${ex('criteria', 'The definition: criteria, source, patients added and removed manually (JSON)', 'Definition (JSON)')}
        ${ex('data', "Every database's rows about the members, a cell per line: patient, database, line, column, value", "Members' data (CSV)")}
        ${ex('xlsx', 'Members, definition, and a sheet per database', 'Excel')}</span>
    </div>
    <div id="co-panel"></div>`;
  co.moffset = 0;
  co.found = null;
  $('#co-find').value = '';
  await loadMembers();
  $('#co-hist').open = false;
  $('#co-hist-body').innerHTML = '';
  if (params.get('refresh') && c.can_edit) {
    setParams({ refresh: '' });
    await refresh(true);
  }
}

async function loadMembers() {
  const c = co.cohort;
  const m = await api(`cohorts/${c.id}/members?${qs({ limit: PAGE, offset: co.moffset })}`);
  const how = (p) => (p.how === 'manual' ? 'added manually' : c.source ? 'by the definition' : 'by the criteria');
  let h = `<table class="list"><tr><th>Patient</th><th>Patient ID</th><th>Sex</th>${ages() ? '<th class="num">Age</th>' : ''}<th>Data</th><th>Added</th>${c.can_edit ? '<th></th>' : ''}</tr>`;
  for (const p of m.items) {
    h += `<tr>${patientCells(p)}<td>${how(p)} <span class="faint">${fmtDate(p.added_at)}</span></td>
      ${c.can_edit ? `<td><button class="icon x" data-out="${p.id}" title="Remove from the cohort (recorded in its definition)" aria-label="Remove">✕</button></td>` : ''}</tr>`;
  }
  if (!m.items.length) h += `<tr><td colspan="8" class="empty">No member.</td></tr>`;
  h += '</table>';
  h += `<div class="pager"><span class="muted">${m.total ? `${co.moffset + 1}–${co.moffset + m.items.length} of ${m.total}` : ''}</span><span><button id="co-mprev"${co.moffset ? '' : ' disabled'}>‹ Previous</button> <button id="co-mnext"${co.moffset + m.items.length >= m.total ? ' disabled' : ''}>Next ›</button></span></div>`;
  $('#co-members').innerHTML = h;
  $('#co-addbox').classList.toggle('hidden', !c.can_edit);
  if (c.can_edit) {
    $('#co-find').placeholder = phi()
      ? 'Add patients: search by name, MRN, patient ID, date of birth, encounter…'
      : 'Add patients: search by pseudonym (MO-…), encounter, unit…';
    renderFound();
  }
}

// ------------------------------------------------------------ adding patients by search
const FOUND = 10;

/** The Patient Catalog's search, its matches marked when they are members already. */
async function find() {
  const q = $('#co-find').value.trim();
  const seq = ++co.fseq;
  if (!q) { co.found = null; renderFound(); return; }
  const r = await api(`patients?${qs({ q, mark: co.cohort.id, limit: FOUND })}`);
  if (seq !== co.fseq) return;
  co.found = { q, total: r.total, items: r.items, ticked: new Set() };
  renderFound();
}

function renderFound() {
  const f = co.found;
  if (!f) { $('#co-found').innerHTML = ''; return; }
  const fresh = f.items.filter((p) => !p.member);
  let h = '<table class="list"><tr><th></th><th>Patient</th><th>Patient ID</th><th>Sex</th>';
  h += `${ages() ? '<th class="num">Age</th>' : ''}<th>Data</th></tr>`;
  for (const p of f.items) {
    h += `<tr><td>${p.member ? '<span class="faint small">member</span>' : `<input type="checkbox" data-addpick="${p.id}"${f.ticked.has(p.id) ? ' checked' : ''} aria-label="Tick">`}</td>${patientCells(p)}</tr>`;
  }
  if (!f.items.length) h += `<tr><td colspan="6" class="empty">No patient matches “${esc(f.q)}”.</td></tr>`;
  h += '</table>';
  const more = f.total > f.items.length ? `the first ${f.items.length} of ${f.total} shown` : `${f.total} found`;
  h += `<div class="ftools"><span class="muted small">${more}</span><span class="spacer"></span>
    <button class="btn primary" id="co-add-ticked"${f.ticked.size ? '' : ' disabled'}>Add the ${f.ticked.size || ''} ticked</button>
    <button class="btn" id="co-add-all"${f.total && (fresh.length || f.total > f.items.length) ? '' : ' disabled'}>Add all ${f.total} found</button>
    <button class="btn" id="co-find-clear">Clear</button></div>`;
  $('#co-found').innerHTML = h;
}

async function addFound(all) {
  const c = co.cohort;
  const f = co.found;
  const json = all ? { add_search: { q: f.q } } : { add: [...f.ticked] };
  const after = await api(`cohorts/${c.id}`, { method: 'PATCH', json });
  const added = after.members - c.members;
  toast(`${added} patient${added === 1 ? '' : 's'} added to “${after.name}”`);
  $('#co-find').value = '';
  co.found = null;
  await showCohort(c.id, new URLSearchParams());
}

// ------------------------------------------------------------ panels: refresh, clone, merge, share, delete
const panel = (html) => { $('#co-panel').innerHTML = `<div class="card panel">${html}</div>`; $('#co-panel').scrollIntoView({ block: 'nearest' }); };
const closeBtn = '<button class="btn" id="co-panel-close">Cancel</button>';

/** Whether the catalog's data changed since the cohort's members were found. */
function newData(c) {
  const now = co.version;
  const then = c.source_versions?.catalog;
  return c.refreshed_at && now != null && then != null && String(now) !== String(then);
}

const plural = (n, one, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Who the definition selects now that is not a member, to add (all, or those ticked), and
 * the members it no longer selects, who leave only with every change applied. */
async function refresh(quiet = false) {
  const c = co.cohort;
  const r = await api(`cohorts/${c.id}/refresh`, { method: 'POST', json: { dry_run: true } });
  if (quiet && !r.added.length && !r.removed.length) return;
  co.refresh = { r, chosen: new Set(r.added) };
  const nj = r.added.length;
  const nl = r.removed.length;
  if (!nj && !nl) {
    panel(`<b>The members are up to date</b>
      <p class="muted">The definition selects exactly the current members: nobody new to add, nobody to take out.</p>
      <div class="ftools"><button class="btn" id="co-panel-close">Close</button></div>`);
    return;
  }
  const table = (rows, total, pick) => `<div class="table-wrap short"><table class="list">
      <tr>${pick ? `<th class="tick"><input type="checkbox" id="co-join-all" checked aria-label="All the new patients"></th>` : ''}<th>Patient</th><th>Patient ID</th><th>Sex</th>${ages() ? '<th class="num">Age</th>' : ''}<th>Data</th></tr>
      ${rows.map((p) => `<tr>${pick ? `<td class="tick"><input type="checkbox" data-join="${p.id}" checked aria-label="Add this patient"></td>` : ''}${patientCells(p)}</tr>`).join('')}
      ${total > rows.length ? `<tr><td colspan="7" class="muted">and ${total - rows.length} more${pick ? ', added with all the others' : ''}</td></tr>` : ''}</table></div>`;
  panel(`<b>What the definition selects now</b>
    <p class="muted small">Members added manually stay; members removed manually stay out.</p>
    ${nj ? `<h4 class="refh">${plural(nj, 'new patient')} would join</h4>${table(r.joining, nj, true)}` : '<p>No new patient would join.</p>'}
    ${nl ? `<details class="refleave"><summary>${plural(nl, 'member')} the definition no longer selects would leave</summary>${table(r.leaving, nl, false)}</details>` : ''}
    <div class="ftools">
      ${nj ? '<button class="btn primary" id="co-add-new"></button>' : ''}
      ${nl ? `<button class="btn" id="co-apply" title="Add the new patients ticked and take out the members the definition no longer selects">Apply these changes</button>` : ''}
      ${closeBtn}</div>`);
  joinButton();
}

/** The add button as the ticks are: all the new patients, or those chosen. */
function joinButton() {
  const { r, chosen } = co.refresh;
  const n = chosen.size;
  const apply = $('#co-apply');
  if (apply) apply.textContent = `Apply these changes (${n} join, ${r.removed.length} leave)`;
  const b = $('#co-add-new');
  if (!b) return;
  b.disabled = !n;
  b.textContent = n === r.added.length ? `Add the ${plural(n, 'new patient')}` : `Add ${plural(n, 'chosen patient')}`;
  const all = $('#co-join-all');
  if (all) {
    all.checked = n === r.added.length;
    all.indeterminate = n > 0 && n < r.added.length;
  }
}

function clonePanel() {
  const c = co.cohort;
  panel(`<b>Clone “${esc(c.name)}”</b>
    <p class="muted small">A cohort of your own with the same definition and members${c.source ? ', following the same cohorts when refreshed' : ''}. “${esc(c.name)}” does not change.</p>
    <div class="ftools"><input id="co-clone-name" value="${esc(`${c.name} (copy)`)}" aria-label="Name"><button class="btn primary" id="co-clone-go">Clone</button>${closeBtn}</div>`);
  $('#co-clone-name').select();
}

function mergePanel() {
  const c = co.cohort;
  const others = co.list.filter((x) => x.id !== c.id);
  if (!others.length) {
    panel(`<b>Merge “${esc(c.name)}” with other cohorts</b><p class="muted small">There is no other cohort to merge it with.</p><div class="ftools">${closeBtn}</div>`);
    return;
  }
  const n = esc(c.name);
  const op = (v, label, what, on) => `<label><input type="radio" name="co-mop" value="${v}"${on ? ' checked' : ''}> <b>${label}</b> <span class="muted">${what}</span></label>`;
  panel(`<b>Merge “${n}” with other cohorts</b>
    <div class="mops">
      ${op('union', 'Union', `patients in “${n}” or in any of the others`, true)}
      ${op('intersection', 'Intersection', `patients in “${n}” and in all of the others`)}
      ${op('difference', 'Difference', `patients in “${n}” and in none of the others`)}
    </div>
    <div class="mlist">${others.map((x) => `<label><input type="checkbox" data-mpick="${x.id}"> ${esc(x.name)} <span class="faint">${x.members}</span></label>`).join('')}</div>
    <p class="muted small">A new cohort, with “${n}” and the others left as they are. It follows them when it is refreshed.</p>
    <div class="ftools"><input id="co-merge-name" placeholder="The new cohort's name" aria-label="Name"><button class="btn primary" id="co-merge-go">Make the cohort</button>${closeBtn}</div>`);
}

async function share() {
  const c = co.cohort;
  if (!co.people.length) co.people = (await api('cohorts/people')).items;
  const has = Object.fromEntries((c.shares || []).map((s) => [s.user_id, s]));
  panel(`<b>Share “${esc(c.name)}”</b>
    ${co.people.length ? `<table class="grid"><tr><th>User</th><th>May see it</th><th>May edit it</th></tr>${co.people.map((p) => `<tr><td>${esc(p.display_name || p.username)} <span class="faint">${esc(p.username)}</span></td>
      <td><input type="checkbox" data-see="${p.id}"${has[p.id] ? ' checked' : ''} aria-label="May see"></td><td><input type="checkbox" data-edit="${p.id}"${has[p.id]?.can_edit ? ' checked' : ''} aria-label="May edit"></td></tr>`).join('')}</table>`
    : '<p class="muted">There is no other user.</p>'}
    <p class="muted small">Users see the members as their own permissions allow: identifiers only with the PHI permission.</p>
    <div class="ftools"><button class="btn primary" id="co-share-save">Save</button>${closeBtn}</div>`);
}

async function deletePanel() {
  const c = co.cohort;
  if (c.demo) {
    // the demo's cohort: the demo goes with it
    const r = await api('demo/removal');
    const n = (k, one) => `${r[k]} ${one}${r[k] === 1 ? '' : 's'}`;
    panel(`<b>Delete the demo “${esc(c.name)}”?</b>
      <p>This removes the demo's invented patients from the catalog, with everything about them: ${n('patients', 'patient')}, ${n('encounters', 'encounter')}, ${n('bed_stays', 'bed stay')}, ${n('ehr_items', 'item')} (results, notes, files) and ${n('recordings', 'recording')} with their results and indexes; its OMOP database (demo_omop) is disconnected, and this cohort is deleted.${r.kept_recordings + r.kept_encounters ? ` What other sources also hold stays (${r.kept_recordings} recording(s), ${r.kept_encounters} encounter(s)).` : ''} Nothing else in the catalog changes, and the demo's files stay where they are.</p>
      <p class="muted small">This cannot be undone; an admin can load the demo again with <code>medorama demo add</code>.</p>
      <div class="ftools"><button class="btn danger" id="co-delete-go">Delete the demo</button>${closeBtn}</div>`);
    return;
  }
  const made = co.list.filter((x) => x.definition.sources.some((s) => s.id === c.id && !s.deleted));
  panel(`<b>Delete “${esc(c.name)}”?</b>
    <p>${c.members === 1 ? 'The patient in it stays' : `The ${c.members} patients in it stay`} in the catalog.${made.length ? ` ${made.length === 1 ? 'One cohort is' : `${made.length} cohorts are`} made from it (${made.map((x) => `“${esc(x.name)}”`).join(', ')}): ${made.length === 1 ? 'it keeps' : 'they keep'} its current members.` : ''} This cannot be undone.</p>
    <div class="ftools"><button class="btn danger" id="co-delete-go">Delete</button>${closeBtn}</div>`);
}

/** The actions of the cohort shown, from its page or the New / Edit menu. */
async function act(what) {
  if (!co.cohort) return;
  if (what === 'clone') clonePanel();
  else if (what === 'merge') mergePanel();
  else if (what === 'share') await share();
  else if (what === 'delete') await deletePanel();
  else if (what === 'refresh') await refresh();
}

async function loadHistory() {
  const r = await api(`cohorts/${co.cohort.id}/history`);
  $('#co-hist-body').innerHTML = r.items.length
    ? `<table class="list"><tr><th>When</th><th>Who</th><th>What</th></tr>${r.items.map((e) => `<tr><td class="nowrap">${fmtTime(e.time)}</td><td>${esc(e.user || 'command line')}</td><td class="what">${esc(e.what)}</td></tr>`).join('')}</table>`
    : '<p class="muted small">No change recorded.</p>';
}

// ------------------------------------------------------------ the builder: a new cohort, a subcohort, an edit
async function showBuilder({ editing = null, parent = null } = {}) {
  $('#co-one').classList.add('hidden');
  $('#co-builder').classList.remove('hidden');
  co.cohort = null;
  co.editing = editing;
  co.parent = parent;
  const mode = editing ? `edit:${editing.id}` : parent ? `sub:${parent.id}` : 'new';
  if (editing) {
    const g = toGroups(editing.criteria);
    co.inc = g || [];
    co.kept = g ? null : editing.criteria.include;
    // an exclusion that is a group (made through the API) is kept as it is, never dropped
    co.exc = (editing.criteria.exclude || []).map((e) => (e.op ? { kind: 'group', expr: e } : { ...e, mode: e.kind === 'column' && e.text ? 'text' : undefined }));
  } else if (co.mode !== mode || (!co.inc.length && !co.exc.length)) {
    // a new search starts empty; coming back to the same one keeps it
    co.inc = [[blank('expression')]];
    co.exc = [];
    co.kept = null;
  }
  co.mode = mode;
  co.selected = new Set();
  co.offset = 0;
  await loadList();
  const crumb = (c) => `<a class="crumb" href="${esc(href('cohorts', [String(c.id)]))}">${esc(c.name)}</a><span class="sep">›</span>`;
  $('#co-crumb').innerHTML = `<a class="crumb" href="#/cohorts">Cohorts</a><span class="sep">›</span>${
    editing ? `${crumb(editing)}<span class="crumb now">Edit</span>` : parent ? `${crumb(parent)}<span class="crumb now">New subcohort</span>` : '<span class="crumb now">New cohort</span>'}`;
  if (editing) {
    $('#co-bhead').innerHTML = `<div class="bfields">
        <label for="co-ename">Name</label><input id="co-ename" value="${esc(editing.name)}">
        <label for="co-edesc">Description</label><input id="co-edesc" value="${esc(editing.description)}" placeholder="Optional">
        ${editing.definition.op ? `<span class="lbl">From</span><span>${sourceHtml(editing.definition)} <span class="muted small">(set when it was made; the criteria below choose among those patients)</span></span>` : ''}
      </div>
      <p class="muted small">Members are changed on the cohort's page: added by search, removed with ✕. Both are recorded in its definition.</p>`;
  } else if (parent) {
    $('#co-bhead').innerHTML = `<div class="within">A subcohort of “<a href="${esc(href('cohorts', [String(parent.id)]))}">${esc(parent.name)}</a>” (${parent.members} patient${parent.members === 1 ? '' : 's'}): the conditions below choose among its members. It follows “${esc(parent.name)}” when it is refreshed.</div>`;
  } else {
    $('#co-bhead').innerHTML = '';
  }
  renderCriteria();
  await search();
}

export default {
  async show(r) {
    await references();
    const id = r.parts[0];
    if (!id) return showBuilder();
    if (id === 'new') {
      const within = r.params.get('within');
      return showBuilder({ parent: within ? await api(`cohorts/${within}`) : null });
    }
    if (r.parts[1] === 'edit') return showBuilder({ editing: await api(`cohorts/${id}`) });
    return showCohort(+id, r.params);
  },
  wire() {
    const builder = $('#co-builder');
    const target = (t) => { const g = t.dataset.g; return at(g === 'x' ? 'x' : +g, +t.dataset.i); };
    builder.addEventListener('input', (e) => {
      const t = e.target.closest('input[data-i]');
      if (!t || t.type === 'checkbox') return;
      target(t)[t.dataset.k] = t.value;
      scheduleSearch();
    });
    builder.addEventListener('change', (e) => {
      const t = e.target.closest('select[data-i], input[type="checkbox"][data-i]');
      if (!t) return;
      const k = target(t);
      const key = t.dataset.k;
      const g = t.dataset.g === 'x' ? 'x' : +t.dataset.g;
      const i = +t.dataset.i;
      if (key === 'kind') {
        const fresh = { ...blank(t.value), not: k.not };
        if (g === 'x') co.exc[i] = fresh; else co.inc[g][i] = fresh;
      } else if (key === 'not') k.not = t.value === '1';
      else if (key === 'latest') k.latest = t.checked;
      else if (key === 'import') { k.import = t.value ? +t.value : undefined; if (k.kind === 'column') { k.name = ''; if (k.import && !co.columns[k.import]) loadColumns(k.import); } }
      else k[key] = t.value;
      if (['kind', 'cmp', 'import', 'name', 'mode'].includes(key)) renderCriteria();
      scheduleSearch();
    });
    builder.addEventListener('click', (e) => {
      const b = e.target.closest('button');
      if (!b) return;
      if (b.dataset.del) {
        const [g, i] = b.dataset.del.split(':');
        if (g === 'x') co.exc.splice(+i, 1);
        else { co.inc[+g].splice(+i, 1); if (!co.inc[+g].length) co.inc.splice(+g, 1); }
      } else if (b.dataset.addto !== undefined) co.inc[+b.dataset.addto].push(blank('measurement'));
      else if (b.dataset.addexpr !== undefined) co.inc[+b.dataset.addexpr].push(blank('expression'));
      else if (b.id === 'co-addgroup') co.inc.push([blank('measurement')]);
      else if (b.id === 'co-addexpr') co.inc.push([blank('expression')]);
      else if (b.id === 'co-addexc') co.exc.push(blank('diagnosis'));
      else if (b.id === 'co-unkeep') { co.kept = null; co.inc = [[blank('expression')]]; }
      else return;
      renderCriteria();
      // a keyboard user goes on where the change is: the new condition, or the group's tools
      const groups = document.querySelectorAll('#co-criteria .cgroup');
      if (b.dataset.addto !== undefined || b.dataset.addexpr !== undefined) focusCondition([...groups[+(b.dataset.addto ?? b.dataset.addexpr)].querySelectorAll('.crit')].pop());
      else if (['co-addgroup', 'co-addexpr', 'co-unkeep'].includes(b.id)) focusCondition([...(groups[groups.length - 1]?.querySelectorAll('.crit') || [])].pop());
      else if (b.id === 'co-addexc') focusCondition([...document.querySelectorAll('#co-exclude .crit')].pop());
      if (focusLost()) $('#co-addgroup, #co-addexc')?.focus();
      scheduleSearch();
    });
    $('#co-results').addEventListener('change', (e) => {
      if (e.target.id === 'co-all') {
        for (const p of co.result.items) { if (e.target.checked) co.selected.add(p.id); else co.selected.delete(p.id); }
        renderResults();
        return;
      }
      const t = e.target.closest('[data-pick]');
      if (!t) return;
      if (t.checked) co.selected.add(+t.dataset.pick); else co.selected.delete(+t.dataset.pick);
      renderResults();
    });
    $('#co-results').addEventListener('click', (e) => {
      if (e.target.id === 'co-prev') { co.offset = Math.max(0, co.offset - PAGE); search().catch(fail); }
      if (e.target.id === 'co-next') { co.offset += PAGE; search().catch(fail); }
    });
    $('#co-save').addEventListener('click', async (e) => {
      try {
        if (e.target.id === 'co-create') {
          const name = $('#co-name').value.trim();
          if (!name) { $('#co-name').focus(); return toast('Give the cohort a name', true); }
          const source = co.parent ? { op: 'within', of: [co.parent.id] } : undefined;
          const c = await api('cohorts', { method: 'POST', json: { name, criteria: query(), source, patients: chosen() } });
          toast(`Cohort “${c.name}”: ${c.members} patient${c.members === 1 ? '' : 's'}`);
          co.inc = [];
          co.exc = [];
          go(href('cohorts', [String(c.id)])).catch(fail);
        } else if (e.target.id === 'co-add') {
          const id = $('#co-target').value;
          const before = co.list.find((x) => String(x.id) === id);
          const c = await api(`cohorts/${id}`, { method: 'PATCH', json: { add: chosen() } });
          const added = c.members - (before?.members ?? 0);
          toast(`${added} patient${added === 1 ? '' : 's'} added to “${c.name}”`);
          await loadList();
        } else if (e.target.id === 'co-save-changes') {
          const name = $('#co-ename').value.trim();
          if (!name) { $('#co-ename').focus(); return toast('A cohort needs a name', true); }
          const id = co.editing.id;
          await api(`cohorts/${id}`, { method: 'PATCH', json: { name, description: $('#co-edesc').value, criteria: query() } });
          toast('Saved');
          go(href('cohorts', [String(id)], { refresh: 1 })).catch(fail);
        }
      } catch (err) { fail(err); }
    });
    // the New / Edit menu
    $('#co-list').addEventListener('click', (e) => {
      const ne = e.target.closest('#co-ne');
      const menu = $('#co-list .nemenu');
      if (ne && menu) {
        const open = menu.classList.toggle('hidden') === false;
        ne.setAttribute('aria-expanded', String(open));
        return;
      }
      const item = e.target.closest('.nemenu [data-act]');
      if (menu) menu.classList.add('hidden');
      if (item) act(item.dataset.act).catch(fail);
    });
    document.addEventListener('click', (e) => {
      if (!e.target.closest('.nebox')) $('#co-list .nemenu')?.classList.add('hidden');
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') $('#co-list .nemenu')?.classList.add('hidden');
    });
    // a cohort's page
    $('#co-one').addEventListener('click', async (e) => {
      const c = co.cohort;
      try {
        const id = e.target.id;
        const actBtn = e.target.closest('[data-act]');
        if (actBtn) await act(actBtn.dataset.act);
        else if (id === 'co-apply') {
          // the changes the panel shows (the new patients ticked, the members who leave), as the
          // definition still makes them: not a set found again since
          const { r: seen, chosen } = co.refresh;
          const r = await api(`cohorts/${c.id}/refresh`, { method: 'POST', json: { dry_run: false, join: [...chosen], remove: seen.removed } });
          const since = (r.not_applied || []).length + (r.not_chosen || 0);
          toast(`${r.added.length} joined, ${r.removed.length} left${since ? `; ${plural(since, 'change')} since the preview not applied: refresh again to see them` : ''}`);
          await showCohort(c.id, new URLSearchParams());
        } else if (id === 'co-add-new') {
          const join = [...co.refresh.chosen];
          const r = await api(`cohorts/${c.id}/refresh`, { method: 'POST', json: { dry_run: false, join, leave: false } });
          toast(`${plural(r.added.length, 'new patient')} added to “${c.name}”`);
          await showCohort(c.id, new URLSearchParams());
        } else if (id === 'co-panel-close') $('#co-panel').innerHTML = '';
        else if (id === 'co-share-save') {
          const shares = co.people.filter((p) => $(`[data-see="${p.id}"]`).checked || $(`[data-edit="${p.id}"]`).checked)
            .map((p) => ({ user_id: p.id, can_edit: $(`[data-edit="${p.id}"]`).checked }));
          await api(`cohorts/${c.id}`, { method: 'PATCH', json: { shares } });
          toast(shares.length ? `Shared with ${shares.length} user${shares.length === 1 ? '' : 's'}` : 'Not shared');
          await showCohort(c.id, new URLSearchParams());
        } else if (id === 'co-clone-go') {
          const name = $('#co-clone-name').value.trim();
          if (!name) { $('#co-clone-name').focus(); return toast('Give the clone a name', true); }
          const n = await api(`cohorts/${c.id}/clone`, { method: 'POST', json: { name } });
          toast(`Cohort “${n.name}”: ${n.members} patient${n.members === 1 ? '' : 's'}`);
          go(href('cohorts', [String(n.id)])).catch(fail);
        } else if (id === 'co-merge-go') {
          const of = [c.id, ...[...document.querySelectorAll('[data-mpick]:checked')].map((x) => +x.dataset.mpick)];
          const name = $('#co-merge-name').value.trim();
          if (of.length < 2) return toast('Tick at least one other cohort', true);
          if (!name) { $('#co-merge-name').focus(); return toast('Give the new cohort a name', true); }
          const op = $('input[name="co-mop"]:checked').value;
          const n = await api('cohorts', { method: 'POST', json: { name, source: { op, of } } });
          toast(`Cohort “${n.name}”: ${n.members} patient${n.members === 1 ? '' : 's'}`);
          go(href('cohorts', [String(n.id)])).catch(fail);
        } else if (id === 'co-delete-go') {
          await api(`cohorts/${c.id}`, { method: 'DELETE' });
          toast(c.demo ? 'The demo was removed from the catalog' : `Cohort “${c.name}” deleted`);
          go('#/cohorts').catch(fail);
        } else if (e.target.closest('[data-out]')) {
          const p = +e.target.closest('[data-out]').dataset.out;
          await api(`cohorts/${c.id}`, { method: 'PATCH', json: { remove: [p] } });
          await showCohort(c.id, new URLSearchParams());
        } else if (e.target.closest('[data-more]')) {
          const b = e.target.closest('[data-more]');
          b.nextElementSibling.classList.remove('hidden');
          b.remove();
        } else if (id === 'co-add-ticked') await addFound(false);
        else if (id === 'co-add-all') await addFound(true);
        else if (id === 'co-find-clear') { $('#co-find').value = ''; co.found = null; renderFound(); }
        else if (id === 'co-mprev') { co.moffset = Math.max(0, co.moffset - PAGE); await loadMembers(); }
        else if (id === 'co-mnext') { co.moffset += PAGE; await loadMembers(); }
      } catch (err) { fail(err); }
    });
    $('#co-one').addEventListener('change', (e) => {
      // the refresh panel's ticks: whom of the new patients to add
      const j = e.target.closest('[data-join], #co-join-all');
      if (j && co.refresh) {
        const { r, chosen } = co.refresh;
        if (j.id === 'co-join-all') {
          chosen.clear();
          if (j.checked) r.added.forEach((id) => chosen.add(id));
          document.querySelectorAll('#co-panel [data-join]').forEach((x) => { x.checked = j.checked; });
        } else if (j.checked) chosen.add(+j.dataset.join);
        else chosen.delete(+j.dataset.join);
        joinButton();
        return;
      }
      const t = e.target.closest('[data-addpick]');
      if (!t || !co.found) return;
      if (t.checked) co.found.ticked.add(+t.dataset.addpick); else co.found.ticked.delete(+t.dataset.addpick);
      renderFound();
    });
    $('#co-find').addEventListener('input', () => {
      clearTimeout(co.ftimer);
      co.ftimer = setTimeout(() => find().catch(fail), 250);
    });
    $('#co-hist').addEventListener('toggle', () => {
      if ($('#co-hist').open && co.cohort) loadHistory().catch(fail);
    });
  },
};
