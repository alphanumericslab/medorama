# The demo library

`library/` holds short pieces of real signals and pixels, transformed so that they represent no
particular person, from which [`make-demo-data.py`](../make-demo-data.py) builds the demo's ECGs,
ICU and Holter recordings, CT series and chest radiographs. Everything else in the demo is made
from models. The library is small (about 13 MB, 36 files, none over 5 MB) and is shipped with
Medorama: a demo built from it may be shown, taught and published, for noncommercial purposes and
with credit. The library is licensed under CC BY-NC 4.0
([LICENSES/CC-BY-NC-4.0.txt](../../LICENSES/CC-BY-NC-4.0.txt)); its CT series and radiographs are
adapted from the collections below, whose own licences also apply ([REUSE.toml](../../REUSE.toml)).

A maintainer made it once, with a tool kept outside the repository on purpose: that tool reads
real files, and nothing that reads real files belongs in this repository. Only its output, the
transformed library, is committed. Anyone can build the demo from the library; nobody needs the
tool or its sources.

## What it holds

| Folder | Items | What each item is | Made from |
|---|---|---|---|
| `ecg12/` | 16 | A 12-lead heart-beat template (0.8 s, 500 Hz, µV) | Resting 12-lead ECGs, and a synthetic beat |
| `monitor/` | 12 | A 3-lead monitor beat template (leads I, II and a chest lead; 0.8 s, 240 Hz) | ICU bedside-monitor recordings, and a synthetic beat |
| `ct/` | 4 | A CT series: 24 slices of 256 × 256, in Hounsfield units + 1024, 12 bits (two chest, two abdomen) | Four public CT collections (below) |
| `xray/` | 2 | A frontal chest radiograph, 256 × 256, 12 bits | Projections of the two chest CT volumes |
| `manifest.json` | | What each file is: its kind, shape, sampling rate or pixel size, the collection it comes from (`C1` … `C4`); and the **pulse models**: the ranges of the arterial-pressure and pleth cycle shapes (time to peak, decay), fitted to ICU recordings | |
| `checks.json` | | The results of the checks below, as distributions and counts | |

Every array is raw little-endian 16-bit (`.i16` signed, `.u16` unsigned), with values below 2^13
in magnitude: every other byte is then a control byte, so no text can hide in an array, and the
string check can read every byte. No file holds any text of a source: no name, identifier,
date, header, tag or file name.

### The public collections

The CT series and radiographs derive from these collections of The Cancer Imaging Archive, as
redistributed by Saga IT (saga-it.com/dicom/samples) from the NCI Imaging Data Commons, under
the Creative Commons Attribution licences named. Each series the demo writes carries an
`ATTRIBUTION.txt` that names its collection, its licence and the changes made.
[THIRD_PARTY_LICENSES.md](../../THIRD_PARTY_LICENSES.md) lists them too.

