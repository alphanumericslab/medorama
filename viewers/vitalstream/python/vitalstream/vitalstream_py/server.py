"""HTTP API (version 1, under /api/v1/; see docs/viewer-api.md) and static front end.

  GET  /api/v1/config                      time zone, data roots, home folder, version, limits
  GET  /api/v1/browse?path=                folders and recordings (default: home)
  GET  /api/v1/complete?prefix=            path completion for the path box
  GET  /api/v1/scan?path=                  what opening a folder or a WFDB record would read
  POST /api/v1/open {"path": ...}          index (in a worker process) or load from cache
  POST /api/v1/cancel {"key": ...}         stop an index being built
  GET  /api/v1/datasets/{key}              indexing progress, or the dataset description
  GET  /api/v1/datasets/{key}/window       ?t0&t1&px&ch=0,1,2  samples or min/max per channel
  GET  /api/v1/datasets/{key}/vitals       ?t0&t1&px&names=a|b  trend points
  GET  /api/v1/datasets/{key}/vitals_at    ?t  latest value of every vital sign
  GET  /api/v1/datasets/{key}/markers      ?t0&t1&ch=...  waveform markers

Access: when a password is set (auth.py), every /api request needs the login cookie that
POST /login sets.  The page uses relative URLs, so it also works behind a reverse proxy
that serves it under a path prefix.  Only paths
allowed by settings.allowed are served (symbolic links: see settings.py).  Patient names
and dates of birth are never read.  Every request must name an allowed host (DNS rebinding), and
one that changes something must come from an allowed origin with a JSON body (another site's
page cannot send one): docs/viewer-api.md, "Requests".  Query parameters are checked strictly,
with the same statuses and messages as the Rust viewer.
"""

import asyncio
import math
import os
import re
import shutil
import tempfile
import threading
import time
from collections import OrderedDict
from concurrent.futures import ProcessPoolExecutor
from concurrent.futures.process import BrokenProcessPool
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, HTTPException, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, RedirectResponse, Response
from fastapi.staticfiles import StaticFiles
from starlette.concurrency import run_in_threadpool

from . import __version__, auth
from . import adapters, paths
from .common import json_loads
from .dataset import Dataset
from .index import build_index, cache_key, load_index, read_progress
from .settings import FS_ROOT, HOME, IN_REPO, SAMPLE_DATA, absolute
from .sources import format_label, kind_of, unreadable, wfdb_folder

# The page: packaged copy (installed wheel), else <repo>/web/ (editable install, run from a clone).
STATIC = Path(__file__).parent / 'static'
if not (STATIC / 'index.html').is_file():
    STATIC = Path(__file__).resolve().parents[3] / 'web'
MAX_LOADED = 4
MAX_ENTRIES = 5000        # folder listing limit
MAX_COMPLETIONS = 50
MAX_CHANNELS = 64         # the most channels of one window or markers request
MAX_SERIES = 512          # the most series of one vitals request
MAX_LOGIN_BODY = 4 * 1024           # the largest request bodies: login (a password)
MAX_API_BODY = 64 * 1024            # and the API's (open, cancel)
# wrong passwords: the delay before the next attempt from the same address is checked doubles
# with each (1, 3, 7, 15 s, at most 30 s), for FAILURES_KEPT seconds
MAX_LOGIN_DELAY_S = 30
FAILURES_KEPT = 15 * 60
# the Content-Security-Policy of the page and the sign-in page
CSP = ("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; "
       "object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'")

LOGIN_PAGE = """<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>VitalStream</title>
<link rel="stylesheet" href="static/login.css"></head><body>
<form method="post" action="login"><h1>VitalStream</h1>{error}
<input type="password" name="password" placeholder="Password" autofocus autocomplete="current-password">
<button>Sign in</button></form></body></html>"""

_DECIMAL = re.compile(r'[+-]?(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?')
_INTEGER = re.compile(r'[+-]?[0-9]+')
_DIGITS = re.compile(r'[0-9]+')


class _Flag:
    """The cancel flag of one index being built, which reaches the worker process: a file that
    exists once the flag is set."""

    def __init__(self, path):
        self.path = path

    def __call__(self):
        return os.path.exists(self.path)

    def set(self):
        open(self.path, 'w').close()

    def clear(self):
        try:
            os.remove(self.path)
        except FileNotFoundError:
            pass


