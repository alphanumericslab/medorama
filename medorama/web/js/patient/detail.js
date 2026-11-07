// The detail of the selected entry: a laboratory panel with trends, a day of vital signs, an
// item with its text or attached file, an ICU recording at a glance (click to open the viewer
// there); and the notes written around that moment.

import { API, catClass, privacy, esc, externalViewerLink, fmtDay, fmtDate, fmtDuration, fmtHM, fmtShort, mark, phi, viewerMode } from '../core.js';
import { go, href } from '../router.js';
import { color, overview, spark } from './charts.js';
import { APPS, appHref } from '../views/apps.js';

const fmtV = (v) => (v === null || v === undefined || v === '' ? '' : String(v));

// whose entry the detail shows: the viewers it opens carry it (their banner)
let shown = {};

function viewerTarget(path, t0, t1) {
  const mode = viewerMode();
  if (mode === 'embedded') return { href: href('viewer', [], { path, t0, t1, ...shown }), ext: false };
  if (mode === 'link') return { href: externalViewerLink(path, t0, t1), ext: true };
  return null;
}

function rowsTable(d, terms) {
  const vital = d.key.startsWith('vit:');
  let h = `<table class="t"><tr><th>${vital ? 'Sign' : 'Test'}</th><th class="num">${vital ? 'Last' : 'Value'}</th><th>Unit</th><th>Reference</th>${vital ? '' : '<th class="num">Previous</th>'}<th title="${vital ? 'The 12 hours before and after this reading' : 'This test over the stay'}">${vital ? '±12 h' : 'This stay'}</th></tr>`;
  for (const r of d.rows) {
    const cls = r.level ? (r.num !== null && r.ref_low !== null && r.num < r.ref_low ? 'low' : 'bad') : '';
    const badge = r.level ? ` <span class="badge${cls === 'low' ? ' lo' : ''}">${esc(r.flag || (cls === 'low' ? 'L' : 'H'))}</span>` : '';
    const ref = r.ref_low !== null || r.ref_high !== null ? `${fmtV(r.ref_low)} – ${fmtV(r.ref_high)}` : '';
    const mk = r.num !== null && r.time ? [r.time, r.num] : null;
    h += `<tr><td>${mark(r.name, terms)}</td><td class="v ${cls}">${esc(r.value)}</td><td class="u">${esc(r.unit)}${badge}</td><td class="u">${esc(ref)}</td>
      ${vital ? '' : `<td class="num u">${esc(r.previous || '')}</td>`}<td>${spark(r.series, r.ref_low, r.ref_high, { w: 110, h: 22, stroke: color(`c-${d.category}`), mark: mk })}</td></tr>`;
  }
  return `${h}</table>`;
}

function notesList(notes, terms) {
  if (!notes.length) return '';
  const first = notes[0].key;
  return `<div class="notes"><div class="label">Notes around this time <span>${notes.length}</span></div>${notes.map((n) => `
    <div class="nt${n.key === first ? ' open' : ''}" data-note="${esc(n.key)}" tabindex="0" role="button"><span class="when">${esc(fmtShort(n.time))}<br>${esc(fmtHM(n.time))}</span>
      <div><b>${esc(n.title)}</b> <span class="by">${esc([n.author, n.service].filter(Boolean).join(' · '))}</span>
      ${n.text !== undefined ? `<p>${mark(n.text, terms)}</p>` : '<p>The text of notes is shown to users with the PHI permission.</p>'}</div></div>`).join('')}</div>`;
}

/**
 * An attached file: shown here when the page can (a picture, a PDF, a 12-lead ECG drawn by the
 * ECG viewer, a video), and opened in its viewer's tab (viewers/: ECG, PACS).
 */
function attachment(a) {
  const url = API + a.url;
  const open = a.viewer
    ? `<div class="ftools"><a class="btn primary" href="${esc(appHref(a.viewer, a, API, shown))}">Open in ${esc(APPS[a.viewer])} ↗</a><span class="faint">${esc(a.name)}</span></div>`
    : '';
  if (a.viewer === 'ecg' && ['wfdb', 'edf', 'json', 'xml', 'dicom'].includes(a.kind)) {
    // the ECG on paper, by the ECG viewer itself
    const p = new URLSearchParams({ src: new URL(API + a.src, location.href).pathname, name: a.name, theme: document.documentElement.dataset.theme || 'light', compact: '1' });
    return `<div class="attach ecg"><iframe src="viewers/ecg/#${esc(p.toString())}" title="${esc(a.name)}"></iframe></div>${open}`;
  }
  if (a.kind === 'image') return `<div class="attach"><a href="${esc(url)}" target="_blank" rel="noopener"><img src="${esc(url)}" alt="${esc(a.name)}"></a></div>${open || `<div class="faint">${esc(a.name)} · <a href="${esc(url)}" target="_blank" rel="noopener">open full size ↗</a></div>`}`;
  if (a.kind === 'pdf') return `<div class="attach"><iframe src="${esc(url)}" title="${esc(a.name)}"></iframe></div>${open}`;
  if (a.kind === 'video') return `<div class="attach"><video src="${esc(url)}" controls loop muted playsinline></video></div>${open}`;
  return open || `<div class="attach"><a class="btn" href="${esc(url)}" target="_blank" rel="noopener">Open ${esc(a.name)} ↗</a></div>`;
}

