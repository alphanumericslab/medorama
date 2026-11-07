// VitalStream front end: state, data loading and interaction.

import { api } from './api.js';
import { LABEL_W, PAD_R, drawAxis, drawLanes, drawOverview, fmtVal, readTheme, repairedSpans } from './render.js';
import { escapeHtml, displayModality, infoContent } from './text.js';
import { fields, fmtDuration, fmtFull, fmtTime, toUnix, zoneName } from './timefmt.js';

const $ = (sel) => document.querySelector(sel);
const MODALITY_ORDER = ['ecg', 'pressure', 'spo2', 'resp', 'co2', 'gas', 'vent', 'eeg', 'icg', 'svo2'];
const MODALITY_NAMES = {
  ecg: 'ECG', pressure: 'Pressure', spo2: 'SpO₂ / pleth', resp: 'Respiration', co2: 'CO₂', gas: 'Gases',
  vent: 'Ventilator', eeg: 'EEG, EOG, EMG', icg: 'ICG', svo2: 'SvO₂', wfdb: 'Signals', edf: 'Signals',
};
const DENSITY = { compact: 0.68, normal: 1, tall: 1.6 };
const MARKER_SPAN_S = 120;          // show waveform markers in views up to this long
const MAX_WAVE_LANES = 64;          // waveform lanes shown at once (the others are listed to pick)
const MIN_SPAN_S = 0.02;

const store = {
  get(k, d) { try { const v = localStorage.getItem(`vitalstream.${k}`); return v === null ? d : JSON.parse(v); } catch (_) { return d; } },
  set(k, v) { try { localStorage.setItem(`vitalstream.${k}`, JSON.stringify(v)); } catch (_) { /* private mode */ } },
};

const s = {
  config: null,
  tz: 'UTC',
  ds: null,            // dataset description
  key: null,
  range: null,         // [t0, t1] of all data
  view: { t0: 0, t1: 1 },
  lanes: [],
  win: null,           // window response
  trends: null,
  markers: null,
  cursor: null,
  selection: null,
  loading: false,
  enabledCh: new Set(),
  enabledTr: [],
  modalityOrder: [],
  piecesByModality: {},
  cover: [],           // merged [t0, t1] spans with waveform samples (calendar marks)
  density: store.get('density', 'normal'),
};

let theme = null;
// the host's theme (#theme=light|dark, as Medorama passes it): applied, never saved as the
// viewer's own choice
let hostTheme = null;

// ------------------------------------------------------------------ setup
async function init() {

  // full screen: the top strip of the screen belongs to the system (macOS: the menu bar, beside
  // a camera notch too), and clicks there do not reach the page: the header starts below it
  // (only as the page itself: inside a host's frame, the host's bar is at the top)
  const fullscreen = () => document.body.classList.toggle('fullscreen', window.top === window && (matchMedia('(display-mode: fullscreen)').matches
    || (Math.abs(window.innerHeight - screen.height) <= 1 && Math.abs(window.innerWidth - screen.width) <= 1)));
  window.addEventListener('resize', fullscreen);
  fullscreen();
  applyTheme(store.get('theme', null));
  try {
    s.config = await api.config();
  } catch (e) {
    status(`Server not reachable: ${e.message}`, true);
    return;
  }
  setupTimezones();
  setupDensity();
  setupInteraction();
  setupBrowser();
  setupGoto();
  $('#show-all').onchange = (e) => showAll(e.target.checked);
  $('#btn-open').onclick = () => openDialog();
  $('#btn-open-2').onclick = () => openDialog();
  $('#btn-open-many').onclick = () => openDialog(undefined, 'many');
  $('#btn-open-many-2').onclick = () => openDialog(undefined, 'many');
  $('#btn-help').onclick = () => $('#dlg-help').showModal();
  $('#btn-theme').onclick = () => {
    applyTheme(document.documentElement.dataset.theme === 'light' ? 'dark' : 'light', true);
  };
  $('#trend-filter').oninput = renderTrendList;
  new ResizeObserver(() => draw()).observe($('#lanes-scroll'));
  window.addEventListener('hashchange', fromHash);
  if (!(await fromHash())) draw();
}

// a host's context (host.js: Medorama's patient and recording) in place of the file's name
function hosted(h) {
  document.body.classList.toggle('hosted', !!window.ViewerHost && window.ViewerHost.render($('#host'), h));
}

const systemTheme = () => (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');
matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
  if (!hostTheme && !store.get('theme', null)) applyTheme(null);
});

// t: 'light', 'dark', or null to follow the system setting.
function applyTheme(t, save = false) {
  document.documentElement.dataset.theme = t || systemTheme();
  if (save) store.set('theme', t);
  theme = readTheme();
  if (s.ds) { buildLanes(); renderChannelList(); }
  draw();
}

function setupTimezones() {
  const sel = $('#tz');
  const zones = typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : [];
  const def = s.config.timezone;
  const list = Array.from(new Set([def, 'UTC', ...zones]));
  sel.replaceChildren(...list.map((z) => new Option(z === def ? `${z} (default)` : z, z)));
  s.tz = store.get('tz', def);
  if (!list.includes(s.tz)) s.tz = def;
  sel.value = s.tz;
  sel.onchange = () => {
    s.tz = sel.value;
    store.set('tz', s.tz);
    renderInfo();
    draw();
    updateCursorPanel();
    if (!$('#goto').hidden) openGoto();
  };
}

function setupDensity() {
  const btns = document.querySelectorAll('[data-density]');
  const mark = () => btns.forEach((b) => b.classList.toggle('on', b.dataset.density === s.density));
  btns.forEach((b) => {
    b.onclick = () => { s.density = b.dataset.density; store.set('density', s.density); mark(); if (s.ds) buildLanes(); draw(); };
  });
  mark();
}

function status(msg, error = false) {
  const el = $('#status-msg');
  el.textContent = msg || '';
  el.style.fontWeight = error ? '600' : '';   // errors in bold: red is for alarms only
}

function busy(on) {
  $('#busy').hidden = !on;
}

// ------------------------------------------------------------ open dialog
let nav = null;               // current listing from /api/browse
// 'record': open one record (a WFDB record, an EDF file, or a file an add-on reads); 'many': open a folder,
// every record in it and in its subfolders, as one session
let mode = 'record';
let indexing = null;          // key of the index being built, while the progress bar shows

