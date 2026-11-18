// One patient: identity bar, encounters and conditions, the journey, the report and the detail
// of the selected entry.  Its address holds the selection (#/patient/12?item=…&enc=…&q=…), so
// that back and forward, reloading and sharing keep it.

import { $, api, daysBetween, esc, fail, fmtDate, fmtHM, fmtShort, fmtYear, privacy, qs, store } from '../core.js';
import { focusLost, go, href, navigation, setParams, superseded } from '../router.js';
import { drawJourney, hitAt } from '../patient/journey.js';
import { context, renderList } from '../patient/report.js';
import { renderDetail } from '../patient/detail.js';

const pp = {
  id: null, sum: null, jour: null, entries: [], counts: [], terms: [], fuzzy: [], trends: null, labels: {},
  mode: 'timeline', q: '', enc: null, zoom: 1, selected: null, hidden: new Set(store.get('hidden', [])), hits: [],
  qtimer: null, ftimer: null, detailSeq: 0, reportSeq: 0, findSeq: 0, suggest: [], sugIdx: -1,
  // details read lately (the one shown, and those fetched ahead: the previous and next of its kind)
  details: new Map(),
};

/** How long a detail read ahead stays good, and how many are kept. */
const DETAIL_TTL = 30000;
const DETAILS_KEPT = 24;

/** `pp.trends` for this patient, unless another patient's page opened while it was read. */
async function loadTrends() {
  const id = pp.id;
  const t = await api(`patients/${id}/trends`);
  if (pp.id === id) pp.trends = t;
}

const encOf = (id) => pp.sum?.encounters.find((e) => e.id === id);
const entryOf = (key) => pp.entries.find((x) => x.key === key);

function name(sum) {
  if (sum.phi && (sum.phi.last_name || sum.phi.first_name)) return [sum.phi.last_name, sum.phi.first_name].filter(Boolean).join(', ');
  return sum.patient_id || sum.pseudonym;
}

// ------------------------------------------------------------ identity and summary
function renderBar() {
  const s = pp.sum;
  // the search box's chip: the name, or in Privacy Mode the pseudonym (more useful than a placeholder)
  $('#pp-chip').innerHTML = `${esc(privacy.on ? s.pseudonym || s.patient_id : name(s))} <button data-close title="Close this patient">✕</button>`;
  const meta = [];
  const sexAge = [s.sex, s.age !== undefined ? `${s.age} y` : ''].filter(Boolean).join(' · ');
  if (sexAge) meta.push([sexAge, s.phi?.birth_date ? `born ${s.phi.birth_date}` : s.age !== undefined ? 'sex · age' : 'sex']);
  // in Privacy Mode the patient is named by their pseudonym; otherwise both, for talking with
  // those who only see pseudonyms
  if (privacy.on) meta.push([s.pseudonym || s.patient_id, 'pseudonym']);
  else if (s.phi && s.patient_id) meta.push([s.patient_id, 'patient ID'], [s.pseudonym, 'pseudonym']);
  else meta.push([s.pseudonym, 'pseudonym']);
  const mrns = s.phi?.mrns || [];
  if (mrns.length) meta.push([mrns.map((m) => m[1]).join(' · '), mrns.some((m) => m[0]) ? `MRN · ${mrns.map((m) => m[0] || 'default').join(', ')}` : 'MRN']);
  // (the visits are counted once, in the column beside; the allergies are its first section)
  if (s.first) meta.push([`${fmtDate(s.first)} – ${fmtDate(s.last)}`, 'first – latest data']);
  // where the data come from, apart from who the patient is: at the row's end, with a
  // connected database that did not answer (what it holds is missing from the page)
  const missing = (s.unavailable || []).length
    ? `<span class="pill" title="This patient's data in ${esc(s.unavailable.join(', '))} could not be read now: the page shows the other sources">${esc(s.unavailable.join(', '))}: not available</span>`
    : '';
  const dbs = (s.sources || []).length
    ? `<div>${esc(s.sources.join(', '))}<small>${s.sources.length === 1 ? 'database' : 'databases'}</small></div>`
    : '';
  $('#pp-bar').innerHTML = `<div class="nm">${esc(name(s))}</div><div class="meta">${meta.map(([a, b]) => `<div>${esc(a)}<small>${esc(b)}</small></div>`).join('')}</div>`
    + (dbs || missing ? `<div class="meta src">${missing}${dbs}</div>` : '');
}

