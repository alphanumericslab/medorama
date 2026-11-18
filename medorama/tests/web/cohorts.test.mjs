// The Cohorts tab (web/js/views/cohorts.js): a condition that cannot be read is never left out
// of a search silently (the search waits, the count says why, nothing can be saved); the sex is
// written female or male; a redraw for new data keeps the open panel.
import test from 'node:test';
import assert from 'node:assert/strict';
import { answer, el, install, wait } from './dom.mjs';

install({ hash: '#/cohorts' });
const searches = [];
let hold = null;
globalThis.fetch = async (url, init = {}) => {
  const path = new URL(url, 'http://x/').pathname.replace(/^\/api\/v1\//, '');
  const body = init.body ? JSON.parse(init.body) : null;
  if (path === 'cohorts/parse') {
    return answer(body.text === 'female' ? { ok: true, words: 'female', expr: { kind: 'sex', text: 'F' } } : { ok: false, error: 'a value is missing after >' });
  }
  if (path === 'cohorts/search') {
    searches.push(body);
    if (hold) await hold;
    return answer({ total: 2, ids: [1, 2], items: [], excluded_total: 0, excluded: [], searched: [[1, 'hosp']], left_out: [] });
  }
  if (path === 'patients') return answer({ categories: [['lab', 'Labs']], items: [], total: 0 });
  if (path === 'databases') return answer({ items: [{ id: 1, name: 'hosp', status: 'online', permission: 'view' }] });
  if (path === 'cohorts') return answer({ items: [] });
  if (path === 'cohorts/7') return answer({ id: 7, name: 'Seven', members: 0, is_owner: true, can_edit: true, created_at: 0, definition: { databases: [], added: [], removed: [], exclude: [], sources: [] }, source_versions: {} });
  if (path === 'cohorts/7/members') return answer({ items: [], total: 0 });
  if (path === 'version') return answer({ data: 1 });
  return answer({});
};

const { st } = await import('../../web/js/core.js');
st.cfg = { timezone: 'UTC' };
st.user = { id: 1, role: 'viewer', privacy: true };
const view = (await import('../../web/js/views/cohorts.js')).default;
view.wire();
const builder = el('#co-builder');
const field = (k, value, g = '0', i = '0') => {
  const t = { dataset: { g, i, k }, value, type: 'text', checked: false };
  t.closest = () => t;
  return t;
};
const show = (hash) => {
  globalThis.location.hash = hash;
  const [path, q = ''] = hash.replace(/^#\/?/, '').split('?');
  return view.show({ name: 'cohorts', parts: path.split('/').slice(1).filter(Boolean), params: new URLSearchParams(q) });
};

test('an expression that does not read stops the search, says why, and nothing can be saved', async () => {
  await show('#/cohorts');
  assert.equal(searches.length, 1, 'the empty builder searches (every patient)');
  builder.on.input({ target: field('text', 'QT >') });
  await wait(450);
  assert.equal(searches.length, 1, 'no search with a condition that cannot be read');
  assert.match(el('#co-count').textContent, /^1 condition cannot be read: “QT >”: a value is missing after >/);
  assert.ok(el('#co-count').classList.contains('err'));
  assert.match(el('#co-save').innerHTML, /id="co-create" disabled/);

  // corrected: searched, with the expression in the criteria
  builder.on.input({ target: field('text', 'female ') });
  await wait(450);
  assert.equal(searches.length, 2);
  assert.equal(searches[1].criteria.include.text, 'female', 'a space at its end does not drop it');
  assert.ok(!el('#co-count').classList.contains('err'));
  assert.doesNotMatch(el('#co-save').innerHTML, /id="co-create" disabled/);
});

test('the sex is chosen as female or male; left unchosen, it stops the search', async () => {
  const kind = field('kind', 'sex');
  builder.on.change({ target: kind });
  assert.match(el('#co-criteria').innerHTML, /<option value="F">female<\/option>/);
  assert.match(el('#co-criteria').innerHTML, /<option value="M">male<\/option>/);
  assert.doesNotMatch(el('#co-criteria').innerHTML, />F<\/option>/);
  await wait(450);
  assert.match(el('#co-count').textContent, /Sex: choose female or male/);
});

test('an expression says its meaning (or its error) to screen readers', () => {
  builder.on.change({ target: field('kind', 'expression') });
  assert.match(el('#co-criteria').innerHTML, /class="ewords [^"]*" data-words="0:0" role="status" aria-live="polite"/);
});

test("a redraw for new data keeps a cohort's open panel", async () => {
  await show('#/cohorts/7');
  el('#co-panel').innerHTML = '<div class="card panel">the refresh preview, ticked</div>';
  view.invalidate();
  st.redrawing = true;
  try { await show('#/cohorts/7'); } finally { st.redrawing = false; }
  assert.match(el('#co-panel').innerHTML, /the refresh preview, ticked/);
  // a visit of the page (not a redraw): the panel closed
  await show('#/cohorts/7');
  assert.equal(el('#co-panel').innerHTML, '');
});

test('a search under way is said in the busy pill until its answer arrives', async () => {
  let go;
  hold = new Promise((r) => { go = r; });
  const shown = show('#/cohorts');
  await wait(50);
  const pill = el('#co-count').innerHTML;
  assert.match(pill, /class="searching"/);
  assert.match(pill, /Searching… please wait/);
  go(); hold = null;
  await shown;
  await wait(50);
  assert.doesNotMatch(el('#co-count').innerHTML, /searching/);
  assert.match(el('#co-count').innerHTML, /<b>2<\/b> patients match/);
});
