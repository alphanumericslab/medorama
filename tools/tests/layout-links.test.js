// Tests of the website layout's link script (_layouts/default.html): links to files that are
// not published lead to GitHub, and a malformed link does not stop the others.
//   node --test tools/tests/
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const layout = fs.readFileSync(path.join(__dirname, '..', '..', '_layouts', 'default.html'), 'utf8');
// the first function of the page's script, with the site's values in place of Liquid's
const script = layout.match(/<script>\s*([\s\S]*?\}\)\(\);)/)[1]
  .replace('{{ site.baseurl | jsonify }}', '"/medorama"')
  .replace(/\{% for p in site\.pages %\}.*?\{% endfor %\}/, 'published["/docs/index.html"] = 1;')
  .replace(/\{% for f in site\.static_files %\}.*?\{% endfor %\}/, '')
  .replace('{{ site.repository }}', 'alphanumericslab/medorama');

function run(hrefs) {
  const links = hrefs.map((href) => ({ href, getAttribute: () => href }));
  vm.runInNewContext(script, {
    URL,
    location: { href: 'https://example.org/medorama/docs/index.html', origin: 'https://example.org' },
    document: { querySelectorAll: () => links },
  });
  return links.map((a) => a.href);
}

test('links to unpublished files lead to GitHub; published pages stay', () => {
  assert.deepStrictEqual(run(['index.html', '../Cargo.toml']), [
    'index.html',
    'https://github.com/alphanumericslab/medorama/blob/main/Cargo.toml',
  ]);
});

test('a malformed link is left as it is, and the links after it are still rewritten', () => {
  assert.deepStrictEqual(run(['bad%E0%A4%A.md', '../Cargo.toml']), [
    'bad%E0%A4%A.md',
    'https://github.com/alphanumericslab/medorama/blob/main/Cargo.toml',
  ]);
});