/** The left column: the allergies (when a database of the patient can hold them), the visits,
 * the medications and the conditions; a section with nothing is left out, and the column
 * itself when every one is (an ECG archive's patient: the journey and the report get its room). */
function renderInfo() {
  const s = pp.sum;
  let h = '';
  if (s.allergies !== null) {
    h += s.allergies.length
      ? `<div class="allergies"><div class="label">Allergies <span>${s.allergies.length}</span></div>${s.allergies.map((a) => `<div class="allergy"><span aria-hidden="true">▲</span> ${esc(a)}</div>`).join('')}</div>`
      : '<div><div class="label">Allergies</div><p class="muted small">No allergy recorded</p></div>';
  }
  if (s.encounters.length) h += `<div><div class="label">Visits <span>${s.encounters.length}</span></div>`;
  // each database's visits and stays, latest first; one chosen shows only its data
  for (const e of s.encounters) {
    const { long, days: n } = stayDays(e);
    const span = long
      ? `${fmtShort(e.admit)} – ${fmtShort(e.discharge)}${fmtYear(e.discharge) !== fmtYear(e.admit) ? '' : `, ${fmtYear(e.admit)}`}`
      : fmtDate(e.admit);
    const days = long ? ` · ${n} day${n > 1 ? 's' : ''}` : '';
    const what = `${long ? 'Stay' : 'Visit'}${e.unit ? ` · ${esc(e.unit)}` : ''}${e.icu ? ' · ICU' : ''}`;
    h += `<div class="enc${pp.enc === e.id ? ' on' : ''}" data-enc="${e.id}" tabindex="0" role="button" aria-pressed="${pp.enc === e.id}" title="In the database ${esc(e.source)}"><div>${pp.enc === e.id ? `<b>${what}</b>` : what}<small>${esc(e.source)}${days}</small></div><span class="d">${esc(span || '')}</span></div>`;
  }
  if (s.encounters.length) h += '</div>';
  if (s.medications.length) {
    h += `<div><div class="label">Medications <span>latest stay</span></div>${s.medications.map((m) => `<div class="li"><div>${esc(m.name)} <small>${esc(m.detail)}</small></div><span class="d">${m.end ? `until ${esc(fmtShort(m.end))}` : m.time ? `since ${esc(fmtShort(m.time))}` : ''}</span></div>`).join('')}</div>`;
  }
  if (s.problems.length) {
    // the vocabularies the codes are of, as the databases name them (ICD-10-CM, ICD-9 …)
    const vocab = [...new Set(s.problems.map((p) => p.vocabulary).filter(Boolean))].join(' · ');
    h += `<div><div class="label">Conditions${vocab ? ` <span>${esc(vocab)}</span>` : ''}</div>${s.problems.map((p) => `<div class="li"><div>${p.code ? `<span class="code"${p.vocabulary ? ` title="${esc(p.vocabulary)}"` : ''}>${esc(p.code)}</span>` : ''}${esc(p.name)}</div><span class="d">${esc(p.time ? fmtShort(p.time) : '')}</span></div>`).join('')}</div>`;
  }
  $('#pp-info').innerHTML = h;
  const none = !h;
  if ($('#pp-grid').classList.contains('noinfo') !== none) {
    $('#pp-grid').classList.toggle('noinfo', none);
    // (the journey's width changed: drawn again)
    if (pp.jour) drawJ();
  }
}

/** A patient's identity bar and left column, drawn from their summary. */
export function drawSummary(sum) {
  pp.sum = sum;
  renderBar();
  renderInfo();
}

/** A visit over more than one day (a stay), and its days: calendar days in Medorama's time
 * zone, as the report counts its hospital days (report.js). */
export function stayDays(e) {
  const d = e.admit && e.discharge ? daysBetween(e.admit, e.discharge) : 0;
  return { long: d >= 1, days: Math.max(1, d) };
}

// ------------------------------------------------------------ the journey
function selectedSpan() {
  const x = entryOf(pp.selected);
  if (!x || x.time === null) return null;
  return { t0: x.time, t1: x.end ?? x.time, lane: x.category };
}