def _init_worker(adapters_dirs, limits, roots):
    """A worker process: the adapter folders, the limits on a folder of records and the data
    roots of the settings."""
    adapters.set_folders(adapters_dirs)
    wfdb_folder.set_limits(limits)
    paths.set_allowed_roots(roots)


def _url_decode(s):
    """application/x-www-form-urlencoded decoding as the Rust viewer does it: '+' is a space,
    %XX a byte (a '%' not followed by two hex digits stays), invalid UTF-8 replaced."""
    b = s.encode('utf-8', 'surrogateescape')
    out = bytearray()
    i = 0
    while i < len(b):
        c = b[i]
        if c == 0x2B:
            out.append(0x20)
        elif c == 0x25 and i + 2 < len(b) and re.fullmatch(rb'[0-9A-Fa-f]{2}', b[i + 1:i + 3]):
            out.append(int(b[i + 1:i + 3], 16))
            i += 2
        else:
            out.append(c)
        i += 1
    return out.decode('utf-8', 'replace')


def _query(request):
    """The query parameters; a parameter given twice is a bad request."""
    raw = request.scope.get('query_string', b'').decode('latin-1')
    out = {}
    for kv in raw.split('&'):
        if not kv:
            continue
        k, _, v = kv.partition('=')
        k = _url_decode(k)
        if k in out:
            raise HTTPException(400, f'query parameter {k} is given more than once')
        out[k] = _url_decode(v)
    return out


def _q_f64(q, k):
    """A required time (or other number): a finite decimal number."""
    v = q.get(k)
    if v is None:
        raise HTTPException(400, f'missing query parameter {k}')
    x = float(v) if _DECIMAL.fullmatch(v) else math.nan
    if not math.isfinite(x):
        raise HTTPException(400, f'{k} must be a finite number')
    return x


def _q_span(q):
    """t0 and t1: finite, t0 < t1."""
    t0, t1 = _q_f64(q, 't0'), _q_f64(q, 't1')
    if t1 <= t0:
        raise HTTPException(400, 't1 must be after t0')
    return t0, t1


def _q_i64(q, k, default):
    v = q.get(k)
    if v is None:
        return default
    x = int(v) if _INTEGER.fullmatch(v) else None
    if x is None or not -2 ** 63 <= x < 2 ** 63:
        raise HTTPException(400, f'{k} must be an integer')
    return x


def _q_channels(q, channels):
    """The channel ids of `ch` (a comma-separated list), each once, in order: ids of channels of
    the dataset (0 <= id < channels), at most MAX_CHANNELS."""
    out = []
    for c in (q.get('ch') or '').split(','):
        if not c:
            continue
        cid = int(c) if _DIGITS.fullmatch(c) else None
        if cid is None or cid >= 2 ** 63:
            raise HTTPException(400, 'ch must be a comma-separated list of channel ids')
        if cid >= channels:
            raise HTTPException(400, f'no channel {cid} in this dataset')
        if cid not in out:
            out.append(cid)
    if len(out) > MAX_CHANNELS:
        raise HTTPException(400, f'at most {MAX_CHANNELS} channels at once')
    return out


def _q_bool(q, k):
    return (q.get(k) or '').lower() in ('true', '1', 'yes', 'on')


def _host_name(host):
    """The name of a Host header or an origin's host, lower case, without the port and the
    brackets of an IPv6 address ('[::1]:8060' -> '::1')."""
    h = host.strip().lower()
    if h.startswith('['):
        i = h.find(']')
        return h[1:i] if i >= 0 else ''
    if h.count(':') > 1:                 # a bare IPv6 address (the configured host)
        return h
    return h.split(':', 1)[0]


def _visible(v):
    """A header value as the Rust viewer reads it as text: None unless visible ASCII (and tabs)."""
    if v is None or any(not (32 <= ord(c) < 127 or c == '\t') for c in v):
        return None
    return v


