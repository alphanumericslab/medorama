// The PACS viewer's page: DICOM studies (stacks of slices, cine loops, reports, documents),
// pictures and videos.  Files come from Open, a dropped folder, or #list=URL / #src=URL when
// another program opens it (a folder's listing { files: [...] } beside its files, or one file).
//
// What is shown, per item of the list:
//   stack    single-frame images of one series (CT, MR, NM, X-ray), ordered along the slice
//            direction: the wheel or ↑ ↓ move through them, a drag sets the window
//   cine     a multi-frame file (ultrasound, angiography, cine MR …), a video, or pictures given
//            a frame rate (fps=): played at its own rate, frame by frame with ← →
//   still    one image or picture
//   report   a structured report: its measurements and text, as a table
//   document a PDF (a DICOM encapsulated document, or a PDF file)
// Calipers measure in cm on ultrasound (the regions' scale), in mm with the pixel spacing (at
// the detector for projection images), with mm= for pictures and videos, else in pixels.  The
// corners never show the patient.  A classic script (works from file://); under node (the
// tests) it defines only PacsDrop.

// The files of what was dropped (FileSystemEntry objects): every file under the folders,
// hidden files and folders (a name starting with '.') passed over.  A folder's reader gives its
// entries in batches (Chromium: at most 100 a call): it is read until a batch is empty.
const PacsDrop = (function () {
  'use strict';
  const readAll = (reader) => new Promise((res) => {
    const out = [];
    const next = () => reader.readEntries((es) => {
      if (!es.length) return res(out);
      out.push(...es);
      next();
    }, () => res(out));
    next();
  });
  async function walk(entry, files) {
    if (entry.name.startsWith('.')) return;
    if (entry.isFile) {
      const f = await new Promise((res) => entry.file(res, () => res(null)));
      if (f) files.push(f);
    } else if (entry.isDirectory) {
      const es = await readAll(entry.createReader());
      await Promise.all(es.map((e) => walk(e, files)));
    }
  }
  async function droppedFiles(entries) {
    const files = [];
    await Promise.all(entries.map((e) => walk(e, files)));
    return files;
  }
  return { droppedFiles };
})();
if (typeof module !== 'undefined' && module.exports) module.exports = PacsDrop;

