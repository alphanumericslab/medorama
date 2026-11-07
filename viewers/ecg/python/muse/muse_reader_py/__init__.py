"""Read GE MUSE resting ECG XML files.

    from muse_reader_py import read_muse
    ecg = read_muse('ecg.xml')          # the ECG viewer's JSON form (viewers/ecg/README.md)

What is read: the rhythm strip (8 recorded leads, the other four derived), its sampling rate
and scale; the test's details (acquisition time, device, site, reason, status, age, sex,
filters); the measurements (rates, intervals, axes, QTc); the computer's statements (Marquette
12SL, `OriginalDiagnosis`) and the confirmed or edited ones (`Diagnosis`, with the overreader
and the time of the edit).  What is never read into the output: the patient's identity (name,
ID, date of birth), the order (accounts, admission, physicians' IDs) and other identifiers;
`patient_key` reads the patient ID on its own, for building a database to import.

Written from the format (MUSE XML, restecg.dtd); no dependencies.
"""

import base64
import math
import re
import struct
import xml.etree.ElementTree as ET

__version__ = '0.1.0'

# measurements: element of RestingECGMeasurements -> (name, unit), as the ECG viewer names them
MEASUREMENTS = [
    ('VentricularRate', 'Vent. rate', 'bpm'), ('AtrialRate', 'Atrial rate', 'bpm'),
    ('PRInterval', 'PR', 'ms'), ('QRSDuration', 'QRS', 'ms'), ('QTInterval', 'QT', 'ms'),
    ('QTCorrected', 'QTc', 'ms'), ('QTcFrederica', 'QTcF', 'ms'), ('QTcFridericia', 'QTcF', 'ms'),
    ('PAxis', 'P axis', '°'), ('RAxis', 'R axis', '°'), ('TAxis', 'T axis', '°'),
    ('QRSCount', 'QRS count', 'beats'),
]
STANDARD = ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6']


def _text(e, path):
    x = e.find(path) if e is not None else None
    return (x.text or '').strip() if x is not None and x.text else ''


def _parse(source):
    """An ElementTree root from a path or bytes (the XML declaration gives the encoding), or the
    root itself (a file read once for several of the functions below)."""
    if isinstance(source, ET.Element):
        return source
    data = source if isinstance(source, (bytes, bytearray)) else open(source, 'rb').read()
    try:
        root = ET.fromstring(bytes(data))
    except ET.ParseError as e:
        raise ValueError(f'not XML: {e}') from None
    if root.tag != 'RestingECG':
        raise ValueError(f'not a MUSE resting ECG (root element {root.tag})')
    return root


def _date(d):
    """MUSE dates: MM-DD-YYYY (or YYYY-MM-DD) -> YYYY-MM-DD."""
    m = re.match(r'^(\d{1,2})[-/](\d{1,2})[-/](\d{4})$', d)
    if m:
        return f'{m.group(3)}-{int(m.group(1)):02d}-{int(m.group(2)):02d}'
    return d


def _number(s):
    """A finite number, else None (as the viewer's muse.js reads it)."""
    try:
        v = float(s)
    except ValueError:
        return None
    if not math.isfinite(v):
        return None
    return int(v) if v.is_integer() else v


def _lines(diagnosis):
    """A diagnosis section's statements as lines: fragments joined until a line ends."""
    out, cur = [], []
    for st in diagnosis.findall('DiagnosisStatement'):
        t = _text(st, 'StmtText')
        if t:
            cur.append(t)
        flags = {(f.text or '').strip().upper() for f in st.findall('StmtFlag')}
        if 'ENDSLINE' in flags and cur:
            out.append(' '.join(cur))
            cur = []
    if cur:
        out.append(' '.join(cur))
    return out


def _waveform(root, kind):
    for w in root.findall('Waveform'):
        if _text(w, 'WaveformType').lower() == kind:
            return w
    return None


def _leads(w):
    """The leads of a waveform section, in µV, with the four derived ones when missing."""
    # the rate: SampleBase × 10^SampleExponent; a base that is not a positive number is 500 Hz,
    # an exponent beyond ±6 is refused (as muse.js reads it)
    base = _number(_text(w, 'SampleBase'))
    base = base if base is not None and base > 0 else 500
    exp = _number(_text(w, 'SampleExponent')) or 0
    if abs(exp) > 6:
        raise ValueError(f'not a sampling rate: SampleExponent {exp}')
    fs = base * 10 ** exp
    got = {}
    for ld in w.findall('LeadData'):
        lead = _text(ld, 'LeadID')
        b64 = ''.join(_text(ld, 'WaveFormData').split())
        raw = base64.b64decode(b64 + '=' * (-len(b64) % 4))
        n = len(raw) // 2
        count = _number(_text(ld, 'LeadSampleCountTotal'))
        if isinstance(count, int) and 0 < count < n:
            n = count
        vals = struct.unpack(f'<{n}h', raw[:2 * n])
        k = _number(_text(ld, 'LeadAmplitudeUnitsPerBit')) or 1
        units = _text(ld, 'LeadAmplitudeUnits').upper()
        scale = k * (1000.0 if units == 'MILLIVOLTS' else 1.0)          # to µV
        got[lead] = [round(v * scale, 3) for v in vals]
    # III, aVR, aVL, aVF from I and II (Einthoven, Goldberger), as carts derive them
    if 'I' in got and 'II' in got:
        a, b = got['I'], got['II']
        got.setdefault('III', [round(y - x, 3) for x, y in zip(a, b)])
        got.setdefault('aVR', [round(-(x + y) / 2, 3) for x, y in zip(a, b)])
        got.setdefault('aVL', [round(x - y / 2, 3) for x, y in zip(a, b)])
        got.setdefault('aVF', [round(y - x / 2, 3) for x, y in zip(a, b)])
    order = [l for l in STANDARD if l in got] + [l for l in got if l not in STANDARD]
    return fs, [{'name': l, 'units': 'uV', 'samples': got[l]} for l in order]


