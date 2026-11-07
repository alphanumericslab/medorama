// Time formatting and axis ticks in a chosen IANA time zone (data are Unix seconds, UTC).

const partsCache = new Map();

function formatter(tz) {
  let f = partsCache.get(tz);
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone: tz, hourCycle: 'h23', year: 'numeric', month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit', second: '2-digit', weekday: 'short',
    });
    partsCache.set(tz, f);
  }
  return f;
}

const abbrCache = new Map();
function abbrFormatter(tz) {
  let f = abbrCache.get(tz);
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', { timeZone: tz, timeZoneName: 'short' });
    abbrCache.set(tz, f);
  }
  return f;
}

// The range of a JavaScript Date (±8.64e15 ms): times beyond it (a file's absurd onset) are
// shown at its ends instead of making every drawing fail.
const MAX_MS = 8.64e15;
export const MAX_T = MAX_MS / 1000;
const clampMs = (ms) => (Number.isFinite(ms) ? Math.max(-MAX_MS, Math.min(MAX_MS, ms)) : 0);

// Calendar fields of Unix time t (seconds) in zone tz.
export function fields(t, tz) {
  const ms = clampMs(Math.floor(t * 1000));
  const out = {};
  for (const p of formatter(tz).formatToParts(new Date(ms))) out[p.type] = p.value;
  return {
    y: +out.year, mo: +out.month, d: +out.day, h: +out.hour % 24, mi: +out.minute, s: +out.second,
    wd: out.weekday, ms: ((ms % 1000) + 1000) % 1000,
  };
}

// Offset of zone tz from UTC at time t, in seconds.
export function offset(t, tz) {
  const u = Math.floor(Math.max(-MAX_T + 172800, Math.min(MAX_T - 172800, Number.isFinite(t) ? t : 0)));
  const f = fields(u, tz);
  const local = Date.UTC(f.y, f.mo - 1, f.d, f.h, f.mi, f.s) / 1000;
  return local - u;
}

// Unix time of the wall-clock time y-mo-d h:mi:sec in zone tz (fields may overflow, as in
// Date.UTC).  As Python's zoneinfo with fold=0: a time that occurs twice (the clocks go back) is
// its first occurrence; a time that does not exist (the clocks go forward) is read with the
// offset in force before the change (02:30 on a spring-forward night is 03:30).
export function toUnix(y, mo, d, h, mi, sec, tz) {
  const wall = Date.UTC(y, mo - 1, d, h, mi, sec) / 1000;
  const before = offset(wall - 86400, tz);  // the offsets a day around it: a change lies between
  const after = offset(wall + 86400, tz);
  const fits = [before, after].filter((o) => offset(wall - o, tz) === o).map((o) => wall - o);
  return fits.length ? Math.min(...fits) : wall - before;
}

export function zoneName(t, tz) {
  const p = abbrFormatter(tz).formatToParts(new Date(clampMs(t * 1000))).find((x) => x.type === 'timeZoneName');
  return p ? p.value : tz;
}

const pad = (n, w = 2) => String(n).padStart(w, '0');

export function fmtDate(t, tz) {
  const f = fields(t, tz);
  return `${f.y}-${pad(f.mo)}-${pad(f.d)}`;
}

// Full time stamp with the precision a view of `span` seconds needs.
export function fmtTime(t, tz, span = 60) {
  const f = fields(t, tz);
  let s = `${pad(f.h)}:${pad(f.mi)}:${pad(f.s)}`;
  if (span < 120) s += '.' + pad(f.ms, 3);
  return s;
}

export function fmtFull(t, tz, span = 60) {
  return `${fmtDate(t, tz)} ${fmtTime(t, tz, span)} ${zoneName(t, tz)}`;
}

export function fmtDuration(s) {
  const a = Math.abs(s);
  if (a < 1e-3) return `${(s * 1e6).toFixed(0)} µs`;
  if (a < 1) return `${(s * 1e3).toPrecision(3)} ms`;
  if (a < 120) return `${+s.toPrecision(4)} s`;
  if (a < 7200) return `${+(s / 60).toPrecision(3)} min`;
  if (a < 172800) return `${+(s / 3600).toPrecision(3)} h`;
  return `${+(s / 86400).toPrecision(3)} d`;
}

const STEPS = [
  0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30,
  60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400, 172800, 604800,
];

// Tick times for [t0, t1] across `width` pixels, aligned to local wall-clock time.  Steps of an
// hour or more are counted in wall-clock fields (each tick converted on its own), so that they
// stay on the hour, and days on midnight, across a daylight-saving change; shorter steps divide
// every change of offset and are counted in seconds.
export function ticks(t0, t1, width, tz, minPx = 95) {
  const span = t1 - t0;
  const want = Math.max(1, Math.floor(width / minPx));
  let step = STEPS[STEPS.length - 1];
  for (const s of STEPS) { if (span / s <= want) { step = s; break; } }
  const out = [];
  if (step >= 3600 && Math.abs(t0) <= MAX_T && Math.abs(t1) <= MAX_T) {
    const f = fields(t0, tz);
    let day = Date.UTC(f.y, f.mo - 1, f.d) / 86400000;      // the local date, as days since 1970
    const days = step / 86400;
    if (days >= 1) day = Math.ceil(day / days) * days;
    for (; out.length < 400; day += Math.max(1, days)) {
      const hours = days >= 1 ? [0] : Array.from({ length: 24 / (step / 3600) }, (_, i) => i * (step / 3600));
      let past = false;
      for (const h of hours) {
        const t = toUnix(1970, 1, 1 + day, h, 0, 0, tz);
        if (!Number.isFinite(t) || t > t1 + 1e-9) { past = true; break; }
        if (t >= t0 - 1e-9 && t !== out[out.length - 1]) out.push(t);   // a skipped hour: once
      }
      if (past) break;
    }
    return { step, ticks: out };
  }
  const off = offset(t0, tz);
  const first = Math.ceil((t0 + off) / step) * step - off;
  for (let t = first; t <= t1 + 1e-9 && out.length < 400; t += step) {
    out.push(Math.round(t * 1000) / 1000);
  }
  return { step, ticks: out };
}

export function tickLabel(t, step, tz) {
  const f = fields(t + 1e-6, tz);
  if (step >= 86400) return `${f.wd} ${pad(f.mo)}-${pad(f.d)}`;
  if (step >= 60) return `${pad(f.h)}:${pad(f.mi)}`;
  if (step >= 1) return `${pad(f.h)}:${pad(f.mi)}:${pad(f.s)}`;
  return `${pad(f.h)}:${pad(f.mi)}:${pad(f.s)}.${pad(f.ms, 3)}`.replace(/0+$/, '').replace(/\.$/, '');
}
