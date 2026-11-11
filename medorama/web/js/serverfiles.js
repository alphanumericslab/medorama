// "Choose on the server": the one way the page picks files and folders on this server (admins).
// The server lists them (GET server-files), inside data_roots, as the viewer's Open dialog does;
// the browser's own file dialog only uploads a file from this computer.  Not in Privacy Mode:
// the server refuses, and the dialog says why.

import { $, api, esc, qs, store } from './core.js';
import { size } from './imports.js';

const KINDS = {
  tables: { title: 'Choose tables on the server', none: 'No folder or table here (CSV, spreadsheets, Parquet).' },
  databases: { title: 'Choose a database file on the server', none: 'No folder or SQLite file here.' },
  folders: { title: 'Choose a folder on the server', none: 'No folder here.' },
};

let dlg = null;

function build() {
  dlg = document.createElement('dialog');
  dlg.id = 'sf';
  dlg.className = 'sf';
  dlg.setAttribute('aria-labelledby', 'sf-title');
  dlg.innerHTML = `<form method="dialog" class="sf-body">
    <div class="sf-head"><h2 id="sf-title"></h2><button value="" class="icon" title="Close" aria-label="Close">✕</button></div>
    <div class="ftools"><button type="button" id="sf-up" title="One folder up" aria-label="One folder up">↑</button>
      <input id="sf-dir" class="mono" aria-label="Folder on the server" placeholder="The data roots" spellcheck="false">
      <button type="button" id="sf-go">Go</button><button type="button" id="sf-roots" class="link">Data roots</button></div>
    <div id="sf-list" class="sf-list" role="list"></div>
    <p id="sf-msg" class="muted small"></p>
    <div class="ftools"><span id="sf-count" class="muted"></span><span class="spacer"></span>
      <button value="">Cancel</button><button id="sf-ok" value="ok" class="primary">Choose</button></div>
  </form>`;
  document.body.append(dlg);
}

/**
 * The paths chosen on the server, or null if cancelled.
 * kind: `tables` (several, unless `multiple` is false), `databases` (one SQLite file) or
 * `folders` (the folder shown).
 */
export function chooseOnServer({ kind = 'tables', multiple = kind === 'tables' } = {}) {
  if (!dlg) build();
  const k = KINDS[kind];
  const key = `serverDir.${kind}`;
  const sel = new Set();
  let here = { dir: '', parent: null };
  $('#sf-title', dlg).textContent = k.title;
  $('#sf-ok', dlg).textContent = kind === 'folders' ? 'Choose this folder' : 'Choose';

  const update = () => {
    const ok = kind === 'folders' ? !!here.dir : sel.size > 0;
    $('#sf-ok', dlg).disabled = !ok;
    $('#sf-count', dlg).textContent = kind === 'tables' && sel.size ? `${sel.size} chosen` : '';
    $('#sf-up', dlg).disabled = here.parent === null;
  };

  const show = async (dir, first = false) => {
    $('#sf-msg', dlg).textContent = 'Reading…';
    try {
      const v = await api(`server-files?${qs({ dir, kind })}`);
      here = v;
      $('#sf-dir', dlg).value = v.dir;
      if (v.dir) store.set(key, v.dir);
      $('#sf-list', dlg).innerHTML = v.items.map((it) => `<div role="listitem" class="sf-item">${it.dir
        ? `<button type="button" class="menu" data-dir="${esc(it.path)}">${esc(it.name.replace(/[\\/]+$/, ''))}/</button>`
        : `<label class="sf-file"><input type="${multiple ? 'checkbox' : 'radio'}" name="sf-pick" data-path="${esc(it.path)}"${sel.has(it.path) ? ' checked' : ''}>
           <span>${esc(it.name)}</span><span class="muted small">${size(it.size)}</span></label>`}</div>`).join('');
      $('#sf-msg', dlg).textContent = v.items.length ? (v.truncated ? `Only the first ${v.items.length} are shown.` : '') : k.none;
    } catch (e) {
      // the folder last shown, gone or now outside the roots: the roots instead
      if (first && dir) { show(''); return; }
      $('#sf-msg', dlg).textContent = e.message;
    }
    update();
  };

  return new Promise((resolve) => {
    const onClick = (e) => {
      const b = e.target.closest('[data-dir]');
      if (b) { show(b.dataset.dir); return; }
      if (e.target.closest('#sf-up') && here.parent !== null) show(here.parent);
      else if (e.target.closest('#sf-roots')) show('');
      else if (e.target.closest('#sf-go')) show($('#sf-dir', dlg).value.trim());
    };
    const onChange = (e) => {
      const p = e.target.dataset?.path;
      if (p === undefined) return;
      if (!multiple) sel.clear();
      if (e.target.checked) sel.add(p); else sel.delete(p);
      update();
    };
    const onKey = (e) => {
      if (e.key === 'Enter' && e.target.id === 'sf-dir') { e.preventDefault(); show(e.target.value.trim()); }
    };
    const onClose = () => {
      dlg.removeEventListener('click', onClick);
      dlg.removeEventListener('change', onChange);
      dlg.removeEventListener('keydown', onKey);
      if (dlg.returnValue !== 'ok') resolve(null);
      else resolve(kind === 'folders' ? [here.dir] : [...sel]);
    };
    dlg.addEventListener('click', onClick);
    dlg.addEventListener('change', onChange);
    dlg.addEventListener('keydown', onKey);
    dlg.addEventListener('close', onClose, { once: true });
    dlg.returnValue = '';
    dlg.showModal();
    show(store.get(key, ''), true);
  });
}
