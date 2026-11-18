// The Viewer screen: the one viewer instance Medorama runs (an iframe kept while switching
// screens).  #/viewer?path=…&t0=…&t1=… shows a recording at a time span; the viewer's page
// follows its own address without reloading.  The page's theme goes with it (#theme=), and
// main.js passes a change on.  Until a recording is opened from a patient's page, the tab says
// so, as the ECG and PACS tabs do: the viewer is not started empty (it would open again what it
// showed last, perhaps another user's patient).

import { $, api, fail, st, viewerHash, viewerMode } from '../core.js';
import { hostContext, withContext } from './banner.js';

async function show(r) {
  const ctx = await hostContext(r.params, 'rec');
  const frame = $('#viewer-frame');
  const msg = $('#viewer-msg');
  if (viewerMode() !== 'embedded') {
    msg.textContent = st.cfg?.viewer?.reason || 'No viewer is linked to Medorama.';
    msg.classList.remove('hidden');
    return;
  }
  const path = r.params.get('path');
  if (!path && !frame.getAttribute('src')) {
    msg.textContent = NOTHING;
    msg.classList.remove('hidden');
    frame.classList.add('hidden');
    return;
  }
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
  msg.classList.add('hidden');
  frame.classList.remove('hidden');
  if (hash) {
    try {
      if (frame.contentWindow.location.hash.slice(1) !== hash) frame.contentWindow.location.hash = hash;
    } catch {
      frame.src = `viewer/#${hash}`;
    }
  }
}

/** What the tab says before a recording is opened. */
export const NOTHING = "Open a recording (ICU monitoring, a Holter, a sleep study, an EEG …) from a patient's page.";

export default {
  show: (r) => show(r).catch(fail),
};
