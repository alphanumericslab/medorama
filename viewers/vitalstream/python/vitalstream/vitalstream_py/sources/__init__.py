"""Reading recordings: WFDB records in the viewer itself (wfdb_folder: one record, a
collection, or a folder of records), EDF, EDF+ and BDF files (edf), every other format through
an installed adapter (adapters).  Mirrors the Rust engine's sources/ module: building the index
of a recording (build(path, folder, progress, cancelled)), and a reader of the physical samples
[a, b) of a piece.

WFDB support needs wfdb-x, installed with the extra vitalstream_py[wfdb] (the installers and
`pip install '.[wfdb]'` include it).  Without it, wfdb_none is wfdb_folder here: WFDB records and
folders are still recognised (kind_of says 'wfdb'), and refused with NO_WFDB when they are opened
(unreadable, build); every other format is read as with it."""

from pathlib import Path

from .. import adapters
from ..common import json_loads
from . import edf
from .wfdb_files import NO_WFDB, signal_file_note

try:
    from . import wfdb_folder
except ImportError as e:   # wfdb-x is not installed (vitalstream_py without its [wfdb] extra)
    if e.name != 'wfdbx':
        raise
    from . import wfdb_none as wfdb_folder


def unreadable(path):
    """Why this installation cannot read the recording at `path`, if it cannot (else None): a WFDB
    record or a folder (which opens as WFDB records, build) without WFDB support (NO_WFDB).  The
    viewer refuses such a recording when it is opened, with this message."""
    p = Path(path)
    if not wfdb_folder.SUPPORTED and (p.is_dir() or wfdb_folder.is_record(p)):
        return NO_WFDB
    return None


def kind_of(path):
    """'wfdb' for a WFDB record (.hea file) or a WFDB folder (one that holds records itself, or a
    collection), 'edf' for an EDF or BDF file, an adapter's kind for a file it reads (by
    extension), or None.  Any folder can also be opened as the records below it (build); a folder
    that holds none itself is not a recording when a tree is searched."""
    p = Path(path)
    if p.is_file():
        if wfdb_folder.is_record(p):
            return 'wfdb'
        if edf.is_edf(p):
            return 'edf'
        found = adapters.for_file(p)
        return found[1].kind if found else None
    if wfdb_folder.is_wfdb_folder(p):
        return 'wfdb'
    return None


def not_a_recording(p):
    """Why `p` is not opened as a recording: for a WFDB signal file (.dat, .mat), to open its
    header in its place; else that it is not a recording this program reads."""
    return signal_file_note(p) or f'not a recording this program reads: {p}'


def format_label(path):
    """The label of the format a recording at `path` is read as ('WFDB record', 'EDF+', an
    adapter's label), or ''."""
    p = Path(path)
    if p.is_dir() or wfdb_folder.is_record(p):
        return wfdb_folder.format_label(p)
    if edf.is_edf(p):
        return edf.format_label(p)
    found = adapters.for_file(p)
    return found[1].label if found else ''


def build(path, folder, progress, cancelled=lambda: False):
    """The index of a recording: (meta, arrays) for WFDB records (a folder or a .hea file) and EDF
    files, built here; for a file an adapter reads, the adapter writes the index files into
    `folder` and (meta, None) is returned.  Stops with ValueError('indexing cancelled') once
    `cancelled` returns true."""
    p = Path(path)
    if p.is_dir() or wfdb_folder.is_record(p):
        meta, arrays = wfdb_folder.build(str(p), progress, cancelled)
        meta['kind'] = 'wfdb'
        meta['format'] = wfdb_folder.format_label(p)
        return meta, arrays
    if edf.is_edf(p):
        meta, arrays = edf.build(str(p), progress, cancelled)
        meta['kind'] = 'edf'
        meta['format'] = edf.format_label(p)
        return meta, arrays
    found = adapters.for_file(p)
    if found is None:
        raise ValueError(not_a_recording(p))
    adapter, fmt = found
    adapters.index(adapter, p, folder, progress, cancelled)
    try:
        meta = json_loads((Path(folder) / 'meta.json').read_text(encoding='utf-8'))
    except (OSError, ValueError, RecursionError) as e:
        raise ValueError(f'{adapter.name} wrote no valid meta.json: {e}') from None
    if not isinstance(meta, dict):
        raise ValueError(f'{adapter.name} wrote no valid meta.json')
    meta.update(kind=fmt.kind, format=fmt.label, adapter=adapter.name)
    return meta, None


class _AdapterReader:
    def __init__(self, adapter, folder):
        self.adapter, self.folder = adapter, folder

    def read(self, piece, a, b):
        return adapters.read(self.adapter, self.folder, piece, a, b)

    def close(self):
        pass


class _NoReader:
    def __init__(self, why):
        self.why = why

    def read(self, piece, a, b):
        raise ValueError(self.why)

    def close(self):
        pass


def reader(meta, folder):
    """The reader of the recording indexed in `folder` (its meta)."""
    if meta.get('kind') == 'wfdb':
        return wfdb_folder.Reader(meta['path'], meta)
    if meta.get('kind') == 'edf':
        return edf.Reader(meta['path'], meta)
    name = meta.get('adapter')
    if name:
        a = adapters.named(name)
        if a is None:
            return _NoReader(f'the {name} adapter is not installed: its samples cannot be read')
        return _AdapterReader(a, str(folder))
    return _NoReader('no reader for this recording')
