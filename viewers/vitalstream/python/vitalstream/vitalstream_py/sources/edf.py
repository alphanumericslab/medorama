"""EDF, EDF+ and BDF files (European Data Format, https://www.edfplus.info/specs/): one file is one
recording (docs/edf.md, "Reading rules (for implementers)").  Mirrors the Rust engine's
sources/edf.rs.

- Each ordinary signal is a channel (its label and physical dimension), at
  samples per data record / duration of a data record Hz; physical values are
  pmin + (digital - dmin) * (pmax - pmin) / (dmax - dmin) (the digital values themselves when
  dmin == dmax).  BDF and BDF+ files hold 24-bit samples, EDF and EDF+ 16-bit.
- Data records follow each other from the header's start time (EDF), or from the onsets the
  EDF+ time-keeping annotations state (EDF+C and EDF+D): a record that does not start where the
  one before it ends begins a new piece of every channel, with a break between them.
- The annotations of EDF+ and BDF+ files (EDF Annotations, BDF Annotations signals) are events
  with their onset, duration and text.
- The header's patient and recording identification fields are never part of the index: they
  hold the patient's identity.

Data records are decoded a batch at a time with NumPy; only the annotation lists are read record
by record.
"""

import math
import os
import re
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path
from typing import Optional

import numpy as np

from ..common import Channels, Pieces, untimed_spans

BATCH_BYTES = 1 << 22      # records read at once while indexing (at most this many bytes)
CONTIGUOUS_S = 1e-6        # records closer than this (s) to where the one before ends follow it
# a number as Rust's f64::from_str reads it (the words inf, infinity and nan apart)
_NUMBER = re.compile(r'[+-]?(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?')
_INTEGER = re.compile(r'[+-]?[0-9]+')


def is_edf(path):
    """Whether `path` is a file named as an EDF or BDF file (.edf, .bdf, any case)."""
    p = Path(path)
    return p.is_file() and p.suffix.encode('utf-8', 'surrogateescape').lower() in (b'.edf', b'.bdf')


@dataclass
class Signal:
    """One signal of the header."""
    label: str
    unit: str
    pmin: float
    pmax: float
    dmin: float
    dmax: float
    spr: int                   # samples per data record
    annotations: bool          # an EDF Annotations (BDF Annotations) signal
    offset: int                # where its samples start in a data record (bytes)


@dataclass
class Header:
    """What the header says (the identification fields left out)."""
    bdf: bool                  # 24-bit samples (BDF, BDF+)
    plus: Optional[str]        # EDF+ or BDF+: 'C' continuous, 'D' discontinuous
    start: Optional[float]     # Unix seconds (the header's clock time taken as UTC), if valid
    header_bytes: int
    declared: int              # data records stated in the header (-1: unknown)
    records: int               # data records in the file (whole ones)
    rest: int                  # bytes after the last whole record
    duration: float            # seconds per data record
    signals: list = field(default_factory=list)
    record_bytes: int = 0


def _debug(s):
    """Rust's str Debug text, for the messages: in double quotes, control characters escaped."""
    out = []
    for c in s:
        o = ord(c)
        if c in '"\\':
            out.append('\\' + c)
        elif c in '\n\r\t':
            out.append({'\n': '\\n', '\r': '\\r', '\t': '\\t'}[c])
        elif o == 0:
            out.append('\\0')
        elif o < 0x20 or 0x7f <= o < 0xa0:
            out.append(f'\\u{{{o:x}}}')
        else:
            out.append(c)
    return '"' + ''.join(out) + '"'


def _display(x):
    """Rust's f64 Display (the shortest digits, never an exponent), for the messages."""
    s = format(Decimal(repr(x)), 'f')
    return s.rstrip('0').rstrip('.') if '.' in s else s


def _field(b):
    """A header field: its bytes as Latin-1 text, without the spaces around it."""
    return b.decode('latin-1').strip()


# Unicode whitespace (Rust's char::is_whitespace): str.isspace without the separators \x1c-\x1f,
# which float refuses although str.strip removes them
_WHITESPACE = ('\t\n\x0b\x0c\r \x85\xa0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008'
               '\u2009\u200a\u2028\u2029\u202f\u205f\u3000')