function drawJ() {
  if (!pp.jour) return;
  const e = encOf(pp.enc);
  const window = e && e.admit ? [e.admit - 12 * 3600, (e.discharge ?? e.admit + 86400) + 12 * 3600] : null;
  const el = $('#pp-journey');
  const top = el.scrollTop;
  const sel = selectedSpan();
  const draw = () => drawJourney(el, pp.jour, { hidden: pp.hidden, window, zoom: pp.zoom, selected: sel });
  let r = draw();
  // a scroll bar that takes room: draw again to the narrower width
  if (el.scrollWidth > el.clientWidth) r = draw();
  pp.hits = r.hits;
  pp.yOf = r.yOf;
  pp.jtop = r.top;
  el.scrollTop = top;
  // a newly selected entry is scrolled into sight (a long history opens at the top, the newest)
  if (sel && pp.selected !== pp.drawnSel) {
    const y = r.yOf(sel.t0);
    if (y < el.scrollTop + r.top + 10 || y > el.scrollTop + el.clientHeight - 10) el.scrollTop = Math.max(0, y - (el.clientHeight + r.top) / 2);
  }
  pp.drawnSel = pp.selected;
  drawCols();
  // what the journey shows: every visit, or the one chosen on the left (✕ shows them all again)
  $('#pp-encs').innerHTML = e
    ? `<button class="f on" data-enc-filter="" title="Show all visits">${esc(e.pseudonym || fmtDate(e.admit))} ✕</button>`
    : '<span class="jscope" title="Click a visit on the left to show only it">All visits</span>';
}

/** "Columns…": a box per column of the journey, ticked while it is shown. */
function drawCols() {
  const box = $('#pp-cols-list');
  const lanes = pp.jour?.lanes || [];
  const keys = lanes.map((l) => l.key).join(' ');
  if (box.dataset.keys !== keys) {
    box.dataset.keys = keys;
    box.innerHTML = lanes.map((l) => `<label><input type="checkbox" data-col="${esc(l.key)}"><span class="sw c-${esc(l.key)}" aria-hidden="true"></span>${esc(l.label)}</label>`).join('')
      || '<span class="muted small">No column.</span>';
  }
  for (const c of box.querySelectorAll('[data-col]')) c.checked = !pp.hidden.has(c.dataset.col);
}

/** Show or hide a column of the journey (and its entries in the report). */
function toggleLane(key) {
  if (pp.hidden.has(key)) pp.hidden.delete(key); else pp.hidden.add(key);
  store.set('hidden', [...pp.hidden]);
  const had = document.activeElement?.dataset?.laneKey;
  drawJ();
  list();
  // the column's name, drawn again, keeps the focus
  if (had && focusLost()) document.querySelector(`#pp-journey [data-lane-key="${CSS.escape(had)}"]`)?.focus();
}

function tipFor(k) {
  const lane = pp.jour.lanes[k.lane];
  if (k.kind === 'head') return `<b>${esc(lane.label)}</b><small>Click to ${pp.hidden.has(lane.key) ? 'show' : 'hide'}</small>`;
  let sub = lane.subs.length > 1 ? ` · ${esc(lane.subs[k.sub])}` : '';
  const lvl = k.level === 2 ? ' <span class="bd">critical</span>' : k.level ? ' <span class="bd">abnormal</span>' : '';
  if (k.kind === 'span') {
    if (k.light) sub = ' stay';
    const same = fmtShort(k.t0) === fmtShort(k.t1);
    return `<b>${esc(lane.label)}</b>${sub}${lvl}<small>${esc(fmtShort(k.t0))} ${esc(fmtHM(k.t0))} → ${same ? '' : `${esc(fmtShort(k.t1))} `}${esc(fmtHM(k.t1))}</small>`;
  }
  return `<b>${esc(lane.label)}</b>${sub} · ${k.n} item${k.n === 1 ? '' : 's'}${lvl}<small>${esc(fmtShort(k.t0))} ${esc(fmtHM(k.t0))}–${esc(fmtHM(k.t1))}</small>`;
}

