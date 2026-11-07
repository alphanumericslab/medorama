"""Building blocks shared by all sources: pieces, min/max envelopes, events.

Vocabulary
  channel  one signal as the user sees it (e.g. ECG lead II, ART), with a
           label, unit and modality.
  piece    a stretch of a channel the source proves contiguous: n samples at
           a constant fs starting at t0 (Unix seconds, UTC).  Sample i of a
           piece is at t0 + i / fs.  Nothing is ever resampled or stitched;
           pieces are drawn separately.
  timed    False when the file states no time for the piece's stretch.  The
           piece is then placed next to its neighbours for display and the
           viewer marks it "time not stated".
  envelope per piece, the min and max of every block of LEVEL0 samples, and
           of every FANOUT blocks above that (a min/max pyramid), used to
           draw long windows without decoding samples.
"""

import json
import math
import re
import struct

import numpy as np

LEVEL0 = 32           # samples per bucket at level 0
FANOUT = 4            # buckets merged per level
INDEX_VERSION = 2     # the index format version (docs/index-format.md)
CHUNK = 1 << 16       # samples converted at a time while indexing
# the most pieces a recording may have: a reader refuses a recording with more
# (viewers/vitalstream/docs/index-format.md, "Limits")
MAX_PIECES = 100_000
# the most distinct marker names (mk_code is 16-bit): names after the first MAX_MARKER_NAMES - 1
# share the last code, named OTHER_MARKERS
MAX_MARKER_NAMES = 32_768
OTHER_MARKERS = '(other markers)'
# arrays and objects nested deeper than this are refused in JSON read from files and requests
MAX_JSON_DEPTH = 128


def bucket_size(level):
    return LEVEL0 * FANOUT ** level


class Envelope:
    """Streaming min/max pyramid of one piece (NaN-aware: NaN only where a
    whole bucket is NaN)."""

    def __init__(self):
        self.mins, self.maxs = [], []
        self.rest = np.empty(0, np.float32)

    def add(self, x):
        x = np.asarray(x, np.float32)
        if self.rest.size:
            x = np.concatenate([self.rest, x])
        m = x.size // LEVEL0 * LEVEL0
        if m:
            blocks = x[:m].reshape(-1, LEVEL0)
            self.mins.append(np.fmin.reduce(blocks, axis=1))
            self.maxs.append(np.fmax.reduce(blocks, axis=1))
        self.rest = x[m:].copy()

    def finish(self):
        """List of (min, max) arrays, level 0 first, down to a single bucket."""
        if self.rest.size:
            self.mins.append(np.fmin.reduce(self.rest, keepdims=True))
            self.maxs.append(np.fmax.reduce(self.rest, keepdims=True))
            self.rest = np.empty(0, np.float32)
        lo = np.concatenate(self.mins) if self.mins else np.empty(0, np.float32)
        hi = np.concatenate(self.maxs) if self.maxs else np.empty(0, np.float32)
        self.mins = self.maxs = None
        levels = [(lo, hi)]
        while lo.size > 1:
            pad = (-lo.size) % FANOUT
            if pad:
                lo = np.concatenate([lo, np.full(pad, np.nan, np.float32)])
                hi = np.concatenate([hi, np.full(pad, np.nan, np.float32)])
            lo = np.fmin.reduce(lo.reshape(-1, FANOUT), axis=1)
            hi = np.fmax.reduce(hi.reshape(-1, FANOUT), axis=1)
            levels.append((lo, hi))
        return levels


class Channels:
    """Channel registry: one id per (key), first-seen order."""

    def __init__(self):
        self.items = []
        self._ids = {}

    def get(self, key, label, unit, modality, **extra):
        cid = self._ids.get(key)
        if cid is None:
            cid = self._ids[key] = len(self.items)
            self.items.append({'id': cid, 'label': str(label), 'unit': unit or '',
                               'modality': modality, **extra})
        return cid


class Pieces:
    """Collects pieces with their envelopes and markers."""

    def __init__(self):
        self.items = []
        self.envelopes = []
        self.markers = []          # (piece, sample, name index)
        self.marker_names = []
        self._names = {}

    def room(self, more):
        """A ValueError unless `more` pieces still fit within MAX_PIECES (a reader checks it
        before it adds the pieces of a run or a record part)."""
        if len(self.items) + more > MAX_PIECES:
            raise ValueError(f'the recording has more than {MAX_PIECES} pieces (stretches of one signal): '
                             'it is not indexed')

    def new(self, channel, fs, **info):
        p = {'id': len(self.items), 'ch': channel, 'fs': float(fs), 'n': 0, **info}
        self.items.append(p)
        self.envelopes.append(Envelope())
        return p

    def add(self, p, values):
        self.envelopes[p['id']].add(values)
        p['n'] += len(values)

    def marker(self, p, sample, name):
        i = self._names.get(name)
        if i is None:
            if len(self.marker_names) == MAX_MARKER_NAMES - 1:      # every code taken: the rest share the last
                self.marker_names.append(OTHER_MARKERS)
                i = self._names[OTHER_MARKERS] = MAX_MARKER_NAMES - 1
            elif len(self.marker_names) == MAX_MARKER_NAMES:
                i = MAX_MARKER_NAMES - 1
            else:
                i = self._names[name] = len(self.marker_names)
                self.marker_names.append(name)
        self.markers.append((p['id'], sample, i))

    def markers_capped(self):
        """Whether more marker names were found than codes exist (the rest share one name)."""
        return len(self.marker_names) == MAX_MARKER_NAMES

    def finish(self):
        """meta list and arrays dict (concatenated per level) for the index."""
        arrays = {}
        per_level = {}
        for p, env in zip(self.items, self.envelopes):
            levels = env.finish()
            p['env'] = []
            for lvl, (lo, hi) in enumerate(levels):
                bucket = per_level.setdefault(lvl, ([], [], [0]))
                p['env'].append([bucket[2][0], int(lo.size)])
                bucket[0].append(lo)
                bucket[1].append(hi)
                bucket[2][0] += int(lo.size)
        for lvl, (lo, hi, _) in per_level.items():
            arrays[f'env_min_{lvl}'] = np.concatenate(lo).astype(np.float32)
            arrays[f'env_max_{lvl}'] = np.concatenate(hi).astype(np.float32)
        mk = np.array(self.markers, dtype=np.int64).reshape(-1, 3)
        mk = mk[np.lexsort((mk[:, 1], mk[:, 0]))]          # by piece, then sample
        arrays['mk_piece'] = mk[:, 0].astype(np.int32)
        arrays['mk_sample'] = mk[:, 1].astype(np.int64)
        arrays['mk_code'] = mk[:, 2].astype(np.int16)
        self.envelopes = None
        return self.items, arrays


