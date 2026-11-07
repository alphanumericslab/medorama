// The import progress as the page words it (web/js/imports.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import { describe, duration, progressHtml, size } from '../../web/js/imports.js';

test('durations and sizes as people read them', () => {
  assert.equal(duration(42.4), '42 s');
  assert.equal(duration(150), '3 min');
  assert.equal(duration(4980), '1 h 23 min');
  assert.equal(size(512), '1 kB');
  assert.equal(size(3_400_000), '3.4 MB');
  assert.equal(size(150_000_000), '150 MB');
  assert.equal(size(2_500_000_000), '2.5 GB');
});

test('the progress in words', () => {
  assert.equal(describe({ phase: 'rows', rows: 1234567, rate: 12000, left_s: 125 }), 'Reading and linking rows · 1,234,567 rows · 12,000 rows/s · about 2 min left');
  assert.equal(describe({ phase: 'index', rows: 10 }), 'Building the search index · 10 rows');
  assert.equal(describe({ phase: 'rows', cancelling: true }), 'Stopping (rolling back)…');
});

test('a progress block: its bar, and Cancel only while it runs', () => {
  const running = progressHtml({ id: 'a1', label: 'muse <ecg>', state: 'running', progress: { phase: 'rows', done: 0.47, rows: 10 } });
  assert.match(running, /aria-valuenow="47"/);
  assert.match(running, /data-task-cancel="a1"/);
  assert.match(running, /muse &lt;ecg&gt;/, 'the name is escaped');
  const up = progressHtml({ id: 'upload', label: 'x', state: 'running' }, { upload: { fraction: 0.5, loaded: 5e6, total: 1e7 } });
  assert.match(up, /Uploading 5.0 MB of 10 MB/);
  assert.doesNotMatch(up, /data-task-cancel/);
  assert.doesNotMatch(progressHtml({ id: 'b', label: 'y', state: 'done', progress: { done: 1 } }), /data-task-cancel/);
});
