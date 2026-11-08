"""WFDB records as files, without reading them (no wfdb-x): which paths are records and WFDB
folders, the segment table of a multi-segment record, the format labels, the limits on a folder of
records, and a folder's files for the cache key.  Shared by the reader (wfdb_folder, with wfdb-x)
and by an installation without it (wfdb_none), which re-export it: without WFDB support a WFDB
record is still recognised, and refused (NO_WFDB) rather than taken for something else.  Mirrors
the Rust engine's sources/wfdb_files.rs."""

import os
import re
import struct
from dataclasses import dataclass
from pathlib import Path

from .. import paths

MAX_FINGERPRINT_FILES = 200_000  # the most files a folder's key is computed from (fingerprint)
# The refusal of a WFDB record or folder by an installation without WFDB support (wfdb_none).
NO_WFDB = "this build has no WFDB support (wfdb-x is not installed: pip install 'vitalstream_py[wfdb]')"


@dataclass(frozen=True)
class Limits:
    """What a folder of plain records may hold before it is read: records (header files, the
    segments of multi-segment records included), folder levels below it, and seconds for the
    scan that counts them (docs/wfdb-collection.md, "Limits").  A collection (index.csv) lists
    its records itself and is not limited."""
    max_records: int = 5000
    max_depth: int = 20
    max_scan_s: float = 10.0


_limits = Limits()


def set_limits(limits):
    """Use `limits` for every folder read from now on (the programs' settings)."""
    global _limits
    _limits = limits


def limits():
    """The limits in force."""
    return _limits


def is_wfdb_folder(path, scan_limit=2000):
    """A folder written by the converters (index.csv), or holding WFDB records itself (RECORDS,
    or a .hea file among its first `scan_limit` entries).  Any folder can be opened as records
    (the reader reads its subfolders too); this tells which folders are recordings when a tree is
    searched for them."""
    p = Path(path)
    if not p.is_dir():
        return False
    if (p / 'index.csv').is_file() or (p / 'RECORDS').is_file():
        return True
    try:
        with os.scandir(p) as it:
            for i, e in enumerate(it):
                if i >= scan_limit:
                    break
                if e.name.endswith('.hea'):
                    return True
    except OSError:
        pass
    return False


def is_record(path):
    """A WFDB header file (.hea): one record."""
    p = Path(path)
    return p.suffix == '.hea' and p.is_file()


def header_lines(hea):
    """The lines of a header file that are neither blank nor comments, trimmed."""
    try:
        text = Path(hea).read_bytes().decode('utf-8', 'replace')
    except OSError:
        return []
    lines = (ln.strip() for ln in text.split('\n'))
    return [ln for ln in lines if ln and not ln.startswith('#')]


def segment_table(record):
    """The segments of a multi-segment record, [(name, samples)] in order ('~' names a null
    segment); None for a single-segment record.  `record`: the record's path without .hea."""
    # the name with `.hea` added: a record name may hold a dot (`a.v1`)
    lines = header_lines(Path(f'{record}.hea'))
    first = lines[0].split() if lines else []
    if not first or '/' not in first[0]:
        return None
    nseg = first[0].split('/', 1)[1]
    if not re.fullmatch(r'\+?[0-9]+', nseg):
        return None
    out = []
    for ln in lines[1:1 + int(nseg)]:
        w = ln.split()
        if len(w) > 1 and re.fullmatch(r'[+-]?[0-9]+', w[1]):
            out.append((w[0], int(w[1])))
    return out


def format_label(path):
    """The format label of what `path` opens as."""
    p = Path(path)
    if is_record(p):
        return 'WFDB multi-segment record' if segment_table(p.with_suffix('')) is not None else 'WFDB record'
    if (p / 'index.csv').is_file():
        return 'WFDB collection'
    return 'WFDB records'