async function openDialog(path, how = 'record') {
  const dlg = $('#dlg-open');
  mode = how;
  $('#dlg-open-title').textContent = mode === 'many' ? 'Open multiple records' : 'Open record';
  dlg.classList.toggle('many', mode === 'many');
  if (!dlg.open) dlg.showModal();
  $('#open-progress').hidden = true;
  $('#scan-summary').hidden = true;
  renderPlaces();
  renderRecent();
  const start = path ?? store.get('lastDir', null) ?? s.config.start;
  if (!(await browse(start, true))) await browse(s.config.start);
  $('#path-input').focus();
}

function setupBrowser() {
  const input = $('#path-input');
  const box = $('#suggest');
  let timer = null;
  let ctl = null;
  let items = [];
  let on = -1;

  const hide = () => { box.hidden = true; on = -1; };
  const show = (list) => {
    items = list;
    on = -1;
    box.innerHTML = '';
    for (const [i, it] of list.entries()) {
      const li = document.createElement('li');
      li.innerHTML = `${escapeHtml(it.path)}<span class="k">${it.type === 'file' ? escapeHtml(it.label || '') : it.type === 'wfdb' ? 'WFDB' : ''}</span>`;
      li.onmousedown = (ev) => { ev.preventDefault(); pick(i); };
      box.append(li);
    }
    box.hidden = !list.length;
  };
  const mark = () => box.querySelectorAll('li').forEach((li, i) => {
    li.classList.toggle('on', i === on);
    if (i === on) li.scrollIntoView({ block: 'nearest' });
  });
  const fetchList = async () => {
    ctl?.abort();
    ctl = new AbortController();
    try { return (await api.complete(input.value, ctl.signal)).items; } catch (_) { return null; }
  };
  const pick = (i) => {
    const it = items[i];
    hide();
    if (it.type === 'file') openPath(it.path);
    else { input.value = it.path; browse(it.path); }
  };
  const commonPrefix = (list) => list.reduce((a, b) => {
    let k = 0;
    while (k < a.length && k < b.length && a[k].toLowerCase() === b[k].toLowerCase()) k++;
    return a.slice(0, k);
  });

  input.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(async () => { const list = await fetchList(); if (list) show(list); }, 120);
  });
  input.addEventListener('blur', () => setTimeout(hide, 100));
  input.addEventListener('keydown', async (e) => {
    if (e.key === 'Tab') {
      e.preventDefault();
      const list = await fetchList();
      if (!list || !list.length) return;
      const prefix = list.length === 1 ? list[0].path : commonPrefix(list.map((x) => x.path));
      if (prefix.length > input.value.length) input.value = prefix;
      const again = list.length === 1 ? await fetchList() : list;
      show(again || []);
    } else if (e.key === 'ArrowDown' && !box.hidden) {
      e.preventDefault(); on = Math.min(items.length - 1, on + 1); mark();
    } else if (e.key === 'ArrowUp' && !box.hidden) {
      e.preventDefault(); on = Math.max(-1, on - 1); mark();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (on >= 0) pick(on); else { hide(); browse(input.value); }
    } else if (e.key === 'Escape' && !box.hidden) {
      e.preventDefault(); hide();
    } else if (e.key === 'ArrowUp' && e.altKey && nav?.parent) {
      e.preventDefault(); browse(nav.parent);
    }
  });
  $('#path-go').onclick = () => { hide(); browse(input.value); };
  $('#nav-up').onclick = () => nav?.parent && browse(nav.parent);
  $('#list-filter').oninput = renderList;
  $('#list-filter').onkeydown = (e) => {
    if (e.key !== 'Enter') return;
    const shown = $('#browser').querySelectorAll('li[data-i]');
    if (shown.length === 1) shown[0].click();
  };
  $('#show-hidden').onchange = () => nav && browse(nav.path);
  $('#open-here').onclick = () => nav && openFolder(nav.path);
  $('#scan-back').onclick = () => { $('#scan-summary').hidden = true; };
  $('#open-cancel').onclick = async () => {
    if (!indexing) return;
    $('#open-cancel').disabled = true;
    try { await api.cancel(indexing); } catch (_) { /* already finished */ }
  };
}

// A folder (Open multiple records): what it holds first, from the headers alone; opened when
// confirmed.  Over a limit, the server says which, and what to do instead.
async function openFolder(path) {
  const box = $('#scan-summary');
  browseMessage('Counting the records…');
  let r;
  try {
    r = await api.scan(path);
  } catch (e) {
    box.hidden = true;
    browseMessage(e.message, true);
    return;
  }
  browseMessage('');
  const n = (k, one, many) => `${escapeHtml(Number(k).toLocaleString())} ${k === 1 ? one : many}`;
  const parts = [`<b>${n(r.records, 'record', 'records')}</b>`];
  if (r.segments) parts.push(`${n(r.segments, 'segment', 'segments')} of multi-segment records`);
  parts.push(`${n(r.signals.length, 'signal', 'signals')}: ${escapeHtml(r.signals.slice(0, 12).join(', '))}${r.signals.length > 12 ? ' …' : ''}`);
  const lines = [parts.join(' · ')];
  if (r.t_range) {
    lines.push(`From ${escapeHtml(fmtFull(r.t_range[0], s.tz))} to ${escapeHtml(fmtFull(r.t_range[1], s.tz))} `
      + `(${escapeHtml(fmtDuration(r.t_range[1] - r.t_range[0]))}), ${escapeHtml(fmtDuration(r.duration_s))} of data in all`);
  } else {
    lines.push(`${escapeHtml(fmtDuration(r.duration_s))} of data; no record states its time`);
  }
  if (r.untimed && r.t_range) lines.push(`${n(r.untimed, 'record states', 'records state')} no time: placed after the data before`);
  if (r.skipped) lines.push(`${n(r.skipped, 'record cannot', 'records cannot')} be read and will be left out`);
  lines.push(`${fmtBytes(r.bytes)} to index once; later openings are instant.`);
  box.querySelector('.msg').innerHTML = lines.map((l) => `<div>${l}</div>`).join('');
  box.hidden = false;
  $('#scan-open').onclick = () => { box.hidden = true; openPath(path); };
  $('#scan-open').focus();
}

function browseMessage(text, error = false) {
  const el = $('#browse-msg');
  el.hidden = !text;
  el.textContent = text || '';
  el.classList.toggle('error', error);
}

