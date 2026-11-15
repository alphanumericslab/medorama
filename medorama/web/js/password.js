// A password asked for in a dialog of the page, never in the browser's prompt() (which shows
// it as it is typed): the user's own change (the current one, then the new one twice) and an
// admin's reset of a user's (the new one twice).  Each field is a password field (hidden as
// typed, and offered to the browser's password manager as a new password).

import { esc } from './core.js';

/** The fewest characters of a password (the server refuses fewer: users.rs). */
export const MIN_PASSWORD = 10;

/** The dialog's HTML (exported for the tests). */
export function passwordForm({ title, who = '', current = false }) {
  return `<form method="dialog" class="sf-body" id="pw-form">
    <div class="sf-head"><h2 id="pw-title">${esc(title)}</h2><button type="button" class="icon" data-close title="Close" aria-label="Close">✕</button></div>
    ${who ? `<input type="text" id="pw-who" autocomplete="username" value="${esc(who)}" hidden>` : ''}
    ${current ? '<label class="pwl">Current password <input id="pw-old" type="password" autocomplete="current-password" required></label>' : ''}
    <label class="pwl">New password <input id="pw-new" type="password" autocomplete="new-password" minlength="${MIN_PASSWORD}" required aria-describedby="pw-hint"></label>
    <label class="pwl">The new password again <input id="pw-again" type="password" autocomplete="new-password" minlength="${MIN_PASSWORD}" required></label>
    <p id="pw-hint" class="muted small">At least ${MIN_PASSWORD} characters.</p>
    <p id="pw-msg" class="err small" role="alert"></p>
    <div class="ftools"><span class="spacer"></span><button type="button" data-close>Cancel</button><button id="pw-ok" value="ok" class="primary">Change the password</button></div>
  </form>`;
}

/**
 * Ask for a new password (and, with `current`, the one in use).  Resolves to { old, password },
 * or null when cancelled (Cancel, ✕ or Escape).
 */
export function askPassword({ title, who = '', current = false }) {
  const dlg = document.createElement('dialog');
  dlg.className = 'sf pw';
  dlg.setAttribute('aria-labelledby', 'pw-title');
  dlg.innerHTML = passwordForm({ title, who, current });
  document.body.append(dlg);
  const again = dlg.querySelector('#pw-again');
  const fresh = dlg.querySelector('#pw-new');
  // the two entries must be the same: said by the form's own check, before it closes
  const same = () => again.setCustomValidity(again.value && again.value !== fresh.value ? 'The two entries differ' : '');
  fresh.addEventListener('input', same);
  again.addEventListener('input', same);
  dlg.addEventListener('click', (e) => { if (e.target.closest('[data-close]')) dlg.close(''); });
  return new Promise((resolve) => {
    dlg.addEventListener('close', () => {
      const r = dlg.returnValue === 'ok' ? { old: dlg.querySelector('#pw-old')?.value ?? '', password: fresh.value } : null;
      dlg.remove();
      resolve(r);
    }, { once: true });
    dlg.returnValue = '';
    dlg.showModal();
    (dlg.querySelector('#pw-old') || fresh).focus();
  });
}