def parse_float(s):
    """Python's float(s) as the other viewers read it: surrounding whitespace allowed (not the
    separators \x1c-\x1f), one optional sign, inf/nan words, underscores between digits; ASCII
    only."""
    t = s.strip(_WHITESPACE)
    if not t or not t.isascii():
        return None
    lower = t.lower()
    body = lower[1:] if lower[:1] in ('+', '-') else lower
    if body in ('inf', 'infinity', 'nan'):
        v = math.nan if body == 'nan' else math.inf
        return -v if lower.startswith('-') else v
    if '_' in t:
        for i, c in enumerate(t):
            if c == '_' and not (0 < i < len(t) - 1 and t[i - 1].isdigit() and t[i + 1].isdigit()):
                return None
        t = t.replace('_', '')
    elif any(c not in '0123456789.eE+-' for c in t):
        return None
    return float(t) if _NUMBER.fullmatch(t) else None


def _number(b, what):
    s = _field(b)
    v = parse_float(s)
    if v is None or not math.isfinite(v):
        raise ValueError(f'EDF header: {what} is not a number ({_debug(s)})')
    return v


def _integer(b, what):
    s = _field(b)
    v = int(s) if _INTEGER.fullmatch(s) else None
    if v is None or not -(1 << 63) <= v < (1 << 63):
        raise ValueError(f'EDF header: {what} is not a whole number ({_debug(s)})')
    return v


def _days_from_civil(y, m, d):
    """Days from 1970-01-01 to a date of the proleptic Gregorian calendar."""
    y = y - 1 if m <= 2 else y
    era = (y if y >= 0 else y - 399) // 400
    yoe = y - era * 400
    mp = (m + 9) % 12
    doy = (153 * mp + 2) // 5 + d - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return era * 146097 + doe - 719468


def _three(s, loose):
    """Three two-digit numbers aa?bb?cc (any separators), as EDF writes dates and times; the
    third may be anything when `loose` (the year of an EDF+ date after 2084 is yy).  The text is
    read as its UTF-8 bytes."""
    b = s.encode('utf-8')

    def digit(i):
        return 48 <= b[i] <= 57

    def digits(i):
        return digit(i) and digit(i + 1)

    if len(b) != 8 or not digits(0) or not digits(3) or digit(2) or digit(5):
        return None

    def n(i):
        return (b[i] - 48) * 10 + b[i + 1] - 48

    if digits(6):
        return n(0), n(3), n(6)
    return (n(0), n(3), None) if loose else None


def _plus_year(recording):
    """The four-digit year of an EDF+ recording field ('Startdate 02-MAR-2002 ...'), if it
    states one."""
    words = recording.split(' ')
    if words[0] != 'Startdate' or len(words) < 2:
        return None
    date = words[1].encode('utf-8')
    if len(date) != 11 or not all(48 <= c <= 57 for c in date[7:]):
        return None
    return int(date[7:])


def start_of(date, time, recording=None):
    """The start of a file: its header's date (dd.mm.yy: 85-99 are 1985-1999, the others
    2000-2084; an EDF+ recording field's 'Startdate dd-MMM-yyyy' gives the year in full) and time
    (hh.mm.ss), in Unix seconds, or None."""
    full = _plus_year(recording) if recording is not None else None
    dmy = _three(date, full is not None)
    hms = _three(time, False)
    if dmy is None or hms is None:
        return None
    (d, m, yy), (hh, mi, ss) = dmy, hms
    if full is not None:
        year = full
    elif yy is not None:
        year = 1900 + yy if yy >= 85 else 2000 + yy
    else:
        return None
    if not (1 <= m <= 12 and 1 <= d <= 31) or hh > 23 or mi > 59 or ss > 59:
        return None
    return float(_days_from_civil(year, m, d) * 86400 + hh * 3600 + mi * 60 + ss)


