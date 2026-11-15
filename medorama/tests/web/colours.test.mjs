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

const token = (theme, name) => {
  const block = theme === 'dark' ? css.match(/:root\[data-theme="dark"\] \{([^}]*)\}/)[1] : css.match(/:root \{([^}]*)\}/)[1];
  return block.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`))[1];
};
const lum = (hex) => {
  const n = parseInt(hex.slice(1), 16);
  const f = (v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return 0.2126 * f((n >> 16) & 255) + 0.7152 * f((n >> 8) & 255) + 0.0722 * f(n & 255);
};
const contrast = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };

test('the busy colour reads at 5:1 on the page, and its badge text on it at 4.5:1, in both themes', () => {
  for (const t of ['light', 'dark']) {
    if (t === 'light') assert.ok(contrast(token(t, 'busy'), token(t, 'bg')) >= 5, `${t}: busy on the page`);
    assert.ok(contrast(token(t, 'on-busy'), token(t, 'busy')) >= 4.5, `${t}: a count's text on busy`);
  }
});

test('no colour is written outside the tokens where a warning is shown (the tooltip)', () => {
  assert.match(css, /\.tip \.bd \{ color: var\(--tip-warn\)/);
  assert.doesNotMatch(css, /#ff9b93/);
});
