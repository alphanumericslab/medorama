"""DICOM studies as databases for Medorama, and DICOM ECGs as WFDB records.

    from dicom_index_py import index_folder, measurements, waveform
    header, rows, skipped = index_folder('/data/echo', base='/data')

`index_folder` reads every DICOM file under a folder (headers only, never the pixels) and makes
one row per object a viewer shows: an ultrasound loop, an angiography run, a still, a series of
slices (CT, MR, NM), a structured report, a PDF, an ECG.  Each row names its patient (the
patient ID: PHI, the key that links the row to its patient), its time, its category (echo, cath,
img, ecg, holter), what it is, and its file or folder; the measurements of structured reports
(echo, cath) and of ECGs are columns of their own, named with their unit (`Left Ventricular
Ejection Fraction (%)`, `QT (ms)`).  Patients' names and dates of birth are never written.

Files are only read.  Uses pydicom (MIT).
"""

import math
import os
from pathlib import Path

import pydicom
from pydicom.errors import InvalidDicomError

__version__ = '0.1.0'

ECG_SOPS = {'1.2.840.10008.5.1.4.1.1.9.1.1', '1.2.840.10008.5.1.4.1.1.9.1.2'}
HOLTER_SOPS = {'1.2.840.10008.5.1.4.1.1.9.1.3'}
PDF_SOP = '1.2.840.10008.5.1.4.1.1.104.1'
# modalities of the cath lab: angiography, fluoroscopy, intravascular imaging
CATH = {'XA', 'RF', 'IVUS', 'IVOCT', 'OCT'}

# ECG measurements as the ECG viewer names them (a waveform's annotations, a report's items)
ECG_NAMES = [
    (('ventricular rate', 'heart rate', 'vent rate', 'hr'), 'Vent. rate'), (('atrial rate',), 'Atrial rate'),
    (('pr interval', 'pr'), 'PR'), (('qrs duration', 'qrs interval', 'qrs'), 'QRS'), (('qt interval', 'qt'), 'QT'),
    (('qtc interval', 'qtc', 'qt corrected'), 'QTc'), (('rr interval', 'rr'), 'RR'),
    (('p axis',), 'P axis'), (('qrs axis', 'r axis'), 'R axis'), (('t axis',), 'T axis'),
]


def _s(ds, name, default=''):
    v = ds.get(name, default)
    if v is None:
        return default
    if isinstance(v, pydicom.multival.MultiValue):
        return '\\'.join(str(x) for x in v)
    return str(v).strip()


def _time(ds):
    """The object's time as written (YYYY-MM-DD HH:MM:SS, the device's clock), or ''."""
    dt = _s(ds, 'AcquisitionDateTime')
    if not dt:
        for d, t in (('AcquisitionDate', 'AcquisitionTime'), ('ContentDate', 'ContentTime'), ('SeriesDate', 'SeriesTime'), ('StudyDate', 'StudyTime')):
            if _s(ds, d):
                dt = _s(ds, d) + _s(ds, t)
                break
    if len(dt) < 8 or not dt[:8].isdigit():
        return ''
    out = f'{dt[:4]}-{dt[4:6]}-{dt[6:8]}'
    t = dt[8:].split('.')[0].split('+')[0].split('-')[0]
    if len(t) >= 4:
        out += f' {t[:2]}:{t[2:4]}' + (f':{t[4:6]}' if len(t) >= 6 else '')
    return out


def _code(seq):
    if not seq:
        return None, None, None
    c = seq[0]
    return _s(c, 'CodeValue'), _s(c, 'CodingSchemeDesignator'), _s(c, 'CodeMeaning')


# UCUM units as people write them
READABLE = {'/min': 'bpm', '{beats}/min': 'bpm', '{H.B.}/min': 'bpm', 'deg': '°', 'mm[Hg]': 'mmHg'}


def _unit(seq):
    value, scheme, meaning = _code(seq)
    if scheme == 'UCUM' and value and value != '1':
        return READABLE.get(value, value)
    return '' if value == '1' else READABLE.get(meaning or value or '', meaning or value or '')