// List a folder (or open a recording file).  Returns false if the path could not be shown.
async function browse(path, quiet = false) {
  let r;
  try {
    r = await api.browse(path, $('#show-hidden').checked);
  } catch (e) {
    if (!quiet) browseMessage(e.message, true);
    return false;
  }
  if (r.type === 'file') { openPath(r.path); return true; }
  nav = r;
  store.set('lastDir', r.path);
  const dirs = [r.path, ...store.get('recentDirs', []).filter((d) => d !== r.path)].slice(0, 6);
  store.set('recentDirs', dirs);
  $('#path-input').value = r.path;
  $('#list-filter').value = '';
  $('#nav-up').disabled = !r.parent;
  $('#open-here').hidden = mode !== 'many';
  $('#scan-summary').hidden = true;
  browseMessage(r.truncated ? `Showing the first ${r.entries.length} entries; type a path or use the filter to narrow down.` : '');
  renderCrumbs(r.path);
  renderList();
  renderPlaces();
  return true;
}

function renderCrumbs(path) {
  const sep = s.config.sep || '/';
  const crumbs = $('#crumbs');
  crumbs.innerHTML = '';
  const items = [];
  let acc = '';
  if (path.startsWith(sep)) { acc = sep; items.push([sep, sep]); }
  for (const part of path.split(sep).filter(Boolean)) {
    acc = acc === '' ? part + sep : acc.endsWith(sep) ? acc + part : acc + sep + part;  // 'C:' -> 'C:\'
    items.push([part, acc]);
  }
  items.forEach(([label, target], i) => {
    if (i) crumbs.append(' › ');
    const a = document.createElement('a');
    a.textContent = label;
    a.title = target;
    a.onclick = () => browse(target);
    crumbs.append(a);
  });
}

function renderList() {
  const ul = $('#browser');
  ul.innerHTML = '';
  if (!nav) return;
  const f = $('#list-filter').value.trim().toLowerCase();
  if (nav.parent && !f) {
    const li = document.createElement('li');
    li.innerHTML = '<span class="ic">↰</span><span>..</span>';
    li.onclick = () => browse(nav.parent);
    ul.append(li);
  }
  const shown = nav.entries.filter((e) => !f || e.name.toLowerCase().includes(f));
  if (!shown.length) {
    const li = document.createElement('li');
    li.className = 'muted';
    li.textContent = f ? 'Nothing matches the filter.' : 'No folders or recordings here.';
    ul.append(li);
  }
  for (const e of shown) {
    const li = document.createElement('li');
    li.className = e.type;
    li.dataset.i = '1';
    const icon = { dir: '📁', file: '◆', record: '∿', wfdb: '≋', broken: '⚠' }[e.type];
    const tag = e.type === 'file' ? `${escapeHtml(e.label || e.kind)} · ${escapeHtml(fmtBytes(e.size))}`
      : e.type === 'record' ? escapeHtml(e.label) : e.type === 'wfdb' ? 'WFDB records'
        : e.type === 'broken' ? 'broken link' : '';
    const folder = e.type === 'dir' || e.type === 'wfdb';
    li.innerHTML = `<span class="ic">${icon}</span><span>${escapeHtml(e.name)}</span>`
      + `${e.link ? '<span class="lnk" title="symbolic link">↪ link</span>' : ''}<span class="tag">${tag}</span>`
      + (mode === 'many' && folder ? '<button class="btn open-btn" title="Every record in this folder and its subfolders, as one session">Open</button>' : '');
    // folders are entered with a click; records and recording files open with a click;
    // in Open multiple records, a folder's Open button opens its records as one session
    if (e.type === 'file' || e.type === 'record') li.onclick = () => openPath(e.path);
    else if (e.type !== 'broken') li.onclick = () => browse(e.path);
    const btn = li.querySelector('.open-btn');
    if (btn) btn.onclick = (ev) => { ev.stopPropagation(); openFolder(e.path); };
    ul.append(li);
  }
}

function renderPlaces() {
  const el = $('#places');
  el.innerHTML = '';
  const seen = new Set();
  const add = (label, path, cls = '') => {
    if (seen.has(path)) return;
    seen.add(path);
    const a = document.createElement('a');
    a.textContent = label;
    a.title = path;
    if (cls) a.className = cls;
    a.onclick = () => browse(path);
    el.append(a);
  };
  for (const p of s.config.places || []) add(p.name === 'Home' ? '⌂ Home' : p.name, p.path);
  for (const d of store.get('recentDirs', [])) {
    if (nav && d === nav.path) continue;
    add(d.split(s.config.sep || '/').filter(Boolean).pop() || d, d, 'recentdir');
  }
}

function renderRecent() {
  const recent = store.get('recent', []);
  $('#recent').innerHTML = recent.length
    ? `<div class="muted head">Recent recordings</div>${recent.map((p) => `<a data-path="${escapeAttr(p)}" title="${escapeAttr(p)}">${escapeHtml(p)}</a>`).join('')}`
    : '';
  $('#recent').querySelectorAll('a').forEach((a) => { a.onclick = () => openPath(a.dataset.path); });
}

async function openPath(path, view = null) {
  const dlg = $('#dlg-open');
  const prog = $('#open-progress');
  try {
    status(`Opening ${path}…`);
    busy(true);
    let r = await api.open(path);
    if (r.state !== 'ready') {
      if (!dlg.open) dlg.showModal();
      prog.hidden = false;
      indexing = r.key;
      $('#open-cancel').disabled = false;
      for (;;) {
        const d = await api.dataset(r.key);
        if (d.state === 'ready') { r = d; break; }
        if (d.state === 'error') throw new Error(d.error);
        if (d.state === 'cancelled') {
          status('');
          prog.querySelector('.msg').textContent = 'Cancelled: nothing was kept.';
          $('#open-cancel').disabled = true;
          return;
        }
        const f = d.progress?.fraction ?? 0;
        prog.querySelector('.bar span').style.width = `${(f * 100).toFixed(1)}%`;
        const el = d.progress?.elapsed_s;
        const eta = f > 0.02 && el ? ` · about ${fmtDuration((el / f) * (1 - f))} left` : '';
        prog.querySelector('.msg').textContent = `Indexing ${path.split('/').pop()} … ${(f * 100).toFixed(0)} %${eta}. `
          + 'This is done once; the index is cached.';
        await sleep(600);
      }
      r = { key: r.dataset.key, dataset: r.dataset };
    } else {
      r = { key: r.key, dataset: (await api.dataset(r.key)).dataset };
    }
    if (dlg.open) dlg.close();
    prog.hidden = true;
    const recent = [path, ...store.get('recent', []).filter((p) => p !== path)].slice(0, 8);
    store.set('recent', recent);
    load(r.key, r.dataset, view);
    status('');
  } catch (e) {
    status(`Could not open: ${e.message}`, true);
    prog.querySelector('.msg').textContent = `Error: ${e.message}`;
  } finally {
    indexing = null;
    busy(false);
  }
}

