// The Patient Catalog (web/js/views/patients.js): a patient opens from the keyboard (the name is
// a link); an older answer arriving after a newer one is dropped; "Add to cohort" adds what the
// list shows, not a search typed since.
import test from 'node:test';
import assert from 'node:assert/strict';

// a page of stand-in elements, by selector, made when first asked for
const els = new Map();
const el = (sel) => {
  if (!els.has(sel)) {
    const on = {};
    const classes = new Set(sel === '#pl-addto' ? ['hidden'] : []);
    els.set(sel, {
      sel, innerHTML: '', textContent: '', value: '', placeholder: '', disabled: false, hidden: false, on,
      classList: { contains: (c) => classes.has(c), add: (c) => classes.add(c), remove: (c) => classes.delete(c), toggle: (c, f) => (f ?? !classes.has(c) ? classes.add(c) : classes.delete(c)) },
      addEventListener: (k, f) => { on[k] = f; },
    });
  }
  return els.get(sel);
};
globalThis.document = { querySelector: el, querySelectorAll: () => [], addEventListener: () => {}, activeElement: null };
globalThis.location = { hash: '#/patients' };
globalThis.history = { replaceState: () => {}, pushState: () => {} };
globalThis.sessionStorage = { getItem: () => null, setItem: () => {} };

// the server: each list answer waits until the test lets it go
const pending = [];
const patches = [];
globalThis.fetch = async (url, init) => {
  const u = new URL(url, 'http://x/');
  const answer = (v) => ({ ok: true, status: 200, json: async () => v });
  if (u.pathname.endsWith('/patients')) {
    const q = u.searchParams.get('q') || '';
    return new Promise((resolve) => pending.push({ q, go: () => resolve(answer({
      total: 1, categories: [], items: [{ id: q === 'old' ? 1 : 2, patient_id: `MO-${q}`, sex: 'F', encounters: 1, latest: null, latest_where: '', kinds: [], items: 0 }],
    })) }));
  }
  if (u.pathname.endsWith('/cohorts')) return answer({ items: [{ id: 5, name: 'c', members: 0, can_edit: true }] });
  if (init.method === 'PATCH') { patches.push(JSON.parse(init.body)); return answer({ name: 'c', members: 1 }); }
  return answer({});
};

const { st } = await import('../../web/js/core.js');
st.cfg = { timezone: 'UTC' };
st.user = { privacy: true };
const view = (await import('../../web/js/views/patients.js')).default;
view.wire();
const tick = () => new Promise((r) => setTimeout(r, 0));

test('an older answer is dropped; the name is a link; "Add to cohort" adds what is shown', async () => {
  const first = view.show({ params: new URLSearchParams('q=old') });
  const second = view.show({ params: new URLSearchParams('q=new') });
  await tick();
  pending.find((p) => p.q === 'new').go();
  await second;
  pending.find((p) => p.q === 'old').go();
  await first;
  const table = el('#pl-table').innerHTML;
  assert.match(table, /MO-new/);
  assert.doesNotMatch(table, /MO-old/, 'the late answer of the older search is not drawn');
  assert.match(table, /<td class="nm"><a href="#\/patient\/2">/, 'the name opens the patient from the keyboard');

  // a new search typed (not answered yet), then "Add": the search on screen is the one added
  el('#pl-q').on.input({ target: { value: 'typed' } });
  await new Promise((r) => setTimeout(r, 260));
  el('#pl-addto-open').on.click();
  await tick(); await tick();
  el('#pl-addto-co').value = '5';
  el('#pl-addto').on.click({ target: { id: 'pl-addto-go' } });
  await tick(); await tick();
  assert.equal(patches.length, 1);
  assert.equal(patches[0].add_search.q, 'new');
  pending.forEach((p) => p.go());
});
