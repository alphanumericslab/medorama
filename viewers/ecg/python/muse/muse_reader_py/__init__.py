"""Read GE MUSE resting ECG XML files.

    from muse_reader_py import read_muse
    ecg = read_muse('ecg.xml')          # the ECG viewer's JSON form (viewers/ecg/README.md)

What is read: the rhythm strip (8 recorded leads, the other four derived), its sampling rate
and scale, and the median beats with the P, QRS and T onsets marked; the test's details (acquisition time, device, site, reason, status, age, sex,
filters); the measurements (rates, intervals, axes, QTc); the computer's statements (Marquette
12SL, `OriginalDiagnosis`) and the confirmed or edited ones (`Diagnosis`, with the overreader
and the time of the edit).  What is never read into the output: the patient's identity (name,
ID, date of birth), the order (accounts, admission, physicians' IDs) and other identifiers;
`patient_key` reads the patient ID on its own, for building a database to import.

The rates are read as the viewer's muse.js reads them: fs from the rhythm strip's SampleBase,
else unknown (`fs` None, `rateUnknown` True: the viewer shows the samples by number, and no
WFDB record is written); each file is checked against a list of known GE formats, and what
does not fit is reported in the notes.

Written from the format (MUSE XML, restecg.dtd); no dependencies.
"""

import base64
import math
import re
import struct
import xml.etree.ElementTree as ET

__version__ = '0.4.0'

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


_BOMS = ((b'\xef\xbb\xbf', 'utf-8'), (b'\xff\xfe', 'utf-16-le'), (b'\xfe\xff', 'utf-16-be'))


def _xml_text(data):
    """XML bytes as the parser takes them, as the viewer's muse.js reads them: a byte order mark
    decides the encoding (whatever the declaration says); UTF-16 without one and the encoding the
    declaration names are the parser's own reading; a declaration of UTF-16 in 8-bit text is read
    as UTF-8."""
    data = bytes(data)
    for mark, enc in _BOMS:
        if data.startswith(mark):
            return data[len(mark):].decode(enc, 'replace')
    if len(data) >= 4 and (bool(data[0]) != bool(data[1])) and (bool(data[2]) != bool(data[3])):
        return data                                         # UTF-16 without a mark
    m = re.match(rb'^\s*<\?xml[^>]*encoding\s*=\s*["\']([\w.:-]+)["\']', data[:200], re.I)
    if m and re.match(rb'(utf-?16|ucs-?2)', m.group(1), re.I):
        return data.decode('utf-8', 'replace')
    return data


def _parse(source):
    """An ElementTree root from a path or bytes (in their encoding: _xml_text), or the root
    itself (a file read once for several of the functions below)."""
    if isinstance(source, ET.Element):
        return source
    data = source if isinstance(source, (bytes, bytearray)) else open(source, 'rb').read()
    try:
        root = ET.fromstring(_xml_text(data))
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


