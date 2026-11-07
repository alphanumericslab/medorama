"""dicom-py: DICOM studies as databases for Medorama; DICOM ECGs as WFDB records.

    dicom-py index FOLDER -o imaging.csv     a database of a folder of DICOM files: one row per
                                             loop, run, series, report, PDF or ECG (patient ID,
                                             time, category, measurements, file), to import
    dicom-py wfdb ECG.dcm OUT_DIR            a DICOM ECG (12-lead or Holter) as a WFDB record
    dicom-py serve [--root DIR]              the PACS viewer, served from this computer with the
                                             folders under DIR (default: the home folder)

The database holds the patient ID, the key that links each row to its patient: keep it as PHI.
"""

import argparse
import csv
import sys
from pathlib import Path

from . import __version__, index_folder, page_server, write_wfdb


def main(argv=None):
    p = argparse.ArgumentParser(prog='dicom-py', description=__doc__.split('\n\n')[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--version', action='version', version=f'dicom-py {__version__}')
    sub = p.add_subparsers(dest='cmd', required=True)
    x = sub.add_parser('index', help='a database of a folder of DICOM files, to import')
    x.add_argument('folder')
    x.add_argument('-o', '--output', required=True, help='the CSV to write (file paths in it are relative to its folder)')
    w = sub.add_parser('wfdb', help='a DICOM ECG as a WFDB record')
    w.add_argument('file')
    w.add_argument('out_dir')
    w.add_argument('--name', help='record name (default: the file name)')
    v = sub.add_parser('serve', help='the PACS viewer, served from this computer with the folders under a root')
    page_server.add_arguments(v, 8081)
    a = p.parse_args(argv)
    if a.cmd == 'serve':
        return page_server.run(a, 'pacs', page_server.__file__, 'dicom-py')
    if a.cmd == 'wfdb':
        print(write_wfdb(a.file, a.out_dir, a.name))
        return 0
    out = Path(a.output).resolve()
    header, rows, skipped = index_folder(a.folder, out.parent)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, 'w', newline='', encoding='utf-8') as f:
        wr = csv.writer(f)
        wr.writerow(header)
        wr.writerows(rows)
    print(f'{out}: {len(rows)} object(s)' + (f'; {len(skipped)} not linked' if skipped else ''))
    for path, why in skipped:
        print(f'  not linked {path}: {why}', file=sys.stderr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
