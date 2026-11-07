// What a hosting program tells a viewer about the file it shows, so that the viewer's title says
// whose it is instead of the file's name (Medorama: the patient, the file, its place
// among the patient's others).  In the address's fragment, all optional:
//   who=…   the patient, as the host may show them      what=…   the file: kind, date, encounter
//   pos=…   its place, e.g. "3 of 4"                   nav=back,prev,next   the buttons to offer
// The buttons only tell the host (postMessage to the parent page, same origin); the host
// navigates.  Without who and what, the viewer shows its file's name as before.
// The ECG, PACS and VitalStream viewers each keep a copy of this file (tests check they agree).
// A classic script (it works from file://), and a CommonJS module for tests.

(function (root) {
  'use strict';

  const esc = (t) => String(t ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  /** The keys of the host's context (a viewer that rewrites its address keeps them). */
  const KEYS = ['who', 'what', 'pos', 'nav'];

  /** The host's context in an address's parameters, or null: { who, what, pos, nav (Set) }. */
  function context(params) {
    const who = params.get('who') || '';
    const what = params.get('what') || '';
    if (!who && !what) return null;
    const nav = new Set((params.get('nav') || '').split(',').map((x) => x.trim()).filter(Boolean));
    return { who, what, pos: params.get('pos') || '', nav };
  }

  /** The context as HTML (escaped): ‹ Patient, who, what, ‹ pos ›. */
  function html(c) {
    const b = (a, label, title) => `<button type="button" class="hostnav" data-host="${a}" title="${title}"${c.nav.has(a) ? '' : ' disabled'}>${label}</button>`;
    return `${c.nav.has('back') ? b('back', '‹ Patient', 'Back to the patient&#39;s page') : ''}`
      + `${c.who ? `<span class="hostwho" title="${esc(c.who)}">${esc(c.who)}</span>` : ''}`
      + `${c.what ? `<span class="hostwhat" title="${esc(c.what)}">${esc(c.what)}</span>` : ''}`
      + `${c.pos ? `<span class="hostpos">${b('prev', '‹', 'The previous one')}<span>${esc(c.pos)}</span>${b('next', '›', 'The next one')}</span>` : ''}`;
  }

  /** Tell the host a button was pressed (nothing happens when no page hosts the viewer). */
  function post(action) {
    if (!root.parent || root.parent === root) return;
    const origin = root.location.origin && root.location.origin !== 'null' ? root.location.origin : '*';
    root.parent.postMessage({ viewerHost: action }, origin);
  }

  /** Show the context in `el`; returns whether there is one (the page then hides its file's name). */
  function render(el, params) {
    const c = context(params);
    el.hidden = !c;
    el.innerHTML = c ? html(c) : '';
    el.onclick = (e) => {
      const t = e.target.closest && e.target.closest('[data-host]');
      if (t && !t.disabled) post(t.dataset.host);
    };
    return !!c;
  }

  const api = { KEYS, context, html, render };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.ViewerHost = api;
})(typeof self !== 'undefined' ? self : this);
