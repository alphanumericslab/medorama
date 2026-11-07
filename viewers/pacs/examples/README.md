# PACS viewer examples

This folder holds synthetic DICOM objects to try the [PACS viewer](../README.md) with. Every
value and every image is invented, and no file describes a real person. The files are written by
[`tools/make-viewer-examples.py`](../../../tools/make-viewer-examples.py) with the demo's
generators.

On the website, each link below opens the file in the live viewer. From a clone of the
repository, open [`../index.html`](../index.html) in a browser, then either choose one file with
**Open record…**, or choose this whole folder with **Open multiple records…** to see the three
objects in the series list.

| File | Contents | Open |
|---|---|---|
| `echo_a4c.dcm` | An echocardiography loop (ultrasound, apical four-chamber view, 30 frames): a dilated, poorly contracting left ventricle. | [open](../#src=examples/echo_a4c.dcm) |
| `angio_lca.dcm` | A coronary angiography run (XA) of the left coronary artery. | [open](../#src=examples/angio_lca.dcm) |
| `chest_xray.dcm` | A frontal chest radiograph (DX) with a right lower-lobe opacity. | [open](../#src=examples/chest_xray.dcm) |

To write the files again, run the generator from the repository root. It needs NumPy and
wfdb-x, and also writes the [ECG viewer's examples](../../ecg/examples/README.md):

```bash
python3 tools/make-viewer-examples.py
```