/** A point's height in the journey's drawing: the column names stay put, the rest scrolls. */
const jy = (el, y) => (y < (pp.jtop ?? 0) ? y : y + el.scrollTop);

function clickHit(k) {
  if (!k) return;
  const lane = pp.jour.lanes[k.lane];
  if (k.kind === 'head') return toggleLane(lane.key);
  if (k.kind === 'span' && k.key.startsWith('enc:')) return setEnc(+k.key.slice(4));
  if (k.kind === 'span' && entryOf(k.key)) return select(k.key, { scrollList: true });
  // a cell: the newest entry of this column in that time
  const cands = pp.entries.filter((x) => x.category === lane.key && x.time !== null && x.time >= k.t0 && x.time < k.t1);
  const sub = lane.subs.length > 1 ? lane.subs[k.sub] : null;
  const best = cands.find((x) => sub && (x.title.includes(sub) || x.names.includes(sub))) || cands[0];
  if (best) select(best.key, { scrollList: true });
}

// ------------------------------------------------------------ the report and the detail
function list() {
  renderList($('#pp-list'), {
    entries: pp.entries, summary: pp.sum, terms: pp.terms, counts: pp.counts, fuzzy: pp.fuzzy, q: pp.q, mode: pp.mode, hidden: pp.hidden,
    enc: pp.enc, selected: pp.selected, labels: pp.labels, lanes: pp.jour?.lanes || [], trends: pp.trends,
  });
}

async function loadReport() {
  // the newest report asked for, of the patient shown: older answers are dropped
  const id = pp.id;
  const seq = ++pp.reportSeq;
  const r = await api(`patients/${id}/report?${qs({ q: pp.q })}`);
  if (id !== pp.id || seq !== pp.reportSeq) return;
  pp.entries = r.entries;
  pp.counts = r.counts;
  pp.terms = r.terms;
  pp.fuzzy = r.fuzzy;
  list();
}

function neighbours(key) {
  const x = entryOf(key);
  if (!x) return {};
  const same = pp.entries.filter((y) => y.category === x.category && !pp.hidden.has(y.category) && (pp.enc === null || y.encounter === pp.enc));
  const i = same.findIndex((y) => y.key === key);
  return { next: same[i - 1]?.key, prev: same[i + 1]?.key };
}

async function select(key, { scrollList = false, push = false } = {}) {
  pp.selected = key;
  if (push) go(href('patient', [String(pp.id)], { item: key, enc: pp.enc ?? '', q: pp.q, mode: pp.mode === 'timeline' ? '' : pp.mode })).catch(fail);
  else setParams({ item: key });
  document.querySelectorAll('#pp-list .it.sel').forEach((n) => { n.classList.remove('sel'); n.setAttribute('aria-pressed', 'false'); });
  const row = document.querySelector(`#pp-list .it[data-key="${CSS.escape(key)}"]`);
  if (row) {
    row.classList.add('sel');
    row.setAttribute('aria-pressed', 'true');
    if (scrollList) row.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }
  drawJ();
  const seq = ++pp.detailSeq;
  const el = $('#pp-detail');
  const id = pp.id;
  // a detail not read yet: its outline while it is (a slow database), not the one before
  const waiting = detailKept(key) ? null : setTimeout(() => { if (seq === pp.detailSeq) el.innerHTML = SKELETON; }, 150);
  try {
    const d = await detailOf(id, key);
    if (seq !== pp.detailSeq || id !== pp.id) return;
    const nav = neighbours(key);
    renderDetail(el, d, {
      id: pp.id, summary: pp.sum, terms: pp.terms, nav, width: el.clientWidth,
      context: (enc, t) => context(pp.sum, enc, t),
      setTimeAt: (f) => { el._timeAt = f; },
    });
    // the previous and next of its kind read ahead, so that stepping through them is at once
    for (const k of [nav.prev, nav.next]) if (k && !detailKept(k)) detailOf(id, k).catch(() => {});
  } catch (e) {
    if (seq === pp.detailSeq) el.innerHTML = `<p class="placeholder">${esc(e.message)}</p>`;
  } finally {
    clearTimeout(waiting);
  }
}

