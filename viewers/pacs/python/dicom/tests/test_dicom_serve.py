"""`serve`: the viewer's page and the folders under a root (page_server.py), never outside it."""

import http.client
import json
import os
import socket
import threading
from pathlib import Path

import pytest

from dicom_index_py import page_server as ps

HERE = Path(__file__).resolve().parent
VIEWER = 'pacs'
FORM = {'Content-Type': 'application/x-www-form-urlencoded'}


@pytest.fixture
def served(tmp_path):
    root = tmp_path / 'root'
    (root / 'sub').mkdir(parents=True)
    (root / 'sub' / 'rec.hea').write_text('rec 1 500 10\nrec.dat 16 200/mV 16 0 0 0 0 II\n')
    (root / 'sub' / 'rec.dat').write_bytes(bytes(range(20)))
    (root / 'sub' / 'img.dcm').write_bytes(b'\0' * 132)
    (root / 'page.html').write_text('<script>fetch("files/")</script>')
    (root / '.hidden').write_text('x')
    (root / '.secret').mkdir()
    (root / '.secret' / 'key').write_text('PRIVATE')
    os.symlink(root / '.secret', root / 'innocent', target_is_directory=True)   # a link to a hidden folder
    outside = tmp_path / 'outside.txt'
    outside.write_text('secret')
    os.symlink(outside, root / 'sub' / 'link.txt')
    servers = []

    def start(auth=None, host='127.0.0.1'):
        srv = ps.PageServer((host, 0), VIEWER, ps.page_dir(ps.__file__), os.path.realpath(root), auth)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        servers.append(srv)
        return srv
    yield start
    for s in servers:
        s.shutdown()
        s.server_close()


def get(srv, path, method='GET', body=None, headers=None):
    host, port = srv.server_address[:2]
    c = http.client.HTTPConnection(host, port, timeout=10)
    c.request(method, path, body=body, headers=headers or {})
    r = c.getresponse()
    return r.status, r.read(), r


def raw(srv, request):
    """The status line of the answer to a request sent as it is."""
    with socket.create_connection(srv.server_address[:2], timeout=5) as s:
        s.sendall(request)
        return s.recv(200).split(b'\r\n')[0]


def test_page_folders_and_files(served):
    srv = served()
    status, body, r = get(srv, '/')
    assert status == 200 and b'Open record' in body and b'Open multiple records' in body
    assert "script-src" not in r.getheader('Content-Security-Policy') and "object-src 'none'" in r.getheader('Content-Security-Policy')
    assert get(srv, '/served')[0] == 200
    status, body, _ = get(srv, '/browse?path=sub')
    assert status == 200
    # the viewer's own kinds of record are listed: the ECG viewer a WFDB header, not its signal file
    if VIEWER == 'ecg':
        assert b'>rec.hea<' in body and b'>img.dcm<' in body and b'>rec.dat<' not in body
        assert b'Open the records in this folder' not in body
    else:
        assert b'>img.dcm<' in body and b'>rec.hea<' not in body and b'Open the records in this folder' in body
    assert get(srv, '/files/sub/rec.dat')[1] == bytes(range(20))
    assert json.loads(get(srv, '/list/sub/')[1]) == {'files': ['img.dcm', 'rec.dat', 'rec.hea'], 'truncated': False}


def test_never_outside_the_root(served):
    srv = served()
    assert get(srv, '/files/sub/link.txt')[0] == 403          # a link that leads outside
    assert get(srv, '/files/../outside.txt')[0] in (403, 404)
    assert get(srv, '/files/%2e%2e/outside.txt')[0] == 403
    assert get(srv, '/browse?path=..')[0] == 403
    assert get(srv, '/files/.hidden')[0] == 403                # hidden entries: neither listed nor served
    assert get(srv, '/files/innocent/key')[0] == 403           # nor reached through a link to one
    assert get(srv, '/list/innocent/')[0] == 403
    assert get(srv, '/browse?path=innocent')[0] == 403
    assert get(srv, '/files/sub%5c..%5c.hidden')[0] == 403     # \ separates too
    assert get(srv, '/files/a%00b')[0] == 403
    status, body, _ = get(srv, '/browse?path=')
    assert b'.hidden' not in body and b'innocent' not in body
    assert 'innocent' not in json.loads(get(srv, '/list/')[1])['files']
    assert get(srv, '/page_server.py')[0] == 404               # only the page's own files


