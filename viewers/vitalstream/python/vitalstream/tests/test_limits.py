"""The rules the Rust engine and viewer added, in the Python viewer (the checks of the engine's
tests/wfdb.rs, edf.rs and roots.rs, and of the viewer's tests/viewer.rs): limits on pieces and
marker names, records and files outside the data roots, headers that state too much, the checks
on loading an index, writers that share a key, the requests the server refuses, sign-in, and
adapters held to their limits (docs/adapters.md, "Limits and environment")."""

import json
import os
import struct
import sys
import threading
import time
from pathlib import Path

import numpy as np
import pytest

from vitalstream_py import adapters, auth, common, paths
from vitalstream_py import settings as settings_module
from vitalstream_py.common import MAX_MARKER_NAMES, OTHER_MARKERS, Pieces, json_loads, untimed_spans
from vitalstream_py.dataset import Dataset
from vitalstream_py.index import build_index, load_index
from vitalstream_py.sources import edf, wfdb_folder
from vitalstream_py.sources.wfdb_files import Limits

# The tests on WFDB records are skipped in an installation without wfdb-x, which refuses them
# (test_no_wfdb.py checks that refusal).
needs_wfdb = pytest.mark.skipif(not wfdb_folder.SUPPORTED,
                                reason='WFDB records: an installation without wfdb-x refuses them')

VIEWER = Path(__file__).resolve().parents[3]                    # viewers/vitalstream
TESTDATA = VIEWER / 'testdata'
MISBEHAVING = VIEWER.parents[1] / 'libs' / 'engine' / 'tests' / 'misbehaving_adapter.py'
posix = pytest.mark.skipif(os.name == 'nt', reason='symbolic links and executable scripts on POSIX')


@pytest.fixture(autouse=True)
def default_build(monkeypatch):
    monkeypatch.setattr(settings_module, 'BUILD_ROOT', '')
    monkeypatch.setattr(auth, 'SUPPORTED', True)
    yield
    paths.set_allowed_roots(None)


def record(d, name, head, samples):
    """A single-signal record `name` in `d`: its header line `head` and 16-bit `samples`, with
    their checksum (wfdb-x leaves out a last sample that does not match it)."""
    d = Path(d)
    d.mkdir(parents=True, exist_ok=True)
    total = 0
    for v in samples:
        total = (total + v + 32768) % 65536 - 32768
    (d / f'{name}.hea').write_text(f'{head}\n{name}.dat 16 200/mV 16 0 0 {total} 0 II\n')
    (d / f'{name}.dat').write_bytes(struct.pack(f'<{len(samples)}h', *samples))


def meta_of(path, cache):
    key = build_index(str(path), str(cache))
    return load_index(str(cache), key)


def discontinuous(path, runs, spr, duration='1'):
    """An EDF+D file of `runs` data records of one second, each starting a second after the end of
    the one before: one signal of `spr` samples per record."""
    def f(s, w):
        return s.ljust(w)
    ann = 16
    h = (f('0', 8) + f('X X X X', 80) + f('Startdate 01-MAR-2024 X X X', 80) + '01.03.2413.00.00'
         + f(str(256 * 3), 8) + f('EDF+D', 44) + f(str(runs), 8) + f(duration, 8) + f('2', 4)
         + f('ECG', 16) + f('EDF Annotations', 16) + f('', 160) + f('mV', 8) + f('', 8) + f('-1', 8) * 2
         + f('1', 8) * 2 + f('-32768', 8) * 2 + f('32767', 8) * 2 + f('', 160) + f(str(spr), 8) + f(str(ann), 8)
         + f('', 64))
    b = bytearray(h.encode())
    for r in range(runs):
        b += struct.pack(f'<{spr}h', *range(spr))
        b += f'+{2 * r}\x14\x14\0'.encode().ljust(2 * ann, b'\0')
    Path(path).write_bytes(bytes(b))


