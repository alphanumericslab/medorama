"""VitalStream (Python) tests: the building blocks, the synthetic recordings of testdata/
indexed and queried, WFDB records (the checks of the Rust engine's tests/wfdb.rs, with the same
values), the adapter protocol with the mock adapter of the engine's tests, the viewer's root, the
file browser, scan, open and cancel, and sign-in.  Parity with the other viewers is checked by
tests/parity/."""

import json
import math
import os
import stat
import sys
import time
from pathlib import Path

import numpy as np
import pytest

from vitalstream_py import adapters
from vitalstream_py import settings as settings_module
from vitalstream_py.common import LEVEL0, Envelope
from vitalstream_py.dataset import Dataset
from vitalstream_py.index import build_index, cache_key, load_index
from vitalstream_py.sources import kind_of, wfdb_files, wfdb_folder
from vitalstream_py.sources.wfdb_files import Limits

# The tests on WFDB records are skipped in an installation without wfdb-x, which refuses them
# (test_no_wfdb.py checks that refusal).
needs_wfdb = pytest.mark.skipif(not wfdb_folder.SUPPORTED,
                                reason='WFDB records: an installation without wfdb-x refuses them')

VIEWER = Path(__file__).resolve().parents[3]                    # viewers/vitalstream
TESTDATA = VIEWER / 'testdata'
FAKE_ADAPTER = VIEWER.parents[1] / 'libs' / 'engine' / 'tests' / 'fake_adapter.py'
T0 = 1709287200.0


@pytest.fixture(autouse=True)
def default_build(monkeypatch):
    """The options of a build without VITALSTREAM_ROOT and VITALSTREAM_PASSWORD, whatever the
    installed package was built with: the tests use temporary folders anywhere (the viewer's root
    is the whole file system), and the password is supported."""
    from vitalstream_py import auth
    from vitalstream_py import paths
    monkeypatch.setattr(settings_module, 'BUILD_ROOT', '')
    monkeypatch.setattr(auth, 'SUPPORTED', True)
    yield
    paths.set_allowed_roots(None)          # main() sets the process's data roots


# --------------------------------------------------------------------- unit
def test_envelope_levels_cover_all_samples():
    rng = np.random.default_rng(0)
    x = rng.normal(size=10_007).astype(np.float32)
    x[100:140] = np.nan                       # whole buckets NaN -> NaN, partial -> ignored
    env = Envelope()
    for a in range(0, x.size, 999):
        env.add(x[a:a + 999])
    levels = env.finish()
    lo0, hi0 = levels[0]
    assert lo0.size == math.ceil(x.size / LEVEL0)
    for i in range(lo0.size):
        b = x[i * LEVEL0:(i + 1) * LEVEL0]
        if np.isnan(b).all():
            assert np.isnan(lo0[i]) and np.isnan(hi0[i])
        else:
            assert lo0[i] == np.nanmin(b) and hi0[i] == np.nanmax(b)
    assert levels[-1][0].size == 1
    assert levels[-1][0][0] == np.nanmin(x) and levels[-1][1][0] == np.nanmax(x)


# ----------------------------------------------------------------- recordings
def _open(path, cache):
    build_index(str(path), str(cache))
    return Dataset(*load_index(str(cache), cache_key(path)))


