"""Adapters: separate programs that read recording formats the viewer does not read itself
(licensed or vendor formats), as specified in docs/adapters.md; the Rust engine and the C++
viewer speak the same protocol.

An adapter is an executable named `<name>-adapter` in an adapter folder.  The viewer asks it to
describe itself, to index a recording into the index format (docs/index-format.md), and, in a
`serve` session kept open, for the samples of an indexed recording.
"""

import json
import os
import queue
import signal
import subprocess
import sys
import threading
import time
import unicodedata
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from .common import json_loads

PROTOCOL = 1

# The environment variables a child process gets (all others are removed: the programs may hold
# secrets in theirs); LC_* too.  On Windows, names are compared ignoring case.
ENV_ALLOWED = ('PATH', 'HOME', 'USERPROFILE', 'TMP', 'TEMP', 'TMPDIR', 'LANG', 'SYSTEMROOT', 'SYSTEMDRIVE', 'WINDIR',
               'COMSPEC', 'PATHEXT', 'LOCALAPPDATA', 'APPDATA')
STDERR_KEPT = 64 * 1024          # the bytes of an adapter's standard error kept (the last ones); never printed
MAX_LINE = 1 << 20               # the longest line of an adapter's answer read
MAX_OUTPUT = 16 << 20            # the most an answer of `describe` may hold
READ_CHUNK = 1 << 20             # the samples asked of a session in one request; longer reads are split


@dataclass
class Timeouts:
    """Time limits on an adapter's commands, in seconds (docs/adapters.md, "Limits and
    environment")."""
    describe: float = 30.0       # the whole command
    index_stall: float = 300.0   # the longest time without a line of output (or, after it ends, until exit)
    serve_stall: float = 60.0    # the longest wait for the next bytes of an answer


_timeouts = Timeouts()


def set_timeouts(t):
    """Use these time limits from now on (tests; the defaults otherwise)."""
    global _timeouts
    _timeouts = t


def timeouts():
    return _timeouts


def _env():
    """The environment of a child process: the allowed variables only."""
    def ok(name):
        n = name.upper() if os.name == 'nt' else name
        return n in ENV_ALLOWED or n.startswith('LC_')
    return {k: v for k, v in os.environ.items() if ok(k)}


def _spawn(args, stdin=subprocess.DEVNULL):
    return subprocess.Popen(args, stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=_env())


class _Stderr:
    """The last bytes of a child's standard error, read by a thread of their own (so that a child
    that writes much there never blocks)."""

    def __init__(self, p):
        self.kept = deque()
        self.size = 0
        self.lock = threading.Lock()
        threading.Thread(target=self._drain, args=(p.stderr,), daemon=True).start()

    def _drain(self, f):
        while True:
            try:
                b = f.read1(8192)
            except (OSError, ValueError):
                break
            if not b:
                break
            with self.lock:
                self.kept.append(b)
                self.size += len(b)
                while self.size - len(self.kept[0]) >= STDERR_KEPT:
                    self.size -= len(self.kept.popleft())

    def last_line(self):
        """Its last line that is not blank, at most 200 characters, control characters removed:
        for an error message (never printed)."""
        with self.lock:
            data = b''.join(self.kept)[-STDERR_KEPT:]
        lines = [ln.strip() for ln in data.decode('utf-8', 'replace').splitlines()]
        line = next((ln for ln in reversed(lines) if ln), '')
        return ''.join(c for c in line if unicodedata.category(c) != 'Cc')[:200]


def _stop(p):
    """Stop a child (and collect it)."""
    try:
        p.kill()
    except OSError:
        pass
    try:
        p.wait()
    except OSError:
        pass


def _status(code):
    """A child's exit as Rust's ExitStatus shows it."""
    if code is not None and code < 0:
        try:
            return f'signal: {-code} ({signal.Signals(-code).name})'
        except ValueError:
            return f'signal: {-code}'
    return f'exit status: {code}'


