"""Index cache (viewers/vitalstream/docs/index-format.md): per recording a folder <key>/ with meta.json and one
little-endian binary file per array (envelopes, markers, vital-sign series), memory-mapped
when loaded.  The key derives from the recording's real path, size and modification time.
The Rust and C++ viewers write the same files, so the three can share one cache folder."""

import hashlib
import itertools
import json
import math
import os
import shutil
import threading
import time
from pathlib import Path

import numpy as np

from . import __version__
from .common import INDEX_VERSION, clean, json_loads
from .sources import build, kind_of, wfdb_folder

# value type of each array (by name; envelope arrays are env_min_<L>, env_max_<L>)
TYPES = {'env_min': 'f32', 'env_max': 'f32', 'mk_piece': 'i32', 'mk_sample': 'i64', 'mk_code': 'i16',
         'vit_t': 'f64', 'vit_v': 'f64'}
DTYPES = {'f32': '<f4', 'f64': '<f8', 'i16': '<i2', 'i32': '<i4', 'i64': '<i8'}


def dumps(obj):
    """Compact ASCII JSON text, as the other implementations write it (U+007F escaped too)."""
    return json.dumps(obj, separators=(',', ':'), allow_nan=False).replace('\x7f', '\\u007f')


def real_path(path):
    """The real path (symbolic links resolved), on Windows without the \\\\?\\ prefix."""
    p = str(Path(path).resolve())
    if p.startswith('\\\\?\\UNC\\'):
        return '\\\\' + p[8:]
    return p[4:] if p.startswith('\\\\?\\') else p


def cache_key(path):
    p = real_path(path)
    if os.path.isdir(p) or wfdb_folder.is_record(p):
        fp = [list(f) for f in wfdb_folder.fingerprint(p)]
    else:
        st = os.stat(p)
        fp = [st.st_size, st.st_mtime_ns]
    return hashlib.sha256(dumps([INDEX_VERSION, p, fp]).encode()).hexdigest()[:20]


def paths(cache_dir, key):
    """(index folder, progress file)."""
    d = Path(cache_dir)
    return d / key, d / f'{key}.progress.json'


_count = itertools.count()                 # this process's writers, for unique temporary names
_key_locks = {}                            # key -> the lock its writers take, one at a time
_key_locks_lock = threading.Lock()


def _unique(cache_dir, key, suffix):
    """A folder of `cache_dir` named uniquely for this writer: <key>.<process id>.<n>.<suffix>."""
    return Path(cache_dir) / f'{key}.{os.getpid()}.{next(_count)}.{suffix}'


def _key_lock(key):
    with _key_locks_lock:
        return _key_locks.setdefault(key, threading.Lock())


def _write_json(path, obj):
    tmp = path.with_name(path.name + f'.{os.getpid()}.{next(_count)}.tmp')
    with open(tmp, 'w', encoding='utf-8', newline='\n') as f:
        f.write(dumps(obj))
    os.replace(tmp, path)


def _file(name):
    """(file name, NumPy type) of an array."""
    t = TYPES[name.rsplit('_', 1)[0] if name.startswith('env_') else name]
    return f'{name}.{t}', DTYPES[t]


def write_arrays(folder, arrays):
    """Write the array files of an index into `folder` (viewers/vitalstream/docs/index-format.md, "Files of one
    index"); adapters written in Python use it for their `index` command."""
    for name, a in arrays.items():
        file, dtype = _file(name)
        np.ascontiguousarray(a, dtype=dtype).tofile(Path(folder) / file)