def test_host_must_name_this_server(served):
    srv = served()
    port = srv.server_address[1]
    assert get(srv, '/served', headers={'Host': f'attacker.example:{port}'})[0] == 421  # DNS rebinding
    assert get(srv, '/files/sub/rec.dat', headers={'Host': 'attacker.example'})[0] == 421
    assert get(srv, '/login', 'POST', 'password=x', {'Host': 'attacker.example', **FORM})[0] == 421
    for host in (f'localhost:{port}', f'127.0.0.1:{port}', f'[::1]:{port}', f'192.168.1.5:{port}', 'LOCALHOST'):
        assert get(srv, '/served', headers={'Host': host})[0] == 200, host
    assert raw(srv, b'GET /served HTTP/1.0\r\n\r\n').split()[1] == b'421'   # no Host


def test_files_run_nothing(served):
    srv = served()
    status, body, r = get(srv, '/files/page.html')
    assert status == 200 and r.getheader('Content-Security-Policy') == "sandbox; default-src 'none'"
    assert r.getheader('Content-Type') == 'application/octet-stream' and r.getheader('Content-Disposition') == 'attachment'
    _, _, r = get(srv, '/list/sub/img.dcm')
    assert r.getheader('Content-Type') == 'application/dicom' and r.getheader('Content-Disposition') is None
    assert r.getheader('Content-Security-Policy') == "sandbox; default-src 'none'"


def test_ranges(served):
    srv = served()
    status, body, r = get(srv, '/files/sub/rec.dat', headers={'Range': 'bytes=2-5'})
    assert status == 206 and body == bytes([2, 3, 4, 5]) and r.getheader('Content-Range') == 'bytes 2-5/20'
    assert get(srv, '/files/sub/rec.dat', headers={'Range': 'bytes=18-'})[1] == bytes([18, 19])
    assert get(srv, '/files/sub/rec.dat', headers={'Range': 'bytes=-3'})[1] == bytes([17, 18, 19])
    assert get(srv, '/files/sub/rec.dat', headers={'Range': 'bytes=10-999'})[1] == bytes(range(10, 20))
    status, _, r = get(srv, '/files/sub/rec.dat', headers={'Range': 'bytes=20-'})
    assert status == 416 and r.getheader('Content-Range') == 'bytes */20'
    for whole in ('bytes=0-1,4-5', 'bytes=5-2', 'items=0-1', 'bytes=x-'):  # not served as a part: the whole file
        assert get(srv, '/files/sub/rec.dat', headers={'Range': whole})[:2] == (200, bytes(range(20))), whole
    status, body, r = get(srv, '/files/sub/rec.dat', 'HEAD')
    assert status == 200 and body == b'' and r.getheader('Content-Length') == '20'
    assert ps.byte_range('bytes=-0', 20) is False and ps.byte_range('bytes=-5', 0) is False


def test_listings_say_when_they_are_cut(served, monkeypatch):
    monkeypatch.setattr(ps, 'MAX_ENTRIES', 2)
    srv = served()
    assert json.loads(get(srv, '/list/sub/')[1]) == {'files': ['img.dcm', 'rec.dat'], 'truncated': True}
    assert b'Only the first 2 of 4 entries are shown' in get(srv, '/browse?path=sub')[1]


