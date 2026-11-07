// An accessibility check of what the page shows now (tests/browser.rs runs it on every tab, in
// the light and the dark theme): the WCAG 2.1 AA rules a script can decide.
//   - the page has a language and a title;
//   - every control (input, select, textarea, button, link) has a name a screen reader reads:
//     its text, a <label>, aria-label, aria-labelledby or title;
//   - every picture has a text (alt), empty when it only decorates;
//   - ids are unique (labels and aria-* point by id);
//   - text contrasts with its background: 4.5:1, or 3:1 for large text (WCAG 1.4.3).
// It returns the problems found, each once, as short lines.
(() => {
  const out = new Set();
  const visible = (e) => {
    if (!e.isConnected || e.closest('[hidden], .hidden, [aria-hidden="true"]')) return false;
    const r = e.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return false;
    const s = getComputedStyle(e);
    return s.visibility !== 'hidden' && s.display !== 'none' && +s.opacity !== 0;
  };
  const where = (e) => {
    const id = e.id ? `#${e.id}` : '';
    const cls = !id && typeof e.className === 'string' && e.className.trim() ? `.${e.className.trim().split(/\s+/)[0]}` : '';
    const view = e.closest('[id^="view-"], #login, header, #drawer, dialog');
    return `${view && view !== e ? `${view.id || view.tagName.toLowerCase()} ` : ''}${e.tagName.toLowerCase()}${id}${cls}`;
  };
  const text = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const named = (e) => {
    if (text(e.getAttribute('aria-label')) || text(e.getAttribute('title'))) return true;
    const by = e.getAttribute('aria-labelledby');
    if (by && by.split(/\s+/).some((i) => text(document.getElementById(i)?.textContent))) return true;
    if (e.labels && [...e.labels].some((l) => text(l.textContent))) return true;
    if (['BUTTON', 'A', 'SUMMARY', 'OPTION'].includes(e.tagName) || ['button', 'tab', 'link'].includes(e.getAttribute('role'))) {
      if (text(e.textContent)) return true;
      if ([...e.querySelectorAll('img[alt], svg title')].some((x) => text(x.getAttribute('alt') || x.textContent))) return true;
    }
    if (e.tagName === 'INPUT' && ['submit', 'button', 'reset'].includes(e.type) && text(e.value)) return true;
    return false;
  };

  if (!document.documentElement.lang) out.add('the page has no language (html lang)');
  if (!text(document.title)) out.add('the page has no title');

  const ids = new Map();
  for (const e of document.querySelectorAll('[id]')) ids.set(e.id, (ids.get(e.id) || 0) + 1);
  for (const [id, n] of ids) if (n > 1) out.add(`the id "${id}" is used ${n} times`);

  const controls = 'input:not([type="hidden"]), select, textarea, button, a[href], summary, [role="button"], [role="tab"], [tabindex]:not([tabindex="-1"])';
  for (const e of document.querySelectorAll(controls)) {
    if (visible(e) && !named(e)) out.add(`a control with no name: ${where(e)}`);
  }
  for (const e of document.querySelectorAll('img')) {
    if (visible(e) && !e.hasAttribute('alt')) out.add(`a picture with no alt text: ${where(e)}`);
  }

  // contrast: each element that holds text of its own, against the first background behind it
  const rgba = (c) => {
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) return null;
    const p = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
  };
  const over = (top, under) => ({
    r: top.r * top.a + under.r * (1 - top.a),
    g: top.g * top.a + under.g * (1 - top.a),
    b: top.b * top.a + under.b * (1 - top.a),
    a: 1,
  });
  const background = (e) => {
    const layers = [];
    for (let x = e; x; x = x.parentElement) {
      const s = getComputedStyle(x);
      if (s.backgroundImage !== 'none') return null; // a picture or a gradient: not decided here
      const c = rgba(s.backgroundColor);
      if (c && c.a > 0) {
        layers.push(c);
        if (c.a >= 1) break;
      }
    }
    let c = { r: 255, g: 255, b: 255, a: 1 };
    for (const l of layers.reverse()) c = over(l, c);
    return c;
  };
  const lum = (c) => {
    const f = (v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
    return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
  };
  const ratio = (a, b) => {
    const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
    return (x + 0.05) / (y + 0.05);
  };
  for (const e of document.body.querySelectorAll('*')) {
    if (['SCRIPT', 'STYLE', 'SVG', 'OPTION', 'CANVAS'].includes(e.tagName.toUpperCase())) continue;
    const own = [...e.childNodes].some((n) => n.nodeType === 3 && text(n.textContent));
    if (!own || !visible(e) || e.closest('svg, canvas, :disabled, [aria-disabled="true"]')) continue;
    const s = getComputedStyle(e);
    const fg = rgba(s.color);
    const bg = background(e);
    if (!fg || !bg) continue;
    const r = ratio(over(fg, bg), bg);
    const px = parseFloat(s.fontSize);
    const large = px >= 24 || (px >= 18.66 && +s.fontWeight >= 700);
    const need = large ? 3 : 4.5;
    if (r < need) out.add(`low contrast ${r.toFixed(2)}:1 (needs ${need}): ${where(e)} "${text(e.textContent).slice(0, 30)}"`);
  }
  return [...out];
})()