// ------------------------------------------------------------ dataset load
function load(key, ds, view) {
  for (const c of ds.channels) c.modality = displayModality(c);
  s.key = key;
  s.ds = ds;
  s.win = s.trends = s.markers = null;
  s.cursor = null;
  s.range = ds.t_range[0] !== null ? [ds.t_range[0], ds.t_range[1]] : [0, 1];
  if (s.range[1] <= s.range[0]) s.range[1] = s.range[0] + 1;
  // channel order: by modality, then first appearance
  const mods = new Set(ds.channels.map((c) => c.modality));
  s.modalityOrder = [...MODALITY_ORDER.filter((m) => mods.has(m)), ...[...mods].filter((m) => !MODALITY_ORDER.includes(m)).sort()];
  s.piecesByModality = {};
  const chMod = Object.fromEntries(ds.channels.map((c) => [c.id, c.modality]));
  for (const p of ds.pieces) (s.piecesByModality[chMod[p.ch]] ||= []).push(p);
  const fsOf = {};
  for (const p of ds.pieces) { const f = fsOf[p.ch] ||= {}; f[p.fs] = (f[p.fs] || 0) + p.n; }
  for (const c of ds.channels) {
    const f = fsOf[c.id] || {};
    c.fs = +Object.keys(f).sort((a, b) => f[b] - f[a])[0] || null;
    c.samples = Object.values(f).reduce((a, b) => a + b, 0);
  }
  s.cover = coverage(ds.pieces);
  s.enabledCh = firstLanes(ds);
  s.enabledTr = defaultTrends(ds.vitals.map((v) => v.name));
  $('#btn-goto').disabled = false;
  closeGoto();
  $('#source').innerHTML = `<b>${escapeHtml(ds.name)}</b> · ${escapeHtml(formatName(ds))}`;
  $('#source').title = ds.path;
  $('#empty').hidden = true;
  renderChannelList();
  renderTrendList();
  syncShowAll();
  renderInfo();
  buildLanes();
  if (view) setView(view[0], view[1]);
  else setView(s.range[0], s.range[1]);
}

// The recording's format as the reader or adapter names it, with its own description.
function formatName(ds) {
  return [ds.format, ds.description].filter(Boolean).join(' · ');
}

// Merged time spans covered by waveform pieces.
function coverage(pieces) {
  const spans = pieces.filter((p) => p.t0 !== null && p.fs).map((p) => [p.t0, p.t0 + p.n / p.fs]).sort((a, b) => a[0] - b[0]);
  const out = [];
  for (const [a, b] of spans) {
    const last = out[out.length - 1];
    if (last && a <= last[1]) last[1] = Math.max(last[1], b); else out.push([a, b]);
  }
  return out;
}

// The waveforms a recording opens with: every channel, in lane order, up to MAX_WAVE_LANES.
function firstLanes(ds) {
  const ids = s.modalityOrder.flatMap((m) => ds.channels.filter((c) => c.modality === m).map((c) => c.id));
  if (ids.length > MAX_WAVE_LANES) {
    status(`${ids.length} waveforms: the first ${MAX_WAVE_LANES} are shown; pick others under Waveforms.`);
  }
  return new Set(ids.slice(0, MAX_WAVE_LANES));
}

// "Show all": every waveform (up to MAX_WAVE_LANES) and every trend; off: the selection a
// recording opens with.
function showAll(on) {
  if (!s.ds) return;
  s.enabledCh = firstLanes(s.ds);
  s.enabledTr = on ? s.ds.vitals.map((v) => v.name) : defaultTrends(s.ds.vitals.map((v) => v.name));
  renderChannelList();
  renderTrendList();
  buildLanes();
  draw();
  scheduleFetch();
}

function syncShowAll() {
  const el = $('#show-all');
  el.disabled = !s.ds;
  el.checked = !!s.ds && s.enabledCh.size === Math.min(MAX_WAVE_LANES, s.ds.channels.length)
    && s.enabledTr.length === s.ds.vitals.length;
}

function defaultTrends(names) {
  const pats = [/^HR$/i, /^(SPO2-%|SpO₂|SpO2)$/i, /^(RR|RESP|awRR|RR-IMP)$/i, /^(ART|ABP|AR\d?|P\d)[- ]?(M|MEAN)$/i,
    /^(NBP|NBPa|NIBP)[- ]?(M|MEAN|m)$/i];
  const out = [];
  for (const p of pats) {
    const n = names.find((x) => p.test(x) && !out.includes(x));
    if (n) out.push(n);
  }
  return out;
}

function buildLanes() {
  const ds = s.ds;
  const k = DENSITY[s.density] || 1;
  const lanes = [];
  // a recording's events: alarms, or (EDF+) annotations, which are not alarms
  const notes = ds.alarms.length && ds.alarms.every((a) => a.kind === 'annotation');
  if (ds.alarms.length) lanes.push({ type: 'alarm', title: notes ? 'Annotations' : 'Alarms', sub: `${ds.alarms.length} ${notes ? 'events' : 'intervals'}`, h: Math.round(44 * Math.max(0.8, k)), color: theme['alarm-2'] });
  for (const m of s.modalityOrder) {
    for (const c of ds.channels.filter((x) => x.modality === m && s.enabledCh.has(x.id))) {
      lanes.push({
        type: 'wave', ch: c.id, title: c.label, sub: `${c.unit || 'adu'}${c.fs ? ` · ${+c.fs.toFixed(3)} Hz` : ''}`,
        h: Math.round((m === 'ecg' ? 92 : 80) * k), color: theme.modality(m), unit: c.unit,
      });
    }
  }
  for (const name of s.enabledTr) {
    const v = ds.vitals.find((x) => x.name === name);
    if (!v) continue;
    lanes.push({ type: 'trend', name, title: name, sub: v.unit || '', h: Math.round(62 * k), color: theme['c-trend'], unit: v.unit });
  }
  for (const lane of lanes) lane.readout = readoutFor(lane);
  s.lanes = lanes;
}

