// One patient: identity bar, encounters and conditions, the journey, the report and the detail
// of the selected entry.  Its address holds the selection (#/patient/12?item=…&enc=…&q=…), so
// that back and forward, reloading and sharing keep it.

import { $, api, esc, fail, fmtDate, fmtHM, fmtShort, fmtYear, privacy, qs, store } from '../core.js';
import { focusLost, go, href, navigation, setParams, superseded } from '../router.js';
import { drawJourney, hitAt } from '../patient/journey.js';
import { context, renderList } from '../patient/report.js';
import { renderDetail } from '../patient/detail.js';

const pp = {
  id: null, sum: null, jour: null, entries: [], counts: [], terms: [], fuzzy: [], trends: null, labels: {},
  mode: 'timeline', q: '', enc: null, zoom: 1, selected: null, hidden: new Set(store.get('hidden', [])), hits: [],
  qtimer: null, ftimer: null, detailSeq: 0, reportSeq: 0, findSeq: 0, suggest: [], sugIdx: -1,
};

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
  return sum.patient_id;
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
  else if (s.phi) meta.push([s.patient_id, 'patient ID'], [s.pseudonym, 'pseudonym']);
  const mrns = s.phi?.mrns || [];
  if (mrns.length) meta.push([mrns.map((m) => m[1]).join(' · '), mrns.some((m) => m[0]) ? `MRN · ${mrns.map((m) => m[0] || 'default').join(', ')}` : 'MRN']);
  const stays = s.encounters.length + (s.visits || []).length;
  if (s.first) meta.push([`${fmtDate(s.first)} – ${fmtDate(s.last)}`, `${stays} encounter${stays === 1 ? '' : 's'}`]);
  let allergy;
  if (s.allergies === null) allergy = '<span class="allergy none">Allergies: not recorded</span>';
  else if (!s.allergies.length) allergy = '<span class="allergy none">No allergy recorded</span>';
  else allergy = `<span class="allergy">⚠ Allergy: ${esc(s.allergies.join(', '))}</span>`;
  // a connected database that did not answer: what it holds is missing from the page
  const missing = (s.unavailable || []).length
    ? `<span class="pill" title="This patient's data in ${esc(s.unavailable.join(', '))} could not be read now: the page shows the other sources">${esc(s.unavailable.join(', '))}: not available</span>`
    : '';
  const demo = s.demo ? '<span class="pill" title="An invented patient of the demo: deleting the cohort DEMO removes them">DEMO</span>' : '';
  $('#pp-bar').innerHTML = `<div class="nm">${esc(name(s))}</div>${demo}<div class="meta">${meta.map(([a, b]) => `<div>${esc(a)}<small>${esc(b)}</small></div>`).join('')}</div>${missing}${allergy}`;
}

function renderInfo() {
  const s = pp.sum;
  const visits = s.visits || [];
  let h = `<div><div class="label">Encounters <span>${s.encounters.length + visits.length}</span></div>`;
  if (!s.encounters.length && !visits.length) h += '<p class="muted">No encounter in the databases loaded.</p>';
  for (const e of s.encounters) {
    const kind = e.inpatient ? 'Inpatient' : 'Outpatient';
    const units = [...new Set((e.stays || []).map((x) => x.unit).filter(Boolean))];
    const where = [e.admit_unit, ...units.filter((u) => u !== e.admit_unit)].filter(Boolean).join(' → ');
    const span = e.discharge && e.admit && dayDiff(e.admit, e.discharge) >= 1
      ? `${fmtShort(e.admit)} – ${fmtShort(e.discharge)}${fmtYear(e.discharge) !== fmtYear(e.admit) ? '' : `, ${fmtYear(e.admit)}`}`
      : fmtDate(e.admit);
    const days = e.inpatient && e.admit && e.discharge ? ` · ${Math.max(1, Math.round((e.discharge - e.admit) / 86400))} day${Math.round((e.discharge - e.admit) / 86400) > 1 ? 's' : ''}` : '';
    h += `<div class="enc${pp.enc === e.id ? ' on' : ''}" data-enc="${e.id}" tabindex="0" role="button" aria-pressed="${pp.enc === e.id}"><div>${pp.enc === e.id ? '<b>' : ''}${esc(kind)}${where ? ` · ${esc(where)}` : ''}${pp.enc === e.id ? '</b>' : ''}<small>${esc(e.encounter_id)}${days}</small></div><span class="d">${esc(span)}</span></div>`;
  }
  // the stays and visits of the connected databases (their data are in the journey and the
  // report; the encounters above are the catalog's)
  for (const v of visits) {
    const long = v.admit && v.discharge && dayDiff(v.admit, v.discharge) >= 1;
    const span = long ? `${fmtShort(v.admit)} – ${fmtShort(v.discharge)}, ${fmtYear(v.discharge)}` : fmtDate(v.admit);
    h += `<div class="enc live" title="In the connected database ${esc(v.source)}"><div>${long ? 'Stay' : 'Visit'}${v.unit ? ` · ${esc(v.unit)}` : ''}${v.icu ? ' · ICU' : ''}<small>${esc(v.source)}</small></div><span class="d">${esc(span || '')}</span></div>`;
  }
  h += '</div>';
  if (s.medications.length) {
    h += `<div><div class="label">Medications <span>latest stay</span></div>${s.medications.map((m) => `<div class="li"><div>${esc(m.name)} <small>${esc(m.detail)}</small></div><span class="d">${m.end ? `until ${esc(fmtShort(m.end))}` : m.time ? `since ${esc(fmtShort(m.time))}` : ''}</span></div>`).join('')}</div>`;
  }
  if (s.problems.length) {
    h += `<div><div class="label">Conditions <span>ICD-10</span></div>${s.problems.map((p) => `<div class="li"><div>${p.code ? `<span class="code">${esc(p.code)}</span>` : ''}${esc(p.name)}</div><span class="d">${esc(p.time ? fmtShort(p.time) : '')}</span></div>`).join('')}</div>`;
  }
  $('#pp-info').innerHTML = h;
}

