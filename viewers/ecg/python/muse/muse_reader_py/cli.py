"""muse-py: GE MUSE resting ECG XML files.

    muse-py json ECG.xml                 the ECG in the ECG viewer's JSON form (standard output)
    muse-py wfdb ECG.xml OUT_DIR         a WFDB record (format 16, 1 µV per unit) whose header
                                         comments carry the details, measurements and statements
    muse-py index FOLDER -o ecg.csv      an ECG database to import into Medorama: one row per
                                         file (the MUSE PatientID in a column `mrn`, time,
                                         measurements, statements, file)
    muse-py serve [--root DIR]           the ECG viewer, served from this computer with the
                                         folders under DIR (default: the home folder)

No patient identity is written into the JSON or WFDB outputs; the database (index) holds the
MUSE PatientID, in a column named `mrn` (Medorama reads it as an MRN), as the key that links
each ECG to its patient: keep it as PHI.
"""

import argparse
import csv
import json
import os
import struct
import sys
from pathlib import Path

from . import MEASUREMENTS, __version__, _parse, acquired, page_server, patient_key, read_muse


def one_line(x):
    """Text as one header line holds it: line breaks and runs of white space as one space."""
    return ' '.join(str(x).split())


def write_wfdb(ecg, out_dir, name):
    """A WFDB record of the ECG: `name`.hea and `name`.dat in `out_dir`."""
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    leads = ecg['leads']
    n = min(len(l['samples']) for l in leads)
    data = bytearray()
    for i in range(n):
        for l in leads:
            data += struct.pack('<h', max(-32767, min(32767, round(l['samples'][i]))))
    (out / f'{name}.dat').write_bytes(bytes(data))
    lines = [f'{name} {len(leads)} {ecg["fs"]:g} {n}']
    for j, l in enumerate(leads):
        vals = [max(-32767, min(32767, round(l['samples'][i]))) for i in range(n)]
        check = sum(vals) & 0xffff
        check = check - 0x10000 if check >= 0x8000 else check
        lines.append(f'{name}.dat 16 1000/mV 16 0 {vals[0] if vals else 0} {check} 0 {one_line(l["name"])}')
    meta = ecg['meta']
    for k, v in meta['fields']:
        lines.append(f'# {one_line(k)}: {one_line(v)}')
    for m in meta['measurements']:
        lines.append(f'# {one_line(m["name"])}: {m["value"]} {one_line(m["unit"])}')
    for s in meta['statements']:
        key = {'computer': 'Dx', 'confirmed': 'Dx confirmed', 'edited': 'Dx edited'}[s['source']]
        lines.append(f'# {key}: {"; ".join(one_line(x) for x in s["lines"])}')
        if s.get('by'):
            lines.append(f'# Confirmed by: {one_line(s["by"])}')
    (out / f'{name}.hea').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    return out / f'{name}.hea'


def index_rows(folder, base):
    """One database row per MUSE file under `folder` (hidden files and folders passed over); file
    paths relative to `base`."""
    names = []
    for _, name, unit in MEASUREMENTS:
        col = f'{name} ({unit})'
        if col not in names:
            names.append(col)
    header = ['mrn', 'time', 'category', 'name', 'value', 'unit', *names, 'interpretation', 'confirmed', 'file']
    rows, skipped = [], []
    paths = []
    for root, dirs, files in os.walk(folder):
        dirs[:] = [d for d in dirs if not d.startswith('.')]
        paths += [Path(root) / f for f in files if f.lower().endswith('.xml') and not f.startswith('.')]
    for path in sorted(paths):
        try:
            doc = _parse(str(path))            # read once
            ecg, key, when = read_muse(doc), patient_key(doc), acquired(doc)
        except (ValueError, OSError) as e:
            skipped.append((str(path), str(e)))
            continue
        meas = {f'{m["name"]} ({m["unit"]})': m['value'] for m in ecg['meta']['measurements']}
        st = {s['source']: '; '.join(s['lines']) for s in ecg['meta']['statements']}
        rate = meas.get('Vent. rate (bpm)', '')
        rows.append([key, when, 'ecg', '12-lead ECG', rate, 'bpm' if rate != '' else '', *[meas.get(c, '') for c in names],
                     st.get('computer', ''), st.get('confirmed', st.get('edited', '')),
                     Path(os.path.relpath(path, base)).as_posix()])   # '/' on every system
    return header, rows, skipped


def main(argv=None):
    p = argparse.ArgumentParser(prog='muse-py', description=__doc__.split('\n\n')[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--version', action='version', version=f'muse-py {__version__}')
    sub = p.add_subparsers(dest='cmd', required=True)
    j = sub.add_parser('json', help="the ECG in the ECG viewer's JSON form")
    j.add_argument('file')
    w = sub.add_parser('wfdb', help='a WFDB record, details in its header comments')
    w.add_argument('file')
    w.add_argument('out_dir')
    w.add_argument('--name', help='record name (default: the file name)')
    x = sub.add_parser('index', help='an ECG database of a folder of MUSE files, to import')
    x.add_argument('folder')
    x.add_argument('-o', '--output', required=True, help='the CSV to write (file paths in it are relative to its folder)')
    v = sub.add_parser('serve', help='the ECG viewer, served from this computer with the folders under a root')
    page_server.add_arguments(v, 8080)
    a = p.parse_args(argv)
    if a.cmd == 'serve':
        return page_server.run(a, 'ecg', page_server.__file__, 'muse-py')
    if a.cmd == 'json':
        json.dump(read_muse(a.file), sys.stdout, separators=(',', ':'), ensure_ascii=False, allow_nan=False)
        sys.stdout.write('\n')
    elif a.cmd == 'wfdb':
        print(write_wfdb(read_muse(a.file), a.out_dir, a.name or Path(a.file).stem))
    else:
        out = Path(a.output).resolve()
        header, rows, skipped = index_rows(a.folder, out.parent)
        out.parent.mkdir(parents=True, exist_ok=True)
        with open(out, 'w', newline='', encoding='utf-8') as f:
            wr = csv.writer(f)
            wr.writerow(header)
            wr.writerows(rows)
        print(f'{out}: {len(rows)} ECG(s)' + (f'; {len(skipped)} file(s) not read' if skipped else ''))
        for path, why in skipped:
            print(f'  not read {path}: {why}', file=sys.stderr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