def test_password(served, tmp_path):
    if not ps.PASSWORD:
        pytest.skip('installed without password support')
    f = tmp_path / 'auth.json'
    ps.set_password(f, 'correct horse')
    srv = served(ps.load_auth(f))
    assert get(srv, '/')[0] == 401 and get(srv, '/files/sub/rec.dat')[0] == 401
    status, _, r = get(srv, '/login', 'POST', 'password=correct+horse', FORM)
    cookie = r.getheader('Set-Cookie').split(';')[0]
    assert status == 303 and get(srv, '/files/sub/rec.dat', headers={'Cookie': cookie})[0] == 200
    # a cookie of the right name but not ASCII: refused, not an error
    assert raw(srv, f'GET /served HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {srv.cookie}=9999999999.\xe9\r\n\r\n'.encode('latin-1')).split()[1] == b'401'
    # signing out ends the sign-in (a copy of the cookie no longer opens anything)
    assert get(srv, '/logout', 'POST', '', headers={'Cookie': cookie})[0] == 303
    assert get(srv, '/files/sub/rec.dat', headers={'Cookie': cookie})[0] == 401
    # a sign-in is signed with this run's key: another server with the same password file refuses it
    _, _, r = get(srv, '/login', 'POST', 'password=correct+horse', FORM)
    cookie = r.getheader('Set-Cookie').split(';')[0]
    other = served(ps.load_auth(f))
    assert get(other, '/served', headers={'Cookie': cookie.replace(srv.cookie, other.cookie)})[0] == 401
    # a sign-out without the sign-in (another site's form) ends nothing
    assert get(srv, '/logout', 'POST', '')[0] == 303 and get(srv, '/served', headers={'Cookie': cookie})[0] == 200


def test_wrong_passwords_wait(served, tmp_path):
    if not ps.PASSWORD:
        pytest.skip('installed without password support')
    f = tmp_path / 'auth.json'
    ps.set_password(f, 'correct horse')
    srv = served(ps.load_auth(f))
    status, body, r = get(srv, '/login', 'POST', 'password=wrong', FORM)
    assert status == 401 and r.getheader('Retry-After') == '1'
    # until then, every try is refused unchecked, the right password too, from any browser
    status, body, r = get(srv, '/login', 'POST', 'password=correct+horse', FORM)
    assert status == 429 and b'not checked' in body and r.getheader('Set-Cookie') is None
    srv.retry_at = 0
    assert get(srv, '/login', 'POST', 'password=wrong', FORM)[2].getheader('Retry-After') == '2'   # doubles
    srv.retry_at = 0
    assert get(srv, '/login', 'POST', 'password=correct+horse', FORM)[0] == 303
    assert srv.failures == 0
    srv.failures = 40
    srv.retry_at = 0
    assert get(srv, '/login', 'POST', 'password=wrong', FORM)[2].getheader('Retry-After') == str(ps.BACKOFF_MAX)


def test_sign_in_bodies(served, tmp_path):
    if not ps.PASSWORD:
        pytest.skip('installed without password support')
    f = tmp_path / 'auth.json'
    ps.set_password(f, 'correct horse')
    srv = served(ps.load_auth(f))
    post = 'POST /login HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\npassword=x'
    assert raw(srv, post.format(-1).encode()).split()[1] == b'400'    # answered at once, the body unread
    assert raw(srv, post.format('abc').encode()).split()[1] == b'400'
    assert raw(srv, post.format(5000).encode()).split()[1] == b'413'
    assert raw(srv, b'POST /login HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n').split()[1] == b'400'
    assert srv.failures == 0                                           # no password was checked


def test_ipv6(served):
    assert ps.page_url('::1', 8080) == 'http://[::1]:8080/' and ps.page_url('127.0.0.1', 80) == 'http://127.0.0.1:80/'
    try:
        srv = served(host='::1')
    except OSError:
        pytest.skip('no IPv6 here')
    assert get(srv, '/served')[0] == 200


def test_the_two_viewers_carry_the_same_server():
    viewers = HERE.parents[3]
    a = viewers / 'ecg' / 'python' / 'muse' / 'muse_reader_py' / 'page_server.py'
    b = viewers / 'pacs' / 'python' / 'dicom' / 'dicom_index_py' / 'page_server.py'
    assert a.read_bytes() == b.read_bytes()
