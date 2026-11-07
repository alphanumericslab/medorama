"""Settings: defaults, overridden by a TOML file, overridden by command-line options (the same
keys as the Rust and C++ viewers').

root          the viewer's root, fixed when it is installed (VITALSTREAM_ROOT at build time,
              written into the generated module _build.py by setup.py; '~' is the home folder
              of whoever runs the viewer; empty or unset, as when running from the source tree:
              the whole file system, the root of the home folder's drive): it never opens
              anything outside it.
data_roots    folders the viewer may open, inside its root.  Default: its root; the file
              browser starts in the home folder.  A root that contains the viewer's root is
              narrowed to it, and one outside it is left out (the settings narrow the root,
              never widen it).
max_records, max_depth, max_scan_s
              the limits on a folder of WFDB records (docs/wfdb-collection.md, "Limits"):
              5000 header files, 20 folder levels, 10 s to count them.
require_password
              refuse to start without a password (Medorama sets it for the viewer it runs; a
              build without password support then never starts).
cache_dir     the index cache, shared by the three viewers (they write the same index).
              Default, run from a clone of the repository: <viewer>/sample_data/outputs/index
              (sample_data/ is never pushed); installed elsewhere: ~/.cache/vitalstream.
adapters_dir  the folder of adapters, programs that read formats the viewer does not read
              itself (docs/adapters.md).  Default: ~/.local/share/medorama/adapters.
qc_verbose    also mark the irregularities a reader repaired (data blocks it put back in
              order); default false: what was repaired is not shaded.
auth_file     where the optional password is kept (see auth.py; shared by the three viewers);
              default ~/.config/vitalstream/auth.json.  Set it with `vitalstream-py --set-password`.
allowed_hosts host names the server answers for besides localhost, 127.0.0.1, ::1 and `host` (a
              reverse proxy's public name; "*": any), docs/viewer-api.md, "Requests".
secure_cookie send the sign-in cookie only over HTTPS (behind an HTTPS proxy).
Relative paths in a settings file are relative to that file's folder.

The settings file: --config FILE, else the first that exists of <viewer>/vitalstream.toml (only
when this package runs from a clone of the repository) and ~/.config/vitalstream/vitalstream.toml.
A vitalstream.toml in the working directory is never read on its own (a shared data folder could
hold one that runs programs through adapters_dir, or opens the viewer to the network).  The file
is shared by the three viewers (vitalstream-py, vitalstream-rs, vitalstream-cpp): top-level keys
apply to all of them, and the keys of a [py] section only to this one (sections [rs] and [cpp]
are ignored here).  A key of the wrong type is an error (data_roots must be a list of strings,
port an integer from 0 to 65535, ...).

Symbolic links: a path is allowed when the file or folder it leads to, every symbolic link
resolved, lies inside a data root.
"""

import os
from dataclasses import dataclass, field, fields
from pathlib import Path

from .adapters import default_folder

try:
    import tomllib
except ImportError:                    # Python < 3.11: the same parser, as the package tomli
    try:
        import tomli as tomllib
    except ImportError:
        tomllib = None

try:                                   # written by setup.py when the package is built
    from ._build import ROOT as BUILD_ROOT
except ImportError:                    # run from the source tree: the whole file system
    BUILD_ROOT = ''

REPO = Path(__file__).resolve().parents[3]          # <viewer>/python/vitalstream/vitalstream_py/
SAMPLE_DATA = REPO / 'sample_data'
# run from a clone of the repository (an editable install): this file lies in its viewer folder,
# never decided from the working directory, which may be anyone's folder
IN_REPO = ((REPO / 'python' / 'vitalstream' / 'pyproject.toml').is_file() and SAMPLE_DATA.is_dir()
           and (REPO / 'web').is_dir() and (REPO / 'rust' / 'Cargo.toml').is_file())

HOME = str(Path.home())
FS_ROOT = Path.home().anchor or '/'                  # the root of the home folder's drive
DEFAULT_CACHE = str(SAMPLE_DATA / 'outputs' / 'index') if IN_REPO else '~/.cache/vitalstream'
DEFAULT_FILES = ([str(REPO / 'vitalstream.toml')] if IN_REPO else []) + ['~/.config/vitalstream/vitalstream.toml']


def built_root():
    """The root fixed when this viewer was built (VITALSTREAM_ROOT; '~' is the home folder of
    whoever runs it), else the whole file system; absolute."""
    return absolute(BUILD_ROOT) if BUILD_ROOT else FS_ROOT


def _count(v):
    return isinstance(v, int) and not isinstance(v, bool) and v >= 0


