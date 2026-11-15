"""The index store (index format 3, viewers/vitalstream/docs/index-format.md, "Version 3: the index
store"): an index packed into it answers as the same index in a folder (format 2) does, across
chunk boundaries, encrypted or not; without its key nothing is read; an index is kept, replaced
and removed whole; a cancelled build leaves nothing; the keys on a pipe; an adapter's recording
read again through a private folder; and stores written by the Rust viewer read here, and the
reverse (when vitalstream-rs is built)."""

import io
import json
import os
import socket
import stat
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import numpy as np
import pytest

from vitalstream_py import adapters
from vitalstream_py import settings as settings_module
from vitalstream_py.dataset import Dataset
from vitalstream_py.index import build_index, build_into_store, cache_key, load_index
from vitalstream_py.sources import wfdb_folder
from vitalstream_py.store import CHUNK_VALUES, Chunked, Store, Stores, read_keys

needs_wfdb = pytest.mark.skipif(not wfdb_folder.SUPPORTED,
                                reason='WFDB records: an installation without wfdb-x refuses them')
posix = pytest.mark.skipif(os.name == 'nt', reason='POSIX permissions and programs')

VIEWER = Path(__file__).resolve().parents[3]                    # viewers/vitalstream
REPO = VIEWER.parents[1]
TESTDATA = VIEWER / 'testdata'
FAKE_ADAPTER = REPO / 'libs' / 'engine' / 'tests' / 'fake_adapter.py'
EXE = '.exe' if os.name == 'nt' else ''
RUST = next((p for p in (REPO / 'target' / d / f'vitalstream-rs{EXE}' for d in ('release', 'debug')) if p.is_file()),
            None)
needs_rust = pytest.mark.skipif(RUST is None, reason='vitalstream-rs is not built (cargo build -p vitalstream-rs)')
KEY1, KEY2 = '01' * 32, '02' * 32
# what each recording is read with: WFDB records need wfdb-x, EDF files do not
RECORDINGS = [pytest.param('collection', marks=needs_wfdb), pytest.param('session', marks=needs_wfdb),
              'edf/psg_plus.edf', 'edf/gappy_plus.edf']


@pytest.fixture(autouse=True)
def default_build(monkeypatch):
    from vitalstream_py import auth, paths
    monkeypatch.setattr(settings_module, 'BUILD_ROOT', '')
    monkeypatch.setattr(auth, 'SUPPORTED', True)
    yield
    paths.set_allowed_roots(None)


def strip(meta):
    """meta without what differs between versions and builds."""
    return {k: v for k, v in meta.items() if k not in ('index_version', 'key', 'engine', 'indexed_s')}


def same_answers(a, b):
    """The same queries on two datasets of one recording give the same answers."""
    ia, ib = a.info(), b.info()
    ia.pop('key')
    ib.pop('key')
    assert ia == ib
    t0, t1 = ia['t_range']
    channels = list(range(len(a.meta['channels'])))
    for x, y, px in [(t0, t1, 800), (t0, t0 + 10.0, 1200), (t0 + (t1 - t0) / 3, t0 + (t1 - t0) / 2, 50)]:
        assert a.window_json(channels, x, y, px) == b.window_json(channels, x, y, px)
        assert a.markers(channels, x, y) == b.markers(channels, x, y)


def rows(path, key=None):
    """(meta without engine and indexed_s, arrays rows, chunk rows) of every index of a store."""
    from sqlcipher3 import dbapi2 as sqlite
    c = sqlite.connect(str(path))
    if key:
        c.execute(f'PRAGMA key = "x\'{key}\'"')
    metas = {k: {n: v for n, v in json.loads(m).items() if n not in ('engine', 'indexed_s')}
             for k, m in c.execute('SELECT key, meta FROM indexes')}
    arrays = sorted(c.execute('SELECT key, array, n_values FROM arrays'))
    chunks = sorted((k, a, n, bytes(d)) for k, a, n, d in c.execute('SELECT key, array, n, data FROM chunks'))
    c.close()
    return metas, arrays, chunks


