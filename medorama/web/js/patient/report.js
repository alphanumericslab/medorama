// The report: every entry of a patient, newest first (by date, or by category), or the trends
// of every measurement; with a search, the entries found and their snippets.

import { catClass, dayKey, esc, fmtDay, fmtHM, fmtShort, mark } from '../core.js';
import { spark, color } from './charts.js';

/** Where the patient was at time t: encounter, hospital day, unit. */
export function context(summary, encId, t) {
  const e = summary.encounters.find((x) => x.id === encId);
  if (!e) return '';
  const parts = [e.encounter_id];
  if (e.inpatient && e.admit && isFinite(t)) {
    const d0 = Date.parse(`${dayKey(e.admit)}T00:00:00Z`);
    const d1 = Date.parse(`${dayKey(t)}T00:00:00Z`);
    parts.push(`hospital day ${Math.round((d1 - d0) / 86400000) + 1}`);
  }
  const stay = (e.stays || []).find((s) => s.start !== null && s.start <= t && (s.end === null || s.end >= t) && s.unit);
  const unit = stay ? `${stay.unit}${stay.bed ? ` bed ${stay.bed}` : ''}` : e.admit_unit;
  if (unit) parts.push(unit);
  return parts.join(' · ');
}

function entryRow(x, terms, selected) {
  const flag = x.abnormal ? ` <span class="badge">${x.abnormal} abnormal</span>` : '';
  const sum = x.summary ? ` <span class="muted">· ${mark(x.summary, terms)}</span>` : '';
  const snip = x.snippet ? `<span class="snip">${mark(x.snippet, terms)}</span>` : '';
  const go = x.attachment ? '<span class="go" title="Opens a viewer or a file">↗</span>' : '<span></span>';
  return `<div class="it${x.key === selected ? ' sel' : ''}" data-key="${esc(x.key)}" tabindex="0" role="button" aria-pressed="${x.key === selected}"><span class="tm">${esc(fmtHM(x.time))}</span><span class="sw ${catClass(x.category)}"></span>
    <span class="txt"><b>${mark(x.title, terms)}</b>${flag}${sum}${snip}</span>${go}</div>`;
}

/**
 * Draw the list.  st: { entries, summary, terms, counts, fuzzy, q, mode, hidden, enc,
 * selected, labels, trends }.
 */
export function renderList(el, st) {
  const shown = st.entries.filter((x) => !st.hidden.has(x.category) && (st.enc === null || x.encounter === st.enc));
  let html = '';
  if (st.q) {
    const chips = (st.counts || []).map(([k, n]) => `<span class="f on"><span class="sw ${catClass(k)}"></span>${esc(st.labels[k] || k)} · ${n}</span>`).join('');
    const fuzzy = (st.fuzzy || []).map(([a, b]) => `“${esc(a)}”: showing “${esc(b)}”`).join(' · ');
    html += `<div class="qsum"><span><b>${shown.length}</b> result${shown.length === 1 ? '' : 's'}</span>${chips}</div>${fuzzy ? `<div class="muted">${fuzzy}</div>` : ''}`;
  }
  if (st.mode === 'trends') return renderTrends(el, st);
  if (!shown.length) {
    el.innerHTML = `${html}<p class="placeholder">${st.q ? 'Nothing found.' : 'No entries.'}</p>`;
    return;
  }
  if (st.mode === 'category') {
    const order = st.lanes.map((l) => l.key);
    const groups = new Map();
    for (const x of shown) {
      if (!groups.has(x.category)) groups.set(x.category, []);
      groups.get(x.category).push(x);
    }
    const keys = [...groups.keys()].sort((a, b) => (order.indexOf(a) + 99) % 199 - (order.indexOf(b) + 99) % 199);
    for (const k of keys) {
      const xs = groups.get(k);
      html += `<div class="day"><b>${esc(st.labels[k] || k)}</b><span>${xs.length}</span></div>`;
      for (const x of xs) html += entryRow({ ...x, title: x.title, summary: `${fmtShort(x.time)}${x.summary ? ` · ${x.summary}` : ''}` }, st.terms, st.selected);
    }
  } else {
    let day = null;
    for (const x of shown) {
      const d = dayKey(x.time);
      if (d !== day) {
        day = d;
        const ctx = context(st.summary, x.encounter, x.time);
        html += `<div class="day"><b>${esc(fmtDay(x.time))}</b><span>${esc(ctx)}</span></div>`;
      }
      html += entryRow(x, st.terms, st.selected);
    }
  }
  el.innerHTML = html;
}

function renderTrends(el, st) {
  if (!st.trends) {
    el.innerHTML = '<p class="placeholder">Loading…</p>';
    return;
  }
  const items = st.trends.filter((t) => !st.hidden.has(t.category) && (!st.q || t.name.toLowerCase().includes(st.q.toLowerCase())));
  if (!items.length) {
    el.innerHTML = '<p class="placeholder">No measurements.</p>';
    return;
  }
  let html = '';
  let cat = null;
  for (const t of items.sort((a, b) => (a.category > b.category ? 1 : a.category < b.category ? -1 : b.n - a.n))) {
    if (t.category !== cat) {
      cat = t.category;
      html += `<div class="day"><b>${esc(st.labels[cat] || cat)}</b></div>`;
    }
    const last = t.points[t.points.length - 1];
    const badge = last[2] === 2 ? ' <span class="badge">critical</span>' : last[2] ? ' <span class="badge">abnormal</span>' : '';
    const v = `${+last[1].toPrecision(4)} ${t.unit}`.trim();
    html += `<div class="trend" data-trend="${esc(t.category)}|${last[0]}" tabindex="0" role="button"><span><b>${esc(t.name)}</b><br><small>${t.n} values · last ${esc(fmtShort(last[0]))}</small></span>
      <span>${esc(v)}${badge}</span>${spark(t.points.map((p) => [p[0], p[1]]), t.ref_low, t.ref_high, { w: 180, h: 28, stroke: color(`c-${t.category}`) })}</div>`;
  }
  el.innerHTML = html;
}
