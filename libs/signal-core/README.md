# signal-core

`signal-core` is the Rust library of values, number formatting and text helpers that the
[recording engine](../engine), the Rust readers and the viewers of [Medorama](../../README.md)
share. This page is for developers. Everything in it behaves as Python does, so that the Rust,
C++ and Python implementations write byte-identical output: the index, JSON answers and CSV
files.

| Module | Contents |
|---|---|
| [`value`](src/value.rs) | `Value` (None, booleans, integers, floats, strings, lists, ordered dictionaries and compact arrays) and the `dict!` macro. JSON text and `str()` and `repr()` are written exactly as Python writes them, and JSON is parsed as Python's `json` module reads it. |
| [`pyfmt`](src/pyfmt.rs) | Number and time formatting identical to Python's: `repr(float)`, `'%.Ng' % x`, `round(x, n)`, the shortest float32 text, UTC ISO times, and CSV rows as Python's `csv` module writes them. |
| [`text`](src/text.rs) | `hex`, and Python's `str.strip()` and `float(str)`. |
| [`Error`](src/lib.rs) | The error type of the readers and the engine. |

Its C++ counterpart is `vs_core`, in `viewers/vitalstream/cpp/core`.

## Example

The following code builds a dictionary and writes it as JSON and as Python's `str()` would:

```rust
use signal_core::{dict, value::Value};
let v = Value::Dict(dict! { "fs" => 250.0, "label" => "II", "n" => 1500 });
assert_eq!(v.to_json(), r#"{"fs":250.0,"label":"II","n":1500}"#);
assert_eq!(v.py_str(), "{'fs': 250.0, 'label': 'II', 'n': 1500}");
```

## Tests

Run the library's tests with `cargo test -p signal-core`.
