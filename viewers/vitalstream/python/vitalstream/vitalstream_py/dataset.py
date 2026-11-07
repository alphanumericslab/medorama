"""An indexed recording: answers window, vital-sign and marker queries from its index (the same
responses as the Rust and C++ viewers)."""

import bisect
import math

import numpy as np

from .common import LEVEL0, bucket_size, fmax, total_order
from .sources import reader

RAW_PER_PX = 2          # send samples as they are up to this many per pixel
MAX_RAW = 2_000_000     # never decode more samples than this for one window
VITAL_MAX_AGE_S = 600   # vitals_at: ignore values older than this


_I64 = 2.0 ** 63


def _as_i64(x):
    """A float as an integer the way Rust's `as i64` converts it: saturating, NaN -> 0."""
    if x != x:
        return 0
    if x >= _I64:
        return 2 ** 63 - 1
    if x <= -_I64:
        return -2 ** 63
    return int(x)


def _floor(x):
    return math.floor(x) if math.isfinite(x) else x


def _ceil(x):
    return math.ceil(x) if math.isfinite(x) else x


def _clamp(x, lo, hi):
    return x if x != x else min(max(x, lo), hi)


def _span(p, t0, t1):
    """Samples [a, b) of piece `p` that a window [t0, t1] takes: one sample more before, two
    after; within [0, n] (any finite times)."""
    fs, n = p['fs'], p['n']
    a = _clamp(_floor((t0 - p['t0']) * fs) - 1, 0, n)
    b = _clamp(_ceil((t1 - p['t0']) * fs) + 2, 0, n)
    return _as_i64(a), _as_i64(b)


def _fmt(x, digits):
    """JSON array text of floats (NaN -> null)."""
    fmt = f'%.{digits}g'
    return '[' + ','.join('null' if v != v or v in (math.inf, -math.inf) else fmt % v
                          for v in np.asarray(x, np.float64).tolist()) + ']'


