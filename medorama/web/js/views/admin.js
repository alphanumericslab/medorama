// Admin: users, settings, add-ons, the audit log.

import { $, api, esc, fail, fmtDay, fmtTime, resetFormats, st, toast } from '../core.js';

// ------------------------------------------------------------ Privacy Mode: what it withholds
function showDisplay(d) {
  const opts = (xs, on) => xs.map((x) => `<option value="${esc(x.key)}"${x.key === on ? ' selected' : ''}>${esc(x.example)} · ${esc(x.label)}</option>`).join('');
  $('#disp-date').innerHTML = opts(d.dates, d.date);
  $('#disp-clock').innerHTML = opts(d.clocks, d.clock);
  $('#disp-zone').textContent = st.cfg?.timezone || 'UTC';
  previewDisplay();
}
/** The current time as the choices write it (applied to the page only when saved). */
function previewDisplay() {
  const saved = st.cfg.display;
  st.cfg.display = { ...saved, date: $('#disp-date').value, clock: $('#disp-clock').value };
  const now = Date.now() / 1000;
  $('#disp-now').textContent = fmtTime(now);
  $('#disp-long').textContent = fmtDay(now);
  st.cfg.display = saved;
}

function showDeid(d) {
  $('#deid-fields').innerHTML = d.choices.map((c) => `<label class="inline"><input type="checkbox" data-deid="${esc(c.key)}"${d.fields.includes(c.key) ? ' checked' : ''}> ${esc(c.label)}${c.hipaa ? ' <span class="pill" title="An identifier of HIPAA\'s Safe Harbor list">HIPAA</span>' : ''}</label>`).join('');
  for (const k of ['years', 'months', 'days', 'hours']) $(`#deid-${k}`).value = d.offset[k];
  $('#deid-spread').value = d.spread_days ?? 365;
  sumDeid();
}
function sumDeid() {
  const o = Object.fromEntries(['years', 'months', 'days', 'hours'].map((k) => [k, Math.max(0, Math.floor(+$(`#deid-${k}`).value || 0))]));
  const ref = new Date(Date.UTC(2000, 0, 1));
  const back = new Date(Date.UTC(2000 - o.years, 0 - o.months, 1) - (o.days * 24 + o.hours) * 3600e3);
  const s = (ref - back) / 1000;
  const spread = Math.max(0, Math.floor(+$('#deid-spread').value || 0));
  $('#deid-sum').textContent = `= ${Math.floor(s / 86400)} days${s % 86400 ? ` ${Math.round((s % 86400) / 3600)} h` : ''}${spread ? ` and up to ${spread} more` : ''}: 2000-01-01 shows as ${back.toISOString().slice(0, 16).replace('T', ' ')}${spread ? ' or earlier' : ''}`;
  return o;
}

async function load() {
  const [users, audit, d, addons] = await Promise.all([api('users'), api('audit?limit=300'), api('deid'), api('admin/addons')]);
  showDeid(d);
  if (st.cfg.display) showDisplay(st.cfg.display);
  // the institution's sign-in (sign_in = "proxy"): Medorama's passwords are not used
  const proxy = st.cfg.sign_in === 'proxy';
  $('#usr-pass').classList.toggle('hidden', proxy);
  $('#usr-pass').required = !proxy;
  let html = '<tr><th>User</th><th>Role</th><th title="Sees identifiers: may leave Privacy Mode (without it, Privacy Mode is always on)">PHI permission</th><th>Enabled</th><th></th></tr>';
  for (const u of users.items) {
    const roles = ['viewer', 'analyst', 'admin'].map((r) => `<option${r === u.role ? ' selected' : ''}>${r}</option>`).join('');
    html += `<tr><td>${esc(u.username)}<div class="muted">${esc(u.display_name)}</div></td>
      <td><select data-user-role="${u.id}" aria-label="Role of ${esc(u.username)}">${roles}</select></td>
      <td><input type="checkbox" data-user-phi="${u.id}"${u.phi ? ' checked' : ''} aria-label="PHI permission of ${esc(u.username)}"></td>
      <td><input type="checkbox" data-user-enabled="${u.id}"${u.disabled ? '' : ' checked'} aria-label="${esc(u.username)} enabled"></td>
      <td class="nowrap">${proxy ? '' : `<button class="link" data-user-pass="${u.id}">Reset password</button>`}
        <button class="link" data-user-signout="${u.id}" title="Ends their sessions on every computer and browser">Sign out</button>
        ${u.id === st.user.id ? '' : `<button class="link danger" data-user-del="${u.id}" data-name="${esc(u.username)}">Delete</button>`}</td></tr>`;
  }
  $('#usr-table').innerHTML = html;
  html = '<tr><th>Time</th><th>User</th><th>Event</th><th>Target</th><th>Detail</th></tr>';
  for (const a of audit.items) {
    html += `<tr><td class="nowrap">${fmtTime(a.time, true)}</td><td>${esc(a.username)}</td><td>${esc(a.event)}</td><td class="path">${esc(a.target)}</td><td class="muted">${esc(a.detail)}</td></tr>`;
  }
  $('#audit-table').innerHTML = html;
  html = '<tr><th>Adapter</th><th>Version</th><th>Formats</th><th>Program</th></tr>';
  for (const a of addons.items) {
    const formats = a.formats.map((f) => `${esc(f.label)} <span class="muted">(${f.extensions.map((e) => `.${esc(e)}`).join(', ')})</span>`).join('<br>');
    html += `<tr><td>${esc(a.name)}</td><td>${esc(a.version)}${a.implementation ? ` <span class="muted">${esc(a.implementation)}</span>` : ''}</td><td>${formats}</td><td class="path">${esc(a.program)}</td></tr>`;
  }
  if (!addons.items.length) html += `<tr><td colspan="4" class="muted">None installed in ${esc(addons.dir)}</td></tr>`;
  $('#addon-table').innerHTML = html;
  const c = st.cfg;
  $('#cfg-list').innerHTML = [
    ['Version', c.version], ['Results', c.results_dir], ['Indexes', c.index_dir], ['Time zone', c.timezone],
    ['Workers', c.workers], ['Automatic indexing', c.auto_index ? 'on' : 'off'],
    ['Viewer', { embedded: `the ${c.viewer.flavor} viewer, run by the catalog${c.viewer.running ? ' (running)' : ''}`, link: `${c.viewer.url} (viewer_url)`, none: `none: ${c.viewer.reason}` }[c.viewer.mode]],
  ].map(([k, v]) => `<dt>${k}</dt><dd>${esc(v)}</dd>`).join('');
}