def _seconds(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and v >= 0


def _port(v):
    return isinstance(v, int) and not isinstance(v, bool) and 0 <= v <= 65535


def _strings(v):
    return isinstance(v, list) and all(isinstance(x, str) for x in v)


def _hosts(v):
    return isinstance(v, list) and all(isinstance(x, str) and x.strip() for x in v)


def _text(v):
    return isinstance(v, str)


def _switch(v):
    return isinstance(v, bool)


# the settings and what they may hold
CHECKS = {'timezone': (_text, 'a string'),
          'data_roots': (_strings, 'a list of strings'),
          'cache_dir': (_text, 'a string'),
          'host': (_text, 'a string'),
          'port': (_port, 'an integer from 0 to 65535'),
          'index_workers': (_count, 'a whole number, 0 or more'),
          'auth_file': (_text, 'a string'),
          'adapters_dir': (_text, 'a string'),
          'qc_verbose': (_switch, 'true or false'),
          'require_password': (_switch, 'true or false'),
          'secure_cookie': (_switch, 'true or false'),
          'allowed_hosts': (_hosts, 'a list of host names'),
          'max_records': (_count, 'a whole number, 0 or more'),
          'max_depth': (_count, 'a whole number, 0 or more'),
          'max_scan_s': (_seconds, 'a number of seconds, 0 or more')}


@dataclass
class Settings:
    timezone: str = 'America/New_York'
    data_roots: list = field(default_factory=lambda: [built_root()])
    cache_dir: str = DEFAULT_CACHE
    host: str = '127.0.0.1'
    port: int = 8050
    index_workers: int = 2
    auth_file: str = '~/.config/vitalstream/auth.json'
    adapters_dir: str = ''
    qc_verbose: bool = False
    max_records: int = 5000
    max_depth: int = 20
    max_scan_s: float = 10.0
    require_password: bool = False
    allowed_hosts: list = field(default_factory=list)
    secure_cookie: bool = False
    # data roots left out because they lie outside the viewer's root
    outside: list = field(default_factory=list, init=False)
    # the settings file read, if any
    source: str = field(default=None, init=False)

    def __post_init__(self):
        for k, (ok, what) in CHECKS.items():
            if not ok(getattr(self, k)):
                raise ValueError(f'bad value for {k}: {getattr(self, k)!r} ({what})')
        self.max_scan_s = float(self.max_scan_s)
        # absolute roots inside the viewer's root
        top = built_root()
        real_top = _realpath(top)
        # built without a root: the whole file system, every drive on Windows (where top is only
        # the home folder's drive, the folder it shows first)
        whole = not BUILD_ROOT
        roots = []
        for r in (absolute(x) for x in self.data_roots):
            real = _realpath(r)
            if _within(real, real_top) or (whole and not _within(real_top, real)):
                kept = r
            elif _within(real_top, real):
                kept = top
            else:
                self.outside.append(r)
                continue
            if kept not in roots:
                roots.append(kept)
        if not roots:
            raise ValueError(f"no data root inside this viewer's root, {top} (set when it was built)")
        self.data_roots = roots
        self._real_roots = [_realpath(r) for r in self.data_roots]
        self.cache_dir = str(Path(os.path.expanduser(self.cache_dir)).resolve())
        self.auth_file = os.path.expanduser(self.auth_file)
        self.adapters_dir = absolute(self.adapters_dir) if self.adapters_dir else default_folder()
        try:
            from zoneinfo import ZoneInfo
            ZoneInfo(self.timezone)
        except Exception as e:
            raise ValueError(f'unknown time zone {self.timezone!r}') from e

    @property
    def limits(self):
        """The limits on a folder of WFDB records."""
        from .sources.wfdb_files import Limits
        return Limits(self.max_records, self.max_depth, self.max_scan_s)

    def allowed(self, path):
        """True if the file or folder `path` leads to (every symbolic link resolved) lies inside
        a data root."""
        r = _realpath(absolute(path))
        return any(_within(r, root) for root in self._real_roots)


def absolute(path):
    """Absolute, normalized path with ~ expanded; symbolic links are NOT resolved."""
    return os.path.normpath(os.path.abspath(os.path.expanduser(str(path))))


def _realpath(p):
    """The real path of an existing file or folder (every link resolved); others unchanged."""
    try:
        return str(Path(p).resolve(strict=True))
    except (OSError, RuntimeError):
        return p


def _within(path, root):
    r = root.rstrip('/' + os.sep)
    return path == root or not r or (path.startswith(r) and path[len(r):len(r) + 1] in ('/', os.sep))


def _relative_to(base, p):
    p = Path(os.path.expanduser(p))
    return str(p if p.is_absolute() else base / p)


def load_settings(path=None, **overrides):
    """Settings from `path` (or the first default file that exists) plus overrides."""
    values = {}
    source = None
    candidates = [path] if path else DEFAULT_FILES
    for c in candidates:
        p = Path(os.path.expanduser(c))
        if p.is_file():
            if tomllib is None:
                raise RuntimeError('reading a settings file needs Python 3.11+, or the package tomli (pip install tomli)')
            with open(p, 'rb') as f:
                values = tomllib.load(f)
            own = values.pop('py', {})          # this viewer's section overrides the shared keys
            values.pop('rs', None)
            values.pop('cpp', None)
            if not isinstance(own, dict):
                raise ValueError(f'{p}: [py] must be a table')
            values.update(own)
            base = p.resolve().parent
            for k, (ok, _) in CHECKS.items():
                if k in values and not ok(values[k]):
                    raise ValueError(f'{p}: bad value for {k}')
            if 'data_roots' in values:
                values['data_roots'] = [_relative_to(base, r) for r in values['data_roots']]
            for k in ('cache_dir', 'auth_file', 'adapters_dir'):
                if k in values:
                    values[k] = _relative_to(base, values[k])
            source = str(p.resolve())
            break
        if path:
            raise FileNotFoundError(f'{p}: no such settings file')
    known = {f.name for f in fields(Settings) if f.init}
    unknown = set(values) - known
    if unknown:
        raise ValueError(f'unknown settings: {", ".join(sorted(unknown))}')
    values.update({k: v for k, v in overrides.items() if v is not None})
    s = Settings(**values)
    s.source = source
    return s
