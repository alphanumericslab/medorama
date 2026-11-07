// Medorama page: sign-in, the top bar, and the screens (one module each).
// Talks to api/v1 with relative URLs, so it also works behind a reverse proxy.

import { $, $$, api, privacy, esc, fail, isAdmin, canRun, phi, resetFormats, setUnauthorized, st, store, toast, viewerMode } from './core.js';
import * as router from './router.js';
import patients from './views/patients.js';
import encounters from './views/encounters.js';
import patient from './views/patient.js';
import recordings from './views/recordings.js';
import viewer from './views/viewer.js';
import actions from './views/actions.js';
import jobs, { pollJobs } from './views/jobs.js';
import history from './views/history.js';
import sources from './views/sources.js';
import review, { reviewBadge } from './views/review.js';
import admin from './views/admin.js';
import { APPS, appView, checkApps, markIdentifiable } from './views/apps.js';
import { invalidateBanner } from './views/banner.js';
import { checkData, onDataChange, stopChecking } from './live.js';
import { resume as resumeImports } from './imports.js';
import databases from './views/databases.js';
import cohorts from './views/cohorts.js';

const VIEWS = {
  patients, encounters, patient, recordings, viewer, actions, jobs, history, sources, review, admin, databases, cohorts,
  ...Object.fromEntries(Object.keys(APPS).map((a) => [a, appView(a)])),
};

function applyTheme(t) {
  const sys = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  document.documentElement.dataset.theme = t || sys;
  window.dispatchEvent(new Event('themechange'));
  // the viewers in their tabs follow
  document.querySelectorAll('#view-apps iframe[src]').forEach((f) => {
    const p = new URLSearchParams(f.dataset.hash || '');
    p.set('theme', t || sys);
    f.dataset.hash = p.toString();
    try { f.contentWindow.location.hash = f.dataset.hash; } catch { /* not loaded yet */ }
  });
  // … and VitalStream: its own address (what it shows), with the theme changed
  const vs = document.getElementById('viewer-frame');
  if (vs?.getAttribute('src')) {
    try {
      const p = new URLSearchParams(vs.contentWindow.location.hash.slice(1));
      if (p.get('theme') !== (t || sys)) {
        p.set('theme', t || sys);
        vs.contentWindow.location.hash = p.toString();
      }
    } catch { /* not loaded yet: it opens with the page's theme (views/viewer.js) */ }
  }
}

// Signed out (or the session expired): the form, once. Later refusals (the data check, a
// viewer's request) leave it as it is, so that what is being typed is not interrupted.  When
// the institution's sign-in (a proxy in front of Medorama) signs users in, there is no form:
// the page says so (and why, when the sign-in named someone without an account here).  A page
// that showed a user's screens is loaded again first: nothing they saw (a patient, identified)
// may stay in it for the next person at the browser.
async function showLogin(why) {
  stopChecking();
  if (st.user) {
    window.location.reload();
    return;
  }
  const box = $('#login');
  if (!box.classList.contains('hidden') && !why) return;
  $('#app').classList.add('hidden');
  box.classList.remove('hidden');
  const how = await api('sign-in').catch(() => ({ method: 'password' }));
  const proxy = how.method === 'proxy';
  $('#login-form').classList.toggle('hidden', proxy);
  $('#login-proxy').classList.toggle('hidden', !proxy);
  if (proxy) {
    $('#login-proxy-msg').textContent = why
      ? `${why[0].toUpperCase()}${why.slice(1)}.`
      : "You sign in to Medorama through your institution's sign-in, which has not signed you in (or your session with it has ended).";
  } else {
    $('#login-user').focus();
  }
}

/** Sign out: of Medorama's session, or (the institution's sign-in) through its own sign-out. */
async function signOut(everywhere) {
  const r = await api('logout', { method: 'POST', json: everywhere ? { everywhere: true } : undefined }).catch(() => ({}));
  if (r?.logout_url) window.location.assign(r.logout_url);
  else window.location.reload();
}

async function login(e) {
  e.preventDefault();
  $('#login-error').classList.add('hidden');
  try {
    const r = await api('login', { method: 'POST', json: { username: $('#login-user').value, password: $('#login-pass').value } });
    $('#login-pass').value = '';
    // another user than the one this page showed: loaded again, empty
    if (st.user && st.user.id !== r?.user?.id) {
      window.location.reload();
      return;
    }
    await start();
  } catch (err) {
    $('#login-error').textContent = err.message;
    $('#login-error').classList.remove('hidden');
  }
}

async function start() {
  st.cfg = await api('config');
  st.user = st.cfg.user;
  resetFormats();
  $('#login').classList.add('hidden');
  $('#app').classList.remove('hidden');
  const u = st.user;
  const name = u.display_name || u.username;
  $('#btn-user').textContent = name.split(/[\s._-]+/).filter(Boolean).slice(0, 2).map((w) => w[0].toUpperCase()).join('') || '?';
  $('#who').innerHTML = `<b>${esc(name)}</b><span class="pill">${esc(u.role)}</span>${phi() ? ' <span class="pill phi" title="May see identifiers">PHI</span>' : ''}`;
  $$('.admin-only').forEach((e) => e.classList.toggle('hidden', !isAdmin()));
  // the institution's sign-in keeps the user's password and sessions, not Medorama
  const proxy = st.cfg.sign_in === 'proxy';
  $('#btn-password').classList.toggle('hidden', proxy);
  $('#btn-logout-all').classList.toggle('hidden', proxy);
  $$('.phi-only').forEach((e) => e.classList.toggle('hidden', !phi()));
  $$('.run-only').forEach((e) => e.classList.toggle('hidden', !canRun()));
  const vb = $('#nav-viewer');
  vb.classList.toggle('disabled', viewerMode() !== 'embedded');
  vb.title = viewerMode() === 'embedded' ? `The ${st.cfg.viewer.flavor || ''} viewer` : (st.cfg?.viewer?.reason || 'No viewer is linked to the catalog');
  if (viewerMode() === 'link') vb.classList.add('hidden');
  showPrivacy();
  await router.render();
  pollJobs();
  checkData();
  if (isAdmin()) {
    reviewBadge().catch(() => {});
    resumeImports().catch(() => {});
  }
}

