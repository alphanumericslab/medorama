# WFDB records and collections

This page specifies how Medorama and the three VitalStream viewers read
[WFDB](https://physionet.org/content/wfdb/) records. It is written for data managers who prepare
recordings, and for developers who write converters or check a reader.

Three forms are read:

- **One record:** its `.hea` file, single- or multi-segment.
- **A WFDB collection:** a folder of records with a few CSV files beside them that say how the
  records fit together in time and what else the recording held (vital signs, alarms, breaks).
  Converters write collections (the stp-adapter add-on's `wfdb` export is one; the stp-adapter is
  private and available to institutions under agreement), and anyone can write one with a few
  lines of code.
- **A folder of plain records**, subfolders included, read as one session.

WFDB records themselves are read with **wfdb-x**, a public WFDB library with Rust, C++ and Python
implementations (<https://github.com/alphanumericslab/wfdb-x>), so any signal format that library
reads is accepted. The rules on this page (collections, sessions, placement on the time axis) are
the engines' own ([readers.md](readers.md)).

## Contents

- [Layout](#layout)
- [index.csv](#indexcsv)
- [gaps.csv](#gapscsv)
- [vitals.csv](#vitalscsv)
- [events.csv](#eventscsv)
- [Markers](#markers)
- [How a collection is read](#how-a-collection-is-read)
- [One record](#one-record)
- [Multi-segment records](#multi-segment-records)
- [A folder of records: one session](#a-folder-of-records-one-session)
- [Overlapping records](#overlapping-records)
- [Limits](#limits)
- [An example](#an-example)

## Layout

```
recording/                      the collection (one recording)
├── index.csv                   the records, in order (required: it makes the folder a collection)
├── gaps.csv                    breaks between records (optional)
├── vitals.csv                  vital-sign values (optional)
├── events.csv                  alarms and other events with a duration (optional)
├── README.txt                  anything for people (ignored by the readers)
├── ecg/                        a folder per modality, each with its RECORDS file
│   ├── RECORDS
│   ├── ecg_250_0001.hea  ecg_250_0001.dat  ecg_250_0001.mrk
│   └── …
└── pressure/
    └── …
```

- Records are standard WFDB records (any signal format wfdb-x reads). Each signal of each
  record becomes a piece of a channel.
- Every record's header must state its sampling frequency. The readers assume none, so a
  collection with a record that does not is not read. A signal with a gain of 0 (uncalibrated)
  is shown in stored counts, with the unit `adu`; a signal whose header gives no units has none.
- Records may sit in subfolders; `index.csv` names them by their path relative to the
  collection, with `/` between folders (`ecg/ecg_250_0001`).
- CSV files are UTF-8, comma-separated, with a header row, as Python's `csv` module writes them
  (with quotes around fields that need them). Columns not described here are ignored, so a
  writer may add its own.
- Times are Unix seconds (UTC), written as decimal numbers.

## index.csv

`index.csv` has one row per record, in the order the records were recorded.

| Column | Required | Meaning |
|---|---|---|
| `record` | yes | the record's path relative to the collection, without extension. It stays inside the collection: a name that is absolute (starts with `/`, `\` or `~`, or has a drive) or has a `..` component is not read ([How a collection is read](#how-a-collection-is-read)) |
| `modality` | no | the kind of signals (`ecg`, `pressure`, `resp`, `spo2` …); channels are grouped by it. Default `wfdb` |
| `start_unix` | no | the time of the record's first sample. Empty: the record states no time ([placement](#how-a-collection-is-read)) |
| `stretch` | no | a label shared by the records of one stretch of contiguous data; records without a start time that share it are placed together |

Other columns (`fs`, `n_sig`, `signals`, `units`, `sig_len`, `duration_s`, `start_utc`,
`time_source`, `time_uncertainty_s`, provenance, checksums) are for people and other programs;
the readers take the rate, signals and length from the headers.

## gaps.csv

`gaps.csv` has one row per break in the data, or per change that started a new record.

| Column | Meaning |
|---|---|
| `scope` | `all` (every channel) or the modality or channels it concerns |
| `reason` | why: `signal_loss`, `corrupt_bytes`, … The reasons `channel_absent`, `channel_set_change`, `label_change` and `packet_count_change` mark where a record was split while the data stayed contiguous: they are not shown as breaks |
| `end_before_unix`, `start_after_unix` | the end of the data before and the start of the data after, when known |
| `gap_s` | the length of the break, when known (even if neither end has a stated time) |
| `gap_uncertainty_s`, `measured_by`, `exact`, `notes` | how it was measured: `exact` is `True` when `gap_s` is exact |
| `records_before`, `records_after` | the records on each side, `;`-separated, by name or path |

## vitals.csv

`vitals.csv` has one row per value.

| Column | Meaning |
|---|---|
| `time` | when the value was measured |
| `label` (else `name`) | what was measured: `HR`, `SpO2`, `NBP-S` … |
| `value` | the value (rows whose value is not a number are left out) |
| `unit` | its unit |
| `validity` | empty or `0`: valid; anything else: the row is left out |
| `source` | where the value came from, when a recording has several (the monitor's numerics, an import) |

**Naming.** A label measured by one source is named by its label. When a label has values from
several sources, the first source in the file keeps the plain label and the others are named
with their source: `HR`, `HR [import]`. Series of different sources are never merged.

## events.csv

`events.csv` lists alarms and other events that have a start and, possibly, an end.

| Column | Meaning |
|---|---|
| `start_unix` | the start (required) |
| `end_unix` | the end; empty: an instant |
| `level` | the severity: 1 advisory, 2 warning, 3 crisis (shown by color; the alarm colors are the only red on the viewers' pages) |
| `text` | what is shown |
| `kind` | the kind of event (`alarm`, the system that raised it …); default `event` |

Events are kept when their start lies within a day of the recording's data (a guard against
zero dates), and are sorted by start.

## Markers

A record may have a `.mrk` annotation file (WFDB's MIT format): marks at samples, such as
R waves or pacing spikes. The annotation's `chan` is the signal it belongs to. A marker's name
is its auxiliary text when it has one (`R_WAVE`), else the WFDB mnemonic of its code (`N`,
`V`, `+` …). Markers are drawn on their signal and can be listed by name.

## How a collection is read

1. **Records** are taken in `index.csv` order, each signal a piece of the channel (modality,
   signal description, unit). An `index.csv` without a `record` column is refused (`index.csv
   has no record column`). A row whose `record` is empty, absolute or has a `..` component is
   left out (under its name, or `row N` when empty, counting the rows after the header from 1),
   with the reason `index.csv names a record outside its folder (an absolute name, or one with
   ..)`; a record that cannot be read is left out with the reason, as in
   [a folder of records](#a-folder-of-records-one-session); the recording's notes say how many
   were left out, and a collection of which no record can be read is refused.
2. **Placement on the time axis.** A record with `start_unix` starts there. A record without one
   follows the data before it, after the break `gaps.csv` states before it (`gap_s` on a row of
   scope `all` whose `records_after` names it), and records of the same `stretch` start
   together. Such records are drawn as **time not stated**: their place is known relative to
   their neighbours, not on the clock.
3. **Breaks** (`gaps.csv`, except the record splits) are drawn between the data, with their
   reason and length; a break without stated times takes them from the records on each side.
4. **Vital signs, events and markers** as above.

The three VitalStream viewers and Medorama read a collection to the same index, byte for byte;
[`viewers/vitalstream/testdata/collection`](../viewers/vitalstream/testdata/collection) is the
test collection they are checked on.

## One record

A record's header file (`rec.hea`) opens **that record only**: its signals, read from the
signal files its header names, with its `.mrk` markers. Its start is the header's base date
and time, taken as UTC; a header that states no date and time gives data marked **time not
stated**, which starts at 0 (1970-01-01). A record whose header states no sampling frequency, or
one that is not a finite number greater than 0 (`nan`), is not read.

**Samples stated and samples held.** A header may state more samples than its signal files hold.
The number of samples read is the least of the number the header states and, for each signal
file whose signals all have a storage format of fixed size, the frames that file can hold:
`ceil(8 × file size in bytes / bits per frame)`, where a frame takes, for each signal of that
file, its samples per frame (at least 1) times the bits of its format (8 and 80: 8; 16, 61 and
160: 16; 212: 12; 310 and 311: 32/3; 24: 24; 32: 32). Files of other formats (compressed) and
files that do not exist are not counted. Reading also stops at the end of the data, however
many samples are stated. The file's byte offset is not subtracted, so a well-formed record keeps
the number its header states.

**Files the record reads** are the header, the signal files it names (an absolute name as given,
a name starting with `~` in the home folder, any other next to the header), its `.mrk` file and,
for a multi-segment record, those of its segments. A viewer opens none of them when it leads
outside its data roots, symbolic links resolved
([index-format.md](../viewers/vitalstream/docs/index-format.md#files-outside-the-data-roots)).

## Multi-segment records

A multi-segment record (WFDB's `name/n` record line, followed by `n` segment lines) is read as
one record:

- Its **segments** follow each other from the record's start (the master header's base date
  and time), each at its offset: the samples of the segments before it, divided by the record's
  sampling frequency. Each signal of each segment is a piece of the channel of its name.
- A **null segment** (`~`) is a break of known length (its samples ÷ the sampling frequency),
  shown with the reason `null segment`.
- A **layout segment** (the first segment, of length 0, of a record with a variable layout)
  names the record's signals and holds no samples. In a variable layout, a segment holds some
  of the signals; a signal absent from a segment is a break in its channel only.
- The record's **markers** are the master's `.mrk` file: sample numbers from the record's start
  and `chan` numbers of the master header's signals (the layout's). Each marker goes to the
  segment its sample falls in and to the signal of that name there; a marker in a null segment
  is left out.
- Opening one **segment's own header** (`rec_0001.hea`) reads that segment alone, as one record.

Segments are found next to their master header. Medorama and the viewers index a multi-segment
record as one recording whose files are the master header, the layout header and every
segment's header, signal files and markers ([the index key](../viewers/vitalstream/docs/index-format.md#the-key)).

## A folder of records: one session

A folder without `index.csv` is read as **one session**: every record in it and in its
subfolders, on one time axis. This is how a patient's records, split into folders by device or
by day, are seen together.

1. **Records** are the `.hea` files below the folder, at every level, in the order of their
   path relative to the folder (`/` between folders, compared character by character). Hidden
   entries (names that start with `.`) and symbolic links are skipped. The segments and layout
   header of a multi-segment record are part of that record, never records of their own.
2. **Placement:** a record whose header states a base date and time starts there (UTC). A
   record without one is placed after the data before it (in that order), marked **time not
   stated**.
3. A record that cannot be read, whose header states no valid sampling frequency, or one of
   whose files leads outside the data roots, is left out; the recording's notes say how many were
   left out.
4. A `RECORDS` file is not needed and not read. `vitals.csv`, `events.csv` and `gaps.csv`, when
   present in the folder itself, are read as for a collection.

When a tree of folders is searched for recordings (Medorama's import), a folder is a recording
when it holds records itself (a `.hea` file or a `RECORDS` file) or an `index.csv`; it is then
taken whole, its subfolders included. The viewers' **Open multiple records…** opens any folder.

## Overlapping records

A signal recorded twice at the same time (two records of the same channel whose times overlap)
is never drawn on top of itself: each piece goes into the first **lane** of its channel that it
does not overlap, and the second lane is a channel of its own, named with its lane: `II`,
`II (2)`, `II (3)` … Pieces are placed in the order of the records (index.csv's order, or their
paths), and the signals of a record in header order. The recording's notes say so when it
happens. A channel is a modality (`wfdb` for plain records), a signal description and a unit.

## Limits

So that opening a large folder by mistake cannot overwhelm a viewer, a folder of plain records
is read only within these limits. They are VitalStream settings (`max_records`, `max_depth` and
`max_scan_s` in the viewer's settings file). Medorama has no such settings, and indexes a
recording's folder within the defaults:

| Limit | Default | Over it |
|---|---|---|
| header files below the folder (segments and layout headers counted) | 5000 | refused |
| folder levels below the folder | 20 | refused |
| seconds to count the records before opening (the viewers' summary) | 10 | refused |

A refusal says which limit was reached and what to do instead: open a subfolder, or describe the
records in an `index.csv` (a collection lists its records itself and is not limited by these).
Every recording, a collection included, has at most 100,000 pieces (signals × records), and a
folder's index key is computed from at most 200,000 of its files within `max_depth` levels
([index-format.md, Limits](../viewers/vitalstream/docs/index-format.md#limits)). The ECG
viewer's **Open multiple records…** lists at most 5000 ECGs of a folder.
Before opening a folder, the viewers show what it holds (records, segments, signals, time span,
size), read from the headers alone; indexing starts only when it is confirmed, shows its
progress, and can be cancelled.

## An example

The smallest collection: one ECG record with a start time, and an alarm.

```
index.csv
  record,modality,start_unix
  ecg/rec_0001,ecg,1709287200.0
events.csv
  start_unix,end_unix,level,text,kind
  1709287201,1709287203,2,HR HIGH 151 > 150,alarm
ecg/RECORDS
  rec_0001
ecg/rec_0001.hea, ecg/rec_0001.dat
```

[`viewers/vitalstream/testdata/generate.py`](../viewers/vitalstream/testdata/generate.py) writes
a complete one (three stretches, a break of stated length, a record without a start time, vital
signs from two sources, alarms and markers) with the Python standard library only.