/** Render a detail (GET patients/{id}/detail).  ctx: { id, summary, context, terms, nav }. */
export function renderDetail(el, d, ctx) {
  shown = { patient: ctx.id, entry: d.key };
  const where = ctx.context(d.encounter, d.time);
  const when = d.end && d.end - d.time > 3600 ? `${fmtDate(d.time)} ${fmtHM(d.time)} → ${fmtShort(d.end)} ${fmtHM(d.end)}` : `${fmtDay(d.time)} · ${fmtHM(d.time)}`;
  const sub = [when, where, d.sources.length ? d.sources.join(', ') : ''].filter(Boolean).join(' · ');
  let h = `<div class="row"><div><h2><span class="sw ${catClass(d.category)}"></span> ${esc(d.title)}</h2><div class="sub">${esc(sub)}</div></div>
    <div class="nav2"><button class="btn" data-step="-1" title="Previous of this kind"${ctx.nav.prev ? '' : ' disabled'}>‹</button><button class="btn" data-step="1" title="Next of this kind"${ctx.nav.next ? '' : ' disabled'}>›</button></div></div>`;
  if (d.rows.length) {
    const ab = d.rows.filter((r) => r.level).length;
    if (ab) h += `<div><span class="badge">${ab} abnormal</span></div>`;
    h += rowsTable(d, ctx.terms);
  }
  if (d.monitoring) h += monitoring(d, ctx);
  if (d.fields.length) h += `<dl class="fields">${d.fields.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${mark(v, ctx.terms)}</dd>`).join('')}</dl>`;
  // free text may still name the patient: in Privacy Mode it is marked, as files are
  const warn = privacy.on && d.category === 'note' ? '<p class="idnote"><span class="idwarn" role="img" aria-label="May show identifiers">▲</span> Free text: it may name the patient (the de-identified view cannot remove names from it).</p>' : '';
  if (d.text) h += `${warn}<p class="notetext">${mark(d.text, ctx.terms)}</p>`;
  else if (d.category === 'note' && !phi()) h += '<p class="muted">The text of notes is shown to users with the PHI permission.</p>';
  if (d.attachment) h += attachment(d.attachment);
  h += notesList(d.notes, ctx.terms);
  el.innerHTML = h;
  el.scrollTop = 0;
  const ov = el.querySelector('.overview svg');
  if (ov && d.monitoring) {
    const m = d.monitoring;
    const r = m.recording;
    ov.addEventListener('click', (e) => {
      const box = ov.getBoundingClientRect();
      const t = el._timeAt(e.clientX - box.left);
      const v = viewerTarget(r.path, t - 30, t + 30);
      if (!v) return;
      if (v.ext) window.open(v.href, 'vitalstream-viewer');
      else go(v.href);
    });
  }
}

function monitoring(d, ctx) {
  const m = d.monitoring;
  const r = m.recording;
  const t0 = r.t_start ?? d.time;
  const t1 = r.t_end ?? (t0 + 3600);
  const recorded = ['Recorded', m.recorded !== null && m.recorded !== undefined ? `${Math.round(m.recorded * 100)} %` : fmtDuration(t1 - t0), m.recorded !== null ? `of ${fmtDuration(t1 - t0)}` : ''];
  // a heart rate found from the ECG (a Holter): its mean, and its range over the recording
  const hr = m.trends.find((x) => (m.derived || []).includes(x.name));
  const fin = (a) => (a || []).filter((v) => v !== null && isFinite(v));
  const stats = hr
    ? [
      recorded,
      ['Leads', m.waveforms.length ? m.waveforms.join(' · ') : '—', ''],
      ['Heart rate', fin(hr.v).length ? String(Math.round(fin(hr.v).reduce((a, b) => a + b, 0) / fin(hr.v).length)) : '—', 'bpm, mean'],
      ['Range', fin(hr.lo).length ? `${Math.round(Math.min(...fin(hr.lo)))}–${Math.round(Math.max(...fin(hr.hi)))}` : '—', 'bpm'],
    ]
    : [
      recorded,
      ['Waveforms', m.waveforms.length ? m.waveforms.join(' · ') : '—', ''],
      ['Vital signs', String(m.vitals.length || '—'), ''],
      ['Where', [r.unit, r.bed && `bed ${r.bed}`].filter(Boolean).join(' ') || '—', r.format || ''],
    ];
  let h = `<div class="stats">${stats.map(([k, v, s]) => `<div><small>${esc(k)}</small><b>${esc(v)}</b>${s ? ` <span>${esc(s)}</span>` : ''}</div>`).join('')}</div>`;
  const v0 = viewerTarget(r.path);
  if (m.trends.length) {
    const width = Math.max(360, Math.min(680, (ctx.width || 600) - 50));
    const o = overview(m.trends, m.gaps, t0, t1, { w: width, rowH: m.trends.length === 1 ? 130 : 46 });
    ctx.setTimeAt(o.timeAt);
    h += `<div class="overview"><div class="label">Whole recording <span>${v0 ? 'click to open the viewer at that moment' : ''}</span></div>${o.svg}</div>`;
  } else if (r.status !== 'ready') {
    h += `<p class="muted">Not indexed yet: its overview appears once indexing is done.</p>`;
  }
  if (v0) {
    const vs = viewerTarget(r.path, t0, Math.min(t1, t0 + 60));
    h += `<div class="ftools"><a class="btn primary" href="${esc(vs.href)}"${vs.ext ? ' target="vitalstream-viewer" rel="noopener"' : ''}>Open in VitalStream ↗</a><span class="faint">${esc(r.name)}</span></div>`;
  } else {
    h += '<p class="muted">No viewer is linked to the catalog.</p>';
  }
  return h;
}
