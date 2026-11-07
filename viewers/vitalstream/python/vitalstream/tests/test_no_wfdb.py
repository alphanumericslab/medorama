"""An installation without WFDB support (vitalstream_py without its [wfdb] extra: wfdb-x not
installed), made here by making wfdbx unimportable and importing the package afresh: the WFDB
recordings of testdata/ (one record, a multi-segment record, a folder of records, a collection)
are recognised and refused with one message, neither indexed nor read, while EDF files are read as
with wfdb-x; and the viewer refuses to open them, with that message."""

import importlib
import sys
import time
from pathlib import Path

import pytest

TESTDATA = Path(__file__).resolve().parents[3] / 'testdata'
WFDB = ['plain/rec_a.hea', 'session/ms/ms_rec.hea', 'plain', 'collection', 'session']


@pytest.fixture
def no_wfdb(monkeypatch):
    """The package imported afresh with wfdbx unimportable; the modules imported before are put
    back afterwards, and those imported here dropped."""
    before = {n for n in sys.modules if n == 'vitalstream_py' or n.startswith('vitalstream_py.')}
    monkeypatch.setitem(sys.modules, 'wfdbx', None)    # import wfdbx: ImportError
    for n in before:
        monkeypatch.delitem(sys.modules, n)
    try:
        yield importlib.import_module('vitalstream_py.sources')
    finally:
        for n in [n for n in sys.modules if n == 'vitalstream_py' or n.startswith('vitalstream_py.')]:
            if n not in before:
                del sys.modules[n]


def test_a_wfdb_record_is_refused_and_edf_is_read(no_wfdb, tmp_path):
    sources = no_wfdb
    from vitalstream_py.dataset import Dataset
    from vitalstream_py.index import build_index, load_index
    assert sources.wfdb_folder.__name__ == 'vitalstream_py.sources.wfdb_none'
    assert not sources.wfdb_folder.SUPPORTED
    cache = tmp_path / 'cache'
    for name in WFDB:
        p = TESTDATA / name
        # a folder of subfolders of records is no recording when a tree is searched, but opens as
        # the records below it
        assert sources.kind_of(p) == (None if name == 'session' else 'wfdb'), name
        assert sources.unreadable(p) == sources.NO_WFDB, name
        with pytest.raises(ValueError, match='^this build has no WFDB support'):
            build_index(str(p), str(cache))
        with pytest.raises(ValueError, match='^this build has no WFDB support'):
            sources.wfdb_folder.scan(p, sources.wfdb_folder.Limits())
    assert sources.format_label(TESTDATA / 'session' / 'ms' / 'ms_rec.hea') == 'WFDB multi-segment record'
    assert not cache.exists() or not [d for d in cache.iterdir() if d.is_dir()]
    # the samples of a WFDB index made by an installation with WFDB support
    r = sources.reader({'kind': 'wfdb', 'path': str(TESTDATA / 'plain')}, cache)
    with pytest.raises(ValueError, match='^this build has no WFDB support'):
        r.read(0, 0, 10)

    # EDF: read as always
    edf = TESTDATA / 'edf' / 'psg_plus.edf'
    assert sources.kind_of(edf) == 'edf' and sources.unreadable(edf) is None
    key = build_index(str(edf), str(cache))
    meta, arrays, folder = load_index(str(cache), key)
    ds = Dataset(meta, arrays, folder)
    n = ds.pieces[0]['n']
    assert n > 0 and len(ds.samples(0, 0, n)) == n


def test_the_viewer_refuses_a_wfdb_record(no_wfdb, tmp_path):
    sources = no_wfdb
    from fastapi.testclient import TestClient
    from vitalstream_py import paths
    from vitalstream_py.server import create_app
    from vitalstream_py.settings import Settings
    st = Settings(data_roots=[str(tmp_path), str(TESTDATA)], cache_dir=str(tmp_path / 'cache'),
                  auth_file=str(tmp_path / 'auth.json'), port=8060)
    c = TestClient(create_app(st), base_url='http://localhost:8060')
    try:
        for name in WFDB:
            p = str((TESTDATA / name).resolve())
            r = c.post('/api/v1/open', json={'path': p})
            assert (r.status_code, r.json()['detail']) == (422, sources.NO_WFDB), name
            r = c.get('/api/v1/scan', params={'path': p})
            assert (r.status_code, r.json()['detail']) == (422, sources.NO_WFDB), name
        # still listed as a record, so that opening it says why it is not read
        entries = c.get('/api/v1/browse', params={'path': str((TESTDATA / 'plain').resolve())}).json()['entries']
        assert any(e['type'] == 'record' for e in entries)
        r = c.post('/api/v1/open', json={'path': str((TESTDATA / 'edf' / 'psg_plus.edf').resolve())})
        assert r.status_code == 200, r.text
        key = r.json()['key']
        for _ in range(400):
            d = c.get(f'/api/v1/datasets/{key}').json()
            if d['state'] != 'indexing':
                break
            time.sleep(0.025)
        assert d['state'] == 'ready', d
    finally:
        c.close()
        paths.set_allowed_roots(None)