@pytest.mark.parametrize('name', ['collection', 'plain'])
@needs_wfdb
def test_index_and_query_every_recording(name, tmp_path):
    ds = _open(TESTDATA / name, tmp_path)
    info = ds.info()
    t0, t1 = info['t_range']
    chs = list(range(len(info['channels'])))
    for a, b in ((t0, t1), (t0, t0 + 2.0), (t0 + 1.0, t0 + 1.2)):
        assert len(json.loads(ds.window_json(chs, a, b, 800))['channels']) == len(chs)
    for p in ds.pieces:
        assert ds.samples(p['id'], 0, p['n']).size == p['n']
    names = [v['name'] for v in info['vitals']]
    ds.vitals_window(names, t0, t1, 100)
    ds.vitals_at((t0 + t1) / 2)
    ds.markers(chs, t0, t1)
    assert info['format'] == ('WFDB collection' if name == 'collection' else 'WFDB records')
    if name == 'plain':
        # nothing assumed that a header does not state: ADC units for an uncalibrated signal, no
        # unit when none is stated, and the record without a sampling frequency left out
        assert sorted(c['unit'] for c in info['channels']) == ['', 'adu', 'mV', 'mmHg']
    if name == 'collection':
        # three stretches; the last, without a start time, after the 0.5 s break stated before it
        assert [p['t0'] for p in ds.pieces if p['ch'] == 0] == [1709287200.0, 1709287205.5, 1709287209.0]
        assert names == ['HR', 'SpO2', 'HR [import]']     # the invalid row left out; a second source named
        assert len(info['alarms']) == 2 and info['alarms'][0]['text'] == 'HR HIGH 151 > 150'
        assert len(info['gaps']) == 2
        marks = ds.markers([0], t0, t1)
        assert {'R wave', 'V'} <= set(marks['names'])


@needs_wfdb
def test_window_json_raw_and_envelope(tmp_path):
    ds = _open(TESTDATA / 'collection', tmp_path)
    p = max(ds.pieces, key=lambda q: q['n'])
    t0 = p['t0'] + 1.0
    # zoomed in: raw samples at their times
    w = json.loads(ds.window_json([p['ch']], t0, t0 + 2.0, 1000))
    blk = [b for b in w['channels'][str(p['ch'])] if b['piece'] == p['id']][0]
    assert blk['kind'] == 'raw' and blk['dt'] == pytest.approx(1 / p['fs'])
    a = round((blk['t0'] - p['t0']) * p['fs'])
    x = ds.samples(p['id'], a, a + len(blk['y']))
    assert np.allclose([np.nan if v is None else v for v in blk['y']], x, equal_nan=True)
    # zoomed out: envelope buckets bound the samples
    w = json.loads(ds.window_json([p['ch']], p['t0'], p['t0'] + p['n'] / p['fs'], 16))
    blk = [b for b in w['channels'][str(p['ch'])] if b['piece'] == p['id']][0]
    assert blk['kind'] == 'env'
    x = ds.samples(p['id'], 0, p['n'])
    step = round(blk['dt'] * p['fs'])
    i0 = round((blk['t0'] - p['t0']) * p['fs']) // step
    for i, (lo, hi) in enumerate(zip(blk['min'], blk['max'])):
        seg = x[(i0 + i) * step:(i0 + i + 1) * step]
        if lo is not None:
            assert lo == pytest.approx(np.nanmin(seg), rel=1e-6, abs=1e-6)
            assert hi == pytest.approx(np.nanmax(seg), rel=1e-6, abs=1e-6)


# ------------------------------------------------------------ WFDB records
def _meta(path, cache):
    return load_index(str(cache), build_index(str(path), str(cache)))[0]


@needs_wfdb
def test_a_folder_of_records_is_one_session(tmp_path):
    m = _meta(TESTDATA / 'session', tmp_path)
    assert m['format'] == 'WFDB records'
    assert [c['label'] for c in m['channels']] == ['ART', 'II', 'II (2)', 'V']
    pieces = m['pieces']
    # subfolders in name order; the segments of the multi-segment records, never on their own;
    # the hidden folder left out
    assert [p['record'] for p in pieces] == ['abp/abp_a', 'ecg/hr_a', 'ecg/hr_b', 'ecg/hr_c', 'ms/ms_fix_0001',
                                             'ms/ms_fix_0002', 'ms/ms_rec_0001', 'ms/ms_rec_0001', 'ms/ms_rec_0002']
    assert [p['t0'] - T0 for p in pieces] == [7200.0, 7200.0, 7201.0, 7204.0, 7205.0, 7206.0, 7210.0, 7210.0, 7212.5]
    assert pieces[2]['ch'] == 2                   # the overlapping record is in a second lane
    # without a start time: placed after the data before them
    assert [p['timed'] for p in pieces] == [True, True, True, False, False, False, True, True, True]
    gaps = m['gaps']
    assert len(gaps) == 1
    assert (gaps[0]['reason'], gaps[0]['t0'] - T0, gaps[0]['t1'] - T0) == ('null segment', 7212.0, 7212.5)
    assert any('second lane' in n for n in m['notes'])