async function change(id, ch) {
  try {
    await api(`users/${id}`, { method: 'PATCH', json: ch });
    toast('Saved');
  } catch (err) {
    fail(err);
  }
  load();
}

export default {
  show: () => load(),
  wire() {
    $('#deid-form').addEventListener('input', sumDeid);
    $('#disp-form').addEventListener('change', previewDisplay);
    $('#disp-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      try {
        st.cfg.display = await api('admin/display', { method: 'PUT', json: { date: $('#disp-date').value, clock: $('#disp-clock').value } });
        resetFormats();
        showDisplay(st.cfg.display);
        load().catch(fail);
        toast('Saved: every page writes dates and times this way');
      } catch (err) { fail(err); }
    });
    $('#deid-hipaa').addEventListener('click', async () => {
      const d = await api('deid');
      showDeid({ ...d, fields: d.choices.filter((c) => c.default).map((c) => c.key), offset: { years: 0, months: 0, days: 36570, hours: 0 }, spread_days: 365 });
    });
    $('#deid-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const fields = [...document.querySelectorAll('[data-deid]')].filter((x) => x.checked).map((x) => x.dataset.deid);
      try {
        st.cfg.deid = await api('admin/deid', { method: 'PUT', json: { fields, offset: sumDeid(), spread_days: Math.max(0, Math.floor(+$('#deid-spread').value || 0)) } });
        resetFormats();
        toast('Saved: Privacy Mode withholds what is ticked');
        showDeid(st.cfg.deid);
      } catch (err) { fail(err); }
    });
    $('#usr-add').addEventListener('submit', (e) => {
      e.preventDefault();
      api('users', {
        method: 'POST',
        json: { username: $('#usr-name').value.trim(), display_name: $('#usr-display').value.trim(), role: $('#usr-role').value, phi: $('#usr-phi').checked, password: $('#usr-pass').value },
      }).then(() => { $('#usr-add').reset(); toast('User added'); load(); }).catch(fail);
    });
    $('#usr-table').addEventListener('change', (e) => {
      const t = e.target;
      if (t.dataset.userRole) change(+t.dataset.userRole, { role: t.value });
      else if (t.dataset.userPhi) change(+t.dataset.userPhi, { phi: t.checked });
      else if (t.dataset.userEnabled) change(+t.dataset.userEnabled, { disabled: !t.checked });
    });
    $('#usr-table').addEventListener('click', (e) => {
      const b = e.target.closest('[data-user-pass],[data-user-del],[data-user-signout]');
      if (!b) return;
      if (b.dataset.userSignout) {
        api(`users/${b.dataset.userSignout}`, { method: 'PATCH', json: { sign_out: true } }).then(() => toast('Signed out everywhere')).catch(fail);
      } else if (b.dataset.userPass) {
        const pw = prompt('New password (at least 10 characters):');
        if (pw) change(+b.dataset.userPass, { password: pw });
      } else if (confirm(`Delete the user ${b.dataset.name}?`)) {
        api(`users/${b.dataset.userDel}`, { method: 'DELETE' }).then(load).catch(fail);
      }
    });
  },
};
