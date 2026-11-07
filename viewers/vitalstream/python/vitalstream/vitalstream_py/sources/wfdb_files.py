"""WFDB records as files, without reading them (no wfdb-x): which paths are records and WFDB
folders, the segment table of a multi-segment record, the format labels, the limits on a folder of
records, and a folder's files for the cache key.  Shared by the reader (wfdb_folder, with wfdb-x)
and by an installation without it (wfdb_none), which re-export it: without WFDB support a WFDB
record is still recognised, and refused (NO_WFDB) rather than taken for something else.  Mirrors
the Rust engine's sources/wfdb_files.rs."""

import os
import re
from dataclasses import dataclass
from pathlib import Path

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