@needs_wfdb
def test_one_record(tmp_path):
    ms = TESTDATA / 'session' / 'ms'
    assert kind_of(ms / 'ms_rec.hea') == 'wfdb'
    assert wfdb_folder.format_label(ms / 'ms_rec.hea') == 'WFDB multi-segment record'
    assert wfdb_folder.format_label(ms / 'ms_rec_0001.hea') == 'WFDB record'
    # a multi-segment record: its segments from its start, the null segment a break, the markers
    # of the record in the segments they fall in (the one in the null segment left out)
    m = _meta(ms / 'ms_rec.hea', tmp_path)
    assert m['format'] == 'WFDB multi-segment record'
    assert [(p['record'], p['n'], p['t0'] - T0) for p in m['pieces']] == \
        [('ms_rec_0001', 500, 7210.0), ('ms_rec_0001', 500, 7210.0), ('ms_rec_0002', 250, 7212.5)]
    _, arrays, _ = load_index(str(tmp_path), cache_key(ms / 'ms_rec.hea'))
    assert arrays['mk_piece'].tolist() == [0, 1, 2]
    assert arrays['mk_sample'].tolist() == [100, 200, 75]
    # a segment alone: only that record, and its header states no start time
    m = _meta(ms / 'ms_rec_0001.hea', tmp_path)
    assert len(m['pieces']) == 2 and all(p['timed'] is False for p in m['pieces'])
    # a record of a folder: that record only
    m = _meta(TESTDATA / 'plain' / 'rec_a.hea', tmp_path)
    assert [c['label'] for c in m['channels']] == ['II', 'ART']


@needs_wfdb
def test_the_limits_of_a_folder(tmp_path):
    session = TESTDATA / 'session'
    scan = wfdb_folder.scan(session, Limits())
    assert list(scan) == ['path', 'format', 'records', 'segments', 'signals', 'duration_s', 't_range', 'untimed',
                          'bytes', 'skipped', 'scan_s']
    assert (scan['records'], scan['segments'], scan['untimed']) == (6, 4, 2)
    assert scan['signals'] == ['ART', 'II', 'V']
    assert (scan['t_range'][0] - T0, scan['t_range'][1] - T0) == (7200.0, 7213.5)
    # over a limit: refused, with what to do instead (header files are counted, segments too)
    with pytest.raises(ValueError) as e:
        wfdb_folder.scan(session, Limits(max_records=5))
    assert 'more than 5 WFDB records' in str(e.value) and 'index.csv' in str(e.value)
    with pytest.raises(ValueError, match='nested more than 0 levels deep'):
        wfdb_folder.scan(session, Limits(max_depth=0))
    with pytest.raises(ValueError, match='took more than 0 s'):
        wfdb_folder.scan(session, Limits(max_scan_s=0.0))
    # a folder without records
    (tmp_path / 'notes.txt').write_text('no records')
    with pytest.raises(ValueError, match='no WFDB records'):
        wfdb_folder.scan(tmp_path, Limits())