def header(path):
    """The header of the EDF or BDF file at `path`."""
    size = os.path.getsize(path)
    with open(path, 'rb') as f:
        b = f.read(256)
        if len(b) != 256:
            raise ValueError(f'not an EDF or BDF file: {path}')
        # the signal fields, when the number of signals is one
        ns = _field(b[252:256])
        if _INTEGER.fullmatch(ns) and 1 <= int(ns) <= 9999:
            rest = f.read(256 * int(ns))
            if len(rest) != 256 * int(ns):
                raise ValueError(f'EDF header cut short: {path}')
            b += rest
    return _parse_header(b, size, path)


def header_from_bytes(data):
    """The header of an EDF or BDF file from its first bytes (at least the whole header), the
    file being len(data) bytes long: header without a file (for tests)."""
    return _parse_header(bytes(data), len(data), 'the data')


def _parse_header(b, size, path):
    """The header in `b` (the file's first bytes) of a file of `size` bytes, named `path` in
    errors."""
    fixed = b[:256]
    if len(fixed) != 256:
        raise ValueError(f'not an EDF or BDF file: {path}')
    bdf = fixed[0] == 0xFF and fixed[1:8] == b'BIOSEMI'
    if not bdf and _field(fixed[0:8]) != '0':
        raise ValueError(f'not an EDF or BDF file: {path}')
    reserved = _field(fixed[192:236])
    plus = next((p[-1] for p in ('EDF+C', 'EDF+D', 'BDF+C', 'BDF+D') if reserved.startswith(p)), None)
    recording = _field(fixed[88:168])
    start = start_of(_field(fixed[168:176]), _field(fixed[176:184]), recording if plus else None)
    header_bytes = _integer(fixed[184:192], 'the number of header bytes')
    declared = _integer(fixed[236:244], 'the number of data records')
    duration = _number(fixed[244:252], 'the duration of a data record')
    ns = _integer(fixed[252:256], 'the number of signals')
    if not 1 <= ns <= 9999 or header_bytes != 256 * (ns + 1):
        raise ValueError(f'EDF header: {ns} signals do not fit its {header_bytes} header bytes: {path}')
    rest = b[256:256 * (ns + 1)]
    if len(rest) != 256 * ns:
        raise ValueError(f'EDF header cut short: {path}')
    # the signal fields, each for every signal in turn
    cols, at = [], 0
    for w in (16, 80, 8, 8, 8, 8, 8, 80, 8, 32):
        cols.append([rest[at + k * w:at + (k + 1) * w] for k in range(ns)])
        at += w * ns
    width = 3 if bdf else 2
    signals, offset = [], 0
    for k in range(ns):
        label = _field(cols[0][k])
        spr = _integer(cols[8][k], 'a number of samples per data record')
        if spr < 0:
            raise ValueError(f'EDF header: signal {k + 1} has {spr} samples per data record')
        s = Signal(label=label, unit=_field(cols[2][k]), pmin=_number(cols[3][k], 'a physical minimum'),
                   pmax=_number(cols[4][k], 'a physical maximum'), dmin=_number(cols[5][k], 'a digital minimum'),
                   dmax=_number(cols[6][k], 'a digital maximum'), spr=spr,
                   annotations=label in ('EDF Annotations', 'BDF Annotations'), offset=offset)
        offset += s.spr * width
        signals.append(s)
    body = max(size - header_bytes, 0)
    records, rest_bytes = (0, body) if offset == 0 else divmod(body, offset)
    if declared >= 0:
        records = min(records, declared)
    if not duration > 0.0 and any(not s.annotations and s.spr > 0 for s in signals):
        raise ValueError(f'EDF header: a data record lasts {_display(duration)} s: {path}')
    # a sampling rate that is not a finite number (a data record of 1e-320 s)
    for s in signals:
        if not s.annotations and s.spr > 0 and not math.isfinite(s.spr / duration):
            raise ValueError(f'EDF header: signal {s.label} has no finite sampling rate '
                             f'({s.spr} samples in {_display(duration)} s): {path}')
    return Header(bdf=bdf, plus=plus, start=start, header_bytes=header_bytes, declared=declared, records=records,
                  rest=rest_bytes, duration=duration, signals=signals, record_bytes=offset)


