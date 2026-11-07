// Keeping every open page current: the server's data version moves whenever a database is
// imported or removed, or patients, encounters or recordings change (here or in anyone else's
// browser).  It is asked for every few seconds, and at once after this page changes something;
// when it has moved, the listeners redraw what they show.

import { api } from './core.js';

const EVERY_MS = 5000;
let version = null;
let timer = null;
const listeners = [];

/** The catalog's data version as last asked (null before the first answer). */
export function dataVersion() { return version; }

/** Call `fn` whenever the data have changed. */
export function onDataChange(fn) { listeners.push(fn); }

/** Ask now (e.g. after an import or a removal made on this page). */
export async function checkData() {
  clearTimeout(timer);
  try {
    const { data } = await api('version');
    const moved = version !== null && data !== version;
    version = data;
    if (moved) for (const fn of listeners) await Promise.resolve(fn()).catch(() => {});
  } catch { /* signed out, or the server is restarting: try again later */ }
  timer = setTimeout(checkData, EVERY_MS);
}

export function stopChecking() { clearTimeout(timer); version = null; }