@pytest.mark.parametrize('name', RECORDINGS)
@pytest.mark.parametrize('key', [None, KEY1], ids=['plain', 'encrypted'])
def test_a_store_answers_as_a_folder_does(name, key, tmp_path):
    path = TESTDATA / name
    key2 = build_index(str(path), str(tmp_path / 'cache'))
    meta2, arrays2, folder2 = load_index(str(tmp_path / 'cache'), key2)
    # cut into small chunks, so that envelopes are read across chunk boundaries
    store = Store(tmp_path / 'index.sqlite', key, chunk_values=100)
    key3 = build_into_store(str(path), store)
    assert key3 != key2 and key3 == cache_key(path, 3)
    meta3, arrays3, folder3 = store.load(key3)
    assert strip(meta2) == strip(meta3)
    assert meta3['index_version'] == 3 and meta3['key'] == key3
    assert isinstance(arrays3['env_min_0'], Chunked)
    same_answers(Dataset(meta2, arrays2, folder2), Dataset(meta3, arrays3, folder3))
    # the same bytes as the files of version 2, in chunks of 100 values
    for name2, a in arrays2.items():
        b = arrays3[name2]
        assert np.array_equal(np.asarray(a), b[0:b.size] if isinstance(b, Chunked) else b, equal_nan=True)
    _, arrays, chunks = rows(store.path, key)
    assert all(len(d) <= 100 * 8 for *_, d in chunks)
    assert {a for _, a, _ in arrays} == {a for _, a, _, _ in chunks}
    assert os.listdir(store.tmp_dir) == []          # nothing of the build is left


def test_chunks_are_read_by_range(tmp_path):
    store = Store(tmp_path / 'index.sqlite', None, chunk_values=7)
    assert store.chunk_values == 7
    assert Store(tmp_path / 'index.sqlite').chunk_values == 7           # an existing store keeps its own
    assert Store(tmp_path / 'other.sqlite').chunk_values == CHUNK_VALUES == 4096
    key = build_into_store(str(TESTDATA / 'edf' / 'psg_plus.edf'), store)
    _, arrays, _ = store.load(key)
    a = arrays['env_min_0']
    whole = a[0:a.size]
    assert whole.size == a.size > 20
    for o in range(0, 22):
        for n in (0, 1, 6, 7, 8, 14, 15):
            if o + n <= a.size:
                assert np.array_equal(a[o:o + n], whole[o:o + n], equal_nan=True), (o, n)
    assert a[a.size - 3:a.size + 50].size == 3                          # sliced as NumPy slices
    # a chunk whose length is not what the arrays table implies fails the read
    from sqlcipher3 import dbapi2 as sqlite
    c = sqlite.connect(store.path)
    c.execute("UPDATE chunks SET data = substr(data, 1, 8) WHERE key = ? AND array = 'env_min_0.f32' AND n = 1",
              (key,))
    c.commit()
    c.close()
    _, arrays, _ = Store(store.path).load(key)
    with pytest.raises(ValueError, match='not of its length'):
        arrays['env_min_0'][0:20]
    # and a whole array that is not its length is not loaded
    c = sqlite.connect(store.path)
    c.execute("UPDATE arrays SET n_values = n_values + 1 WHERE key = ? AND array = 'vit_t.f64'", (key,))
    c.commit()
    c.close()
    assert Store(store.path).load(key) is None


def test_without_its_key_a_store_is_not_read(tmp_path):
    file = tmp_path / 'index.sqlite'
    store = Store(file, KEY1)
    key = build_into_store(str(TESTDATA / 'edf' / 'psg_plus.edf'), store)
    store.close()
    for wrong in (None, KEY2):
        with pytest.raises(ValueError, match='encrypted, or its key is wrong'):
            Store(file, wrong)
    with pytest.raises(ValueError, match='64 hex digits'):
        Store(file, 'abc')
    # the file holds no readable text of the index
    data = b''.join(p.read_bytes() for p in tmp_path.glob('index.sqlite*'))
    for text in (key.encode(), b'"pieces":', b'psg_plus', b'SQLite format 3', b'env_min_0'):
        assert text not in data, text
    assert Store(file, KEY1.upper()).load(key) is not None


