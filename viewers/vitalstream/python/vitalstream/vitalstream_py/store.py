"""The index store (index format 3, viewers/vitalstream/docs/index-format.md, "Version 3: the
index store"): one SQLite database of indexes, encrypted with SQLCipher when its owner has a key.
An index is built as version 2 files in a temporary folder, then packed into the store in one
transaction; envelopes are read back by range, a chunk at a time, their lengths from the arrays
table (a chunk or an array not of the length it states is refused).  The Rust engine
(libs/engine/src/store.rs) writes and reads the same stores.

Stores: the stores a viewer reads, and the one it writes: its own (cache_dir/index.sqlite,
encrypted with the key in index_key_file when one is set), or those Medorama gives it on standard
input (--keys-on-stdin), one JSON line each.  Keys are kept in memory only, and never logged."""

import collections
import itertools
import json
import os
import shutil
import threading
import time
import weakref
from pathlib import Path

import numpy as np
# SQLCipher 4 on SQLite (the wheels of sqlcipher3 bundle both): plain stores are read with it too
from sqlcipher3 import dbapi2 as sqlite

STORE_FORMAT = 3            # the store's format (store.format), and the index version it holds
CHUNK_VALUES = 4096         # values per chunk in a new store (store.chunk_values)
CACHED_CHUNKS = 16          # chunks of one array kept in memory once read
SIZES = {'f32': 4, 'f64': 8, 'i16': 2, 'i32': 4, 'i64': 8}
DTYPES = {'f32': '<f4', 'f64': '<f8', 'i16': '<i2', 'i32': '<i4', 'i64': '<i8'}
WHOLE = ('mk_piece.i32', 'mk_sample.i64', 'mk_code.i16', 'vit_t.f64', 'vit_v.f64')

_count = itertools.count()                 # this process's private folders, for unique names


def private_dir(path):
    """Make `path` (and its parents), a folder only its owner may read."""
    os.makedirs(path, exist_ok=True)
    if os.name == 'posix':
        os.chmod(path, 0o700)


def key_of(text):
    """A key's 64 hex digits, lowercase; None when `text` is not that."""
    text = text.strip() if isinstance(text, str) else ''
    if len(text) != 64:
        return None
    try:
        return bytes.fromhex(text).hex()
    except ValueError:
        return None


class Chunked:
    """An envelope array of a store read by range, a chunk at a time, the last chunks read kept.
    Sliced like a NumPy array (a[o:o+n]); `size` is its number of values."""

    def __init__(self, store, key, array, size, dtype):
        self._store, self._key, self._array = store, key, array
        self.size, self.dtype = size, np.dtype(dtype)
        self._chunk = store.chunk_values
        self._cache = collections.OrderedDict()
        self._lock = threading.Lock()

    def __len__(self):
        return self.size

    def _chunk_at(self, n):
        with self._lock:
            v = self._cache.get(n)
            if v is not None:
                return v
        data = self._store._query_one('SELECT data FROM chunks WHERE key = ? AND array = ? AND n = ?',
                                      (self._key, self._array, n))
        # a chunk holds chunk_values values, the last one the rest
        want = min(self._chunk, self.size - n * self._chunk) * self.dtype.itemsize
        if data is None or len(data) != want:
            raise ValueError(f'{self._array}: chunk {n} is missing or not of its length')
        v = np.frombuffer(bytes(data), self.dtype)
        with self._lock:
            self._cache[n] = v
            while len(self._cache) > CACHED_CHUNKS:
                self._cache.popitem(last=False)
        return v

    def range(self, o, n):
        """The values [o, o+n) (within the array)."""
        if n <= 0:
            return np.empty(0, self.dtype)
        c = self._chunk
        first, last = o // c, (o + n - 1) // c
        parts = []
        for k in range(first, last + 1):
            v = self._chunk_at(k)
            start = o - k * c if k == first else 0
            end = o + n - k * c if k == last else v.size
            parts.append(v[start:end])
        return parts[0].copy() if len(parts) == 1 else np.concatenate(parts)

    def __getitem__(self, s):
        if not isinstance(s, slice) or s.step not in (None, 1):
            raise TypeError('a chunked array is read by ranges')
        start, stop, _ = s.indices(self.size)
        return self.range(start, max(0, stop - start))


class ReadFolder(os.PathLike):
    """A private folder holding only an adapter recording's meta.json, for the adapter to read
    samples again (protocol 1); removed by remove(), or once nothing holds it."""

    def __init__(self, path, meta_text):
        self.path = str(path)
        private_dir(self.path)
        with open(os.path.join(self.path, 'meta.json'), 'w', encoding='utf-8', newline='\n') as f:
            f.write(meta_text)
        self._finalizer = weakref.finalize(self, shutil.rmtree, self.path, True)

    def __fspath__(self):
        return self.path

    def __str__(self):
        return self.path

    def remove(self):
        self._finalizer()


