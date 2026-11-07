#!/usr/bin/env python3
"""Packs a release folder into an archive that is the same, byte for byte, whenever it is made
from the same files (.github/workflows/release.yml):

    python3 tools/release-archive.py NAME.tar.gz FOLDER
    python3 tools/release-archive.py NAME.zip FOLDER

The entries are sorted, carry the time SOURCE_DATE_EPOCH (the release commit's time; 0 when it
is unset), no owner, and the mode 755 (folders and programs) or 644; the gzip header carries no
time or name.  The Python standard library only.
"""

import gzip
import io
import os
import sys
import tarfile
import time
import zipfile
from pathlib import Path


def entries(folder):
    """(path in the archive, file or None for a folder, mode), sorted, the folder itself first."""
    base = folder.parent
    out = [(folder.name, None, 0o755)]
    for p in sorted(folder.rglob('*'), key=lambda p: p.relative_to(base).as_posix()):
        name = p.relative_to(base).as_posix()
        if p.is_symlink():
            sys.exit(f'release-archive.py: {name} is a symbolic link; copy the file instead')
        if p.is_dir():
            out.append((name, None, 0o755))
        else:
            # Windows has no execute bit: its programs are the .exe files
            program = p.suffix.lower() == '.exe' or (os.name != 'nt' and os.access(p, os.X_OK))
            out.append((name, p, 0o755 if program else 0o644))
    return out


def tar_gz(path, folder, epoch):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode='w', format=tarfile.GNU_FORMAT) as tar:
        for name, file, mode in entries(folder):
            info = tarfile.TarInfo(name)
            info.mtime, info.mode, info.uid, info.gid, info.uname, info.gname = epoch, mode, 0, 0, '', ''
            if file is None:
                info.type = tarfile.DIRTYPE
                tar.addfile(info)
            else:
                info.size = file.stat().st_size
                with open(file, 'rb') as f:
                    tar.addfile(info, f)
    with open(path, 'wb') as out, gzip.GzipFile(filename='', mode='wb', fileobj=out, mtime=0, compresslevel=9) as gz:
        gz.write(raw.getvalue())


def zip_(path, folder, epoch):
    # a zip's times are local, two-second and from 1980 on: the epoch's UTC time
    when = max(time.gmtime(epoch)[:6], (1980, 1, 1, 0, 0, 0))
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, file, mode in entries(folder):
            info = zipfile.ZipInfo(name + ('/' if file is None else ''), when)
            info.create_system = 3
            info.external_attr = (mode | (0o040000 if file is None else 0o100000)) << 16
            if file is None:
                z.writestr(info, b'')
            else:
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, file.read_bytes())


def main():
    if len(sys.argv) != 3 or not sys.argv[1].endswith(('.tar.gz', '.zip')):
        sys.exit(__doc__)
    path, folder = Path(sys.argv[1]), Path(sys.argv[2]).resolve()
    if not folder.is_dir():
        sys.exit(f'release-archive.py: {folder} is not a folder')
    epoch = int(os.environ.get('SOURCE_DATE_EPOCH') or 0)
    (tar_gz if path.name.endswith('.tar.gz') else zip_)(path, folder, epoch)


if __name__ == '__main__':
    main()
