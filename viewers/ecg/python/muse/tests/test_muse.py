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


def test_the_rate_as_muse_js_reads_it():
    assert b'<SampleBase>500</SampleBase>' in XML.read_bytes() and b'<SampleExponent>0</SampleExponent>' in XML.read_bytes()
    assert read_muse(_rate('250', '1'))['fs'] == 2500
    for base in ('nan', 'inf', '-500', '0', ''):                 # not a positive number: 500 Hz
        assert read_muse(_rate(base, '0'))['fs'] == 500, base
    for exp in ('7', '-7', '3000000', '5000'):                   # refused at once, not a huge number
        with pytest.raises(ValueError, match='SampleExponent'):
            read_muse(_rate('500', exp))
    m = read_muse(XML.read_bytes().replace(b'<QTInterval>410</QTInterval>', b'<QTInterval>NaN</QTInterval>'))
    assert 'QT' not in {x['name'] for x in m['meta']['measurements']}
    json.dumps(m, allow_nan=False)


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
