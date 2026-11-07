#!/usr/bin/env python3
"""The example files of the ECG and PACS viewers (viewers/ecg/examples/, viewers/pacs/examples/):
synthetic ECGs and DICOM objects, made by the demo's generators (tools/make-demo-data.py), so
that anyone can try the viewers, on the website or from a clone. Every value is invented.

    python3 tools/make-viewer-examples.py        # needs numpy and wfdb-x (for the WFDB record)

ECG: a 12-lead WFDB record (with the cart's report in its header), a GE MUSE XML ECG (atrial
fibrillation, the computer's reading), a DICOM 12-lead ECG, a 12-lead EDF+ file, a PDF and a PNG
printout.
PACS: an echo loop (apical four-chamber), a coronary angiography run, a chest X-ray.
"""

import importlib.util
import struct
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('demo', ROOT / 'tools' / 'make-demo-data.py')
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)

PATIENT = demo.PATIENTS[0]          # the demo's invented patient
WHEN = datetime(2024, 2, 20, 9, 20)


def edf_field(text, width):
    """An EDF header field: ASCII, left-aligned, padded with spaces."""
    b = str(text).encode('ascii')
    assert len(b) <= width, text
    return b + b' ' * (width - len(b))


def tal(onset, texts=(), duration=None):
    """An EDF+ time-stamped annotation list: +onset[\\x15duration]\\x14[text\\x14 ...]\\x00.  The
    time-keeping one, first in each data record, has no text (an empty first annotation)."""
    head = f'{onset:+g}'.encode() + (f'\x15{duration:g}'.encode() if duration is not None else b'')
    return head + b'\x14' + b''.join(t.encode('utf-8') + b'\x14' for t in (texts or [''])) + b'\x00'


def ecg_edf(path, start, leads, fs=500, notes=()):
    """A 12-lead EDF+C file (after viewers/vitalstream/testdata/generate.py write_edf): data records
    of 1 s, the leads in µV (16 bits over ±5 mV), an annotation signal; notes: [(onset, duration
    or None, text)].  The patient field is the anonymous `X X X X`."""
    seconds = len(leads[0]) // fs
    pmin, pmax, dmin, dmax = -5000, 5000, -32768, 32767
    signals = [(lead, 'uV', 'AgAgCl electrode', 'HP:0.05Hz LP:150Hz', fs) for lead in demo.LEADS12]
    signals.append(('EDF Annotations', '', '', '', 60))
    head = edf_field('0', 8) + edf_field('X X X X', 80)
    head += edf_field(f'Startdate {start.strftime("%d-%b-%Y").upper()} X X X', 80)
    head += edf_field(start.strftime('%d.%m.%y'), 8) + edf_field(start.strftime('%H.%M.%S'), 8)
    head += edf_field(256 * (len(signals) + 1), 8) + edf_field('EDF+C', 44)
    head += edf_field(seconds, 8) + edf_field(1, 8) + edf_field(len(signals), 4)
    for field, w in ((lambda s: s[0], 16), (lambda s: s[2], 80), (lambda s: s[1], 8),
                     (lambda s: pmin if s[1] else -1, 8), (lambda s: pmax if s[1] else 1, 8),
                     (lambda s: dmin, 8), (lambda s: dmax, 8), (lambda s: s[3], 80),
                     (lambda s: s[4], 8), (lambda s: '', 32)):
        head += b''.join(edf_field(field(s), w) for s in signals)
    out = bytearray(head)
    for r in range(seconds):
        for v in leads:
            for x in v[r * fs:(r + 1) * fs]:
                q = round((x * 1000 - pmin) * (dmax - dmin) / (pmax - pmin) + dmin)
                out += struct.pack('<h', max(dmin, min(dmax, q)))
        b = tal(r) + b''.join(tal(o, [t], d) for o, d, t in notes if r <= o < r + 1)
        assert len(b) <= 120
        out += b + b'\0' * (120 - len(b))
    path.write_bytes(bytes(out))


def ecg(out):
    out.mkdir(parents=True, exist_ok=True)
    for f in out.iterdir():
        if f.name != 'README.md':
            f.unlink()
    # sinus rhythm with ST depression, as a cart's WFDB export
    leads = demo.ecg12(72, st_depression=True)
    meas = demo.ecg_measures(72)
    computer = demo.twelve_sl(meas, False, True, False)
    comments = ['Age: 74', 'Sex: F', 'Device: MAC 5500 HD', 'Filters: 0.05-150 Hz, 60 Hz notch', 'Program: Marquette 12SL v243']
    comments += [f'{k}: {v} {u}' for k, (v, u) in meas.items()]
    comments += [f'ST {lead}: {v:+.2f} mV' for lead, v in demo.ST_DEPRESSION.items()]
    comments.append(f'Dx: {"; ".join(computer)}')
    demo.ecg_wfdb(out, 'sinus_st_depression', WHEN, leads, comments=comments)
    # atrial fibrillation, MUSE XML, the computer's reading confirmed
    leads = demo.ecg12(110, af=True)
    meas = demo.ecg_measures(110, af=True)
    computer = demo.twelve_sl(meas, True, False, False)
    demo.ecg_muse(out / 'atrial_fibrillation_muse.xml', PATIENT, WHEN, leads, meas, computer, computer)
    # a DICOM 12-lead ECG; a PDF and a PNG printout of a normal ECG
    leads = demo.ecg12(64)
    meas = demo.ecg_measures(64)
    demo.ecg_dicom(out / 'normal_dicom.dcm', PATIENT[1], WHEN, leads, meas, demo.twelve_sl(meas, False, False, False))
    # a 12-lead EDF+ file of a normal ECG, with two annotations
    edf_leads = demo.ecg12(80)
    ecg_edf(out / 'normal_edf.edf', WHEN, edf_leads, notes=[(2.5, None, 'Lead check'), (6, 1.5, 'Event button')])
    demo.ecg_pdf(out / 'normal_printout.pdf', leads, 'Synthetic 12-lead ECG (not a patient)   Normal sinus rhythm   64 bpm')
    demo.ecg_png(out / 'normal_printout.png', leads)


def pacs(out):
    out.mkdir(parents=True, exist_ok=True)
    for f in out.iterdir():
        if f.name != 'README.md':
            f.unlink()
    when = demo.t('2024-01-02 10:30')
    demo.echo_loop(out / 'echo_a4c.dcm', PATIENT[0], when, ef=0.38, frames=30)
    demo.angio_run(out / 'angio_lca.dcm', PATIENT[0], demo.t('2023-12-29 08:10'))
    demo.chest_xray(out / 'chest_xray.dcm', PATIENT[0], when, opacity='right')


if __name__ == '__main__':
    ecg(ROOT / 'viewers' / 'ecg' / 'examples')
    pacs(ROOT / 'viewers' / 'pacs' / 'examples')
    for d in ('ecg', 'pacs'):
        for f in sorted((ROOT / 'viewers' / d / 'examples').iterdir()):
            print(f'{f.relative_to(ROOT)}  {f.stat().st_size:,} bytes')
