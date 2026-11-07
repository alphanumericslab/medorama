"""Synthetic recordings for the viewers' and the engine's tests: deterministic, no patient data.

    python testdata/generate.py        # rewrites testdata/collection/, plain/, session/ and edf/

collection/  a WFDB collection (docs/wfdb-collection.md): ECG (250 Hz, two signals) and
             arterial pressure (125 Hz) in three stretches: a break of exactly 1.5 s, then a
             stretch without a stated start time placed after the 0.5 s break stated before it; invalid
             samples; vital signs (one invalid row, one from a second source); two alarms in
             events.csv; markers in .mrk
             annotation files (with and without text).
plain/       plain WFDB records listed in RECORDS: two (250 Hz and 500 Hz) with start times in
             their headers, taken as UTC; one with an uncalibrated signal (gain 0, shown in ADC
             units) and one without units; one without a sampling frequency, which no viewer
             reads.
session/     a folder of plain records in subfolders, opened as one session: ECG and arterial
             pressure in two subfolders; two ECG records that overlap in time (the second goes
             into a second lane); one without a start time; a multi-segment record with a
             variable layout (a layout header, a null segment of 0.5 s, a segment holding one
             signal of two, markers on the record), and one with a fixed layout and no start
             time; a hidden folder, which is not read.
edf/         EDF, EDF+ and BDF files, one recording each: an EDF+C file of three signals at
             250, 25 and 1 Hz whose first record starts 0.25 s after the header's time, with
             notes (with and without a duration) and an (invented) identity in its patient field;
             an EDF+D file of three runs of records with breaks between them; a plain EDF file of
             1999 whose header leaves the number of records unknown and whose last record is cut
             short; a BDF+C file (24-bit samples) with a signal whose digital range is empty.

Only the Python standard library is used. WFDB files are written as the WFDB library writes
them (format 16, little-endian; MIT annotation format); EDF files as the EDF and EDF+
specifications define them (https://www.edfplus.info/specs/).
"""

import csv
import math
import struct
from pathlib import Path

HERE = Path(__file__).resolve().parent
T0 = 1709287200  # 2024-03-01 10:00:00 UTC


def ecg(t, lead):
    """A plausible ECG shape: narrow beats at 72 per minute plus a slow wave (mV)."""
    phase = (t * 72 / 60) % 1.0
    beat = 1.2 * math.exp(-((phase - 0.3) / 0.015) ** 2) - 0.15 * math.exp(-((phase - 0.36) / 0.02) ** 2)
    wave = 0.25 * math.exp(-((phase - 0.6) / 0.06) ** 2)
    return (beat + wave) * (1.0 if lead == 0 else 0.7) + 0.05 * math.sin(2 * math.pi * 0.3 * t)


def pressure(t):
    """Arterial pressure (mmHg): 80 to 120, at the same rate."""
    phase = (t * 72 / 60) % 1.0
    return 80 + 40 * math.exp(-((phase - 0.35) / 0.12) ** 2)


def checksum(column):
    s = sum(column) & 0xFFFF
    return s - 0x10000 if s >= 0x8000 else s


def write_multi(folder, name, fs, nsig, segments, start=None, layout=None):
    """A multi-segment record: segments [(name or '~', samples)], already written (null
    segments: '~'); layout [(description, unit, gain)] writes a layout header first (variable
    layout); nsig: the number of signals."""
    lines = []
    segs = list(segments)
    if layout:
        lines_l = [f'{name}_layout {len(layout)} {fs:g} 0']
        for desc, unit, gain in layout:
            lines_l.append(f'~ 0 {gain:g}/{unit} 16 0 0 0 0 {desc}')
        (folder / f'{name}_layout.hea').write_text('\n'.join(lines_l) + '\n')
        segs = [(f'{name}_layout', 0)] + segs
    n = sum(k for _, k in segs)
    when = f' {start[0]} {start[1]}' if start else ''
    lines.append(f'{name}/{len(segs)} {nsig} {fs:g} {n}{when}')
    lines += [f'{seg} {k}' for seg, k in segs]
    (folder / f'{name}.hea').write_text('\n'.join(lines) + '\n')


