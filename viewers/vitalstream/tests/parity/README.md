# Parity tests: the three viewers against each other

This folder holds the tests that run the three [VitalStream](../../README.md) viewers against
each other. It is written for developers who change any of the three. The viewers must write the
same index and give the same answers, so that any of them can open a recording another one
indexed, and Medorama can run any of them.

| Script | Checks |
|---|---|
| [`index_parity.py`](index_parity.py) | Each viewer indexes each recording (with `--index`) into its own folder. The keys must be equal to each other and to the key computed in the script from [the specification](../../docs/index-format.md#the-key); every array file must be byte-identical; `meta.json` must be byte-identical apart from its informative `engine` and `indexed_s`. |
| [`viewer_parity.py`](viewer_parity.py) | Each viewer is started on a free port, and each recording is opened in each. The dataset description and the window, vital-sign, cursor and marker requests are compared value by value, at many zoom levels. Before the recordings, it compares the answers and errors of requests about folders and records: `config`'s limits, `browse` (including a folder of EDF files and their format labels), `scan`, `open` and `cancel` of folders over a limit or without records, and `complete` with symbolic links. Then a table of errors ([viewer-api.md, Requests and Errors](../../docs/viewer-api.md#requests)): each request's status must be the one the table gives, and the `detail` texts must be equal (of a malformed body, its `bad request body:` prefix). The table covers the `Host` check, `Origin` and `Sec-Fetch-Site`, `415`, `413`, body field types, `404` and `405`, the query grammar (`t0`, `t1`, `t`, `px`, `ch`, `names`, a parameter given twice, times far outside the recording), a record whose signal file leads outside the data roots (`403`), the error state of recordings that cannot be indexed, and the `nosniff` and CSP headers. |

Without arguments, both scripts use the synthetic recordings of [`testdata/`](../../testdata):
WFDB records, collections and sessions, and the EDF, EDF+ and BDF files of `testdata/edf/`. They
also make recordings for the rules of [index-format.md](../../docs/index-format.md) and
[wfdb-collection.md](../../../../docs/wfdb-collection.md) in a temporary folder: an EDF+D file of
3000 runs (more pieces than pixel columns) and a collection that skips what it cannot read (both
scripts); a header that states more samples than its file holds, a rate that is not a number, a
record of 300 signals, more distinct marker names than codes (with the Python module `wfdbx`)
and a folder of symbolic links (`index_parity.py`).
Run them from the repository root:

```bash
python viewers/vitalstream/tests/parity/index_parity.py                     # every viewer that is built
python viewers/vitalstream/tests/parity/viewer_parity.py --impl py,cpp      # two of them
python viewers/vitalstream/tests/parity/index_parity.py --adapters-dir DIR FILE…   # recordings an adapter reads
```

The scripts find the viewers as [`impls.py`](impls.py) describes: `vitalstream-py` in the
Python environment that runs the script, `target/release/vitalstream-rs`, and
`viewers/vitalstream/cpp/build/release/bin/vitalstream-cpp`. Continuous integration runs both on
Linux, macOS and Windows (`.github/workflows/cpp.yml`); `make parity` runs them locally.
