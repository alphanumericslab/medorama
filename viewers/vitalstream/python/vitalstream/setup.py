"""Build step: the viewer's page lives once, in <repo>/web/ (shared by the Python, Rust and C++
viewers).  Building the package copies it into vitalstream_py/static/.  An editable install
(pip install -e) serves <repo>/web/ directly instead.

The build also writes vitalstream_py/_build.py, the options fixed when the viewer is installed,
from environment variables (as the Rust viewer's are fixed when it is compiled):

    VITALSTREAM_ROOT      the viewer's root: it never opens anything outside it ('~' is the home
                          folder of whoever runs the viewer, expanded when it runs).  Empty or
                          unset: the whole file system.
    VITALSTREAM_PASSWORD  0: without password support (no password can be set, the page never
                          asks for one, and the viewer refuses to start where one is required).
                          Unset or anything else: with it.

An editable install writes _build.py into the source tree only when one of them is set (the file
is ignored by git); without it, run from the source tree, the defaults apply.

The licence (<repo>/LICENSE, with its Required Notice, and COMMERCIAL.md) is copied beside this
file, where license-files (pyproject.toml) puts it into the wheel and the sdist.  The sdist also
carries a copy of the page (web/), so that it builds on its own.
"""

import os
import shutil
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py
from setuptools.command.sdist import sdist

HERE = Path(__file__).resolve().parent
# an unpacked sdist (PKG-INFO) carries its own copies; a clone takes them from the repository
IN_SDIST = (HERE / 'PKG-INFO').is_file()
REPO = None if IN_SDIST else HERE.parents[3]   # an unpacked sdist may lie anywhere, /tmp/x/ too
WEB = HERE / 'web' if IN_SDIST else HERE.parents[1] / 'web'
LICENCE_FILES = ('LICENSE', 'COMMERCIAL.md')
GENERATED = '# Written by setup.py when the package was built (VITALSTREAM_ROOT, VITALSTREAM_PASSWORD).\n'


def copy_licence():
    """<repo>/LICENSE and COMMERCIAL.md beside this file, for license-files."""
    for name in LICENCE_FILES:
        if IN_SDIST:
            if not (HERE / name).is_file():
                raise RuntimeError(f'the sdist lacks {name}')
            continue
        source = REPO / name
        if not source.is_file():
            raise RuntimeError(f'the licence is missing: {source} (build from a clone of the repository or from an sdist)')
        dest = HERE / name
        if not dest.is_file() or dest.read_bytes() != source.read_bytes():
            shutil.copyfile(source, dest)


def build_options():
    """The text of _build.py, from the environment."""
    root = os.environ.get('VITALSTREAM_ROOT', '')
    password = os.environ.get('VITALSTREAM_PASSWORD', '').strip() != '0'
    return (f'{GENERATED}\n'
            f"# the viewer's root ('~': the home folder of whoever runs it; empty: the whole file system)\n"
            f'ROOT = {root!r}\n'
            f'# password support\n'
            f'PASSWORD = {password!r}\n')


class BuildPy(build_py):
    def run(self):
        super().run()
        if getattr(self, 'editable_mode', False):
            source = HERE / 'vitalstream_py' / '_build.py'
            if 'VITALSTREAM_ROOT' in os.environ or 'VITALSTREAM_PASSWORD' in os.environ:
                source.write_text(build_options(), encoding='utf-8')
            elif source.is_file() and source.read_text(encoding='utf-8').startswith(GENERATED):
                source.unlink()          # an earlier editable install's options: the defaults again
            return
        (Path(self.build_lib) / 'vitalstream_py' / '_build.py').write_text(build_options(), encoding='utf-8')
        if not WEB.is_dir():
            raise RuntimeError(f'the viewer page is missing: {WEB}')
        dest = Path(self.build_lib) / 'vitalstream_py' / 'static'
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(WEB, dest, ignore=shutil.ignore_patterns('README.md'))


class Sdist(sdist):
    """The sdist carries the page as web/, so that a wheel builds from it alone."""

    def make_release_tree(self, base_dir, files):
        super().make_release_tree(base_dir, files)
        if not WEB.is_dir():
            raise RuntimeError(f'the viewer page is missing: {WEB}')
        shutil.copytree(WEB, Path(base_dir) / 'web', ignore=shutil.ignore_patterns('README.md'))


copy_licence()
setup(cmdclass={'build_py': BuildPy, 'sdist': Sdist})
