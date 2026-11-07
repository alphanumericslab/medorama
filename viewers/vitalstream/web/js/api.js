// Thin client for the VitalStream HTTP API.

async function request(url, options = {}) {
  const r = await fetch(url, options);
  if (r.status === 401) {          // signed out (password set or changed): show the sign-in page
    location.reload();
    throw new Error('sign in required');
  }
  if (!r.ok) {
    let msg = `${r.status} ${r.statusText}`;
    try { const j = await r.json(); if (j.detail) msg = j.detail; } catch (_) { /* not JSON */ }
    throw new Error(msg);
  }
  return r.json();
}

const q = (o) => new URLSearchParams(Object.entries(o).filter(([, v]) => v !== undefined && v !== null)).toString();

// Requests that change something carry `X-Medorama: 1`, which Medorama's gateway requires (a page
// on another site cannot add it); the viewers on their own ignore it.
const POST = { 'Content-Type': 'application/json', 'X-Medorama': '1' };

export const api = {
  config: () => request('api/v1/config'),
  browse: (path = '', hidden = false) => request(`api/v1/browse?${q({ path, hidden })}`),
  complete: (prefix, signal) => request(`api/v1/complete?${q({ prefix })}`, { signal }),
  open: (path, reindex = false) => request('api/v1/open', {
    method: 'POST', headers: POST, body: JSON.stringify({ path, reindex }),
  }),
  scan: (path) => request(`api/v1/scan?${q({ path })}`),
  cancel: (key) => request('api/v1/cancel', {
    method: 'POST', headers: POST, body: JSON.stringify({ key }),
  }),
  dataset: (key) => request(`api/v1/datasets/${key}`),
  window: (key, t0, t1, px, ch, signal) =>
    request(`api/v1/datasets/${key}/window?${q({ t0, t1, px, ch: ch.join(',') })}`, { signal }),
  vitals: (key, t0, t1, px, names, signal) =>
    request(`api/v1/datasets/${key}/vitals?${q({ t0, t1, px, names: names.join('|') })}`, { signal }),
  vitalsAt: (key, t, signal) => request(`api/v1/datasets/${key}/vitals_at?${q({ t })}`, { signal }),
  markers: (key, t0, t1, ch, signal) =>
    request(`api/v1/datasets/${key}/markers?${q({ t0, t1, ch: ch.join(',') })}`, { signal }),
};
