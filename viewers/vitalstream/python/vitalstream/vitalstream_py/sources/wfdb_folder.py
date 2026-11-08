"""WFDB records (docs/wfdb-collection.md): one record (its .hea file, single- or multi-segment),
a WFDB collection (records described by index.csv), or a folder of plain WFDB records,
subfolders included, read as one session.  Mirrors the Rust engine's sources/wfdb_folder.rs.

Each signal of each record (each segment of a multi-segment record) is a piece.  Start times
come from index.csv (UTC) or from the headers (base date and time, taken as UTC); records
without one are placed after the data before them and marked "time not stated" (records of the
same stretch share that start).  The segments of a multi-segment record follow each other from
the record's start; its null segments are breaks of known length.  A piece that would overlap
another piece of its channel goes into a second lane of that channel ('II (2)'), so overlapping
records are never drawn on top of each other.  Vital signs, alarms, breaks and markers come from
vitals.csv, events.csv, gaps.csv and the .mrk annotation files.

Records are read with wfdb-x (wfdbx), whose Python library keeps module-wide state: every call
holds one lock.  Nothing is assumed that a header does not state: a record without a sampling
frequency is left out, an uncalibrated signal (gain 0) is shown in ADC units ('adu'), and signals
without units have none.

This module needs wfdb-x (installed with the extra: pip install 'vitalstream_py[wfdb]'); without
it, wfdb_none takes its place (sources/__init__.py).  What needs no wfdb-x (which paths are
records, segment tables, limits) is in wfdb_files, shared by both.
"""

import csv
import datetime
import math
import os
import threading
import time
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path

import numpy as np
import wfdbx

from .. import paths
# re-exported: the same names as wfdb_none
from .wfdb_files import MAX_FINGERPRINT_FILES, Limits, format_label, is_record, is_wfdb_folder  # noqa: F401
from .wfdb_files import limits, segment_table, set_limits, signal_file_note  # noqa: F401
from . import wfdb_files
from ..common import MAX_MARKER_NAMES, OTHER_MARKERS, Channels, Pieces, Vitals, plausible_time, untimed_spans

CHUNK_SAMPLES = 1 << 20          # samples (frames x signals) decoded at once; at least MIN_CHUNK_FRAMES frames
MIN_CHUNK_FRAMES = 4096
# gaps.csv rows that only mark where a record was split (channel set, label or packet count
# changed); the data are contiguous there.
SPLIT_REASONS = {'channel_absent', 'channel_set_change', 'label_change', 'packet_count_change'}
SUPPORTED = True           # this installation reads WFDB records (wfdb-x is installed)
_WFDB = threading.Lock()   # wfdb-x's Python library keeps module-wide state
wfdbx.wfdbquiet()           # problems come back as exceptions, not printed messages


def _header(record):
    """The header of a record (no signal file is opened); ValueError if it cannot be read or
    states no sampling frequency (or one that is not a finite positive number).  The number of
    samples is at most what the signal files can hold (_frames_held)."""
    try:
        with _WFDB:
            h = wfdbx.read_header(str(record))
    except Exception as e:
        raise ValueError(f'{record}: {e}') from e
    if not (h.fs > 0 and math.isfinite(h.fs)):
        raise ValueError(f'{record}: the header states no sampling frequency')
    h.nsamples = min(max(h.nsamples, 0), _frames_held(record, h))
    return h


# three times the bits a sample of each WFDB storage format of a fixed size takes
_BITS3 = {8: 24, 80: 24, 16: 48, 61: 48, 160: 48, 212: 36, 310: 32, 311: 32, 24: 72, 32: 96}


def _signal_file(record, fname):
    """The file a signal file name of a header designates, as wfdb-x opens it: '~' expanded, an
    absolute name as given, any other next to the header; None for a signal without a file."""
    f = fname or ''
    if f in ('', '-', '~'):
        return None
    f = paths.expand(f)
    if os.path.isabs(f):
        return Path(f)
    return Path(record).parent / f