# ------------------------------------------------------------- building blocks
def test_marker_names_and_pieces_are_bounded(monkeypatch):
    p = Pieces()
    piece = p.new(0, 1.0, t0=0.0)
    for k in range(MAX_MARKER_NAMES + 10):
        p.marker(piece, k, f'm{k}')
    assert p.markers_capped()
    _, arrays = p.finish()
    assert len(p.marker_names) == MAX_MARKER_NAMES and p.marker_names[-1] == OTHER_MARKERS
    assert arrays['mk_code'][MAX_MARKER_NAMES - 2] == MAX_MARKER_NAMES - 2
    assert (arrays['mk_code'][MAX_MARKER_NAMES - 1:] == 32767).all()
    p = Pieces()
    p.room(common.MAX_PIECES)
    with pytest.raises(ValueError, match='more than 100000 pieces'):
        p.room(common.MAX_PIECES + 1)


def test_untimed_spans_with_nan():
    def piece(t0, fs):
        return {'t0': t0, 'n': 10, 'fs': fs, 'timed': False}
    # a NaN rate (an index written before rates were checked) does not fail the sort
    assert untimed_spans([piece(5.0, float('nan')), piece(0.0, 1.0), piece(float('nan'), 1.0)])


def test_json_nesting_is_bounded():
    deep = lambda n: '[' * n + '1' + ']' * n           # noqa: E731
    assert json_loads(deep(128)) is not None
    with pytest.raises(ValueError, match='nested too deeply'):
        json_loads(deep(129))
    with pytest.raises(ValueError):
        json_loads('[' * 1_000_000)
    assert json_loads('{"a": "[[[[[[" }') == {'a': '[[[[[['}      # brackets in strings do not count


def test_parse_float_one_sign_and_whitespace():
    assert edf.parse_float('+nan') != edf.parse_float('+nan')    # NaN
    assert edf.parse_float('+Infinity') == float('inf')
    assert edf.parse_float('\xa02\n') == 2.0
    for bad in ('+-nan', '--inf', '-+inf', '1\x1c', '\x1f1', '0x10', '--1'):
        assert edf.parse_float(bad) is None, bad


# ------------------------------------------------------------------ WFDB rules
@needs_wfdb
def test_a_header_stating_too_much_is_read_to_the_end_of_its_data(tmp_path):
    record(tmp_path, 'r', 'r 1 250 1000000000000', [1, 2, 3, 4, 5, 6, 7, 8])
    t = time.time()
    meta, _, _ = meta_of(tmp_path / 'r.hea', tmp_path / 'cache')
    assert time.time() - t < 10
    assert meta['pieces'][0]['n'] == 8
    assert wfdb_folder.scan(tmp_path / 'r.hea', Limits())['duration_s'] == 8 / 250


@needs_wfdb
def test_reading_stops_at_the_end_of_the_data(tmp_path):
    # a format whose size is not counted (here: a header of 100 samples, 8 held, read past)
    record(tmp_path, 'r', 'r 1 250 100', [1, 2, 3, 4, 5, 6, 7, 8])
    x = wfdb_folder._read(tmp_path / 'r', 1, 0, 100)
    assert x[:, 0].tolist() == [v / 200 for v in range(1, 9)]


@needs_wfdb
def test_a_rate_that_is_not_a_number_is_refused(tmp_path):
    record(tmp_path / 'f', 'a', 'a 1 -nan 4', [1, 2, 3, 4])
    record(tmp_path / 'f', 'b', 'b 1 250 4', [1, 2, 3, 4])
    with pytest.raises(ValueError, match='no sampling frequency'):
        build_index(str(tmp_path / 'f' / 'a.hea'), str(tmp_path / 'cache'))
    meta, _, _ = meta_of(tmp_path / 'f', tmp_path / 'cache')
    assert len(meta['pieces']) == 1 and 'a' in meta['skipped']


@needs_wfdb
def test_a_collection_skips_what_it_cannot_read(tmp_path):
    c = tmp_path / 'c'
    record(c, 'good', 'good 1 250 4', [1, 2, 3, 4])
    record(tmp_path / 'elsewhere', 'far', 'far 1 250 4', [1, 2, 3, 4])
    (c / 'index.csv').write_text('name,start_unix\ngood,0\n')
    with pytest.raises(ValueError, match='no record column'):
        build_index(str(c), str(tmp_path / 'cache'))
    far = str(tmp_path / 'elsewhere' / 'far')
    (c / 'index.csv').write_text(f'record,start_unix\ngood,0\nmissing,0\n../elsewhere/far,0\n{far},0\n,0\n')
    meta, _, _ = meta_of(c, tmp_path / 'cache')
    assert len(meta['pieces']) == 1
    assert list(meta['skipped']) == ['missing', '../elsewhere/far', far, 'row 5']
    assert 'outside its folder' in meta['skipped']['../elsewhere/far']
    assert any('4 records could not be read' in n for n in meta['notes'])


