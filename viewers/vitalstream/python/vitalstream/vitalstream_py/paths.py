"""The folders the readers may open files in (the Rust engine's paths::set_allowed_roots).

A program that limits the folders it opens (the viewer's data roots) also limits every file a
reader opens for a recording, symbolic links resolved: a WFDB record's header, the signal files
its header names, its .mrk file, its segments, and the CSV files of a folder or collection
(viewers/vitalstream/docs/index-format.md, "Files outside the data roots")."""

import os
from pathlib import Path

_allowed = None          # the real paths of the allowed folders; None: anywhere


def expand(p):
    """`~` at the start of a path is the home folder (`~user` is not expanded)."""
    p = str(p)
    if p == '~':
        return str(Path.home())
    if p.startswith('~') and p[1:2] in ('/', os.sep):
        return str(Path.home()).rstrip('/' + os.sep) + os.sep + p[2:]
    return p


def _absolute(p):
    return os.path.normpath(os.path.abspath(expand(p)))


def _real(p):
    """The real path (every link resolved), or None when it does not exist."""
    try:
        return str(Path(p).resolve(strict=True))
    except (OSError, RuntimeError):
        return None


def _within(path, root):
    r = root.rstrip('/' + os.sep)
    return path == root or not r or (path.startswith(r) and path[len(r):len(r) + 1] in ('/', os.sep))


def set_allowed_roots(roots):
    """Restrict every file the readers open to these folders, links resolved; None (the
    default) allows any file."""
    global _allowed
    _allowed = None if roots is None else [_real(_absolute(r)) or _absolute(r) for r in roots]


def allowed_roots():
    return None if _allowed is None else list(_allowed)


def allowed_file(p):
    """Whether a reader may open the file at `p`: its real path lies inside an allowed root.  A
    file that does not exist is allowed (nothing can be read from it); a broken link is not."""
    if _allowed is None:
        return True
    r = _real(p)
    if r is not None:
        return any(_within(r, root) for root in _allowed)
    return not os.path.lexists(p)
