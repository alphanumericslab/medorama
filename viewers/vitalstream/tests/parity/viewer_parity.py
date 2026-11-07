"""Viewer parity: the three viewers answer the same API requests with the same JSON
(docs/viewer-api.md), for the same recordings.

    python tests/parity/viewer_parity.py [--impl py,rs,cpp] [--adapters-dir DIR] [RECORDING ...]

Without recordings, the synthetic recordings of testdata/ are used.  Every viewer that is built
is started on a free port with its own temporary cache and the same settings file; each
recording is opened in each; then the dataset description and window / vitals / vitals_at /
markers requests at many zoom levels are compared value by value (NaN == NaN; floats exactly).
Fields that legitimately differ (cache and scan timing) are left out.  --adapters-dir gives the
viewers an adapter folder, for recordings that an adapter reads.

Before the recordings, the requests about WFDB records are compared, answers and errors alike:
config's limits; browse of a folder of multi-segment records (with and without its hidden
entries) and of a record's .hea; scan of a folder of records, of single records, of a folder
over a limit (the settings file lowers max_depth) and of one without records; open of those two
folders (422); cancel of a key nothing is being built under (404); browse and complete of a
folder holding symbolic links, one of which leads outside the data roots (not listed); browse of
the folder of EDF files (their format labels).

Then the errors, table-driven (docs/viewer-api.md, "Requests" and "Errors"): each request's status
must be the one the table gives, and the viewers' `detail` texts must be equal (for a malformed
body, its "bad request body:" prefix: the rest is the JSON library's).  The table covers the Host
check (DNS rebinding), Origin and Sec-Fetch-Site, 415 for a body that is not JSON, 413 for one over
its limit, body field types, 404 and 405, the query grammar (t0, t1, t, px, ch and names, a
parameter given twice), opening a record whose signal file leads outside the data roots (403), and
the error state of recordings that cannot be indexed (index.csv without a record column, an EDF+D
file of more than 100,000 pieces, an EDF rate that is not finite), and the headers nosniff and CSP.
Last, generated recordings are compared as the others: an EDF+D file of 3000 runs (more pieces
than pixels: one block per column) and a collection that skips what it cannot read.
"""

import http.client
import json
import math
import struct
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from impls import TESTDATA, select, viewer  # noqa: E402

SKIP = {'indexed_s', 'scan_s'}
# the settings file every viewer is started with: a folder is refused past this depth, so that a
# folder over a limit is easy to make
SETTINGS = 'max_depth = 2\n'



def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


