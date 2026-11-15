// Passwords and Privacy Mode's settings (web/js/password.js, views/admin.js): a password is
// typed in a password field (never prompt()); Cancel never submits; the identifiers Privacy Mode
// always withholds are not offered as choices, and stay withheld when the settings are saved.
import test from 'node:test';
import assert from 'node:assert/strict';
import { install } from './dom.mjs';

install();
const { passwordForm } = await import('../../web/js/password.js');
const { ALWAYS, deidFields } = await import('../../web/js/views/admin.js');

test('a new password is typed in password fields, at least 10 characters, for a password manager', () => {
  const h = passwordForm({ title: 'Change your password', who: 'boss', current: true });
  assert.match(h, /id="pw-old" type="password" autocomplete="current-password" required/);
  for (const id of ['pw-new', 'pw-again']) assert.match(h, new RegExp(`id="${id}" type="password" autocomplete="new-password" minlength="10" required`));
  assert.doesNotMatch(passwordForm({ title: 'Reset' }), /pw-old|autocomplete="username"/, "a reset: no current password, nobody's user name");
});

test("Cancel and ✕ are not submit buttons: Enter in the form changes the password, it never cancels", () => {
  const h = passwordForm({ title: 'x' });
  const buttons = h.match(/<button[^>]*>/g);
  const firstSubmit = buttons.find((b) => !b.includes('type="button"'));
  assert.match(firstSubmit, /id="pw-ok" value="ok"/);
  assert.equal(buttons.filter((b) => b.includes('data-close')).every((b) => b.includes('type="button"')), true);
});

test('names, MRNs, dates of birth and account numbers: always withheld, kept when saving', () => {
  assert.deepEqual([...ALWAYS].sort(), ['account_numbers', 'birth_date', 'mrn', 'names']);
  assert.deepEqual(deidFields(['dates'], ['names', 'mrn', 'notes', 'dates']).sort(), ['dates', 'mrn', 'names']);
  assert.deepEqual(deidFields(['notes'], []), ['notes'], 'a server that no longer lists them: nothing added');
});
