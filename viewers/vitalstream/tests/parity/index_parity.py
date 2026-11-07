"""Index parity: the three viewers write the same index (docs/index-format.md) for the same
recordings, so that they can share one cache folder.

    python tests/parity/index_parity.py [--impl py,rs,cpp] [--adapters-dir DIR] [RECORDING ...]

Without recordings, the synthetic recordings of testdata/ are used, and recordings made here for
the rules of docs/index-format.md and docs/wfdb-collection.md: a collection that skips records
(missing, outside its folder, an empty name), a header that states more samples than its file
holds, a folder with a rate that is not a number, a record of 300 signals (read in chunks), more
distinct marker names than codes (with the Python module wfdbx), a folder of symbolic links
(where links can be made), and an EDF+D file of 3000 runs.  Each viewer indexes each
recording into its own temporary cache (`--index`); then the keys must be equal, the key must
be the one computed here from the specification, every array file must be byte-identical, and
meta.json must be byte-identical apart from its informative `engine` and `indexed_s`.
"""

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from impls import TESTDATA, select, viewer  # noqa: E402

INDEX_VERSION = 2
# the bounds of a folder's walk (docs/index-format.md, the key): the default max_depth, and the
# most files listed
MAX_DEPTH = 20
MAX_FILES = 200_000
INFORMATIVE = ('engine', 'indexed_s')


def header_lines(hea):
    """A header file's lines that are neither blank nor comments."""
    text = Path(hea).read_bytes().decode('utf-8', 'replace')
    return [ln.strip() for ln in text.split('\n') if ln.strip() and not ln.strip().startswith('#')]


def record_files(base, name):
    """The files of a WFDB record as docs/index-format.md lists them (read here from the header
    text, independently of wfdb-x): its header, the signal files it names, its .mrk file, and the
    same for each segment of a multi-segment record."""
    out = set()

    def add(rec):
        folder = rec.rsplit('/', 1)[0] + '/' if '/' in rec else ''
        out.add(f'{rec}.hea')
        lines = header_lines(base / f'{rec}.hea')
        first = lines[0].split()
        nseg = int(first[0].split('/')[1]) if '/' in first[0] else 0
        nsig = int(first[1]) if len(first) > 1 else 0
        if not nseg:
            for ln in lines[1:1 + nsig]:
                f = ln.split()[0]
                if f not in ('-', '~'):
                    out.add(folder + f)
        if (base / f'{rec}.mrk').is_file():
            out.add(f'{rec}.mrk')
        return [ln.split()[0] for ln in lines[1:1 + nseg]]

    for seg in add(name):
        if seg != '~':
            add((name.rsplit('/', 1)[0] + '/' if '/' in name else '') + seg)
    return sorted(out)


def spec_key(path):
    """The key as docs/index-format.md specifies it (an implementation independent of the viewers)."""
    p = os.path.realpath(path)
    if p.startswith('\\\\?\\'):
        p = '\\\\' + p[8:] if p.startswith('\\\\?\\UNC\\') else p[4:]
    if os.path.isdir(p):
        fp = []

        # every file below the folder, names in order, at most MAX_DEPTH levels of subfolders and
        # MAX_FILES files; a link to a file counts with its target's size and time, links to
        # folders and broken links do not
        def walk(folder, rel, depth):
            for e in sorted(os.scandir(folder), key=lambda e: e.name):
                if len(fp) >= MAX_FILES:
                    return
                if e.name.startswith('.'):
                    continue
                r = f'{rel}/{e.name}' if rel else e.name
                if e.is_dir(follow_symlinks=False):
                    if depth < MAX_DEPTH:
                        walk(e.path, r, depth + 1)
                elif e.is_symlink() and not os.path.isfile(e.path):
                    continue
                else:
                    st = os.stat(e.path)
                    fp.append([r, st.st_size, st.st_mtime_ns])
        walk(p, '', 0)
        fp.sort()
    elif p.endswith('.hea'):
        base = Path(p).parent
        fp = []
        for f in record_files(base, Path(p).stem):
            st = os.stat(base / f)
            fp.append([f, st.st_size, st.st_mtime_ns])
    else:
        st = os.stat(p)
        fp = [st.st_size, st.st_mtime_ns]
    text = json.dumps([INDEX_VERSION, p, fp], separators=(',', ':')).replace('\x7f', '\\u007f')
    return hashlib.sha256(text.encode()).hexdigest()[:20]


def record(folder, name, head, samples):
    """A single-signal record: header line `head`, 16-bit `samples` with their checksum."""
    folder.mkdir(parents=True, exist_ok=True)
    total = sum(samples) & 0xffff
    total = total - 0x10000 if total >= 0x8000 else total
    (folder / f'{name}.hea').write_text(f'{head}\n{name}.dat 16 200/mV 16 0 0 {total} 0 II\n')
    (folder / f'{name}.dat').write_bytes(struct.pack(f'<{len(samples)}h', *samples))