def write_record(folder, name, fs, signals, start=None):
    """signals: [(description, unit, gain, digital samples)]; start: (hh:mm:ss, dd/mm/yyyy).
    A gain of 0 writes an uncalibrated signal; an empty unit writes none; fs None writes a record
    line that states no sampling frequency (nor length)."""
    folder.mkdir(parents=True, exist_ok=True)
    n = len(signals[0][3])
    with open(folder / f'{name}.dat', 'wb') as f:
        for i in range(n):
            f.write(b''.join(struct.pack('<h', s[3][i]) for s in signals))
    when = f' {start[0]} {start[1]}' if start else ''
    lines = [f'{name} {len(signals)} {fs:g} {n}{when}' if fs else f'{name} {len(signals)}']
    for desc, unit, gain, x in signals:
        g = f'{gain:g}/{unit}' if gain and unit else f'{gain:g}'
        lines.append(f'{name}.dat 16 {g} 16 0 {x[0]} {checksum(x)} 0 {desc}')
    (folder / f'{name}.hea').write_text('\n'.join(lines) + '\n')


def write_annotations(path, marks):
    """marks: [(sample, code, chan, text)] in time order, MIT annotation format."""
    out = bytearray()
    prev, chan = 0, 0
    for sample, code, c, text in marks:
        d = sample - prev
        if d > 1023:
            out += struct.pack('<H', 59 << 10) + struct.pack('<HH', (d >> 16) & 0xFFFF, d & 0xFFFF)
            d = 0
        out += struct.pack('<H', (code << 10) | d)
        if c != chan:
            out += struct.pack('<H', (62 << 10) | c)
            chan = c
        if text:
            b = text.encode('ascii')
            out += struct.pack('<H', (63 << 10) | len(b)) + b + (b'\0' if len(b) % 2 else b'')
        prev = sample
    out += b'\0\0'
    path.write_bytes(bytes(out))


def digital(values, gain, invalid=()):
    return [-32768 if i in invalid else max(-32767, min(32767, round(v * gain))) for i, v in enumerate(values)]


def utc(t):
    import datetime as dt
    d = dt.datetime.fromtimestamp(t, dt.timezone.utc)
    frac = f'.{d.microsecond // 1000:03d}' if d.microsecond else ''
    return d.strftime('%H:%M:%S') + frac, d.strftime('%d/%m/%Y'), d.strftime('%Y-%m-%dT%H:%M:%S') + frac + '+00:00'


def write_csv(path, header, rows):
    with open(path, 'w', newline='') as f:
        w = csv.writer(f, lineterminator='\n')
        w.writerow(header)
        w.writerows(rows)


