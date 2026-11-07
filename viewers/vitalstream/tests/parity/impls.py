"""The three viewers, as the parity checks run them.

    py   vitalstream-py   (from the Python environment running the checks)
    rs   vitalstream-rs   (<repo>/target/release/)
    cpp  vitalstream-cpp  (viewers/vitalstream/cpp/build/release/bin/)

The first one available is the reference; the others are compared with it.
"""

import os
import sys
from pathlib import Path

VIEWER = Path(__file__).resolve().parents[2]                 # viewers/vitalstream
REPO = VIEWER.parents[1]
TESTDATA = VIEWER / 'testdata'
EXE = '.exe' if os.name == 'nt' else ''
BIN = {
    'py': Path(sys.executable).parent,
    'rs': REPO / 'target' / 'release',
    'cpp': VIEWER / 'cpp' / 'build' / 'release' / 'bin',
}


def viewer(impl):
    """The `vitalstream-<impl>` command."""
    return BIN[impl] / f'vitalstream-{impl}{EXE}'


def select(names=None, report=print):
    """The viewers to compare: those named (default: all that are built), at least two."""
    want = [n.strip() for n in names.split(',')] if names else list(BIN)
    out = []
    for impl in want:
        if impl not in BIN:
            raise SystemExit(f'unknown viewer {impl!r}: py, rs or cpp')
        if viewer(impl).exists():
            out.append(impl)
        elif names:
            raise SystemExit(f'{viewer(impl)} not found: build it first')
        else:
            report(f'(skipping {impl}: not built)')
    if len(out) < 2:
        raise SystemExit('parity needs at least two viewers built')
    return out