def test_an_index_is_kept_replaced_and_removed_whole(tmp_path):
    from sqlcipher3 import dbapi2 as sqlite
    store = Store(tmp_path / 'index.sqlite')
    path = str(TESTDATA / 'edf' / 'psg_plus.edf')
    key = build_into_store(path, store)

    def written():
        c = sqlite.connect(store.path)
        try:
            return c.execute('SELECT written_at, engine FROM indexes WHERE key = ?', (key,)).fetchone()
        finally:
            c.close()
    before = written()
    # built again by the same engine: the index there is kept
    assert build_into_store(path, store) == key
    assert written() == before and store.keys() == [key]
    # by another engine: replaced, whole
    c = sqlite.connect(store.path)
    c.execute("UPDATE indexes SET engine = 'another' WHERE key = ?", (key,))
    c.execute("INSERT INTO chunks VALUES (?, 'stale.f32', 0, x'00000000')", (key,))
    c.execute("INSERT INTO arrays VALUES (?, 'stale.f32', 1)", (key,))
    c.commit()
    c.close()
    assert build_into_store(path, store) == key
    assert written()[1] == before[1] and written()[0] > before[0]
    _, arrays, chunks = rows(store.path)
    assert 'stale.f32' not in {a for _, a, _ in arrays} | {a for _, a, _, _ in chunks}
    assert json.loads(store.meta(key))['key'] == key and store.engine_of(key) == before[1]
    # removed: every row of the key
    assert store.remove(key) and not store.remove(key)
    assert store.load(key) is None and store.keys() == [] and rows(store.path)[1:] == ([], [])


@pytest.mark.parametrize('stop', ['cancelled', 'failure'])
def test_a_cancelled_build_leaves_nothing(stop, tmp_path):
    store = Store(tmp_path / 'index.sqlite')
    kept = build_into_store(str(TESTDATA / 'edf' / 'plain.edf'), store)

    def cancelled():
        if stop == 'failure':
            raise RuntimeError('a bug')
        return True
    with pytest.raises((ValueError, RuntimeError)):
        build_into_store(str(TESTDATA / 'edf' / 'psg_plus.edf'), store, cancelled=cancelled)
    assert store.keys() == [kept]
    assert os.listdir(store.tmp_dir) == []


@posix
def test_the_build_folder_is_private(tmp_path):
    store = Store(tmp_path / 'index.sqlite')
    seen = []

    def progress(fraction):
        seen.extend(p for p in Path(store.tmp_dir).iterdir() if p not in seen)
    build_into_store(str(TESTDATA / 'edf' / 'psg_plus.edf'), store, progress=progress)
    assert seen and stat.S_IMODE(os.stat(store.tmp_dir).st_mode) == 0o700
    assert not any(p.exists() for p in seen)


