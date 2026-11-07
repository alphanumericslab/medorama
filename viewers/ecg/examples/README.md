# ECG viewer examples

This folder holds synthetic ECGs to try the [ECG viewer](../README.md) with, one in each format
it reads. Every value is invented, and no file describes a real person. The files are written by
[`tools/make-viewer-examples.py`](../../../tools/make-viewer-examples.py) with the demo's
generators.

On the website, each link below opens the file in the live viewer. From a clone of the
repository, open [`../index.html`](../index.html) in a browser, then either choose one file with
**Open record…** (for the WFDB record, choose its `.hea` and `.dat` files together), or choose
this whole folder with **Open multiple records…** and step through the ECGs with ‹ and ›.

| File | Contents | Open |
|---|---|---|
| `sinus_st_depression.hea`, `sinus_st_depression.dat` | A WFDB record of twelve leads (500 Hz, 10 s): sinus rhythm with inferolateral ST depression. The cart's report (measurements and the computer's statements) is in the header's comments. | [open](../#src=examples/sinus_st_depression.hea) |
| `atrial_fibrillation_muse.xml` | A GE MUSE resting ECG XML file: atrial fibrillation with a rapid ventricular response, with the measurements and the 12SL statements. | [open](../#src=examples/atrial_fibrillation_muse.xml) |
| `normal_dicom.dcm` | A DICOM 12-lead ECG waveform object: normal sinus rhythm, with its annotations as the report. | [open](../#src=examples/normal_dicom.dcm) |
| `normal_edf.edf` | An EDF+ file of twelve leads (500 Hz, 10 s, in µV): normal sinus rhythm, with two annotations (a lead check and an event button press). Its patient field holds the anonymous value `X X X X`. | [open](../#src=examples/normal_edf.edf) |
| `normal_printout.pdf` | A PDF printout of a 12-lead ECG, shown as it is. | [open](../#src=examples/normal_printout.pdf) |
| `normal_printout.png` | The same printout as a PNG picture. | [open](../#src=examples/normal_printout.png) |

To write the files again, run the generator from the repository root. It needs NumPy and
wfdb-x, and also writes the [PACS viewer's examples](../../pacs/examples/README.md):

```bash
python3 tools/make-viewer-examples.py
```
