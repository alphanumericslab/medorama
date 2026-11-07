// The page's colours (web/style.css): red is for alarms only (--alarm), in the light and the
// dark theme; every other colour token stays clear of red.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const css = fs.readFileSync(new URL('../../web/style.css', import.meta.url), 'utf8');

function hsl(hex) {
  const n = parseInt(hex.slice(1).replace(/^(.)(.)(.)$/, '$1$1$2$2$3$3'), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => v / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2, d = max - min;
  if (!d) return { h: 0, s: 0, l };
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return { h: (h * 60 + 360) % 360, s, l };
}

test('red only for alarms', () => {
  const tokens = [...css.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]{3,6})\b/g)];
  assert.ok(tokens.length > 40, `${tokens.length} colour tokens`);
  const red = tokens
    .filter(([, name]) => !name.startsWith('alarm'))
    .filter(([, , hex]) => { const c = hsl(hex); return (c.h >= 345 || c.h <= 15) && c.s > 0.4 && c.l > 0.2 && c.l < 0.8; })
    .map(([, name, hex]) => `--${name} ${hex}`);
  assert.deepEqual(red, [], 'red outside the alarm tokens');
  assert.ok(tokens.some(([, name]) => name === 'alarm'), 'the alarm colour is defined');
});