def fingerprint(path, record_files):
    """(name, size, mtime_ns) of each file, for the cache key: every file below a folder (hidden
    entries and links to folders skipped; a link to a file counts with its target's size and
    time), or the files of one record (record_files(base, name)); `name` relative to the folder (the
    record's folder), with '/' between components on every system
    (viewers/vitalstream/docs/index-format.md).  A folder's walk visits names in order, enters at
    most `max_depth` levels of subfolders (limits) and stops after MAX_FINGERPRINT_FILES files."""
    def stat(p):
        try:
            st = os.stat(p)
        except OSError:
            return None
        return st.st_size, max(st.st_mtime_ns, 0)

    out = []
    if is_record(path):
        p = Path(path)
        for f in record_files(p.parent, p.stem):
            s = stat(p.parent / f)
            if s is not None:
                out.append((f, *s))
        return out

    def walk(d, rel, depth):
        try:
            with os.scandir(d) as it:
                entries = sorted((e for e in it if not e.name.startswith('.')), key=lambda e: e.name)
        except OSError:
            return
        for e in entries:
            if len(out) >= MAX_FINGERPRINT_FILES:
                return
            r = f'{rel}/{e.name}' if rel else e.name
            try:
                is_dir, is_link = e.is_dir(follow_symlinks=False), e.is_symlink()
            except OSError:
                continue
            if is_dir:
                if depth < max_depth:
                    walk(e.path, r, depth + 1)
            elif is_link and not os.path.isfile(e.path):
                continue                 # a link to a folder, or a broken link
            else:
                s = stat(e.path)
                if s is not None:
                    out.append((r, *s))

    max_depth = limits().max_depth
    walk(str(path), '', 0)
    return sorted(out)


# ------------------------------------------------------------------ signal files

def is_signal_file(path):
    """Whether `path` is named as a WFDB signal file is (.dat, or .mat, a MATLAB file a header
    names): not a record of its own, but read with the header that names it."""
    return Path(path).suffix.lower() in ('.dat', '.mat')


def signal_file_note(path):
    """For a WFDB signal file (.dat, .mat) opened or added as a recording: what to open in its
    place, its header (with the name of the header next to it that names it, when there is one).
    None for any other path."""
    p = Path(path)
    if not p.is_file() or not is_signal_file(p):
        return None
    hea = p.with_suffix('.hea')
    names_it = any((f := _signal_path(hea, s.file)) is not None and f.name == p.name and f.parent == hea.parent
                   for s in _header_record(hea)[1])
    if names_it:
        return f'{p.name} is a WFDB signal file: open its header, {hea.name}'
    return f'{p.name} is a WFDB signal file: open the header (.hea) that names it'


@dataclass(frozen=True)
class SignalLine:
    """One signal line of a header: its file name, storage format, samples per frame and byte
    offset (`file fmt[xspf][:skew][+offset] ...`)."""
    file: str
    fmt: int
    spf: int
    offset: int


def _header_record(hea):
    """The record line's number of samples per signal (0: not stated), and the signal lines of a
    single-segment header (none for a multi-segment one, or a header that cannot be read)."""
    lines = header_lines(hea)
    rec = lines[0].split() if lines else []
    if not rec or '/' in rec[0]:
        return 0, []
    nsig = int(rec[1]) if len(rec) > 1 and rec[1].isdigit() else 0
    nsamp = int(rec[3]) if len(rec) > 3 and rec[3].isdigit() else 0

    def digits(t):
        """The number at the start of t, or None."""
        d = re.match(r'[0-9]*', t).group()
        return int(d) if d else None

    out = []
    for ln in lines[1:1 + nsig]:
        w = ln.split()
        if len(w) < 2:
            break
        f = w[1]
        fmt = digits(f)
        if fmt is None:
            break
        spf = digits(f.split('x', 1)[1]) if 'x' in f else None
        offset = digits(f.split('+', 1)[1]) if '+' in f else None
        out.append(SignalLine(w[0], fmt, max(spf or 1, 1), offset or 0))
    return nsamp, out


def _signal_path(hea, f):
    """The file a header's signal file name designates, as wfdb-x opens it: '~' expanded, an
    absolute name as given, any other next to the header; None for a signal without a file."""
    if f in ('', '-', '~'):
        return None
    f = paths.expand(f)
    if os.path.isabs(f):
        return Path(f)
    return Path(hea).parent / f