/** The Privacy Mode switch as the session is: on (highlighted) or off; locked on for users
 * without the PHI permission. */
function showPrivacy() {
  const b = $('#btn-privacy');
  b.classList.remove('hidden');
  b.classList.toggle('on', privacy.on);
  b.classList.toggle('locked', !privacy.allowed);
  b.setAttribute('aria-pressed', privacy.on ? 'true' : 'false');
  b.title = !privacy.allowed
    ? 'Privacy Mode is always on for your account: no identifier is shown, dates are moved back'
    : privacy.on
      ? 'Privacy Mode is on: no identifier is shown, dates are moved back. Click to show identifiers'
      : 'Privacy Mode is off: identifiers are shown. Click to hide them';
  document.body.classList.toggle('privacy', privacy.on);
  markIdentifiable();
}

async function changeOwnPassword() {
  const old = prompt('Current password:');
  if (old === null) return;
  const pw = prompt('New password (at least 10 characters):');
  if (pw === null) return;
  if (prompt('Repeat the new password:') !== pw) return toast('The two entries differ', true);
  await api('me/password', { method: 'POST', json: { old, new: pw } });
  toast('Password changed; please sign in again');
  setTimeout(showLogin, 1500);
}

function wire() {
  $('#login-form').addEventListener('submit', login);
  $('#hist-back').addEventListener('click', router.back);
  $('#hist-fwd').addEventListener('click', router.forward);
  $('#btn-theme').addEventListener('click', () => {
    const t = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
    store.set('theme', t);
    applyTheme(t);
  });
  // the server switches the session (and audits it); the page reloads, so nothing identified
  // stays behind
  $('#btn-privacy').addEventListener('click', () => {
    if (!privacy.allowed) return toast('Privacy Mode is always on for your account', false);
    privacy.set(!privacy.on).catch(fail);
  });
  $('#btn-user').addEventListener('click', (e) => { e.stopPropagation(); $('#user-pop').classList.toggle('hidden'); });
  document.addEventListener('click', (e) => { if (!e.target.closest('.usermenu')) $('#user-pop').classList.add('hidden'); });
  // what is drawn as a button without being one (role="button": a report's entries, trends,
  // encounters, notes) answers Enter and Space as a button does
  document.addEventListener('keydown', (e) => {
    const t = e.target;
    if ((e.key === 'Enter' || e.key === ' ') && t instanceof Element && t.matches('[role="button"]:not(button, a, input, select, textarea)')) {
      e.preventDefault();
      t.click();
    }
  });
  $('#btn-password').addEventListener('click', () => changeOwnPassword().catch(fail));
  $('#btn-logout').addEventListener('click', () => signOut(false));
  $('#btn-logout-all').addEventListener('click', () => signOut(true));
  // nav links: the router (a new history entry each time)
  $('header.top').addEventListener('click', (e) => {
    const a = e.target.closest('a[href^="#/"]');
    if (!a) return;
    e.preventDefault();
    router.go(a.getAttribute('href')).catch(fail);
  });
  document.addEventListener('click', (e) => {
    const a = e.target.closest('a[href^="#/"]');
    if (!a || a.closest('header.top') || e.metaKey || e.ctrlKey || e.button) return;
    e.preventDefault();
    router.go(a.getAttribute('href')).catch(fail);
  });
  for (const [name, v] of Object.entries(VIEWS)) {
    router.view(name, v);
    v.wire?.();
  }
  // Import: databases, and the recordings under it
  router.view('import', sources);
  // when the data change (an import, a removal; here or elsewhere): redraw what shows data,
  // and close viewers whose file is gone
  const LIVE = new Set(['patients', 'patient', 'cohorts', 'databases', 'encounters', 'recordings', 'import', 'sources', 'review']);
  onDataChange(async () => {
    for (const v of Object.values(VIEWS)) v.invalidate?.();
    invalidateBanner();
    await checkApps();
    if (!LIVE.has(router.route().name)) return;
    // the page's own redraw, not the user's: its requests do not keep an idle session alive
    st.redrawing = true;
    try { await router.render(); } finally { st.redrawing = false; }
  });
}

// full screen: the top strip of the screen belongs to the system (macOS: the menu bar, beside a
// camera notch too; the browser's toolbar slides over it), so the bar starts below it (style.css)
function watchFullscreen() {
  const check = () => {
    const full = matchMedia('(display-mode: fullscreen)').matches
      || (Math.abs(window.innerHeight - screen.height) <= 1 && Math.abs(window.innerWidth - screen.width) <= 1);
    document.body.classList.toggle('fullscreen', full);
  };
  window.addEventListener('resize', check);
  matchMedia('(display-mode: fullscreen)').addEventListener?.('change', check);
  check();
}

async function init() {
  watchFullscreen();
  // whatever no screen caught is said, not lost (a screen never fails silently)
  router.onError(fail);
  window.addEventListener('unhandledrejection', (e) => { fail(e.reason || e); });
  applyTheme(store.get('theme', null));
  setUnauthorized(showLogin);
  wire();
  router.start();
  try {
    await start();
  } catch (e) {
    // refused with a reason (the institution's sign-in named someone without an account)
    showLogin(/no account/.test(e?.message || '') ? e.message : undefined);
  }
}

init();
