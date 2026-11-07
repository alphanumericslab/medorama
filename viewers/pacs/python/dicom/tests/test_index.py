"""dicom-py on synthetic DICOM files (made here, with made-up patients)."""

import csv

import numpy as np
import pytest
from pydicom.dataset import Dataset, FileMetaDataset
from pydicom.sequence import Sequence
from pydicom.uid import ExplicitVRLittleEndian, generate_uid

from dicom_index_py import ecg_name, index_folder, write_wfdb
from dicom_index_py.cli import main

US_MF = '1.2.840.10008.5.1.4.1.1.3.1'
CT = '1.2.840.10008.5.1.4.1.1.2'
XA = '1.2.840.10008.5.1.4.1.1.12.1'
SR = '1.2.840.10008.5.1.4.1.1.88.33'
PDF = '1.2.840.10008.5.1.4.1.1.104.1'
ECG = '1.2.840.10008.5.1.4.1.1.9.1.1'


def save(path, sop, pid, **kw):
    ds = Dataset()
    ds.file_meta = FileMetaDataset()
    ds.file_meta.MediaStorageSOPClassUID = sop
    ds.file_meta.MediaStorageSOPInstanceUID = generate_uid()
    ds.file_meta.TransferSyntaxUID = ExplicitVRLittleEndian
    ds.SOPClassUID = sop
    ds.SOPInstanceUID = ds.file_meta.MediaStorageSOPInstanceUID
    ds.PatientID = pid
    ds.PatientName = 'Test^Synthetic'
    ds.PatientBirthDate = '19500304'
    ds.StudyInstanceUID = kw.pop('study', generate_uid())
    ds.SeriesInstanceUID = kw.pop('series', generate_uid())
    ds.StudyDate = '20240102'
    ds.StudyTime = '093000'
    for k, v in kw.items():
        setattr(ds, k, v)
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        ds.save_as(path, enforce_file_format=True)          # pydicom 3
    except TypeError:
        ds.is_little_endian, ds.is_implicit_VR = True, False  # pydicom 2
        ds.save_as(path, write_like_original=False)
    return ds


def code(value, scheme, meaning):
    c = Dataset()
    c.CodeValue, c.CodingSchemeDesignator, c.CodeMeaning = value, scheme, meaning
    return Sequence([c])


def num(meaning, value, unit, mean=False):
    it = Dataset()
    it.RelationshipType, it.ValueType = 'CONTAINS', 'NUM'
    it.ConceptNameCodeSequence = code('x', 'LN', meaning)
    mv = Dataset()
    mv.NumericValue = str(value)
    mv.MeasurementUnitsCodeSequence = code(unit, 'UCUM', unit)
    it.MeasuredValueSequence = Sequence([mv])
    if mean:
        d = Dataset()
        d.RelationshipType, d.ValueType = 'HAS CONCEPT MOD', 'CODE'
        d.ConceptNameCodeSequence = code('121401', 'DCM', 'Derivation')
        d.ConceptCodeSequence = code('R-00317', 'SRT', 'Mean')
        it.ContentSequence = Sequence([d])
    return it