@needs_wfdb
def test_many_signals_are_read_a_chunk_at_a_time(tmp_path):
    nsig, n = 300, 5000
    head = [f'w {nsig} 100 {n}']
    for k in range(nsig):
        total = 0
        for i in range(n):
            total = (total + i + k + 32768) % 65536 - 32768
        head.append(f'w.dat 16 1/mV 16 0 0 {total} 0 s{k}')
    (tmp_path / 'w.hea').write_text('\n'.join(head) + '\n')
    x = (np.arange(n)[:, None] + np.arange(nsig)[None, :]).astype('<i2')
    (tmp_path / 'w.dat').write_bytes(x.tobytes())
    assert wfdb_folder._chunk_frames(nsig) == 4096
    meta, arrays, folder = meta_of(tmp_path / 'w.hea', tmp_path / 'cache')
    ds = Dataset(meta, arrays, folder)
    assert len(ds.pieces) == nsig
    assert ds.samples(299, 4090, 4100).tolist() == [float(i + 299) for i in range(4090, 4100)]


@needs_wfdb
def test_marker_names_are_capped_in_a_record(tmp_path, monkeypatch):
    import wfdbx
    monkeypatch.setattr(common, 'MAX_MARKER_NAMES', 4)
    monkeypatch.setattr(wfdb_folder, 'MAX_MARKER_NAMES', 4)
    record(tmp_path, 'r', 'r 1 250 8', list(range(8)))
    wfdbx.write_annotations('r', 'mrk', list(range(1, 7)), symbols=['"'] * 6, aux=[f'm{k}' for k in range(6)],
                            dir=str(tmp_path))
    meta, arrays, _ = meta_of(tmp_path / 'r.hea', tmp_path / 'cache')
    assert meta['marker_names'] == ['m0', 'm1', 'm2', OTHER_MARKERS]
    assert arrays['mk_code'].tolist() == [0, 1, 2, 3, 3, 3]
    assert meta['notes'][-1] == 'More than 3 distinct marker names: the others are shown as "(other markers)".'


@posix
@needs_wfdb
def test_files_outside_the_roots_are_not_read(tmp_path):
    root, outside = tmp_path / 'root', tmp_path / 'outside'
    outside.mkdir()
    (outside / 'secret.dat').write_bytes(bytes([1, 0, 2, 0, 3, 0, 4, 0]))
    record(root, 'good', 'good 1 250 4', [1, 2, 3, 4])
    (root / 'linked.hea').write_text('linked 1 250 4\nlinked.dat 16 200/mV 16 0 0 10 0 II\n')
    (root / 'linked.dat').symlink_to(outside / 'secret.dat')
    (root / 'absolute.hea').write_text(f'absolute 1 250 4\n{outside / "secret.dat"} 16 200/mV 16 0 0 10 0 II\n')
    cache = tmp_path / 'cache'
    paths.set_allowed_roots(None)                       # a library's default: anything is read
    build_index(str(root / 'linked.hea'), str(cache))
    paths.set_allowed_roots([str(root)])
    for name in ('linked', 'absolute'):
        with pytest.raises(ValueError, match='outside the data roots'):
            build_index(str(root / f'{name}.hea'), str(cache))
    meta, arrays, folder = meta_of(root, cache)
    assert list(meta['skipped']) == ['absolute', 'linked']
    # reading samples checks again: a signal file replaced by a link afterwards is not read
    ds = Dataset(meta, arrays, folder)
    assert ds.samples(0, 0, 4).size == 4
    (root / 'good.dat').unlink()
    (root / 'good.dat').symlink_to(outside / 'secret.dat')
    with pytest.raises(ValueError, match='outside the data roots'):
        ds.samples(0, 0, 4)