def ecg_name(name):
    """An ECG measurement's name as the ECG viewer writes it (QT, QTc, PR …), else as given."""
    low = ' '.join(name.lower().replace('-', ' ').split())
    for words, canon in ECG_NAMES:
        if low in words:
            return canon
    return name


def measurements(ds):
    """The NUM items of a structured report, in order: [(name, value, unit)].  The same name met
    again (a repeated measurement) keeps its first value, unless a later one is the mean."""
    out = {}
    order = []

    def walk(seq):
        for it in seq or []:
            vt = _s(it, 'ValueType')
            if vt == 'NUM' and it.get('MeasuredValueSequence'):
                _, _, name = _code(it.get('ConceptNameCodeSequence'))
                mv = it.MeasuredValueSequence[0]
                try:
                    v = float(mv.NumericValue)
                except (TypeError, ValueError, AttributeError):
                    v = None
                if name and v is not None and math.isfinite(v):
                    unit = _unit(mv.get('MeasurementUnitsCodeSequence'))
                    mean = any(_code(m.get('ConceptCodeSequence'))[2] in ('Mean', 'Average')
                               for m in it.get('ContentSequence', []) if _s(m, 'ValueType') == 'CODE')
                    key = (name, unit)
                    if key not in out:
                        order.append(key)
                        out[key] = v
                    elif mean:
                        out[key] = v
            walk(it.get('ContentSequence'))

    walk(ds.get('ContentSequence'))
    return [(n, out[(n, u)], u) for n, u in order]


def report_text(ds):
    """The TEXT items of a report (findings, conclusions), joined."""
    out = []

    def walk(seq):
        for it in seq or []:
            if _s(it, 'ValueType') == 'TEXT' and _s(it, 'TextValue'):
                out.append(_s(it, 'TextValue'))
            walk(it.get('ContentSequence'))

    walk(ds.get('ContentSequence'))
    return '; '.join(out)


def ecg_annotations(ds):
    """A waveform's annotations: [(name, value, unit)] and its statements (text)."""
    ms, text = [], []
    for a in ds.get('WaveformAnnotationSequence', []):
        if _s(a, 'UnformattedTextValue'):
            text.append(_s(a, 'UnformattedTextValue'))
            continue
        _, _, name = _code(a.get('ConceptNameCodeSequence'))
        try:
            v = float(a.NumericValue[0] if isinstance(a.get('NumericValue'), pydicom.multival.MultiValue) else a.NumericValue)
        except (TypeError, ValueError, AttributeError):
            continue
        if name:
            ms.append((ecg_name(name), v, _unit(a.get('MeasurementUnitsCodeSequence')) or ''))
    return ms, text


def waveform(ds):
    """The longest group of a DICOM waveform: (fs, [(name, units, samples as floats)])."""
    best = None
    for i, g in enumerate(ds.get('WaveformSequence', [])):
        fs = float(g.SamplingFrequency)
        n = int(g.NumberOfWaveformSamples)
        if best is None or n / fs > best[0]:
            best = (n / fs, i)
    if best is None:
        raise ValueError('no waveform in this DICOM file')
    i = best[1]
    g = ds.WaveformSequence[i]
    data = ds.waveform_array(i)            # samples × channels, scaled by the sensitivity
    chans = []
    for k, c in enumerate(g.ChannelDefinitionSequence):
        _, _, meaning = _code(c.get('ChannelSourceSequence'))
        name = (meaning or _s(c, 'ChannelLabel') or f'Channel {k + 1}').replace('Lead ', '').strip()
        units = _unit(c.get('ChannelSensitivityUnitsSequence')) or 'uV'
        chans.append((name, units, [float(x) for x in data[:, k]]))
    return float(g.SamplingFrequency), chans


def _read(path):
    try:
        return pydicom.dcmread(path, stop_before_pixels=True)
    except (InvalidDicomError, OSError, ValueError, KeyError, AttributeError, EOFError):
        return None


