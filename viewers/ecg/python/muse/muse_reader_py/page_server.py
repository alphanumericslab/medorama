"""The viewer's page served from this computer, with the folders under a root (`serve`).

The ECG and PACS viewers are pages that read the files they are given in the browser. Served
this way, they also open the files of the computer that serves them: on a server where the
data are, reached through an SSH tunnel, like VitalStream. The same module in muse-py (the ECG
viewer) and dicom-py (the PACS viewer).

- **Root:** only files and folders under the root are listed and served, every symbolic link
  resolved (a link that leads outside is refused); hidden entries (names that start with `.`)
  are neither listed nor served, nor anything reached through one (a link to a hidden folder
  included). The root is `--root` (default: the home folder), inside the root the package was
  installed with (`VIEWER_ROOT` at install time; default: the whole file system), never outside it.
- **Host:** a request must name this server in its `Host`: a loopback name, an IP address, or
  `--host`. A page elsewhere whose name is made to lead here (DNS rebinding) is refused.
- **Password** (optional, the same file and scheme as VitalStream's: one password for the
  viewers): set once with `--set-password`; the page then asks for it. A sign-in lasts 30 days,
  until the server stops, or until a browser signs out (`POST logout`), which ends every
  sign-in: the cookie is signed with a key of this run as well, so it opens only this server
  while it runs (cookies do not tell ports apart). After a wrong password, the next try, from
  any browser, waits 1 s, then 2, 4 … up to 60 s; a right one resets the wait. A package
  installed with `VIEWER_PASSWORD=0` has no password support.
- **Files** are sent as they are read, in part when asked (`Range: bytes=…`, one range), under a
  policy that runs nothing in them (`Content-Security-Policy: sandbox`); a kind of file the
  viewers do not read is sent as a download (`application/octet-stream`). The page has a
  strict policy too (its own scripts, styles and pictures; blob: for what it shows).
- **Requests:** `GET /` the page; `GET browse?path=` a folder's records and subfolders (HTML);
  `GET files/<path>` a file; `GET list/<path>/` a folder's files as `{"files": [...]}` (the PACS
  viewer's `#list=`), and `list/<path>` without a final `/` the file itself (a listed file, read
  at the listing's address + its name); `GET served` answers 200 `{"viewer", "root"}` (the page
  then shows its **Server folders…** button). A listing holds at most 5000 entries
  (`"truncated": true` when there are more).

Only the Python standard library is used.
"""

import hashlib
import hmac
import html
import json
import ipaddress
import mimetypes
import os
import re
import secrets
import socket
import threading
import time
import urllib.parse
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

try:  # written when the package is built (setup.py): the root and password support it was installed with
    from . import _build
except ImportError:  # the source tree
    _build = None

LOGIN_DAYS = 30
_SCRYPT = dict(n=2 ** 14, r=8, p=1, dklen=32)
AUTH_FILE = '~/.config/vitalstream/auth.json'
PASSWORD = getattr(_build, 'PASSWORD', True)
# What each viewer lists as a record (by extension) besides folders.
RECORDS = {
    'ecg': {'.hea', '.xml', '.json', '.dcm', '.dicom', '.edf', '.bdf', '.pdf', '.png', '.jpg', '.jpeg', '.gif', '.webp'},
    'pacs': {'.dcm', '.dicom', '.mp4', '.webm', '.mov', '.png', '.jpg', '.jpeg', '.pdf', ''},
}
MAX_ENTRIES = 5000
MAX_LOGIN = 4096        # the largest sign-in form read
BACKOFF_MAX = 60        # the longest wait (s) after wrong passwords
CHUNK = 1 << 20
LOOPBACK = {'localhost', '127.0.0.1', '::1'}
# The types /files/ sends files under (what the viewers read); any other file is a download.
FILE_TYPES = {
    '.dcm': 'application/dicom', '.dicom': 'application/dicom', '.pdf': 'application/pdf',
    '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.gif': 'image/gif', '.webp': 'image/webp',
    '.bmp': 'image/bmp', '.mp4': 'video/mp4', '.m4v': 'video/mp4', '.webm': 'video/webm', '.mov': 'video/quicktime',
    '.ogv': 'video/ogg', '.xml': 'application/xml', '.json': 'application/json', '.hea': 'text/plain; charset=utf-8',
}
# Policies: the files under the root run nothing; the viewer's page runs its own scripts and shows
# what it reads from blob: addresses (as under Medorama); the server's own pages are plain HTML.
FILE_CSP = "sandbox; default-src 'none'"
PAGE_CSP = ("default-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; frame-src 'self' blob:; "
            "object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'")
