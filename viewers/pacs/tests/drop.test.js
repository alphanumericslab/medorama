// node --test viewers/pacs/tests/*.test.js — a dropped folder's files (app.js, PacsDrop): every
// batch of a folder's reader is read, and hidden files and folders are passed over.
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const { droppedFiles } = require('../app.js');

// FileSystemEntry stand-ins; a folder's reader gives `batch` entries a call, then an empty batch
function file(name) {
  return { name, isFile: true, isDirectory: false, file: (ok) => setTimeout(() => ok({ name }), 0) };
}
function folder(name, children, batch = 100) {
  return {
    name, isFile: false, isDirectory: true,
    createReader() {
      let at = 0;
      return { readEntries(ok) { const es = children.slice(at, at + batch); at += es.length; setTimeout(() => ok(es), 0); } };
    },
  };
}

test('a folder of more than 100 entries: every one, over several batches', async () => {
  const many = Array.from({ length: 250 }, (_, i) => file(`IM${i}.dcm`));
  const files = await droppedFiles([folder('study', many)]);
  assert.strictEqual(files.length, 250);
  assert.deepStrictEqual(new Set(files.map((f) => f.name)).size, 250);
});

test('hidden files and folders are passed over, at any depth', async () => {
  const tree = folder('study', [
    file('a.dcm'), file('.DS_Store'),
    folder('.git', [file('HEAD'), file('config')]),
    folder('series', [file('b.dcm'), folder('.thumbs', [file('c.dcm')]), file('._b.dcm')], 1),
  ]);
  const files = await droppedFiles([tree, file('d.dcm'), folder('.hidden', [file('e.dcm')])]);
  assert.deepStrictEqual(files.map((f) => f.name).sort(), ['a.dcm', 'b.dcm', 'd.dcm']);
});

test('a reader that fails gives what it read so far', async () => {
  const broken = { name: 'x', isFile: false, isDirectory: true, createReader: () => ({ readEntries: (ok, fail) => fail(new Error('gone')) }) };
  assert.deepStrictEqual(await droppedFiles([broken]), []);
});