# ----------------------------------------------------------------------- EDF
def test_many_breaks_are_bounded(tmp_path, monkeypatch):
    p = tmp_path / 'runs.edf'
    discontinuous(p, 3000, 4)
    meta, arrays, folder = meta_of(p, tmp_path / 'cache')
    ds = Dataset(meta, arrays, folder)
    assert len(ds.pieces) == 3000
    t0 = ds.pieces[0]['t0']
    w = json.loads(ds.window_json([0], t0, t0 + 6000.0, 100))
    assert 90 <= len(w['channels']['0']) <= 100
    w = json.loads(ds.window_json([0], t0 + 10.0, t0 + 20.0, 400))
    assert [b['piece'] for b in w['channels']['0']] == [5, 6, 7, 8, 9, 10]
    # more pieces than a recording may have: refused
    monkeypatch.setattr(common, 'MAX_PIECES', 10)
    discontinuous(tmp_path / 'too_many.edf', 11, 1)
    with pytest.raises(ValueError, match='more than 10 pieces'):
        build_index(str(tmp_path / 'too_many.edf'), str(tmp_path / 'cache'))


def test_a_rate_that_is_not_finite_is_refused(tmp_path):
    p = tmp_path / 'tiny.edf'
    discontinuous(p, 2, 4, '1e-320')
    with pytest.raises(ValueError, match='no finite sampling rate'):
        edf.header(p)
    with pytest.raises(ValueError, match='no finite sampling rate'):
        edf.header_from_bytes(p.read_bytes())
    discontinuous(p, 2, 4, '1')
    h = edf.header_from_bytes(p.read_bytes())
    assert (h.records, len(h.signals)) == (2, 2)
    for cut in (p.read_bytes()[:300], b''):
        with pytest.raises(ValueError):
            edf.header_from_bytes(cut)