def _category(ds, sop):
    mod = _s(ds, 'Modality').upper()
    if sop in ECG_SOPS:
        return 'ecg'
    if sop in HOLTER_SOPS:
        return 'holter'
    if mod == 'US':
        return 'echo'
    if mod in CATH:
        return 'cath'
    if mod in ('SR', 'DOC') or sop == PDF_SOP:
        # a report's category: from its title's words (echo, cath, ECG), else imaging
        title = (_code(ds.get('ConceptNameCodeSequence'))[2] or _s(ds, 'DocumentTitle') or '').lower()
        if 'echo' in title or 'ultrasound' in title:
            return 'echo'
        if 'cath' in title or 'angio' in title or 'ivus' in title:
            return 'cath'
        if 'ecg' in title or 'electrocardiogram' in title:
            return 'ecg'
    return 'img'


def _what(ds, sop):
    """(name, kind): what the object is called, and how it is shown."""
    frames = int(ds.get('NumberOfFrames', 1) or 1)
    mod = _s(ds, 'Modality').upper()
    desc = _s(ds, 'SeriesDescription') or _s(ds, 'StudyDescription')
    if sop == PDF_SOP:
        return _s(ds, 'DocumentTitle') or desc or 'Report (PDF)', 'document'
    if sop in ECG_SOPS:
        return '12-lead ECG', 'ecg'
    if sop in HOLTER_SOPS:
        return 'Holter ECG', 'holter'
    if mod == 'SR' or 'ContentSequence' in ds:
        return _code(ds.get('ConceptNameCodeSequence'))[2] or 'Report', 'report'
    if frames > 1:
        return desc or ({'US': 'Echo loop', 'XA': 'Angiography run'}.get(mod) or f'{mod} cine'), 'cine'
    return desc or f'{mod} image', 'image'


def index_folder(folder, base=None):
    """Rows for every DICOM object under `folder`: (header, rows, skipped).  File paths are
    relative to `base` (default: `folder`).  `skipped`: (path, why)."""
    folder = Path(folder)
    base = Path(base) if base else folder
    files = []
    for root, dirs, names in os.walk(folder):
        dirs[:] = sorted(d for d in dirs if not d.startswith('.'))
        for n in sorted(names):
            if n.startswith('.') or n.upper() == 'DICOMDIR':
                continue
            files.append(Path(root) / n)
    objs = []
    skipped = []
    for p in files:
        try:
            ds = _read(p)
            if ds is None or not ('SOPClassUID' in ds or 'Modality' in ds):
                continue
        except Exception as e:      # a damaged file (nested too deep …): listed, the others go on
            skipped.append((str(p), f'not read: {type(e).__name__}'))
            continue
        objs.append((p, ds))
    # which patients each folder holds (a folder is linked only when it holds one patient's files)
    patients_in = {}
    for p, ds in objs:
        patients_in.setdefault(p.parent, set()).add(_s(ds, 'PatientID'))
    rows = []
    stacks = {}
    for p, ds in objs:
        try:
            sop = _s(ds, 'SOPClassUID')
            name, kind = _what(ds, sop)
            if kind == 'image':
                stacks.setdefault((_s(ds, 'SeriesInstanceUID') or str(p)), []).append((p, ds))
                continue
            rows.append(_row(ds, sop, name, p, base))
        except Exception as e:      # a damaged header: this file is listed, the others go on
            skipped.append((str(p), f'not read: {e}'))
    for key, members in stacks.items():
        p, ds = members[0]
        try:
            name, _ = _what(ds, _s(ds, 'SOPClassUID'))
            row = _row(ds, _s(ds, 'SOPClassUID'), name, p, base)
        except Exception as e:
            skipped.append((str(p), f'not read: {e}'))
            continue
        if len(members) == 1:
            rows.append(row)
            continue
        dirs = {m[0].parent for m in members}
        if len(dirs) != 1:
            skipped.append((str(p.parent), f'series {name!r}: its files are in several folders'))
            continue
        d = dirs.pop()
        if len(patients_in[d]) != 1:
            skipped.append((str(d), f'series {name!r}: the folder holds other patients\' files too'))
            continue
        row['file'] = Path(os.path.relpath(d, base)).as_posix()   # '/' on every system
        row['images'] = len(members)
        rows.append(row)
    header = ['mrn', 'time', 'category', 'name', 'modality', 'description', 'study_uid', 'series_uid', 'images', 'frames', 'fps']
    extra = []
    for r in rows:
        for k in r:
            if k not in header and k not in extra and k not in ('interpretation', 'file'):
                extra.append(k)
    header += extra + ['interpretation', 'file']
    rows.sort(key=lambda r: (r['mrn'], r['time'], r['file']))
    return header, [[r.get(h, '') for h in header] for r in rows], skipped


