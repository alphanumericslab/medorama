# Libraries

This folder holds the Rust libraries that Medorama and the Rust VitalStream viewer share. It is
for developers who work on reading recordings or on the viewers. The C++ and Python viewers
implement the same behavior in their own languages, and the three implementations are checked
against each other by the [parity tests](../viewers/vitalstream/tests/parity).

| Library | Crate | What it provides |
|---|---|---|
| [engine](engine) | `recording-engine` | Everything needed to show a recording quickly without a server around it: the readers for WFDB records, collections and folders of records ([WFDB records and collections](../docs/wfdb-collection.md)) and for EDF, EDF+ and BDF files ([EDF](../docs/edf.md)); other formats through installed add-ons ([adapters](../docs/adapters.md)); the index ([index format](../viewers/vitalstream/docs/index-format.md)); and the queries the viewers answer. |
| [signal-core](signal-core) | `signal-core` | Values, number formatting and text helpers that behave exactly as Python does, so that the three implementations write byte-identical indexes, JSON and CSV. |

Both libraries are licensed like the rest of the repository ([LICENSE](../LICENSE)).
