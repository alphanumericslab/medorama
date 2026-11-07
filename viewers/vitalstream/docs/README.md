# VitalStream specifications

This folder holds the specifications that the three VitalStream viewers follow: the Python
viewer ([`../python/vitalstream`](../python/vitalstream)), the Rust viewer ([`../rust`](../rust))
and the C++ viewer ([`../cpp`](../cpp)). They are written for developers of the viewers, of
adapters, and of programs that share an index folder with them. The
[parity tests](../tests/parity) hold the three viewers to these pages. For how VitalStream fits
into Medorama, see [docs/architecture.md](../../../docs/architecture.md).

| Document | Covers |
|---|---|
| [data-model.md](data-model.md) | What the viewers show: channels, pieces, lanes, time, breaks, units, missing values, vital signs, alarms, annotations and markers. |
| [index-format.md](index-format.md) | The index that the viewers and Medorama write and share, byte by byte (version 2), including WFDB and EDF recordings. |
| [viewer-api.md](viewer-api.md) | The viewers' HTTP API, version 1: browsing, `scan`, `open`, `cancel` and the dataset queries. |
| [naming.md](naming.md) | The names of the commands, packages, build options, settings and files of the three viewers. |

The recordings the viewers read are described on these pages:

| Recordings | Page |
|---|---|
| WFDB records: one record, multi-segment records, a folder of records read as one session, collections, and the limits on a folder | [docs/wfdb-collection.md](../../../docs/wfdb-collection.md) |
| EDF, EDF+, BDF and BDF+ files | [docs/edf.md](../../../docs/edf.md) |
| Other formats, read through installed adapters | [docs/adapters.md](../../../docs/adapters.md) |
| How the readers of all of Medorama's formats are organized | [docs/readers.md](../../../docs/readers.md) |
