"""Index parity: the three viewers write the same index (docs/index-format.md, index format 3:
the index store) for the same recordings, so that they can share one store.

    python tests/parity/index_parity.py [--impl py,rs,cpp] [--adapters-dir DIR] [RECORDING ...]

Without recordings, the synthetic recordings of testdata/ are used, and recordings made here for
the rules of docs/index-format.md and docs/wfdb-collection.md: a collection that skips records
(missing, outside its folder, an empty name), a header that states more samples than its file
holds, a folder with a rate that is not a number, a record of 300 signals (read in chunks), more
distinct marker names than codes (with the Python module wfdbx), a folder of symbolic links
(where links can be made), an EDF+D file of 3000 runs, a header without a number of samples (and
a compressed one, with wfdbx), a multi-frequency record with a marker, a folder whose first
record states no start time, and an EDF file whose date is a day its month does not have.  Each viewer indexes each
recording into the plain index store of its own temporary cache (`--index`, `cache_dir/
index.sqlite`); then each store must hold one index, under the key computed here from the
specification (version 3), and the stores' rows must be the same: the `store` settings, the
`meta` text apart from its informative `engine` and `indexed_s`, the `arrays` rows (the number
of values of each array), and the `chunks` rows byte for byte.  A viewer that does not write
index stores yet (it writes an index folder, format 2) is reported and left out of the
comparison; a viewer that fails to index a recording is a difference.
"""

import argparse
import hashlib
import json
import os
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from impls import TESTDATA, select, viewer  # noqa: E402

INDEX_VERSION = 3
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
    # a header without a number of samples: read to the end of its data
    record(tmp / 'nocount', 'r', 'r 1 250', list(range(1, 9)))
    out.append(tmp / 'nocount' / 'r.hea')
    # a multi-frequency record (4, 1 and 2 samples per frame), a marker at frame 2
    mf = tmp / 'multifreq'
    mf.mkdir()
    frames = [[4 * f + k for k in range(4)] + [1000 + f] + [2000 + 10 * f + k for k in range(2)] for f in range(5)]
    cols = [[v for x in frames for v in x[0:4]], [x[4] for x in frames], [v for x in frames for v in x[5:7]]]
    sums = [sum(c) & 0xffff for c in cols]
    sums = [t - 0x10000 if t >= 0x8000 else t for t in sums]
    (mf / 'mf.hea').write_text(f'mf 3 100 5\nmf.dat 16x4 1/mV 16 0 0 {sums[0]} 0 A\n'
                               f'mf.dat 16 1/mV 16 0 0 {sums[1]} 0 B\nmf.dat 16x2 1/mV 16 0 0 {sums[2]} 0 C\n')
    (mf / 'mf.dat').write_bytes(b''.join(struct.pack('<7h', *x) for x in frames))
    (mf / 'mf.mrk').write_bytes(bytes([2, 4, 0, 0]))
    out.append(mf / 'mf.hea')
    # a folder whose first record states no start time: it ends where the next one starts
    lead = tmp / 'leading'
    record(lead, 'a', 'a 1 250 4', [1, 2, 3, 4])
    record(lead, 'b', 'b 1 250 4 10:00:00 01/03/2024', [1, 2, 3, 4])
    record(lead, 'c', 'c 1 250 4', [1, 2, 3, 4])
    out.append(lead)
    # an EDF file dated 31 February: no start
    feb = tmp / 'feb31.edf'
    b = bytearray((TESTDATA / 'edf' / 'psg_plus.edf').read_bytes())
    b[168:176] = b'31.02.24'
    feb.write_bytes(bytes(b))
    out.append(feb)
    try:
        import wfdbx
        z = tmp / 'flac'
        z.mkdir()
        wfdbx.write_record('z', [[k] for k in range(300)], 250, fmt=516, units=['mV'], gains=[1], adcres=[16],
                           names=['II'], dir=str(z))
        lines = (z / 'z.hea').read_text().splitlines()
        (z / 'z.hea').write_text('\n'.join(['z 1 250'] + lines[1:]) + '\n')
        out.append(z / 'z.hea')
    except ImportError:
        print('(no compressed record: the Python module wfdbx is not installed)')
    except Exception as e:  # a wfdbx without that writer
        print(f'(no compressed record: {e})')
    return out


def meta_text(text):
    """meta.json's text without its informative keys (key order kept)."""
    meta = json.loads(text)
    for k in INFORMATIVE:
        meta.pop(k, None)
    return json.dumps(meta, separators=(',', ':'))


def store_rows(path):
    """The rows of a plain index store: {'store': ..., 'keys': ..., 'meta': ..., 'arrays': ...,
    'chunks': ...}, sorted (meta without its informative keys)."""
    c = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
    try:
        return {'store': sorted(c.execute('SELECT name, value FROM store')),
                'keys': [k for (k,) in c.execute('SELECT key FROM indexes ORDER BY key')],
                'meta': sorted((k, meta_text(m)) for k, m in c.execute('SELECT key, meta FROM indexes')),
                'arrays': sorted(c.execute('SELECT key, array, n_values FROM arrays')),
                'chunks': sorted((k, a, n, bytes(d)) for k, a, n, d in c.execute('SELECT key, array, n, data FROM chunks'))}
    finally:
        c.close()


def compare(recording, names=None, adapters_dir=None, report=print):
    impls = select(names, report=report)
    tmp = Path(tempfile.mkdtemp(prefix='index-parity-'))
    problems = []
    try:
        stores = {}
        for impl in impls:
            cache = tmp / impl
            extra = ['--adapters-dir', adapters_dir] if adapters_dir else []
            r = subprocess.run([str(viewer(impl)), '--cache-dir', str(cache), *extra, '--index', str(recording)],
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, errors='replace')
            if r.returncode != 0:
                last = (r.stderr.strip().splitlines() or [f'exit status {r.returncode}'])[-1]
                problems.append(f'{impl}: --index failed: {last}')
                continue
            store = cache / 'index.sqlite'
            if not store.is_file():
                folders = [d.name for d in cache.iterdir() if d.is_dir()] if cache.is_dir() else []
                report(f'  ({impl}: writes no index store yet{" (an index folder, format 2)" if folders else ""}: '
                       'left out)')
                continue
            try:
                stores[impl] = store_rows(store)
            except (sqlite3.Error, ValueError) as e:
                problems.append(f'{impl}: its store cannot be read: {e}')
        expected = spec_key(recording)
        for impl, rows in stores.items():
            if rows['keys'] != [expected]:
                problems.append(f'{impl}: keys {rows["keys"]}, the specification gives {expected}')
        if len(stores) < 2:
            if not problems:
                report(f'  (only {", ".join(stores) or "no viewer"} wrote a store: nothing to compare)')
            return problems
        ref, *others = list(stores)
        for impl in others:
            for table in ('store', 'meta', 'arrays'):
                if stores[impl][table] != stores[ref][table]:
                    problems.append(f'{impl}: the {table} rows differ from {ref}\'s')
            a, b = stores[ref]['chunks'], stores[impl]['chunks']
            if a != b:
                which = sorted({(r[1], r[2]) for r in set(a) ^ set(b)})
                problems.append(f'{impl}: chunks differ from {ref}\'s: {which[:5]}{" ..." if len(which) > 5 else ""}')
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