# The known GE formats: (cart, 12SL program) -> the rates and lengths they record with (the same
# table as muse.js).
# (device, program, fs, median fs, rhythm samples, median samples, measurement rate)
FORMATS = [
    ('CASE', '12SL 20.1', 500, 500, 5000, 600, 500), ('CASE', '12SL V15C', 500, 500, 5000, 600, 500),
    ('CASE', '12SL V19', 500, 500, 5000, 600, 500), ('CASE', '12SL V21', 500, 500, 5000, 600, 500),
    ('CSYS', '237', 500, 500, 5000, 600, 500), ('CSYS', '239', 500, 500, 5000, 600, 500),
    ('D3K', '241', 500, 500, 5000, 600, 500), ('Datam', 'NO SERIAL', 500, 500, 5000, 600, 500),
    ('MAC 8', '250', 250, 250, 2500, 300, 500), ('MAC V', '243', 500, 500, 5000, 600, 500),
    ('MAC2K', '241', 500, 500, 5000, 600, 500), ('MAC35', '241', 500, 500, 5000, 600, 500),
    ('MAC5', '245', 500, 500, 5000, 600, 500),
    ('MAC55', '237', 500, 500, 5000, 600, 500), ('MAC55', '239', 500, 500, 5000, 600, 500),
    ('MAC55', '239', 250, 250, 2500, 300, 500), ('MAC55', '241', 500, 500, 5000, 600, 500),
    ('MAC55', '241 HD', 500, 500, 5000, 600, 500),
    ('MAC5K', '229', 250, 250, 2500, 300, 500), ('MAC5K', '231', 250, 250, 2500, 300, 500),
    ('MAC5K', '231', 500, 500, 5000, 600, 500), ('MAC5K', '233', 250, 250, 2500, 300, 500),
    ('MAC5K', '233', 500, 500, 5000, 600, 500), ('MAC5K', '235', 250, 250, 2500, 300, 500),
    ('MAC5K', '235', 500, 500, 5000, 600, 500), ('MAC5K', '237', 250, 250, 2500, 300, 500),
    ('MAC5K', '237', 500, 500, 5000, 600, 500), ('MAC5K', '239', 500, 500, 5000, 600, 500),
    ('MAC80', '239', 500, 500, 5000, 600, 500), ('MV360', '243', 500, 500, 5000, 600, 500),
]
# what a check found, as the viewer says it
CHECKS = {
    'BAD_SAMPLE_BASE': 'the sampling rate is not given (SampleBase): shown by sample number',
    'NEW_FORMAT': 'a cart, 12SL program, rate or length not seen before',
    'SAMPLE_EXPONENT_NOT_0': 'a SampleExponent other than 0',
    'MEDIAN_RATE_DIFFERS': "the median beats are not at the rhythm strip's rate",
    'LEAD_LENGTHS_DIFFER': 'the leads have different lengths',
    'RHYTHM_NOT_10_S': 'the rhythm strip is not 10 s long',
    'MEDIAN_NOT_1.2_S': 'the median beats are not 1.2 s long',
    'MULTIPLE_RHYTHM': 'more than one rhythm strip (the first is shown)',
    'MEASUREMENT_RATE_UNKNOWN': 'the rate of the P, QRS and T onsets is unknown: they are not marked',
    'ONSETS_CONTRADICT_BASE': 'the onsets do not give the QRS duration at their rate',
    'QTRGGR_CONTRADICTS_FS': 'the QRS trigger (QTRGGR) is not at 496 ms at this rate',
}
RR_SCALE = {500: 1, 250: 4}             # ms per unit of GlobalRR, by fs
TRIGGER_MS = 496                        # QTRGGR = TRIGGER_MS x fs / 1000
ONSETS = [('POnset', 'P'), ('POffset', 'P end'), ('QOnset', 'QRS'), ('QOffset', 'QRS end'), ('TOffset', 'T end')]


def _rate(base, exponent):
    """base x 10^exponent in Hz, or None when that is not a rate (a missing exponent is 0)."""
    b = _number(base)
    e = _number(exponent) if exponent.strip() else 0
    if b is None or not b > 0 or e is None or not float(e).is_integer() or abs(e) > 6:
        return None
    r = b * 10 ** int(e)
    return int(r) if float(r).is_integer() else r


def _samples(w):
    """The samples of a waveform section: (rate or None, exponent, {lead: [µV]})."""
    got = {}
    for ld in w.findall('LeadData'):
        lead = _text(ld, 'LeadID')
        size = _number(_text(ld, 'LeadSampleSize'))
        if size is not None and size != 2:
            raise ValueError(f"lead {lead}: samples of {size} bytes (MUSE's are 16-bit)")
        k = _number(_text(ld, 'LeadAmplitudeUnitsPerBit'))
        if k is None or not k > 0:
            raise ValueError(f'lead {lead}: no amplitude scale (LeadAmplitudeUnitsPerBit)')
        b64 = ''.join(_text(ld, 'WaveFormData').split())
        raw = base64.b64decode(b64 + '=' * (-len(b64) % 4))
        n = len(raw) // 2
        count = _number(_text(ld, 'LeadSampleCountTotal'))
        if isinstance(count, int) and 0 < count < n:
            n = count
        vals = struct.unpack(f'<{n}h', raw[:2 * n])
        units = _text(ld, 'LeadAmplitudeUnits').upper()
        scale = k * (1000.0 if units == 'MILLIVOLTS' else 1.0)          # to µV
        got[lead] = [round(v * scale, 3) for v in vals]
    return _rate(_text(w, 'SampleBase'), _text(w, 'SampleExponent')), _number(_text(w, 'SampleExponent')) or 0, got