function dayDiff(a, b) { return Math.floor(b / 86400) - Math.floor(a / 86400); }

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
  // what the journey shows: every encounter, or the one chosen on the left (✕ shows them all again)
  $('#pp-encs').innerHTML = e
    ? `<button class="f on" data-enc-filter="" title="Show all encounters">${esc(e.encounter_id)} ✕</button>`
    : '<span class="jscope" title="Click an encounter on the left to show only it">All encounters</span>';
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
  if (k.kind === 'head') {
    if (pp.hidden.has(lane.key)) pp.hidden.delete(lane.key); else pp.hidden.add(lane.key);
    store.set('hidden', [...pp.hidden]);
    drawJ();
    list();
    return;
  }
  if (k.kind === 'span' && k.key.startsWith('enc:')) return setEnc(+k.key.slice(4));
  if (k.kind === 'span' && entryOf(k.key)) return select(k.key, { scrollList: true });
  // a cell: the newest entry of this column in that time
  const cands = pp.entries.filter((x) => x.category === lane.key && x.time !== null && x.time >= k.t0 && x.time < k.t1);
  const sub = lane.subs.length > 1 ? lane.subs[k.sub] : null;
  const best = cands.find((x) => sub && (x.title.includes(sub) || x.summary.includes(sub))) || cands[0];
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
  try {
    const d = await api(`patients/${id}/detail?${qs({ key })}`);
    if (seq !== pp.detailSeq || id !== pp.id) return;
    renderDetail(el, d, {
      id: pp.id, summary: pp.sum, terms: pp.terms, nav: neighbours(key), width: el.clientWidth,
      context: (enc, t) => context(pp.sum, enc, t),
      setTimeAt: (f) => { el._timeAt = f; },
    });
  } catch (e) {
    if (seq === pp.detailSeq) el.innerHTML = `<p class="placeholder">${esc(e.message)}</p>`;
  }
}

function emptyDetail() {
  pp.selected = null;
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

// ------------------------------------------------------------ loading
async function open(id) {
  pp.id = id;
  pp.trends = null;
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

export default {
  /** The data changed: load the patient again at the next show. */
  invalidate() { pp.id = null; pp.q = null; },
  async show(r) {
    $('#pp-find').placeholder = privacy.on
      ? 'Search another patient: pseudonym (MO-…), encounter, unit…'
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
    document.querySelectorAll('#pp-mode button').forEach((b) => b.classList.toggle('on', b.dataset.mode === pp.mode));
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
  hide() { $('#pp-suggest').classList.add('hidden'); $('#pp-tip').classList.add('hidden'); },
  wire() {
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
      const box = jel.getBoundingClientRect();
      clickHit(hitAt(pp.hits, e.clientX - box.left, jy(jel, e.clientY - box.top)));
    });
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
      document.querySelectorAll('#pp-mode button').forEach((x) => x.classList.toggle('on', x === b));
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
    const showSug = () => {
      sug.innerHTML = pp.suggest.map((p, i) => {
        const nm = p.phi ? [p.phi.last_name, p.phi.first_name].filter(Boolean).join(', ') || p.patient_id : p.patient_id;
        const sub = [p.patient_id, p.sex, p.age !== undefined ? `${p.age} y` : '', p.phi?.birth_date, (p.phi?.mrns || []).map((m) => m[1]).join(', ')].filter(Boolean).join(' · ');
        return `<a href="${esc(href('patient', [String(p.id)]))}" class="${i === pp.sugIdx ? 'on' : ''}"><b>${esc(nm)}</b><span class="muted">${p.encounters} enc.</span><small>${esc(sub)}</small></a>`;
      }).join('') || '<div class="muted placeholder">No patient matches.</div>';
      sug.classList.remove('hidden');
    };
    find.addEventListener('input', () => {
      clearTimeout(pp.ftimer);
      const q = find.value.trim();
      if (!q) { sug.classList.add('hidden'); return; }
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
      if (e.key === 'Escape') { sug.classList.add('hidden'); find.blur(); }
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        pp.sugIdx = Math.max(0, Math.min(pp.suggest.length - 1, pp.sugIdx + (e.key === 'ArrowDown' ? 1 : -1)));
        showSug();
      }
      if (e.key === 'Enter' && pp.suggest[pp.sugIdx]) {
        sug.classList.add('hidden');
        find.value = '';
        go(href('patient', [String(pp.suggest[pp.sugIdx].id)])).catch(fail);
      }
    });
    sug.addEventListener('click', () => { sug.classList.add('hidden'); find.value = ''; });
    document.addEventListener('click', (e) => { if (!e.target.closest('#view-patient .sline')) sug.classList.add('hidden'); });
    $('#pp-chip').addEventListener('click', (e) => { if (e.target.closest('[data-close]')) go('#/patients').catch(fail); });
    // redraw on resize and theme change
    let rt = null;
    const redraw = () => { clearTimeout(rt); rt = setTimeout(() => { if (!$('#view-patient').classList.contains('hidden')) { drawJ(); if (pp.selected) select(pp.selected); } }, 120); };
    window.addEventListener('resize', redraw);
    window.addEventListener('themechange', redraw);
  },
};