OWN_CSP = "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"


def built_root():
    """The root the package was installed with (`VIEWER_ROOT`; `~` is the home folder of whoever
    runs it), else the whole file system."""
    r = getattr(_build, 'ROOT', '') or os.path.abspath(os.sep)
    return os.path.realpath(os.path.expanduser(r))


def within(path, root):
    return path == root or path.startswith(root.rstrip(os.sep) + os.sep)


def hidden(rel):
    """Whether a relative path names a hidden entry on its way (`/` and `\\` both separate)."""
    return any(c.startswith('.') for c in re.split(r'[\\/]', rel) if c)


def byte_range(value, size):
    """The part of a file of `size` bytes a `Range` header asks for, as (start, end): None for the
    whole file (no header, or one not served: other units, several ranges, a malformed one),
    False when it lies outside the file."""
    unit, _, spec = (value or '').partition('=')
    if unit.strip().lower() != 'bytes' or ',' in spec:
        return None
    a, dash, b = spec.strip().partition('-')
    if not dash or not (a or b) or not (a + b).isascii() or not (a + b).isdigit():
        return None
    if not a:  # the last b bytes
        n = int(b)
        return (max(0, size - n), size) if n and size else False
    start = int(a)
    if b and int(b) < start:
        return None
    return (start, min(int(b) + 1 if b else size, size)) if start < size else False


# ---------------------------------------------------------------- the password (VitalStream's scheme)
def load_auth(path):
    """The auth file's contents, or None when no password is set (or none is supported)."""
    if not PASSWORD:
        return None
    try:
        with open(os.path.expanduser(path), encoding='utf-8') as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    return data if data.get('hash') and data.get('secret') else None


def set_password(path, password):
    if not PASSWORD:
        raise OSError('this viewer was installed without password support')
    path = os.path.expanduser(path)
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    salt = secrets.token_bytes(16)
    data = {'scheme': 'scrypt', 'salt': salt.hex(),
            'hash': hashlib.scrypt(password.encode(), salt=salt, **_SCRYPT).hex(),
            'secret': secrets.token_hex(32)}
    tmp = path + '.tmp'
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8', newline='\n') as f:
        json.dump(data, f)
    os.replace(tmp, path)


def check_password(auth, password):
    h = hashlib.scrypt(password.encode(), salt=bytes.fromhex(auth['salt']), **_SCRYPT).hex()
    return hmac.compare_digest(h, auth['hash'])


def _sign(auth, key, expires):
    """The signature of a sign-in: the auth file's secret and the server's key of this run."""
    return hmac.new(bytes.fromhex(auth['secret']) + key, str(expires).encode(), hashlib.sha256).hexdigest()


def make_cookie(auth, key):
    expires = int(time.time() + LOGIN_DAYS * 86400)
    return f'{expires}.{_sign(auth, key, expires)}'


def cookie_ok(auth, key, value):
    try:
        expires, sig = str(value).split('.', 1)
        good = _sign(auth, key, int(expires)).encode()
        return int(expires) > time.time() and hmac.compare_digest(sig.encode('utf-8', 'replace'), good)
    except (ValueError, AttributeError, TypeError):
        return False