/** The outline of a detail being read. */
const SKELETON = '<div class="skel" aria-busy="true" aria-label="Loading"><i class="w60"></i><i class="w35"></i><div class="skel-tiles"><i></i><i></i><i></i><i></i></div><i class="tall"></i></div>';

function detailKept(key) {
  const k = pp.details.get(key);
  return k && Date.now() - k.at < DETAIL_TTL;
}

/** An entry's detail: as read in the last seconds (shown, or read ahead), else read now. */
function detailOf(id, key) {
  if (detailKept(key)) return pp.details.get(key).d;
  const d = api(`patients/${id}/detail?${qs({ key })}`);
  pp.details.set(key, { at: Date.now(), d });
  d.catch(() => pp.details.delete(key));
  // the oldest first out
  while (pp.details.size > DETAILS_KEPT) pp.details.delete(pp.details.keys().next().value);
  return d;
}

function emptyDetail() {
  pp.selected = null;
  pp.detailSeq += 1;
  $('#pp-detail').innerHTML = '<p class="placeholder">Choose an entry, or a mark in the journey.</p>';
}

function setEnc(id) {
  pp.enc = id || null;
  setParams({ enc: pp.enc ?? '' });
  const had = document.activeElement?.dataset?.enc;
  renderInfo();
  // the encounters drawn again: the focus back on the one chosen
  if (had && focusLost()) document.querySelector(`#pp-info [data-enc="${CSS.escape(had)}"]`)?.focus();
  drawJ();
  list();
}

/** The report's mode: its button on, and said to be pressed. */
function showMode() {
  document.querySelectorAll('#pp-mode button').forEach((b) => {
    b.classList.toggle('on', b.dataset.mode === pp.mode);
    b.setAttribute('aria-pressed', String(b.dataset.mode === pp.mode));
  });
}

// ------------------------------------------------------------ loading
async function open(id) {
  pp.id = id;
  pp.trends = null;
  pp.details.clear();
  pp.zoom = 1;
  $('#pp-bar').innerHTML = '';
  $('#pp-info').innerHTML = '';
  $('#pp-journey').innerHTML = '';
  $('#pp-list').innerHTML = '<p class="placeholder">Loading…</p>';
  emptyDetail();
  let sum;
  let jour;
  try {
    [sum, jour] = await Promise.all([api(`patients/${id}`), api(`patients/${id}/journey`)]);
  } catch (e) {
    // another patient opened meanwhile: theirs is shown
    if (pp.id !== id) throw Object.assign(new Error('superseded'), { quiet: true });
    // removed (its databases deleted, merged into another), or never there
    pp.id = null;
    $('#pp-list').innerHTML = `<p class="placeholder">This patient is not in the catalog (any more): ${esc(e.message)}. <a href="#/patients">Patient Catalog</a></p>`;
    throw Object.assign(new Error('gone'), { quiet: true });
  }
  if (pp.id !== id) throw Object.assign(new Error('superseded'), { quiet: true });
  pp.sum = sum;
  pp.jour = jour;
  pp.labels = Object.fromEntries(jour.lanes.map((l) => [l.key, l.label]));
  renderBar();
}

// ------------------------------------------------------------ the keyboard and the columns' widths
/** ↑ and ↓ move to the line above or below in the report (the detail follows), Enter or Space
 * selects the line that has the focus, Escape clears the selection: anywhere on the page but in
 * a box being typed in or the journey (whose column names take Enter and Space). */
function onKey(e) {
  const t = e.target;
  if (t.closest?.('input, textarea, select, [contenteditable], .sline, #pp-journey, .gutter') || e.altKey || e.ctrlKey || e.metaKey) return;
  const rows = [...document.querySelectorAll('#pp-list .it[data-key]')];
  if (!rows.length) return;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    const at = rows.indexOf(t.closest?.('.it')) >= 0 ? rows.indexOf(t.closest('.it')) : rows.findIndex((r) => r.dataset.key === pp.selected);
    const next = rows[at < 0 ? 0 : at + (e.key === 'ArrowDown' ? 1 : -1)];
    if (!next) return;
    e.preventDefault();
    next.focus({ preventScroll: true });
    next.scrollIntoView({ block: 'nearest' });
    select(next.dataset.key).catch(fail);
  } else if ((e.key === 'Enter' || e.key === ' ') && t.closest?.('#pp-list .it[data-key]')) {
    e.preventDefault();
    select(t.closest('.it').dataset.key).catch(fail);
  } else if (e.key === 'Escape' && pp.selected) {
    e.preventDefault();
    clearSelection();
  }
}

