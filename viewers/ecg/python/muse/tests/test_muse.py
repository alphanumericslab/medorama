"""The MUSE reader on a synthetic MUSE file (tests/fixtures of the ECG viewer; made-up patient)."""

import csv
import json
import shutil
from pathlib import Path

import pytest

from muse_reader_py import acquired, patient_key, read_muse
from muse_reader_py.cli import main

FIX = Path(__file__).resolve().parents[3] / 'tests' / 'fixtures'
XML = FIX / 'synthetic_muse.xml'


def test_leads_scale_and_derived():
    e = read_muse(str(XML))
    assert e['fs'] == 500
    names = [l['name'] for l in e['leads']]
    assert names == ['I', 'II', 'III', 'aVR', 'aVL', 'aVF', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6']
    by = {l['name']: l['samples'] for l in e['leads']}
    assert all(len(v) == 1000 and l['units'] == 'uV' for l, v in zip(e['leads'], by.values()))
    i, ii = by['I'], by['II']
    n = max(range(1000), key=lambda k: ii[k])            # the R peak, 1 mV in II
    assert 950 < ii[n] < 1050
    assert by['III'][n] == pytest.approx(ii[n] - i[n], abs=0.01)
    assert by['aVR'][n] == pytest.approx(-(i[n] + ii[n]) / 2, abs=0.01)
    assert by['aVL'][n] == pytest.approx(i[n] - ii[n] / 2, abs=0.01)
    assert by['aVF'][n] == pytest.approx(ii[n] - i[n] / 2, abs=0.01)


def test_details_measurements_statements():
    m = read_muse(XML.read_bytes())['meta']
    f = dict(m['fields'])
    assert f['Acquired'] == '2020-03-14 08:15:30'
    assert f['Site'] == 'Clinique Médicale'               # ISO-8859-1
    assert f['Reason'] == 'Chest pain' and f['Sex'] == 'F' and f['Age'] == '47'
    assert f['Filters'] == '0.16–150 Hz, 60 Hz notch'
    q = {x['name']: (x['value'], x['unit']) for x in m['measurements']}
    assert q['QT'] == (410, 'ms') and q['R axis'] == (-10, '°') and q['Vent. rate'] == (60, 'bpm')
    comp, conf = m['statements']
    assert comp == {'source': 'computer', 'program': 'Marquette 12SL v241',
                    'lines': ['Sinus rhythm with borderline left axis', 'Borderline ECG']}
    assert conf['source'] == 'confirmed' and conf['by'] == 'TEST READER' and conf['time'] == '2020-03-15 10:02:00'
    assert conf['lines'][0] == 'Normal sinus rhythm'


def test_no_identity_in_the_output():
    text = json.dumps(read_muse(str(XML)))
    for x in ('SYN000123', 'TESTPATIENT', 'SYNTHETIC', '1970'):
        assert x not in text
    assert patient_key(str(XML)) == 'SYN000123'
    assert acquired(str(XML)) == '2020-03-14 08:15:30'


def test_edited_not_confirmed():
    data = XML.read_bytes().replace(b'<Status>CONFIRMED</Status>', b'<Status>UNCONFIRMED</Status>')
    st = read_muse(data)['meta']['statements']
    assert [s['source'] for s in st] == ['computer', 'edited'] and 'by' not in st[1]


def test_not_muse():
    with pytest.raises(ValueError):
        read_muse(b'<?xml version="1.0"?><Other/>')


def test_golden_json():
    """The JavaScript reader (viewers/ecg/muse.js) is tested against the same file."""
    assert read_muse(str(XML)) == json.loads((FIX / 'synthetic_muse.json').read_text(encoding='utf-8'))


def test_cli_wfdb_and_index(tmp_path, capsys):
    main(['wfdb', str(XML), str(tmp_path / 'rec'), '--name', 'ecg1'])
    hea = (tmp_path / 'rec' / 'ecg1.hea').read_text(encoding='utf-8').splitlines()
    assert hea[0] == 'ecg1 12 500 1000'
    assert hea[1].startswith('ecg1.dat 16 1000/mV 16 0 ') and hea[1].endswith(' I')
    assert '# QT: 410 ms' in hea and '# Dx: Sinus rhythm with borderline left axis; Borderline ECG' in hea
    assert not any('SYN000123' in h or 'TESTPATIENT' in h for h in hea)
    assert (tmp_path / 'rec' / 'ecg1.dat').stat().st_size == 12 * 1000 * 2
    src = tmp_path / 'ecgs' / 'a'
    src.mkdir(parents=True)
    shutil.copy(XML, src / 'one.xml')
    (src / 'broken.xml').write_text('<RestingECG>')
    main(['index', str(tmp_path / 'ecgs'), '-o', str(tmp_path / 'ecg.csv')])
    rows = list(csv.DictReader(open(tmp_path / 'ecg.csv', encoding='utf-8')))
    assert len(rows) == 1
    r = rows[0]
    assert r['mrn'] == 'SYN000123' and r['time'] == '2020-03-14 08:15:30' and r['category'] == 'ecg'
    assert r['QT (ms)'] == '410' and r['QTc (ms)'] == '410' and r['value'] == '60' and r['unit'] == 'bpm'
    assert r['interpretation'].startswith('Sinus rhythm') and r['confirmed'].startswith('Normal sinus')
    assert r['file'] == str(Path('ecgs') / 'a' / 'one.xml')
    assert 'not read' in capsys.readouterr().err


def _rate(base, exp):
    return XML.read_bytes().replace(b'<SampleBase>500</SampleBase>', f'<SampleBase>{base}</SampleBase>'.encode()) \
        .replace(b'<SampleExponent>0</SampleExponent>', f'<SampleExponent>{exp}</SampleExponent>'.encode())


def test_the_rate_as_muse_js_reads_it(tmp_path):
    assert b'<SampleBase>500</SampleBase>' in XML.read_bytes() and b'<SampleExponent>0</SampleExponent>' in XML.read_bytes()
    assert read_muse(_rate('250', '1'))['fs'] == 2500
    for base, exp in (('nan', '0'), ('inf', '0'), ('-500', '0'), ('0', '0'), ('', '0'), ('500', '7'), ('500', '-7'),
                      ('500', '3000000'), ('500', '5000'), ('500', '0.5')):
        e = read_muse(_rate(base, exp))                     # not a rate: unknown, shown by sample number
        assert e['fs'] is None and e['rateUnknown'] is True, (base, exp)
        assert any('BAD_SAMPLE_BASE' in n for n in e['meta']['notes'])
        assert e['median']['marks'] == []
        json.dumps(e, allow_nan=False)
    from muse_reader_py.cli import write_wfdb
    with pytest.raises(ValueError, match='sampling rate'):
        write_wfdb(read_muse(_rate('', '')), tmp_path, 'x')
    assert not list(tmp_path.iterdir())                       # nothing written
    m = read_muse(XML.read_bytes().replace(b'<QTInterval>410</QTInterval>', b'<QTInterval>NaN</QTInterval>'))
    assert 'QT' not in {x['name'] for x in m['meta']['measurements']}
    json.dumps(m, allow_nan=False)


def _made(fs=250, device='MAC 8', program='250', qtrggr=124, upb='4.88', rhythm_n=None):
    """A MUSE file of fs Hz: a 10 s rhythm strip and a 1.2 s median beat, the onsets at 500 Hz
    (the same as made() in tests/muse.test.js)."""
    import base64, math, struct
    rhythm_n = 10 * fs if rhythm_n is None else rhythm_n

    def b64(n):
        return base64.b64encode(struct.pack(f'<{n}h', *(round(200 * math.sin(i / 7)) for i in range(n)))).decode()

    def wave(kind, n):
        leads = ''.join(f'<LeadData><LeadSampleCountTotal>{n}</LeadSampleCountTotal><LeadAmplitudeUnitsPerBit>{upb}</LeadAmplitudeUnitsPerBit>'
                        f'<LeadAmplitudeUnits>MICROVOLTS</LeadAmplitudeUnits><LeadID>{l}</LeadID><WaveFormData>{b64(n)}</WaveFormData></LeadData>'
                        for l in ('I', 'II', 'V1', 'V2', 'V3', 'V4', 'V5', 'V6'))
        return f'<Waveform><WaveformType>{kind}</WaveformType><SampleBase>{fs}</SampleBase><SampleExponent>0</SampleExponent>{leads}</Waveform>'
    return (f'<RestingECG><TestDemographics><AcquisitionDevice>{device}</AcquisitionDevice><AnalysisSoftwareVersion>{program}</AnalysisSoftwareVersion></TestDemographics>'
            '<RestingECGMeasurements><QRSDuration>92</QRSDuration><POnset>230</POnset><QOnset>300</QOnset><QOffset>346</QOffset><TOffset>500</TOffset>'
            '<ECGSampleBase>500</ECGSampleBase></RestingECGMeasurements>'
            f'<QRSTimesTypes><GlobalRR>200</GlobalRR><QTRGGR>{qtrggr}</QTRGGR></QRSTimesTypes>'
            f'{wave("Median", round(1.2 * fs))}{wave("Rhythm", rhythm_n)}</RestingECG>').encode()


def test_a_250_hz_format_with_onsets_at_500_hz():
    e = read_muse(_made())
    assert e['fs'] == 250 and e['meta']['notes'] == []
    assert ['Recording', 'MAC 8 · 12SL 250 · 250 Hz · known format'] in e['meta']['fields']
    assert e['median']['marks'] == [{'name': 'P', 'sample': 115}, {'name': 'QRS', 'sample': 150},
                                    {'name': 'QRS end', 'sample': 173}, {'name': 'T end', 'sample': 250}]
    assert {'name': 'RR', 'value': 800, 'unit': 'ms'} in e['meta']['measurements']
    assert any('NEW_FORMAT' in n for n in read_muse(_made(fs=500))['meta']['notes'])
    q = read_muse(_made(qtrggr=248))
    assert q['fs'] == 250 and any('QTRGGR_CONTRADICTS_FS' in n for n in q['meta']['notes'])
    short = read_muse(_made(rhythm_n=1))
    assert any('RHYTHM_NOT_10_S' in n for n in short['meta']['notes'])
    with pytest.raises(ValueError, match='amplitude scale'):
        read_muse(_made(upb=''))


def test_formats_are_the_viewers():
    """The table of formats is the same in muse.js."""
    from muse_reader_py import FORMATS, CHECKS
    js = (Path(__file__).resolve().parents[3] / 'muse.js').read_text(encoding='utf-8')
    for f in FORMATS:
        assert "['%s', '%s', %d, %d, %d, %d, %d]" % f in js, f
    assert len(FORMATS) == 30 and js.count("', 500, 500, 5000, 600, 500]") + js.count("', 250, 250, 2500, 300, 500]") == 30
    for code, text in CHECKS.items():
        assert text.replace("'", "\\'") in js or text in js, code


def test_wfdb_header_one_line_per_value(tmp_path):
    from muse_reader_py.cli import write_wfdb
    e = read_muse(str(XML))
    e['meta']['fields'].append(['Reason', 'Chest pain\nrec2 1 500 10\r\n'])
    e['meta']['statements'][0]['lines'].append('Line one\nline two')
    e['leads'][0]['name'] = 'I\nII'
    hea = write_wfdb(e, tmp_path, 'r').read_text(encoding='utf-8').splitlines()
    assert '# Reason: Chest pain rec2 1 500 10' in hea and hea[1].endswith(' I II')
    assert all(h.startswith('#') for h in hea[13:]) and len(hea) == 13 + len([h for h in hea if h.startswith('#')])
    assert any(h.endswith('; Line one line two') for h in hea)


def test_index_passes_over_hidden_folders_and_reads_each_file_once(tmp_path, monkeypatch):
    import muse_reader_py as m
    from muse_reader_py import cli
    (tmp_path / 'ecgs' / '.git').mkdir(parents=True)
    shutil.copy(XML, tmp_path / 'ecgs' / 'one.xml')
    shutil.copy(XML, tmp_path / 'ecgs' / '.git' / 'copy.xml')
    reads = []
    real = m._parse
    monkeypatch.setattr(cli, '_parse', lambda s: reads.append(s) or real(s))
    monkeypatch.setattr(m, '_parse', lambda s: real(s) if isinstance(s, m.ET.Element) else pytest.fail('read again'))
    header, rows, skipped = cli.index_rows(tmp_path / 'ecgs', tmp_path)
    assert len(rows) == 1 and rows[0][-1] == str(Path('ecgs') / 'one.xml') and not skipped
    assert reads == [str(tmp_path / 'ecgs' / 'one.xml')]


def test_utf16_and_utf8_files_as_muse_js_reads_them():
    text = XML.read_bytes().decode('iso-8859-1')
    golden = read_muse(str(XML))
    as_ = lambda t, d: t.replace('encoding="ISO-8859-1"', f'encoding="{d}"')  # noqa: E731
    files = {
        'UTF-16LE, BOM': b'\xff\xfe' + as_(text, 'UTF-16').encode('utf-16-le'),
        'UTF-16BE, BOM': b'\xfe\xff' + as_(text, 'UTF-16').encode('utf-16-be'),
        'UTF-16LE, no BOM': as_(text, 'UTF-16').encode('utf-16-le'),
        'UTF-16BE, no BOM': as_(text, 'UTF-16').encode('utf-16-be'),
        'UTF-16LE, BOM, a wrong declaration': b'\xff\xfe' + text.encode('utf-16-le'),
        'UTF-8, BOM': b'\xef\xbb\xbf' + as_(text, 'UTF-8').encode('utf-8'),
        'UTF-8, BOM, a wrong declaration': b'\xef\xbb\xbf' + text.encode('utf-8'),
        'UTF-8 declared UTF-16': as_(text, 'UTF-16').encode('utf-8'),
    }
    for what, data in files.items():
        e = read_muse(data)
        assert e == golden, what
        assert dict(e['meta']['fields'])['Site'] == 'Clinique Médicale', what
