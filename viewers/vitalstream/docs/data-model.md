# Data model

This page describes what the VitalStream viewers make of a recording, whatever its format, in
language-neutral terms. It is written for developers of readers and adapters, and for anyone who
wants to know exactly what the viewer shows. It is part of the [VitalStream](../README.md)
documentation.

A **reader** builds this model from a recording: the WFDB and EDF readers built into the
engines, or an [adapter](../../../docs/adapters.md) (an add-on program for another format). The
[index](index-format.md) stores the model, and the [viewer API](viewer-api.md) serves it to the
page.

- [Principles](#1-principles)
- [Structure](#2-structure)
- [Values](#3-values)
- [Vital signs, alarms, annotations and markers](#4-vital-signs-alarms-annotations-and-markers)
- [Identifiers and privacy](#5-identifiers-and-privacy)

## 1. Principles

- **Only what the recording states.** The viewers do not resample, interpolate, fill gaps,
  re-time or deduplicate. Every sample drawn is a stored sample.
- **Ambiguity is shown, not resolved.** Where the recording does not settle something, such as
  a break's exact length or a start time, the viewer says so, through the `exact` and `timed`
  flags and the recording's notes.
- **Times are UTC**, as Unix seconds. The display time zone is a setting of the viewer only.

## 2. Structure

| Term | Meaning |
|---|---|
| **channel** | One signal as the user sees it (ECG lead II, ART, Pleth …), with a label, a unit and a modality. |
| **piece** | Samples of one channel that the recording proves contiguous, at one constant sampling rate `fs`: sample *i* is at `t0 + i / fs`. A channel is the pieces it is made of. Pieces are drawn apart and never joined. |
| **lane** | The pieces of a channel never overlap in time. In WFDB records, a piece that would overlap another piece of its channel (a signal recorded twice at the same time) goes into the first lane of that channel it does not overlap. Each further lane is a channel of its own, labeled with its number: `II`, `II (2)`, `II (3)` ([wfdb-collection.md](../../../docs/wfdb-collection.md#overlapping-records)). In EDF files, a second signal with the same label is likewise a channel named `ECG (2)`. |
| **timed** | A piece's flag, false when the recording states no time for it. Such a piece is placed after the data before it and marked **time not stated**. |
| **envelope** | Per piece, the minimum and maximum of every block of 32 samples, and of every 4 blocks above that. Long windows are drawn from it. |
| **break** | A place where contiguity is not proven. It has a reason, its length when known (`gap_s`), whether that length is `exact`, how it was measured, and the channels it concerns. An EDF+D file's interruptions are breaks with the reason "not recorded". |

## 3. Values

- **Units.** Values are in physical units where the recording states a calibration, and in
  `adu` (analog-to-digital units) where it states none; the value drawn is then the stored
  count.
- **Nothing is assumed that a recording does not state.**
  - A WFDB record whose header gives no sampling frequency is not read. Opened alone, it is
    refused. In a folder of plain records it is left out, listed in the index's `skipped` with
    the reason and counted in the notes. In a WFDB collection, the collection is not read.
  - A WFDB signal with a gain of 0 is shown in stored counts (`adu`), and a signal whose header
    gives no units has none. WFDB's old defaults (250 Hz, a gain of 200, millivolts) are not
    used.
  - An EDF signal whose digital minimum equals its digital maximum is shown in its digital
    values.
  - An EDF file's start time is taken as UTC, since EDF does not state a time zone; the
    recording's notes say so.
- **Missing samples** are NaN, drawn as a gap in the line and never as zero. In WFDB records
  they are the format's invalid value.
- **Zero is a value**, never "missing".

## 4. Vital signs, alarms, annotations and markers

| Item | Meaning |
|---|---|
| **vital-sign series** | Values at their times, by name and unit. There is one series per name and source; series are never merged ([wfdb-collection.md](../../../docs/wfdb-collection.md#vitalscsv)). |
| **alarm** | An interval (start, end) with a text, a kind and a level: 1 advisory, 2 warning, 3 crisis. Alarm colors are the only red on the page. |
| **annotation** | An event of kind `annotation`: an EDF+ or BDF+ annotation, with its text, its onset and, when the file states one, its duration (else it is an instant). Annotations have no level, so they are never drawn in red. When every event of a recording is an annotation, the viewer titles the events' lane **Annotations** instead of **Alarms**. |
| **marker** | A mark at a sample of a piece (an R wave, a pacing spike), by name. |

## 5. Identifiers and privacy

The viewers never read, keep or show a patient's identity. The index holds no name, identifier
or date of birth, and neither do the viewers' answers. An EDF header's patient and recording
identification fields are never read into the index.

A recording's path and file name may still identify a patient: bedside-monitor systems name
files by unit, bed and time. Every value of a recording is patient data. Keep recordings, and
the index folders made from them, on storage protected like the recordings themselves. In a
clone of the repository, `sample_data/` is the place for them; git never pushes it.