function readoutFor(lane) {
  if (lane.type === 'wave') {
    return (t) => {
      const blocks = s.win?.channels[lane.ch] || [];
      for (const b of blocks) {
        const n = b.kind === 'raw' ? b.y.length : b.min.length;
        if (b.kind === 'raw') {
          const i = Math.round((t - b.t0) / b.dt);
          if (i >= 0 && i < n) {
            const v = b.y[i];
            return v === null ? 'no value' : `${fmtVal(v)} ${lane.unit || ''}`;
          }
        } else {
          const i = Math.floor((t - b.t0) / b.dt);
          if (i >= 0 && i < n) {
            return b.min[i] === null ? 'no value' : `${fmtVal(b.min[i])} … ${fmtVal(b.max[i])} ${lane.unit || ''}`;
          }
        }
      }
      return null;
    };
  }
  if (lane.type === 'trend') {
    return (t) => {
      const tr = s.trends?.[lane.name];
      if (!tr || !tr.t.length) return null;
      let lo = 0;
      let hi = tr.t.length - 1;
      if (t < tr.t[0]) return null;
      while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (tr.t[mid] <= t) lo = mid; else hi = mid - 1; }
      if (t - tr.t[lo] > 900) return null;
      const v = tr.kind === 'env' ? `${fmtNum(tr.min[lo])} … ${fmtNum(tr.max[lo])}` : fmtNum(tr.v[lo]);
      return `${v} ${lane.unit || ''}`;
    };
  }
  return null;
}

// ------------------------------------------------------------ sidebar
function renderChannelList() {
  const el = $('#channel-list');
  el.innerHTML = '';
  if (!s.ds.channels.length) {
    el.innerHTML = '<div class="muted">No waveforms with a stated sampling rate.</div>';
    return;
  }
  for (const m of s.modalityOrder) {
    const g = document.createElement('div');
    g.className = 'group';
    g.textContent = MODALITY_NAMES[m] || m;
    el.append(g);
    for (const c of s.ds.channels.filter((x) => x.modality === m)) {
      const lab = document.createElement('label');
      lab.innerHTML = `<input type="checkbox" ${s.enabledCh.has(c.id) ? 'checked' : ''}>
        <span class="sw"></span>
        <span>${escapeHtml(c.label)}</span><span class="meta">${escapeHtml(c.unit || 'adu')}${c.fs ? ` · ${+(+c.fs).toFixed(3)} Hz` : ''}</span>`;
      lab.querySelector('.sw').style.background = theme.modality(m);   // CSSOM: no style attribute (CSP)
      lab.querySelector('input').onchange = (ev) => {
        if (ev.target.checked && s.enabledCh.size >= MAX_WAVE_LANES) {
          ev.target.checked = false;
          status(`At most ${MAX_WAVE_LANES} waveform lanes at once: untick one first.`);
          return;
        }
        if (ev.target.checked) s.enabledCh.add(c.id); else s.enabledCh.delete(c.id);
        syncShowAll();
        buildLanes();
        draw();
        scheduleFetch();
      };
      el.append(lab);
    }
  }
}

function renderTrendList() {
  if (!s.ds) return;
  const el = $('#trend-list');
  const f = $('#trend-filter').value.trim().toLowerCase();
  el.innerHTML = '';
  const items = s.ds.vitals.filter((v) => !f || v.name.toLowerCase().includes(f));
  if (!s.ds.vitals.length) el.innerHTML = '<div class="muted">No vital signs.</div>';
  for (const v of items) {
    const lab = document.createElement('label');
    lab.innerHTML = `<input type="checkbox" ${s.enabledTr.includes(v.name) ? 'checked' : ''}>
      <span>${escapeHtml(v.name)}</span><span class="meta">${escapeHtml(v.unit || '')} · ${escapeHtml(Number(v.n).toLocaleString())}</span>`;
    lab.querySelector('input').onchange = (ev) => {
      if (ev.target.checked) s.enabledTr.push(v.name); else s.enabledTr = s.enabledTr.filter((x) => x !== v.name);
      syncShowAll();
      buildLanes();
      draw();
      scheduleFetch();
    };
    el.append(lab);
  }
}

function renderInfo() {
  const ds = s.ds;
  if (!ds) return;
  // text only (textContent): names and reasons come from the files (a skipped record's reason
  // holds its path)
  const { rows, notes } = infoContent(ds, s.range, s.tz);
  const el = $('#info');
  el.replaceChildren();
  for (const [a, b] of rows) {
    const row = document.createElement('div');
    row.className = 'row';
    const k = document.createElement('span');
    k.textContent = a;
    const v = document.createElement('span');
    v.className = 'v';
    v.textContent = b;
    row.append(k, v);
    el.append(row);
  }
  for (const n of notes) {
    const d = document.createElement('div');
    d.className = 'note';
    d.textContent = n;
    el.append(d);
  }
}

let vitalsTimer = null;
let vitalsCtl = null;
function updateCursorPanel() {
  const t = s.cursor ?? (s.ds ? (s.view.t0 + s.view.t1) / 2 : null);
  if (t === null) return;
  $('#cursor-time').textContent = fmtFull(t, s.tz, s.view.t1 - s.view.t0);
  clearTimeout(vitalsTimer);
  vitalsTimer = setTimeout(async () => {
    if (!s.key || !s.ds.vitals.length) { $('#vitals-now').innerHTML = ''; return; }
    vitalsCtl?.abort();
    vitalsCtl = new AbortController();
    try {
      const list = await api.vitalsAt(s.key, t, vitalsCtl.signal);
      const order = new Map(s.ds.vitals.map((v, i) => [v.name, i]));
      list.sort((a, b) => (s.enabledTr.includes(b.name) - s.enabledTr.includes(a.name)) || order.get(a.name) - order.get(b.name));
      $('#vitals-now').innerHTML = list.length
        ? list.map((v) => `<tr><td>${escapeHtml(v.name)}</td><td class="v">${fmtNum(v.value)}</td><td class="u">${escapeHtml(v.unit || '')}</td>
            <td class="age" title="${escapeAttr(fmtFull(v.t, s.tz, 3600))}">${t - v.t < 1 ? 'now' : `−${fmtDuration(t - v.t)}`}</td></tr>`).join('')
        : '<tr><td class="muted">No values in the 10 minutes before.</td></tr>';
    } catch (e) {
      if (e.name !== 'AbortError') status(e.message, true);
    }
  }, 120);
}

// ------------------------------------------------------------ view + data
function plotWidth() {
  return Math.max(50, $('#lanes').clientWidth - LABEL_W - PAD_R);
}

