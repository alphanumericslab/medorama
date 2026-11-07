// The Viewer screen: the one viewer instance the catalog runs (an iframe kept while switching
// screens).  #/viewer?path=…&t0=…&t1=… shows a recording at a time span; the viewer's page
// follows its own address without reloading.  The page's theme goes with it (#theme=), and
// main.js passes a change on.

import { $, api, fail, st, viewerHash, viewerMode } from '../core.js';
import { hostContext, withContext } from './banner.js';

async function show(r) {
  const ctx = await hostContext(r.params, 'rec');
  const frame = $('#viewer-frame');
  const msg = $('#viewer-msg');
  if (viewerMode() !== 'embedded') {
    msg.textContent = st.cfg?.viewer?.reason || 'No viewer is linked to the catalog.';
    msg.classList.remove('hidden');
    return;
  }
  const path = r.params.get('path');
  // the recording at its span, with whose it is (the viewer shows it in place of the file's name)
  // and the page's theme
  const theme = document.documentElement.dataset.theme || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  const p = path ? withContext(new URLSearchParams(viewerHash(path, r.params.get('t0'), r.params.get('t1'))), ctx) : null;
  p?.set('theme', theme);
  const hash = p ? p.toString() : '';
  if (!frame.getAttribute('src')) {
    msg.textContent = 'Starting the viewer…';
    msg.classList.remove('hidden');
    try {
      st.cfg.viewer = await api('viewer/start', { method: 'POST' });
    } catch (e) {
      msg.textContent = `The viewer cannot start: ${e.message}`;
      return;
    }
    msg.classList.add('hidden');
    frame.classList.remove('hidden');
    frame.src = `viewer/#${hash || `theme=${theme}`}`;
    return;
  }
  if (hash) {
    try {
      if (frame.contentWindow.location.hash.slice(1) !== hash) frame.contentWindow.location.hash = hash;
    } catch {
      frame.src = `viewer/#${hash}`;
    }
  }
}

export default {
  show: (r) => show(r).catch(fail),
};