def generated(tmp):
    """The recordings made for the rules (see the module's text), in `tmp`."""
    tmp = Path(tmp)
    out = []
    coll = tmp / 'skipping'
    record(coll, 'good', 'good 1 250 4', [1, 2, 3, 4])
    record(tmp / 'elsewhere', 'far', 'far 1 250 4', [1, 2, 3, 4])
    (coll / 'index.csv').write_text(f'record,start_unix\ngood,0\nmissing,0\n../elsewhere/far,0\n'
                                    f'{tmp / "elsewhere" / "far"},0\n,0\n')
    out.append(coll)
    record(tmp / 'toomuch', 'r', 'r 1 250 1000000000000', list(range(1, 9)))
    out.append(tmp / 'toomuch' / 'r.hea')
    record(tmp / 'nanrate', 'a', 'a 1 -nan 4', [1, 2, 3, 4])
    record(tmp / 'nanrate', 'b', 'b 1 250 4', [1, 2, 3, 4])
    out.append(tmp / 'nanrate')
    wide, (nsig, n) = tmp / 'wide', (300, 5000)
    wide.mkdir()
    head = [f'w {nsig} 100 {n}']
    for k in range(nsig):
        total = sum((i + k) & 0xffff for i in range(n)) & 0xffff
        head.append(f'w.dat 16 1/mV 16 0 0 {total - 0x10000 if total >= 0x8000 else total} 0 s{k}')
    (wide / 'w.hea').write_text('\n'.join(head) + '\n')
    (wide / 'w.dat').write_bytes(b''.join(struct.pack(f'<{nsig}h', *[i + k for k in range(nsig)]) for i in range(n)))
    out.append(wide / 'w.hea')
    try:
        import wfdbx
        names = 32_800                                        # beyond the 32,767 codes
        record(tmp / 'markers', 'r', f'r 1 250 {names + 2}', [k % 100 for k in range(names + 2)])
        wfdbx.write_annotations('r', 'mrk', list(range(1, names + 1)), symbols=['"'] * names,
                                aux=[f'm{k}' for k in range(names)], dir=str(tmp / 'markers'))
        out.append(tmp / 'markers' / 'r.hea')
    except ImportError:
        print('(no marker names beyond the codes: the Python module wfdbx is not installed)')
    links = tmp / 'links'
    record(links, 'a', 'a 1 250 4', [1, 2, 3, 4])
    (tmp / 'target.dat').write_bytes(bytes(8))
    try:
        (links / 'b.dat').symlink_to(tmp / 'target.dat')
        (links / 'up').symlink_to(tmp)
        (links / 'broken').symlink_to(tmp / 'nothing')
        out.append(links)
    except OSError:
        print('(no folder of links: links cannot be made here)')
    viewer_parity = __import__('viewer_parity')
    viewer_parity.edf_runs(tmp / 'runs.edf', 3000)
    out.append(tmp / 'runs.edf')
    return out


def meta_text(path):
    """meta.json without its informative keys, as text (key order kept)."""
    meta = json.loads(path.read_text(encoding='utf-8'))
    for k in INFORMATIVE:
        meta.pop(k, None)
    return json.dumps(meta, separators=(',', ':'))


def compare(recording, names=None, adapters_dir=None, report=print):
    impls = select(names, report=report)
    tmp = Path(tempfile.mkdtemp(prefix='index-parity-'))
    problems = []
    try:
        folders = {}
        for impl in impls:
            cache = tmp / impl
            extra = ['--adapters-dir', adapters_dir] if adapters_dir else []
            subprocess.run([str(viewer(impl)), '--cache-dir', str(cache), *extra, '--index', str(recording)],
                           check=True, stdout=subprocess.DEVNULL)
            keys = [d for d in cache.iterdir() if d.is_dir()]
            if len(keys) != 1:
                problems.append(f'{impl}: {len(keys)} index folders')
                continue
            folders[impl] = keys[0]
        expected = spec_key(recording)
        for impl, d in folders.items():
            if d.name != expected:
                problems.append(f'{impl}: key {d.name}, the specification gives {expected}')
        ref, *others = list(folders)
        ref_files = sorted(f.name for f in folders[ref].iterdir())
        for impl in others:
            files = sorted(f.name for f in folders[impl].iterdir())
            if files != ref_files:
                problems.append(f'{impl}: files {sorted(set(files) ^ set(ref_files))} differ from {ref}')
                continue
            for name in files:
                a, b = folders[ref] / name, folders[impl] / name
                same = meta_text(a) == meta_text(b) if name == 'meta.json' else a.read_bytes() == b.read_bytes()
                if not same:
                    problems.append(f'{impl}: {name} differs from {ref}')
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return problems


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('recordings', nargs='*')
    ap.add_argument('--impl', help='comma-separated: py,rs,cpp (default: every viewer that is built)')
    ap.add_argument('--adapters-dir', help='adapter folder given to the viewers')
    args = ap.parse_args()
    made = tempfile.mkdtemp(prefix='index-parity-made-')
    recordings = args.recordings or [str(TESTDATA / x) for x in (
        'collection', 'plain', 'session', 'plain/rec_a.hea', 'session/ms/ms_rec.hea', 'session/ms/ms_rec_0001.hea',
        'session/ms/ms_fix.hea', 'edf/psg_plus.edf', 'edf/gappy_plus.edf', 'edf/plain.edf', 'edf/eeg_plus.bdf')]
    if not args.recordings:
        recordings += [str(x) for x in generated(made)]
    bad = 0
    for arg in recordings:
        print(arg)
        p = compare(Path(arg).resolve(), args.impl, args.adapters_dir)
        print('  OK' if not p else '  ' + '\n  '.join(p))
        bad += bool(p)
    shutil.rmtree(made, ignore_errors=True)
    sys.exit(1 if bad else 0)