@needs_wfdb
def test_the_key_of_a_record_follows_its_files(tmp_path):
    import shutil
    ms = tmp_path / 'ms'
    shutil.copytree(TESTDATA / 'session' / 'ms', ms)
    rec = ms / 'ms_rec.hea'
    k0 = cache_key(rec)
    assert [f[0] for f in wfdb_folder.fingerprint(rec)] == \
        ['ms_rec.hea', 'ms_rec.mrk', 'ms_rec_0001.dat', 'ms_rec_0001.hea', 'ms_rec_0002.dat', 'ms_rec_0002.hea',
         'ms_rec_layout.hea']
    (ms / 'ms_fix_0001.dat').write_bytes(bytes(4))        # another record of the folder: not this key
    assert cache_key(rec) == k0
    (ms / 'ms_rec_0002.dat').write_bytes(bytes(4))        # one of its segments: a new key
    assert cache_key(rec) != k0


def test_the_key_of_a_folder_skips_hidden_entries_and_follows_linked_files(tmp_path):
    import shutil
    folder = tmp_path / 'plain'
    shutil.copytree(TESTDATA / 'plain', folder)
    k0 = cache_key(folder)
    (folder / '.notes').write_text('x')
    (folder / 'up').symlink_to(tmp_path)                  # a link to a folder: not counted
    (folder / 'broken').symlink_to(tmp_path / 'nothing')  # nor a broken link
    assert cache_key(folder) == k0
    (folder / 'notes.txt').write_text('x')
    assert cache_key(folder) != k0
    # a link to a file counts with its target's size and time (wfdb-x reads through it)
    target = tmp_path / 'target.dat'
    target.write_bytes(bytes(8))
    (folder / 'b.dat').symlink_to(target)
    assert 'b.dat' in [f[0] for f in wfdb_folder.fingerprint(folder)]
    k1 = cache_key(folder)
    target.write_bytes(bytes(10))
    assert cache_key(folder) != k1


def test_the_key_of_a_folder_is_bounded(tmp_path, monkeypatch):
    # subfolders down to max_depth levels; at most MAX_FINGERPRINT_FILES files, in name order
    deep = tmp_path / 'f' / 'a' / 'b'
    deep.mkdir(parents=True)
    (tmp_path / 'f' / 'top.txt').write_text('x')
    (tmp_path / 'f' / 'a' / 'mid.txt').write_text('x')
    (deep / 'low.txt').write_text('x')
    wfdb_folder.set_limits(Limits(max_depth=1))
    try:
        assert [f[0] for f in wfdb_folder.fingerprint(tmp_path / 'f')] == ['a/mid.txt', 'top.txt']
    finally:
        wfdb_folder.set_limits(Limits())
    monkeypatch.setattr(wfdb_files, 'MAX_FINGERPRINT_FILES', 2)
    assert [f[0] for f in wfdb_folder.fingerprint(tmp_path / 'f')] == ['a/b/low.txt', 'a/mid.txt']


@needs_wfdb
def test_cancelling_leaves_nothing(tmp_path):
    cache = tmp_path / 'cache'
    with pytest.raises(ValueError, match='cancelled'):
        build_index(str(TESTDATA / 'session'), str(cache), cancelled=lambda: True)
    assert list(cache.iterdir()) == []


@pytest.mark.skipif(os.name == 'nt' or not FAKE_ADAPTER.is_file(), reason='the mock adapter runs on POSIX')
def test_adapter_end_to_end(tmp_path):
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
        adapters.set_folders([tmp_path / 'none'])     # without the adapter: a file of an unknown format
        assert kind_of(rec) is None
        with pytest.raises(ValueError):
            build_index(str(rec), str(tmp_path / 'cache'))
        adapters.set_folders([folder])
        found = adapters.installed()
        assert [(a.name, a.formats[0].label) for a in found] == [('fake', 'Fake recording')]
        assert kind_of(rec) == 'fake'
        ds = _open(rec, tmp_path / 'cache')
        assert ds.meta['adapter'] == 'fake' and ds.meta['kind'] == 'fake'
        # samples read on demand through the adapter's session, twice (the session is kept)
        assert np.array_equal(ds.samples(0, 10, 20), samples[10:20])
        assert np.array_equal(ds.samples(0, 490, 500), samples[490:500])
        assert json.loads(ds.window_json([0], 1700000000.0, 1700000001.0, 400))['channels']['0'][0]['kind'] == 'raw'
        # cancelled at the adapter's first progress line: nothing left behind
        with pytest.raises(ValueError, match='cancelled'):
            build_index(str(rec), str(tmp_path / 'cache2'), cancelled=lambda: True)
        assert list((tmp_path / 'cache2').iterdir()) == []
    finally:
        adapters.set_folders([adapters.default_folder()])