async def _body(request, limit):
    """A request body of at most `limit` bytes (413 beyond)."""
    too_big = HTTPException(413, f'the request body is larger than {limit} bytes')
    n = request.headers.get('content-length')
    if n is not None and n.strip().isdigit() and int(n) > limit:
        raise too_big
    data = bytearray()
    async for chunk in request.stream():
        data += chunk
        if len(data) > limit:
            raise too_big
    return bytes(data)


def _json_body(body, fields):
    """The JSON object of a request body, its `fields` ({name: (type, default)}) checked: a
    missing field without a default, or one of another type, is a bad request; others are
    ignored."""
    def bad(why):
        return HTTPException(400, f'bad request body: {why}')
    try:
        d = json_loads(body.decode('utf-8'))
    except (UnicodeDecodeError, ValueError, RecursionError) as e:
        raise bad(e) from None
    if not isinstance(d, dict):
        raise bad('expected a JSON object')
    out = {}
    for k, (kind, default) in fields.items():
        if k not in d:
            if default is None:
                raise bad(f'missing field `{k}`')
            out[k] = default
        elif type(d[k]) is not kind:
            raise bad(f'invalid type for `{k}`: expected a {"boolean" if kind is bool else "string"}')
        else:
            out[k] = d[k]
    return out


def _form_value(body, key):
    for kv in body.decode('utf-8', 'replace').split('&'):
        k, _, v = kv.partition('=')
        if _url_decode(k) == key:
            return _url_decode(v)
    return ''


def _error(status, detail):
    return JSONResponse({'detail': detail}, status_code=status)


def _entry(path, name):
    """Listing entry for path/name, or None for things the viewer cannot open."""
    full = os.path.join(path, name)
    link = os.path.islink(full)
    if link and not os.path.exists(full):
        return {'name': name, 'path': full, 'type': 'broken', 'link': True}
    if wfdb_folder.is_record(full):
        return {'name': name[:-4] if name.endswith('.hea') else name, 'path': full, 'type': 'record', 'kind': 'wfdb',
                'label': wfdb_folder.format_label(full), 'link': link}
    k = kind_of(full)
    if k == 'wfdb':
        return {'name': name, 'path': full, 'type': 'wfdb', 'link': link}
    if k is not None:
        return {'name': name, 'path': full, 'type': 'file', 'kind': k, 'label': format_label(full),
                'size': os.path.getsize(full), 'link': link}
    if os.path.isdir(full):
        return {'name': name, 'path': full, 'type': 'dir', 'link': link}
    return None