class Vitals:
    """Vital-sign series: name -> times and values (finite values only)."""

    def __init__(self):
        self.series = {}

    def add(self, name, unit, t, value):
        try:
            t, value = float(t), float(value)
        except (TypeError, ValueError):
            return
        if not (math.isfinite(t) and math.isfinite(value)):
            return
        s = self.series.get(name)
        if s is None:
            s = self.series[name] = {'unit': unit or '', 't': [], 'v': []}
        s['t'].append(t)
        s['v'].append(value)

    def finish(self):
        meta, ts, vs, off = [], [], [], 0
        for name in sorted(self.series, key=lambda k: (-len(self.series[k]['t']), k)):
            s = self.series[name]
            t = np.asarray(s['t'], np.float64)
            v = np.asarray(s['v'], np.float64)
            order = np.argsort(t, kind='stable')
            ts.append(t[order])
            vs.append(v[order])
            meta.append({'name': name, 'unit': s['unit'], 'offset': off, 'n': int(t.size)})
            off += int(t.size)
        arrays = {'vit_t': np.concatenate(ts) if ts else np.empty(0),
                  'vit_v': np.concatenate(vs) if vs else np.empty(0)}
        return meta, arrays


def plausible_time(t, lo, hi, margin=86400.0):
    """t if it lies within [lo - margin, hi + margin], else None (e.g. .NET zero dates)."""
    try:
        t = float(t)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(t) or lo is None or hi is None:
        return None
    return t if lo - margin <= t <= hi + margin else None


def total_order(x):
    """A sort key of a float in IEEE total order (Rust's total_cmp: NaN sorts after infinity)."""
    bits = struct.unpack('<q', struct.pack('<d', float(x)))[0]
    return bits ^ 0x7FFF_FFFF_FFFF_FFFF if bits < 0 else bits


def fmax(a, b):
    """The larger of two floats, ignoring a NaN (Rust's f64::max)."""
    return b if a != a else a if b != b else max(a, b)


def divide(a, b):
    """a / b as IEEE floats divide (no ZeroDivisionError)."""
    a, b = float(a), float(b)
    if b == 0.0:
        return math.nan if a == 0.0 or a != a else math.copysign(math.inf, a) * math.copysign(1.0, b)
    return a / b


def untimed_spans(pieces):
    """Merged [t0, t1] spans covered by pieces whose time is not stated."""
    spans = []
    for p in pieces:
        if p.get('timed', True) is False:
            t0 = float(p.get('t0', 0.0))
            spans.append((t0, t0 + divide(p.get('n', 0), p.get('fs', math.nan))))
    spans.sort(key=lambda s: (total_order(s[0]), total_order(s[1])))
    out = []
    for a, b in spans:
        if out and a <= out[-1][1] + 1e-9:
            out[-1][1] = fmax(out[-1][1], b)
        else:
            out.append([a, b])
    return [{'t0': a, 't1': b} for a, b in out]


_STRING = re.compile(r'"(?:[^"\\]|\\.)*"', re.S)


def json_loads(text):
    """json.loads of JSON read from a file or a request, refusing arrays and objects nested more
    than MAX_JSON_DEPTH deep (ValueError), as the other viewers do."""
    if isinstance(text, (bytes, bytearray)):
        text = text.decode('utf-8')
    bare = np.frombuffer(_STRING.sub('', text).encode('utf-8', 'replace'), np.uint8)
    step = (np.isin(bare, (ord('['), ord('{'))).astype(np.int64)
            - np.isin(bare, (ord(']'), ord('}'))).astype(np.int64))
    if step.size and int(np.cumsum(step).max()) > MAX_JSON_DEPTH:
        raise ValueError('arrays and objects nested too deeply')
    return json.loads(text)


def clean(obj):
    """JSON-safe copy: NaN/inf -> None, numpy scalars -> Python."""
    if isinstance(obj, dict):
        return {str(k): clean(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [clean(v) for v in obj]
    if isinstance(obj, (np.integer,)):
        return int(obj)
    if isinstance(obj, (np.floating, float)):
        f = float(obj)
        return f if math.isfinite(f) else None
    if isinstance(obj, np.bool_):
        return bool(obj)
    return obj