def format_label(path):
    """The format label of an EDF or BDF file: 'EDF', 'EDF+', 'BDF', 'BDF+'."""
    try:
        h = header(path)
    except (OSError, ValueError):
        return 'EDF'
    return ('BDF' if h.bdf else 'EDF') + ('+' if h.plus else '')


def _tal_number(b):
    try:
        v = parse_float(b.decode('utf-8'))
    except UnicodeDecodeError:
        return None
    return v if v is not None and math.isfinite(v) else None


def tals(data):
    """The time-stamped annotation lists of an annotation signal's bytes: (onset, duration,
    texts), each +onset[\\x15duration]\\x14text\\x14...\\x14\\x00; texts decoded as UTF-8 (invalid
    bytes replaced), with their whitespace runs collapsed."""
    out = []
    for tal in data.split(b'\0'):
        if not tal:
            continue
        parts = tal.split(b'\x14')
        t = parts[0].split(b'\x15')
        onset = _tal_number(t[0])
        if onset is None:
            continue
        duration = _tal_number(t[1]) if len(t) > 1 else None
        out.append((onset, duration, [' '.join(p.decode('utf-8', 'replace').split()) for p in parts[1:]]))
    return out


def _digital(block, s, bdf):
    """The digital values of signal `s` in records `block` (uint8, records x record bytes):
    int32, records x samples per record."""
    n = block.shape[0]
    if bdf:
        b = block[:, s.offset:s.offset + 3 * s.spr].reshape(n, s.spr, 3).astype(np.int32)
        return ((b[..., 0] | (b[..., 1] << 8) | (b[..., 2] << 16)) ^ 0x800000) - 0x800000
    return np.ascontiguousarray(block[:, s.offset:s.offset + 2 * s.spr]).view('<i2').astype(np.int32)


def _physical(d, s):
    """Physical values (float64) of digital values `d` of signal `s`."""
    x = d.astype(np.float64)
    if s.dmax == s.dmin:
        return x
    gain = (s.pmax - s.pmin) / (s.dmax - s.dmin)
    return s.pmin + (x - s.dmin) * gain


def _read_records(f, h, first, count, path):
    n = count * h.record_bytes
    f.seek(h.header_bytes + first * h.record_bytes)
    buf = f.read(n)
    if len(buf) != n:
        raise OSError(f'{path}: failed to fill whole buffer')
    return np.frombuffer(buf, np.uint8).reshape(count, h.record_bytes)