def create_app(settings, protected=None):
    """protected: require the password (default: when one is set)."""
    # indexing runs in worker processes, which look for adapters in the same folder, read
    # folders of records within the same limits and open files in the same data roots; a pool
    # broken by a worker that died is made again
    def new_pool():
        return ProcessPoolExecutor(max_workers=max(1, settings.index_workers), initializer=_init_worker,
                                   initargs=([settings.adapters_dir], settings.limits, settings.data_roots))
    pool = [new_pool()]
    # this process reads samples too: from no file outside the data roots
    paths.set_allowed_roots(settings.data_roots)
    flags = tempfile.mkdtemp(prefix='vitalstream-cancel-')     # the cancel flags of the jobs

    @asynccontextmanager
    async def lifespan(app):
        yield
        pool[0].shutdown(wait=False, cancel_futures=True)
        shutil.rmtree(flags, ignore_errors=True)

    app = FastAPI(title='VitalStream', version=__version__, docs_url='/api/v1/docs', openapi_url='/api/v1/openapi.json', redoc_url=None,
                  lifespan=lifespan)
    app.state.pool = pool
    jobs = {}                     # key -> (Future, its cancel _Flag)
    loaded = OrderedDict()        # key -> Dataset
    opened = set()                # keys opened through allowed paths in this session
    lock = threading.Lock()
    creds = auth.load(settings.auth_file)
    if protected is None:
        protected = creds is not None
    if protected and creds is None:
        raise RuntimeError('password protection requested but no password is set')
    cookie = f'vitalstream_{settings.port}'
    checking = asyncio.Semaphore(1)        # one password checked at a time (scrypt takes 16 MiB)
    failures = {}                          # wrong passwords by client address: (how many, the last)
    revoked = auth.revoked(settings.auth_file)  # sign-ins ended by logout: signature -> expiry

    def cookie_value(request):
        return request.cookies.get(cookie)

    def logged_in(request):
        if not protected:
            return True
        value = cookie_value(request)
        if value is not None and '.' in value and value.split('.', 1)[1] in revoked:
            return False
        return auth.cookie_ok(creds, value)

    def host_allowed(host):
        """Whether a request names a host this server answers for (docs/viewer-api.md,
        "Requests"): localhost, 127.0.0.1, ::1, the host it listens on, or one of
        allowed_hosts ("*": any), with any port."""
        name = _host_name(host)
        if not name:
            return False
        configured = _host_name(settings.host)
        return (name in ('localhost', '127.0.0.1', '::1')
                or (name == configured and configured not in ('0.0.0.0', '::', ''))
                or any(h == '*' or _host_name(h) == name for h in settings.allowed_hosts))

    @app.middleware('http')
    async def guard(request: Request, call_next):
        # Every request: a Host this server answers for (400).  A request that changes something
        # (any method but GET and HEAD): no foreign Origin and no cross-site fetch (403), and, a
        # POST to the API, a JSON body (415).  Then deny by default: before sign-in only the page
        # itself (which then shows the sign-in form), /login, /logout and the static files are
        # served.  Paths are taken relative to any prefix a reverse proxy mounts the app under.
        path = request.scope.get('path', '')
        root = request.scope.get('root_path', '')
        if root and path.startswith(root):
            path = path[len(root):] or '/'
        host = _visible(request.headers.get('host'))
        if host is None or not host_allowed(host):
            r = _error(400, 'this server does not answer for that host (see allowed_hosts)')
        elif request.method not in ('GET', 'HEAD') and not _same_site(request):
            r = _error(403, 'a request from another site')
        elif request.method == 'POST' and path.startswith('/api/') and not _json_type(request):
            r = _error(415, 'the request body must be JSON (Content-Type: application/json)')
        elif not (path in ('/', '/login', '/logout') or path.startswith('/static/')) and not logged_in(request):
            r = _error(401, 'sign in required')
        else:
            try:
                r = await call_next(request)
            except Exception:
                r = _error(500, 'internal error')
        r.headers['X-Content-Type-Options'] = 'nosniff'
        if r.headers.get('content-type', '').startswith('text/html'):
            r.headers['Content-Security-Policy'] = CSP
        return r

    def _same_site(request):
        origin = request.headers.get('origin')
        if origin is not None:
            o = _visible(origin) or ''
            rest = o[7:] if o.startswith('http://') else o[8:] if o.startswith('https://') else None
            if rest is None or '/' in rest or not host_allowed(rest):
                return False
        return request.headers.get('sec-fetch-site') != 'cross-site'

    def _json_type(request):
        t = _visible(request.headers.get('content-type'))
        return t is not None and t.split(';', 1)[0].strip().lower() == 'application/json'

    @app.exception_handler(RequestValidationError)
    async def bad_request(request, e):
        errors = e.errors()
        why = errors[0].get('msg', 'bad request') if errors else 'bad request'
        return _error(400, str(why))

    def check_path(path):
        """Absolute path as given (links not resolved), if allowed and existing."""
        p = absolute(path)
        if not settings.allowed(p):
            raise HTTPException(403, 'path outside the configured data roots')
        if not os.path.exists(p):
            raise HTTPException(404, f'no such file or folder: {p}')
        return p

    def check_record(hea):
        """403 unless every file a WFDB record reads (its header, the signal files it names, its
        segments, its .mrk file) leads inside the data roots, links resolved."""
        hea = Path(hea)
        for f in wfdb_folder.files_read(hea.parent, hea.name[:-4] if hea.name.endswith('.hea') else hea.stem):
            if os.path.lexists(f) and not settings.allowed(str(f)):
                raise HTTPException(403, f'{f.name}, a file of this record, lies outside the data roots')

    def dataset(key):
        with lock:
            ds = loaded.get(key)
            if ds is not None:
                loaded.move_to_end(key)
                return ds
        found = load_index(settings.cache_dir, key)
        if found is None:
            return None
        ds = Dataset(*found)
        if key not in opened and not settings.allowed(ds.meta['path']):
            raise HTTPException(403, 'path outside the configured data roots')
        with lock:
            loaded[key] = ds
            while len(loaded) > MAX_LOADED:
                loaded.popitem(last=False)[1].close()
        return ds

    def need(key):
        ds = dataset(key)
        if ds is None:
            raise HTTPException(404, 'dataset not indexed')
        return ds

    def start_folder():
        if settings.allowed(HOME) and os.path.isdir(HOME):
            return HOME
        return settings.data_roots[0]

    @app.get('/api/v1/config')
    def config():
        places = [{'name': 'Home', 'path': HOME}] if settings.allowed(HOME) else []
        if IN_REPO and settings.allowed(SAMPLE_DATA):
            places.append({'name': 'sample_data', 'path': str(SAMPLE_DATA)})
        places += [{'name': r, 'path': r} for r in settings.data_roots if r not in (HOME, FS_ROOT)]
        return {'timezone': settings.timezone, 'data_roots': settings.data_roots, 'home': HOME,
                'start': start_folder(), 'places': places, 'sep': os.sep, 'version': __version__,
                'qc_verbose': settings.qc_verbose,
                'limits': {'max_records': settings.max_records, 'max_depth': settings.max_depth,
                           'max_scan_s': settings.max_scan_s if math.isfinite(settings.max_scan_s) else None}}

    @app.get('/api/v1/browse')
    def browse(request: Request):
        """List a folder.  A path to a recording file (a WFDB record's .hea, or a file an adapter
        reads) returns {'type': 'file'} (the page opens it)."""
        q = _query(request)
        p = check_path(q.get('path') or start_folder())
        hidden = _q_bool(q, 'hidden')
        k = kind_of(p)
        if k is not None and not os.path.isdir(p):
            return {'path': p, 'type': 'file'}
        if not os.path.isdir(p):
            raise HTTPException(400, wfdb_folder.signal_file_note(p) or f'not a folder or a recording this viewer reads: {p}')
        try:
            names = sorted(os.listdir(p), key=str.lower)
        except PermissionError:
            raise HTTPException(403, f'permission denied: {p}')
        # the segments and layout headers of the multi-segment records here: part of those
        # records, listed only with the hidden entries
        segments = set()
        for n in names:
            if n.endswith('.hea'):
                for seg, _ in wfdb_folder.segment_table(os.path.join(p, n[:-4])) or []:
                    segments.add(f'{seg}.hea')
        entries, truncated = [], False
        for name in names:
            if (name.startswith('.') or name in segments) and not hidden:
                continue
            # a symbolic link that leads outside the data roots is not listed
            if not settings.allowed(os.path.join(p, name)):
                continue
            try:
                e = _entry(p, name)
            except OSError:
                continue
            if e is not None:
                entries.append(e)
                if len(entries) >= MAX_ENTRIES:
                    truncated = True
                    break
        entries.sort(key=lambda e: (e['type'] in ('file', 'record'), e['name'].lower()))
        parent = os.path.dirname(p)
        return {'path': p, 'type': 'dir', 'kind': k, 'entries': entries, 'truncated': truncated,
                'parent': parent if parent != p and settings.allowed(parent) else None}

    @app.get('/api/v1/complete')
    def complete(request: Request):
        """Folders and recordings whose path starts with `prefix` (for the path box)."""
        prefix = _query(request).get('prefix', '')
        text = paths.expand(prefix) if prefix else start_folder() + os.sep
        folder, part = (text, '') if text.endswith(os.sep) else os.path.split(text)
        folder = absolute(folder or os.sep)
        if not settings.allowed(folder) or not os.path.isdir(folder):
            return {'items': []}
        out = []
        try:
            names = sorted(os.listdir(folder), key=str.lower)
        except OSError:
            return {'items': []}
        for name in names:
            if not name.lower().startswith(part.lower()) or (name.startswith('.') and not part.startswith('.')):
                continue
            if not settings.allowed(os.path.join(folder, name)):
                continue
            try:
                e = _entry(folder, name)
            except OSError:
                continue
            if e is not None and e['type'] != 'broken':
                out.append({'path': e['path'] + (os.sep if e['type'] == 'dir' else ''), 'type': e['type']})
                if len(out) >= MAX_COMPLETIONS:
                    break
        return {'items': out}

    @app.get('/api/v1/scan')
    def scan(request: Request):
        """What opening a folder of records (or one record) would read, from the headers alone,
        within the limits (docs/viewer-api.md, scan)."""
        p = check_path(_query(request).get('path', ''))
        if not os.path.isdir(p) and not wfdb_folder.is_record(p):
            raise HTTPException(400, f'not a folder or a WFDB record: {p}')
        try:
            return wfdb_folder.scan(p, settings.limits)
        except Exception as e:
            raise HTTPException(422, str(e))

    def start_job(key, p):
        """Index `p` under `key` in a worker process (a pool broken by a worker that died is made
        again)."""
        with lock:
            job = jobs.get(key)
            if job is not None and not job[0].done():
                return
            flag = _Flag(os.path.join(flags, key))
            flag.clear()
            try:
                future = pool[0].submit(build_index, str(p), settings.cache_dir, flag)
            except (BrokenProcessPool, RuntimeError):
                pool[0].shutdown(wait=False, cancel_futures=True)
                pool[0] = new_pool()
                future = pool[0].submit(build_index, str(p), settings.cache_dir, flag)
            jobs[key] = (future, flag)

    def open_path(path, reindex):
        p = check_path(path)
        if not os.path.isdir(p) and kind_of(p) is None:
            raise HTTPException(400, wfdb_folder.signal_file_note(p) or f'not a recording this viewer reads: {p}')
        # a WFDB record or folder without WFDB support
        why = unreadable(p)
        if why:
            raise HTTPException(422, why)
        # one record: every file it reads lies inside the data roots (its signal files may be links)
        if wfdb_folder.is_record(p):
            check_record(p)
        # a folder: within the limits on the records it holds, before anything is read
        if os.path.isdir(p) and not os.path.isfile(os.path.join(p, 'index.csv')):
            try:
                wfdb_folder.scan(p, settings.limits)
            except Exception as e:
                raise HTTPException(422, str(e))
        key = cache_key(p)
        opened.add(key)
        if not reindex and dataset(key) is not None:
            return {'key': key, 'state': 'ready'}
        start_job(key, p)
        return {'key': key, 'state': 'indexing'}

    @app.post('/api/v1/open')
    async def open_source(request: Request):
        req = _json_body(await _body(request, MAX_API_BODY), {'path': (str, None), 'reindex': (bool, False)})
        return await run_in_threadpool(open_path, req['path'], req['reindex'])

    def cancelled(job):
        """True for a job that stopped because it was cancelled."""
        return job[0].done() and job[0].exception() is not None and job[1]()

    def cancel_key(key):
        with lock:
            job = jobs.get(key)
            if job is not None and not job[0].done():
                job[1].set()
                state = 'cancelling'
            elif job is not None and cancelled(job):
                state = 'cancelled'
            else:
                raise HTTPException(404, 'no index being built under this key')
        return {'key': key, 'state': state}

    @app.post('/api/v1/cancel')
    async def cancel(request: Request):
        """Stop an index being built (docs/viewer-api.md, cancel)."""
        req = _json_body(await _body(request, MAX_API_BODY), {'key': (str, None)})
        return await run_in_threadpool(cancel_key, req['key'])

    @app.get('/api/v1/datasets/{key}')
    def describe(key: str):
        job = jobs.get(key)
        if job is not None and not job[0].done():
            prog = read_progress(settings.cache_dir, key) or {'state': 'indexing', 'fraction': 0}
            return {'state': 'indexing', 'progress': prog}
        if job is not None and cancelled(job):
            return {'state': 'cancelled'}
        if job is not None and job[0].exception() is not None:
            e = job[0].exception()
            # a worker that died while indexing: the index folder is removed on the way out
            why = 'indexing stopped on an internal error' if isinstance(e, BrokenProcessPool) else str(e)
            return {'state': 'error', 'error': why}
        ds = dataset(key)
        if ds is None:
            raise HTTPException(404, 'dataset not indexed')
        return {'state': 'ready', 'dataset': ds.info()}

    def channel_count(ds):
        return len(ds.meta.get('channels') or [])

    @app.get('/api/v1/datasets/{key}/window')
    def window(key: str, request: Request):
        q = _query(request)
        t0, t1 = _q_span(q)
        px = _q_i64(q, 'px', 1200)
        ds = need(key)
        ch = _q_channels(q, channel_count(ds))
        try:
            text = ds.window_json(ch, t0, t1, px)
        except Exception as e:
            raise HTTPException(500, str(e))
        return Response(text, media_type='application/json')

    @app.get('/api/v1/datasets/{key}/vitals')
    def vitals(key: str, request: Request):
        q = _query(request)
        t0, t1 = _q_span(q)
        px = max(16, min(_q_i64(q, 'px', 1200), 8000))
        names = []
        for n in (q.get('names') or '').split('|'):
            if n and n not in names:
                names.append(n)
        if len(names) > MAX_SERIES:
            raise HTTPException(400, f'at most {MAX_SERIES} vital-sign series at once')
        return need(key).vitals_window(names, t0, t1, px)

    @app.get('/api/v1/datasets/{key}/vitals_at')
    def vitals_at(key: str, request: Request):
        t = _q_f64(_query(request), 't')
        return need(key).vitals_at(t)

    @app.get('/api/v1/datasets/{key}/markers')
    def markers(key: str, request: Request):
        q = _query(request)
        t0, t1 = _q_span(q)
        ds = need(key)
        return ds.markers(_q_channels(q, channel_count(ds)), t0, t1)

    @app.get('/')
    def index(request: Request):
        if logged_in(request):
            return FileResponse(STATIC / 'index.html', headers={'Cache-Control': 'no-cache'})
        return HTMLResponse(LOGIN_PAGE.replace('{error}', ''), status_code=401)

    def login_delay(client):
        """The delay before a password from `client` is checked: none, then 1, 3, 7, 15 s ... (at
        most MAX_LOGIN_DELAY_S) after wrong passwords in the last FAILURES_KEPT seconds."""
        now = time.monotonic()
        for c in [c for c, (_, last) in failures.items() if now - last >= FAILURES_KEPT]:
            del failures[c]
        n = failures.get(client, (0, 0))[0]
        return min((1 << min(n, 6)) - 1, MAX_LOGIN_DELAY_S)

    def login_failed(client):
        if len(failures) >= 10_000 and client not in failures:
            failures.clear()                 # many addresses: start again rather than grow without bound
        failures[client] = (failures.get(client, (0, 0))[0] + 1, time.monotonic())

    app.state.login_delay, app.state.login_failed = login_delay, login_failed

    def cookie_options():
        return dict(path='/', httponly=True, samesite='lax', secure=settings.secure_cookie)

    @app.post('/login')
    async def login(request: Request):
        if not protected:
            return RedirectResponse('./', status_code=303)
        client = request.client.host if request.client else '?'
        password = _form_value(await _body(request, MAX_LOGIN_BODY), 'password')
        await asyncio.sleep(login_delay(client))
        async with checking:
            try:
                good = await asyncio.to_thread(auth.check_password, creds, password)
            except Exception:
                good = False
        if not good:
            login_failed(client)
            await asyncio.sleep(1.0)                  # slow down guessing
            return HTMLResponse(LOGIN_PAGE.replace('{error}', '<p class="err">Wrong password.</p>'),
                                status_code=401)
        failures.pop(client, None)
        r = RedirectResponse('./', status_code=303)
        r.set_cookie(cookie, auth.make_cookie(creds), max_age=auth.LOGIN_DAYS * 86400, **cookie_options())
        return r

    @app.post('/logout')
    async def logout(request: Request):
        """Remove the cookie; a valid sign-in presented here is also ended on the server, so that
        a copy of the cookie no longer works (kept in the auth file: docs/viewer-api.md,
        "Sign-in")."""
        value = cookie_value(request)
        if protected and logged_in(request) and value and '.' in value:
            expires, sig = value.split('.', 1)
            if expires.isascii() and expires.isdigit():
                revoked[sig] = int(expires)
                try:
                    await asyncio.to_thread(auth.save_revoked, settings.auth_file, dict(revoked))
                except (OSError, ValueError):
                    pass
        r = RedirectResponse('./', status_code=303)
        r.delete_cookie(cookie, **cookie_options())
        return r

    app.mount('/static', StaticFiles(directory=STATIC), name='static')
    return app