# ------------------------------------------------------------- file browser
def _client(tmp_path, roots, password=None, **more):
    from fastapi.testclient import TestClient
    from vitalstream_py import auth
    from vitalstream_py.server import create_app
    from vitalstream_py.settings import Settings
    auth_file = tmp_path / 'auth.json'
    if password:
        auth.set_password(auth_file, password)
    st = Settings(data_roots=[str(r) for r in roots], cache_dir=str(tmp_path / 'cache'),
                  auth_file=str(auth_file), port=9, **more)
    return TestClient(create_app(st), base_url='http://localhost:8060'), st


def test_password(tmp_path):
    from fastapi.testclient import TestClient
    from vitalstream_py.server import create_app
    c, st = _client(tmp_path, [tmp_path])
    assert c.get('/api/v1/config').status_code == 200            # no password set: no sign-in
    c, st = _client(tmp_path, [tmp_path], password='correct horse')
    assert oct((tmp_path / 'auth.json').stat().st_mode & 0o777) == '0o600'
    assert c.get('/api/v1/config').status_code == 401
    assert c.get('/').status_code == 401 and 'Password' in c.get('/').text
    assert c.post('/login', data={'password': 'wrong'}).status_code == 401
    r = c.post('/login', data={'password': 'correct horse'}, follow_redirects=False)
    assert r.status_code == 303 and r.headers['location'] == './'
    assert c.get('/api/v1/config').status_code == 200
    assert c.get('/').status_code == 200 and 'static/js/app.js' in c.get('/').text
    # the login survives a server restart (same auth file)
    c2 = TestClient(create_app(st), base_url='http://localhost:8060')
    c2.cookies = c.cookies
    assert c2.get('/api/v1/config').status_code == 200
    # a new password signs everyone out
    from vitalstream_py import auth
    auth.set_password(st.auth_file, 'another one')
    c3 = TestClient(create_app(st), base_url='http://localhost:8060')
    c3.cookies = c.cookies
    assert c3.get('/api/v1/config').status_code == 401


def test_the_viewers_root(tmp_path, monkeypatch):
    from vitalstream_py.settings import FS_ROOT, HOME, Settings, absolute
    # built without a root: the whole file system
    st = Settings()
    assert st.data_roots == [FS_ROOT]
    assert st.allowed(HOME) and st.allowed('/') and st.allowed('/tmp')
    # built with a root: the default data root; a wider one is narrowed to it, one outside it is
    # left out, and none inside it is refused
    top, outside = tmp_path / 'top', tmp_path / 'outside'
    (top / 'a').mkdir(parents=True)
    outside.mkdir()
    monkeypatch.setattr(settings_module, 'BUILD_ROOT', str(top))
    assert Settings().data_roots == [str(top)]
    st = Settings(data_roots=[FS_ROOT, str(top / 'a')])
    assert st.data_roots == [str(top), str(top / 'a')]
    st = Settings(data_roots=[str(outside), str(top)])
    assert (st.data_roots, st.outside) == ([str(top)], [str(outside)])
    assert st.allowed(top / 'a') and not st.allowed(outside)
    with pytest.raises(ValueError, match="no data root inside this viewer's root"):
        Settings(data_roots=[str(outside)])
    # '~' is the home folder of whoever runs the viewer
    monkeypatch.setattr(settings_module, 'BUILD_ROOT', '~')
    assert Settings().data_roots == [absolute(HOME)]


