// The page's addresses (web/js/router.js): every screen has one, read back as it was written.
import test from 'node:test';
import assert from 'node:assert/strict';
import { href, route } from '../../web/js/router.js';

const at = (hash) => { globalThis.location = { hash }; return route(); };

test('an address is read into its screen, parts and parameters', () => {
  const r = at('#/patient/12?item=labs%3A3%3A1704175800&enc=4');
  assert.equal(r.name, 'patient');
  assert.deepEqual(r.parts, ['12']);
  assert.equal(r.params.get('item'), 'labs:3:1704175800');
  assert.equal(r.params.get('enc'), '4');
});

test('no address: the cohorts', () => {
  assert.equal(at('').name, 'cohorts');
  assert.equal(at('#/').name, 'cohorts');
});

test('a malformed address is read as it is, not refused', () => {
  assert.deepEqual(at('#/patient/%E0').parts, ['%E0']);
});

test('href and route agree, whatever the text', () => {
  for (const part of ['a b', 'x/y', 'é?#&', '%41']) {
    const h = href('cohorts', [part], { q: part, empty: '', none: null });
    const r = at(h);
    assert.equal(r.name, 'cohorts');
    assert.deepEqual(r.parts, [part], h);
    assert.equal(r.params.get('q'), part);
    assert.equal(r.params.has('empty') || r.params.has('none'), false);
  }
});