@pytest.fixture
def study(tmp_path):
    root = tmp_path / 'dicom'
    px = np.zeros((4, 8, 8), np.uint8).tobytes()
    save(root / 'P1' / 'echo' / 'A4C.dcm', US_MF, 'SYN1', Modality='US', SeriesDescription='Apical 4-chamber',
         NumberOfFrames=4, FrameTime='33.3333', Rows=8, Columns=8, SamplesPerPixel=1, PhotometricInterpretation='MONOCHROME2',
         BitsAllocated=8, BitsStored=8, HighBit=7, PixelRepresentation=0, PixelData=px)
    save(root / 'P1' / 'cath' / 'run1.dcm', XA, 'SYN1', Modality='XA', NumberOfFrames=2, CineRate=15, Rows=8, Columns=8)
    ct = generate_uid()
    for z in range(3):
        save(root / 'P1' / 'ct' / f'IM{z + 1:04d}', CT, 'SYN1', Modality='CT', SeriesDescription='Cardiac CT', series=ct, InstanceNumber=z + 1)
    # a series in a folder that also holds another patient's files: not linked
    mixed = generate_uid()
    for z in range(2):
        save(root / 'flat' / f'a{z}.dcm', CT, 'SYN1', Modality='CT', series=mixed)
    save(root / 'flat' / 'b.dcm', CT, 'SYN2', Modality='CT')
    # an echo report: LVEF twice (the mean counts), a dimension, a finding
    lv = Dataset()
    lv.RelationshipType, lv.ValueType = 'CONTAINS', 'CONTAINER'
    lv.ConceptNameCodeSequence = code('29468-8', 'LN', 'Left Ventricle')
    lv.ContentSequence = Sequence([num('Left Ventricular Ejection Fraction', 40, '%'), num('Left Ventricular Ejection Fraction', 38, '%', mean=True),
                                   num('Left Ventricle Internal End Diastolic Dimension', 6.1, 'cm')])
    finding = Dataset()
    finding.RelationshipType, finding.ValueType = 'CONTAINS', 'TEXT'
    finding.ConceptNameCodeSequence = code('121071', 'DCM', 'Finding')
    finding.TextValue = 'Dilated LV, reduced function'
    save(root / 'P1' / 'echo' / 'SR.dcm', SR, 'SYN1', Modality='SR', ValueType='CONTAINER',
         ConceptNameCodeSequence=code('125200', 'DCM', 'Adult Echocardiography Procedure Report'), ContentSequence=Sequence([lv, finding]))
    save(root / 'P1' / 'echo' / 'report.dcm', PDF, 'SYN1', Modality='DOC', DocumentTitle='Echo report',
         MIMETypeOfEncapsulatedDocument='application/pdf', EncapsulatedDocument=b'%PDF-1.4\n%%EOF\n\0')
    # a 12-lead ECG (two leads), with a QT and a statement
    g = Dataset()
    g.MultiplexGroupLabel = 'RHYTHM'
    g.NumberOfWaveformChannels = 2
    g.NumberOfWaveformSamples = 4
    g.SamplingFrequency = '500'
    g.WaveformBitsAllocated = 16
    g.WaveformSampleInterpretation = 'SS'
    chans = []
    for lead in ('I', 'II'):
        c = Dataset()
        c.ChannelSourceSequence = code(f'5.6.3-9-{lead}', 'SCPECG', f'Lead {lead}')
        c.ChannelSensitivity = '2.5'
        c.ChannelSensitivityUnitsSequence = code('uV', 'UCUM', 'microvolt')
        c.ChannelSensitivityCorrectionFactor = '1'
        c.ChannelBaseline = '0'
        c.WaveformBitsStored = 16
        chans.append(c)
    g.ChannelDefinitionSequence = Sequence(chans)
    g.WaveformData = np.array([[0, 0], [100, -40], [200, -80], [300, -120]], '<i2').tobytes()
    qt = Dataset()
    qt.ConceptNameCodeSequence = code('8634-8', 'LN', 'QT interval')
    qt.NumericValue = '412'
    qt.MeasurementUnitsCodeSequence = code('ms', 'UCUM', 'ms')
    stmt = Dataset()
    stmt.UnformattedTextValue = 'Sinus rhythm'
    save(root / 'P1' / 'ecg' / 'ecg.dcm', ECG, 'SYN1', Modality='ECG', AcquisitionDateTime='20240102081500',
         WaveformSequence=Sequence([g]), WaveformAnnotationSequence=Sequence([qt, stmt]))
    return root


def test_index_rows(study):
    header, rows, skipped = index_folder(study, base=study.parent)
    by = {r[header.index('name')]: dict(zip(header, r)) for r in rows}
    loop = by['Apical 4-chamber']
    assert (loop['category'], loop['modality'], loop['frames'], loop['fps'], loop['mrn']) == ('echo', 'US', 4, 30.0, 'SYN1')
    assert loop['file'] == 'dicom/P1/echo/A4C.dcm' and loop['time'] == '2024-01-02 09:30:00'
    run = by['Angiography run']
    assert (run['category'], run['fps']) == ('cath', 15)
    ct = by['Cardiac CT']
    assert (ct['category'], ct['images'], ct['file']) == ('img', 3, 'dicom/P1/ct')
    sr = by['Adult Echocardiography Procedure Report']
    assert sr['category'] == 'echo'
    assert sr['Left Ventricular Ejection Fraction (%)'] == 38, 'the mean'
    assert sr['Left Ventricle Internal End Diastolic Dimension (cm)'] == 6.1
    assert sr['interpretation'] == 'Dilated LV, reduced function'
    assert by['Echo report']['category'] == 'echo' and by['Echo report']['file'].endswith('report.dcm')
    ecg = by['12-lead ECG']
    assert (ecg['category'], ecg['QT (ms)'], ecg['interpretation'], ecg['time']) == ('ecg', 412, 'Sinus rhythm', '2024-01-02 08:15:00')
    assert [s[1] for s in skipped] == ["series 'CT image': the folder holds other patients' files too"]
    # (the full birth date: random UIDs may hold any digits)
    assert not any('Synthetic' in str(c) or '19500304' in str(c) for r in rows for c in r), 'no names, no birth dates'