function clearSelection() {
  document.querySelectorAll('#pp-list .it.sel').forEach((n) => { n.classList.remove('sel'); n.setAttribute('aria-pressed', 'false'); });
  emptyDetail();
  setParams({ item: '' });
  drawJ();
}

/** The columns a divider sets the width of, its bounds and its default (none: the stylesheet's). */
const GUTTERS = { c1: { el: '#pp-grid', min: 220, max: 560 }, c2: { el: '#pp-grid', min: 260, max: 760 }, c3: { el: '#pp-split', min: 240, max: 900 } };

/** The widths chosen by dragging the dividers, as kept in this browser. */
function applyWidths() {
  const w = store.get('widths', {});
  for (const [k, g] of Object.entries(GUTTERS)) {
    const box = $(g.el);
    if (w[k]) box.style.setProperty(`--${k}`, `${w[k]}px`); else box.style.removeProperty(`--${k}`);
  }
}

function setWidth(k, px) {
  const w = store.get('widths', {});
  if (px === null) delete w[k]; else w[k] = Math.round(Math.max(GUTTERS[k].min, Math.min(GUTTERS[k].max, px)));
  store.set('widths', w);
  applyWidths();
}

/** The width the column left of a divider has now. */
function widthOf(k) {
  if (k === 'c1') return $('#pp-info').getBoundingClientRect().width;
  if (k === 'c2') return document.querySelector('#pp-grid .col.journey').getBoundingClientRect().width;
  return $('#pp-list').getBoundingClientRect().width;
}

function wireGutters() {
  applyWidths();
  const redrawn = () => { drawJ(); if (pp.selected) select(pp.selected); };
  for (const g of document.querySelectorAll('#view-patient .gutter')) {
    const k = g.dataset.gutter;
    g.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      g.setPointerCapture(e.pointerId);
      const x0 = e.clientX;
      const w0 = widthOf(k);
      document.body.classList.add('resizing');
      const move = (m) => setWidth(k, w0 + m.clientX - x0);
      const up = () => {
        g.removeEventListener('pointermove', move);
        document.body.classList.remove('resizing');
        redrawn();
      };
      g.addEventListener('pointermove', move);
      g.addEventListener('pointerup', up, { once: true });
      g.addEventListener('pointercancel', up, { once: true });
    });
    g.addEventListener('dblclick', () => { setWidth(k, null); redrawn(); });
    // from the keyboard: ← and → by 24 pixels, Home back to the default
    g.addEventListener('keydown', (e) => {
      const step = { ArrowLeft: -24, ArrowRight: 24 }[e.key];
      if (step) { e.preventDefault(); setWidth(k, widthOf(k) + step); redrawn(); }
      if (e.key === 'Home') { e.preventDefault(); setWidth(k, null); redrawn(); }
    });
  }
}

/** The suggestions closed (and said to be closed). */
function hideSug() {
  $('#pp-suggest').classList.add('hidden');
  $('#pp-find').setAttribute('aria-expanded', 'false');
  $('#pp-find').removeAttribute('aria-activedescendant');
}