function setView(t0, t1) {
  const full = s.range[1] - s.range[0];
  const span = Math.min(Math.max(t1 - t0, MIN_SPAN_S), full * 1.2 + 10);
  const lo = s.range[0] - full * 0.1;          // allowed centre range: the data plus a margin
  const hi = s.range[1] + full * 0.1;
  let c = (t0 + t1) / 2;
  c = span >= hi - lo ? (lo + hi) / 2 : Math.min(Math.max(c, lo + span / 2), hi - span / 2);
  s.view = { t0: c - span / 2, t1: c + span / 2 };
  draw();
  scheduleFetch();
  saveHash();
  if (s.cursor === null) updateCursorPanel();
}

let fetchTimer = null;
let fetchCtl = null;
function scheduleFetch() {
  clearTimeout(fetchTimer);
  fetchTimer = setTimeout(fetchData, 60);
}

async function fetchData() {
  if (!s.key) return;
  fetchCtl?.abort();
  const ctl = new AbortController();
  fetchCtl = ctl;
  const { t0, t1 } = s.view;
  const span = t1 - t0;
  // fetch a margin on each side so that short pans need no new data
  const a = t0 - span * 0.25;
  const b = t1 + span * 0.25;
  const px = Math.round(plotWidth() * 1.5);
  const chs = s.lanes.filter((l) => l.type === 'wave').map((l) => l.ch);
  const trs = s.lanes.filter((l) => l.type === 'trend').map((l) => l.name);
  s.loading = true;
  busy(true);
  const t = performance.now();
  try {
    const [win, trends, markers] = await Promise.all([
      chs.length ? api.window(s.key, a, b, px, chs, ctl.signal) : null,
      trs.length ? api.vitals(s.key, a, b, px, trs, ctl.signal) : null,
      chs.length && span <= MARKER_SPAN_S && s.ds.markers.count ? api.markers(s.key, a, b, chs, ctl.signal) : null,
    ]);
    if (ctl !== fetchCtl) return;
    s.win = win;
    s.trends = trends;
    s.markers = markers ? groupMarkers(markers) : null;
    s.loading = false;
    busy(false);
    status(`updated in ${Math.round(performance.now() - t)} ms`);
    draw();
  } catch (e) {
    if (e.name === 'AbortError') return;
    s.loading = false;
    busy(false);
    status(e.message, true);
  }
}

function groupMarkers(m) {
  const byCh = {};
  for (const [ch, t, code] of m.items) (byCh[ch] ||= []).push([t, code]);
  const short = m.names.map((n) => ({ R_WAVE: 'R', P_WAVE: 'P', PACE: 'pace', NO_DISP: '', DEFIB: 'defib' }[n] ?? n));
  return { byCh, short, names: m.names };
}

// ------------------------------------------------------------ drawing
let frame = 0;
function draw() {
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    if (!theme) theme = readTheme();
    drawOverview($('#overview'), s, theme);
    drawAxis($('#axis'), s, theme);
    drawLanes($('#lanes'), s, theme);
    const span = s.view.t1 - s.view.t0;
    if (s.ds) {
      const fastest = Math.max(0, ...s.lanes.filter((l) => l.type === 'wave').map((l) => s.ds.channels[l.ch].fs || 0));
      const spp = fastest ? (fastest * span) / plotWidth() : 0;
      $('#status-view').textContent = `${fmtFull(s.view.t0, s.tz, span)} → ${fmtTime(s.view.t1, s.tz, span)} · ${fmtDuration(span)}`
        + (fastest ? ` · ${spp < 1 ? `${(1 / spp).toFixed(1)} px/sample` : `${spp.toFixed(spp < 10 ? 1 : 0)} samples/px`}` : '');
    }
  });
}

// ------------------------------------------------------------ interaction
function tAt(clientX) {
  const r = $('#lanes').getBoundingClientRect();
  const x = clientX - r.left - LABEL_W;
  return s.view.t0 + (x / plotWidth()) * (s.view.t1 - s.view.t0);
}

function zoomAt(t, factor) {
  const { t0, t1 } = s.view;
  setView(t - (t - t0) * factor, t + (t1 - t) * factor);
}