class Viewer:
    def __init__(self, cmd, port, cache, root, adapters_dir=None, config=None):
        self.base = f'http://127.0.0.1:{port}/'
        # --root: the recording's drive.  By default the viewers open only the home folder's
        # drive (on Windows; on Linux and macOS everything), and CI checkouts may be elsewhere.
        # (root: one folder, or a list of them)
        extra = ['--adapters-dir', adapters_dir] if adapters_dir else []
        extra += ['--config', config] if config else []
        extra += [x for r in ([root] if isinstance(root, str) else root) for x in ('--root', str(r))]
        self.proc = subprocess.Popen([*cmd, '--no-browser', '--no-auth', '--port', str(port), '--cache-dir', cache,
                                      *extra],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(200):
            try:
                self.get('api/v1/config')
                return
            except OSError:
                time.sleep(0.1)
        raise RuntimeError(f'{cmd[0]} did not start')

    def get(self, path, **q):
        url = self.base + path + ('?' + urllib.parse.urlencode(q) if q else '')
        with urllib.request.urlopen(url, timeout=600) as r:
            return json.loads(r.read())

    def post(self, path, body):
        req = urllib.request.Request(self.base + path, data=json.dumps(body).encode(),
                                     headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=600) as r:
            return json.loads(r.read())

    def answer(self, method, endpoint, body=None, query=None):
        """(status, JSON) of a request, errors included."""
        url = self.base + endpoint + ('?' + urllib.parse.urlencode(query) if query else '')
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, method=method, headers={'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read())

    def open(self, path):
        r = self.post('api/v1/open', {'path': str(path)})
        key = r['key']
        while True:
            d = self.get(f'api/v1/datasets/{key}')
            if d['state'] == 'ready':
                return key, d['dataset']
            if d['state'] == 'error':
                raise RuntimeError(d['error'])
            time.sleep(0.2)

    def close(self):
        self.proc.terminate()
        self.proc.wait(10)


def diff(a, b, path='$'):
    if isinstance(a, float) and isinstance(b, float) and math.isnan(a) and math.isnan(b):
        return None
    if isinstance(a, bool) or isinstance(b, bool):
        return None if a is b else f'{path}: {a!r} != {b!r}'
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return None if a == b else f'{path}: {a!r} != {b!r}'
    if isinstance(a, dict) and isinstance(b, dict):
        ka, kb = set(a) - SKIP, set(b) - SKIP
        if ka != kb:
            return f'{path}: keys {sorted(ka ^ kb)}'
        for k in sorted(ka):
            d = diff(a[k], b[k], f'{path}.{k}')
            if d:
                return d
        return None
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return f'{path}: length {len(a)} != {len(b)}'
        for i, (x, y) in enumerate(zip(a, b)):
            d = diff(x, y, f'{path}[{i}]')
            if d:
                return d
        return None
    return None if a == b else f'{path}: {a!r} != {b!r}'


def requests(info):
    """(endpoint, params) pairs covering zoom levels from the whole recording to samples."""
    lo, hi = info['t_range']
    if lo is None:
        return []
    chs = [c['id'] for c in info['channels']]
    names = [v['name'] for v in info['vitals']]
    out = []
    span = max(hi - lo, 1.0)
    centres = [lo + span * f for f in (0.13, 0.5, 0.87)]
    for width in (span, span / 10, 600, 60, 10, 2, 0.5):
        for c in centres:
            t0, t1 = c - width / 2, c + width / 2
            for px in (300, 1200):
                if chs:
                    out.append(('window', dict(t0=t0, t1=t1, px=px, ch=','.join(map(str, chs)))))
            if names:
                out.append(('vitals', dict(t0=t0, t1=t1, px=400, names='|'.join(names))))
            if chs and width <= 600:
                out.append(('markers', dict(t0=t0, t1=t1, ch=','.join(map(str, chs)))))
    for c in centres + [lo, hi, lo - 3600]:
        out.append(('vitals_at', dict(t=c)))
    return out


def start(viewers, impls, tmp, root, adapters_dir=None):
    """Start the viewers into `viewers`, each with its own cache in `tmp` and the settings file
    SETTINGS."""
    config = Path(tmp) / 'vitalstream.toml'
    config.write_text(SETTINGS)
    for impl in impls:
        viewers[impl] = Viewer([str(viewer(impl))], free_port(), str(Path(tmp) / impl), root, adapters_dir,
                               str(config))


def api_requests(tmp):
    """(method, endpoint, body, query, expected status) of the requests about WFDB records; the
    folders they need are made in `tmp`: area/, a data root besides testdata/, and elsewhere/,
    outside the data roots (api_roots)."""
    area, elsewhere = api_roots(tmp)[0], Path(tmp) / 'elsewhere'
    deep = area / 'deep' / 'a' / 'b' / 'c'               # records 3 levels down: over max_depth
    deep.mkdir(parents=True)
    elsewhere.mkdir()
    for f in ('rec_a.hea', 'rec_a.dat'):
        shutil.copy(TESTDATA / 'plain' / f, deep / f)
        shutil.copy(TESTDATA / 'plain' / f, elsewhere / f)
    empty = area / 'empty'                               # no records
    empty.mkdir()
    (empty / 'notes.txt').write_text('no records')
    # links: inside the data roots (listed), outside them (not listed), and a dangling one; none
    # where links cannot be made (Windows without the right to)
    try:
        (area / 'link_in').symlink_to(area / 'deep')
        (area / 'link_out').symlink_to(elsewhere)
        (area / 'link_rec.hea').symlink_to(elsewhere / 'rec_a.hea')
        (area / 'dangling').symlink_to(Path(tmp) / 'nothing')
        links = True
    except OSError:
        links = False
    session, ms = TESTDATA / 'session', TESTDATA / 'session' / 'ms'
    out = [('GET', 'api/v1/config', None, {}, 200)]
    out += [('GET', 'api/v1/browse', None, dict(path=str(ms)), 200),
            ('GET', 'api/v1/browse', None, dict(path=str(ms), hidden='true'), 200),
            ('GET', 'api/v1/browse', None, dict(path=str(session)), 200),
            ('GET', 'api/v1/browse', None, dict(path=str(ms / 'ms_rec.hea')), 200),
            ('GET', 'api/v1/browse', None, dict(path=str(TESTDATA / 'edf')), 200)]
    out += [('GET', 'api/v1/scan', None, dict(path=str(p)), 200)
            for p in (session, ms / 'ms_rec.hea', ms / 'ms_rec_0001.hea', ms / 'ms_fix.hea', TESTDATA / 'plain')]
    out += [('GET', 'api/v1/scan', None, dict(path=str(area / 'deep')), 422),
            ('GET', 'api/v1/scan', None, dict(path=str(empty)), 422),
            ('POST', 'api/v1/open', {'path': str(area / 'deep')}, {}, 422),
            ('POST', 'api/v1/open', {'path': str(empty)}, {}, 422),
            ('POST', 'api/v1/cancel', {'key': '0123456789abcdef0123'}, {}, 404)]
    out += [('GET', 'api/v1/browse', None, dict(path=str(area)), 200),
            ('GET', 'api/v1/complete', None, dict(prefix=str(area / 'link')), 200),
            ('GET', 'api/v1/browse', None, dict(path=str(area / 'link_out')), 403)][:None if links else 1]
    return out


def api_roots(tmp):
    """The data roots of the viewers for api_requests."""
    return [Path(tmp) / 'area', TESTDATA]


def compare_api(names=None, report=print):
    """Compare the built viewers' answers about WFDB records (api_requests); returns the
    differences."""
    impls = select(names, report=report)
    tmp = tempfile.mkdtemp(prefix='viewer-parity-')
    viewers = {}
    problems = []
    try:
        todo = api_requests(tmp)                        # the folders first: the roots must exist
        start(viewers, impls, tmp, api_roots(tmp))
        ref = impls[0]
        for method, ep, body, q, status in todo:
            what = f'{method} {ep} {body or q}'
            answers = {impl: list(v.answer(method, ep, body, q)) for impl, v in viewers.items()}
            if ep == 'api/v1/config':                    # only the settings that concern records
                answers = {impl: [c, a.get('limits')] for impl, (c, a) in answers.items()}
            for impl, (code, _) in answers.items():
                if code != status:
                    problems.append(f'{impl} {what}: status {code}, expected {status}')
            for impl in impls[1:]:
                d = diff(answers[ref], answers[impl])
                if d:
                    problems.append(f'{impl} {what}: {d}')
    finally:
        for v in viewers.values():
            v.close()
        shutil.rmtree(tmp, ignore_errors=True)
    return problems


def compare(recording, names=None, report=print, adapters_dir=None):
    """Compare the built viewers on `recording`; returns the differences."""
    impls = select(names, report=report)
    tmp = tempfile.mkdtemp(prefix='viewer-parity-')
    viewers = {}
    problems = []
    try:
        opened, timing = {}, []
        start(viewers, impls, tmp, recording.anchor, adapters_dir)
        for impl in impls:
            t = time.time()
            opened[impl] = viewers[impl].open(recording)
            timing.append(f'{impl} {time.time() - t:.1f} s')
        report(f'  indexed: {", ".join(timing)}')
        ref = impls[0]
        kref, iref = opened[ref]
        for impl in impls[1:]:
            k, info = opened[impl]
            d = diff(iref, info)
            if d:
                problems.append(f'{impl} dataset: {d}')
            for ep, q in requests(iref):
                a = viewers[ref].get(f'api/v1/datasets/{kref}/{ep}', **q)
                b = viewers[impl].get(f'api/v1/datasets/{k}/{ep}', **q)
                d = diff(a, b)
                if d:
                    problems.append(f'{impl} {ep} {q}: {d}')
                    if len(problems) > 10:
                        break
    finally:
        for v in viewers.values():
            v.close()
        shutil.rmtree(tmp, ignore_errors=True)
    return problems


# --------------------------------------------------------------------------- errors, table-driven

def edf_runs(path, runs, spr=4, duration='1'):
    """An EDF+D file of `runs` data records of one second, each starting a second after the end of
    the one before (a break between every two): one signal of `spr` samples per record (as
    libs/engine/tests/edf.rs makes it)."""
    f = lambda s, w: s.ljust(w)  # noqa: E731
    ann = 16
    h = (f('0', 8) + f('X X X X', 80) + f('Startdate 01-MAR-2024 X X X', 80) + '01.03.2413.00.00' + f('768', 8)
         + f('EDF+D', 44) + f(str(runs), 8) + f(duration, 8) + f('2', 4) + f('ECG', 16) + f('EDF Annotations', 16)
         + f('', 160) + f('mV', 8) + f('', 8) + f('-1', 8) * 2 + f('1', 8) * 2 + f('-32768', 8) * 2
         + f('32767', 8) * 2 + f('', 160) + f(str(spr), 8) + f(str(ann), 8) + f('', 64))
    b = bytearray(h.encode())
    for r in range(runs):
        b += struct.pack(f'<{spr}h', *range(spr))
        b += f'+{2 * r}\x14\x14\0'.encode().ljust(2 * ann, b'\0')
    Path(path).write_bytes(bytes(b))


def generated(tmp):
    """Recordings made for the new rules, in `tmp`: an EDF+D file of 3000 runs, and a collection
    whose index.csv names a record that does not exist, one outside its folder (with ..) and an
    empty name, all left out."""
    tmp = Path(tmp)
    edf_runs(tmp / 'runs.edf', 3000)
    coll = tmp / 'skipping'
    coll.mkdir()
    for f in ('rec_a.hea', 'rec_a.dat'):
        shutil.copy(TESTDATA / 'plain' / f, coll / f)
    (coll / 'index.csv').write_text('record,start_unix\nrec_a,1709287200\nmissing,0\n../plain/rec_a,0\n,0\n')
    return [tmp / 'runs.edf', coll]


class Raw:
    """Requests with any headers (a browser's: Host localhost:<port> unless given)."""

    def __init__(self, port):
        self.port = port

    def __call__(self, method, path, body=None, headers=None):
        h = {'Host': f'localhost:{self.port}'}
        h.update(headers or {})
        if body is not None and 'Content-Type' not in h:
            h['Content-Type'] = 'application/json'
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=600)
        try:
            c.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
            for k, v in h.items():
                if v is not None:
                    c.putheader(k, v)
            data = body.encode() if isinstance(body, str) else body
            if data is not None:
                c.putheader('Content-Length', str(len(data)))
            c.endheaders(data)
            r = c.getresponse()
            text = r.read()
            headers = {k.lower(): v for k, v in r.getheaders()}
        finally:
            c.close()
        try:
            answer = json.loads(text)
        except ValueError:
            answer = None
        return r.status, answer, headers

    def opened(self, path):
        """The final state of opening `path` (or the error answered)."""
        status, d, _ = self('POST', '/api/v1/open', json.dumps({'path': str(path)}))
        if status != 200:
            return d
        key = d['key']
        for _ in range(3000):
            _, d, _ = self('GET', f'/api/v1/datasets/{key}')
            if d.get('state') != 'indexing':
                return d
            time.sleep(0.05)
        raise RuntimeError(f'{path}: still indexing')


def error_cases(tmp):
    """The fixtures of the error table in `tmp` (root/, the data root, and outside/), and the
    table: (name, request(raw) -> (status, answer, headers), expected status or None)."""
    tmp = Path(tmp)
    root, outside = tmp / 'root', tmp / 'outside'
    root.mkdir()
    outside.mkdir()
    shutil.copytree(TESTDATA / 'collection', root / 'collection')
    (outside / 'secret.dat').write_bytes(bytes([1, 0, 2, 0]))
    (root / 'r.hea').write_text('r 1 250 2\nr.dat 16 200/mV 16 0 0 3 0 II\n')
    try:
        (root / 'r.dat').symlink_to(outside / 'secret.dat')
        links = True
    except OSError:
        links = False
    (root / 'norecord').mkdir()
    (root / 'norecord' / 'index.csv').write_text('name\nx\n')
    edf_runs(root / 'too_many.edf', 100_001, spr=1)
    edf_runs(root / 'tiny.edf', 2, duration='1e-320')
    coll = json.dumps({'path': str(root / 'collection')})
    get = lambda path, **h: lambda r: r('GET', path, headers=h)  # noqa: E731
    post = lambda path, body, **h: lambda r: r('POST', path, body, h)  # noqa: E731
    cases = [
        # the Host check (DNS rebinding)
        ('host evil', get('/api/v1/config', Host='evil.example:8060'), 400),
        ('host suffix', get('/api/v1/config', Host='localhost.evil.example'), 400),
        ('host empty', get('/api/v1/config', Host=''), 400),
        ('host ::1', get('/api/v1/config', Host='[::1]:8060'), 200),
        ('host 127.0.0.1', get('/api/v1/config', Host='127.0.0.1'), 200),
        ('host case', get('/api/v1/config', Host='LOCALHOST'), 200),
        # requests that change something
        ('text/plain', post('/api/v1/open', coll, **{'Content-Type': 'text/plain'}), 415),
        ('form', post('/api/v1/open', coll, **{'Content-Type': 'application/x-www-form-urlencoded'}), 415),
        ('no content type', post('/api/v1/cancel', '{"key": "0"}', **{'Content-Type': None}), 415),
        ('origin evil', post('/api/v1/open', coll, Origin='http://evil.example'), 403),
        ('origin null', post('/api/v1/open', coll, Origin='null'), 403),
        ('origin with a path', post('/api/v1/open', coll, Origin='http://localhost/x'), 403),
        ('cross-site', post('/api/v1/cancel', '{"key": "0"}', **{'Sec-Fetch-Site': 'cross-site'}), 403),
        ('origin local', post('/api/v1/open', coll, Origin='http://localhost:8060',
                              **{'Content-Type': 'application/json; charset=utf-8'}), 200),
        # bodies
        ('reindex not a boolean', post('/api/v1/open', coll.replace('}', ', "reindex": "yes"}')), 400),
        ('path not a string', post('/api/v1/open', '{"path": 3}'), 400),
        ('no path', post('/api/v1/open', '{}'), 400),
        ('not JSON', post('/api/v1/open', '{"path"'), 400),
        ('nested deeply', post('/api/v1/open', '[' * 20000), 400),
        ('key not a string', post('/api/v1/cancel', '{"key": 1}'), 400),
        ('body over 64 KiB', post('/api/v1/cancel', ' ' * 70000), 413),
        # routing
        ('unknown path', get('/api/v1/nothing'), 404),
        ('DELETE', lambda r: r('DELETE', '/api/v1/config'), 405),
        ('PUT', lambda r: r('PUT', '/api/v1/open', '{}'), 405),
        ('GET of a POST', get('/api/v1/open'), 405),
        ('cancel unknown key', post('/api/v1/cancel', '{"key": "0123456789abcdef0123"}'), 404),
        ('dataset not indexed', get('/api/v1/datasets/0123456789abcdef0123/window?t0=0&t1=1&ch=0'), 404),
        # queries
        ('a parameter twice', get(f'/api/v1/browse?path={root}&path={root}'), 400),
        ('scan without a path', get('/api/v1/scan'), 403),
        # recordings that cannot be indexed: the state error, with the reason
        ('no record column', lambda r: (200, r.opened(root / 'norecord'), {}), 200),
        ('more than 100000 pieces', lambda r: (200, r.opened(root / 'too_many.edf'), {}), 200),
        ('a rate that is not finite', lambda r: (200, r.opened(root / 'tiny.edf'), {}), 200),
        # headers
        ('the page', lambda r: (lambda a: (a[0], None, {k: a[2].get(k) for k in (
            'content-security-policy', 'x-content-type-options')}))(r('GET', '/')), 200),
        ('an API answer', lambda r: (lambda a: (a[0], None, {'x-content-type-options': a[2].get(
            'x-content-type-options')}))(r('GET', '/api/v1/config')), 200),
        ('the sign-in style', get('/static/login.css'), 200),
    ]
    if links:
        cases.append(('a signal file outside', post('/api/v1/open', json.dumps({'path': str(root / 'r.hea')})), 403))
    return root, cases


def query_cases(key, t0):
    """(name, path, expected status) of queries of the dataset `key` (testdata/collection, whose
    data start at t0)."""
    t1, t2 = t0 + 1, t0 + 2
    w = f'/api/v1/datasets/{key}/'
    many = ','.join(['0'] * 3000)
    return [
        ('a channel many times', f'{w}window?t0={t1}&t1={t2}&ch={many}', 200),
        ('unknown channel', f'{w}window?t0={t1}&t1={t2}&ch=0,99', 400),
        ('negative channel', f'{w}window?t0={t1}&t1={t2}&ch=-1', 400),
        ('channel too large', f'{w}window?t0={t1}&t1={t2}&ch=99999999999999999999', 400),
        ('-inf', f'{w}window?t0=-inf&t1={t2}&ch=0', 400),
        ('nan', f'{w}window?t0=nan&t1={t2}&ch=0', 400),
        ('hex', f'{w}window?t0=0x10&t1={t2}&ch=0', 400),
        ('underscore', f'{w}window?t0=1_0&t1={t2}&ch=0', 400),
        ('overflow', f'{w}window?t0=1e400&t1={t2}&ch=0', 400),
        ('space', f'{w}window?t0=%20{t1}&t1={t2}&ch=0', 400),
        ('px not an integer', f'{w}window?t0={t1}&t1={t2}&px=1.5&ch=0', 400),
        ('px too large', f'{w}window?t0={t1}&t1={t2}&px=99999999999999999999&ch=0', 400),
        ('t0 twice', f'{w}window?t0={t1}&t0={t1}&t1={t2}&ch=0', 400),
        ('t1 before t0', f'{w}window?t0={t2}&t1={t1}&ch=0', 400),
        ('t1 equal to t0', f'{w}markers?t0={t1}&t1={t1}&ch=0', 400),
        ('missing t0', f'{w}window?t1={t2}&ch=0', 400),
        ('vitals backwards', f'{w}vitals?t0={t2}&t1={t1}&names=HR', 400),
        ('vitals inf', f'{w}vitals?t0={t1}&t1=inf&names=HR', 400),
        ('markers Infinity', f'{w}markers?t0={t1}&t1=Infinity&ch=0', 400),
        ('vitals_at -inf', f'{w}vitals_at?t=-inf', 400),
        ('decimals', f'{w}window?t0=.5&t1=5.&ch=0', 200),
        ('an exponent', f'{w}window?t0=1E3&t1=1e%2B4&ch=0', 200),
        ('far outside', f'{w}window?t0=-1e308&t1=1e308&ch=0,1', 200),
        ('far away', f'{w}window?t0=1e300&t1=1.5e300&ch=0', 200),
        ('markers far outside', f'{w}markers?t0=-1e308&t1=1e308&ch=0', 200),
        ('vitals far outside', f'{w}vitals?t0=-1e308&t1=1e308&names=HR', 200),
        ('a series many times', f'{w}vitals?t0={t0}&t1={t2}&names={"|".join(["HR"] * 2000)}', 200),
        ('too many series', f'{w}vitals?t0={t0}&t1={t2}&names={"|".join(f"n{k}" for k in range(600))}', 400),
    ]


# the rows of the error table whose headers are compared (those rows answer only the headers
# that matter)
HEADER_CASES = ('the page', 'an API answer')


def same_answer(a, b):
    """Whether two answers of the error table agree (a malformed body: its prefix only)."""
    prefix = 'bad request body: '
    if isinstance(a, dict) and isinstance(b, dict) and str(a.get('detail', '')).startswith(prefix):
        return str(b.get('detail', '')).startswith(prefix) and set(a) == set(b)
    return diff(a, b) is None


def compare_errors(names=None, report=print):
    """Compare the viewers' errors (error_cases, query_cases); returns the differences."""
    impls = select(names, report=report)
    tmp = tempfile.mkdtemp(prefix='viewer-parity-')
    viewers = {}
    problems = []
    try:
        root, cases = error_cases(tmp)
        start(viewers, impls, tmp, [root])
        raws = {impl: Raw(int(v.base.rsplit(':', 1)[1].rstrip('/'))) for impl, v in viewers.items()}
        ref = impls[0]

        def check(name, asks, status):
            answers = {impl: asks[impl](raw) for impl, raw in raws.items()}
            for impl, (code, answer, headers) in answers.items():
                if status is not None and code != status:
                    problems.append(f'{impl} {name}: status {code}, expected {status} ({answer})')
            a = answers[ref]
            for impl in impls[1:]:
                b = answers[impl]
                if a[0] != b[0] or not same_answer(a[1], b[1]):
                    problems.append(f'{impl} {name}: {b[:2]}, {ref} {a[:2]}')
                if name in HEADER_CASES and a[2] != b[2]:
                    problems.append(f'{impl} {name}: headers {b[2]}, {ref} {a[2]}')

        for name, ask, status in cases:
            check(name, {impl: ask for impl in impls}, status)
        info = {impl: raw.opened(root / 'collection') for impl, raw in raws.items()}
        for impl in impls:
            if info[impl].get('state') != 'ready':
                problems.append(f'{impl}: testdata/collection not opened: {info[impl]}')
                return problems
        t0 = info[ref]['dataset']['t_range'][0]
        tables = {impl: query_cases(info[impl]['dataset']['key'], t0) for impl in impls}
        for k, (name, _, status) in enumerate(tables[ref]):
            check(f'query {name}', {impl: (lambda raw, q=tables[impl][k][1]: raw('GET', q)) for impl in impls}, status)
    finally:
        for v in viewers.values():
            v.close()
        shutil.rmtree(tmp, ignore_errors=True)
    return problems



if __name__ == '__main__':
    import argparse
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('recordings', nargs='*')
    ap.add_argument('--impl', help='comma-separated: py,rs,cpp (default: every viewer that is built)')
    ap.add_argument('--adapters-dir', help='adapter folder given to the viewers')
    args = ap.parse_args()
    recordings = args.recordings or [str(TESTDATA / x) for x in (
        'collection', 'plain', 'session', 'session/ms/ms_rec.hea', 'edf/psg_plus.edf', 'edf/gappy_plus.edf',
        'edf/plain.edf', 'edf/eeg_plus.bdf')]
    bad = 0
    made = tempfile.mkdtemp(prefix='viewer-parity-made-')
    if not args.recordings:
        print('requests about WFDB records')
        p = compare_api(args.impl)
        print('  OK' if not p else '  ' + '\n  '.join(p))
        bad += bool(p)
        print('errors')
        p = compare_errors(args.impl)
        print('  OK' if not p else '  ' + '\n  '.join(p))
        bad += bool(p)
        recordings += [str(x) for x in generated(made)]
    for arg in recordings:
        print(arg)
        p = compare(Path(arg).resolve(), args.impl, adapters_dir=args.adapters_dir)
        print('  OK' if not p else '  ' + '\n  '.join(p))
        bad += bool(p)
    shutil.rmtree(made, ignore_errors=True)
    sys.exit(1 if bad else 0)
