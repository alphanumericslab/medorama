// The ECG viewer's page: open a record (Open record…, a drop, or #src=URL when another program
// opens it), or a folder of them (Open multiple records…: every ECG in it and its subfolders,
// stepped through one at a time), show it, and the controls.  A WFDB record is opened by its
// header: the page then asks for the signal files it names (a page reads only the files chosen),
// and for a signal file chosen alone, asks for its header.  Opened from an address (#src), it
// fetches them itself, next to the header.  What a file is, and which reader
// reads it, the registry says (readers.js): records are drawn on paper (paper.js); pictures and
// PDF ECGs are shown as they are.  A classic script: the page works from file:// too.

(function () {
  'use strict';
  const $ = (s) => document.querySelector(s);
  const store = {
    get(k, d) { try { const v = localStorage.getItem(`ecg.${k}`); return v === null ? d : JSON.parse(v); } catch { return d; } },
    set(k, v) { try { localStorage.setItem(`ecg.${k}`, JSON.stringify(v)); } catch { /* private mode */ } },
  };
  const params = new URLSearchParams(location.hash.slice(1));

  // full screen: the top strip of the screen belongs to the system (macOS: the menu bar, beside
  // a camera notch too), and clicks there do not reach the page: the header starts below it
  // (only as the page itself: inside a host's frame, the host's bar is at the top)
  const fullscreen = () => document.body.classList.toggle('fullscreen', window.top === window && (matchMedia('(display-mode: fullscreen)').matches
    || (Math.abs(window.innerHeight - screen.height) <= 1 && Math.abs(window.innerWidth - screen.width) <= 1)));
  window.addEventListener('resize', fullscreen);
  fullscreen();
  const embedded = params.has('src');
  const st = {
    rec: null, layout: store.get('layout', '3x4'), speed: store.get('speed', 25), gain: store.get('gain', 10),
    zoom: 0, calipers: false, marks: [], drawn: null, url: null,
  };

  // ---------------------------------------------------------------- theme
  function setTheme(t) {
    document.documentElement.dataset.theme = t;
    $('#theme').textContent = t === 'dark' ? '☀' : '☾';
  }
  const prefersDark = () => window.matchMedia && matchMedia('(prefers-color-scheme: dark)').matches;
  setTheme(params.get('theme') || store.get('theme', prefersDark() ? 'dark' : 'light'));
  $('#theme').addEventListener('click', () => {
    const t = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    store.set('theme', t);
    setTheme(t);
  });
  document.body.classList.toggle('embedded', embedded);
  // compact: the paper alone, fitted to the frame (shown inside another page)
  document.body.classList.toggle('compact', params.get('compact') === '1');

  // ---------------------------------------------------------------- what the page reads
  /** "a, b and c" */
  const series = (xs, and) => (xs.length < 2 ? xs.join('') : `${xs.slice(0, -1).join(', ')} ${and} ${xs.at(-1)}`);
  const reads = (and) => series(Readers.labels(), and);
  $('#file').accept = Readers.accept();
  $('#open').title = `One ECG: ${reads('or')}`;

  function message(html, isErr = false) {
    $('#stage').innerHTML = `<div class="msg${isErr ? ' err' : ''}">${html}</div>`;
    $('#tools').classList.add('off');
  }

  /** The controls: shown for every ECG; usable only for one drawn on paper (not a picture or PDF). */
  function tools(usable) {
    $('#tools').classList.remove('off');
    $('#tools').classList.toggle('inert', !usable);
    for (const el of document.querySelectorAll('#tools select, #tools button')) el.disabled = !usable;
    $('#tools').title = usable ? '' : 'A picture or a PDF is shown as it is: its layout, speed and gain are fixed, and calipers need the signal';
    if (!usable && st.calipers) { st.calipers = false; $('#cal').classList.remove('on'); document.body.classList.remove('measuring'); }
  }
  const esc = (t) => String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  // ---------------------------------------------------------------- opening
  /** Show a file: its name, its bytes, and how to get the files next to it (WFDB).  ask(file):
   * when a file it needs is missing (an error with `missing`), ask for it; else the error is
   * shown. */
  async function open(name, bytes, type, sibling, ask) {
    if (st.url) { URL.revokeObjectURL(st.url); st.url = null; }
    st.rec = null;
    st.marks = [];
    const reader = Readers.find(name, bytes, type);
    $('#name').textContent = name || 'ECG';
    try {
      if (!reader) {
        message(`This is not an ECG the viewer reads: <b>${esc(name)}</b>.<br>It reads ${esc(reads('and'))}.`, true);
      } else if (reader.shown) {
        // a picture or a PDF: shown as it is
        const pdf = reader.shown === 'pdf';
        // the address's type: a PDF's or a picture's, never another the file or server states
        st.url = URL.createObjectURL(new Blob([bytes], { type: pdf ? 'application/pdf' : /^image\/[\w.+-]+$/i.test(type || '') && !/svg/i.test(type) ? type : 'image/png' }));
        tools(false);
        $('#info').textContent = pdf ? 'PDF' : 'picture';
        $('#stage').innerHTML = pdf
          ? `<iframe class="pdf" title="${esc(name)}" src="${st.url}"></iframe>`
          : `<div class="pic"><img alt="${esc(name)}" src="${st.url}"></div>`;
      } else {
        show(await reader.read(name, bytes, sibling));
      }
    } catch (e) {
      if (e.missing && ask) ask(e.missing);
      else message(esc(e.message || String(e)), true);
    }
  }

  async function openUrl(src, name) {
    const url = new URL(src, location.href);
    const get = async (u) => {
      const r = await fetch(u, { credentials: 'same-origin' });
      if (!r.ok) throw new Error(`${r.status} ${r.statusText}: ${decodeURIComponent(u.pathname.split('/').pop())}`);
      return r;
    };
    try {
      message('Loading…');
      const r = await get(url);
      const bytes = await r.arrayBuffer();
      await open(name || decodeURIComponent(url.pathname.split('/').pop()), bytes, r.headers.get('content-type'),
        async (f) => (await get(new URL(f, url))).arrayBuffer());
    } catch (e) {
      message(esc(e.message), true);
    }
  }

  // the files chosen for the record being opened, by name: each choice of the second chooser
  // (#more) adds to them
  let chosen = {};

  /** Ask for a file the record needs (a WFDB header's signal file, or a segment's header), by
   * the name its header gives it: the second chooser.  `tried`: the files of the last choice,
   * none of them the one asked for. */
  function askFor(file, tried) {
    const base = file.split('/').pop();
    const signal = !/\.hea$/i.test(base);
    const what = signal ? `This record's samples are in <b>${esc(base)}</b>` : `This record's segment <b>${esc(base)}</b> is missing`;
    const not = tried && tried.length ? `<br>${esc(tried.join(', '))} ${tried.length > 1 ? 'are' : 'is'} not the file its header names.` : '';
    ask(`${what}: choose it (or all the record's files at once).${not}`, `Choose ${base}…`,
      signal ? Readers.companions() : '.hea');
  }

  /** A message with a button that opens the second chooser, for files of these extensions. */
  function ask(html, label, accept) {
    message(`${html}<br><button class="primary ask">${esc(label)}</button>`);
    $('#more').accept = accept;
    $('#stage .ask').addEventListener('click', () => $('#more').click());
  }

  async function openFiles(list, keepFolder = false, more = false) {
    const files = [...list];
    if (!files.length) return;
    if (!keepFolder) folder(null);
    if (!more) chosen = {};
    for (const f of files) chosen[f.name] = f;
    const all = keepFolder ? files : Object.values(chosen);
    // the file the others are read with (a WFDB header: its signal files), else one the page
    // reads (a folder's ECG: the first)
    const known = (f) => Readers.byName(f.name);
    const main = keepFolder ? files[0] : all.find((f) => known(f)?.companions) || all.find(known) || all[0];
    // a signal file without its header (WFDB .dat, .mat): the header is the record
    if (!keepFolder && !known(main) && Readers.companionOf(main.name)) {
      const stem = main.name.replace(/\.[^.]+$/, '');
      $('#name').textContent = main.name;
      ask(`<b>${esc(main.name)}</b> is a WFDB signal file: choose its header (<b>${esc(stem)}.hea</b>, or the .hea file that names it).`,
        'Choose the header…', '.hea');
      return;
    }
    const byName = keepFolder ? Object.fromEntries(files.map((f) => [f.name, f])) : chosen;
    await open(main.name, await main.arrayBuffer(), main.type, async (f) => {
      const file = byName[f] || byName[f.split('/').pop()];
      if (!file) {
        if (keepFolder) throw new Error(`The file ${f} is missing from the folder of ${main.name}.`);
        throw Object.assign(new Error(`The file ${f} is missing.`), { missing: f });
      }
      return file.arrayBuffer();
    }, keepFolder ? null : (f) => askFor(f, more ? files.map((x) => x.name).filter((n) => n !== f.split('/').pop()) : null));
  }

  // ---------------------------------------------------------------- drawing
  function show(rec) {
    st.rec = rec;
    st.twelve = ECG.twelveLead(rec);
    tools(true);
    $('#layout').disabled = !st.twelve;
    const dur = Math.max(...rec.leads.map((l) => l.v.length / l.fs));
    // a de-identified view: the start moved back as the report's dates are
    const when = rec.start ? ECG.startText(rec.start, +params.get('shift') || 0) : '';
    $('#info').textContent = [when, `${rec.leads.length} leads`, `${rec.fs} Hz`, `${dur.toFixed(1)} s`].filter(Boolean).join(' · ');
    render();
  }

  function render() {
    if (!st.rec) return;
    const d = Paper.draw(st.rec, { layout: st.layout, speed: st.speed, gain: st.gain, twelve: st.twelve });
    st.drawn = d;
    const stage = $('#stage');
    stage.innerHTML = `${report(st.rec.meta)}<div class="sheetwrap">${d.svg}</div>`;
    const svg = stage.querySelector('svg');
    // fit the width, or a zoom in px per mm
    const compact = document.body.classList.contains('compact');
    const pad = compact ? 0 : 40;
    let fit = (stage.clientWidth - pad) / d.width;
    // inside another page: the paper also fits the height left under the report
    if (compact) {
      const rep = stage.querySelector('.report');
      fit = Math.min(fit, (stage.clientHeight - (rep ? rep.offsetHeight : 0)) / d.height);
    }
    const ppm = st.zoom || (pad ? Math.max(2, fit) : fit);
    svg.setAttribute('width', Math.round(d.width * ppm));
    svg.setAttribute('height', Math.round(d.height * ppm));
    $('#zoomv').textContent = st.zoom ? `${Math.round((st.zoom / 3.78) * 100)} %` : 'Fit';
    for (const [id, v] of [['layout', st.layout], ['speed', st.speed], ['gain', st.gain]]) $(`#${id}`).value = String(v);
    drawCalipers();
  }

  // ---------------------------------------------------------------- the report above the paper
  /** What the file says besides the signal: details, measurements, the interpretation. */
  function report(meta) {
    // opened by a de-identified view: its dates moved back, ages above 89 as 90+
    meta = ECG.deidMeta(meta, +params.get('shift') || 0, params.get('cap90') === '1');
    if (!meta) return '';
    const { fields = [], measurements = [], statements = [], notes = [] } = meta;
    if (!fields.length && !measurements.length && !statements.length && !notes.length) return '';
    // millivolts with two decimals (ST levels: -0.10), others as they are, to two decimals at most
    const fmt = (v, u) => (typeof v !== 'number' ? esc(v) : /^mv$/i.test(u || '') ? v.toFixed(2) : String(Math.round(v * 100) / 100));
    const global = measurements.filter((m) => !m.lead);
    const byLead = measurements.filter((m) => m.lead);
    let h = '<section class="report">';
    if (fields.length) h += `<dl class="rfields">${fields.map(([k, v]) => `<div><dt>${esc(k)}</dt><dd>${esc(v)}</dd></div>`).join('')}</dl>`;
    if (global.length) h += `<div class="rmeas">${global.map((m) => `<span><b>${esc(m.name)}</b> ${fmt(m.value, m.unit)}<small>${esc(m.unit)}</small></span>`).join('')}</div>`;
    // a lead's measurements on one line per measurement, as the others: ST  I -0.07 · II -0.07 … mV
    if (byLead.length) {
      const names = [...new Set(byLead.map((m) => m.name))];
      h += `<div class="rmeas rlead">${names.map((n) => {
        const ms = byLead.filter((m) => m.name === n);
        const u = ms[0]?.unit || '';
        return `<span><b>${esc(n)}</b> ${ms.map((m) => `<i>${esc(m.lead)}</i> ${fmt(m.value, m.unit)}`).join(' · ')}${u ? `<small>${esc(u)}</small>` : ''}</span>`;
      }).join('')}</div>`;
    }
    // the statements side by side: the computer's, and the physician's confirmed or edited ones
    if (statements.length) {
      h += '<div class="rstmts">';
      for (const x of statements) {
        const head = x.source === 'confirmed'
          ? `Confirmed${x.by ? ` by ${esc(x.by)}` : ''}${x.time ? ` · ${esc(x.time)}` : ''}`
          : x.source === 'edited'
            ? `Edited, not confirmed${x.time ? ` · ${esc(x.time)}` : ''}`
            : `Computer interpretation${x.program ? ` · ${esc(x.program)}` : ''}`;
        h += `<div class="rstmt ${x.source}"><h4>${head}</h4>${x.lines.map((l) => `<p>${esc(l)}</p>`).join('')}</div>`;
      }
      h += '</div>';
    }
    if (notes.length) h += `<div class="rnotes">${notes.map((n) => `<p>${esc(n)}</p>`).join('')}</div>`;
    return `${h}</section>`;
  }

  // ---------------------------------------------------------------- calipers
  function toMm(ev) {
    const svg = $('#stage svg');
    const p = svg.createSVGPoint();
    p.x = ev.clientX; p.y = ev.clientY;
    return p.matrixTransform(svg.getScreenCTM().inverse());
  }
  function drawCalipers() {
    const g = $('#stage .calipers');
    if (!g || !st.drawn) return;
    const d = st.drawn;
    let s = '';
    for (const m of st.marks) s += `<path class="cal" d="M${m.x} 0V${d.height}"/><circle class="caldot" cx="${m.x}" cy="${m.y}" r="0.7"/>`;
    if (st.marks.length === 2) {
      const [a, b] = st.marks;
      // the time between the two points, and the voltage between them (on the traces)
      const ms = Math.round(Math.abs(b.t - a.t) * 1000);
      const uv = Math.round((b.v - a.v) * 1000);
      const y = Math.min(a.y, b.y) - 4;
      const xm = (a.x + b.x) / 2;
      s += `<path class="cal" d="M${a.x} ${y}H${b.x}"/>`;
      const txt = `${ms} ms · ${uv >= 0 ? '+' : '−'}${Math.abs(uv)} µV`;
      const w = txt.length * 1.75 + 4;
      s += `<rect class="calbox" x="${xm - w / 2}" y="${y - 6.5}" width="${w}" height="5.4" rx="1.2"/><text class="caltext" x="${xm}" y="${y - 2.6}" text-anchor="middle">${txt}</text>`;
    }
    g.innerHTML = s;
  }

  // ---------------------------------------------------------------- controls
  $('#stage').addEventListener('click', (ev) => {
    if (!st.calipers || !st.drawn || !ev.target.closest('svg')) return;
    const p = toMm(ev);
    // the point of the trace nearest the click, not the paper under it
    const hit = st.drawn.snap(p.x, p.y);
    if (!hit) return;
    if (st.marks.length === 2) st.marks = [];
    st.marks.push({ x: +hit.x.toFixed(3), y: +hit.y.toFixed(3), t: hit.t, v: hit.v, lead: hit.lead });
    drawCalipers();
  });
  $('#cal').addEventListener('click', () => {
    st.calipers = !st.calipers;
    st.marks = [];
    $('#cal').classList.toggle('on', st.calipers);
    document.body.classList.toggle('measuring', st.calipers);
    drawCalipers();
  });
  for (const id of ['layout', 'speed', 'gain']) {
    $(`#${id}`).addEventListener('change', (e) => {
      st[id] = id === 'layout' ? e.target.value : +e.target.value;
      store.set(id, st[id]);
      st.marks = [];
      render();
    });
  }
  const ZOOMS = [2, 2.5, 3, 3.78, 4.5, 5.5, 7, 9, 12];
  $('#zin').addEventListener('click', () => { st.zoom = ZOOMS.find((z) => z > (st.zoom || currentPpm()) + 0.01) || ZOOMS.at(-1); render(); });
  $('#zout').addEventListener('click', () => { st.zoom = [...ZOOMS].reverse().find((z) => z < (st.zoom || currentPpm()) - 0.01) || ZOOMS[0]; render(); });
  $('#zoomv').addEventListener('click', () => { st.zoom = 0; render(); });
  const currentPpm = () => { const svg = $('#stage svg'); return svg && st.drawn ? svg.clientWidth / st.drawn.width : 3; };
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { st.marks = []; drawCalipers(); }
  });
  let t;
  window.addEventListener('resize', () => { clearTimeout(t); t = setTimeout(() => { if (!st.zoom) render(); }, 120); });

  // ---------------------------------------------------------------- a folder of ECGs
  // At most this many ECGs in a folder (docs/wfdb-collection.md, "Limits"): a larger one by
  // mistake must not overwhelm the page.
  const MAX_RECORDS = 5000;
  /** The folder's ECGs: [{ label, main: File, files: {name: File} (its folder's) }], or null. */
  function folder(list) {
    st.list = list;
    st.at = 0;
    $('#nav').hidden = !list;
    if (!list) return;
    $('#pick').innerHTML = list.map((r, i) => `<option value="${i}">${esc(r.label)}</option>`).join('');
  }
  async function openFolder(fileList) {
    const files = [...fileList].filter((f) => !(f.webkitRelativePath || f.name).split('/').some((p) => p.startsWith('.')));
    const dirs = new Map();
    for (const f of files) {
      const rel = f.webkitRelativePath || f.name;
      const dir = rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '';
      if (!dirs.has(dir)) dirs.set(dir, {});
      dirs.get(dir)[f.name] = f;
    }
    const list = [];
    for (const [dir, byName] of [...dirs].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))) {
      // the files that belong to a record (a multi-segment record's segments and layout)
      const claimed = new Set();
      for (const f of Object.values(byName)) {
        const r = Readers.byName(f.name);
        if (r && r.parts) for (const p of r.parts(f.name, await f.arrayBuffer())) claimed.add(p);
      }
      for (const name of Object.keys(byName).sort()) {
        const r = Readers.byName(name);
        if (!r || claimed.has(name)) continue;
        // a record of several files is named without the extension of the one opened
        const base = r.companions ? name.replace(/\.[^.]+$/, '') : name;
        list.push({ label: dir ? `${dir}/${base}` : base, main: byName[name], files: byName });
        if (list.length > MAX_RECORDS) {
          folder(null);
          message(`This folder holds more than ${MAX_RECORDS} ECGs: open a subfolder.`, true);
          return;
        }
      }
    }
    if (!list.length) { folder(null); message('No ECG in this folder or its subfolders.', true); return; }
    folder(list);
    await step(0);
  }
  async function step(i) {
    if (!st.list) return;
    st.at = Math.max(0, Math.min(st.list.length - 1, i));
    $('#pick').value = String(st.at);
    $('#prev').disabled = st.at === 0;
    $('#next').disabled = st.at === st.list.length - 1;
    const r = st.list[st.at];
    await openFiles([r.main, ...Object.values(r.files).filter((f) => f !== r.main)], true);
    $('#name').textContent = `${r.label} (${st.at + 1} of ${st.list.length})`;
  }
  $('#prev').addEventListener('click', () => step(st.at - 1));
  $('#next').addEventListener('click', () => step(st.at + 1));
  $('#pick').addEventListener('change', (e) => step(+e.target.value));
  window.addEventListener('keydown', (e) => {
    if (!st.list || e.target.closest('select, input') || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'ArrowLeft') { e.preventDefault(); step(st.at - 1); }
    if (e.key === 'ArrowRight') { e.preventDefault(); step(st.at + 1); }
  });

  // served by `muse-py serve` or `dicom-py serve`: the folders of the computer serving the page
  if (location.protocol.startsWith('http')) {
    fetch('served', { credentials: 'same-origin' }).then((r) => { if (r.ok) $('#server').hidden = false; }).catch(() => {});
  }
  $('#open').addEventListener('click', () => $('#file').click());
  $('#file').addEventListener('change', (e) => openFiles(e.target.files));
  $('#more').addEventListener('change', (e) => {
    const files = [...e.target.files];
    e.target.value = '';                 // the same file may be chosen again
    openFiles(files, false, true);
  });
  $('#open-many').addEventListener('click', () => $('#dir').click());
  $('#dir').addEventListener('change', (e) => openFolder(e.target.files));
  document.addEventListener('dragover', (e) => { e.preventDefault(); document.body.classList.add('drag'); });
  document.addEventListener('dragleave', (e) => { if (!e.relatedTarget) document.body.classList.remove('drag'); });
  document.addEventListener('drop', (e) => {
    e.preventDefault();
    document.body.classList.remove('drag');
    openFiles(e.dataTransfer.files);
  });
  // a host's context (whose ECG this is) in place of the file's name
  const hosted = (p) => document.body.classList.toggle('hosted', ViewerHost.render($('#host'), p));
  hosted(params);
  window.addEventListener('hashchange', () => {
    const p = new URLSearchParams(location.hash.slice(1));
    hosted(p);
    if (p.get('theme')) setTheme(p.get('theme'));
    if (p.get('src')) openUrl(p.get('src'), p.get('name'));
  });

  if (embedded) openUrl(params.get('src'), params.get('name'));
  else message(`<b>Open record…</b> (or drop it here): one ECG: ${esc(reads('or'))}.<br>A WFDB record: choose its header (.hea); the page then asks for its signal files.<br><b>Open multiple records…</b>: a folder, every ECG in it and in its subfolders, one at a time (‹ ›, ← →).`);
})();