LOGIN_PAGE = '''<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>{title}</title>
<style>:root{{color-scheme:light dark}}body{{font:14px -apple-system,Segoe UI,Roboto,Arial,sans-serif;display:grid;
place-items:center;height:100vh;margin:0}}form{{border:1px solid #8884;border-radius:12px;padding:24px;width:min(380px,90vw)}}
h1{{font-size:18px;margin:0 0 14px}}input{{width:100%;box-sizing:border-box;padding:8px;border-radius:7px;border:1px solid #8886;
margin:0 0 12px}}button{{padding:7px 14px;border-radius:7px;border:0;background:#2563eb;color:#fff;font-weight:600}}
.err{{font-weight:600;margin:0 0 10px}}</style></head><body>
<form method="post" action="login"><h1>{title}</h1>{error}
<input type="password" name="password" placeholder="Password" autofocus autocomplete="current-password">
<button>Sign in</button></form></body></html>'''

BROWSE_PAGE = '''<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>{title}: {folder}</title>
<style>:root{{color-scheme:light dark;--line:#8883;--muted:#8a94a3}}body{{font:14px -apple-system,Segoe UI,Roboto,Arial,sans-serif;
max-width:900px;margin:24px auto;padding:0 16px}}h1{{font-size:17px;margin:0 0 4px}}.path{{color:var(--muted);margin:0 0 14px;
word-break:break-all}}ul{{list-style:none;padding:0;margin:0}}li{{display:flex;gap:10px;align-items:center;padding:7px 8px;
border-bottom:1px solid var(--line)}}li a{{text-decoration:none}}.k{{margin-left:auto;color:var(--muted);font-size:12px}}
.btn{{border:1px solid var(--line);border-radius:7px;padding:4px 10px;font-size:12px}}</style></head><body>
<h1>{title}: server folders</h1><p class="path">{folder}</p>{actions}<ul>{items}</ul></body></html>'''


# ---------------------------------------------------------------- the server
class PageServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, viewer, page_dir, root, auth):
        if ':' in address[0]:
            self.address_family = socket.AF_INET6
        super().__init__(address, Handler)
        self.viewer, self.page_dir, self.root, self.auth = viewer, Path(page_dir), root, auth
        self.title = {'ecg': 'ECG viewer', 'pacs': 'PACS viewer'}[viewer]
        self.cookie = f'viewer_{viewer}_{address[1]}'
        # the names a request's Host may give: loopback names and --host (IP addresses as well)
        self.hosts = LOOPBACK | ({address[0].strip('[]').lower()} - {''})
        # sign-ins: signed with this run's key too (a new key signs every browser out); one count
        # of wrong passwords for the whole server, whatever browser they come from
        self.key = secrets.token_bytes(32)
        self.lock = threading.Lock()
        self.failures, self.retry_at = 0, 0.0

    def try_password(self, password):
        """'right', or ('wrong' | 'early', seconds until the next try). A try before then is
        'early', refused unchecked; after the n-th wrong one in a row, the next waits 2**(n-1) s,
        at most BACKOFF_MAX."""
        with self.lock:
            now = time.monotonic()
            if now < self.retry_at:
                return 'early', self.retry_at - now
            if check_password(self.auth, password):
                self.failures = 0
                return 'right', 0
            self.failures += 1
            wait = min(2 ** min(self.failures - 1, 16), BACKOFF_MAX)
            self.retry_at = now + wait
            return 'wrong', wait

    def sign_out(self):
        """End every sign-in: a new key."""
        with self.lock:
            self.key = secrets.token_bytes(32)