| Key | Collection | Licence | Data citation |
|---|---|---|---|
| `C1` | C4KC-KiTS (Kidney Tumor Segmentation Challenge), abdominal CT | CC BY 3.0 | Heller, N., Sathianathen, N., Kalapara, A., et al. (2019). Data from C4KC-KiTS [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/TCIA.2019.IX49E8NX |
| `C2` | LIDC-IDRI (Lung Image Database Consortium), chest CT | CC BY 3.0 | Armato III, S. G., McLennan, G., Bidaut, L., et al. (2015). Data From LIDC-IDRI [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/K9/TCIA.2015.LO9QL9SX |
| `C3` | NLST (National Lung Screening Trial), chest CT | CC BY 4.0 | National Lung Screening Trial Research Team. (2013). Data from the National Lung Screening Trial (NLST) [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/TCIA.HMQ8-J677 |
| `C4` | Pancreas-CT, abdominal CT | CC BY 3.0 | Roth, H., Lu, L., Farag, A., et al. Data From Pancreas-CT (Version 2) [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/K9/TCIA.2016.tNB1kqBU |

## The transformations

No long contiguous stretch of any source is kept: a beat template is under a second long, and a
CT slice is never one of the source's slices.

**Heart beats.** From each source ECG, the beats are found (R waves), the regular ones aligned
and their median taken. Each template then blends the transformed median beats of two different
sources with a synthetic beat (Gaussian P, QRS and T waves), no source weighing more than 0.55;
each source beat is first time-warped about its R wave (the parts before and after it stretched
by their own factors), its leads mixed (a random linear combination near the identity) and given
new gains; the blend is low-pass filtered at a random cut-off, tapered and noised. The 12-lead
templates mix the eight independent leads and derive III, aVR, aVL and aVF from I and II, as a
cart does. Polarity is not inverted: an inverted standard lead is not physiological.

**How the demo uses them.** `make-demo-data.py` places a template's beats on a new RR series it
invents (the patient's heart rate, with beat-to-beat variability, or an irregular rhythm for
atrial fibrillation), stretches each beat's ST-T part with the RR (the QT follows the rate),
removes the P waves for atrial fibrillation, draws ST depression or left ventricular hypertrophy
when the story needs them, inserts premature ventricular beats, and adds baseline wander and
noise. Every 12-lead ECG, ICU ECG and Holter of the demo is built so.

**Pulses.** Arterial pressure and pleth waves of real recordings correlate with one another over
10 s windows as much as with anything periodic at a similar rate, so no shape of a recording is
kept: the demo draws its pulse cycles from a model (an upstroke, a decay, a dicrotic notch for
arterial pressure) whose time to peak and decay rate are drawn from ranges fitted to the
recordings (their 10th and 90th percentiles: numbers only). Respiration, vital signs and their
trends are models alone.

**CT series.** From each collection's largest series, 24 slices are re-sliced at new positions
between the source's slices (interpolated), across the middle 80 % of the volume. The body's
square box is rotated (up to 7°), scaled (0.92 to 1.06), shifted, and elastically deformed (a
smooth random displacement field of about 6 pixels), resampled to 256 × 256; the Hounsfield units
are remapped by a random monotone curve and Gaussian noise is added. Images are not flipped: a
mirrored chest or abdomen is not anatomical.

**Chest radiographs.** Each chest CT volume is projected from front to back (the attenuation
summed along each ray, as a transmission image), inverted so that bone is bright, square-cut,
deformed and remapped as above, and noised.

**Text.** Nothing of a source's text is used: identifiers, names, dates, devices, sites and
statements in the demo are invented by `make-demo-data.py`. MUSE XML files and PDF printouts in
the demo are written anew from the demo's tracings, never copied; pictures (printouts, photos)
were not used at all, since a printout's pixels carry its text.

## The re-identification checks

The tool runs three checks on everything it writes, and fails without writing a library when
one does not pass. It prints, and `checks.json` keeps, only counts and distributions.

1. **Strings.** Every text of every source (headers, DICOM elements, XML text, the recordings'
   index entries and file names) is read in memory and split into tokens. No library file may
   hold any of them that has 4 or more characters (numbers of fewer than 6 digits, which are
   geometry rather than identifiers, and the library's own generic vocabulary, such as `lead` or
   `chest`, excepted): text files are scanned ignoring case, the arrays byte for byte. Result:
   4,752 source strings, no library file holding one.
2. **Waveforms.** Each beat template is rendered as the demo renders it, three times, into 10 s
   (at random rates of 55 to 125 per minute), downsampled to 100 Hz; for each rendering, the
   maximum over all lags of the normalised cross-correlation with every 10 s window (5 s apart)
   of every source ECG lead must stay **below 0.9**. Result over 684 renderings against 204
   source windows: median 0.37, 95th percentile 0.53, maximum 0.67. For the record, the pulse
   models' renderings reach a maximum of 0.86 (arterial pressure) and 0.92 (pleth) against the
   recordings, while windows of different recordings reach 0.70 and 0.94 against one another:
   at that level a 10 s pulse window says nothing about whose it is, which is why no pulse is
   kept.
3. **Images.** No output image may equal a source image, and each output image's normalised
   correlation must stay **below 0.9** with every source slice of its collection cut and
   resampled to the output's geometry (the same box, without the deformation), on the raw
   intensities and on the fine structure (the image minus its blur), and with every source slice
   of every collection at 64 × 64. Results over 98 outputs: same geometry, raw, median 0.72 and
   maximum 0.88; fine structure maximum 0.08; any source at 64 × 64, maximum 0.84; no output
   equal to a source. For reference, slices of **different** collections, that is of different
   people, reach 0.89 against one another at 64 × 64: the outline of a body dominates a raw
   correlation, and the fine structure, where anatomy is personal, does not survive.

## Rebuilding it

Only a maintainer rebuilds the library, with the same tool outside the repository, from the same
kinds of sources; the checks must pass. A rebuilt library changes the demo
(`make-demo-data.py` reads every item by its kind and number); run
`python3 -B -m unittest discover -s tools/tests` with NumPy and wfdb-x, then commit the library
with its `manifest.json` and `checks.json`. The privacy hook ([PRIVACY.md](../../PRIVACY.md))
allows the library's arrays here only, and refuses DICOM and MUSE files here as anywhere.

## Every feature extends the demo

The demo must show every feature of Medorama. A new feature adds what it needs to
`make-demo-data.py` (a story, a table, a recording, a database) and its entry in `FEATURES`
there, and its row in the table of [tools/README.md, The demo's
features](../README.md#the-demos-features); the tests check that the two agree and that the demo
holds each one. A feature that needs real-looking signals or pictures the library lacks asks a
maintainer to add them to the library, transformed and checked as above.