function setupInteraction() {
  const cv = $('#lanes');
  let drag = null;
  cv.addEventListener('pointerdown', (e) => {
    if (!s.ds || e.button !== 0) return;
    const r = cv.getBoundingClientRect();
    if (e.clientX - r.left < LABEL_W) return;
    cv.setPointerCapture(e.pointerId);
    const t = tAt(e.clientX);
    drag = e.shiftKey ? { mode: 'select', t } : { mode: 'pan', x: e.clientX, view: { ...s.view }, moved: false };
    if (drag.mode === 'select') s.selection = [t, t];
  });
  cv.addEventListener('pointermove', (e) => {
    if (!s.ds) return;
    const r = cv.getBoundingClientRect();
    const inPlot = e.clientX - r.left >= LABEL_W;
    s.cursor = inPlot || drag ? tAt(e.clientX) : null;
    if (drag?.mode === 'pan') {
      const dt = ((e.clientX - drag.x) / plotWidth()) * (drag.view.t1 - drag.view.t0);
      if (Math.abs(e.clientX - drag.x) > 2) drag.moved = true;
      setView(drag.view.t0 - dt, drag.view.t1 - dt);
      cv.style.cursor = 'grabbing';
    } else if (drag?.mode === 'select') {
      s.selection[1] = tAt(e.clientX);
    }
    tooltip(e, inPlot && !drag);
    draw();
    updateCursorPanel();
  });
  const end = (e) => {
    if (drag?.mode === 'select') {
      const [a, b] = s.selection;
      s.selection = null;
      if (Math.abs(b - a) / (s.view.t1 - s.view.t0) * plotWidth() > 5) setView(Math.min(a, b), Math.max(a, b));
    }
    drag = null;
    cv.style.cursor = '';
    draw();
  };
  cv.addEventListener('pointerup', end);
  cv.addEventListener('pointercancel', end);
  cv.addEventListener('pointerleave', () => { if (!drag) { s.cursor = null; $('#tooltip').hidden = true; draw(); } });
  cv.addEventListener('dblclick', (e) => {
    if (s.ds && e.clientX - cv.getBoundingClientRect().left >= LABEL_W) setView(s.range[0], s.range[1]);
  });
  cv.addEventListener('wheel', (e) => {
    if (!s.ds) return;
    const r = cv.getBoundingClientRect();
    if (e.clientX - r.left < LABEL_W) return;            // scroll the lanes
    e.preventDefault();
    const span = s.view.t1 - s.view.t0;
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      const dt = (e.deltaX / plotWidth()) * span;
      setView(s.view.t0 + dt, s.view.t1 + dt);
    } else {
      const k = e.ctrlKey ? 0.01 : e.deltaMode === 1 ? 0.05 : 0.0016;
      zoomAt(tAt(e.clientX), Math.exp(e.deltaY * k));
    }
  }, { passive: false });

  // overview: click or drag to move the view
  const ov = $('#overview');
  let ovDrag = false;
  const ovMove = (e) => {
    const r = ov.getBoundingClientRect();
    const pw = r.width - LABEL_W - PAD_R;
    const t = s.range[0] + ((e.clientX - r.left - LABEL_W) / pw) * (s.range[1] - s.range[0]);
    const span = s.view.t1 - s.view.t0;
    setView(t - span / 2, t + span / 2);
  };
  ov.addEventListener('pointerdown', (e) => { if (!s.ds) return; ovDrag = true; ov.setPointerCapture(e.pointerId); ovMove(e); });
  ov.addEventListener('pointermove', (e) => { if (ovDrag) ovMove(e); });
  ov.addEventListener('pointerup', () => { ovDrag = false; });
  ov.addEventListener('wheel', (e) => {
    if (!s.ds) return;
    e.preventDefault();
    zoomAt((s.view.t0 + s.view.t1) / 2, Math.exp(e.deltaY * 0.002));
  }, { passive: false });

  window.addEventListener('keydown', (e) => {
    if (e.target.closest('input:not([type=checkbox]), select, textarea') || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'o' || e.key === 'O') { e.preventDefault(); openDialog(undefined, e.shiftKey ? 'many' : 'record'); return; }
    if (e.key === '?') { $('#dlg-help').showModal(); return; }
    if ((e.key === 'g' || e.key === 'G') && s.ds) { e.preventDefault(); openGoto(); return; }
    if (!s.ds || document.querySelector('dialog[open]')) return;
    const { t0, t1 } = s.view;
    const span = t1 - t0;
    const c = s.cursor ?? (t0 + t1) / 2;
    const act = {
      ArrowLeft: () => setView(t0 - span * 0.1, t1 - span * 0.1),
      ArrowRight: () => setView(t0 + span * 0.1, t1 + span * 0.1),
      PageUp: () => setView(t0 - span, t1 - span),
      PageDown: () => setView(t0 + span, t1 + span),
      '+': () => zoomAt(c, 0.5),
      '=': () => zoomAt(c, 0.5),
      '-': () => zoomAt(c, 2),
      Home: () => setView(s.range[0], s.range[1]),
      1: () => setView(c - 5, c + 5),
      2: () => setView(c - 30, c + 30),
      3: () => setView(c - 300, c + 300),
    }[e.key];
    if (act) { e.preventDefault(); act(); }
  });
}

function tooltip(e, show) {
  const tip = $('#tooltip');
  if (!show || !s.ds) { tip.hidden = true; return; }
  const cv = $('#lanes');
  const r = cv.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  const X = (t) => LABEL_W + ((t - s.view.t0) / (s.view.t1 - s.view.t0)) * plotWidth();
  const span = s.view.t1 - s.view.t0;
  let text = null;
  const lane = s.lanes.find((l) => y >= l.y && y < l.y + l.h);
  if (lane?.type === 'alarm' && lane.hits) {
    const hit = lane.hits.find(([a, b, ya, yb]) => x >= a - 2 && x <= b + 2 && y >= ya && y <= yb);
    if (hit) {
      const a = hit[4];
      text = `${a.text}\n${a.kind}${a.level !== null && a.level !== undefined ? ` · level ${a.level}` : ''}\n`
        + `${fmtFull(a.t0, s.tz, 3600)}\n→ ${fmtFull(a.t1, s.tz, 3600)} (${fmtDuration(a.t1 - a.t0)})`;
    }
  }
  if (!text) {
    const g = s.ds.gaps.find((gg) => x >= X(gg.t0) - 3 && x <= Math.max(X(gg.t1), X(gg.t0) + 2) + 3
      && (!gg.channels || (lane?.type === 'wave' && gg.channels.includes(lane.ch))));
    if (g) {
      text = `Break in the data: ${g.reason}\n`
        + (g.gap_s !== null ? `gap ${fmtDuration(g.gap_s)}${g.exact ? ' (exact)' : ''} · measured by ${g.measured_by}\n` : 'gap duration unknown\n')
        + `${fmtTime(g.t0, s.tz, span)} → ${fmtTime(g.t1, s.tz, span)}${g.notes ? `\n${g.notes}` : ''}`;
    }
  }
  if (!text) {
    const r = repairedSpans(s).find((g) => x >= X(g.t0) - 2 && x <= X(g.t1) + 2);
    if (r) {
      text = `Data block stored late in the file (${r.late_by_s} s`
        + `${r.late_by_blocks !== null && r.late_by_blocks !== undefined ? `, ${r.late_by_blocks} blocks` : ''}); `
        + 'the reader put it back in order.\nNot a break: shown because qc_verbose is on.';
    }
  }
  if (!text) {
    const u = s.ds.untimed.find((uu) => x >= X(uu.t0) && x <= X(uu.t1));
    if (u) text = 'Time not stated in the file for this data.\nPlaced next to the data before it; the sample timing within it is exact.';
  }
  if (!text) { tip.hidden = true; return; }
  tip.textContent = text;
  tip.hidden = false;
  const vr = $('.viewer').getBoundingClientRect();
  const left = Math.min(e.clientX - vr.left + 14, vr.width - tip.offsetWidth - 8);
  tip.style.left = `${left}px`;
  tip.style.top = `${e.clientY - vr.top + 14}px`;
}

// ------------------------------------------------------------ go to a date and time
const gt = { y: 0, mo: 0, sel: null };       // month shown, day picked ({y, mo, d}), display zone
const pad2 = (n, w = 2) => String(n).padStart(w, '0');

function setupGoto() {
  const pop = $('#goto');
  $('#btn-goto').onclick = () => (pop.hidden ? openGoto() : closeGoto());
  $('#cal-prev').onclick = () => shiftMonth(-1);
  $('#cal-next').onclick = () => shiftMonth(1);
  $('#goto-start').onclick = () => openGoto(s.range[0]);
  $('#goto-end').onclick = () => openGoto(s.range[1]);
  $('#goto-go').onclick = goTo;
  pop.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); goTo(); }
    if (e.key === 'Escape') { e.preventDefault(); closeGoto(); $('#btn-goto').focus(); }
  });
  document.addEventListener('pointerdown', (e) => {
    if (!pop.hidden && !e.target.closest('.goto-wrap')) closeGoto();
  });
}