def test_keys_on_a_pipe(tmp_path):
    db_store, own = tmp_path / 'db-0001' / 'index.sqlite', tmp_path / 'viewer' / 'index.sqlite'
    rec, other = TESTDATA / 'edf' / 'psg_plus.edf', TESTDATA / 'edf' / 'plain.edf'
    build_into_store(str(rec), Store(db_store, KEY1))            # Medorama indexed one recording
    stores = Stores()

    def line(p, k, own=False):
        return json.dumps({'store': str(p), 'key': k, 'own': own})
    # read from the pipe: the lines applied (a wrong one reported without its text), and every
    # store forgotten when it closes
    errors, piped = [], Stores()
    pipe = io.StringIO('\n'.join([line(db_store, KEY1), line(own, KEY2, True), line(db_store, '09' * 32),
                                  'not json', '{"store": "x"}', '']))
    read_keys(piped, pipe, errors.append)
    assert errors == ['a store from standard input: ' + e for e in
                      (f'{db_store}: cannot be read: it is encrypted, or its key is wrong', 'not JSON', 'no key')]
    assert piped.paths() == [] and piped.own() is None
    stores.apply(line(db_store, KEY1))
    stores.apply(line(own, KEY2, True))
    with pytest.raises(ValueError):
        stores.apply(line(db_store, '09' * 32))
    assert stores.paths() == [str(db_store), str(own)]
    # found in the database's store, nothing written to the viewer's
    from fastapi.testclient import TestClient
    from vitalstream_py import paths
    from vitalstream_py.server import create_app
    from vitalstream_py.settings import Settings
    st = Settings(data_roots=[str(tmp_path), str(TESTDATA)], cache_dir=str(tmp_path / 'cache'),
                  auth_file=str(tmp_path / 'auth.json'), port=8060)
    c = TestClient(create_app(st, stores=stores), base_url='http://localhost:8060')
    try:
        r = c.post('/api/v1/open', json={'path': str(rec)}).json()
        assert r['state'] == 'ready'
        assert Store(own, KEY2).keys() == []
        # another recording: indexed into the viewer's own store, encrypted with its key
        key = c.post('/api/v1/open', json={'path': str(other)}).json()['key']
        for _ in range(400):
            d = c.get(f'/api/v1/datasets/{key}').json()
            if d['state'] != 'indexing':
                break
            time.sleep(0.025)
        assert d['state'] == 'ready', d
        assert Store(own, KEY2).keys() == [key]
        with pytest.raises(ValueError):
            Store(own)
        assert not (tmp_path / 'cache' / 'index.sqlite').exists()        # no store of its own in cache_dir
        # forgotten: the database's store is no longer read; at the end of the pipe, none is
        stores.apply(json.dumps({'forget': str(db_store)}))
        assert stores.paths() == [str(own)] and stores.holding(r['key']) is None
        read_keys(stores, io.StringIO(''), errors.append)
        assert stores.paths() == [] and stores.own() is None
    finally:
        c.close()
        paths.set_allowed_roots(None)


def test_the_own_store_and_its_key_file(tmp_path):
    from vitalstream_py.settings import load_settings
    (tmp_path / 'key.txt').write_text(KEY1 + '\n')
    f = tmp_path / 'vitalstream.toml'
    f.write_text('cache_dir = "cache"\nindex_key_file = "key.txt"\n[rs]\nport = 9\n')
    s = load_settings(str(f))
    assert s.index_key_file == str(tmp_path / 'key.txt')
    own = Stores.own_from(s).own()
    assert own.path == str(tmp_path / 'cache' / 'index.sqlite')
    with pytest.raises(ValueError, match='encrypted, or its key is wrong'):
        Store(own.path)
    f.write_text('cache_dir = "cache"\nindex_key_file = ""\n')
    assert load_settings(str(f)).index_key_file == ''
    (tmp_path / 'key.txt').write_text('not a key')
    with pytest.raises(ValueError, match='not 64 hex digits'):
        Stores.own_from(s)
    # --index writes into the own store
    from vitalstream_py.__main__ import main
    (tmp_path / 'key.txt').write_text(KEY2)
    plain = tmp_path / 'p'
    assert main(['--cache-dir', str(plain), '--index', str(TESTDATA / 'edf' / 'plain.edf')]) == 0
    assert Store(plain / 'index.sqlite').keys() == [cache_key(TESTDATA / 'edf' / 'plain.edf', 3)]


@pytest.mark.skipif(os.name == 'nt' or not FAKE_ADAPTER.is_file(), reason='the mock adapter runs on POSIX')
def test_an_adapter_reads_samples_through_a_private_folder(tmp_path):
    folder = tmp_path / 'adapters'
    folder.mkdir()
    program = folder / 'fake-adapter'
    program.write_text(f'#!{sys.executable}\n' + FAKE_ADAPTER.read_text().split('\n', 1)[1])
    program.chmod(program.stat().st_mode | stat.S_IXUSR)
    samples = np.sin(np.arange(500) / 10.0)
    rec = tmp_path / 'rec.fake'
    rec.write_text(json.dumps({'fs': 100.0, 't0': 1700000000.0, 'label': 'II', 'unit': 'mV',
                               'patient': 'Doe^Jane', 'samples': samples.tolist()}))
    try:
        adapters.set_folders([folder])
        store = Store(tmp_path / 'store' / 'index.sqlite', KEY1)
        key = build_into_store(str(rec), store)
        ds = Dataset(*store.load(key))
        read = Path(ds.folder)
        assert read.parent == Path(store.tmp_dir) and os.listdir(read) == ['meta.json']
        assert stat.S_IMODE(read.stat().st_mode) == 0o700
        assert json.loads((read / 'meta.json').read_text()) == json.loads(store.meta(key))
        assert np.array_equal(ds.samples(0, 10, 20), samples[10:20])
        ds.close()
        assert not read.exists() and os.listdir(store.tmp_dir) == []
    finally:
        adapters.set_folders([adapters.default_folder()])


