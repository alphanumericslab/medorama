// The ECG and PACS viewers (viewers/ecg, pacs: independent programs, served by Medorama)
// as tabs: #/ecg?src=…, #/pacs?list=… (echo, angiography, CT, MR … all open in PACS).  Each keeps its frame
// while switching screens; its tab is dimmed until something is opened in it.  `src` is a
// file's address, `list` a folder's (its files listed as JSON); both come from the API.

import { $, API, privacy } from '../core.js';
import { hostContext, listen, setOpener, withContext } from './banner.js';

export const APPS = { ecg: 'ECG', pacs: 'PACS' };

const theme = () => document.documentElement.dataset.theme || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');

/** The frame's hash for a file or folder of the API (a path from this page), with whose it is. */
function frameHash(params, ctx) {
  const p = new URLSearchParams();
  for (const k of ['src', 'list']) {
    // an address the viewer can fetch from its own page: absolute
    if (params.get(k)) p.set(k, new URL(params.get(k), location.href).pathname);
  }
  if (params.get('name')) p.set('name', params.get('name'));
  for (const k of ['mm', 'fps']) if (params.get(k)) p.set(k, params.get(k));
  p.set('theme', theme());
  return withContext(p, ctx).toString();
}

// What a viewer shows as it is, identifiers included: pictures and PDFs of ECGs, DICOM files
// (their headers, text burned into the pixels), echo loops, the bedside recordings (real times).
const AS_IS = { ecg: /\.(png|jpe?g|gif|webp|bmp|tiff?|pdf|dcm)$/i, pacs: /./ };
const WARN = 'May show identifiers: what this file shows (its pixels, its pages) is sent as it is; Privacy Mode cannot hide it';

/** The warning triangles (amber: red is for alarms) by the tabs whose file may show identifiers (Privacy Mode only). */
export function markIdentifiable() {
  for (const app of Object.keys(APPS)) {
    const p = new URLSearchParams($(`#app-${app}`).dataset.hash || '');
    const file = p.get('name') || p.get('src') || p.get('list') || '';
    setWarn($(`#nav-${app}`), privacy.on && !!file && AS_IS[app].test(file));
  }
}

function setWarn(a, on) {
  if (!a) return;
  let w = a.querySelector('.idwarn');
  if (on && !w) {
    w = document.createElement('span');
    w.className = 'idwarn';
    w.textContent = '▲';
    w.setAttribute('role', 'img');
    a.append(w);
  }
  if (w) { w.hidden = !on; w.title = WARN; w.setAttribute('aria-label', WARN); }
}

async function show(app, r) {
  const ctx = await hostContext(r.params, 'app');
  const frame = $(`#app-${app}`);
  document.querySelectorAll('#view-apps iframe').forEach((f) => f.classList.toggle('hidden', f !== frame));
  $('#app-msg').classList.toggle('hidden', !!(r.params.get('src') || r.params.get('list') || frame.getAttribute('src')));
  if (!r.params.get('src') && !r.params.get('list')) return;
  $(`#nav-${app}`).classList.remove('dim');
  const want = frameHash(r.params, ctx);
  if (!frame.getAttribute('src')) frame.src = `viewers/${app}/#${want}`;
  else if (frame.dataset.hash !== want) frame.contentWindow.location.hash = want;
  frame.dataset.hash = want;
  markIdentifiable();
}

/**
 * After the data changed (a database removed): close the viewers whose file is gone, saying
 * why, so that no tab keeps showing what the catalog no longer holds.
 */
export async function checkApps() {
  for (const app of Object.keys(APPS)) {
    const frame = $(`#app-${app}`);
    const p = new URLSearchParams(frame.dataset.hash || '');
    const url = p.get('src') || p.get('list');
    if (!url) continue;
    let gone = false;
    try {
      // the page's own check (the data changed): it does not keep an idle session alive
      const r = await fetch(url, { method: 'HEAD', credentials: 'same-origin', cache: 'no-store', headers: { 'X-Background': '1' } });
      gone = r.status === 404;
    } catch { /* offline: leave it */ }
    if (!gone) continue;
    frame.src = 'about:blank';
    frame.removeAttribute('src');
    frame.dataset.hash = '';
    frame.classList.add('hidden');
    $(`#nav-${app}`).classList.add('dim');
    if (location.hash.startsWith(`#/${app}`)) {
      $('#app-msg').textContent = `What this ${APPS[app]} tab showed was removed from the catalog (its database was deleted).`;
      $('#app-msg').classList.remove('hidden');
    }
  }
}

/** The view of one viewer, for the router. */
export const appView = (app) => ({ section: 'apps', nav: app, show: (r) => show(app, r) });

/**
 * The address that opens an attachment in its viewer; `ctx` ({ patient, entry }) names whose
 * file it is, for the banner above the viewer.
 */
export function appHref(app, a, api, ctx) {
  const p = new URLSearchParams({ [a.kind === 'folder' ? 'list' : 'src']: api + a.src, name: a.name });
  if (ctx?.patient && ctx?.entry) { p.set('patient', ctx.patient); p.set('entry', ctx.entry); }
  // a picture's scale, pictures' frame rate (from the item's row)
  for (const [k, v] of Object.entries(a.hints || {})) p.set(k, v);
  return `#/${app}?${p}`;
}

// the viewers' buttons (back to the patient, previous, next), and how a patient's file opens
setOpener((a, ctx) => appHref(a.viewer, a, API, ctx));
listen(() => [...Object.keys(APPS).map((a) => [$(`#app-${a}`), 'app']), [$('#viewer-frame'), 'rec']]);