class Dataset:
    def __init__(self, meta, arrays, folder):
        self.meta = meta
        self.arrays = arrays
        self.pieces = meta['pieces']
        # per channel, for finding the pieces in a window quickly: the pieces in order of their
        # start (then id), the latest end of the pieces up to each, and the margin around a
        # piece's span that a window may still take samples from (three samples at its lowest rate)
        self.by_channel = {}
        for i, p in enumerate(self.pieces):
            self.by_channel.setdefault(p['ch'], []).append(i)
        self._order = {}
        for ch, ids in self.by_channel.items():
            order = sorted(ids, key=lambda i: (total_order(self.pieces[i]['t0']), i))
            ends, latest = [], -math.inf
            for i in order:
                p = self.pieces[i]
                latest = fmax(latest, p['t0'] + p['n'] / p['fs'])
                ends.append(latest)
            margin = 0.0
            for i in order:
                margin = fmax(margin, 3.0 / self.pieces[i]['fs'])
            self._order[ch] = (order, [self.pieces[i]['t0'] for i in order], ends, margin)
        self.reader = reader(meta, folder)
        self.vitals = {v['name']: v for v in meta['vitals']}

    def close(self):
        self.reader.close()

    # ------------------------------------------------------------------ info
    def info(self):
        m = self.meta
        starts = [p['t0'] for p in self.pieces]
        ends = [p['t0'] + p['n'] / p['fs'] for p in self.pieces]
        vt = self.arrays['vit_t']
        if vt.size:
            starts.append(float(vt.min()))
            ends.append(float(vt.max()))
        t_lo, t_hi = min(starts, default=None), max(ends, default=None)
        return {'key': m['key'], 'name': m['name'], 'path': m['path'], 'kind': m['kind'],
                'format': m.get('format'), 'description': m.get('description'), 't_range': [t_lo, t_hi],
                'time_resolution_s': m.get('time_resolution_s'),
                'channels': m['channels'],
                'pieces': [{'id': p['id'], 'ch': p['ch'], 't0': p['t0'], 'fs': p['fs'], 'n': p['n'],
                            'timed': p.get('timed', True)} for p in self.pieces],
                'vitals': [{'name': v['name'], 'unit': v['unit'], 'n': v['n']} for v in m['vitals']],
                'alarms': m['alarms'], 'gaps': m['gaps'], 'untimed': m['untimed'],
                'late_blocks': m.get('late_blocks', []),
                'markers': {'names': m['marker_names'], 'count': int(self.arrays['mk_piece'].size)},
                'skipped': m.get('skipped', {}), 'notes': m.get('notes', []),
                'indexed_s': m.get('indexed_s')}

    # ---------------------------------------------------------------- window
    def pieces_in(self, ch, t0, t1, px):
        """The channel's pieces that lie in [t0, t1], in order of id.  With more than `px` of them,
        only the first (by start, then id) to start in each pixel column."""
        found = self._order.get(ch)
        if found is None:
            return []
        order, starts, ends, margin = found
        hi = bisect.bisect_left(starts, t1 + margin)
        lo = bisect.bisect_right(ends, t0 - margin, 0, hi)
        # ends: e + margin <= t0 for the first `lo`, computed as Rust compares them
        while lo > 0 and not ends[lo - 1] + margin <= t0:
            lo -= 1
        while lo < hi and ends[lo] + margin <= t0:
            lo += 1
        while hi > 0 and not starts[hi - 1] < t1 + margin:
            hi -= 1
        while hi < len(starts) and starts[hi] < t1 + margin:
            hi += 1
        out = []
        for i in order[lo:hi]:
            a, b = _span(self.pieces[i], t0, t1)
            if b > a:
                out.append(i)
        if len(out) > px:
            kept, last = [], None
            for i in out:
                col = _as_i64(_floor((max(self.pieces[i]['t0'], t0) - t0) / (t1 - t0) * px))
                if col != last:
                    kept.append(i)
                last = col
            out = kept
        return sorted(out)

    def window_json(self, channels, t0, t1, px):
        """JSON text: per channel, blocks of samples ('raw') or min/max ('env')
        covering [t0, t1] at about `px` points across (t0 < t1, both finite)."""
        px = max(16, min(int(px), 8000))
        span = t1 - t0
        parts = []
        for ch in channels:
            blocks = []
            for i in self.pieces_in(ch, t0, t1, float(px)):
                p = self.pieces[i]
                a, b = _span(p, t0, t1)
                per_px = p['fs'] * span / px            # samples per pixel column
                blocks.append(self._block(p, a, b, per_px))
            parts.append(f'"{ch}":[' + ','.join(blocks) + ']')
        return '{"t0":%r,"t1":%r,"channels":{%s}}' % (t0, t1, ','.join(parts))

    def _block(self, p, a, b, per_px):
        fs = p['fs']
        if per_px < LEVEL0 and b - a <= MAX_RAW:
            y = self.reader.read(p['id'], a, b)
            if per_px <= RAW_PER_PX:
                return '{"piece":%d,"kind":"raw","t0":%r,"dt":%r,"y":%s}' % (
                    p['id'], p['t0'] + a / fs, 1.0 / fs, _fmt(y, 8))
            step = max(1, int(per_px))
            m = y.size // step * step
            lo = np.fmin.reduce(y[:m].reshape(-1, step), axis=1) if m else np.empty(0)
            hi = np.fmax.reduce(y[:m].reshape(-1, step), axis=1) if m else np.empty(0)
            if m < y.size:
                lo = np.append(lo, np.fmin.reduce(y[m:]))
                hi = np.append(hi, np.fmax.reduce(y[m:]))
            return '{"piece":%d,"kind":"env","t0":%r,"dt":%r,"min":%s,"max":%s}' % (
                p['id'], p['t0'] + a / fs, step / fs, _fmt(lo, 7), _fmt(hi, 7))
        levels = p['env']
        lvl = 0
        while lvl + 1 < len(levels) and bucket_size(lvl + 1) <= per_px:
            lvl += 1
        size = bucket_size(lvl)
        off, length = levels[lvl]
        i0, i1 = a // size, min(length, -(-b // size))
        lo = self.arrays[f'env_min_{lvl}'][off + i0:off + i1]
        hi = self.arrays[f'env_max_{lvl}'][off + i0:off + i1]
        return '{"piece":%d,"kind":"env","t0":%r,"dt":%r,"min":%s,"max":%s}' % (
            p['id'], p['t0'] + i0 * size / fs, size / fs, _fmt(lo, 7), _fmt(hi, 7))

    def samples(self, piece, a, b):
        """Physical samples [a, b) of a piece (for tests and export)."""
        return self.reader.read(piece, a, b)

    # ---------------------------------------------------------------- vitals
    def _series(self, name):
        v = self.vitals[name]
        sl = slice(v['offset'], v['offset'] + v['n'])
        return self.arrays['vit_t'][sl], self.arrays['vit_v'][sl]

    def vitals_window(self, names, t0, t1, px):
        """Per series, its points in [t0, t1]; with more than 3 per pixel,
        the min and max of each pixel column (at the column's first time)."""
        out = {}
        for name in names:
            if name not in self.vitals:
                continue
            t, v = self._series(name)
            i0 = max(0, int(np.searchsorted(t, t0, 'left')) - 1)
            i1 = min(t.size, int(np.searchsorted(t, t1, 'right')) + 1)
            t, v = t[i0:i1], v[i0:i1]
            if t.size > 3 * px and t1 > t0:
                with np.errstate(all='ignore'):
                    col = np.clip(np.floor((t - t0) / (t1 - t0) * px), -_I64, _I64 - 1024)
                col = np.nan_to_num(col, nan=0.0).astype(np.int64)
                starts = np.flatnonzero(np.r_[True, col[1:] != col[:-1]])
                lo = np.minimum.reduceat(v, starts)
                hi = np.maximum.reduceat(v, starts)
                ts = t[starts]
                out[name] = {'unit': self.vitals[name]['unit'], 'kind': 'env', 't': ts.tolist(),
                             'min': lo.tolist(), 'max': hi.tolist()}
            else:
                out[name] = {'unit': self.vitals[name]['unit'], 'kind': 'points', 't': t.tolist(),
                             'v': v.tolist()}
        return out

    def vitals_at(self, t, max_age=VITAL_MAX_AGE_S):
        """Latest value of every series at time t (not older than max_age)."""
        out = []
        for v in self.meta['vitals']:
            ts, vs = self._series(v['name'])
            i = int(np.searchsorted(ts, t, 'right')) - 1
            if i >= 0 and t - ts[i] <= max_age:
                out.append({'name': v['name'], 'unit': v['unit'], 'value': float(vs[i]), 't': float(ts[i])})
        return out

    # --------------------------------------------------------------- markers
    def markers(self, channels, t0, t1, limit=20000):
        """Markers in [t0, t1] on the given channels: (channel, time, name index)."""
        mk_p = self.arrays['mk_piece']           # sorted by piece, then sample
        mk_s = self.arrays['mk_sample']
        mk_c = self.arrays['mk_code']
        items = []
        chs = set(channels)
        for p in self.pieces:
            if p['ch'] not in chs:
                continue
            fs = p['fs']
            # saturating, as the other viewers convert: any finite times
            a = _as_i64(_floor((t0 - p['t0']) * fs))
            b = _as_i64(_ceil((t1 - p['t0']) * fs))
            if b < 0 or a > p['n']:
                continue
            lo = np.searchsorted(mk_p, p['id'], 'left')
            hi = np.searchsorted(mk_p, p['id'], 'right')
            s = mk_s[lo:hi]
            j0, j1 = lo + np.searchsorted(s, a, 'left'), lo + np.searchsorted(s, b, 'right')
            for k in range(j0, j1):
                items.append([p['ch'], p['t0'] + mk_s[k] / fs, int(mk_c[k])])
                if len(items) >= limit:
                    return {'names': self.meta['marker_names'], 'items': items, 'truncated': True}
        return {'names': self.meta['marker_names'], 'items': items, 'truncated': False}