def build(path, progress, cancelled=lambda: False):
    """(meta, arrays) of the EDF or BDF file at `path`; stops with ValueError('indexing
    cancelled') once `cancelled` returns true."""
    h = header(path)
    data = [k for k, s in enumerate(h.signals) if not s.annotations and s.spr > 0]
    notes_sig = [k for k, s in enumerate(h.signals) if s.annotations]
    if not data:
        raise ValueError(f'{path} holds no signals, only annotations')
    if h.records == 0:
        raise ValueError(f'{path} holds no data records')
    start = h.start if h.start is not None else 0.0
    timed = h.start is not None
    width = 3 if h.bdf else 2

    # channels, in the order of the signals; a label repeated gets its count ('ECG (2)')
    channels, seen, ch = Channels(), {}, []
    for k in data:
        s = h.signals[k]
        n = seen[s.label] = seen.get(s.label, 0) + 1
        ch.append(channels.get(f'edf\x01{k}', s.label if n == 1 else f'{s.label} ({n})', s.unit, 'edf'))

    pieces = Pieces()
    info = []              # per piece: signal, first data record
    current = []           # the pieces of the current run
    runs = []              # [start, end) of each run, from the file's start
    events = []
    batch = max(BATCH_BYTES // max(h.record_bytes, 1), 1)
    with open(path, 'rb') as f:
        k = 0
        while k < h.records:
            if cancelled():
                raise ValueError('indexing cancelled')
            count = min(batch, h.records - k)
            block = _read_records(f, h, k, count, path)
            segs = []      # [first, end) records of the batch in one run, and the run's pieces
            for j in range(count):
                # the record's onset: its time-keeping annotation (EDF+), else where it follows on
                onset = None
                rec = block[j]
                for i, a in enumerate(notes_sig):
                    s = h.signals[a]
                    for n, (t, d, texts) in enumerate(tals(rec[s.offset:s.offset + s.spr * width].tobytes())):
                        if i == 0 and n == 0 and h.plus:
                            onset = t
                        events += [(t, d if d is not None and d > 0.0 else 0.0, x) for x in texts if x]
                follows = runs[-1][1] if runs else None
                if onset is None:
                    onset = follows if follows is not None else 0.0
                if follows is None or abs(onset - follows) > CONTIGUOUS_S:
                    pieces.room(len(data))
                    runs.append([onset, onset])
                    current = []
                    for s, c in zip(data, ch):
                        info.append((s, k + j))
                        current.append(pieces.new(c, h.signals[s].spr / h.duration, t0=start + onset, timed=timed))
                    segs.append([j, j, current])
                elif not segs:
                    segs.append([j, j, current])
                runs[-1][1] = onset + h.duration
                segs[-1][1] = j + 1
            for q, s in enumerate(data):
                sig = h.signals[s]
                x = _physical(_digital(block, sig, h.bdf), sig).astype(np.float32)
                for a, b, ps in segs:
                    pieces.add(ps[q], x[a:b].ravel())
            k += count
            progress(k / h.records)

    items, arrays = pieces.finish()
    arrays.update(vit_t=np.empty(0, np.float64), vit_v=np.empty(0, np.float64))
    for p, (sig, first) in zip(items, info):
        p['sig'] = sig
        p['data_record'] = first
    gaps = []
    for (_, end), (begin, _) in zip(runs, runs[1:]):
        a, b = start + end, start + begin
        if b > a:
            gaps.append({'t0': a, 't1': b, 'reason': 'not recorded', 'gap_s': b - a, 'exact': True,
                         'measured_by': None, 'notes': ''})
    alarms = [{'t0': start + t, 't1': start + t + d, 'text': text, 'level': None, 'kind': 'annotation'}
              for t, d, text in events]
    alarms.sort(key=lambda a: a['t0'])

    notes = ["The file's start time is taken as UTC." if timed else
             'The file states no valid start time: times are counted from its start, and marked '
             '"time not stated".']
    if h.declared >= 0 and h.declared > h.records:
        notes.append(f'The header states {h.declared} data records; the file holds {h.records}.')
    elif h.rest > 0:
        notes.append(f'The file ends inside a data record: its last {h.rest} bytes are left out.')
    meta = {'channels': channels.items, 'pieces': items, 'vitals': [], 'alarms': alarms, 'gaps': gaps,
            'untimed': untimed_spans(items), 'marker_names': pieces.marker_names, 'skipped': {}, 'notes': notes,
            'time_resolution_s': None, 'reader': {}}
    return meta, arrays


class Reader:
    """Physical samples of an indexed EDF or BDF file's pieces."""

    def __init__(self, path, meta):
        self.path = path
        # per piece: signal, first data record, samples
        self.pieces = [(int(p.get('sig', 0)), int(p.get('data_record', 0)), int(p.get('n', 0)))
                       for p in meta.get('pieces', [])]

    def read(self, piece, a, b):
        """Physical samples [a, b) of a piece."""
        if not 0 <= piece < len(self.pieces):
            raise ValueError(f'no piece {piece} in this recording')
        sig, first, n = self.pieces[piece]
        a, b = min(max(a, 0), n), min(max(b, 0), n)
        if b <= a:
            return np.empty(0, np.float64)
        h = header(self.path)
        if sig >= len(h.signals) or h.signals[sig].spr == 0:
            raise ValueError('the file has changed since it was indexed')
        s = h.signals[sig]
        r0, r1 = a // s.spr, (b - 1) // s.spr
        if first + r1 >= h.records:
            raise ValueError('the file has changed since it was indexed')
        with open(self.path, 'rb') as f:
            block = _read_records(f, h, first + r0, r1 - r0 + 1, self.path)
        x = _physical(_digital(block, s, h.bdf), s).ravel()
        return x[a - r0 * s.spr:b - r0 * s.spr]

    def close(self):
        pass