def _leads(raw):
    """The leads in µV, with III, aVR, aVL, aVF derived from I and II when missing (Einthoven,
    Goldberger), as carts derive them."""
    got = dict(raw)
    if 'I' in got and 'II' in got:
        a, b = got['I'], got['II']
        got.setdefault('III', [round(y - x, 3) for x, y in zip(a, b)])
        got.setdefault('aVR', [round(-(x + y) / 2, 3) for x, y in zip(a, b)])
        got.setdefault('aVL', [round(x - y / 2, 3) for x, y in zip(a, b)])
        got.setdefault('aVF', [round(y - x / 2, 3) for x, y in zip(a, b)])
    order = [l for l in STANDARD if l in got] + [l for l in got if l not in STANDARD]
    return [{'name': l, 'units': 'uV', 'samples': got[l]} for l in order]


def _lengths(raw):
    ns = [len(v) for v in raw.values()]
    return (max(ns) if ns else 0), len(set(ns)) > 1


def _qrs_fits(m, r):
    a, b, d = (_number(_text(m, k)) for k in ('QOnset', 'QOffset', 'QRSDuration'))
    if None in (a, b, d) or not r:
        return None
    return abs((b - a) * 1000 / r - d) <= 1000 / r + 1


def _sampling(root, rhythms, rw, mw):
    """The rates of a file and what does not fit the known formats (muse.js `sampling`)."""
    test = root.find('TestDemographics')
    device = _text(test, 'AcquisitionDevice')
    program = _text(test, 'AnalysisSoftwareVersion')
    rows = [f for f in FORMATS if f[0] == device and f[1] == program]
    warnings = []

    def warn(c):
        if c not in warnings:
            warnings.append(c)
    if rhythms > 1:
        warn('MULTIPLE_RHYTHM')
    fs, rexp, rraw = rw
    if fs is None:
        warn('BAD_SAMPLE_BASE')
    rn, rdiffer = _lengths(rraw)
    mfs, mexp, mraw = mw if mw else (None, 0, {})
    mn, mdiffer = _lengths(mraw) if mw else (None, False)
    if rexp or mexp:
        warn('SAMPLE_EXPONENT_NOT_0')
    if rdiffer or mdiffer:
        warn('LEAD_LENGTHS_DIFFER')
    if fs and mfs and mfs != fs:
        warn('MEDIAN_RATE_DIFFERS')
    if fs and rn != 10 * fs:
        warn('RHYTHM_NOT_10_S')
    if mfs and mn != round(1.2 * mfs):
        warn('MEDIAN_NOT_1.2_S')
    fmt = 'unreadable'
    if fs is not None:
        fmt = 'known' if any((f[2], f[3], f[4], f[5]) == (fs, mfs, rn, mn) for f in rows) else 'new'
        if fmt == 'new':
            warn('NEW_FORMAT')
    qtrggr = _number(_text(root.find('QRSTimesTypes'), 'QTRGGR'))
    if fs and qtrggr is not None and qtrggr != TRIGGER_MS * fs / 1000:
        warn('QTRGGR_CONTRADICTS_FS')
    fin = root.find('RestingECGMeasurements')
    orig = root.find('OriginalRestingECGMeasurements')

    def stored(x):
        return _rate(_text(x, 'ECGSampleBase'), _text(x, 'ECGSampleExponent'))

    def same(k):
        f = _number(_text(fin, k))
        return f is not None and f == _number(_text(orig, k))
    as_original = same('QOnset') and same('QOffset')
    fits = [x for x in (250, 500, 1000) if _qrs_fits(fin, x)]
    known = sorted({f[6] for f in rows if fs is None or f[2] == fs})
    edited = _number(_text(fin, 'QRSDuration')) != _number(_text(orig, 'QRSDuration'))
    mrate = stored(fin)
    if mrate:
        if _qrs_fits(fin, mrate) is False and not (edited and as_original and _qrs_fits(orig, stored(orig))):
            warn('ONSETS_CONTRADICT_BASE')
    elif as_original and stored(orig) and _qrs_fits(orig, stored(orig)):
        mrate = stored(orig)
    elif len(fits) == 1:
        mrate = fits[0]
    elif len(known) == 1:
        mrate = known[0]
    else:
        mrate = None
        if any(_number(_text(fin, k)) is not None for k, _ in ONSETS):
            warn('MEASUREMENT_RATE_UNKNOWN')
    rr = _number(_text(root.find('QRSTimesTypes'), 'GlobalRR'))
    return {'device': device, 'program': program, 'fs': fs, 'mfs': mfs, 'format': fmt, 'warnings': warnings,
            'mrate': mrate, 'rr_ms': rr * RR_SCALE[fs] if rr is not None and fs in RR_SCALE else None}