def _secs(x):
    return str(int(x)) if float(x).is_integer() else repr(float(x))


def _lines(out):
    """A queue of the lines of `out` (each at most MAX_LINE bytes; a longer one ends the stream
    with an error string), then None at its end; read by a thread of its own."""
    q = queue.Queue(64)

    def run():
        while True:
            try:
                line = out.readline(MAX_LINE + 1)
            except (OSError, ValueError):
                break
            if not line:
                break
            if len(line) > MAX_LINE:
                q.put('an adapter wrote a line longer than 1 MiB')
                break
            q.put(line)
        q.put(None)
    threading.Thread(target=run, daemon=True).start()
    return q


def _wait_until(p, deadline):
    """The exit status of a child that exits before `deadline` (time.monotonic), else None (it is
    stopped)."""
    try:
        return p.wait(timeout=max(0.0, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        _stop(p)
        return None


@dataclass
class Format:
    kind: str                       # the recording kind written in the index (meta.kind)
    label: str
    extensions: list = field(default_factory=list)    # lower case, without the dot


@dataclass
class Adapter:
    name: str
    version: str
    program: str
    formats: list = field(default_factory=list)


def default_folder():
    """~/.local/share/medorama/adapters (Windows: %LOCALAPPDATA%\\medorama\\adapters)."""
    if os.name == 'nt' and os.environ.get('LOCALAPPDATA'):
        return str(Path(os.environ['LOCALAPPDATA']) / 'medorama' / 'adapters')
    return str(Path.home() / '.local' / 'share' / 'medorama' / 'adapters')


_lock = threading.Lock()
_folders = [default_folder()]
_found = None


def set_folders(dirs):
    """The folders searched for adapters, in order (the setting adapters_dir)."""
    global _folders, _found
    with _lock:
        _folders = [str(d) for d in dirs]
        _found = None


def _is_adapter(name):
    n = name.lower()
    return n.endswith('-adapter') or n.endswith('-adapter.exe')


def _run(program, args, limit, tell=True):
    """Run `program` with `args` within `limit` seconds; its standard output.  The error says why,
    with the last line of its standard error when `tell`."""
    deadline = time.monotonic() + limit
    name = os.path.basename(program)
    try:
        p = _spawn([program, *args])
    except OSError as e:
        raise RuntimeError(f'{program}: {e}') from e
    err = _Stderr(p)
    got = []
    reader = threading.Thread(target=lambda: got.append(p.stdout.read(MAX_OUTPUT + 1)), daemon=True)
    reader.start()
    reader.join(max(0.0, deadline - time.monotonic()))
    if reader.is_alive():
        _stop(p)
        raise RuntimeError(f'{name} did not answer within {_secs(limit)} s: stopped')
    text = got[0] if got else b''
    if len(text) > MAX_OUTPUT:
        _stop(p)
        raise RuntimeError(f'{name} answered with more than 16 MiB')
    status = _wait_until(p, deadline)
    if status is None:
        raise RuntimeError(f'{name} did not exit within {_secs(limit)} s: stopped')
    if status != 0:
        why = err.last_line() if tell else ''
        raise RuntimeError(f'{name} failed ({_status(status)})' + (f': {why}' if why else ''))
    return text.decode('utf-8', 'replace')


def _describe(program):
    d = json_loads(_run(program, ['describe'], timeouts().describe))
    if not isinstance(d, dict):
        raise ValueError('an adapter answered with something other than a JSON object')
    if d.get('protocol') != PROTOCOL:
        raise ValueError(f'speaks protocol {d.get("protocol")}; this program speaks {PROTOCOL}')
    formats = []
    for f in d.get('formats') or []:
        if isinstance(f, dict) and f.get('kind'):
            exts = [str(e).lower().lstrip('.') for e in f.get('extensions') or []]
            formats.append(Format(str(f['kind']), str(f.get('label') or ''), exts))
    return Adapter(str(d.get('name') or ''), str(d.get('version') or ''), program, formats)


def installed():
    """The adapters installed, each described once."""
    global _found
    with _lock:
        if _found is not None:
            return list(_found)
        out = []
        for folder in _folders:
            try:
                names = sorted(os.listdir(folder))
            except OSError:
                continue
            for n in names:
                p = os.path.join(folder, n)
                if not (_is_adapter(n) and os.path.isfile(p)):
                    continue
                try:
                    a = _describe(p)
                except Exception as e:          # an adapter that cannot describe itself is skipped
                    print(f'adapter {p}: {e}', file=sys.stderr)
                    continue
                if all(x.name != a.name for x in out):
                    out.append(a)
        _found = out
        return list(out)


def for_file(path):
    """(adapter, format) for a file, by its extension, or None."""
    ext = Path(path).suffix.lower().lstrip('.')
    if not ext:
        return None
    for a in installed():
        for f in a.formats:
            if ext in f.extensions:
                return a, f
    return None


def named(name):
    return next((a for a in installed() if a.name == name), None)


def index(adapter, path, folder, progress, cancelled=lambda: False):
    """Write the index files of `path` into `folder` (the adapter's `index`), reporting progress.
    The adapter is stopped once `cancelled` returns true (checked ten times a second), and when
    it writes no line for the time limit."""
    stall = timeouts().index_stall
    try:
        p = _spawn([adapter.program, 'index', str(path), str(folder)])
    except OSError as e:
        raise RuntimeError(f'{adapter.program}: {e}') from e

    def fail(why):
        return RuntimeError(f'{adapter.name} could not index {path}: {why}')
    err = _Stderr(p)
    lines = _lines(p.stdout)
    done, deadline = False, time.monotonic() + stall
    try:
        while True:
            if cancelled():
                _stop(p)
                raise ValueError('indexing cancelled')
            try:
                line = lines.get(timeout=max(0.0, min(0.1, deadline - time.monotonic())))
            except queue.Empty:
                if time.monotonic() >= deadline:
                    _stop(p)
                    raise fail(f'no answer for {_secs(stall)} s: stopped') from None
                continue
            if line is None:
                break
            if isinstance(line, str):
                _stop(p)
                raise fail(line)
            deadline = time.monotonic() + stall
            try:
                d = json.loads(line)
            except (ValueError, RecursionError):
                continue
            if isinstance(d, dict):
                f = d.get('progress')
                if isinstance(f, (int, float)) and not isinstance(f, bool):
                    progress(0.0 if f != f else min(1.0, max(0.0, float(f))))
                if d.get('done') is True:
                    done = True
        status = _wait_until(p, time.monotonic() + stall)
        if status is None:
            raise fail(f'it did not exit within {_secs(stall)} s: stopped')
    except BaseException:
        if p.poll() is None:
            _stop(p)
        raise
    if status != 0 or not done:
        why = err.last_line()
        raise fail(why or f'it failed ({_status(status)})')


class _Answer(Exception):
    """The adapter answered {"error": ...}: the session stays in step."""


class _Ended(Exception):
    """The session ended before answering."""


class _Protocol(Exception):
    """A malformed answer, or none in time: the session is out of step."""


class _Session:
    """A running `serve` session of one adapter: its standard output read by a thread of its
    own, in chunks."""

    def __init__(self, adapter):
        self.p = _spawn([adapter.program, 'serve'], stdin=subprocess.PIPE)
        _Stderr(self.p)                          # kept, never shown: it could carry identifiers
        # a few chunks ahead at most: an adapter that writes more than it was asked waits
        self.out = queue.Queue(16)
        self.buf = bytearray()
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while True:
            try:
                b = self.p.stdout.read1(1 << 16)
            except (OSError, ValueError):
                b = b''
            self.out.put(b)
            if not b:
                break

    def ended(self):
        return self.p.poll() is not None

    def _more(self):
        """More bytes of the answer, waiting at most the time limit; False at its end."""
        stall = timeouts().serve_stall
        try:
            b = self.out.get(timeout=stall)
        except queue.Empty:
            raise _Protocol(f'no answer for {_secs(stall)} s') from None
        if not b:
            self.out.put(b)                      # the end, for the next reader too
            return False
        self.buf += b
        return True

    def read(self, request, want):
        """The samples answered to `request`, at most `want` of them."""
        while True:
            try:
                b = self.out.get_nowait()
            except queue.Empty:
                break
            if not b:
                self.out.put(b)
                break
            self.buf += b
        if self.buf:
            raise _Protocol('the adapter wrote more than it was asked for')
        try:
            self.p.stdin.write(request)
            self.p.stdin.flush()
        except (OSError, ValueError):
            raise _Ended from None
        while True:
            i = self.buf.find(b'\n')
            if i >= 0:
                line = bytes(self.buf[:i + 1])
                del self.buf[:i + 1]
                break
            if len(self.buf) > MAX_LINE:
                raise _Protocol('an answer line longer than 1 MiB')
            if not self._more():
                if self.buf:
                    raise _Protocol('the session ended inside an answer')
                raise _Ended
        try:
            d = json_loads(line)
        except (ValueError, RecursionError) as e:
            raise _Protocol(str(e)) from None
        if not isinstance(d, dict):
            raise _Protocol('an adapter answered with something other than a JSON object')
        if isinstance(d.get('error'), str):
            raise _Answer(d['error'])
        n = d.get('n')
        if not (isinstance(n, int) and not isinstance(n, bool) and 0 <= n <= want):
            raise _Protocol(f'an answer of {n} samples, for {want} asked')
        while len(self.buf) < 8 * n:
            if not self._more():
                raise _Protocol('the session ended inside an answer')
        x = np.frombuffer(bytes(self.buf[:8 * n]), '<f8').astype(np.float64)
        del self.buf[:8 * n]
        if self.buf:
            raise _Protocol('the adapter wrote more than it was asked for')
        return x

    def close(self):
        _stop(self.p)


_sessions = {}                   # adapter name -> [session or None, its lock]
_sessions_lock = threading.Lock()


def read(adapter, index_dir, piece, a, b):
    """Physical samples [a, b) of a piece of the recording indexed in `index_dir`, from the
    adapter's `serve` session (started once, kept open), in requests of at most READ_CHUNK
    samples."""
    out = []
    at = max(int(a), 0)
    while at < b:
        to = min(int(b), at + READ_CHUNK)
        x = _read_chunk(adapter, index_dir, piece, at, to)
        out.append(x)
        if x.size < to - at:
            break                        # the piece ends here
        at = to
    return np.concatenate(out) if out else np.empty(0, np.float64)


def _read_chunk(adapter, index_dir, piece, a, b):
    request = (json.dumps({'op': 'read', 'index': str(index_dir), 'piece': int(piece), 'a': int(a), 'b': int(b)},
                          separators=(',', ':')) + '\n').encode()
    # this adapter's session only: the others are not held up while it answers
    with _sessions_lock:
        slot = _sessions.setdefault(adapter.name, [None, threading.Lock()])
    with slot[1]:
        # a session that ended (the adapter was updated, or failed) is started again, once
        for attempt in range(2):
            if slot[0] is None or slot[0].ended():
                if slot[0] is not None:
                    slot[0].close()
                try:
                    slot[0] = _Session(adapter)
                except OSError as e:
                    slot[0] = None
                    raise RuntimeError(f'{adapter.program}: {e}') from e
            s = slot[0]
            try:
                return s.read(request, b - a)
            except _Answer as e:
                raise RuntimeError(str(e)) from None
            except _Ended:
                s.close()
                slot[0] = None
                if attempt:
                    raise RuntimeError(f'{adapter.name}: its session ended') from None
            except _Protocol as e:
                s.close()
                slot[0] = None
                raise RuntimeError(f'{adapter.name}: {e}; its session was stopped') from None
    raise RuntimeError(f'{adapter.name}: its session could not be started')