def build_index(path, cache_dir, cancelled=None):
    """Index `path` into the cache (runs in a worker process).  Returns the key.  Once
    `cancelled` (a function) returns true, indexing stops with ValueError('indexing
    cancelled'), and nothing is left behind: no index, no temporary folder, no progress file.

    The index is built in a folder named uniquely for this writer, checked as a reader would
    check it, and moved into place: an index already there, complete and written by this
    version, is kept (the same content); any other is moved aside first, so that readers never
    see a folder half replaced (docs/index-format.md, "Writing and sharing")."""
    stop = cancelled or (lambda: False)
    os.makedirs(cache_dir, exist_ok=True)
    key = cache_key(path)
    folder, prog_path = paths(cache_dir, key)
    last = [0.0]
    t_start = time.time()

    def progress(fraction):
        now = time.time()
        if now - last[0] >= 0.5:
            last[0] = now
            _write_json(prog_path, {'state': 'indexing', 'fraction': min(1.0, max(0.0, fraction)),
                                    'elapsed_s': now - t_start})

    with _key_lock(key):
        tmp = _unique(cache_dir, key, 'tmp')
        old = None
        try:
            progress(0.0)
            tmp.mkdir()
            meta, arrays = build(path, tmp, progress, stop)
            if arrays is not None:          # otherwise the adapter wrote the arrays into tmp
                write_arrays(tmp, arrays)
            meta.update(path=real_path(path), name=Path(path).name, key=key, index_version=INDEX_VERSION,
                        engine=__version__, indexed_s=round(time.time() - t_start, 1))
            _write_json(tmp / 'meta.json', clean(meta))
            # what a reader would refuse is not kept (times or rates that are not finite numbers, ...)
            try:
                _load_from(tmp)
            except ValueError as e:
                raise ValueError(f'{path}: the index would be inconsistent: {e}') from None
            try:
                current = _load_from(folder)[0].get('engine') == __version__
            except ValueError:
                current = False
            if not current:
                if folder.exists():
                    old = _unique(cache_dir, key, 'old')
                    os.replace(folder, old)
                try:
                    os.replace(tmp, folder)
                except OSError as e:
                    # another program moved its index of this key in first
                    try:
                        _load_from(folder)
                    except ValueError:
                        raise e from None
            _write_json(prog_path, {'state': 'ready', 'fraction': 1.0, 'elapsed_s': time.time() - t_start})
        except BaseException:
            # the error is the job's (its message); nothing is left in the index folder
            try:
                os.remove(prog_path)
            except OSError:
                pass
            raise
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
            if old is not None:
                shutil.rmtree(old, ignore_errors=True)
    return key