(function () {
  'use strict';
  if (typeof document === 'undefined') return;
  const $ = (s) => document.querySelector(s);
  const esc = (t) => String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const store = {
    get(k, d) { try { const v = localStorage.getItem(`pacs.${k}`); return v === null ? d : JSON.parse(v); } catch { return d; } },
    set(k, v) { try { localStorage.setItem(`pacs.${k}`, JSON.stringify(v)); } catch { /* private mode */ } },
  };
  let params = new URLSearchParams(location.hash.slice(1));

  // full screen: the top strip of the screen belongs to the system (macOS: the menu bar, beside
  // a camera notch too), and clicks there do not reach the page: the header starts below it
  // (only as the page itself: inside a host's frame, the host's bar is at the top)
  const fullscreen = () => document.body.classList.toggle('fullscreen', window.top === window && (matchMedia('(display-mode: fullscreen)').matches
    || (Math.abs(window.innerHeight - screen.height) <= 1 && Math.abs(window.innerWidth - screen.width) <= 1)));
  window.addEventListener('resize', fullscreen);
  fullscreen();
  const embedded = params.has('src') || params.has('list');
  document.body.classList.toggle('embedded', embedded);

  const PRESETS = [
    ['Auto', null], ['Soft tissue', [40, 400]], ['Lung', [-600, 1500]], ['Bone', [400, 1800]], ['Brain', [40, 80]], ['Liver', [60, 160]], ['Cardiac', [300, 600]],
  ];
  const CINE = new Set(['US', 'IVUS', 'XA', 'RF', 'OCT', 'ES']);
  const CANVAS = ['stack', 'cine', 'still', 'video'];
  const st = {
    items: [], studies: 1, cur: 0, frame: 0, win: null, zoom: 1, pan: [0, 0], cache: new Map(), urls: [],
    playing: false, speed: 1, last: 0, acc: 0, calipers: false, marks: [], map: null, video: null,
  };

  // ---------------------------------------------------------------- theme
  function setTheme(t) { document.documentElement.dataset.theme = t; $('#theme').textContent = t === 'dark' ? '☀' : '☾'; }
  setTheme(params.get('theme') || store.get('theme', 'dark'));
  $('#theme').addEventListener('click', () => {
    const t = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    store.set('theme', t);
    setTheme(t);
  });

  function message(html, isErr) {
    $('#view').classList.add('hidden');
    $('#msg').innerHTML = html;
    $('#msg').className = `msg${isErr ? ' err' : ''}`;
  }

  // ---------------------------------------------------------------- loading
  function typeOf(f) {
    const e = (/\.([a-z0-9]+)$/i.exec(f.name) || [])[1]?.toLowerCase() || '';
    const head = new Uint8Array(f.bytes.slice(0, 4));
    if (['mp4', 'm4v', 'webm', 'mov', 'ogv'].includes(e) || /^video\//.test(f.type || '')) return 'video';
    if (e === 'pdf' || String.fromCharCode(...head) === '%PDF') return 'pdf';
    if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'].includes(e) || /^image\/(png|jpeg|gif|webp|bmp)/.test(f.type || '')) return 'picture';
    return 'dicom';
  }
  const blobUrl = (bytes, type) => { const u = URL.createObjectURL(new Blob([bytes], { type })); st.urls.push(u); return u; };
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  function normal(info) {
    const o = info?.orientation;
    if (!Array.isArray(o) || o.length < 6) return null;
    return [o[1] * o[5] - o[2] * o[4], o[2] * o[3] - o[0] * o[5], o[0] * o[4] - o[1] * o[3]];
  }

  /** What a folder of studies carries besides them: its index, notes, the system's files. */
  function companion(name) {
    const leaf = name.split('/').pop();
    return leaf.startsWith('.') || leaf.toUpperCase() === 'DICOMDIR' || /\.(txt|md|csv|json|xml|html?)$/i.test(leaf);
  }

  /** Read files ([{ name, bytes, type }]) into the list's items. */
  async function load(files) {
    for (const u of st.urls) URL.revokeObjectURL(u);
    st.urls = [];
    st.cache.clear();
    const fps = +params.get('fps') || 0;
    const items = [];
    const stacks = new Map();
    const pictures = [];
    const problems = [];
    for (const f of files) {
      if (companion(f.name)) continue;
      const t = typeOf(f);
      if (t === 'video') { items.push({ kind: 'video', title: f.name, modality: 'Video', url: blobUrl(f.bytes, /^video\//i.test(f.type || '') ? f.type : 'video/mp4') }); continue; }
      if (t === 'pdf') { items.push({ kind: 'document', title: f.name, modality: 'PDF', url: blobUrl(f.bytes, 'application/pdf') }); continue; }
      if (t === 'picture') { pictures.push(f); continue; }
      let k;
      try { k = Dicom.kind(f.bytes); } catch { problems.push(`${f.name}: not a DICOM file`); continue; }
      const base = { study: k.study, series: k.series, modality: k.modality };
      try {
        if (k.kind === 'pdf') {
          // only a PDF is shown, and always as one (dicom.js: the stated type is not trusted)
          const d = Dicom.documentOf(f.bytes);
          const title = d.title || k.title || 'Document';
          if (d.pdf) items.push({ ...base, kind: 'document', title, modality: k.modality || 'DOC', url: blobUrl(d.bytes, 'application/pdf') });
          else items.push({ ...base, kind: 'elsewhere', title, modality: k.modality || 'DOC', why: `A document of type ${d.mime.slice(0, 64)}: the viewer shows PDF documents only. Open it in a desktop DICOM viewer.` });
        } else if (k.kind === 'sr') {
          const r = Dicom.report(f.bytes);
          items.push({ ...base, kind: 'report', title: r.title, modality: k.modality || 'SR', report: r });
        } else if (k.kind === 'ecg' || k.kind === 'holter' || k.kind === 'waveform') {
          items.push({ ...base, kind: 'elsewhere', title: k.title || (k.kind === 'holter' ? 'Holter ECG' : 'ECG'), modality: k.modality || 'ECG',
            why: k.kind === 'holter' ? 'A DICOM Holter recording: VitalStream does not read DICOM. Convert it to a WFDB record (dicom-py wfdb FILE.dcm OUT/) and open that record in VitalStream.' : 'A DICOM ECG waveform: it opens in the ECG viewer.' });
        } else if (k.kind === 'image') {
          const info = Dicom.image(f.bytes);
          info.name = f.name;
          if (info.frames > 1) {
            // a multi-frame file: a cine loop (ultrasound, angiography …), else a stack of frames
            const cine = CINE.has(info.modality) || info.frameTime > 0 || info.cineRate > 0 || !!info.frameTimes;
            items.push({ ...base, kind: cine ? 'cine' : 'stack', title: info.seriesDescription || (cine ? 'Loop' : 'Series'), number: info.seriesNumber, instance: info.instance,
              frames: Array.from({ length: info.frames }, (_, i) => ({ info, frame: i })), fps: Dicom.fps(info) });
          } else {
            const key = info.series || f.name;
            if (!stacks.has(key)) {
              const it = { ...base, kind: 'stack', title: info.seriesDescription || info.modality || 'Series', number: info.seriesNumber, frames: [], fps: 10 };
              stacks.set(key, it);
              items.push(it);
            }
            stacks.get(key).frames.push({ info, frame: 0 });
          }
        } else {
          problems.push(`${f.name}: a DICOM object without an image`);
        }
      } catch (e) {
        if (e.unsupported) items.push({ ...base, kind: 'elsewhere', title: k.title || f.name, modality: k.modality, why: `${e.message}. To read the file, open it in a desktop DICOM viewer, or convert it to an uncompressed transfer syntax first (for example gdcmconv --raw IN.dcm OUT.dcm; a video's frames with ffmpeg).` });
        else problems.push(`${f.name}: ${e.message}`);
      }
    }
    // pictures: one cine when a frame rate is given (frames of a loop), else stills
    if (pictures.length > 1 && fps) {
      pictures.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
      items.push({ kind: 'cine', title: params.get('name') || 'Loop', modality: 'Pictures', fps, frames: pictures.map((p) => ({ picture: new Blob([p.bytes], { type: p.type || 'image/png' }), name: p.name })) });
    } else {
      for (const p of pictures) items.push({ kind: 'still', title: p.name, modality: 'Picture', frames: [{ picture: new Blob([p.bytes], { type: p.type || 'image/png' }), name: p.name }] });
    }
    for (const it of items) {
      if (it.kind !== 'stack') continue;
      // order along the slice normal, else by instance number
      const n = normal(it.frames[0]?.info);
      it.frames.sort((a, b) => (n && a.info.position && b.info.position ? dot(n, a.info.position) - dot(n, b.info.position) : 0)
        || (a.info.instance ?? 0) - (b.info.instance ?? 0) || a.frame - b.frame);
      if (it.frames.length === 1) it.kind = 'still';
    }
    // studies in the order met; within a study, series by number
    const studies = [...new Set(items.map((i) => i.study || ''))];
    items.forEach((it) => { it.studyIndex = studies.indexOf(it.study || ''); });
    st.items = items.sort((a, b) => a.studyIndex - b.studyIndex || (a.number ?? 1e9) - (b.number ?? 1e9) || (a.instance ?? 0) - (b.instance ?? 0) || String(a.title).localeCompare(String(b.title)));
    st.studies = studies.length;
    if (!st.items.length) return message(problems.length ? `These files are not ones the viewer reads.<br><small>${problems.slice(0, 5).map(esc).join('<br>')}</small>` : 'Nothing to show.', true);
    $('#msg').className = 'msg hidden';
    $('#view').classList.remove('hidden');
    renderList();
    select(0);
  }

  async function openUrls(listUrl, src) {
    const get = async (u) => {
      const r = await fetch(u, { credentials: 'same-origin' });
      if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
      return r;
    };
    try {
      message('Loading…');
      let urls;
      if (listUrl) {
        const base = new URL(listUrl, location.href);
        const { files } = await (await get(base)).json();
        urls = files.filter((f) => !companion(f)).map((f) => new URL(encodeURIComponent(f), base.href.endsWith('/') ? base : new URL('./', base)));
      } else {
        urls = [new URL(src, location.href)];
      }
      const files = [];
      let done = 0;
      await Promise.all(urls.map(async (u) => {
        const r = await get(u);
        files.push({ name: decodeURIComponent(u.pathname.split('/').pop()), bytes: await r.arrayBuffer(), type: r.headers.get('content-type') });
        message(`Loading… ${++done} of ${urls.length}`);
      }));
      files.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
      await load(files);
    } catch (e) {
      message(esc(e.message), true);
    }
  }

  // At most this many files, and bytes, are read at once (they are all held in memory): a
  // folder larger by mistake must not overwhelm the page.
  const MAX_FILES = 5000;
  const MAX_BYTES = 4 * 1024 ** 3;
  async function openFiles(list) {
    const chosen = [...list].filter((f) => !(f.webkitRelativePath || f.name).split('/').some((p) => p.startsWith('.')));
    const bytes = chosen.reduce((a, f) => a + f.size, 0);
    if (chosen.length > MAX_FILES || bytes > MAX_BYTES) {
      message(`This folder holds ${chosen.length.toLocaleString()} files (${(bytes / 1024 ** 3).toFixed(1)} GB): more than ${MAX_FILES} files or ${MAX_BYTES / 1024 ** 3} GB, which are held in memory at once. Open one study's folder.`, true);
      return;
    }
    const files = await Promise.all(chosen.map(async (f) => ({ name: f.name, bytes: await f.arrayBuffer(), type: f.type })));
    files.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
    await load(files);
  }

  // ---------------------------------------------------------------- the list
  const KIND = { report: 'report', document: 'document', video: 'video' };
  function sub(it) {
    const n = it.frames?.length;
    const bits = [it.modality, it.number !== undefined && it.number !== null ? `#${it.number}` : ''];
    if (it.kind === 'stack') bits.push(`${n} image${n === 1 ? '' : 's'}`);
    else if (it.kind === 'cine') bits.push(`${n} frames · ${Math.round(it.fps)} fps`);
    else bits.push(KIND[it.kind] || '');
    return bits.filter(Boolean).join(' · ');
  }
  function renderList() {
    let h = '';
    let study = -1;
    st.items.forEach((it, i) => {
      if (st.studies > 1 && it.studyIndex !== study) { study = it.studyIndex; h += `<div class="study">Study ${study + 1}</div>`; }
      h += `<button class="s${i === st.cur ? ' on' : ''}" data-i="${i}"><canvas width="64" height="64" data-thumb="${i}"></canvas>
        <span><b>${esc(it.title)}</b><small>${esc(sub(it))}</small></span></button>`;
    });
    $('#series').innerHTML = h;
    st.items.forEach((it, i) => thumb(it, $(`canvas[data-thumb="${i}"]`)));
  }
  async function thumb(it, canvas) {
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, 64, 64);
    try {
      let img = null;
      if (it.frames) img = await frameImage(it, it.kind === 'cine' ? Math.floor(it.frames.length / 3) : Math.floor(it.frames.length / 2), true);
      else if (it.kind === 'video') img = await videoStill(it.url);
      if (!img) {
        ctx.fillStyle = '#8b96a3'; ctx.font = '600 12px Inter, sans-serif'; ctx.textAlign = 'center';
        ctx.fillText({ report: 'SR', document: 'PDF', elsewhere: it.modality || '…' }[it.kind] || '', 32, 37);
        return;
      }
      const w = img.videoWidth || img.width;
      const hh = img.videoHeight || img.height;
      const k = Math.min(64 / w, 64 / hh);
      ctx.drawImage(img, (64 - w * k) / 2, (64 - hh * k) / 2, w * k, hh * k);
    } catch { /* no thumbnail */ }
  }
  function videoStill(url) {
    return new Promise((res) => {
      const v = document.createElement('video');
      v.muted = true; v.preload = 'auto'; v.src = url;
      v.onloadeddata = () => res(v);
      v.onerror = () => res(null);
    });
  }

  // ---------------------------------------------------------------- drawing
  /** Frame i of an item as a drawable image, windowed (grey DICOM). */
  async function frameImage(it, i, auto) {
    const fr = it.frames[i];
    if (fr.picture) {
      if (!st.cache.has(fr.picture)) st.cache.set(fr.picture, await createImageBitmap(fr.picture));
      return st.cache.get(fr.picture);
    }
    const info = fr.info;
    if (info.jpeg && !info.decoded) {
      const key = `j:${info.name}:${fr.frame}`;
      if (!st.cache.has(key)) st.cache.set(key, await createImageBitmap(new Blob([Dicom.jpegFrame(info, fr.frame)], { type: 'image/jpeg' })));
      return st.cache.get(key);
    }
    const key = `v:${info.name}:${fr.frame}`;
    let values = st.cache.get(key);
    if (!values) { values = Dicom.nativeFrame(info, fr.frame); st.cache.set(key, values); }
    const c = document.createElement('canvas');
    c.width = info.cols; c.height = info.rows;
    const ctx = c.getContext('2d');
    const im = ctx.createImageData(info.cols, info.rows);
    if (info.samples === 3) {
      for (let k = 0, p = 0; k < values.length; k += 3, p += 4) { im.data[p] = values[k]; im.data[p + 1] = values[k + 1]; im.data[p + 2] = values[k + 2]; im.data[p + 3] = 255; }
    } else {
      let w = auto ? null : st.win;
      if (!w) {
        // one window for the whole item (a cine does not flicker): the file's, else the first frame's
        w = it.win || (it.win = Dicom.autoWindow(info, values));
        if (!auto) st.win = w;
      }
      Dicom.toRgba(info, values, w[0], w[1], im.data);
    }
    ctx.putImageData(im, 0, 0);
    return c;
  }

  const item = () => st.items[st.cur];
  const grey = (it) => !!(it?.frames?.[0]?.info && it.frames[0].info.samples === 1);

  function select(i) {
    st.cur = i;
    const it = item();
    st.frame = it.kind === 'stack' ? Math.floor(it.frames.length / 2) : 0;
    st.win = null; st.zoom = 1; st.pan = [0, 0]; st.marks = []; st.acc = 0;
    st.playing = it.kind === 'cine';
    document.querySelectorAll('#series .s').forEach((b) => b.classList.toggle('on', +b.dataset.i === i));
    $('#preset').value = 'Auto';
    $('#preset').disabled = !grey(it);
    const n = it.frames?.length || 0;
    $('#slider').max = Math.max(0, n - 1);
    $('#slider').classList.toggle('hidden', n < 2);
    const moving = n > 1 || it.kind === 'video';
    for (const id of ['#prev', '#play', '#next', '#speed']) $(id).disabled = !moving;
    const drawn = CANVAS.includes(it.kind);
    $('#cal').disabled = !drawn;
    $('#zin').disabled = !drawn;
    $('#zout').disabled = !drawn;
    if (!drawn && st.calipers) setCalipers(false);
    // what is not drawn: a report, a document, a pointer elsewhere
    const page = $('#page');
    st.video?.pause();
    st.video = null;
    page.classList.toggle('hidden', drawn);
    page.innerHTML = it.kind === 'report' ? reportHtml(it.report)
      : it.kind === 'document' ? `<iframe title="${esc(it.title)}" src="${it.url}"></iframe>`
        : it.kind === 'elsewhere' ? `<div class="elsewhere"><b>${esc(it.title)}</b><p>${esc(it.why)}</p></div>` : '';
    if (it.kind === 'video') {
      const v = document.createElement('video');
      v.src = it.url; v.loop = true; v.muted = true; v.playsInline = true; v.playbackRate = st.speed;
      st.video = v;
      v.addEventListener('loadeddata', draw);
      v.addEventListener('play', status);
      v.addEventListener('pause', status);
      v.play().catch(() => status());
    }
    status();
    draw();
  }

  function reportHtml(r) {
    let h = `<div class="report"><h2>${esc(r.title)}</h2><table>`;
    for (const x of r.items) {
      // indented by depth (classes: no inline styles, which a strict page policy refuses)
      const pad = `d${Math.min(6, x.depth)}`;
      if (x.type === 'CONTAINER') { h += `<tr class="sec"><th colspan="2" class="${pad}">${esc(x.name)}</th></tr>`; continue; }
      const v = x.type === 'NUM' ? `${esc(x.value ?? '')}${x.unit ? ` <small>${esc(x.unit)}</small>` : ''}` : esc(x.value ?? '');
      h += `<tr><td class="${pad}">${esc(x.name)}</td><td class="${x.type === 'NUM' ? 'num' : ''}">${v}</td></tr>`;
    }
    return `${h}</table></div>`;
  }

  /** The scale at a point of the current frame, for the calipers. */
  function scale(fr, ax, ay) {
    const info = fr?.info;
    if (info) {
      const r = (info.regions || []).find((g) => g.unitsX === 3 && g.dx > 0 && ax >= (g.x0 ?? 0) && ax <= (g.x1 ?? 1e9) && ay >= (g.y0 ?? 0) && ay <= (g.y1 ?? 1e9));
      if (r) return { kx: r.dx, ky: r.dy || r.dx, unit: 'cm', digits: 2, note: '' };
      const sp = Array.isArray(info.spacing) ? info.spacing : Array.isArray(info.imagerSpacing) ? info.imagerSpacing : null;
      // DICOM spacing: between rows (y), then between columns (x)
      if (sp && sp[0] > 0) return { kx: sp[1] || sp[0], ky: sp[0], unit: 'mm', digits: 1, note: info.spacing ? '' : ' at the detector' };
    }
    const mm = +params.get('mm');
    if (mm > 0) return { kx: mm, ky: mm, unit: 'mm', digits: 1, note: '' };
    return { kx: 1, ky: 1, unit: 'px', digits: 0, note: '' };
  }
  function distance(it) {
    const [[ax, ay], [bx, by]] = st.marks;
    const s = scale(it.frames?.[st.frame], ax, ay);
    return `${Math.hypot((bx - ax) * s.kx, (by - ay) * s.ky).toFixed(s.digits)} ${s.unit}${s.note}`;
  }

  let drawing = 0;
  async function draw() {
    const it = item();
    if (!it) return;
    const cv = $('#canvas');
    const box = cv.parentElement.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const W = Math.round(box.width * dpr);
    const H = Math.round(box.height * dpr);
    if (cv.width !== W || cv.height !== H) { cv.width = W; cv.height = H; }
    const seq = ++drawing;
    let img;
    let failed = '';
    try {
      if (it.kind === 'video') img = st.video && st.video.readyState >= 2 ? st.video : null;
      else if (it.frames) img = await frameImage(it, st.frame);
    } catch (e) {
      // a frame that cannot be decoded (a damaged file): say so, stop playing, keep the rest usable
      failed = `This ${it.kind === 'cine' ? 'frame' : 'image'} cannot be decoded${e?.message ? `: ${e.message}` : ''}.`;
      if (st.playing) { st.playing = false; status(); }
    }
    if (seq !== drawing) return;
    const ctx = cv.getContext('2d');
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, W, H);
    if (failed) {
      ctx.fillStyle = '#c3ccd7';
      ctx.font = `${13 * dpr}px Inter, sans-serif`;
      ctx.textAlign = 'center';
      ctx.fillText(failed, W / 2, H / 2);
      ctx.textAlign = 'start';
    }
    if (!img) { corners(it); return; }
    const iw = img.videoWidth || img.width;
    const ih = img.videoHeight || img.height;
    const info = it.frames?.[st.frame]?.info;
    // non-square pixels are drawn to scale
    const sp = Array.isArray(info?.spacing) ? info.spacing : null;
    const aspect = sp && sp[0] > 0 && sp[1] > 0 ? sp[0] / sp[1] : 1;
    const k = Math.min(W / iw, H / (ih * aspect)) * st.zoom;
    const x0 = (W - iw * k) / 2 + st.pan[0] * dpr;
    const y0 = (H - ih * aspect * k) / 2 + st.pan[1] * dpr;
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(img, x0, y0, iw * k, ih * aspect * k);
    st.map = { x0, y0, kx: k, ky: k * aspect, dpr };
    if (st.marks.length) {
      ctx.strokeStyle = '#ffd54a'; ctx.fillStyle = '#ffd54a'; ctx.lineWidth = 1.5 * dpr;
      const P = st.marks.map(([x, y]) => [x0 + x * k, y0 + y * k * aspect]);
      for (const [x, y] of P) { ctx.beginPath(); ctx.arc(x, y, 3.5 * dpr, 0, 7); ctx.fill(); }
      if (P.length === 2) {
        ctx.setLineDash([5 * dpr, 4 * dpr]);
        ctx.beginPath(); ctx.moveTo(...P[0]); ctx.lineTo(...P[1]); ctx.stroke();
        ctx.setLineDash([]);
        ctx.font = `600 ${13 * dpr}px Inter, sans-serif`;
        ctx.shadowColor = '#000'; ctx.shadowBlur = 3 * dpr;
        ctx.fillText(distance(it), (P[0][0] + P[1][0]) / 2 + 8 * dpr, (P[0][1] + P[1][1]) / 2 - 8 * dpr);
        ctx.shadowBlur = 0;
      }
    }
    $('#slider').value = st.frame;
    corners(it, info, iw, ih);
  }

  function corners(it, info, iw, ih) {
    const n = it.frames?.length || 0;
    $('#tl').innerHTML = CANVAS.includes(it.kind) ? `<b>${esc(it.title)}</b><br>${esc(it.modality || '')}${+info?.thickness ? ` · ${esc(+(+info.thickness).toFixed(2))} mm` : ''}${info?.heartRate ? ` · HR ${esc(info.heartRate)}` : ''}` : '';
    $('#tr').innerHTML = n > 1 ? `${it.kind === 'cine' ? 'Frame' : 'Image'} ${st.frame + 1} / ${n}${it.kind !== 'cine' && info?.location !== null && info?.location !== undefined ? `<br>${(+info.location).toFixed(1)} mm` : ''}` : '';
    $('#bl').innerHTML = info && info.samples === 1 && st.win ? `W ${Math.round(st.win[1])} · L ${Math.round(st.win[0])}` : '';
    $('#br').innerHTML = iw ? `${iw} × ${ih}${st.zoom !== 1 ? ` · ${Math.round(st.zoom * 100)} %` : ''}${it.kind === 'cine' ? ` · ${Math.round(it.fps * st.speed)} fps` : ''}` : '';
  }

  function status() {
    const it = item();
    const playing = st.video ? !st.video.paused : st.playing;
    $('#play').textContent = playing ? '❚❚' : '▶';
    $('#play').title = playing ? 'Pause (Space)' : 'Play (Space)';
    $('#name').textContent = params.get('name') || it?.title || '';
  }

  // the cine clock: the item's frame rate (or its frame time vector), times the speed
  function tick(t) {
    const it = item();
    if (it?.frames && st.playing && it.frames.length > 1) {
      st.acc += Math.min(0.5, (t - (st.last || t)) / 1000);
      const info = it.frames[st.frame].info;
      const ft = Array.isArray(info?.frameTimes) ? info.frameTimes[st.frame + 1] || info.frameTimes[st.frame] : 0;
      const dt = (ft > 0 ? ft / 1000 : 1 / (it.fps || 20)) / st.speed;
      if (st.acc >= dt) {
        st.acc -= dt;
        st.frame = (st.frame + 1) % it.frames.length;
        draw();
      }
    } else if (it?.kind === 'video' && st.video && !st.video.paused) {
      draw();
    }
    st.last = t;
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);

  // ---------------------------------------------------------------- interaction
  function step(d) {
    const it = item();
    if (st.video) { st.video.pause(); st.video.currentTime = Math.max(0, st.video.currentTime + d / 30); setTimeout(draw, 60); return; }
    if (!it?.frames) return;
    const n = it.frames.length;
    st.playing = false;
    st.frame = it.kind === 'cine' ? (st.frame + d + n) % n : Math.max(0, Math.min(n - 1, st.frame + d));
    status();
    draw();
  }
  function play() {
    if (st.video) { if (st.video.paused) st.video.play(); else st.video.pause(); return; }
    if ((item()?.frames?.length || 0) < 2) return;
    st.playing = !st.playing;
    st.acc = 0;
    status();
  }
  function setCalipers(on) {
    st.calipers = on;
    st.marks = [];
    $('#cal').classList.toggle('on', on);
    document.body.classList.toggle('measuring', on);
    draw();
  }

  $('#canvas').addEventListener('wheel', (e) => {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) { st.zoom = Math.max(0.25, Math.min(8, st.zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1))); draw(); } else step(e.deltaY > 0 ? 1 : -1);
  }, { passive: false });
  let drag = null;
  $('#canvas').addEventListener('pointerdown', (e) => {
    if (st.calipers && e.button === 0 && !e.shiftKey) {
      if (!st.map) return;
      const box = e.target.getBoundingClientRect();
      const { x0, y0, kx, ky, dpr } = st.map;
      if (st.marks.length === 2) st.marks = [];
      st.marks.push([((e.clientX - box.left) * dpr - x0) / kx, ((e.clientY - box.top) * dpr - y0) / ky]);
      if (st.playing) { st.playing = false; status(); }
      st.video?.pause();
      draw();
      return;
    }
    drag = { x: e.clientX, y: e.clientY, win: st.win && [...st.win], pan: [...st.pan], panMode: e.shiftKey || e.button === 1 || !grey(item()) };
    $('#canvas').setPointerCapture(e.pointerId);
  });
  $('#canvas').addEventListener('pointermove', (e) => {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (drag.panMode) { st.pan = [drag.pan[0] + dx, drag.pan[1] + dy]; draw(); return; }
    if (!drag.win) return;
    // right: wider window; down: higher level
    st.win = [drag.win[0] + dy * Math.max(1, drag.win[1] / 300), Math.max(1, drag.win[1] + dx * Math.max(1, drag.win[1] / 300))];
    $('#preset').value = 'Custom';
    draw();
  });
  $('#canvas').addEventListener('pointerup', () => { drag = null; });
  $('#canvas').addEventListener('dblclick', () => { st.win = null; st.zoom = 1; st.pan = [0, 0]; $('#preset').value = 'Auto'; draw(); });
  $('#slider').addEventListener('input', (e) => { st.frame = +e.target.value; st.playing = false; status(); draw(); });
  $('#series').addEventListener('click', (e) => { const b = e.target.closest('.s'); if (b) select(+b.dataset.i); });
  $('#preset').innerHTML = PRESETS.map(([n]) => `<option>${n}</option>`).join('') + '<option hidden>Custom</option>';
  $('#preset').addEventListener('change', (e) => {
    const p = PRESETS.find(([n]) => n === e.target.value);
    st.win = p && p[1] ? [...p[1]] : null;
    draw();
  });
  $('#zin').addEventListener('click', () => { st.zoom = Math.min(8, st.zoom * 1.25); draw(); });
  $('#zout').addEventListener('click', () => { st.zoom = Math.max(0.25, st.zoom / 1.25); draw(); });
  $('#play').addEventListener('click', play);
  $('#prev').addEventListener('click', () => step(-1));
  $('#next').addEventListener('click', () => step(1));
  $('#speed').addEventListener('change', (e) => { st.speed = +e.target.value; if (st.video) st.video.playbackRate = st.speed; draw(); });
  $('#cal').addEventListener('click', () => setCalipers(!st.calipers));
  window.addEventListener('keydown', (e) => {
    if (e.target instanceof Element && e.target.closest('input, select')) return;
    const it = item();
    if (e.key === ' ') { e.preventDefault(); play(); return; }
    if (e.key === 'Escape') { st.marks = []; draw(); return; }
    if (it?.kind === 'cine' || it?.kind === 'video') {
      // ← → frames, ↑ ↓ the other loops
      if (e.key === 'ArrowRight') step(1);
      if (e.key === 'ArrowLeft') step(-1);
      if (e.key === 'ArrowDown' && st.cur < st.items.length - 1) select(st.cur + 1);
      if (e.key === 'ArrowUp' && st.cur > 0) select(st.cur - 1);
      return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowRight' || e.key === 'PageDown') { e.preventDefault(); step(1); }
    if (e.key === 'ArrowUp' || e.key === 'ArrowLeft' || e.key === 'PageUp') { e.preventDefault(); step(-1); }
    if (e.key === 'Home') { st.frame = 0; draw(); }
    if (e.key === 'End') { st.frame = (it?.frames?.length || 1) - 1; draw(); }
  });
  let t;
  window.addEventListener('resize', () => { clearTimeout(t); t = setTimeout(draw, 80); });

  // served by `muse-py serve` or `dicom-py serve`: the folders of the computer serving the page
  if (location.protocol.startsWith('http')) {
    fetch('served', { credentials: 'same-origin' }).then((r) => { if (r.ok) $('#server').hidden = false; }).catch(() => {});
  }
  $('#open').addEventListener('click', () => $('#file').click());
  $('#openDir').addEventListener('click', () => $('#dir').click());
  $('#file').addEventListener('change', (e) => openFiles(e.target.files));
  $('#dir').addEventListener('change', (e) => openFiles(e.target.files));
  document.addEventListener('dragover', (e) => { e.preventDefault(); document.body.classList.add('drag'); });
  document.addEventListener('dragleave', (e) => { if (!e.relatedTarget) document.body.classList.remove('drag'); });
  document.addEventListener('drop', async (e) => {
    e.preventDefault();
    document.body.classList.remove('drag');
    // a dropped folder: every file in it (hidden ones passed over)
    const entries = [...e.dataTransfer.items].map((i) => i.webkitGetAsEntry && i.webkitGetAsEntry()).filter(Boolean);
    if (entries.some((i) => i.isDirectory)) {
      openFiles(await PacsDrop.droppedFiles(entries));
    } else {
      openFiles(e.dataTransfer.files);
    }
  });
  // a host's context (whose study this is) in place of the file's name
  const hosted = () => document.body.classList.toggle('hosted', ViewerHost.render($('#host'), params));
  hosted();
  window.addEventListener('hashchange', () => {
    params = new URLSearchParams(location.hash.slice(1));
    hosted();
    if (params.get('theme')) setTheme(params.get('theme'));
    if (params.get('list') || params.get('src')) openUrls(params.get('list'), params.get('src'));
  });

  if (embedded) openUrls(params.get('list'), params.get('src'));
  else message('<b>Open a study</b> (files or a folder), or drop it here.<br>DICOM: echo and other ultrasound loops, angiography, CT, MR, nuclear, X-ray; structured reports and PDF documents (explicit or implicit VR little endian; native, RLE or JPEG). Pictures, videos and PDFs too.');
})();
