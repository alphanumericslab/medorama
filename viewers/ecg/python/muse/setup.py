"""Build step: the viewer's page lives once, in its folder (viewers/ecg/: index.html, the scripts,
style.css), where it also works on its own.  Building the package copies it into
muse_reader_py/page/, for `serve`; an editable install (pip install -e) serves the folder itself, and its build options
are written into the source tree (when VIEWER_ROOT or VIEWER_PASSWORD is set).  The
build also writes muse_reader_py/_build.py: the root the viewer serves at most (`VIEWER_ROOT`,
default: the whole file system) and whether it has password support (`VIEWER_PASSWORD=0`:
none).

The licence (<repo>/LICENSE, with its Required Notice, and COMMERCIAL.md) is copied beside this
file, where license-files (pyproject.toml) puts it into the wheel and the sdist.  The sdist also
carries a copy of the page (page/), so that it builds on its own."""

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
PAGE = HERE / 'page' if IN_SDIST else HERE.parents[1]
FILES = ('index.html', 'style.css')
LICENCE_FILES = ('LICENSE', 'COMMERCIAL.md')


GENERATED = '"""Written when the package was built (setup.py)."""'


def page_files():
    """The page's files: index.html, style.css and the scripts."""
    if not (PAGE / 'index.html').is_file():
        raise RuntimeError(f'the viewer page is missing: {PAGE}')
    return [*FILES, *sorted(p.name for p in PAGE.glob('*.js'))]


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
    root = os.environ.get('VIEWER_ROOT', '')
    password = os.environ.get('VIEWER_PASSWORD', '1') != '0'
    return f'{GENERATED}\n\nROOT = {root!r}\nPASSWORD = {password!r}\n'


class BuildPy(build_py):
    def run(self):
        super().run()
        if getattr(self, 'editable_mode', False):
            # an editable install runs the source tree: the options go there (git ignores the file)
            source = HERE / 'muse_reader_py' / '_build.py'
            if 'VIEWER_ROOT' in os.environ or 'VIEWER_PASSWORD' in os.environ:
                source.write_text(build_options(), encoding='utf-8')
            elif source.is_file() and source.read_text(encoding='utf-8').startswith(GENERATED):
                source.unlink()          # an earlier editable install's options: the defaults again
            return
        dest = Path(self.build_lib) / 'muse_reader_py'
        (dest / '_build.py').write_text(build_options(), encoding='utf-8')
        page = dest / 'page'
        shutil.rmtree(page, ignore_errors=True)
        page.mkdir(parents=True)
        for f in page_files():
            shutil.copy2(PAGE / f, page / f)


class Sdist(sdist):
    """The sdist carries the page as page/, so that a wheel builds from it alone."""

    def make_release_tree(self, base_dir, files):
        super().make_release_tree(base_dir, files)
        page = Path(base_dir) / 'page'
        page.mkdir()
        for f in page_files():
            shutil.copy2(PAGE / f, page / f)


copy_licence()
setup(cmdclass={'build_py': BuildPy, 'sdist': Sdist})