@dataclass(frozen=True)
class _MatLayout:
    """How a MAT file stores its matrix: element type, byte order, size and where the elements
    start; `nbytes`: the bytes of the elements a level 5 file states (None for level 4)."""
    elem: str
    big_endian: bool
    rows: int
    cols: int
    start: int
    nbytes: object = None


# element: (bytes, text)
_MAT_ELEMS = {
    'i8': (1, '8-bit integers'), 'u8': (1, '8-bit unsigned integers'), 'i16': (2, '16-bit integers'),
    'u16': (2, '16-bit unsigned integers'), 'i32': (4, '32-bit integers'), 'u32': (4, '32-bit unsigned integers'),
    'i64': (8, '64-bit integers'), 'u64': (8, '64-bit unsigned integers'), 'f32': (4, 'single-precision numbers'),
    'f64': (8, 'double-precision numbers'),
}
# the elements a WFDB storage format reads from a MAT file, and their byte order (None: either,
# for one byte); a format not here is held by no MAT matrix
_MAT_OF_FORMAT = {80: ('u8', None), 16: ('i16', False), 61: ('i16', True), 160: ('u16', False), 32: ('i32', False)}
_MAT4_ELEMS = ['f64', 'f32', 'i32', 'i16', 'u16', 'u8']
_MAT5_ELEMS = {1: 'i8', 2: 'u8', 3: 'i16', 4: 'u16', 5: 'i32', 6: 'u32', 7: 'f32', 9: 'f64', 12: 'i64', 13: 'u64'}


class _NotMat(Exception):
    pass