class Handler(BaseHTTPRequestHandler):
    server: PageServer

    def log_message(self, fmt, *args):  # quiet: no paths in a log
        pass

    # -------------------------------------------------- answers
    def head(self, code, ctype, length, headers=(), csp=OWN_CSP):
        self.send_response(code)
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(length))
        self.send_header('Cache-Control', 'no-cache')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Referrer-Policy', 'no-referrer')
        self.send_header('Content-Security-Policy', csp)
        for k, v in headers:
            self.send_header(k, v)
        if self.close_connection:
            self.send_header('Connection', 'close')
        self.end_headers()

    def send(self, code, body, ctype='text/html; charset=utf-8', headers=(), csp=OWN_CSP):
        data = body.encode() if isinstance(body, str) else body
        self.head(code, ctype, len(data), headers, csp)
        if self.command != 'HEAD':
            self.wfile.write(data)

    def json(self, code, obj):
        self.send(code, json.dumps(obj), 'application/json')

    def refuse(self, code, why):
        self.json(code, {'detail': why})

    def signed_in(self):
        a = self.server.auth
        if a is None:
            return True
        for part in (self.headers.get('Cookie') or '').split(';'):
            k, _, v = part.strip().partition('=')
            if k == self.server.cookie and cookie_ok(a, self.server.key, v):
                return True
        return False

    def host_ok(self):
        """Whether the request's Host names this server: a loopback name, an IP address, or --host."""
        try:
            name = urllib.parse.urlsplit('//' + (self.headers.get('Host') or '').strip()).hostname or ''
        except ValueError:
            return False
        if name in self.server.hosts:
            return True
        try:
            ipaddress.ip_address(name.split('%')[0])
            return True
        except ValueError:
            return False

    def shown(self, p):
        """Whether a real path lies under the root with no hidden entry on the way there."""
        root = self.server.root
        return within(p, root) and (p == root or not hidden(os.path.relpath(p, root)))

    def real(self, rel):
        """The real path of `rel` under the root, or None when it lies outside, does not exist, or
        names or leads through a hidden entry (a name that starts with `.`)."""
        if '\0' in rel or hidden(rel):
            return None
        p = os.path.realpath(os.path.join(self.server.root, rel.lstrip('/')))
        return p if self.shown(p) and os.path.exists(p) else None

    # -------------------------------------------------- requests
    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        url = urllib.parse.urlsplit(self.path)
        path = urllib.parse.unquote(url.path)
        if not self.host_ok():
            return self.refuse(421, 'this server is not reached by that name')
        if not self.signed_in():
            if path in ('/', '/index.html'):
                return self.send(401, LOGIN_PAGE.format(title=self.server.title, error=''))
            return self.refuse(401, 'sign in required')
        if path == '/served':
            return self.json(200, {'viewer': self.server.viewer, 'root': self.server.root})
        if path == '/browse':
            return self.browse(urllib.parse.parse_qs(url.query).get('path', [''])[0])
        if path.startswith('/files/'):
            return self.file(path[len('/files/'):])
        if path.startswith('/list/'):
            rel = path[len('/list/'):]
            return self.listing(rel) if rel.endswith('/') or rel == '' else self.file(rel)
        name = 'index.html' if path in ('/', '') else path.lstrip('/')
        f = (self.server.page_dir / name).resolve()
        if f.parent != self.server.page_dir.resolve() or not f.is_file() or f.name.startswith('.'):
            return self.refuse(404, 'Not Found')
        self.send(200, f.read_bytes(), mimetypes.guess_type(f.name)[0] or 'application/octet-stream', csp=PAGE_CSP)

    def do_POST(self):
        path = urllib.parse.urlsplit(self.path).path
        self.close_connection = True  # a body is read only for a sign-in
        if not self.host_ok():
            return self.refuse(421, 'this server is not reached by that name')
        if path == '/logout':
            if self.server.auth is not None and self.signed_in():
                self.server.sign_out()
            return self.send(303, '', headers=[('Location', './'), ('Set-Cookie', f'{self.server.cookie}=""; Max-Age=0; Path=/; SameSite=lax')])
        if path != '/login' or self.server.auth is None:
            return self.send(303, '', headers=[('Location', './')])
        try:
            n = int(self.headers.get('Content-Length', ''))
        except ValueError:
            n = -1
        if not 0 <= n <= MAX_LOGIN:
            return self.refuse(413 if n > MAX_LOGIN else 400, 'a sign-in is a short form with its Content-Length')
        form = urllib.parse.parse_qs(self.rfile.read(n).decode('utf-8', 'replace'))
        got, wait = self.server.try_password(form.get('password', [''])[0])
        if got != 'right':
            s = max(1, int(wait + 0.999))
            error = ('Wrong password.' if got == 'wrong' else 'Too many wrong passwords: the password was not checked.') + f' Try again in {s} s.'
            return self.send(401 if got == 'wrong' else 429, LOGIN_PAGE.format(title=self.server.title, error=f'<p class="err">{error}</p>'),
                             headers=[('Retry-After', str(s))])
        cookie = f'{self.server.cookie}={make_cookie(self.server.auth, self.server.key)}; HttpOnly; Max-Age={LOGIN_DAYS * 86400}; Path=/; SameSite=lax'
        self.send(303, '', headers=[('Location', './'), ('Set-Cookie', cookie)])

    def file(self, rel):
        p = self.real(rel)
        if not p or not os.path.isfile(p):
            return self.refuse(403 if p is None else 404, 'not a file under the root')
        ctype = FILE_TYPES.get(os.path.splitext(p)[1].lower())
        headers = [('Accept-Ranges', 'bytes')] + ([] if ctype else [('Content-Disposition', 'attachment')])
        with open(p, 'rb') as f:
            size = os.fstat(f.fileno()).st_size
            part = byte_range(self.headers.get('Range'), size)
            if part is False:
                return self.send(416, '', 'text/plain', headers=[('Content-Range', f'bytes */{size}')], csp=FILE_CSP)
            start, end = part or (0, size)
            if part:
                headers.append(('Content-Range', f'bytes {start}-{end - 1}/{size}'))
            self.head(206 if part else 200, ctype or 'application/octet-stream', end - start, headers, FILE_CSP)
            if self.command == 'HEAD':
                return
            f.seek(start)
            left = end - start
            while left > 0:
                data = f.read(min(CHUNK, left))
                if not data:
                    break
                self.wfile.write(data)
                left -= len(data)

    def listing(self, rel):
        p = self.real(rel)
        if not p or not os.path.isdir(p):
            return self.refuse(403, 'not a folder under the root')
        names = sorted(e.name for e in os.scandir(p) if e.is_file() and not e.name.startswith('.')
                       and self.shown(os.path.realpath(e.path)))
        self.json(200, {'files': names[:MAX_ENTRIES], 'truncated': len(names) > MAX_ENTRIES})

    def browse(self, rel):
        rel = rel.strip('/')
        p = self.real(rel)
        if not p or not os.path.isdir(p):
            return self.refuse(403, 'not a folder under the root')
        q = lambda r: urllib.parse.quote(r)  # noqa: E731
        items, records = [], 0
        if rel:
            items.append(f'<li>↰ <a href="browse?path={q(rel.rpartition("/")[0])}">..</a></li>')
        kinds = RECORDS[self.server.viewer]
        entries = sorted(os.scandir(p), key=lambda e: e.name.lower())
        for e in entries[:MAX_ENTRIES]:
            if e.name.startswith('.') or not self.shown(os.path.realpath(e.path)):
                continue
            r = f'{rel}/{e.name}' if rel else e.name
            name = html.escape(e.name)
            if e.is_dir():
                items.append(f'<li>📁 <a href="browse?path={q(r)}">{name}</a></li>')
            elif os.path.splitext(e.name)[1].lower() in kinds and not (self.server.viewer == 'ecg' and e.name.endswith('.dat')):
                records += 1
                frag = urllib.parse.urlencode({'src': f'files/{r}', 'name': e.name}, quote_via=urllib.parse.quote)
                items.append(f'<li>◆ <a href="./#{frag}">{name}</a><span class="k">{os.path.getsize(e.path):,} bytes</span></li>')
        actions = f'<p>Only the first {MAX_ENTRIES} of {len(entries)} entries are shown.</p>' if len(entries) > MAX_ENTRIES else ''
        if self.server.viewer == 'pacs' and records:
            frag = urllib.parse.urlencode({'list': f'list/{rel}/' if rel else 'list/'}, quote_via=urllib.parse.quote)
            actions += f'<p><a class="btn" href="./#{frag}">Open the records in this folder</a></p>'
        self.send(200, BROWSE_PAGE.format(title=self.server.title, folder=html.escape(p), actions=actions,
                                          items=''.join(items) or '<li>Nothing here.</li>'))


