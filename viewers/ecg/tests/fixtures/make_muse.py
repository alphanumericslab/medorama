"""Writes synthetic_muse.xml: a made-up MUSE resting ECG (invented patient, signals and
statements) for the tests of the MUSE readers (Python, JavaScript).  ISO-8859-1, as MUSE
writes them, with a non-ASCII site name; the computer's statements in fragments; a confirmed
diagnosis the physician edited.

    python3 make_muse.py            # writes synthetic_muse.xml and synthetic_muse.json (golden)
"""

import base64
import json
import math
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
FS = 500
N = 1000            # 2 s
UPB = 4.88          # µV per bit


def beat(t):
    """A crude PQRST, mV, 1 s period."""
    t = t % 1.0
    g = lambda c, w, a: a * math.exp(-((t - c) / w) ** 2)
    return g(0.2, 0.025, 0.15) - g(0.33, 0.008, 0.1) + g(0.35, 0.01, 1.0) - g(0.37, 0.008, 0.25) + g(0.6, 0.05, 0.3)


def lead(gain, n):
    vals = [round(gain * beat(i / FS) * 1000 / UPB) for i in range(n)]
    return base64.b64encode(struct.pack(f'<{n}h', *vals)).decode()


def wrap(b64):
    return '\n'.join(b64[i:i + 76] for i in range(0, len(b64), 76))


def lead_xml(name, gain, n):
    return f"""    <LeadData>
      <LeadByteCountTotal>{2 * n}</LeadByteCountTotal>
      <LeadSampleCountTotal>{n}</LeadSampleCountTotal>
      <LeadAmplitudeUnitsPerBit>{UPB}</LeadAmplitudeUnitsPerBit>
      <LeadAmplitudeUnits>MICROVOLTS</LeadAmplitudeUnits>
      <LeadID>{name}</LeadID>
      <WaveFormData>
{wrap(lead(gain, n))}
      </WaveFormData>
    </LeadData>
"""


def waveform(kind, n):
    gains = {'I': 0.6, 'II': 1.0, 'V1': -0.5, 'V2': 0.3, 'V3': 0.8, 'V4': 1.2, 'V5': 1.0, 'V6': 0.7}
    leads = ''.join(lead_xml(k, g, n) for k, g in gains.items())
    return f"""  <Waveform>
    <WaveformType>{kind}</WaveformType>
    <WaveformStartTime>0</WaveformStartTime>
    <NumberofLeads>8</NumberofLeads>
    <SampleType>CONTINUOUS_SAMPLES</SampleType>
    <SampleBase>{FS}</SampleBase>
    <SampleExponent>0</SampleExponent>
    <HighPassFilter>16</HighPassFilter>
    <LowPassFilter>150</LowPassFilter>
    <ACFilter>60</ACFilter>
{leads}  </Waveform>
"""


def stmt(text, *flags):
    f = ''.join(f'<StmtFlag>{x}</StmtFlag>' for x in flags)
    return f'    <DiagnosisStatement>{f}<StmtText>{text}</StmtText></DiagnosisStatement>\n'


XML = f"""<?xml version="1.0" encoding="ISO-8859-1"?>
<!DOCTYPE RestingECG SYSTEM "restecg.dtd">
<RestingECG>
  <MuseInfo><MuseVersion>9.0.9.18167</MuseVersion></MuseInfo>
  <PatientDemographics>
    <PatientID>SYN000123</PatientID>
    <PatientAge>47</PatientAge>
    <AgeUnits>YEARS</AgeUnits>
    <DateofBirth>01-02-1970</DateofBirth>
    <Gender>FEMALE</Gender>
    <PatientLastName>TESTPATIENT</PatientLastName>
    <PatientFirstName>SYNTHETIC</PatientFirstName>
  </PatientDemographics>
  <TestDemographics>
    <DataType>RESTING</DataType>
    <Site>1</Site>
    <SiteName>Clinique Médicale</SiteName>
    <AcquisitionDevice>MAC5500</AcquisitionDevice>
    <Status>CONFIRMED</Status>
    <EditListStatus>Edited</EditListStatus>
    <Priority>NORMAL</Priority>
    <Location>12</Location>
    <LocationName>TEST WARD</LocationName>
    <AcquisitionTime>08:15:30</AcquisitionTime>
    <AcquisitionDate>03-14-2020</AcquisitionDate>
    <AcquisitionSoftwareVersion>010B</AcquisitionSoftwareVersion>
    <AnalysisSoftwareVersion>241</AnalysisSoftwareVersion>
    <EditTime>10:02:00</EditTime>
    <EditDate>03-15-2020</EditDate>
    <OverreaderID>99</OverreaderID>
    <OverreaderLastName>READER</OverreaderLastName>
    <OverreaderFirstName>TEST</OverreaderFirstName>
  </TestDemographics>
  <Order><AdmitDiagnosis>made up</AdmitDiagnosis><TestReason>Chest pain</TestReason></Order>
  <RestingECGMeasurements>
    <VentricularRate>60</VentricularRate>
    <AtrialRate>60</AtrialRate>
    <PRInterval>150</PRInterval>
    <QRSDuration>92</QRSDuration>
    <QTInterval>410</QTInterval>
    <QTCorrected>410</QTCorrected>
    <PAxis>45</PAxis>
    <RAxis>-10</RAxis>
    <TAxis>40</TAxis>
    <QRSCount>2</QRSCount>
    <QTcFrederica>410</QTcFrederica>
  </RestingECGMeasurements>
  <OriginalDiagnosis>
{stmt('Sinus rhythm')}{stmt('with ', )}{stmt('borderline left axis', 'ENDSLINE')}{stmt('Borderline ECG', 'ENDSLINE')}  </OriginalDiagnosis>
  <Diagnosis>
{stmt('Normal sinus rhythm', 'ENDSLINE')}{stmt('Normal ECG', 'ENDSLINE')}{stmt('When compared with ECG of 01-JAN-2019', 'ENDSLINE')}  </Diagnosis>
{waveform('Median', 600)}{waveform('Rhythm', N)}</RestingECG>
"""


if __name__ == '__main__':
    (HERE / 'synthetic_muse.xml').write_bytes(XML.encode('iso-8859-1'))
    sys.path.insert(0, str(HERE.parents[1] / 'python' / 'muse'))
    from muse_reader_py import read_muse
    (HERE / 'synthetic_muse.json').write_text(json.dumps(read_muse(str(HERE / 'synthetic_muse.xml')), ensure_ascii=False) + '\n', encoding='utf-8')
    print('wrote synthetic_muse.xml, synthetic_muse.json')