def _mat_layout(head):
    """The layout of the first matrix of a MAT file (level 4, or level 5 uncompressed), from its
    first bytes; _NotMat saying why it is not one WFDB can read."""
    # level 5: "If any of the first four bytes is zero, it is a level 4 file" (the MAT-file format)
    if len(head) >= 4 and all(head[:4]):
        return _mat5_layout(head)
    if len(head) < 20:
        raise _NotMat('it is too short for a MAT file')

    def word(i, be):
        return struct.unpack_from('>I' if be else '<I', head, i)[0]

    # the type MOPT: M the byte order (0 little-endian, 1 big-endian), O 0, P the element, T the kind
    def valid(t, m):
        return t < 5000 and t // 1000 == m and (t // 100) % 10 == 0 and (t // 10) % 10 <= 5
    if valid(word(0, False), 0):
        be = False
    elif valid(word(0, True), 1):
        be = True
    else:
        raise _NotMat('it is not a MAT file (level 4 or 5) that WFDB reads')
    t = word(0, be)
    if t % 10:
        raise _NotMat('it holds text, not a numeric matrix' if t % 10 == 1 else 'it holds a sparse matrix')
    if word(12, be):
        raise _NotMat('it holds a complex matrix')
    return _MatLayout(_MAT4_ELEMS[(t // 10) % 10], be, word(4, be), word(8, be), 20 + word(16, be))


def _mat5_layout(head):
    order = head[126:128]
    if order not in (b'IM', b'MI'):
        raise _NotMat('it is not a MAT file (level 4 or 5) that WFDB reads')
    be = order == b'MI'
    e = '>' if be else '<'
    version = struct.unpack_from(e + 'H', head, 124)[0]
    if version == 0x0200:
        raise _NotMat("it is a MATLAB 7.3 (HDF5) file: save it with save(..., '-v6') or '-v4'")
    if version != 0x0100:
        raise _NotMat(f'it is a MAT file of an unknown version ({version:#06x})')

    def u32(i):
        if i + 4 > len(head):
            raise _NotMat('it ends inside its first matrix')
        return struct.unpack_from(e + 'I', head, i)[0]

    def tag(i):
        """A data element: its type, bytes, where its data start and where the next element
        does; a small element packs type and bytes in 4 bytes, with its data in the next 4."""
        w = u32(i)
        if w >> 16:
            return w & 0xffff, w >> 16, i + 4, i + 8
        n = u32(i + 4)
        return w, n, i + 8, i + 8 + -(-n // 8) * 8

    kind, _, at, _ = tag(128)
    if kind == 15:
        raise _NotMat("it is compressed (MATLAB saves so by default): save it with save(..., '-v6') or '-v4'")
    if kind != 14:
        raise _NotMat('its first variable is not a matrix')
    # array flags: the class in the low byte, the complex flag (0x08) in the next
    _, _, data, at = tag(at)
    flags = u32(data)
    if (flags >> 8) & 0x08:
        raise _NotMat('it holds a complex matrix')
    if flags & 0xff == 5:
        raise _NotMat('it holds a sparse matrix')
    if not 6 <= flags & 0xff <= 15:
        raise _NotMat('its first variable is not a numeric matrix')
    # dimensions: two 32-bit integers
    _, n, data, at = tag(at)
    if n != 8:
        raise _NotMat(f'its first variable has {n // 4} dimensions, not 2')
    rows, cols = u32(data), u32(data + 4)
    _, _, _, at = tag(at)          # the name
    t, n, data, _ = tag(at)        # the real part
    if t not in _MAT5_ELEMS:
        raise _NotMat(f'its elements are of an unknown type ({t})')
    return _MatLayout(_MAT5_ELEMS[t], be, rows, cols, data, n)


def _mat_mismatch(head, size, signals, nsamp):
    """Why a MAT signal file does not match the header that names it, or None: the matrix must
    hold the elements of the header's format, one row per signal of the file (one per sample of
    a frame), one column per frame, starting at the header's byte offset.  `signals`: the
    header's signals in this file; `nsamp`: the record line's samples per signal (0: not stated)."""
    try:
        m = _mat_layout(head)
    except _NotMat as e:
        return str(e)
    s = signals[0]
    other = next((o for o in signals if o.fmt != s.fmt), None)
    if other:
        return f'the header gives it two storage formats ({s.fmt} and {other.fmt})'
    if s.fmt not in _MAT_OF_FORMAT:
        return (f'the header gives format {s.fmt}, which no MAT matrix holds '
                '(8-bit: 80, 16-bit: 16 or 61, 32-bit: 32)')
    elem, order = _MAT_OF_FORMAT[s.fmt]
    endian = {False: 'little-endian', True: 'big-endian'}
    if m.elem != elem or (order is not None and order != m.big_endian):
        size_of, text = _MAT_ELEMS[m.elem]
        got = f'{endian[m.big_endian]} {text}' if size_of > 1 else text
        want = f'{endian[order]} {_MAT_ELEMS[elem][1]}' if order is not None else _MAT_ELEMS[elem][1]
        return f'it holds {got}; format {s.fmt} reads {want}'
    rows = sum(x.spf for x in signals)
    if m.rows != rows:
        hint = ' (it is transposed: one column per signal)' if m.cols == rows else ''
        return f'its matrix has {m.rows} rows; the header gives {rows} signals in it, one row each{hint}'
    if m.start != s.offset:
        return f'its samples start at byte {m.start}; the header gives +{s.offset}'
    if nsamp > 0 and m.cols != nsamp:
        return f'it holds {m.cols} samples per signal; the header gives {nsamp}'
    need = m.rows * m.cols * _MAT_ELEMS[m.elem][0]
    if (m.nbytes is not None and m.nbytes != need) or size < m.start + need:
        return f'it is shorter than its {m.rows} × {m.cols} matrix'
    return None


def check_mat_files(hea):
    """ValueError for the first MAT signal file (.mat) of a single-segment header that does not
    match it (_mat_mismatch): a MAT file is read as WFDB reads a signal file, its matrix as
    frames from the header's byte offset, so a file of another layout (compressed, of doubles,
    transposed) would be read as noise.  Files that cannot be opened, or lie outside the data
    roots, are not checked here (reading them fails)."""
    nsamp, signals = _header_record(hea)
    files = {}
    for s in signals:
        if Path(s.file).suffix.lower() == '.mat':
            files.setdefault(s.file, []).append(s)
    for file, sigs in files.items():
        p = _signal_path(hea, file)
        if p is None or not paths.allowed_file(p):
            continue
        try:
            with open(p, 'rb') as fh:
                size = os.fstat(fh.fileno()).st_size
                head = fh.read(512)
        except OSError:
            continue
        why = _mat_mismatch(head, size, sigs, nsamp)
        if why:
            raise ValueError(f'{file} does not match its header {Path(hea).name}: {why}')