# --------------------------------------------------------------------- index
@needs_wfdb
def test_writers_of_one_key_do_not_collide(tmp_path):
    cache = tmp_path / 'cache'
    keys, errors = [], []

    def one():
        try:
            keys.append(build_index(str(TESTDATA / 'session'), str(cache)))
        except Exception as e:                           # noqa: BLE001
            errors.append(e)
    threads = [threading.Thread(target=one) for _ in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors and len(set(keys)) == 1
    assert load_index(str(cache), keys[0]) is not None
    assert sorted(p.name for p in cache.iterdir() if not p.name.endswith('.progress.json')) == [keys[0]]
    # building it again keeps the complete index there
    before = (cache / keys[0] / 'meta.json').stat().st_mtime_ns
    build_index(str(TESTDATA / 'session'), str(cache))
    assert (cache / keys[0] / 'meta.json').stat().st_mtime_ns == before


@needs_wfdb
def test_a_failure_while_building_leaves_nothing(tmp_path):
    cache = tmp_path / 'cache'

    def bug():
        raise RuntimeError("a reader's bug")
    with pytest.raises(RuntimeError):
        build_index(str(TESTDATA / 'plain'), str(cache), cancelled=bug)
    assert list(cache.iterdir()) == []


@needs_wfdb
def test_an_index_that_disagrees_with_its_arrays_is_not_used(tmp_path):
    cache = tmp_path / 'cache'
    key = build_index(str(TESTDATA / 'plain'), str(cache))
    folder = cache / key
    meta = json.loads((folder / 'meta.json').read_text())

    def usable(change):
        m = json.loads(json.dumps(meta))
        change(m)
        (folder / 'meta.json').write_text(json.dumps(m))
        return load_index(str(cache), key) is not None

    def piece(k, v):
        return lambda m: m['pieces'][0].__setitem__(k, v)
    assert usable(lambda m: None)
    assert not usable(piece('env', [[0, 1_000_000]]))
    assert not usable(piece('fs', 0.0))
    assert not usable(piece('t0', None))
    assert not usable(piece('ch', 99))
    assert not usable(piece('n', -1))
    assert not usable(lambda m: m.__setitem__('vitals', [{'name': 'x', 'offset': 5, 'n': 1}]))
    assert not usable(lambda m: m.__setitem__('marker_names', []) if m['marker_names'] else m.__setitem__('pieces', 3))
    assert not usable(lambda m: m.__setitem__('index_version', 2.0))
    assert usable(lambda m: None)
    # an array of a size that is not a whole number of values, and a level missing
    (folder / 'vit_t.f64').write_bytes(bytes(3))
    assert load_index(str(cache), key) is None
    (folder / 'vit_t.f64').write_bytes(b'')
    assert load_index(str(cache), key) is not None
    (folder / 'env_max_0.f32').unlink()
    assert load_index(str(cache), key) is None


# -------------------------------------------------------------------- server
def client(tmp_path, roots=None, password=None, **more):
    from fastapi.testclient import TestClient
    from vitalstream_py.server import create_app
    from vitalstream_py.settings import Settings
    auth_file = tmp_path / 'auth.json'
    if password and not auth_file.exists():
        auth.set_password(auth_file, password)
    st = Settings(data_roots=[str(r) for r in (roots or [tmp_path, TESTDATA])], cache_dir=str(tmp_path / 'cache'),
                  auth_file=str(auth_file), port=8060, **more)
    paths.set_allowed_roots(st.data_roots)
    return TestClient(create_app(st), base_url='http://localhost:8060')


def opened(c, path):
    r = c.post('/api/v1/open', json={'path': str(path)})
    assert r.status_code == 200, r.text
    key = r.json()['key']
    for _ in range(400):
        d = c.get(f'/api/v1/datasets/{key}').json()
        if d['state'] != 'indexing':
            return key, d
        time.sleep(0.025)
    raise AssertionError('still indexing')


def test_requests_from_other_sites(tmp_path):
    c = client(tmp_path)
    for host, want in [('localhost:8060', 200), ('127.0.0.1:8060', 200), ('[::1]:8060', 200), ('LOCALHOST', 200),
                       ('evil.example:8060', 400), ('localhost.evil.example', 400), ('', 400)]:
        assert c.get('/api/v1/config', headers={'host': host}).status_code == want, host
    assert c.get('/api/v1/config', headers={'host': 'evil.example'}).json() == \
        {'detail': 'this server does not answer for that host (see allowed_hosts)'}
    proxied = client(tmp_path, allowed_hosts=['portal.example.org'])
    assert proxied.get('/api/v1/config', headers={'host': 'portal.example.org'}).status_code == 200
    assert proxied.get('/api/v1/config', headers={'host': 'other.example.org'}).status_code == 400
    body = json.dumps({'path': str(TESTDATA / 'edf' / 'plain.edf')})

    def post(ct, origin=None, url='/api/v1/open', data=body, more=None):
        h = {'content-type': ct, **({'origin': origin} if origin else {}), **(more or {})}
        return c.post(url, content=data, headers=h).status_code
    assert post('text/plain') == 415
    assert post('application/x-www-form-urlencoded') == 415
    assert post('application/json', 'http://evil.example') == 403
    assert post('application/json', 'null') == 403
    assert post('application/json', url='/api/v1/cancel', data='{"key":"0"}', more={'sec-fetch-site': 'cross-site'}) == 403
    assert post('application/json; charset=utf-8', 'http://localhost:8060') == 200
    assert post('application/json', data=body.replace('}', ', "reindex": "yes"}')) == 400
    assert post('application/json', data='{"path": 3}') == 400
    assert post('application/json', data='[' * 20000) == 400
    assert post('application/json', url='/api/v1/cancel', data=' ' * 70_000) == 413
    r = c.delete('/api/v1/config')
    assert (r.status_code, r.json()) == (405, {'detail': 'Method Not Allowed'})
    r = c.get('/')
    csp = r.headers['content-security-policy']
    assert "script-src 'self'" in csp and "frame-ancestors 'self'" in csp and 'unsafe' not in csp
    assert r.headers['x-content-type-options'] == 'nosniff'
    from vitalstream_py.server import LOGIN_PAGE
    assert '<style' not in LOGIN_PAGE and 'static/login.css' in LOGIN_PAGE
    assert c.get('/static/login.css').status_code == 200


@needs_wfdb
def test_queries_are_checked_and_bounded(tmp_path):
    c = client(tmp_path)
    key, v = opened(c, TESTDATA / 'collection')
    t0 = v['dataset']['t_range'][0]
    t1, t2 = t0 + 1.0, t0 + 2.0
    r = c.get(f'/api/v1/datasets/{key}/window?t0={t1}&t1={t2}&ch={",".join(["0"] * 3000)}')
    assert r.status_code == 200 and len(r.json()['channels']) == 1
    for q, detail in [
            (f'window?t0={t1}&t1={t2}&ch=0,99', 'no channel 99 in this dataset'),
            (f'window?t0={t1}&t1={t2}&ch=-1', 'ch must be a comma-separated list of channel ids'),
            (f'window?t0=-inf&t1={t2}&ch=0', 't0 must be a finite number'),
            (f'window?t0=nan&t1={t2}&ch=0', 't0 must be a finite number'),
            (f'window?t0=0x10&t1={t2}&ch=0', 't0 must be a finite number'),
            (f'window?t0=1_0&t1={t2}&ch=0', 't0 must be a finite number'),
            (f'window?t0=1e400&t1={t2}&ch=0', 't0 must be a finite number'),
            (f'window?t0=%20{t1}&t1={t2}&ch=0', 't0 must be a finite number'),
            (f'window?t0={t1}&t1={t2}&px=1.5&ch=0', 'px must be an integer'),
            (f'window?t0={t1}&t0={t1}&t1={t2}&ch=0', 'query parameter t0 is given more than once'),
            (f'window?t0={t2}&t1={t1}&ch=0', 't1 must be after t0'),
            (f'vitals?t0={t2}&t1={t1}&names=HR', 't1 must be after t0'),
            (f'vitals?t0={t1}&t1=inf&names=HR', 't1 must be a finite number'),
            (f'markers?t0={t1}&t1=Infinity&ch=0', 't1 must be a finite number'),
            ('vitals_at?t=-inf', 't must be a finite number'),
            ('window?t1=5', 'missing query parameter t0'),
            ('vitals?t0=0&t1=1&names=' + '|'.join(f'n{i}' for i in range(600)), 'at most 512 vital-sign series at once')]:
        r = c.get(f'/api/v1/datasets/{key}/{q}')
        assert (r.status_code, r.json()) == (400, {'detail': detail}), q
    for q in ('window?t0=-1e308&t1=1e308&ch=0,1', 'window?t0=1e300&t1=1.5e300&ch=0',
              'markers?t0=-1e308&t1=1e308&ch=0', 'vitals?t0=-1e308&t1=1e308&names=HR'):
        assert c.get(f'/api/v1/datasets/{key}/{q}').status_code == 200, q
    r = c.get(f'/api/v1/datasets/{key}/vitals?t0={t0}&t1={t2}&names=' + '|'.join(['HR'] * 2000))
    assert r.status_code == 200 and len(r.json()) == 1


@needs_wfdb
def test_a_recording_that_cannot_be_indexed_is_an_error(tmp_path):
    folder = tmp_path / 'c'
    folder.mkdir()
    (folder / 'index.csv').write_text('name\nx\n')
    c = client(tmp_path)
    _, v = opened(c, folder)
    assert v['state'] == 'error' and 'no record column' in v['error']
    assert list((tmp_path / 'cache').iterdir()) == []          # nothing left behind


def _hang_adapter(folder):
    folder.mkdir()
    program = folder / 'hang-adapter'
    program.write_text(f'#!{sys.executable}\n' + MISBEHAVING.read_text().split('\n', 1)[1])
    program.chmod(0o755)


@posix
@needs_wfdb
def test_a_worker_that_dies_is_an_error_and_the_pool_is_made_again(tmp_path):
    # a worker process killed while it indexes (here: an adapter that never answers)
    _hang_adapter(tmp_path / 'adapters')
    (tmp_path / 'x.hang').write_text('x')
    adapters.set_folders([str(tmp_path / 'adapters')])
    try:
        c = client(tmp_path, adapters_dir=str(tmp_path / 'adapters'))
        r = c.post('/api/v1/open', json={'path': str(tmp_path / 'x.hang')})
        assert r.json()['state'] == 'indexing'
        key = r.json()['key']
        pool = c.app.state.pool[0]
        for _ in range(200):
            if pool._processes:
                break
            time.sleep(0.05)
        for pid in list(pool._processes):
            os.kill(pid, 9)
        for _ in range(200):
            d = c.get(f'/api/v1/datasets/{key}').json()
            if d['state'] != 'indexing':
                break
            time.sleep(0.05)
        assert d == {'state': 'error', 'error': 'indexing stopped on an internal error'}
        # the next index is built in a new pool
        _, v = opened(c, TESTDATA / 'plain')
        assert v['state'] == 'ready'
        assert c.app.state.pool[0] is not pool
    finally:
        adapters.set_folders([adapters.default_folder()])


@posix
@needs_wfdb
def test_a_signal_file_outside_the_roots_is_refused(tmp_path):
    root, outside = tmp_path / 'root', tmp_path / 'outside'
    root.mkdir()
    outside.mkdir()
    (outside / 'secret.dat').write_bytes(bytes([1, 0, 2, 0]))
    (root / 'r.hea').write_text('r 1 250 2\nr.dat 16 200/mV 16 0 0 3 0 II\n')
    (root / 'r.dat').symlink_to(outside / 'secret.dat')
    c = client(tmp_path, roots=[root])
    r = c.post('/api/v1/open', json={'path': str(root / 'r.hea')})
    assert (r.status_code, r.json()) == (403, {'detail': 'r.dat, a file of this record, lies outside the data roots'})
    assert 'outside/' not in r.text


def test_sign_in(tmp_path):
    c = client(tmp_path, password='correct horse')
    assert c.post('/login', content='x' * 5000).status_code == 413
    r = c.post('/login', data={'password': 'correct horse'}, follow_redirects=False)
    assert r.status_code == 303 and 'secure' not in r.headers['set-cookie'].lower()
    value = c.cookies.get('vitalstream_8060')
    assert c.get('/api/v1/config').status_code == 200
    # a cookie of characters other than ASCII is a wrong one, never an error
    bad = f'{value.split(".")[0]}.\u00e9\u00e9'
    c2 = client(tmp_path, password='correct horse')
    assert c2.get('/api/v1/config', headers=[(b'cookie', f'vitalstream_8060={bad}'.encode('latin-1'))]).status_code == 401
    assert not auth.cookie_ok(auth.load(tmp_path / 'auth.json'), bad)
    # logout ends the sign-in on the server: a copy of the cookie no longer works, even after
    # a restart (kept in the auth file)
    r = c.post('/logout', follow_redirects=False)
    assert r.status_code == 303 and 'Max-Age=0' in r.headers['set-cookie']
    for server in (c, client(tmp_path, password='correct horse')):
        server.cookies.set('vitalstream_8060', value)
        assert server.get('/api/v1/config').status_code == 401
    assert auth.load(tmp_path / 'auth.json') is not None, 'the password is kept'
    assert list(auth.revoked(tmp_path / 'auth.json')) == [value.split('.', 1)[1]]
    # behind HTTPS: the cookie is marked Secure
    s = client(tmp_path, password='correct horse', secure_cookie=True)
    r = s.post('/login', data={'password': 'correct horse'}, follow_redirects=False)
    assert r.status_code == 303 and '; secure' in r.headers['set-cookie'].lower()


def test_wrong_passwords_slow_down_their_sender(tmp_path):
    c = client(tmp_path, password='correct horse')
    delay, failed = c.app.state.login_delay, c.app.state.login_failed
    got = []
    for _ in range(8):
        got.append(delay('10.0.0.1'))
        failed('10.0.0.1')
    assert got == [0, 1, 3, 7, 15, 30, 30, 30]
    assert delay('10.0.0.2') == 0, 'per address'


def test_settings_files(tmp_path, monkeypatch):
    from vitalstream_py.settings import load_settings
    toml = tmp_path / 'vitalstream.toml'
    toml.write_text('adapters_dir = "ad"\nhost = "0.0.0.0"\n')
    monkeypatch.chdir(tmp_path)
    s = load_settings()
    assert s.source != str(toml.resolve()) and s.adapters_dir != str(tmp_path / 'ad')
    s = load_settings(str(toml))
    assert (s.adapters_dir, s.host, s.source) == (str(tmp_path.resolve() / 'ad'), '0.0.0.0', str(toml.resolve()))
    for bad in ('data_roots = "/"', 'port = "8060"', 'port = 70000', 'index_workers = -1', 'allowed_hosts = "x"',
                'allowed_hosts = [" "]', 'qc_verbose = 1', 'secure_cookie = "yes"', 'timezone = 3', 'port = true'):
        toml.write_text(bad)
        with pytest.raises(ValueError, match='bad value'):
            load_settings(str(toml))
    toml.write_text('allowed_hosts = ["portal.example.org"]\nsecure_cookie = true\n')
    s = load_settings(str(toml))
    assert (s.allowed_hosts, s.secure_cookie) == (['portal.example.org'], True)


# ------------------------------------------------------------------ adapters
def _misbehaving(folder, mode):
    folder.mkdir(exist_ok=True)
    program = folder / f'{mode}-adapter'
    program.write_text(f'#!{sys.executable}\n' + MISBEHAVING.read_text().split('\n', 1)[1])
    program.chmod(0o755)
    return adapters.Adapter(mode, '1', str(program), [])


@posix
def test_adapters_are_held_to_their_limits(tmp_path, monkeypatch):
    out = tmp_path / 'out'
    out.mkdir()
    src = tmp_path / 'x.noisy'
    src.write_text('fail')
    # much on standard error never blocks it; only its last line is shown
    noisy = _misbehaving(tmp_path / 'a', 'noisy')
    with pytest.raises(RuntimeError) as e:
        adapters.index(noisy, src, out, lambda f: None)
    assert str(e.value).endswith(': the last words')
    src.write_text('ok')
    adapters.index(noisy, src, out, lambda f: None)
    # an adapter that never answers is stopped
    monkeypatch.setattr(adapters, '_timeouts', adapters.Timeouts(describe=1.0, index_stall=1.0, serve_stall=1.0))
    hang = _misbehaving(tmp_path / 'a', 'hang')
    t = time.monotonic()
    with pytest.raises(RuntimeError, match='no answer for 1 s: stopped'):
        adapters.index(hang, src, out, lambda f: None)
    assert time.monotonic() - t < 10
    with pytest.raises(RuntimeError, match='did not answer within 1 s'):
        adapters._describe(_misbehaving(tmp_path / 'a', 'hangdescribe').program)
    # a cancelled index stops the adapter at once
    t = time.monotonic()
    monkeypatch.setattr(adapters, '_timeouts', adapters.Timeouts())
    with pytest.raises(ValueError, match='cancelled'):
        adapters.index(hang, src, out, lambda f: None, cancelled=lambda: time.monotonic() - t > 0.3)
    assert time.monotonic() - t < 5


@posix
def test_a_session_out_of_step_is_stopped(tmp_path, monkeypatch):
    monkeypatch.setattr(adapters, '_timeouts', adapters.Timeouts(serve_stall=1.0))
    liar = _misbehaving(tmp_path / 'a', 'liar')
    want = [float(i) for i in range(10, 20)]
    assert adapters.read(liar, tmp_path, 0, 10, 20).tolist() == want
    # more samples than asked, an absurd count, no JSON: errors, and the session is stopped
    for piece in (1, 2, 4):
        with pytest.raises(RuntimeError, match='its session was stopped'):
            adapters.read(liar, tmp_path, piece, 10, 20)
        assert adapters.read(liar, tmp_path, 0, 10, 20).tolist() == want
    # an error answered: the session stays in step
    session = adapters._sessions['liar'][0]
    with pytest.raises(RuntimeError, match='^no such piece$'):
        adapters.read(liar, tmp_path, 3, 0, 5)
    assert adapters._sessions['liar'][0] is session
    # no answer: stopped after the stall time
    with pytest.raises(RuntimeError, match='no answer for 1 s'):
        adapters.read(liar, tmp_path, 5, 0, 5)
    assert adapters.read(liar, tmp_path, 0, 10, 20).tolist() == want
    # a long read is asked in requests of at most READ_CHUNK samples
    monkeypatch.setattr(adapters, 'READ_CHUNK', 7)
    assert adapters.read(liar, tmp_path, 0, 0, 23).tolist() == [float(i) for i in range(23)]
    adapters._sessions.pop('liar')[0].close()


@posix
def test_an_adapter_gets_a_cleared_environment(tmp_path, monkeypatch):
    monkeypatch.setenv('MEDORAMA_KEY', 'secret')
    env = _misbehaving(tmp_path / 'a', 'env')
    out = adapters._run(env.program, ['identify', 'x'], 10)
    d = json.loads(out)
    assert d['identifiers'] == {'full_name': '', 'patient_id': 'PATH'}
