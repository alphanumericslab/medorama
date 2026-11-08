"""An installation without WFDB support (wfdb-x not installed: vitalstream_py without its [wfdb]
extra; the Rust engine's wfdb_none.rs): no wfdb-x code at all.  WFDB records are still
recognised by their files (a .hea file, a folder of records, a collection: wfdb_files), so that
opening one is refused with NO_WFDB rather than taken for something else; every other format is
read as with wfdb-x.  The same names as wfdb_folder, so the callers are the same."""

from pathlib import Path

from . import wfdb_files
# re-exported: the same names as wfdb_folder
from .wfdb_files import MAX_FINGERPRINT_FILES, Limits, format_label, is_record, is_wfdb_folder  # noqa: F401
from .wfdb_files import limits, segment_table, set_limits, signal_file_note  # noqa: F401
from .wfdb_files import NO_WFDB

SUPPORTED = False   # this installation reads no WFDB records (wfdb-x is not installed)


def build(path, progress, cancelled=lambda: False):
    """Refused: NO_WFDB."""
    raise ValueError(NO_WFDB)


def scan(path, lim):
    """Refused: NO_WFDB."""
    raise ValueError(NO_WFDB)


def record_files(base, name):
    """The files of record `name` known without reading its header: the header itself (the
    signal files it names are known only to wfdb-x)."""
    return [f'{name}.hea']


def files_read(base, name):
    """record_files, as paths below `base`."""
    return [Path(f'{Path(base) / name}.hea')]


def check_files(base, name):
    """Refused: NO_WFDB (the record is not read at all)."""
    raise ValueError(NO_WFDB)


def fingerprint(path):
    """(name, size, mtime_ns) of each file, for the cache key (wfdb_files)."""
    return wfdb_files.fingerprint(path, record_files)


class Reader:
    """The samples of a WFDB recording indexed by an installation with WFDB support: refused."""

    def __init__(self, path, meta):
        pass

    def read(self, piece, a, b):
        raise ValueError(NO_WFDB)

    def close(self):
        pass
