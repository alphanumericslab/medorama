"""EDF, EDF+ and BDF files (docs/edf.md) on the synthetic files of testdata/edf, as the Rust
engine's tests/edf.rs: channels and rates, the start of the first record (EDF+ time keeping),
records in runs with breaks (EDF+D), annotations as events, a two-digit year, a record cut short,
24-bit samples, and the patient field never read; and the header dates and the annotation lists
themselves."""

import math
from pathlib import Path

import pytest

from vitalstream_py.dataset import Dataset
from vitalstream_py.index import build_index, load_index
from vitalstream_py.server import _entry
from vitalstream_py.sources import edf, format_label, kind_of

EDF = Path(__file__).resolve().parents[3] / 'testdata' / 'edf'
START = 1709298000.0  # 2024-03-01 13:00:00 UTC


def indexed(name, cache):
    key = build_index(EDF / name, cache)
    meta, arrays, folder = load_index(cache, key)
    return key, meta, Dataset(meta, arrays, folder)


def test_an_edf_plus_file(tmp_path):
    p = EDF / 'psg_plus.edf'
    assert kind_of(p) == 'edf'
    assert edf.format_label(p) == format_label(p) == 'EDF+'
    assert _entry(str(EDF), 'psg_plus.edf')['label'] == 'EDF+'
    key, m, ds = indexed('psg_plus.edf', tmp_path)
    assert (m['kind'], m['format']) == ('edf', 'EDF+')
    assert [(c['label'], c['unit']) for c in m['channels']] == [('ECG', 'mV'), ('Resp', ''), ('SpO2', '%')]
    # the first record starts 0.25 s after the header's time (its time-keeping annotation)
    assert [(p['fs'], p['n'], p['t0'] - START) for p in m['pieces']] == [(250.0, 5000, 0.25), (25.0, 500, 0.25),
                                                                         (1.0, 20, 0.25)]
    assert m['gaps'] == []
    assert [(a['t0'] - START, a['t1'] - START, a['text'], a['kind']) for a in m['alarms']] == [
        (0.25, 10.25, 'Sleep stage W', 'annotation'), (0.5, 0.5, 'Lights off', 'annotation'),
        (4.25, 7.25, 'Arousal', 'annotation')]
    assert all(a['level'] is None for a in m['alarms'])
    # the patient field (an invented identity here) is in no file of the index
    for f in (tmp_path / key).iterdir():
        assert b'Synthetic' not in f.read_bytes() and b'MO-TEST' not in f.read_bytes(), f
    # samples: the physical values the generator wrote, to within one digital step
    x = ds.samples(2, 0, 20)
    assert len(x) == 20
    for k, v in enumerate(x):
        assert abs(v - (96 + math.sin((0.25 + k) / 3))) <= 0.1 / 2 + 1e-9, (k, v)
    assert len(ds.samples(1, 498, 600)) == 2
    ds.close()


def test_records_in_runs(tmp_path):
    _, m, ds = indexed('gappy_plus.edf', tmp_path)
    assert [(p['t0'] - START, p['n'], p['data_record']) for p in m['pieces']] == [(0.0, 375, 0), (3.0, 250, 3),
                                                                                  (10.0, 125, 5)]
    assert [(g['t0'] - START, g['t1'] - START, g['reason']) for g in m['gaps']] == [(1.5, 3.0, 'not recorded'),
                                                                                    (4.0, 10.0, 'not recorded')]
    assert [a['text'] for a in m['alarms']] == ['Electrode off']
    # samples of a later run come from its own records
    assert len(ds.samples(1, 0, 250)) == 250
    ds.close()


def test_plain_edf_and_bdf(tmp_path):
    _, m, _ = indexed('plain.edf', tmp_path)
    assert m['format'] == 'EDF'
    # 1999 (a two-digit year); the number of records is unknown: the 8 whole records of the file
    assert m['pieces'][0]['t0'] == 920368800.0
    assert [p['n'] for p in m['pieces']] == [1600, 8]
    assert any('ends inside a data record' in n for n in m['notes']), m['notes']
    assert m['alarms'] == []

    _, m, ds = indexed('eeg_plus.bdf', tmp_path)
    assert m['format'] == 'BDF+'
    assert [c['label'] for c in m['channels']] == ['Fp1', 'Flat']
    # a digital range of zero: the digital values themselves
    assert list(ds.samples(1, 0, 12)) == [7.0] * 12
    x = ds.samples(0, 0, 768)
    assert len(x) == 768
    step = (262143 + 262144) / (8388607 + 8388608)
    for k, v in enumerate(x):
        assert abs(v - 80 * math.sin(2 * math.pi * 9 * k / 256)) <= step, (k, v)
    assert [(a['t0'] - START, a['text']) for a in m['alarms']] == [(1.5, 'Eyes closed')]
    ds.close()


def test_not_edf(tmp_path):
    p = tmp_path / 'x.edf'
    p.write_bytes(b'not an EDF file at all')
    with pytest.raises(ValueError, match='not an EDF or BDF file'):
        build_index(p, tmp_path / 'cache')


def test_dates_and_annotations():
    # two-digit years: 85-99 are 19xx, the rest 20xx; EDF+ states the year in full
    plus = 'Startdate 01-MAR-2091 X X X'
    assert edf.start_of('02.03.99', '10.00.00') == 920368800.0
    assert edf.start_of('01.03.24', '13.00.00') == 1709298000.0
    assert edf.start_of('01.03.yy', '13.00.00', plus) == 3823592400.0
    assert edf.start_of('01.03.yy', '13.00.00') is None
    assert edf.start_of('01.03.91', '13.00.00', plus) == 3823592400.0
    assert edf.start_of('31.02.24', '25.00.00') is None
    t = edf.tals(b'+0.25\x14\x14\x00+1.5\x152\x14Arousal\x14 two  words \x14\x00\x00\x00')
    assert t == [(0.25, None, ['', '']), (1.5, 2.0, ['Arousal', 'two words', ''])]
    # invalid UTF-8: one replacement character per maximal subpart; an onset that is not a
    # number skips its list
    assert edf.tals(b'+1\x14\xe2\x82A\xff\x1fb\x14\x00x\x14lost\x14\x00') == [(1.0, None, ['�A� b', ''])]
    # numbers as the other viewers read them
    assert [edf.parse_float(s) for s in (' 1_000.5 ', '+.5', '1.', '1e3', '٣', '1__0', '_1', '', '1x')] == [
        1000.5, 0.5, 1.0, 1000.0, None, None, None, None, None]