def collection():
    root = HERE / 'collection'
    # stretches: (start offset s or None, duration s)
    stretches = [(0.0, 4.0), (5.5, 3.0), (None, 2.0)]
    index, gaps = [], []
    starts = {}
    for k, (start, dur) in enumerate(stretches):
        for modality, fs in (('ecg', 250), ('pressure', 125)):
            name = f'{modality}_{fs}_{k + 1:04d}'
            off = start if start is not None else 9.0  # where the untimed stretch really is
            n = round(dur * fs)
            t = [off + i / fs for i in range(n)]
            if modality == 'ecg':
                sig = [('II', 'mV', 200, digital([ecg(x, 0) for x in t], 200)),
                       ('V', 'mV', 200, digital([ecg(x, 1) for x in t], 200))]
            else:
                sig = [('ART', 'mmHg', 10, digital([pressure(x) for x in t], 10, invalid={40, 41, 42}))]
            when = utc(T0 + start) if start is not None else None
            write_record(root / modality, name, fs, sig, when[:2] if when else None)
            if modality == 'ecg':
                beats = [round(((j + 0.3) / 1.2 - off) * fs) for j in range(int(off * 1.2), int((off + dur) * 1.2) + 1)]
                marks = [(b, 1 if j % 5 else 5, 0, 'R wave' if j % 5 else '') for j, b in enumerate(beats) if 0 <= b < n]
                marks.append((n // 2, 1, 1, 'lead check'))
                write_annotations(root / modality / f'{name}.mrk', sorted(marks))
            starts.setdefault(k, []).append(f'{modality}/{name}')
            index.append([f'{modality}/{name}', modality, f'{fs:.1f}', len(sig), ';'.join(s[0] for s in sig),
                          ';'.join(s[1] for s in sig), n, f'{dur:.1f}',
                          when[2] if when else '', f'{T0 + start:.1f}' if start is not None else '',
                          'header time' if when else '', '0' if when else '', f'stretch {k}'])
    for modality in ('ecg', 'pressure'):
        (root / modality / 'RECORDS').write_text(
            ''.join(r[0].split('/')[1] + '\n' for r in index if r[1] == modality))
    write_csv(root / 'index.csv',
              ['record', 'modality', 'fs', 'n_sig', 'signals', 'units', 'sig_len', 'duration_s', 'start_utc',
               'start_unix', 'time_source', 'time_uncertainty_s', 'stretch'], index)
    gaps = [
        ['all', 'signal_loss', utc(T0 + 4.0)[2], utc(T0 + 5.5)[2], f'{T0 + 4.0:.1f}', f'{T0 + 5.5:.1f}', '1.5', '0',
         'time stamps', 'True', ';'.join(starts[0]), ';'.join(starts[1]), ''],
        ['all', 'signal_loss', '', '', '', '', '0.5', '0', 'sample count', 'True', ';'.join(starts[1]),
         ';'.join(starts[2]), 'the next stretch has no start time of its own'],
    ]
    write_csv(root / 'gaps.csv',
              ['scope', 'reason', 'end_before_utc', 'start_after_utc', 'end_before_unix', 'start_after_unix', 'gap_s',
               'gap_uncertainty_s', 'measured_by', 'exact', 'records_before', 'records_after', 'notes'], gaps)
    vitals = []
    for s in range(0, 9):
        vitals.append(['monitor', f'{T0 + s}', 'HR', 'HR', f'{72 + s % 3}', 'bpm', '0'])
        if s % 2 == 0:
            vitals.append(['monitor', f'{T0 + s}', 'SpO2', 'SpO2', f'{97 - s % 4}', '%', '0'])
    vitals.append(['monitor', f'{T0 + 3}', 'HR', 'HR', '999', 'bpm', '1'])  # invalid: left out
    vitals.append(['import', f'{T0 + 4}', 'HR', 'HR', '75', 'bpm', '0'])    # a second source: "HR [import]"
    write_csv(root / 'vitals.csv', ['source', 'time', 'name', 'label', 'value', 'unit', 'validity'], vitals)
    write_csv(root / 'events.csv', ['start_unix', 'end_unix', 'level', 'text', 'kind'], [
        [f'{T0 + 1}', f'{T0 + 3}', '2', 'HR HIGH 151 > 150', 'alarm'],
        [f'{T0 + 6}', '', '1', 'Leads off', 'alarm'],
    ])
    (root / 'README.txt').write_text(
        'A synthetic WFDB collection (docs/wfdb-collection.md), written by testdata/generate.py.\n')


def plain():
    root = HERE / 'plain'
    for name, fs, n, start in (('rec_a', 250, 1500, T0 + 3600), ('rec_b', 500, 1000, T0 + 3610)):
        t = [i / fs for i in range(n)]
        sig = [('II', 'mV', 200, digital([ecg(x, 0) for x in t], 200))]
        if name == 'rec_a':
            sig.append(('ART', 'mmHg', 10, digital([pressure(x) for x in t], 10)))
        write_record(root, name, fs, sig, utc(start)[:2])
    # nothing assumed that a header does not state: an uncalibrated signal (shown in ADC units)
    # and one without units; a record without a sampling frequency, which no viewer reads
    t = [i / 125 for i in range(500)]
    write_record(root, 'rec_c', 125, [('Resp', '', 0, [round(300 * math.sin(2 * math.pi * 0.3 * x)) for x in t]),
                                      ('Temp', '', 100, digital([37 + 0.1 * math.sin(x) for x in t], 100))],
                 utc(T0 + 3620)[:2])
    write_record(root, 'rec_d', None, [('II', 'mV', 200, digital([ecg(i / 250, 0) for i in range(250)], 200))])
    (root / 'RECORDS').write_text('rec_a\nrec_b\nrec_c\nrec_d\n')


def session():
    root = HERE / 'session'
    t0 = T0 + 7200

    def leads(fs, n, off, which=('II', 'V')):
        t = [off + i / fs for i in range(n)]
        return [(lead, 'mV', 200, digital([ecg(x, k) for x in t], 200)) for k, lead in enumerate(('II', 'V')) if lead in which]

    write_record(root / 'abp', 'abp_a', 125, [('ART', 'mmHg', 10, digital([pressure(i / 125) for i in range(500)], 10))],
                 utc(t0)[:2])
    write_record(root / 'ecg', 'hr_a', 250, leads(250, 1000, 0, ('II',)), utc(t0)[:2])
    write_record(root / 'ecg', 'hr_b', 250, leads(250, 500, 1, ('II',)), utc(t0 + 1)[:2])  # overlaps hr_a
    write_record(root / 'ecg', 'hr_c', 250, leads(250, 250, 5, ('II',)))                   # no start time
    ms = root / 'ms'
    write_record(ms, 'ms_rec_0001', 250, leads(250, 500, 10))
    write_record(ms, 'ms_rec_0002', 250, leads(250, 250, 12.5, ('II',)))
    write_multi(ms, 'ms_rec', 250, 2, [('ms_rec_0001', 500), ('~', 125), ('ms_rec_0002', 250)], utc(t0 + 10)[:2],
                layout=[('II', 'mV', 200), ('V', 'mV', 200)])
    # markers of the multi-segment record, sample numbers from its start: in the first segment
    # (lead II and V) and in the second (lead II); one in the null segment, which is left out
    write_annotations(ms / 'ms_rec.mrk', [(100, 1, 0, 'R wave'), (200, 1, 1, ''), (550, 1, 0, 'lost'),
                                         (700, 1, 0, 'R wave')])
    write_record(ms, 'ms_fix_0001', 250, leads(250, 250, 20, ('II',)))
    write_record(ms, 'ms_fix_0002', 250, leads(250, 250, 21, ('II',)))
    write_multi(ms, 'ms_fix', 250, 1, [('ms_fix_0001', 250), ('ms_fix_0002', 250)])
    write_record(root / '.hidden', 'ignored', 250, leads(250, 250, 0, ('II',)), utc(t0)[:2])


def edf_field(text, width):
    """An EDF header field: ASCII, left-aligned, padded with spaces."""
    b = text.encode('ascii')
    assert len(b) <= width, text
    return b + b' ' * (width - len(b))


def tal(onset, texts=(), duration=None):
    """An EDF+ time-stamped annotation list: +onset[\\x15duration]\\x14[text\\x14 ...]\\x00.  The
    time-keeping one, first in each data record, has no text (an empty first annotation)."""
    head = f'{onset:+g}'.encode() + (f'\x15{duration:g}'.encode() if duration is not None else b'')
    return head + b'\x14' + b''.join(t.encode('utf-8') + b'\x14' for t in (texts or [''])) + b'\x00'


def write_edf(path, start, duration, signals, onsets, notes=None, version='', n_records=None, bdf=False,
              patient='X X X X', extra=b''):
    """An EDF, EDF+ or BDF file.  signals: [(label, unit, pmin, pmax, dmin, dmax, samples per record,
    value at time t)]; onsets: each data record's start, in seconds after `start`; version: the
    header's reserved field ('EDF+C', 'EDF+D', 'BDF+C'; EDF+ and BDF+ files get an annotation
    signal, whose time-keeping annotations state the onsets); notes: {record: [(onset, duration
    or None, text)]}; n_records: the count in the header (-1: unknown); extra: bytes after the
    last record (a record cut short)."""
    import datetime as dt
    d = dt.datetime.fromtimestamp(start, dt.timezone.utc)
    width = 3 if bdf else 2
    sigs = list(signals)
    if version:
        lo, hi = (-8388608, 8388607) if bdf else (-32768, 32767)
        sigs.append(('BDF Annotations' if bdf else 'EDF Annotations', '', -1, 1, lo, hi, 30, None))
    num = str
    head = b'\xffBIOSEMI' if bdf else edf_field('0', 8)
    head += edf_field(patient, 80)
    head += edf_field(f'Startdate {d.strftime("%d-%b-%Y").upper()} X X X' if version else 'Synthetic test file', 80)
    head += edf_field(d.strftime('%d.%m.%y'), 8) + edf_field(d.strftime('%H.%M.%S'), 8)
    head += edf_field(str(256 * (len(sigs) + 1)), 8) + edf_field(version or ('24BIT' if bdf else ''), 44)
    head += edf_field(str(len(onsets) if n_records is None else n_records), 8) + edf_field(num(duration), 8)
    head += edf_field(str(len(sigs)), 4)
    for field, w in ((lambda s: s[0], 16), (lambda s: '', 80), (lambda s: s[1], 8), (lambda s: num(s[2]), 8),
                     (lambda s: num(s[3]), 8), (lambda s: num(s[4]), 8), (lambda s: num(s[5]), 8),
                     (lambda s: '', 80), (lambda s: str(s[6]), 8), (lambda s: '', 32)):
        head += b''.join(edf_field(field(s), w) for s in sigs)
    out = bytearray(head)
    for r, onset in enumerate(onsets):
        for label, unit, pmin, pmax, dmin, dmax, spr, f in sigs:
            if f is None:  # the annotation signal: time keeping, then this record's notes
                b = tal(onset) + b''.join(tal(o, [t], du) for o, du, t in (notes or {}).get(r, []))
                assert len(b) <= spr * width
                out += b + b'\0' * (spr * width - len(b))
                continue
            for i in range(spr):
                v = f(onset + i * duration / spr)
                if dmax == dmin:
                    q = round(v)
                else:
                    q = max(dmin, min(dmax, round((v - pmin) * (dmax - dmin) / (pmax - pmin) + dmin)))
                out += struct.pack('<i', q)[:3] if bdf else struct.pack('<h', q)
    out += extra
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(out))


def edf():
    root = HERE / 'edf'
    start = T0 + 3 * 3600  # 2024-03-01 13:00:00

    def resp(t):
        return 400 * math.sin(2 * math.pi * 0.25 * t)

    # EDF+C: three signals at 250, 25 and 1 Hz; the first record 0.25 s after the header's start
    # time; notes with and without a duration.  The patient field holds an identity (invented),
    # which no viewer reads.
    write_edf(root / 'psg_plus.edf', start, 1, [
        ('ECG', 'mV', -5, 5, -32768, 32767, 250, lambda t: ecg(t, 0)),
        ('Resp', '', -1000, 1000, -2048, 2047, 25, resp),
        ('SpO2', '%', 0, 100, 0, 1000, 1, lambda t: 96 + math.sin(t / 3)),
    ], [0.25 + k for k in range(20)], version='EDF+C', patient='MO-TEST-0001 F 01-JAN-1970 Synthetic_Subject',
        notes={0: [(0.25, 10, 'Sleep stage W'), (0.5, None, 'Lights off')], 4: [(4.25, 3, 'Arousal')]})
    # EDF+D: records of 0.5 s in three runs (breaks after 1.5 s and after 4 s), a note in the second
    write_edf(root / 'gappy_plus.edf', start, 0.5, [('II', 'mV', -5, 5, -32768, 32767, 125, lambda t: ecg(t, 0))],
              [0, 0.5, 1.0, 3.0, 3.5, 10.0], version='EDF+D', notes={3: [(3.25, 0.5, 'Electrode off')]})
    # plain EDF of 1999 (a two-digit year), its number of records unknown (-1), and a record cut
    # short at the end of the file
    write_edf(root / 'plain.edf', T0 - 9131 * 86400, 2, [
        ('EEG Fpz-Cz', 'uV', -200, 200, -2048, 2047, 200, lambda t: 50 * math.sin(2 * math.pi * 10 * t)),
        ('Temp', 'degC', 30, 40, 0, 1000, 1, lambda t: 36.6 + t / 100),
    ], [2 * k for k in range(8)], n_records=-1, extra=b'\x01\x02' * 50)
    # BDF+C, 24-bit samples; a signal whose digital minimum and maximum are equal (shown as its
    # digital values)
    write_edf(root / 'eeg_plus.bdf', start, 1, [
        ('Fp1', 'uV', -262144, 262143, -8388608, 8388607, 256, lambda t: 80 * math.sin(2 * math.pi * 9 * t)),
        ('Flat', '', 0, 0, 0, 0, 4, lambda t: 7),
    ], [0, 1, 2], version='BDF+C', bdf=True, notes={1: [(1.5, None, 'Eyes closed')]})


if __name__ == '__main__':
    collection()
    plain()
    session()
    edf()