def test_settings_file_keys(tmp_path):
    from vitalstream_py.settings import load_settings
    f = tmp_path / 'vitalstream.toml'
    f.write_text('max_records = 7\nmax_depth = 3\nmax_scan_s = 2\nrequire_password = true\n[py]\nmax_depth = 4\n')
    st = load_settings(str(f))
    assert (st.limits, st.require_password) == (Limits(7, 4, 2.0), True)
    for bad in ('max_records = -1', 'max_records = 1.5', 'max_scan_s = -1', 'max_depth = true',
                'require_password = 1'):
        f.write_text(bad + '\n')
        with pytest.raises(ValueError, match='bad value for'):
            load_settings(str(f))
    with pytest.raises(FileNotFoundError, match='no such settings file'):
        load_settings(str(tmp_path / 'none.toml'))


def test_browse_symlinks_typed_paths_and_completion(tmp_path):
    root, outside = tmp_path / 'root', tmp_path / 'outside'
    (root / 'real' / 'sub').mkdir(parents=True)
    (outside / 'data').mkdir(parents=True)
    (outside / 'data' / 'notes.txt').write_bytes(b'x')
    (root / 'link_in').symlink_to(root / 'real')
    (root / 'link_out').symlink_to(outside)
    (root / 'dangling').symlink_to(tmp_path / 'nothing')
    c, _ = _client(tmp_path, [root])
    # default listing: the root (home is outside it here)
    r = c.get('/api/v1/browse').json()
    names = {e['name']: e for e in r['entries']}
    # a link that leads outside the root is not listed; one inside it and a dangling one are
    assert 'link_out' not in names
    assert names['link_in']['link'] and names['dangling']['type'] == 'broken'
    # a link inside the root that leads inside it is followed; the path stays as navigated
    (root / 'real' / 'sub' / 'notes.txt').write_bytes(b'x')
    r = c.get('/api/v1/browse', params={'path': str(root / 'link_in' / 'sub')}).json()
    assert r['path'] == os.path.normpath(str(root / 'link_in' / 'sub'))
    assert r['parent'] == os.path.normpath(str(root / 'link_in'))
    # a file no installed reader reads is not listed
    assert r['entries'] == []
    # and a typed path to it is refused
    r = c.get('/api/v1/browse', params={'path': str(root / 'link_in' / 'sub' / 'notes.txt')})
    assert r.status_code == 400 and 'not a folder or a recording' in r.json()['detail']
    # a link inside the root that leads outside it is refused, as are paths outside the roots
    assert c.get('/api/v1/browse', params={'path': str(root / 'link_out' / 'data')}).status_code == 403
    assert c.get('/api/v1/browse', params={'path': str(root / 'link_out')}).status_code == 403
    assert c.get('/api/v1/browse', params={'path': str(outside)}).status_code == 403
    assert c.get('/api/v1/browse', params={'path': str(root / '..' / 'outside')}).status_code == 403
    # completion
    items = c.get('/api/v1/complete', params={'prefix': str(root / 'li')}).json()['items']
    assert [i['path'] for i in items] == [str(root / 'link_in') + os.sep]
    items = c.get('/api/v1/complete', params={'prefix': str(root / 'link_out' / 'data') + os.sep}).json()['items']
    assert items == []


