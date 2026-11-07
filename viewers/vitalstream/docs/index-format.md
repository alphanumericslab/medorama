# The index format

This page specifies the **index**: the files a program writes once for a recording so that a
viewer can show days of waveforms at once. It is written for developers of programs that write
or read indexes: the VitalStream viewers, Medorama, and adapters for other formats. It is part
of the [VitalStream](../README.md) documentation.

An index holds:
- the recording's structure: its channels, and the pieces each channel is made of;
- minimum and maximum envelopes of every waveform, at several resolutions;
- markers, vital-sign series, alarms and other events, and breaks in the data.

Four programs write and read indexes, and they can share one index folder:
- the three VitalStream viewers: `vitalstream-rs` (with the Rust engine, `libs/engine`),
  `vitalstream-cpp` (with the C++ engine) and `vitalstream-py`;
- Medorama, which indexes with the Rust engine.

Adapters ([docs/adapters.md](../../../docs/adapters.md)), the add-on programs that read formats
the viewers do not read themselves, write the index of those formats in this format too. Every
program writes the same bytes for the same recording, and a test checks it
([Conformance](#conformance)).

- [Index folder](#index-folder)
- [The key](#the-key)
- [Files of one index](#files-of-one-index)
- [meta.json](#metajson)
- [Arrays](#arrays)
- [Source-specific fields](#source-specific-fields)
- [EDF, EDF+ and BDF](#edf-edf-and-bdf)
- [Limits](#limits)
- [Files outside the data roots](#files-outside-the-data-roots)
- [Writing and sharing](#writing-and-sharing)
- [Loading an index](#loading-an-index)
- [Versions](#versions)
- [Conformance](#conformance)

## Index folder

An index folder holds one subfolder per indexed recording, named by the recording's
[key](#the-key):

```
<index folder>/
├── 0f38a5788406244247a4/        one recording
│   ├── meta.json
│   ├── env_min_0.f32  env_max_0.f32  env_min_1.f32 …
│   ├── mk_piece.i32  mk_sample.i64  mk_code.i16
│   └── vit_t.f64  vit_v.f64
└── …
```

Readers ignore any other entry in an index folder, such as temporary folders and the Python
viewer's progress files (`<key>.progress.json`).

## The key

The key identifies a recording **as it is now**. It changes when the recording's files are
changed, moved or replaced, so that an outdated index is never used.

```
key = first 20 hex digits (lowercase) of SHA-256( JSON text of [version, path, fingerprint] )
```

| Element | Value |
|---|---|
| `version` | The index format version, `2`. |
| `path` | The recording's absolute path with every symbolic link resolved (the real path). On Windows, the path is written without the `\\?\` prefix: `C:\data\x.Stp`, and `\\server\share\x.Stp` for network shares. |
| `fingerprint` of a single file (an EDF or BDF file, or a file an adapter reads) | `[size, mtime]`: the size in bytes, and the modification time in **integer nanoseconds** since 1970-01-01 UTC. On Windows, the time is the file's last-write time, which has a resolution of 100 ns. |
| `fingerprint` of a folder (WFDB) | The list of `[name, size, mtime]` of **every file** below the folder, subfolders included. Folders themselves and hidden entries (names that start with `.`) are excluded. A symbolic link to a file counts under the link's name, with its target's size and mtime; links to folders and broken links are excluded. `name` is relative to the folder, with `/` between components on every system. The list is sorted by `name`, then size, then mtime. **Bounds:** the folder is walked depth first, the entries of each folder in order of their names (byte order of the UTF-8 names); subfolders are entered down to `max_depth` levels below the folder ([the limits on a folder of records](../../../docs/wfdb-collection.md#limits), 20 by default), and the walk stops once **200,000** files are listed. |
| `fingerprint` of a WFDB record (a `.hea` file) | The same list for **the record's files**: its header, the signal files the header names (other than `-` and `~`), its `.mrk` file if there is one, and, for a multi-segment record, the same files of each segment and of the layout header. `name` is relative to the header's folder; each file appears once; the list is sorted by `name` ([multi-segment records](../../../docs/wfdb-collection.md#multi-segment-records)). |

**The JSON text** is written as follows:
- It is compact, with no spaces: `[2,"/data/x.Stp",[133325312,1700000000123456789]]`.
- Strings are in double quotes, with these escapes:
  - `\"`, `\\`, `\n`, `\r`, `\t`, `\b`, `\f`;
  - every other character below U+0020 or above U+007E as `\u` followed by 4 lowercase hex
    digits (as a UTF-16 surrogate pair above U+FFFF).
- Integers are written in decimal.

**Example.** For a made-up file `/data/rec.Stp` of 1000 bytes, modified at 1700000000.5 s, the
text is

```
[2,"/data/rec.Stp",[1000,1700000000500000000]]
```

and the key is the first 20 hex digits of that text's SHA-256: `f61f534c1e1a4d244d7a`.

## Files of one index

| File | Content |
|---|---|
| `meta.json` | The recording's structure ([meta.json](#metajson)), as UTF-8 JSON. |
| `env_min_<L>.f32`, `env_max_<L>.f32` | Envelope level `L` = 0, 1, …: the minima and maxima of all pieces, concatenated. |
| `mk_piece.i32`, `mk_sample.i64`, `mk_code.i16` | Markers: the piece, the sample number, and an index into `marker_names`. |
| `vit_t.f64`, `vit_v.f64` | Vital-sign times (Unix seconds, UTC) and values, all series concatenated. |

**Array files** follow these rules:
- Each holds raw little-endian values of the type its extension names (`f32`, `f64`, `i16`,
  `i32`, `i64`), with no header.
- The number of values is the file size divided by the value size.
- An array with no values is an empty file.
- The marker and vital-sign files always exist. Envelope files exist for levels 0 … L-1 only.

## meta.json

`meta.json` is a JSON object, written compact (no spaces) and in ASCII, with the escapes of the
key's text.

**Numbers** are written as follows:
- Integers are written without a decimal point.
- Other numbers are written in the shortest form that reads back exactly (`0.1`, `1e-05`,
  `1709287200.0`).
- `NaN` and infinities never appear: they are written as `null`.

| Key | Type | Meaning |
|---|---|---|
| `index_version` | integer | `2`. |
| `key` | string | The [key](#the-key). |
| `path` | string | The real path used in the key. |
| `name` | string | The file or folder name as opened. |
| `kind` | string | The recording kind: `"wfdb"`, `"edf"`, or the kind of the adapter that read it (`"stp"`). |
| `format` | string | The format's label: `"WFDB record"`, `"WFDB multi-segment record"`, `"WFDB collection"`, `"WFDB records"` (a folder of plain records), `"EDF"`, `"EDF+"`, `"BDF"`, `"BDF+"`, or the adapter's label (`"BedMasterEx .Stp"`). |
| `adapter` | string | The adapter that read the recording, when one did. The adapter reads the samples again on demand. |
| `description` | string | Optional. A sentence about the recording from its adapter (`"GE Unity / Carescape"`). |
| `engine` | string | The version of the program that wrote the index (informative). |
| `indexed_s` | number | The seconds the indexing took (informative). |
| `time_resolution_s` | number or null | The time resolution of the source's time stamps. |
| `channels` | list | Per channel: `id` (its position), `label`, `unit`, `modality`, and source-specific fields. |
| `pieces` | list | Per piece (a stretch of one channel that the source proves contiguous): `id` (its position), `ch` (the channel id), `fs` (Hz), `n` (samples), `t0` (Unix seconds, UTC, of sample 0), `timed` (false when the source states no time; when the key is omitted, true), `env` (per level, `[offset, length]` into that level's envelope arrays), and [source-specific fields](#source-specific-fields). |
| `vitals` | list | Per series: `name`, `unit`, and `offset` and `n` into `vit_t` and `vit_v`. |
| `alarms` | list | Alarms and other events: `t0`, `t1`, `text`, `level` (1 advisory, 2 warning, 3 crisis, or null), `kind`. EDF+ annotations are events of kind `annotation` ([EDF, EDF+ and BDF](#edf-edf-and-bdf)). |
| `gaps` | list | Breaks in the recording: `t0`, `t1`, `reason`, `gap_s`, `exact`, `measured_by`, `notes`, and optionally `channels`. |
| `untimed` | list | Intervals without a stated time: `t0`, `t1`. |
| `late_blocks` | list | Optional. Data blocks the source stored out of order (written by the stp-adapter for PIICiX files): `t0` and `t1` (the block's time span; `t1` is null when unknown), `moved` (true: put back in order, so there is no break; false: left in file order, and the break it causes is in `gaps`, with a note), `reason` (why it was not moved; empty when moved), `late_by_s`, `late_by_blocks` (null when not counted), `segment`. The viewers mark the moved blocks only when their `qc_verbose` setting is on. |
| `marker_names` | list of strings | The marker names, indexed by `mk_code`. |
| `skipped` | object | What could not be shown: an adapter's counts; for WFDB records, the records left out, by name, with the reason. |
| `notes` | list of strings | Remarks for the user. |
| `reader` | object | What the source's reader needs to read samples again (an adapter's state). Only the program that reads the source writes and reads it; other programs must not depend on it. |
| other keys | | Source-specific. Readers ignore keys they do not know. |

The stp-adapter, which reads BedMasterEx `.Stp` files, is a private add-on available to
institutions under agreement ([docs/adapters.md](../../../docs/adapters.md)).

**Order of the keys.** The reader's keys come first, in the order the reader writes them. Then
come `kind`, `format` and `adapter`, then `path`, `name`, `key`, `index_version`, `engine` and
`indexed_s`. Programs that share an index folder write the keys in the same order, so that they
write the same bytes.

## Arrays

**Envelopes** are built as follows:
- Level 0 holds, for each piece, the minimum and maximum of every block of **32** samples, in
  order. The last block may be shorter.
- Each further level combines **4** consecutive buckets of the level below, until a piece has a
  single bucket.
- Minima and maxima ignore NaN (as NumPy's `fmin` and `fmax` do). A bucket whose samples are all
  NaN is NaN.
- Values are in the channel's physical units.
- Piece `p`'s buckets at level `L` are `env_min_L[o : o+len]` and `env_max_L[o : o+len]`, where
  `[o, len] = pieces[p].env[L]`.

**Markers** are sorted by piece, then by sample. Markers with equal values keep the order in
which the source gave them. A marker's time is `t0 + sample / fs` of its piece.

**Vital signs** follow these rules:
- Series are ordered by number of values (most first), then by name.
- Within a series, values are sorted by time.
- Only finite times and values are stored.
- Series `s` is `vit_t[offset : offset+n]` and `vit_v[offset : offset+n]`.

## Source-specific fields

Each built-in reader adds fields of its own to the pieces, after the common keys. The viewers use
them to read samples again; other programs may ignore them.

| Source | `kind` | Piece fields, after the common ones | Channel `modality` |
|---|---|---|---|
| WFDB records | `wfdb` | `record` (the record's name, relative to the folder it is read from) and `sig` (the signal's position in its header), in that order | the modality an `index.csv` states for the record, else `wfdb` |
| EDF, EDF+, BDF, BDF+ files | `edf` | `sig` (the signal's position in the header, annotation signals included) and `data_record` (the run's first data record), in that order | `edf` |
| Files an adapter reads | the adapter's | the adapter's own | the adapter's |

## EDF, EDF+ and BDF

The engines read EDF, EDF+, BDF and BDF+ files themselves, without an adapter. One file is one
recording, and a file is read as EDF when its name ends in `.edf` or `.bdf`, in any letter case.
This section states what the index of such a file holds. The complete reading rules (the header
fields, the start time, the data records and the time-stamped annotation lists) are in
[docs/edf.md, Reading rules](../../../docs/edf.md#reading-rules-for-implementers); the Rust
reader is [`libs/engine/src/sources/edf.rs`](../../../libs/engine/src/sources/edf.rs).

| Key | Value for an EDF or BDF file |
|---|---|
| `kind` | `"edf"`. |
| `format` | `"EDF"`, `"EDF+"`, `"BDF"` or `"BDF+"`. EDF+ and BDF+ files are those whose reserved header field starts with `EDF+C`, `EDF+D`, `BDF+C` or `BDF+D`. |
| `channels` | One per ordinary signal (a signal that is not an annotation signal and has samples), in signal order, created before any data record is read. Each has `id`, `label`, `unit` (the signal's physical dimension) and `modality` `"edf"`. A label that repeats gets its count: the second `ECG` is `ECG (2)`, the third `ECG (3)`. |
| `pieces` | One piece per ordinary signal and **run**, in signal order within each run. A run is a sequence of data records in which each record starts where the one before it ends, to within 10⁻⁶ s; EDF+D files can hold several. Each piece has the common keys (`id`, `ch`, `fs`, `n`, `t0`, `timed`, `env`), then `sig` and `data_record`. `fs` is the signal's samples per data record divided by the duration of a data record. `t0` is the file's start plus the onset of the run's first record. `timed` is always written; it is false for every piece when the header states no valid start. |
| `alarms` | The annotations of EDF+ and BDF+ files. Every non-empty text of every time-stamped annotation list is one event, with `t0` (the file's start plus the onset), `t1` (`t0` plus the duration when the duration is positive, else `t0`), `text`, `level` null and `kind` `"annotation"`. The events are sorted by `t0` with a stable sort, so that events at the same time keep the order of the file. The viewers show them in an **Annotations** lane. |
| `gaps` | One break between consecutive runs whenever the next run starts after the previous one ends: `t0` (the end of the previous run), `t1` (the start of the next), `reason` `"not recorded"`, `gap_s` (`t1 − t0`), `exact` true, `measured_by` null and `notes` `""`. |
| `untimed` | When the header states no valid start, the time spans of the pieces (merged); otherwise empty. |
| `vitals`, `marker_names` | Empty lists. |
| `skipped`, `reader` | `{}`. |
| `time_resolution_s` | `null`. |
| `notes` | First, `The file's start time is taken as UTC.`, or, when the header states no valid start, `The file states no valid start time: times are counted from its start, and marked "time not stated".` Then, when the header states more data records than the file holds, `The header states N data records; the file holds M.`; otherwise, when bytes follow the last whole data record, `The file ends inside a data record: its last N bytes are left out.` |

**Order of the reader's keys.** The reader writes `channels`, `pieces`, `vitals`, `alarms`,
`gaps`, `untimed`, `marker_names`, `skipped`, `notes`, `time_resolution_s` and `reader`; then
come `kind` and `format`, and the common keys ([Order of the keys](#metajson)). There is no
`adapter` key.

**The key** of an EDF or BDF file is a single file's key, from its size and modification time
([The key](#the-key)).

**What is never in the index.** The header's local patient identification and local recording
identification fields hold the patient's identity. No program writes them, or anything read
from them, into the index. The test file `testdata/edf/psg_plus.edf` carries an invented
identity in its patient field, and the tests check that it appears in no index file.

**Files that are refused.** A file with no ordinary signal (an annotation-only EDF+ file) and a
file with no whole data record are not indexed: opening them fails with a message that says why.

## Limits

- **Pieces.** A recording has at most **100,000** pieces. A reader that would create more (an
  EDF+D file with many short runs, a folder of many records with many signals) refuses the
  recording with `the recording has more than 100000 pieces (stretches of one signal): it is not
  indexed`. The check is made before each new run (EDF) or record part (WFDB) adds its pieces.
- **Marker names.** `mk_code` is 16-bit, so at most **32,768** distinct names are kept. The first
  32,767 names keep codes 0 … 32,766; the next name found is written as `(other markers)` with
  code 32,767, and every later new name gets that code too. The WFDB reader then adds the note
  `More than 32767 distinct marker names: the others are shown as "(other markers)".` after the
  note about overlapping records.

## Files outside the data roots

A program that limits the folders it opens (the viewers' data roots) also limits every file a
reader opens for a recording, symbolic links resolved: a WFDB record's header, the signal files
its header names (an absolute name as given, a name starting with `~` in the home folder, any
other next to the header), its `.mrk` file, each segment's header and signal files, and the CSV
files of a folder or collection. A file that does not exist is allowed (nothing is read from
it); a broken link is not. This is checked before a record is indexed and again each time its
samples are read:
- one record (a `.hea` file opened): refused (`403` from the viewer's `open`);
- a record of a folder or collection: left out, in `skipped`, with the reason `<file> lies
  outside the data roots: the record <name> is not read`;
- samples read later from a file that now leads outside: an error.

The Rust engine does this once a program calls `recording_engine::paths::set_allowed_roots`.

## Writing and sharing

- **Atomic writing.** A writer builds the index in a temporary folder next to the final one,
  named uniquely for that writer: `<key>.<process id>.<n>.tmp`, `n` counting the writer's builds.
  The temporary folder is removed on every way out (an error, a cancellation, a failure of the
  program).
- **One writer per key.** Within a program, builds of the same key wait for each other.
- **Keeping or replacing.** When the build is complete, the writer checks the folder `<key>`. If
  it holds a complete index ([Loading an index](#loading-an-index)) written by the same program
  version (`engine`), it is kept and the new build is discarded: the content is the same.
  Otherwise the old folder, if any, is first renamed aside (`<key>.<process id>.<n>.old`), the new
  one is renamed to `<key>`, and the old one is removed. If that rename fails because another
  program has just moved its own complete index of the key in, that one is kept. Readers
  therefore see a complete index, or none for the moment between the two renames.
- **Shared folders.** Several programs (Medorama and the three viewers) may use one index folder
  at the same time. When two write the same key, the content is identical, and either one is
  kept.
- **Read-only after writing.** An index is never modified after it is written, so readers may
  memory-map its arrays.
- **Version check.** A reader uses an index only if `meta.json` exists and its `index_version`
  equals the version the reader implements, and the index passes the checks of
  [Loading an index](#loading-an-index). Otherwise it indexes the recording again.
- **Never next to the recording.** An index folder is never placed inside a folder of
  recordings.

## Loading an index

A reader checks an index before it uses it, so that no query reads outside an array, whoever
wrote the files. An index that fails any check is treated as not indexed. A writer runs the
same checks on what it has just built, and refuses to keep an index that fails them (for
example times or rates that are not finite numbers, which `meta.json` would hold as `null`).

1. `meta.json` is a JSON object whose `index_version` is the reader's version.
2. Every array file exists (`env_max_L` for every `env_min_L` found, the levels counted from 0
   until `env_min_L` is missing), and its size is a whole number of values.
3. `channels`, `pieces`, `vitals` and `marker_names` are lists when present (missing: empty).
4. Piece `i`: `id` is the integer `i`; `ch` is an integer with `0 ≤ ch < len(channels)`; `fs` is a
   number (integer or finite float) greater than 0; `t0` is a number (integer or finite float);
   `n` is an integer `≥ 0`; `env`, when present, is a list of at most as many entries as there are
   envelope levels, and entry `L` is `[o, len]`, two integers with `o ≥ 0`, `len ≥ 0` and
   `o + len ≤` the number of values of the shorter of `env_min_L` and `env_max_L`.
5. `mk_piece`, `mk_sample` and `mk_code` have the same number of values; every `mk_piece` value
   is a piece id (`0 ≤ p < len(pieces)`) and the values never decrease; every `mk_code` value is
   an index into `marker_names`.
6. `vit_t` and `vit_v` have the same number of values, and every series has integers `offset ≥
   0`, `n ≥ 0` with `offset + n ≤` that number.

## Versions

| Version | Changes |
|---|---|
| 1 | The first version. |
| 2 | The current version, described on this page. It added the key of one WFDB record (a `.hea` file), multi-segment records, folders of plain records read with their subfolders (hidden entries and symbolic links skipped, in the folder's key too), and overlapping pieces of a channel in lanes of their own. **EDF, EDF+, BDF and BDF+ files** were added later within version 2, as a new `kind` (`edf`): no existing index changes, so indexes already written stay valid. Later still within version 2: a folder's key counts the files its symbolic links lead to, and its walk is bounded (the keys of folders without links, of at most 200,000 files within `max_depth` levels, are unchanged); a WFDB header that states more samples than its signal files hold is read to the end of its data; the [limits](#limits) and the [checks on loading](#loading-an-index) were added. |

Any change to the files, to `meta.json` or to the key of an existing kind of recording makes a
new version, and indexes of other versions are built again.

## Conformance

[`tests/parity/index_parity.py`](../tests/parity/index_parity.py) indexes recordings with every
viewer that is built, and checks that:
- each key equals the key computed from this specification by an independent implementation in
  the script;
- the viewers' index files are byte-identical (`meta.json` is compared without its informative
  `engine` and `indexed_s`).

By default it uses the synthetic recordings of [`testdata/`](../testdata): WFDB records and
collections, and the EDF, EDF+ and BDF files of `testdata/edf/`. Run it from the repository
root on those recordings, or on recordings an adapter reads:

```bash
python viewers/vitalstream/tests/parity/index_parity.py
python viewers/vitalstream/tests/parity/index_parity.py --adapters-dir DIR FILE…
```

Continuous integration runs it on Linux, macOS and Windows (`.github/workflows/cpp.yml`).