def test_cli_and_wfdb(study, tmp_path, capsys):
    out = tmp_path / 'db' / 'imaging.csv'
    main(['index', str(study), '-o', str(out)])
    rows = list(csv.DictReader(open(out, encoding='utf-8')))
    assert len(rows) == 7 and all(r['file'].startswith('../dicom/') for r in rows)
    assert 'not linked' in capsys.readouterr().err
    hea = write_wfdb(study / 'P1' / 'ecg' / 'ecg.dcm', tmp_path / 'rec', 'e1').read_text().splitlines()
    assert hea[0] == 'e1 2 500 4'
    assert hea[1].startswith('e1.dat 16 1000/mV 16 0 0 ') and hea[1].endswith(' I')
    assert '# QT: 412 ms' in hea and '# Dx: Sinus rhythm' in hea and '# Acquired: 2024-01-02 08:15:00' in hea
    data = np.frombuffer((tmp_path / 'rec' / 'e1.dat').read_bytes(), '<i2').reshape(-1, 2)
    assert data[:, 0].tolist() == [0, 250, 500, 750] and data[:, 1].tolist() == [0, -100, -200, -300]


def test_ecg_names():
    assert [ecg_name(n) for n in ('QT interval', 'QTc Interval', 'PR-interval', 'QRS duration', 'Heart rate', 'T axis', 'LVEF')] == \
        ['QT', 'QTc', 'PR', 'QRS', 'Vent. rate', 'T axis', 'LVEF']


def test_damaged_files_are_listed_not_fatal(study, tmp_path):
    """A damaged header (a frame count that is not a number) or a truncated file: listed as not
    read; every other object is still indexed."""
    # a valid file whose frame count is then damaged in place ('77' becomes 'xx')
    odd = study / 'P1' / 'bad' / 'odd.dcm'
    save(odd, US_MF, 'SYN1', Modality='US', NumberOfFrames='77')
    raw = odd.read_bytes()
    tag = bytes.fromhex('28000800') + b'IS'
    at = raw.index(tag) + 8
    odd.write_bytes(raw[:at] + b'xx' + raw[at + 2:])
    good = (study / 'P1' / 'echo' / 'A4C.dcm').read_bytes()
    (study / 'P1' / 'bad' / 'cut.dcm').write_bytes(good[:300])
    header, rows, skipped = index_folder(study, base=study.parent)
    assert len(rows) == 7
    assert any('odd.dcm' in p for p, _ in skipped)


def test_a_file_nested_too_deep_is_listed_not_fatal(study):
    """A file of sequences nested thousands deep (the reader's recursion runs out): listed as
    not read; every other object is still indexed."""
    import struct
    raw = (study / 'P1' / 'ecg' / 'ecg.dcm').read_bytes()
    nest = b''.join(struct.pack('<HH2sHI', 0x0040, 0xA730, b'SQ', 0, 0xffffffff) + struct.pack('<HHI', 0xfffe, 0xe000, 0xffffffff)
                    for _ in range(5000))
    (study / 'P1' / 'ecg' / 'deep.dcm').write_bytes(raw + nest)
    header, rows, skipped = index_folder(study, base=study.parent)
    assert len(rows) == 7
    assert any(p.endswith('deep.dcm') and 'not read' in why for p, why in skipped)


def test_wfdb_header_one_line_per_value(study, tmp_path):
    from pydicom import dcmread
    f = study / 'P1' / 'ecg' / 'ecg.dcm'
    ds = dcmread(f)
    ds.WaveformAnnotationSequence[1].UnformattedTextValue = 'Sinus rhythm\r\nrec2 1 500 10\r\n  Left axis deviation'
    ds.save_as(tmp_path / 'e.dcm')
    hea = write_wfdb(tmp_path / 'e.dcm', tmp_path / 'rec', 'e').read_text().splitlines()
    assert '# Dx: Sinus rhythm; rec2 1 500 10; Left axis deviation' in hea
    assert all(h.startswith('#') for h in hea[3:])