def page_dir(package_file):
    """The viewer's page: copied into the package when it is built, else the repository's."""
    here = Path(package_file).resolve().parent
    return here / 'page' if (here / 'page' / 'index.html').is_file() else here.parents[2]


def page_url(host, port):
    """The page's address (an IPv6 address in brackets)."""
    return f'http://[{host}]:{port}/' if ':' in host else f'http://{host}:{port}/'


def add_arguments(p, port):
    """The `serve` command's options."""
    p.add_argument('--root', help="folder to serve (default: the home folder), inside the package's built-in root")
    p.add_argument('--host', default='127.0.0.1', help='the address to listen on; requests must name it, a loopback name or an IP address')
    p.add_argument('--port', type=int, default=port)
    p.add_argument('--auth-file', default=AUTH_FILE, help=f'the password file (default {AUTH_FILE}, shared with VitalStream)')
    p.add_argument('--set-password', action='store_true', help='set (or change) the password the page asks for, then exit')
    p.add_argument('--clear-password', action='store_true', help='remove the password, then exit')
    p.add_argument('--no-auth', action='store_true', help='run without asking for the password')
    p.add_argument('--no-browser', action='store_true', help='do not open a browser window')


def run(args, viewer, package_file, prog):
    """The `serve` command: returns the exit status."""
    import getpass
    if args.set_password or args.clear_password:
        if not PASSWORD:
            print(f'{prog}: this viewer was installed without password support')
            return 1
        if args.clear_password:
            try:
                os.remove(os.path.expanduser(args.auth_file))
            except FileNotFoundError:
                pass
            print('Password removed.')
            return 0
        pw = getpass.getpass('New viewer password: ')
        if len(pw) < 8:
            print('Use at least 8 characters.')
            return 1
        if getpass.getpass('Repeat it: ') != pw:
            print('The two entries differ; nothing changed.')
            return 1
        set_password(args.auth_file, pw)
        print(f'Password set ({os.path.expanduser(args.auth_file)}). Restart the viewer for it to take effect.')
        return 0
    top = built_root()
    # installed without a root: the whole file system, every drive on Windows (where top is only
    # the current drive's root)
    whole = not getattr(_build, 'ROOT', '')
    root = os.path.realpath(os.path.expanduser(args.root or '~'))
    if within(top, root):
        root = top
    if not (whole or within(root, top)) or not os.path.isdir(root):
        print(f'{prog}: {root} is not a folder inside this viewer\'s root, {top} (set when it was installed)')
        return 1
    auth = None if args.no_auth else load_auth(args.auth_file)
    srv = PageServer((args.host, args.port), viewer, page_dir(package_file), root, auth)
    url = page_url(args.host, args.port)
    print(f'{srv.title}: open {url}  (server folders: {url}browse)')
    print(f'  root: {root}')
    if auth:
        print(f'  password protected ({prog} serve --set-password to change it)')
    else:
        print('  no password: anyone who can connect to this port can read the files under the root'
              + ('' if PASSWORD else ' (installed without password support)'))
    if not args.no_browser:
        threading.Timer(1.0, lambda: webbrowser.open(url + 'browse')).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0