def _recording(sm):
    """The recording as the report prints it: "MAC5K · 12SL 237 · 250 Hz · known format"."""
    what = {'known': 'known format', 'new': 'new format', 'unreadable': 'rate unknown'}[sm['format']]
    parts = [sm['device'], f"12SL {sm['program']}" if sm['program'] else '', f"{sm['fs']:g} Hz" if sm['fs'] else '', what]
    return ' · '.join(x for x in parts if x)


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
    waves = root.findall('Waveform')
    rhythms = [w for w in waves if _text(w, 'WaveformType').lower() == 'rhythm']
    if not rhythms:
        raise ValueError('no rhythm waveform in this MUSE file')
    rhythm = rhythms[0]
    median_w = next((w for w in waves if _text(w, 'WaveformType').lower() == 'median'), None)
    rw = _samples(rhythm)
    mw = _samples(median_w) if median_w is not None else None
    sm = _sampling(root, len(rhythms), rw, mw)
    fs = sm['fs']
    leads = _leads(rw[2])
    acquired = ' '.join(x for x in (_date(_text(test, 'AcquisitionDate')), _text(test, 'AcquisitionTime')) if x)
    age = _text(pat, 'PatientAge')
    age_units = _text(pat, 'AgeUnits').lower()
    fields = []
    for label, value in [
        ('Acquired', acquired),
        ('Device', ' '.join(x for x in (_text(test, 'AcquisitionDevice'), _text(test, 'AcquisitionSoftwareVersion')) if x)),
        ('Recording', _recording(sm)),
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
    for tag, name in (('SystolicBP', 'Systolic BP'), ('DiastolicBP', 'Diastolic BP')):
        v = _number(_text(meas_el, tag))
        if v is not None:
            measurements.append({'name': name, 'value': v, 'unit': 'mmHg'})
    if sm['rr_ms'] is not None:
        measurements.append({'name': 'RR', 'value': sm['rr_ms'], 'unit': 'ms'})
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
    notes = [f'Check: {CHECKS.get(c, c)} ({c})' for c in sm['warnings']]
    out = {
        'name': 'MUSE ECG',
        'fs': fs,
        'leads': leads,
        'meta': {'fields': fields, 'measurements': measurements, 'statements': statements, 'notes': notes},
    }
    if fs is None:
        out['rateUnknown'] = True
    if mw and mw[2]:
        # the onsets and offsets on the median beat, in its samples: at the onsets' rate in the file
        marks = []
        if mw[0] and sm['mrate']:
            for tag, name in ONSETS:
                v = _number(_text(meas_el, tag))
                if v is not None:
                    s = round(v * mw[0] / sm['mrate'], 3)
                    marks.append({'name': name, 'sample': int(s) if float(s).is_integer() else s})
        out['median'] = {'fs': mw[0], 'leads': _leads(mw[2]), 'marks': marks}
        if mw[0] is None:
            out['median']['rateUnknown'] = True
    return out


def patient_key(source):
    """The patient ID a MUSE file names (PHI: for linking it to its patient, never shown)."""
    return _text(_parse(source).find('PatientDemographics'), 'PatientID')


def acquired(source):
    """The acquisition time as written (YYYY-MM-DD HH:MM:SS, the cart's clock), or ''."""
    test = _parse(source).find('TestDemographics')
    return ' '.join(x for x in (_date(_text(test, 'AcquisitionDate')), _text(test, 'AcquisitionTime')) if x)