def _num(x):
    return int(x) if float(x).is_integer() else round(float(x), 6)


def _row(ds, sop, name, path, base):
    frames = int(ds.get('NumberOfFrames', 1) or 1)
    fps = ''
    if frames > 1:
        ft = ds.get('FrameTime')
        rate = ds.get('CineRate') or ds.get('RecommendedDisplayFrameRate')
        if ft:
            try:
                fps = round(1000 / float(ft), 2)
            except (TypeError, ValueError, ZeroDivisionError):
                fps = ''
        elif rate:
            fps = rate
    r = {
        'mrn': _s(ds, 'PatientID'), 'time': _time(ds), 'category': _category(ds, sop), 'name': name,
        'modality': _s(ds, 'Modality'), 'description': _s(ds, 'StudyDescription'),
        'study_uid': _s(ds, 'StudyInstanceUID'), 'series_uid': _s(ds, 'SeriesInstanceUID'),
        'images': 1, 'frames': frames if frames > 1 else '', 'fps': fps,
        'file': Path(os.path.relpath(path, base)).as_posix(),
    }
    if sop in ECG_SOPS or sop in HOLTER_SOPS:
        ms, text = ecg_annotations(ds)
        for n, v, u in ms:
            r.setdefault(f'{n} ({u})' if u else n, _num(v))
        r['interpretation'] = '; '.join(text)
    elif 'ContentSequence' in ds:
        for n, v, u in measurements(ds):
            r.setdefault(f'{n} ({u})' if u else n, _num(v))
        r['interpretation'] = report_text(ds)
    return r


def _one_line(x):
    """Text as one header line holds it: line breaks and runs of white space as one space."""
    return ' '.join(str(x).split())


def write_wfdb(path, out_dir, name=None):
    """A DICOM ECG (12-lead or Holter) as a WFDB record: format 16, 1 µV per unit; the
    annotations' measurements and statements in the header's comments."""
    ds = pydicom.dcmread(path)
    fs, chans = waveform(ds)
    ms, text = ecg_annotations(ds)
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    name = name or Path(path).stem
    scale = {'uV': 1.0, 'mV': 1000.0, 'V': 1e6}
    vals = [[max(-32767, min(32767, round(x * scale.get(u, 1.0)))) for x in s] for _, u, s in chans]
    n = min(len(v) for v in vals)
    data = bytearray()
    for i in range(n):
        for v in vals:
            data += int(v[i]).to_bytes(2, 'little', signed=True)
    (out / f'{name}.dat').write_bytes(bytes(data))
    lines = [f'{name} {len(chans)} {fs:g} {n}']
    for (cname, _, _), v in zip(chans, vals):
        check = sum(v[:n]) & 0xffff
        check = check - 0x10000 if check >= 0x8000 else check
        lines.append(f'{name}.dat 16 1000/mV 16 0 {v[0] if n else 0} {check} 0 {_one_line(cname)}')
    when = _time(ds)
    if when:
        lines.append(f'# Acquired: {when}')
    for mn, mv, mu in ms:
        lines.append(f'# {_one_line(mn)}: {_num(mv)} {_one_line(mu)}'.rstrip())
    if text:
        # a text value's lines are statements of their own
        lines.append(f'# Dx: {"; ".join(x for t in text for x in map(_one_line, str(t).splitlines()) if x)}')
    (out / f'{name}.hea').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    return out / f'{name}.hea'
