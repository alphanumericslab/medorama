// Signing in (web/js/main.js): a first screen that fails after sign-in (the server answers 500)
// is said, with the page shown; it is never taken for a failed sign-in, whose form loads the
// page again (which would fail again: a loop of reloads).
import test from 'node:test';
import assert from 'node:assert/strict';
import { answer, el, install, tick } from './dom.mjs';

const { reloads } = install({ hash: '#/cohorts' });
const asked = [];
globalThis.fetch = async (url) => {
  const path = new URL(url, 'http://x/').pathname.replace(/^\/api\/v1\//, '');
  asked.push(path);
  if (path === 'config') {
    return answer({
      user: { id: 1, username: 'boss', display_name: 'Boss', role: 'admin', phi: false, phi_allowed: false, privacy: true },
      viewer: { mode: 'none', reason: 'none linked' }, timezone: 'UTC', display: { date: 'ymd', clock: '24h' },
    });
  }
  if (path === 'version') return answer({ data: 1 });
  // the first screen's data: the server fails
  return answer({ detail: 'the database is busy' }, 503);
};

await import('../../web/js/main.js');
for (let i = 0; i < 20; i++) await tick();

test('a first screen that fails after sign-in is said, and the page is not loaded again', () => {
  assert.ok(asked.includes('config'), asked.join(' '));
  assert.ok(asked.includes('databases'), `the first screen asked for its data: ${asked.join(' ')}`);
  assert.equal(reloads.length, 0, 'no reload (the loop)');
  assert.ok(el('#login').classList.contains('hidden'), 'the sign-in form stays hidden');
  assert.ok(!el('#app').classList.contains('hidden'), 'the page is shown');
  assert.match(el('#toast').textContent, /the database is busy/);
  assert.equal(el('#toast').getAttribute('aria-live'), 'assertive', 'a failure is said at once');
});

test('in Privacy Mode the switch is on and "identifiers shown" is not', () => {
  assert.equal(el('#btn-privacy').getAttribute('aria-pressed'), 'true');
  assert.ok(el('#ids-shown').classList.contains('hidden'));
  assert.ok(document.body.classList.contains('privacy'));
  assert.equal(el('#nav-viewer').getAttribute('aria-disabled'), 'true', 'no viewer linked: its tab says it is disabled');
});