export default {
  /** The data changed: load the patient again at the next show. */
  invalidate() { pp.id = null; pp.q = null; pp.details.clear(); },
  async show(r) {
    $('#pp-find').placeholder = privacy.on
      ? 'Search another patient: pseudonym (MO-…), sex, database…'
      : 'Search another patient: name, MRN, ID, date of birth…';
    const id = +r.parts[0];
    if (!id) return go('#/patients', { replace: true });
    // a newer address drawn while this one loads: its answers are dropped
    const nav = navigation();
    const late = () => superseded(nav) || pp.id !== id;
    const fresh = id !== pp.id;
    if (fresh) {
      try { await open(id); } catch (e) { if (e.quiet) return; throw e; }
      if (late()) return;
    }
    pp.mode = r.params.get('mode') || 'timeline';
    pp.enc = r.params.get('enc') ? +r.params.get('enc') : null;
    const q = r.params.get('q') || '';
    showMode();
    $('#pp-q').value = q;
    $('#pp-qx').classList.toggle('hidden', !q);
    if (fresh || q !== pp.q) {
      pp.q = q;
      await loadReport();
      if (late()) return;
    }
    if (pp.mode === 'trends' && !pp.trends) {
      await loadTrends();
      if (late()) return;
    }
    renderInfo();
    list();
    const item = r.params.get('item');
    if (item && entryOf(item)) await select(item, { scrollList: true });
    else { emptyDetail(); drawJ(); }
  },
  hide() { hideSug(); $('#pp-tip').classList.add('hidden'); $('#pp-cols').open = false; },
  wire() {
    wireGutters();
    $('#view-patient').addEventListener('keydown', onKey);
    const jel = $('#pp-journey');
    const tip = $('#pp-tip');
    jel.addEventListener('mousemove', (e) => {
      const box = jel.getBoundingClientRect();
      const k = hitAt(pp.hits, e.clientX - box.left, jy(jel, e.clientY - box.top));
      if (!k) { tip.classList.add('hidden'); jel.style.cursor = ''; return; }
      jel.style.cursor = 'pointer';
      tip.innerHTML = tipFor(k);
      tip.classList.remove('hidden');
      const col = jel.parentElement.getBoundingClientRect();
      const x = Math.min(e.clientX - col.left + 14, col.width - tip.offsetWidth - 6);
      tip.style.left = `${Math.max(4, x)}px`;
      tip.style.top = `${e.clientY - col.top + 14}px`;
    });
    jel.addEventListener('mouseleave', () => tip.classList.add('hidden'));
    jel.addEventListener('click', (e) => {
      // a column's name (also from the keyboard: Enter or Space)
      const head = e.target.closest?.('[data-lane-key]');
      if (head) { toggleLane(head.dataset.laneKey); return; }
      const box = jel.getBoundingClientRect();
      clickHit(hitAt(pp.hits, e.clientX - box.left, jy(jel, e.clientY - box.top)));
    });
    $('#pp-cols-list').addEventListener('change', (e) => {
      const c = e.target.closest('[data-col]');
      if (c && c.checked === pp.hidden.has(c.dataset.col)) toggleLane(c.dataset.col);
    });
    document.addEventListener('click', (e) => { if (!e.target.closest('#pp-cols')) $('#pp-cols').open = false; });
    $('#pp-zin').addEventListener('click', () => { pp.zoom = Math.min(8, pp.zoom * 1.6); drawJ(); });
    $('#pp-zout').addEventListener('click', () => { pp.zoom = Math.max(1, pp.zoom / 1.6); drawJ(); });
    $('#pp-encs').addEventListener('click', (e) => {
      const b = e.target.closest('[data-enc-filter]');
      if (b && b.dataset.encFilter === '') setEnc(null);
    });
    $('#pp-info').addEventListener('click', (e) => {
      const row = e.target.closest('[data-enc]');
      if (row) setEnc(pp.enc === +row.dataset.enc ? null : +row.dataset.enc);
    });
    $('#pp-list').addEventListener('click', (e) => {
      const row = e.target.closest('[data-key]');
      if (row) return select(row.dataset.key);
      const tr = e.target.closest('[data-trend]');
      if (tr) {
        const [cat, t] = tr.dataset.trend.split('|');
        const x = pp.entries.find((y) => y.category === cat && Math.abs((y.time ?? -1) - +t) < 1) || pp.entries.find((y) => y.category === cat);
        if (x) select(x.key);
      }
    });
    $('#pp-detail').addEventListener('click', (e) => {
      const st = e.target.closest('[data-step]');
      if (st) {
        const n = neighbours(pp.selected);
        const k = +st.dataset.step < 0 ? n.prev : n.next;
        if (k) select(k, { scrollList: true });
        return;
      }
      const nt = e.target.closest('[data-note]');
      if (nt) {
        if (!nt.classList.contains('open')) nt.classList.add('open');
        else if (entryOf(nt.dataset.note)) select(nt.dataset.note, { scrollList: true });
      }
    });
    $('#pp-q').addEventListener('input', (e) => {
      clearTimeout(pp.qtimer);
      pp.qtimer = setTimeout(() => {
        pp.q = e.target.value.trim();
        $('#pp-qx').classList.toggle('hidden', !pp.q);
        setParams({ q: pp.q });
        loadReport().catch(fail);
      }, 250);
    });
    $('#pp-qx').addEventListener('click', (e) => {
      e.preventDefault();
      $('#pp-q').value = '';
      pp.q = '';
      $('#pp-qx').classList.add('hidden');
      setParams({ q: '' });
      loadReport().catch(fail);
    });
    $('#pp-mode').addEventListener('click', async (e) => {
      const b = e.target.closest('[data-mode]');
      if (!b) return;
      pp.mode = b.dataset.mode;
      showMode();
      setParams({ mode: pp.mode === 'timeline' ? '' : pp.mode });
      if (pp.mode === 'trends' && !pp.trends) {
        list();
        await loadTrends().catch(fail);
      }
      list();
    });
    // another patient: suggestions while typing
    const find = $('#pp-find');
    const sug = $('#pp-suggest');
    // a list box of the patients found: the arrow keys move in it (aria-activedescendant), the
    // focus stays in the box being typed in
    const showSug = () => {
      sug.innerHTML = pp.suggest.map((p, i) => {
        const nm = (p.phi ? [p.phi.last_name, p.phi.first_name].filter(Boolean).join(', ') : '') || p.patient_id || p.pseudonym;
        const sub = [p.patient_id || p.pseudonym, p.sex, p.age !== undefined ? `${p.age} y` : '', p.phi?.birth_date, (p.phi?.mrns || []).map((m) => m[1]).join(', ')].filter(Boolean).join(' · ');
        return `<a href="${esc(href('patient', [String(p.id)]))}" id="pp-sug-${i}" role="option" aria-selected="${i === pp.sugIdx}" tabindex="-1" class="${i === pp.sugIdx ? 'on' : ''}"><b>${esc(nm)}</b><span class="muted">${p.encounters} visits</span><small>${esc(sub)}</small></a>`;
      }).join('') || '<div class="muted placeholder" role="option" aria-disabled="true" aria-selected="false">No patient matches.</div>';
      sug.classList.remove('hidden');
      find.setAttribute('aria-expanded', 'true');
      if (pp.sugIdx >= 0) find.setAttribute('aria-activedescendant', `pp-sug-${pp.sugIdx}`); else find.removeAttribute('aria-activedescendant');
    };
    find.addEventListener('input', () => {
      clearTimeout(pp.ftimer);
      const q = find.value.trim();
      if (!q) { hideSug(); return; }
      pp.ftimer = setTimeout(async () => {
        const seq = ++pp.findSeq;
        const d = await api(`patients?${qs({ q, limit: 8 })}`).catch(() => ({ items: [] }));
        // only the answer to what the box holds now
        if (seq !== pp.findSeq || find.value.trim() !== q) return;
        pp.suggest = d.items;
        pp.sugIdx = d.items.length ? 0 : -1;
        showSug();
      }, 200);
    });
    find.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') { hideSug(); find.blur(); }
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        pp.sugIdx = Math.max(0, Math.min(pp.suggest.length - 1, pp.sugIdx + (e.key === 'ArrowDown' ? 1 : -1)));
        showSug();
      }
      if (e.key === 'Enter' && pp.suggest[pp.sugIdx]) {
        hideSug();
        find.value = '';
        go(href('patient', [String(pp.suggest[pp.sugIdx].id)])).catch(fail);
      }
    });
    sug.addEventListener('click', () => { hideSug(); find.value = ''; });
    document.addEventListener('click', (e) => { if (!e.target.closest('#view-patient .sline')) hideSug(); });
    $('#pp-chip').addEventListener('click', (e) => { if (e.target.closest('[data-close]')) go('#/patients').catch(fail); });
    // redraw on resize and theme change
    let rt = null;
    const redraw = () => { clearTimeout(rt); rt = setTimeout(() => { if (!$('#view-patient').classList.contains('hidden')) { drawJ(); if (pp.selected) select(pp.selected); } }, 120); };
    window.addEventListener('resize', redraw);
    window.addEventListener('themechange', redraw);
  },
};