def read_progress(cache_dir, key):
    _, prog = paths(cache_dir, key)
    try:
        return json_loads(prog.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return None


def load_index(cache_dir, key):
    """(meta, arrays, index folder) from the cache, or None: no index under `key`, one of another
    version, or one whose meta.json and arrays do not agree (check)."""
    if not key or any(c not in '0123456789abcdefABCDEF' for c in key):
        return None
    folder, _ = paths(cache_dir, key)
    try:
        meta, arrays = _load_from(folder)
    except ValueError:
        return None
    return meta, arrays, folder


def _load_from(folder):
    """(meta, arrays) of the index in `folder`; a ValueError saying why it cannot be used."""
    folder = Path(folder)
    try:
        meta = json_loads((folder / 'meta.json').read_text(encoding='utf-8'))
    except OSError as e:
        raise ValueError(f'meta.json: {e}') from None
    except (ValueError, UnicodeDecodeError, RecursionError):
        raise ValueError('meta.json is not a JSON object') from None
    if not isinstance(meta, dict):
        raise ValueError('meta.json is not a JSON object')
    if not _is_int(meta.get('index_version')) or meta['index_version'] != INDEX_VERSION:
        raise ValueError('another index version')
    names = []
    lvl = 0
    while (folder / f'env_min_{lvl}.f32').exists():
        names += [f'env_min_{lvl}', f'env_max_{lvl}']
        lvl += 1
    names += ['mk_piece', 'mk_sample', 'mk_code', 'vit_t', 'vit_v']
    arrays = {name: _load(folder, name) for name in names}
    check(meta, arrays)
    return meta, arrays


def _is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def check(meta, arrays):
    """A ValueError unless meta.json agrees with the arrays (docs/index-format.md, "Loading an
    index"): every offset and length inside its array, so that queries never read outside one."""
    def lst(k):
        v = meta.get(k)
        if v is None and k not in meta:
            return []
        if not isinstance(v, list):
            raise ValueError(f'{k} is not a list')
        return v

    def get(v, k):
        return v.get(k) if isinstance(v, dict) else None

    def integer(v, k):
        x = get(v, k)
        if not _is_int(x):
            raise ValueError(f'a {k} that is not a whole number')
        return x

    def number(v, k):
        x = get(v, k)
        if _is_int(x) or (isinstance(x, float) and math.isfinite(x)):
            return float(x)
        raise ValueError(f'a {k} that is not a finite number')

    levels = 0
    while f'env_min_{levels}' in arrays:
        levels += 1
    channels = len(lst('channels'))
    pieces = lst('pieces')
    for i, p in enumerate(pieces):
        try:
            if integer(p, 'id') != i:
                raise ValueError('its id is not its position')
            ch = integer(p, 'ch')
            if not 0 <= ch < channels:
                raise ValueError(f'channel {ch} is not among the {channels} channels')
            fs = number(p, 'fs')
            if not fs > 0:
                raise ValueError(f'a sampling rate of {fs}')
            number(p, 't0')
            if integer(p, 'n') < 0:
                raise ValueError('a negative number of samples')
            env = p.get('env', [])
            if not isinstance(env, list):
                raise ValueError('env is not a list')
            if len(env) > levels:
                raise ValueError(f'{len(env)} envelope levels; the index has {levels}')
            for lvl, e in enumerate(env):
                if not (isinstance(e, list) and len(e) == 2 and _is_int(e[0]) and _is_int(e[1])):
                    raise ValueError(f'envelope level {lvl} is not [offset, length]')
                off, length = e
                size = min(arrays[f'env_min_{lvl}'].size, arrays[f'env_max_{lvl}'].size)
                if off < 0 or length < 0 or off + length > size:
                    raise ValueError(f'envelope level {lvl} [{off}, {length}] lies outside its {size} values')
        except ValueError as e:
            raise ValueError(f'piece {i}: {e}') from None
    mp, ms, mc = arrays['mk_piece'], arrays['mk_sample'], arrays['mk_code']
    if not mp.size == ms.size == mc.size:
        raise ValueError('the marker arrays differ in length')
    if mp.size and (int(mp.min()) < 0 or int(mp.max()) >= len(pieces) or bool((np.diff(mp) < 0).any())):
        raise ValueError('mk_piece names a piece that does not exist, or is not in order')
    if mc.size and (int(mc.min()) < 0 or int(mc.max()) >= len(lst('marker_names'))):
        raise ValueError('mk_code names a marker name that does not exist')
    vt = arrays['vit_t'].size
    if vt != arrays['vit_v'].size:
        raise ValueError('vit_t and vit_v differ in length')
    for v in lst('vitals'):
        off, n = integer(v, 'offset'), integer(v, 'n')
        if off < 0 or n < 0 or off + n > vt:
            raise ValueError(f'a vital-sign series [{off}, {n}] lies outside the {vt} values')


def _load(folder, name):
    file, dtype = _file(name)
    path = folder / file
    try:
        size = os.path.getsize(path)
    except OSError as e:
        raise ValueError(f'{file}: {e}') from None
    if size % np.dtype(dtype).itemsize:
        raise ValueError(f'{file}: not a whole number of values')
    if size == 0:                           # empty arrays cannot be memory-mapped
        return np.empty(0, dtype)
    # index files are written once (their folder is renamed into place) and never modified
    try:
        return np.memmap(path, dtype=dtype, mode='r')
    except OSError as e:
        raise ValueError(f'{file}: {e}') from None


__all__ = ['build_index', 'cache_key', 'dumps', 'kind_of', 'load_index', 'read_progress', 'write_arrays']