@needs_wfdb
def test_records_scan_open_and_cancel(tmp_path):
    c, st = _client(tmp_path, [TESTDATA], max_records=5)
    assert c.get('/api/v1/config').json()['limits'] == {'max_records': 5, 'max_depth': 20, 'max_scan_s': 10.0}
    ms = TESTDATA / 'session' / 'ms'
    # records listed by name; the segments and layout headers of the multi-segment records only
    # with the hidden entries
    r = c.get('/api/v1/browse', params={'path': str(ms)}).json()
    assert [(e['name'], e['type'], e['label']) for e in r['entries']] == \
        [('ms_fix', 'record', 'WFDB multi-segment record'), ('ms_rec', 'record', 'WFDB multi-segment record')]
    r = c.get('/api/v1/browse', params={'path': str(ms), 'hidden': 'true'}).json()
    assert [e['name'] for e in r['entries']] == ['ms_fix', 'ms_fix_0001', 'ms_fix_0002', 'ms_rec', 'ms_rec_0001',
                                                 'ms_rec_0002', 'ms_rec_layout']
    # a record's path: a recording file, which the page opens
    assert c.get('/api/v1/browse', params={'path': str(ms / 'ms_rec.hea')}).json() == \
        {'path': str(ms / 'ms_rec.hea'), 'type': 'file'}
    # scan: one record; a folder over a limit; something else
    r = c.get('/api/v1/scan', params={'path': str(ms / 'ms_rec.hea')}).json()
    assert (r['format'], r['records'], r['segments'], r['signals']) == ('WFDB multi-segment record', 1, 2, ['II', 'V'])
    r = c.get('/api/v1/scan', params={'path': str(TESTDATA / 'session')})
    assert r.status_code == 422 and 'more than 5 WFDB records' in r.json()['detail']
    r = c.get('/api/v1/scan', params={'path': str(TESTDATA / 'plain' / 'rec_a.dat')})
    assert r.status_code == 400 and 'not a folder or a WFDB record' in r.json()['detail']
    # open: a folder over a limit is refused before anything is read; a record opens
    r = c.post('/api/v1/open', json={'path': str(TESTDATA / 'session')})
    assert r.status_code == 422 and 'more than 5 WFDB records' in r.json()['detail']
    key = c.post('/api/v1/open', json={'path': str(ms / 'ms_rec.hea')}).json()['key']
    for _ in range(600):
        d = c.get(f'/api/v1/datasets/{key}').json()
        if d['state'] != 'indexing':
            break
        time.sleep(0.05)
    assert d['state'] == 'ready' and d['dataset']['format'] == 'WFDB multi-segment record'
    # cancel: nothing being built under the key
    r = c.post('/api/v1/cancel', json={'key': key})
    assert r.status_code == 404 and r.json()['detail'] == 'no index being built under this key'


@needs_wfdb
def test_cancel_an_index_being_built(tmp_path):
    """Cancelling through the API: the job stops, its state is 'cancelled', and nothing is left
    in the cache."""
    import threading
    from vitalstream_py import server
    started, release = threading.Event(), threading.Event()

    class SlowPool:
        """A pool that runs the job in a thread and holds it until it is released."""
        def __init__(self, *a, **k):
            pass

        def submit(self, fn, *args):
            from concurrent.futures import Future
            fut = Future()

            def run():
                started.set()
                release.wait(10)
                try:
                    fut.set_result(fn(*args))
                except Exception as e:
                    fut.set_exception(e)
            threading.Thread(target=run, daemon=True).start()
            return fut

        def shutdown(self, **k):
            pass

    real = server.ProcessPoolExecutor
    server.ProcessPoolExecutor = SlowPool
    try:
        c, st = _client(tmp_path, [TESTDATA])
    finally:
        server.ProcessPoolExecutor = real
    key = c.post('/api/v1/open', json={'path': str(TESTDATA / 'session')}).json()['key']
    assert started.wait(10)
    assert c.post('/api/v1/cancel', json={'key': key}).json() == {'key': key, 'state': 'cancelling'}
    release.set()
    for _ in range(600):
        d = c.get(f'/api/v1/datasets/{key}').json()
        if d['state'] != 'indexing':
            break
        time.sleep(0.05)
    assert d == {'state': 'cancelled'}
    assert c.post('/api/v1/cancel', json={'key': key}).json() == {'key': key, 'state': 'cancelled'}
    assert list(Path(st.cache_dir).iterdir()) == []


def test_version(capsys):
    """-V and --version print the program and its version, as vitalstream-rs and vitalstream-cpp do."""
    from vitalstream_py import __version__
    from vitalstream_py.__main__ import main
    for flag in ('-V', '--version'):
        with pytest.raises(SystemExit) as e:
            main([flag])
        assert e.value.code == 0
        assert capsys.readouterr().out.strip() == f'vitalstream-py {__version__}'