# ------------------------------------------------------------ the Rust viewer
def rust_index(path, cache, key=None):
    """Index `path` with vitalstream-rs into its own store in `cache` (encrypted with `key`)."""
    cache.mkdir(parents=True, exist_ok=True)
    config = cache / 'vitalstream.toml'
    lines = [f'cache_dir = {json.dumps(str(cache))}']
    if key:
        (cache / 'key.txt').write_text(key)
        lines.append('index_key_file = "key.txt"')
    config.write_text('\n'.join(lines) + '\n')
    subprocess.run([str(RUST), '--config', str(config), '--index', str(path)], check=True, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    return cache / 'index.sqlite'


@needs_rust
@pytest.mark.parametrize('name', RECORDINGS)
@pytest.mark.parametrize('key', [None, KEY1], ids=['plain', 'encrypted'])
def test_a_store_written_by_the_rust_viewer_is_read_here(name, key, tmp_path):
    path = TESTDATA / name
    file = rust_index(path, tmp_path / 'rs', key)
    store = Store(file, key)
    k = cache_key(path, 3)
    assert store.keys() == [k]
    found = store.load(k)
    assert found is not None
    mine = Store(tmp_path / 'py' / 'index.sqlite', key)
    build_into_store(str(path), mine)
    same_answers(Dataset(*mine.load(k)), Dataset(*found))
    # the same rows: meta (apart from engine and indexed_s), array lengths, chunks byte for byte
    assert rows(file, key) == rows(mine.path, key)


def _free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


@needs_rust
@pytest.mark.parametrize('key', [None, KEY1], ids=['plain', 'encrypted'])
def test_a_store_written_here_is_read_by_the_rust_viewer(key, tmp_path):
    path = TESTDATA / 'edf' / 'psg_plus.edf'
    cache = tmp_path / 'cache'
    store = Store(cache / 'index.sqlite', key)
    k = build_into_store(str(path), store)
    ds = Dataset(*store.load(k))
    config = tmp_path / 'vitalstream.toml'
    config.write_text(f'cache_dir = {json.dumps(str(cache))}\n'
                      + (f'index_key_file = {json.dumps(str(tmp_path / "key.txt"))}\n' if key else ''))
    if key:
        (tmp_path / 'key.txt').write_text(key)
    port = _free_port()
    p = subprocess.Popen([str(RUST), '--config', str(config), '--no-auth', '--no-browser', '--port', str(port),
                          '--root', str(TESTDATA)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f'http://127.0.0.1:{port}/api/v1'
    try:
        for _ in range(200):
            try:
                urllib.request.urlopen(f'{base}/config', timeout=1)
                break
            except OSError:
                time.sleep(0.05)
        req = urllib.request.Request(f'{base}/open', data=json.dumps({'path': str(path)}).encode(),
                                     headers={'content-type': 'application/json'})
        r = json.load(urllib.request.urlopen(req, timeout=10))
        # found in the store at once: read and checked, not indexed again
        assert r == {'key': k, 'state': 'ready'}
        t0, t1 = ds.info()['t_range']
        for x, y, px in [(t0, t1, 800), (t0, t0 + 10.0, 1200)]:
            got = urllib.request.urlopen(f'{base}/datasets/{k}/window?t0={x!r}&t1={y!r}&px={px}&ch=0,1', timeout=10)
            assert json.loads(got.read()) == json.loads(ds.window_json([0, 1], x, y, px))
    finally:
        p.terminate()
        p.wait(10)
