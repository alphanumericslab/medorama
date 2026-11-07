// host.js: the context a hosting program gives a viewer (Medorama: whose file it is).
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const Host = require('../host.js');

test('the context, escaped, with only the buttons offered', () => {
  const p = new URLSearchParams({ who: 'Doe, Jane · <b>P1</b>', what: '12-lead ECG · Feb 20, 2024', pos: '3 of 4', nav: 'back,next' });
  const c = Host.context(p);
  assert.deepStrictEqual([c.who, c.pos, [...c.nav]], ['Doe, Jane · <b>P1</b>', '3 of 4', ['back', 'next']]);
  const h = Host.html(c);
  assert.ok(h.includes('&lt;b&gt;P1&lt;/b&gt;') && !h.includes('<b>P1'), 'escaped');
  assert.ok(/data-host="back"[^>]*>‹ Patient/.test(h));
  assert.ok(/data-host="prev"[^>]*disabled/.test(h) && !/data-host="next"[^>]*disabled/.test(h));
  assert.strictEqual(Host.context(new URLSearchParams('src=x&name=y')), null, 'standalone: no context, the file name stays');
});

test('the three viewers carry the same host.js', () => {
  const read = (p) => fs.readFileSync(path.join(__dirname, '..', '..', p), 'utf8');
  const a = read('ecg/host.js');
  assert.strictEqual(read('pacs/host.js'), a, 'viewers/pacs/host.js must be a copy of viewers/ecg/host.js');
  assert.strictEqual(read('vitalstream/web/js/host.js'), a, 'viewers/vitalstream/web/js/host.js must be a copy of viewers/ecg/host.js');
});
