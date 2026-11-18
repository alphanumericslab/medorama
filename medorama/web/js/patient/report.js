// The report: every entry of a patient, newest first (by date, or by category), or the trends
// of every measurement; with a search, the entries found and their snippets.

import { catClass, dayKey, daysBetween, esc, fmtDay, fmtHM, fmtShort, mark } from '../core.js';
import { spark, color } from './charts.js';

/** Where the patient was at time t: the visit (a stay: its day), its unit, its database. */
export function context(summary, encId, t) {
  const e = summary.encounters.find((x) => x.id === encId);
  if (!e) return '';
  // a stay: a visit over more than one day (patient.js), its days counted in Medorama's time zone
  const stay = e.admit && e.discharge && daysBetween(e.admit, e.discharge) >= 1;
  const parts = [stay ? 'Stay' : 'Visit'];
  if (stay && isFinite(t)) parts.push(`hospital day ${daysBetween(e.admit, t) + 1}`);
  if (e.unit) parts.push(e.unit);
  if (e.icu) parts.push('ICU');
  parts.push(e.source);
  return parts.join(' · ');
}

// One line per entry: what was done, by name; its values are the detail's (detail.js), shown
// once.  `when`: its date, where no day heading gives it (by category).
function entryRow(x, terms, selected, when = '') {
  const sum = when ? ` <span class="muted">· ${esc(when)}</span>` : '';
  const snip = x.snippet ? `<span class="snip">${mark(x.snippet, terms)}</span>` : '';
  const go = x.attachment ? '<span class="go" title="Opens a viewer or a file">↗</span>' : '<span></span>';
  return `<div class="it${x.key === selected ? ' sel' : ''}" data-key="${esc(x.key)}" tabindex="0" role="button" aria-pressed="${x.key === selected}"><span class="tm">${esc(fmtHM(x.time))}</span><span class="sw ${catClass(x.category)}"></span>
    <span class="txt"><span class="one" title="${esc(x.title)}"><b>${mark(x.title, terms)}</b>${sum}</span>${snip}</span>${go}</div>`;
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
      for (const x of xs) html += entryRow(x, st.terms, st.selected, fmtShort(x.time));
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

/** A measured value as it was measured: every digit of a whole number (12345, not 12350), a
 * fraction to six significant digits at least (0.1 + 0.2 shows as 0.3). */
export function fmtValue(v) {
  if (!isFinite(v)) return String(v ?? '');
  if (Number.isInteger(v)) return String(v);
  const whole = Math.max(1, Math.ceil(Math.log10(Math.abs(v) + 1)));
  return String(+v.toPrecision(Math.min(17, Math.max(6, whole + 3))));
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
    const v = `${fmtValue(last[1])} ${t.unit}`.trim();
    html += `<div class="trend" data-trend="${esc(t.category)}|${last[0]}" tabindex="0" role="button"><span><b>${esc(t.name)}</b><br><small>${t.n} values · last ${esc(fmtShort(last[0]))}</small></span>
      <span>${esc(v)}${badge}</span>${spark(t.points.map((p) => [p[0], p[1]]), t.ref_low, t.ref_high, { w: 180, h: 28, stroke: color(`c-${t.category}`) })}</div>`;
  }
  el.innerHTML = html;
}