class Store:
    """An index store, opened (or made): `key` a raw 256-bit SQLCipher key as 64 hex digits
    (SQLCipher 4's default settings), or None for a plain store.  A new store gets
    `chunk_values` values per chunk (an existing one keeps its own): tests cut small arrays into
    several chunks.  A ValueError says why a store cannot be opened."""

    def __init__(self, path, key=None, chunk_values=CHUNK_VALUES):
        self.path = str(path)
        if key is not None and key_of(key) is None:
            raise ValueError(f'{self.path}: a key is 64 hex digits')
        self._key = key_of(key) if key is not None else None
        self._lock = threading.Lock()
        Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        c = self._connect()
        try:
            c.execute('PRAGMA journal_mode = WAL')
            c.executescript(
                'CREATE TABLE IF NOT EXISTS store (name TEXT PRIMARY KEY, value TEXT NOT NULL);'
                'CREATE TABLE IF NOT EXISTS indexes (key TEXT PRIMARY KEY, meta TEXT NOT NULL, engine TEXT NOT NULL,'
                ' written_at REAL NOT NULL);'
                'CREATE TABLE IF NOT EXISTS chunks (key TEXT NOT NULL, array TEXT NOT NULL, n INTEGER NOT NULL,'
                ' data BLOB NOT NULL, PRIMARY KEY (key, array, n)) WITHOUT ROWID;'
                'CREATE TABLE IF NOT EXISTS arrays (key TEXT NOT NULL, array TEXT NOT NULL, n_values INTEGER NOT NULL,'
                ' PRIMARY KEY (key, array)) WITHOUT ROWID;')
            c.execute("INSERT OR IGNORE INTO store (name, value) VALUES ('format', ?), ('chunk_values', ?)",
                      (str(STORE_FORMAT), str(max(1, int(chunk_values)))))
            got = dict(c.execute('SELECT name, value FROM store').fetchall())
        except sqlite.Error as e:
            c.close()
            raise ValueError(f'{self.path}: {e}') from None
        if got.get('format') != str(STORE_FORMAT):
            c.close()
            raise ValueError(f'{self.path}: another index store format')
        try:
            self.chunk_values = int(got.get('chunk_values', ''))
        except ValueError:
            self.chunk_values = 0
        if self.chunk_values <= 0:
            c.close()
            raise ValueError(f'{self.path}: no chunk size')
        self._conn = c
        self._sweep_tmp()

    def _sweep_tmp(self):
        """Remove what a writer or reader that ended without cleaning up left in the temporary
        folder: entries untouched for a day (no build takes that long)."""
        try:
            entries = list(os.scandir(self.tmp_dir))
        except OSError:
            return
        for e in entries:
            try:
                if time.time() - e.stat(follow_symlinks=False).st_mtime > 86400:
                    shutil.rmtree(e.path, ignore_errors=True)
            except OSError:
                pass

    def _connect(self):
        """A connection to the store, its key given first, checked."""
        try:
            c = sqlite.connect(self.path, timeout=30, isolation_level=None, check_same_thread=False)
        except sqlite.Error as e:
            raise ValueError(f'{self.path}: {e}') from None
        try:
            if self._key is not None:
                c.execute(f'PRAGMA key = "x\'{self._key}\'"')
            c.execute('SELECT COUNT(*) FROM sqlite_master').fetchone()
        except sqlite.Error:
            c.close()
            raise ValueError(f'{self.path}: cannot be read: it is encrypted, or its key is wrong') from None
        return c

    def close(self):
        with self._lock:
            self._conn.close()

    def forget_key(self):
        """No new connection is made (the key is dropped from memory); the one open stays."""
        self._key = None

    @property
    def tmp_dir(self):
        """Where indexes are built before they are packed: <store folder>/index.tmp."""
        return os.path.join(os.path.dirname(self.path) or '.', 'index.tmp')

    def _query_one(self, sql, args):
        with self._lock:
            row = self._conn.execute(sql, args).fetchone()
        return None if row is None else row[0]

    def _query_all(self, sql, args=()):
        with self._lock:
            return [r[0] for r in self._conn.execute(sql, args).fetchall()]

    def engine_of(self, key):
        """The engine that wrote the index of `key`, if there is one."""
        return self._query_one('SELECT engine FROM indexes WHERE key = ?', (key,))

    def meta(self, key):
        """The meta.json text of the index of `key`, if there is one."""
        return self._query_one('SELECT meta FROM indexes WHERE key = ?', (key,))

    def keys(self):
        """The keys of the indexes held."""
        return self._query_all('SELECT key FROM indexes ORDER BY key')

    def remove(self, key):
        """Remove the index of `key`; whether there was one."""
        with self._lock:
            c = self._conn
            c.execute('BEGIN IMMEDIATE')
            try:
                c.execute('DELETE FROM chunks WHERE key = ?', (key,))
                c.execute('DELETE FROM arrays WHERE key = ?', (key,))
                n = c.execute('DELETE FROM indexes WHERE key = ?', (key,)).rowcount
                c.execute('COMMIT')
            except BaseException:
                c.execute('ROLLBACK')
                raise
        return n > 0

    def pack(self, key, folder):
        """Pack the version 2 files of an index (its meta.json and arrays, in `folder`) into the
        store, in one transaction: an index of the key written by the same engine is kept, any
        other replaced."""
        folder = Path(folder)
        meta = (folder / 'meta.json').read_text(encoding='utf-8')
        try:
            parsed = json.loads(meta)
        except ValueError:
            parsed = None
        if not isinstance(parsed, dict):
            raise ValueError(f'{folder}: meta.json is not a JSON object')
        engine = parsed.get('engine') if isinstance(parsed.get('engine'), str) else ''
        files = sorted((f for f in folder.iterdir() if f.suffix[1:] in SIZES), key=lambda f: str(f))
        step = self.chunk_values
        # a connection of its own: readers of this store in this process are not held up
        c = self._connect()
        try:
            c.execute('BEGIN IMMEDIATE')
            try:
                if c.execute('SELECT engine FROM indexes WHERE key = ?', (key,)).fetchone() == (engine,):
                    c.execute('ROLLBACK')
                    return
                c.execute('DELETE FROM chunks WHERE key = ?', (key,))
                c.execute('DELETE FROM arrays WHERE key = ?', (key,))
                c.execute('DELETE FROM indexes WHERE key = ?', (key,))
                c.execute('INSERT INTO indexes (key, meta, engine, written_at) VALUES (?, ?, ?, ?)',
                          (key, meta, engine, time.time()))
                for f in files:
                    size = SIZES[f.suffix[1:]]
                    data = f.read_bytes()
                    if len(data) % size:
                        raise ValueError(f'{f}: not a whole number of values')
                    # every array has a row, an empty one a chunk of no bytes
                    parts = [data[i:i + step * size] for i in range(0, len(data), step * size)] or [b'']
                    c.execute('INSERT INTO arrays (key, array, n_values) VALUES (?, ?, ?)',
                              (key, f.name, len(data) // size))
                    c.executemany('INSERT INTO chunks (key, array, n, data) VALUES (?, ?, ?, ?)',
                                  [(key, f.name, n, part) for n, part in enumerate(parts)])
                c.execute('COMMIT')
            except BaseException:
                c.execute('ROLLBACK')
                raise
        finally:
            c.close()

    def _lengths(self, key):
        """The number of values of each array of the index of `key` (the arrays table)."""
        with self._lock:
            return dict(self._conn.execute('SELECT array, n_values FROM arrays WHERE key = ?', (key,)).fetchall())

    def _whole(self, key, array, n_values):
        """The bytes of a whole array, which must hold `n_values` values."""
        parts = self._query_all('SELECT data FROM chunks WHERE key = ? AND array = ? ORDER BY n', (key, array))
        data = b''.join(bytes(p) for p in parts)
        if len(data) != n_values * SIZES[array.rsplit('.', 1)[1]]:
            raise ValueError(f'{array}: not the length the arrays table states')
        return data

    def load(self, key):
        """(meta, arrays, folder) of the index of `key`, or None: no such index, or one whose
        meta.json and arrays do not agree (index-format.md, "Loading an index").  The envelopes
        are read by range; `folder`, for an adapter's recording, is a private folder holding its
        meta.json (ReadFolder), for as long as it is held; else the store's temporary folder."""
        try:
            return self._load(key)
        except (ValueError, sqlite.Error, OSError):
            return None

    def _load(self, key):
        from .common import json_loads
        from .index import check, is_int
        text = self.meta(key)
        if text is None:
            raise ValueError('no index')
        try:
            meta = json_loads(text)
        except (ValueError, RecursionError):
            raise ValueError('meta is not a JSON object') from None
        if not isinstance(meta, dict):
            raise ValueError('meta is not a JSON object')
        if not is_int(meta.get('index_version')) or meta['index_version'] != STORE_FORMAT:
            raise ValueError('another index version')
        lengths = self._lengths(key)
        for name, n in lengths.items():
            if not is_int(n) or n < 0:
                raise ValueError(f'{name}: a number of values that is not a whole number')
        arrays = {}
        lvl = 0
        while f'env_min_{lvl}.f32' in lengths:
            lo, hi = f'env_min_{lvl}.f32', f'env_max_{lvl}.f32'
            if hi not in lengths:
                raise ValueError(f'{hi}: missing')
            arrays[f'env_min_{lvl}'] = Chunked(self, key, lo, lengths[lo], '<f4')
            arrays[f'env_max_{lvl}'] = Chunked(self, key, hi, lengths[hi], '<f4')
            lvl += 1
        for name in WHOLE:
            if name not in lengths:
                raise ValueError(f'{name}: missing')
            t = name.rsplit('.', 1)[1]
            arrays[name.rsplit('.', 1)[0]] = np.frombuffer(self._whole(key, name, lengths[name]), DTYPES[t])
        check(meta, arrays)
        # an adapter reads samples again from its index folder (protocol 1): a private folder
        # holding only its meta.json, for as long as the index is loaded
        if isinstance(meta.get('adapter'), str) and meta['adapter']:
            private_dir(self.tmp_dir)
            folder = ReadFolder(os.path.join(self.tmp_dir, f'{key}.{os.getpid()}.{next(_count)}.read'), text)
        else:
            folder = self.tmp_dir
        return meta, arrays, folder


class Stores:
    """The index stores a viewer reads, and the one it writes (index-format.md, "Version 3: the
    index store"): those given on standard input, looked in first, and its own."""

    def __init__(self, own=None):
        self._given = []
        self._own = own
        self._lock = threading.Lock()

    @classmethod
    def own_from(cls, settings):
        """The viewer's own store: cache_dir/index.sqlite, encrypted with the key in
        index_key_file when one is set (a ValueError when it cannot be opened)."""
        key = None
        if settings.index_key_file:
            try:
                with open(settings.index_key_file, encoding='utf-8') as f:
                    key = key_of(f.read())
            except (OSError, UnicodeDecodeError) as e:
                raise ValueError(f'{settings.index_key_file}: {e}') from None
            if key is None:
                raise ValueError(f'{settings.index_key_file}: not 64 hex digits')
        return cls(Store(os.path.join(settings.cache_dir, 'index.sqlite'), key))

    def apply(self, line):
        """One line of the pipe: {"store": PATH, "key": HEX} (with "own": true, the store the
        viewer writes), or {"forget": PATH}.  A ValueError says what is wrong with it (never
        repeating the line, which may hold a key)."""
        try:
            v = json.loads(line)
        except ValueError:
            raise ValueError('not JSON') from None
        if not isinstance(v, dict):
            raise ValueError('not a JSON object')
        if isinstance(v.get('forget'), str):
            self.forget(v['forget'])
            return
        path = v.get('store')
        if not isinstance(path, str):
            raise ValueError('no store')
        key = key_of(v.get('key'))
        if key is None:
            raise ValueError('no key')
        store = Store(path, key)
        with self._lock:
            if v.get('own') is True:
                self._own = store
            else:
                self._given = [s for s in self._given if s.path != path] + [store]

    def forget(self, path):
        """Stop reading (and writing) the store at `path`, and drop its key."""
        with self._lock:
            gone = [s for s in self._given if s.path == path]
            self._given = [s for s in self._given if s.path != path]
            if self._own is not None and self._own.path == path:
                gone.append(self._own)
                self._own = None
        for s in gone:
            s.forget_key()

    def forget_all(self):
        for p in self.paths():
            self.forget(p)

    def holding(self, key):
        """The store holding the index of `key`, if any."""
        with self._lock:
            stores = self._given + ([self._own] if self._own is not None else [])
        for s in stores:
            try:
                if s.engine_of(key) is not None:
                    return s
            except sqlite.Error:
                continue
        return None

    def paths(self):
        """The paths of every store held."""
        with self._lock:
            return [s.path for s in self._given] + ([self._own.path] if self._own is not None else [])

    def own(self):
        """The store the viewer writes, if any."""
        with self._lock:
            return self._own


def read_keys(stores, stream, report):
    """Apply the lines of `stream` (Medorama's pipe) to `stores` until it closes, then forget
    every store.  `report` gets what was wrong with a line, never the line."""
    try:
        for line in stream:
            if not line.strip():
                continue
            try:
                stores.apply(line)
            except ValueError as e:
                report(f'a store from standard input: {e}')
    finally:
        stores.forget_all()