def test_build_without_password_support(tmp_path, monkeypatch, capsys):
    from vitalstream_py import auth
    from vitalstream_py.__main__ import main
    monkeypatch.setattr(auth, 'SUPPORTED', False)
    common = ['--cache-dir', str(tmp_path / 'cache'), '--root', str(tmp_path)]
    assert main(['--set-password', *common]) == 1
    assert 'built without password support' in capsys.readouterr().err
    assert main(['--clear-password', *common]) == 1
    assert 'built without password support' in capsys.readouterr().err
    with pytest.raises(OSError):
        auth.set_password(tmp_path / 'auth.json', 'correct horse')
    assert auth.load(tmp_path / 'auth.json') is None
    # its own (absent) auth file: never the account's ~/.config/vitalstream/auth.json
    f = tmp_path / 'vitalstream.toml'
    f.write_text(f'require_password = true\nauth_file = "{(tmp_path / "auth.json").as_posix()}"\n')
    assert main(['--config', str(f), '--no-browser', *common]) == 1
    assert capsys.readouterr().err.strip().splitlines()[-1] == (
        'vitalstream-py: a password is required here (require_password), and this viewer was built without '
        'password support')
    monkeypatch.setattr(auth, 'SUPPORTED', True)
    assert main(['--config', str(f), '--no-browser', *common]) == 1
    err = capsys.readouterr().err.strip().splitlines()
    assert err == [f'vitalstream-py: settings from {f.resolve()}',
                   'vitalstream-py: a password is required here (require_password); set one with --set-password']


@needs_wfdb
def test_roots_outside_the_viewers_root_are_left_out(tmp_path, monkeypatch, capsys):
    from vitalstream_py.__main__ import main
    top, outside = tmp_path / 'top', tmp_path / 'outside'
    top.mkdir()
    outside.mkdir()
    monkeypatch.setattr(settings_module, 'BUILD_ROOT', str(top))
    common = ['--cache-dir', str(tmp_path / 'cache')]
    assert main(['--root', str(outside), *common, '--index', str(outside)]) == 1
    assert capsys.readouterr().err.strip() == \
        f"vitalstream-py: no data root inside this viewer's root, {top} (set when it was built)"
    import shutil
    shutil.copytree(TESTDATA / 'plain', top / 'plain')    # the readers open nothing outside the data roots
    assert main(['--root', str(outside), '--root', str(top), *common, '--index', str(TESTDATA / 'plain')]) == 1
    assert 'no WFDB records' in capsys.readouterr().err        # each record left out
    assert main(['--root', str(outside), '--root', str(top), *common, '--index', str(top / 'plain')]) == 0
    assert capsys.readouterr().err.strip().splitlines()[-1] == \
        f"vitalstream-py: {outside} is outside this viewer's root ({top}): left out"


def test_behind_path_prefix(tmp_path):
    """Served under a prefix (e.g. a portal's /rnode/<node>/<port>/): relative URLs resolve
    and nothing but the sign-in page is reachable before signing in."""
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    _, st = _client(tmp_path, [tmp_path], password='correct horse')
    from vitalstream_py.server import create_app
    outer = FastAPI()
    outer.mount('/rnode/n1/8050', create_app(st))
    c = TestClient(outer, base_url='http://localhost')
    base = '/rnode/n1/8050/'
    assert c.get(base).status_code == 401
    for p in ('api/v1/config', 'api/v1/browse', 'api/v1/docs', 'api/v1/openapi.json'):
        assert c.get(base + p).status_code == 401, p
    assert c.get(base + 'static/js/app.js').status_code == 200
    r = c.post(base + 'login', data={'password': 'correct horse'}, follow_redirects=False)
    assert r.status_code == 303 and r.headers['location'] == './'
    page = c.get(base).text
    assert 'src="static/js/app.js"' in page and c.get(base + 'api/v1/config').status_code == 200