def _frames_held(record, h):
    """The most frames the signal files of a record can hold: for each file of signals of a
    fixed size, ceil(8 x bytes / bits per frame), the least of them (unbounded when no file can
    be measured).  A header that states more samples is read up to this many."""
    per_file = {}
    for s in h.siginfo:
        f = _signal_file(record, s.fname)
        if f is None:
            continue
        b3 = _BITS3.get(s.fmt)
        bits = None if b3 is None else b3 * max(s.spf, 1)
        prev = per_file.get(f, 0)
        per_file[f] = None if prev is None or bits is None else prev + bits
    held = 2 ** 63 - 1
    for f, bits in per_file.items():
        if not bits:
            continue
        try:
            size = os.stat(f).st_size
        except OSError:
            continue
        held = min(held, -(-size * 24 // bits))
    return held


def _hea(rec):
    return Path(f'{rec}.hea')


def files_read(base, name):
    """Every file reading record `name` (relative to `base`) opens: its header, the signal files
    its header names, its .mrk file, and those of each segment of a multi-segment record."""
    out = {}

    def add(rec):
        out.setdefault(_hea(rec), None)
        try:
            with _WFDB:
                h = wfdbx.read_header(str(rec))
        except Exception:
            h = None
        for s in h.siginfo if h is not None else []:
            f = _signal_file(rec, s.fname)
            if f is not None:
                out.setdefault(f, None)
        mrk = Path(f'{rec}.mrk')
        if os.path.lexists(mrk):
            out.setdefault(mrk, None)

    base = Path(base)
    add(base / name)
    for seg, _ in segment_table(base / name) or []:
        if seg != '~':
            add(base / _beside(name, seg))
    return list(out)


def check_files(base, name):
    """A ValueError naming the first file of record `name` that lies outside the folders the
    readers may open (paths.set_allowed_roots)."""
    for f in files_read(base, name):
        if not paths.allowed_file(f):
            raise ValueError(f'{f} lies outside the data roots: the record {name} is not read')


def _safe_name(name):
    """A record name from index.csv that stays inside its folder: relative, without '..'."""
    if not name or name[0] in '/\\~':
        return False
    p = Path(name)
    return not (p.drive or p.root) and '..' not in p.parts


def _beside(record, name):
    """`name` of a segment, next to its record ('dir/rec' -> 'dir/seg'), with '/' between folders."""
    i = record.rfind('/')
    return f'{record[:i]}/{name}' if i >= 0 else name


def _start_unix(h):
    """Unix time (UTC) of a header's base date and time, when it states both."""
    if h.base_date is None or h.base_time_ms is None:
        return None
    y, m, d = h.base_date
    return (datetime.date(y, m, d) - datetime.date(1970, 1, 1)).days * 86400.0 + h.base_time_ms / 1000.0


def _names_units(h):
    """(name, unit) of each signal: 'adu' for an uncalibrated signal; '' when no units are stated."""
    return [(s.desc or '', 'adu' if s.gain == 0 else (s.units or '')) for s in h.siginfo]


def _physical(x, siginfo):
    """Stored samples (frames x signals) in physical units, as float64: (value - baseline) / gain,
    the stored value for an uncalibrated signal, NaN for WFDB's invalid sample."""
    x = np.asarray(x, dtype=np.int64)
    out = np.empty(x.shape, dtype=np.float64)
    for k, s in enumerate(siginfo):
        col = x[:, k].astype(np.float64)
        if s.gain != 0:
            col = (col - float(s.baseline)) / s.gain
        col[x[:, k] == wfdbx.WFDB_INVALID_SAMPLE] = np.nan
        out[:, k] = col
    return out


def _chunk_frames(nsig):
    """Frames decoded at once for a record of `nsig` signals."""
    return max(CHUNK_SAMPLES // max(nsig, 1), MIN_CHUNK_FRAMES)


def _read_some(record, a, b, channels):
    """(signals, siginfo) of frames [a, b) of a record, or of the frames from a that it holds
    when its data end before b (wfdb-x refuses to read past the end of the data)."""
    def read(to):
        with _WFDB:
            r = wfdbx.read_record(str(record), channels=channels, sampfrom=a, sampto=to, checksum_warning=False)
        return r.signals, r.siginfo
    try:
        return read(b)
    except OSError as e:
        if 'getvec status' not in str(e):        # not the end of the data: the file cannot be read
            raise ValueError(f'{record}: {e}') from e
    # the end of the data: the last frame that can be read
    lo, hi = a, b - 1                    # frames [a, lo) are readable; [a, hi + 1) is not known to be
    got = None
    while lo < hi:
        mid = (lo + hi + 1) // 2
        try:
            got = read(mid)
            lo = mid
        except OSError as e:
            if 'getvec status' not in str(e):
                raise ValueError(f'{record}: {e}') from e
            hi = mid - 1
    if lo <= a:
        return None
    return got if got is not None and len(got[0]) == lo - a else read(lo)


def _decode(record, nsig, a, b, channels, sink):
    """Frames [a, b) of a record of `nsig` signals, a chunk at a time: sink(x) gets the physical
    samples of each chunk (frames x signals `channels`, all when None).  Stops at the end of the
    data (a chunk read short), whatever the header states."""
    chunk = _chunk_frames(nsig)
    while a < b:
        n = min(chunk, b - a)
        got = _read_some(record, a, a + n, channels)
        if got is None:
            return
        x = _physical(*got)
        sink(x)
        if x.shape[0] < n:
            return
        a += n


def _read(record, nsig, a, b, channels=None):
    """Frames [a, b) of a record of `nsig` signals (signals `channels`, all when None) in
    physical units, up to the end of its data."""
    parts = []
    _decode(record, nsig, a, b, channels, parts.append)
    if not parts:
        return np.empty((0, len(channels) if channels is not None else 0), np.float64)
    return np.concatenate(parts) if len(parts) > 1 else parts[0]


def _float(v):
    try:
        f = float(v)
    except (TypeError, ValueError):
        return None
    return f if math.isfinite(f) else None


def _int(v):
    f = _float(v)
    return int(f) if f is not None else None


def _rows(path):
    try:
        with open(path, newline='', encoding='utf-8') as f:
            return [{k: v if v is not None else '' for k, v in r.items() if k is not None} for r in csv.DictReader(f)]
    except (OSError, UnicodeDecodeError):
        return []


def _num(x):
    """A number as the Rust engine writes it in a message (10.0 -> '10', 0.5 -> '0.5', no exponent)."""
    if not math.isfinite(x):
        return repr(x)
    if x == int(x):
        return str(int(x))
    return format(Decimal(repr(x)), 'f')


def _over_limit(what):
    """The refusal for a folder over a limit, with what to do instead."""
    return ValueError(f'{what}: open a subfolder, or describe the records in an index.csv (a WFDB collection, '
                      'docs/wfdb-collection.md)')


def _walk(folder, lim, deadline):
    """Header files below `folder`, as record names (relative, without .hea, '/' between
    folders), sorted.  Hidden entries ('.' names) and symbolic links are skipped.  Over a limit
    (records, depth, or `deadline` (time.monotonic) for the scan) it is a ValueError."""
    out = []

    def go(d, rel, depth):
        # reached, not passed: a clock that ticks every 15 ms (Windows) still ends a 0 s limit
        if deadline is not None and time.monotonic() >= deadline:
            raise _over_limit(f'counting the records took more than {_num(lim.max_scan_s)} s')
        try:
            with os.scandir(d) as it:
                entries = sorted((e.name, e.is_dir(follow_symlinks=False)) for e in it
                                 if not e.name.startswith('.') and not e.is_symlink())
        except OSError:
            return
        for name, is_dir in entries:
            r = f'{rel}/{name}' if rel else name
            if is_dir:
                if depth + 1 > lim.max_depth:
                    raise _over_limit(f'the folder has records nested more than {lim.max_depth} levels deep')
                go(os.path.join(d, name), r, depth + 1)
            elif r.endswith('.hea'):
                out.append(r[:-4])
                if len(out) > lim.max_records:
                    raise _over_limit(f'the folder holds more than {lim.max_records} WFDB records')

    go(str(folder), '', 0)
    return sorted(out)


def _plain_records(folder, names):
    """The records of a folder that are not segments (or layouts) of a multi-segment record in it."""
    claimed = set()
    for n in names:
        for seg, _ in segment_table(Path(folder) / n) or []:
            if seg != '~':
                claimed.add(_beside(n, seg))
    return [n for n in names if n not in claimed]


@dataclass
class _Part:
    """The single-segment record holding samples of a record: itself, or one segment of a
    multi-segment record, `offset` samples after the record's start."""
    rec: str
    offset: int
    h: object


@dataclass
class _Unit:
    """A record to read: index.csv's row (or one made from the header), and its parts."""
    name: str               # the record's name, relative to the folder opened
    row: dict
    fs: float
    n: int
    parts: list
    nulls: list = field(default_factory=list)       # null segments: (offset, samples)
    master: object = None                           # the master header of a multi-segment record


def _unit(base, name, row=None):
    """The record `name` (relative to `base`), with `row` from index.csv or None."""
    check_files(base, name)
    path = Path(base) / name
    h = _header(path)
    # a MAT signal file is read as frames from the header's byte offset: one of another layout
    # would be read as noise
    wfdb_files.check_mat_files(_hea(path))
    if row is None:
        t = _start_unix(h)
        row = {'record': name, 'modality': 'wfdb', 'start_unix': repr(t) if t is not None else ''}
    segs = segment_table(path)
    if segs is None:
        return _Unit(name, row, h.fs, max(h.nsamples, 0), [_Part(name, 0, h)])
    parts, nulls, offset = [], [], 0
    for seg, n in segs:
        if seg == '~':
            if n > 0:
                nulls.append((offset, n))
        elif n > 0:
            rec = _beside(name, seg)
            parts.append(_Part(rec, offset, _header(Path(base) / rec)))
        offset += max(n, 0)          # a layout segment has no samples
    return _Unit(name, row, h.fs, offset, parts, nulls, h)


@dataclass
class _Opened:
    """What is opened: the records and how they were found."""
    base: Path              # the folder record names are relative to
    units: list
    collection: bool        # index.csv describes the records
    single: bool            # one record (a .hea file)
    skipped: list           # records left out, with the reason


def _open(path, lim, deadline):
    p = Path(path)
    if is_record(p):
        return _Opened(p.parent, [_unit(p.parent, p.stem)], False, True, [])
    rows = _rows_within(p / 'index.csv')
    units, skipped = [], []
    collection = bool(rows)
    if collection:
        # a record that cannot be read is left out, as in a folder of plain records
        for i, r in enumerate(rows):
            if 'record' not in r:
                raise ValueError(f'{path}: index.csv has no record column')
            name = r['record']
            if not _safe_name(name):
                skipped.append((name or f'row {i + 1}',
                                'index.csv names a record outside its folder (an absolute name, or one with ..)'))
                continue
            try:
                units.append(_unit(p, name, r))
            except (OSError, ValueError) as e:
                skipped.append((name, str(e)))
    else:
        for name in _plain_records(p, _walk(p, lim, deadline)):
            try:
                units.append(_unit(p, name))
            except (OSError, ValueError) as e:
                skipped.append((name, str(e)))
    if not units:
        raise ValueError(f'no WFDB records in or below {path}')
    return _Opened(p, units, collection, False, skipped)


def _rows_within(path):
    """_rows of a file the readers may open (none otherwise: paths.allowed_file)."""
    return _rows(path) if paths.allowed_file(path) else []


def _rec_path(units, name):
    """gaps.csv may name records without their folder; index.csv has modality/record."""
    for u in units:
        rec = u.name
        if rec == name or rec.endswith('/' + name):
            return rec
    return name


def _placement(units, gap_rows):
    """Start of each record on the time axis, and whether it is stated: records in order; those
    without a start follow the data before them, after the break gaps.csv states before them
    (gap_s), if any; records of the same stretch start together."""
    counted = {}                  # record -> the break stated before it
    for g in gap_rows:
        gs = _float(g.get('gap_s'))
        if g.get('scope') == 'all' and gs is not None:
            for name in (g.get('records_after') or '').split(';'):
                counted[_rec_path(units, name)] = gs
    out = []
    last_end, untimed_group = None, {}
    for i, u in enumerate(units):
        r = u.row
        t = _float(r.get('start_unix'))
        if t is not None:
            s = (t, True)
        else:
            g = r.get('stretch') or f'record {i}'
            if g not in untimed_group:
                untimed_group[g] = (last_end if last_end is not None else 0.0) + counted.get(u.name, 0.0)
            s = (untimed_group[g], False)
        out.append(s)
        end = s[0] + u.n / u.fs
        last_end = end if last_end is None else max(last_end, end)
    return out


class _Lanes:
    """Lanes of each channel: the time spans taken in each, to put a piece in the first lane it
    does not overlap."""

    def __init__(self):
        self.taken = {}

    def place(self, key, t0, t1):
        """The lane (1, 2, ...) of a piece of channel `key` over [t0, t1)."""
        lanes = self.taken.setdefault(key, [])
        for k, spans in enumerate(lanes):
            if all(t1 <= a + 1e-9 or t0 >= b - 1e-9 for a, b in spans):
                spans.append((t0, t1))
                return k + 1
        lanes.append([(t0, t1)])
        return len(lanes)


def build(path, progress, cancelled=lambda: False):
    """(meta, arrays) of the records at `path` (a folder, or a record's .hea file); stops with
    ValueError('indexing cancelled') once `cancelled` returns true."""
    opened = _open(path, limits(), None)
    base, units = opened.base, opened.units
    channels, pieces, vitals = Channels(), Pieces(), Vitals()
    total = sum(max(p.h.nsamples, 0) * p.h.nsig for u in units for p in u.parts) or 1

    def csv_rows(name):
        """The CSV files beside the records (a collection's; a folder of plain records may have
        them too)."""
        return [] if opened.single else _rows_within(base / name)

    gap_rows = csv_rows('gaps.csv')
    starts = _placement(units, gap_rows)

    done = 0
    lanes = _Lanes()
    rec_pieces = {}
    piece_info = []               # per piece: record, signal
    null_gaps = []
    for u, (t0, timed) in zip(units, starts):
        modality = u.row.get('modality') or 'wfdb'
        everything = []
        part_pieces = []          # per part: its pieces, and the index of each of its signal names
        for part in u.parts:
            tp = t0 + part.offset / u.fs
            tend = tp + max(part.h.nsamples, 0) / part.h.fs
            ps, by_name = [], {}
            pieces.room(part.h.nsig)
            for j, (name, unit) in enumerate(_names_units(part.h)):
                key = ('wfdb', modality, name, unit)
                lane = lanes.place(key, tp, tend)
                key, label = (key, name) if lane == 1 else (key + (lane,), f'{name} ({lane})')
                cid = channels.get(key, label, unit, modality)
                ps.append(pieces.new(cid, part.h.fs, t0=tp, timed=timed))
                piece_info.append((part.rec, j))
                by_name.setdefault(name, j)
            everything += ps
            rec_path = base / part.rec
            n = max(part.h.nsamples, 0)

            def sink(x, ps=ps, nsig=part.h.nsig):
                nonlocal done
                if cancelled():
                    raise ValueError('indexing cancelled')
                for j, p in enumerate(ps):
                    pieces.add(p, x[:, j].astype(np.float32))
                done += x.shape[0] * nsig
                progress(done / total)
            if cancelled():
                raise ValueError('indexing cancelled')
            _decode(rec_path, part.h.nsig, 0, n, None, sink)
            part_pieces.append((ps, by_name))
        rec_pieces[u.name] = everything
        # markers: the record's .mrk annotations (of a multi-segment record: the record's, with
        # sample numbers from its start); a marker's name is its text, else the annotation
        # code's mnemonic.  A file that cannot be read (a truncated one) gives none.
        rec = base / u.name
        if Path(f'{rec}.mrk').is_file():
            with _WFDB:
                try:
                    ann = wfdbx.read_annotations(str(rec), 'mrk')
                    names = [(note or '').strip() or (wfdbx.ecgstr(int(code)) or f'[{code}]').strip()
                             for code, note in zip(ann.codes.tolist(), ann.aux)]
                except Exception:
                    ann, names = None, []
            if ann is not None:
                master = [x[0] for x in _names_units(u.master)] if u.master is not None else None
                for s, c, name in zip(ann.samples.tolist(), ann.chans.tolist(), names):
                    s, c = int(s), int(c)
                    if master is None:
                        ps = part_pieces[0][0]
                        if 0 <= c < len(ps):
                            pieces.marker(ps[c], s, name)
                        continue
                    # the segment the marker falls in, and the signal of that name there
                    if not 0 <= c < len(master):
                        continue
                    k = next((k for k, p in enumerate(u.parts) if p.offset <= s < p.offset + max(p.h.nsamples, 0)),
                             None)
                    if k is None:
                        continue
                    ps, by_name = part_pieces[k]
                    j = by_name.get(master[c])
                    if j is not None:
                        pieces.marker(ps[j], s - u.parts[k].offset, name)
        for off, n in u.nulls:
            a, b = t0 + off / u.fs, t0 + (off + n) / u.fs
            null_gaps.append({'t0': a, 't1': b, 'reason': 'null segment', 'gap_s': b - a, 'exact': True,
                              'measured_by': None, 'notes': ''})
    markers_capped = pieces.markers_capped()
    items, arrays = pieces.finish()
    for p, (record, sig) in zip(items, piece_info):
        p['record'], p['sig'] = record, sig
    t_lo = min((p['t0'] for p in items), default=None)
    t_hi = max((p['t0'] + p['n'] / p['fs'] for p in items), default=None)

    # vital signs (vitals.csv: time, name or label, value, unit; rows with a non-zero validity
    # are left out). A label measured by several sources keeps its name for the first source in
    # the file; the others are named with their source ("HR [import]").
    vital_rows = [v for v in csv_rows('vitals.csv') if v.get('validity') in (None, '', '0')]
    sources_of = {}
    for v in vital_rows:
        s = sources_of.setdefault(v.get('label') or v.get('name') or '', [])
        if (v.get('source') or '') not in s:
            s.append(v.get('source') or '')
    for v in vital_rows:
        label = v.get('label') or v.get('name') or ''
        src = v.get('source') or ''
        name = f'{label} [{src}]' if src and sources_of[label][0] != src else label
        vitals.add(name, v.get('unit'), v.get('time'), v.get('value', v.get('value1')))

    # alarms and other events with a duration (events.csv: start_unix, end_unix, level, text, kind)
    alarms = []
    for e in csv_rows('events.csv'):
        t0 = plausible_time(e.get('start_unix'), t_lo, t_hi)
        if t0 is None:
            continue
        t1 = plausible_time(e.get('end_unix'), t_lo, t_hi)
        alarms.append({'t0': t0, 't1': t1 if t1 is not None and t1 >= t0 else t0,
                       'text': ' '.join((e.get('text') or '').split()), 'level': _int(e.get('level')),
                       'kind': e.get('kind') or 'event'})
    alarms.sort(key=lambda a: a['t0'])

    gaps = []
    for g in gap_rows:
        reason = g.get('reason') or ''
        if reason in SPLIT_REASONS:
            continue                  # a new record, not a break in the data
        t0, t1 = _float(g.get('end_before_unix')), _float(g.get('start_after_unix'))
        before = [p for r in (g.get('records_before') or '').split(';') if r
                  for p in rec_pieces.get(_rec_path(units, r), [])]
        after = [p for r in (g.get('records_after') or '').split(';') if r
                 for p in rec_pieces.get(_rec_path(units, r), [])]
        if t0 is None and before:
            t0 = max(p['t0'] + p['n'] / p['fs'] for p in before)
        if t1 is None and after:
            t1 = min(p['t0'] for p in after)
        if t0 is None and t1 is None:
            continue
        scope = g.get('scope') or 'all'
        row = {'t0': t0 if t0 is not None else t1, 't1': t1 if t1 is not None else t0,
               'reason': reason if scope == 'all' else f'{scope}: {reason}',
               'gap_s': _float(g.get('gap_s')), 'exact': g.get('exact') == 'True',
               'measured_by': g.get('measured_by'), 'notes': g.get('notes') or ''}
        if scope != 'all':
            row['channels'] = sorted({p['ch'] for p in before + after})
        gaps.append(row)
    gaps += null_gaps

    vit_meta, vit_arrays = vitals.finish()
    arrays.update(vit_arrays)
    notes = []
    if not opened.collection:
        notes.append("The headers' start times are taken as UTC.")
    if any(not p['timed'] for p in items):
        notes.append('Some records have no start time; they are placed after the data before them '
                     'and marked "time not stated".')
    if any(len(spans) > 1 for spans in lanes.taken.values()):
        notes.append('Some records overlap in time: a signal recorded twice at once is shown in a second lane '
                     '("II (2)").')
    if markers_capped:
        notes.append(f'More than {MAX_MARKER_NAMES - 1} distinct marker names: the others are shown as '
                     f'"{OTHER_MARKERS}".')
    if opened.skipped:
        k = len(opened.skipped)
        notes.append('1 record could not be read and was left out.' if k == 1
                     else f'{k} records could not be read and were left out.')
    meta = {'channels': channels.items, 'pieces': items, 'vitals': vit_meta, 'alarms': alarms, 'gaps': gaps,
            'untimed': untimed_spans(items), 'marker_names': pieces.marker_names,
            'skipped': dict(opened.skipped), 'notes': notes, 'time_resolution_s': None, 'reader': {}}
    return meta, arrays


def scan(path, lim):
    """What opening `path` would read, from the headers alone (no signal file is opened), within
    the limits `lim` (the scan's time limit too): a ValueError over a limit
    (docs/wfdb-collection.md, "Limits")."""
    t_start = time.monotonic()
    # no deadline for an infinite limit (no time limit)
    deadline = t_start + max(lim.max_scan_s, 0.0) if math.isfinite(lim.max_scan_s) else None
    opened = _open(path, lim, deadline)
    starts = _placement(opened.units, [])
    signals, segments, samples_s = {}, 0, 0.0        # signals: names, in order
    lo = hi = None
    untimed = 0
    files = {}                                       # each once, in order
    for u, (t0, timed) in zip(opened.units, starts):
        if u.master is not None:
            segments += len(u.parts)
        dur = u.n / u.fs
        samples_s += dur
        if timed:
            lo = t0 if lo is None else min(lo, t0)
            hi = t0 + dur if hi is None else max(hi, t0 + dur)
        else:
            untimed += 1
        for p in u.parts:
            for name, _ in _names_units(p.h):
                signals.setdefault(name, None)
        for f in record_files(opened.base, u.name):
            files.setdefault(f, None)
    size = 0
    for f in files:
        try:
            size += os.stat(opened.base / f).st_size
        except OSError:
            pass
    return {'path': str(path), 'format': format_label(path), 'records': len(opened.units), 'segments': segments,
            'signals': list(signals), 'duration_s': samples_s, 't_range': [lo, hi] if lo is not None else None,
            'untimed': untimed, 'bytes': size, 'skipped': len(opened.skipped),
            'scan_s': round(time.monotonic() - t_start, 2)}


def record_files(base, name):
    """The files of record `name` (relative to `base`, '/' between folders): its header, the
    signal files its header names, its .mrk file if there is one, and for a multi-segment record
    those of each segment; sorted, each once."""
    base = Path(base)
    out = set()

    def add(name):
        i = name.rfind('/')
        folder = name[:i + 1] if i >= 0 else ''
        out.add(f'{name}.hea')
        try:
            with _WFDB:
                h = wfdbx.read_header(str(base / name))
        except Exception:
            h = None
        for s in h.siginfo if h is not None else []:
            f = s.fname or ''
            if f and f not in ('-', '~'):
                out.add(folder + f)
        if (base / f'{name}.mrk').is_file():
            out.add(f'{name}.mrk')

    add(name)
    for seg, _ in segment_table(base / name) or []:
        if seg != '~':
            add(_beside(name, seg))
    return sorted(out)


class Reader:
    def __init__(self, path, meta):
        """The reader of the records at `path` (a folder, or a record's .hea file)."""
        self.folder = Path(path).parent if is_record(path) else Path(path)
        self.pieces = meta['pieces']

    def read(self, piece, a, b):
        if not 0 <= piece < len(self.pieces):
            raise ValueError(f'no piece {piece} in this recording')
        p = self.pieces[piece]
        check_files(self.folder, p['record'])
        path = self.folder / p['record']
        h = _header(path)
        n = max(h.nsamples, 0)
        a, b = min(max(0, a), n), min(max(0, b), n)
        if b <= a:
            return np.empty(0, dtype=np.float64)
        return _read(path, h.nsig, a, b, channels=[p['sig']])[:, 0]

    def close(self):
        pass


def fingerprint(path):
    """(name, size, mtime_ns) of each file, for the cache key: every file below a folder, or the
    files of one record (record_files); see wfdb_files."""
    return wfdb_files.fingerprint(path, record_files)