// Opens the picker at time t (default: the cursor, else the middle of the view).
function openGoto(t) {
  if (!s.ds) return;
  t = Math.min(Math.max(t ?? s.cursor ?? (s.view.t0 + s.view.t1) / 2, s.range[0]), s.range[1]);
  const f = fields(t, s.tz);
  Object.assign(gt, { y: f.y, mo: f.mo, sel: { y: f.y, mo: f.mo, d: f.d } });
  $('#goto-time').value = `${pad2(f.h)}:${pad2(f.mi)}:${pad2(f.s)}.${pad2(f.ms, 3)}`;
  $('#goto-tz').textContent = zoneName(t, s.tz);
  gotoMessage(`Recording: ${fmtFull(s.range[0], s.tz, 3600)}\n→ ${fmtFull(s.range[1], s.tz, 3600)}`);
  renderCalendar();
  $('#goto').hidden = false;
  $('#goto-time').focus();
}

function closeGoto() {
  $('#goto').hidden = true;
}

function gotoMessage(text, error = false) {
  const el = $('#goto-msg');
  el.textContent = text;
  el.style.whiteSpace = 'pre-line';
  el.classList.toggle('error', error);
}

function shiftMonth(k) {
  const m = gt.y * 12 + (gt.mo - 1) + k;
  gt.y = Math.floor(m / 12);
  gt.mo = (m % 12) + 1;
  renderCalendar();
}

function renderCalendar() {
  const { y, mo } = gt;
  const startOf = (yy, mm, dd) => toUnix(yy, mm, dd, 0, 0, 0, s.tz);
  $('#cal-title').textContent = new Date(Date.UTC(y, mo - 1, 1)).toLocaleString('en-US', { month: 'long', year: 'numeric', timeZone: 'UTC' });
  $('#cal-prev').disabled = startOf(y, mo, 1) <= s.range[0];
  $('#cal-next').disabled = startOf(y, mo + 1, 1) > s.range[1];
  const grid = $('#cal-grid');
  grid.innerHTML = ['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'].map((w) => `<span class="dow">${w}</span>`).join('');
  const lead = (new Date(Date.UTC(y, mo - 1, 1)).getUTCDay() + 6) % 7;     // weeks start on Monday
  for (let i = 0; i < lead; i++) grid.append(document.createElement('span'));
  const days = new Date(Date.UTC(y, mo, 0)).getUTCDate();
  let k = 0;
  for (let d = 1; d <= days; d++) {
    const a = startOf(y, mo, d);
    const b = startOf(y, mo, d + 1);
    while (k < s.cover.length && s.cover[k][1] <= a) k++;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'day';
    btn.textContent = d;
    btn.disabled = b <= s.range[0] || a > s.range[1];
    const data = k < s.cover.length && s.cover[k][0] < b;
    if (data) btn.classList.add('data');
    btn.title = btn.disabled ? 'Outside the recording' : data ? 'Waveforms on this day' : 'No waveforms on this day';
    if (gt.sel && gt.sel.y === y && gt.sel.mo === mo && gt.sel.d === d) btn.classList.add('on');
    btn.onclick = () => { gt.sel = { y, mo, d }; renderCalendar(); };
    btn.ondblclick = goTo;
    grid.append(btn);
  }
}

function goTo() {
  const m = /^(\d{1,2}):(\d{2})(?::(\d{2})(?:\.(\d{1,3}))?)?$/.exec($('#goto-time').value || '00:00');
  if (!m || !gt.sel) { gotoMessage('Pick a day and enter a time (hh:mm:ss).', true); return; }
  const ms = m[4] ? +m[4].padEnd(3, '0') : 0;
  const t = toUnix(gt.sel.y, gt.sel.mo, gt.sel.d, +m[1], +m[2], +(m[3] || 0), s.tz) + ms / 1000;
  if (t < s.range[0] || t > s.range[1]) {
    gotoMessage(`${fmtFull(t, s.tz, 60)} is outside the recording:\n${fmtFull(s.range[0], s.tz, 3600)}\n→ ${fmtFull(s.range[1], s.tz, 3600)}`, true);
    return;
  }
  const choice = $('#goto-span').value;
  const span = choice === 'keep' ? s.view.t1 - s.view.t0 : +choice;
  closeGoto();
  s.cursor = t;                                 // the cursor line marks the time asked for
  setView(t - span / 2, t + span / 2);
  updateCursorPanel();
  status(`Went to ${fmtFull(t, s.tz, 60)}`);
}

// ------------------------------------------------------------ URL state
let hashTimer = null;
function saveHash() {
  clearTimeout(hashTimer);
  hashTimer = setTimeout(() => {
    if (!s.ds) return;
    const h = new URLSearchParams({ path: s.ds.path, t0: s.view.t0.toFixed(3), t1: s.view.t1.toFixed(3) });
    // the host's context (and its theme) stays in the address
    const old = new URLSearchParams(location.hash.slice(1));
    for (const k of [...(window.ViewerHost?.KEYS || []), 'theme']) if (old.get(k)) h.set(k, old.get(k));
    history.replaceState(null, '', `#${h}`);
  }, 300);
}

async function fromHash() {
  const h = new URLSearchParams(location.hash.slice(1));
  hosted(h);
  const t = h.get('theme');                    // #theme=light|dark: the host's, not saved
  if (t === 'light' || t === 'dark') {
    hostTheme = t;
    if (document.documentElement.dataset.theme !== t) applyTheme(t);
  }
  if (h.has('browse')) {                       // #browse=<folder>: open the file browser there
    await openDialog(h.get('browse') || undefined);
    return true;
  }
  const path = h.get('path');
  if (!path) return false;
  const view = h.has('t0') && h.has('t1') ? [+h.get('t0'), +h.get('t1')] : null;
  if (s.ds && s.ds.path === path) {
    if (view) setView(view[0], view[1]);
    return true;
  }
  await openPath(path, view);
  return true;
}

// ------------------------------------------------------------ helpers
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const escapeAttr = escapeHtml;
// Vital-sign values as stored (no padding zeros).
const fmtNum = (v) => (v === null || v === undefined ? '—' : String(+(+v).toPrecision(6)));
function fmtBytes(n) {
  if (n === undefined || n === null) return '';
  n = Number(n);
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}

window.addEventListener('resize', draw);
init();