def _filters(w):
    if w is None:
        return ''
    hp = _number(_text(w, 'HighPassFilter'))
    lp = _number(_text(w, 'LowPassFilter'))
    ac = _number(_text(w, 'ACFilter'))
    parts = []
    if hp is not None:
        parts.append(f'{hp / 100:g}–{lp:g} Hz' if lp is not None else f'from {hp / 100:g} Hz')   # high-pass in 0.01 Hz
    if ac:
        parts.append(f'{ac:g} Hz notch')
    return ', '.join(parts)


def read_muse(source):
    """A MUSE XML file (a path or its bytes) in the ECG viewer's JSON form."""
    root = _parse(source)
    test = root.find('TestDemographics')
    pat = root.find('PatientDemographics')
    rhythm = _waveform(root, 'rhythm')
    if rhythm is None:
        raise ValueError('no rhythm waveform in this MUSE file')
    fs, leads = _leads(rhythm)
    acquired = ' '.join(x for x in (_date(_text(test, 'AcquisitionDate')), _text(test, 'AcquisitionTime')) if x)
    age = _text(pat, 'PatientAge')
    age_units = _text(pat, 'AgeUnits').lower()
    fields = []
    for label, value in [
        ('Acquired', acquired),
        ('Device', ' '.join(x for x in (_text(test, 'AcquisitionDevice'), _text(test, 'AcquisitionSoftwareVersion')) if x)),
        ('Site', _text(test, 'SiteName')),
        ('Location', _text(test, 'LocationName')),
        ('Reason', _text(test, 'TestReason') or _text(root.find('Order'), 'TestReason')),
        ('Status', _text(test, 'Status').capitalize()),
        ('Age', f'{age}{"" if age_units in ("", "years") else " " + age_units}' if age else ''),
        ('Sex', {'MALE': 'M', 'FEMALE': 'F'}.get(_text(pat, 'Gender').upper(), _text(pat, 'Gender'))),
        ('Filters', _filters(rhythm)),
    ]:
        if value:
            fields.append([label, value])
    meas_el = root.find('RestingECGMeasurements')
    measurements, seen = [], set()
    for tag, name, unit in MEASUREMENTS:
        v = _number(_text(meas_el, tag))
        if v is not None and name not in seen:
            seen.add(name)
            measurements.append({'name': name, 'value': v, 'unit': unit})
    statements = []
    original = root.find('OriginalDiagnosis')
    current = root.find('Diagnosis')
    program = 'Marquette 12SL' + (f' v{_text(test, "AnalysisSoftwareVersion")}' if _text(test, 'AnalysisSoftwareVersion') else '')
    if original is not None and _lines(original):
        statements.append({'source': 'computer', 'program': program, 'lines': _lines(original)})
    if current is not None and _lines(current):
        confirmed = _text(test, 'Status').upper() == 'CONFIRMED'
        by = ' '.join(x for x in (_text(test, 'OverreaderFirstName'), _text(test, 'OverreaderLastName')) if x)
        edited = ' '.join(x for x in (_date(_text(test, 'EditDate')), _text(test, 'EditTime')) if x)
        same = original is not None and _lines(current) == _lines(original)
        if confirmed:
            statements.append({'source': 'confirmed', 'by': by, 'time': edited, 'lines': _lines(current)})
        elif not same:
            statements.append({'source': 'edited', 'time': edited, 'lines': _lines(current)})
    return {
        'name': 'MUSE ECG',
        'fs': fs,
        'leads': leads,
        'meta': {'fields': fields, 'measurements': measurements, 'statements': statements, 'notes': []},
    }


def patient_key(source):
    """The patient ID a MUSE file names (PHI: for linking it to its patient, never shown)."""
    return _text(_parse(source).find('PatientDemographics'), 'PatientID')


def acquired(source):
    """The acquisition time as written (YYYY-MM-DD HH:MM:SS, the cart's clock), or ''."""
    test = _parse(source).find('TestDemographics')
    return ' '.join(x for x in (_date(_text(test, 'AcquisitionDate')), _text(test, 'AcquisitionTime')) if x)
