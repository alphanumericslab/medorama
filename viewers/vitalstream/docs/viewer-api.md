# Viewer HTTP API, version 1

This page specifies the HTTP API between the VitalStream page and its server. It is written for
developers who change a viewer, write a program that drives one, or put a viewer behind another
server. It is part of the [VitalStream](../README.md) documentation.

The three viewers, `vitalstream-py`, `vitalstream-rs` and `vitalstream-cpp`, implement this API
and give the same answers to the same requests;
[`tests/parity/viewer_parity.py`](../tests/parity/viewer_parity.py) checks it. A breaking change
gets a new version (`/api/v2/`).

- [Conventions](#conventions)
- [Requests](#requests)
- [Inside Medorama](#inside-medorama)
- [Sign-in](#sign-in)
- [Endpoints](#endpoints)
- [Folder entries](#folder-entries)
- [Errors](#errors)

## Conventions

- All paths are relative to the address the viewer is served at, so the viewer also works under
  a path prefix (for example `https://portal/rnode/<node>/8050/api/v1/config`).
- Times are Unix seconds (UTC).
- NaN values are sent as `null`.
- The terms (pieces, breaks, units) are defined in [data-model.md](data-model.md).

## Requests

The server checks every request before it routes it, in this order. The Rust viewer is the
reference; the other viewers give the same statuses and the same checks.

1. **Host.** The `Host` header (or, without one, the request's authority) must name a host the
   server answers for, compared without its port and in any letter case: `localhost`,
   `127.0.0.1`, `::1` (`[::1]`), the `host` the server listens on (unless that is `0.0.0.0` or
   `::`), or a name in the setting `allowed_hosts` (`"*"` allows any). Otherwise the answer is
   `400`. This defeats DNS rebinding, where another site's name is made to point at the
   viewer's address. A viewer behind a reverse proxy that passes the public host name on lists
   that name in `allowed_hosts`; Medorama's gateway sends `127.0.0.1:<port>`.
2. **Requests that change something** (any method other than `GET` and `HEAD`):
   - an `Origin` header, when present, must be `http://` or `https://` followed by a host
     allowed as in 1 (no path); `null` and any other origin are refused (`403`);
   - `Sec-Fetch-Site: cross-site` is refused (`403`);
   - a `POST` under `api/` must have `Content-Type: application/json` (parameters such as
     `; charset=utf-8` allowed; `415` otherwise). Another site's page cannot send such a
     request without the browser asking the server first, and the server never allows it.
3. **Sign-in** ([Sign-in](#sign-in)): `401` without it.
4. **Routing:** an unknown path is `404`, and a known path with another method `405`
   (`{"detail": "Method Not Allowed"}`).

**Bodies.** `POST api/v1/open` and `POST api/v1/cancel` take a JSON object of at most 64 KiB,
and `POST login` a form of at most 4 KiB; a larger body is `413`. `open` takes `path` (a string)
and `reindex` (a boolean, default false); `cancel` takes `key` (a string). Any other type, such
as `{"reindex": "yes"}`, is `400`, and so is a body that is not a JSON object (an array, a
number); unknown fields are ignored.

**Query parameters** are `application/x-www-form-urlencoded`. A parameter given twice
(`ch=0&ch=1`) is `400`. Numbers are checked strictly, and anything else is `400`:
- a time or other number (`t0`, `t1`, `t`) is a finite decimal number:
  `[+-]` digits `[.` digits`]` `[e[+-]` digits`]`, or `.` digits, with nothing around it.
  `-inf`, `nan`, `0x10`, `1_0`, ` 1` and `1e400` are refused;
- an integer (`px`) is `[+-]` digits, within 64 bits;
- `ch` is a comma-separated list of channel ids, digits only, each the id of a channel of the
  dataset (`0 ≤ id <` the number of channels). An id given more than once is answered once, in
  the order first given, and at most 64 distinct ids are taken;
- `names` (`vitals`) is a `|`-separated list of series names. A name given more than once is
  answered once, at most 512 distinct names are taken, and names the dataset does not have are
  left out of the answer;
- in `window`, `vitals` and `markers`, `t1` must be greater than `t0`. Times far outside the
  recording are allowed and simply find no data.

**Answers** carry `X-Content-Type-Options: nosniff`. The page and the sign-in page carry
`Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'
data:; object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'`; they
have no inline script or style (the sign-in page links `static/login.css`).

## Inside Medorama

Medorama serves a viewer under its own address (`/viewer/`), behind its own sign-in. Its gateway
passes on only the requests the viewer's page needs (`config`, `browse`, `complete`, `scan`,
`open`, `cancel` and `datasets/…`), with rules per user
([docs/administration.md, The waveform viewer](../../../docs/administration.md#the-waveform-viewer)).
A new endpoint in this API must also be given a rule there; until it has one, Medorama refuses
it.

## Sign-in

When a password is set (with `--set-password`; one password serves all three viewers), every
request except the page itself, `login`, `logout` and `static/…` needs the sign-in cookie.
Without the cookie the server answers `401`.

| Request | Effect |
|---|---|
| `POST login` (form field `password=…`, at most 4 KiB) | Sets the sign-in cookie, valid for 30 days, and redirects to `./`. A wrong password gets `401`, after a delay of one second. |
| `POST logout` | Removes the cookie. When the request carries a valid sign-in cookie, that sign-in also ends on the server: its signature is added to `revoked` in the auth file (`{"<signature>": <expiry>}`, kept until the expiry), so that a copy of the cookie no longer works, also after a restart. |

The cookie is `vitalstream_<port>=<expiry>.<HMAC-SHA256(secret, expiry)>` with `HttpOnly`,
`Path=/` and `SameSite=lax`, and `Secure` when the setting `secure_cookie` is true (a viewer
behind an HTTPS proxy). A cookie whose signature is in `revoked` is refused.

Passwords are checked one at a time (scrypt uses 16 MiB). After wrong passwords from one client
address, the next password from it is checked only after a delay: `2^n − 1` seconds after `n`
wrong ones (1, 3, 7, 15 s), at most 30 s; the count is kept 15 minutes after the last wrong
password, and a right password clears it.
The Rust and Python viewers hold such a request until the delay has passed; the C++ viewer does
not hold it: until the delay has passed it answers `429` with `Retry-After` (the seconds left) and
the sign-in page.

## Endpoints

| Endpoint | Returns |
|---|---|
| `GET api/v1/config` | `timezone`, `data_roots`, `home` (the home folder), `start` (the folder `browse` opens at: the home folder when it lies inside the data roots, else the first data root), `places` (shortcuts), `sep` (the path separator), `version`, `qc_verbose` (the setting), and `limits` (`max_records`, `max_depth`, `max_scan_s`: [the limits on a folder of records](../../../docs/wfdb-collection.md#limits)). |
| `GET api/v1/browse?path=&hidden=` | A folder's entries; without `path`, those of the `start` folder. The answer is `{path, type: "dir", kind, entries, truncated, parent}`, where `kind` is the folder's own (`wfdb` when it holds records itself or is a collection, else `null`) and `entries` are described in [Folder entries](#folder-entries). At most 5000 entries are returned (`truncated: true` beyond them). A path to a recording file (a WFDB record's `.hea`, an EDF or BDF file, or a file an adapter reads) answers `{path, type: "file"}`. |
| `GET api/v1/complete?prefix=` | Up to 50 paths that start with `prefix`, with their `type`. Folders end with the separator. |
| `GET api/v1/scan?path=` | What opening a folder or a WFDB record would read, from the headers alone. A folder is read with its subfolders, as one session. The answer has `path`, `format`, `records`, `segments`, `signals` (their names), `duration_s` (the records' total length), `t_range` (`[first, last]` of the records with a stated time, or `null`), `untimed` (the records without one), `bytes` (the size of the records' files), `skipped` (the records that cannot be read) and `scan_s`. A folder over a limit, or without records, is a `422` whose `detail` says which limit and what to do instead. Any other path (an EDF file among them) is a `400`. |
| `POST api/v1/open` `{"path": …, "reindex": false}` | `{key, state}`, where `state` is `ready`, or `indexing` (the index is being built in the background). `path` is a WFDB record (`.hea`), a folder (its records and those of its subfolders, as one session, or a collection), an EDF or BDF file, or a file an adapter reads. A folder over a limit is a `422`, as for `scan`. A WFDB record any of whose files (its header, the signal files it names, its segments, its `.mrk` file) leads outside the data roots is a `403` ([index-format.md, Files outside the data roots](index-format.md#files-outside-the-data-roots)). If building the index fails in any way, the dataset's state becomes `error`, never stays `indexing`. |
| `POST api/v1/cancel` `{"key": …}` | Stops the index being built under `key` and answers `{key, state: "cancelling"}`. The dataset's state then becomes `cancelled`, and nothing is left in the index folder. For a key already cancelled, the answer is `200` `{key, state: "cancelled"}`; for any other key (no index being built, nor cancelled, under it), `404`. |
| `GET api/v1/datasets/{key}` | `{state: "indexing", progress: {state, fraction, elapsed_s}}`, `{state: "error", error}`, `{state: "cancelled"}`, or `{state: "ready", dataset}`. The dataset is `{key, name, path, kind, format, description, t_range, time_resolution_s, channels, pieces: [{id, ch, t0, fs, n, timed}], vitals: [{name, unit, n}], alarms, gaps, untimed, late_blocks, markers: {names, count}, skipped, notes, indexed_s}`; `gaps` are the breaks, and `alarms` include EDF+ annotations (kind `annotation`). |
| `GET api/v1/datasets/{key}/window?t0&t1&px&ch=0,1` | `{t0, t1, channels: {"0": [block, …], …}}`: per channel, the blocks of its pieces that cover `[t0, t1]` at about `px` points (default 1200, clamped to 16–8000), in order of piece id. Each block is `{piece, kind: "raw", t0, dt, y}` (samples) or `{piece, kind: "env", t0, dt, min, max}` (minimum and maximum per bucket). When more than `px` pieces of a channel fall in the window, only the first piece (in order of `t0`, then id) to start in each pixel column is answered, the column being `floor((max(t0 of the piece, t0) − t0) / (t1 − t0) × px)`; so a channel never has more than `px` blocks. |
| `GET api/v1/datasets/{key}/vitals?t0&t1&px&names=HR\|SpO2` | `{name: series, …}`. Each series is `{unit, kind: "points", t, v}` (its points in `[t0, t1]`), or `{unit, kind: "env", t, min, max}` (minimum and maximum per pixel column, when a column holds more than 3 points). `px` defaults to 1200, clamped to 16–8000. |
| `GET api/v1/datasets/{key}/vitals_at?t` | `[{name, unit, value, t}, …]`: the latest value of every vital sign at `t`, no older than 10 minutes. |
| `GET api/v1/datasets/{key}/markers?t0&t1&ch=` | `{names, items, truncated}`: the waveform markers in `[t0, t1]`, each `[channel, time, name index]`, at most 20,000 (`truncated: true` beyond them). |
| `GET api/v1/docs`, `GET api/v1/openapi.json` | Interactive documentation and the OpenAPI description (Python viewer only). |

A dataset's `key` is its index's [key](index-format.md#the-key). It is the same in every viewer
and in Medorama.

## Folder entries

Each entry of `browse` has `name`, `path`, `link` (whether it is a symbolic link) and a `type`:

| `type` | Entry | Further fields |
|---|---|---|
| `dir` | A folder. | |
| `wfdb` | A folder that holds WFDB records itself, or a WFDB collection. | |
| `record` | A WFDB record: its `.hea` file. `name` is given without `.hea`. | `kind` `"wfdb"`; `label` `"WFDB record"` or `"WFDB multi-segment record"` |
| `file` | A recording file read by a built-in reader other than WFDB, or by an installed adapter. | `kind` (`"edf"` for an EDF or BDF file, else the adapter's kind, such as `"stp"`); `label`, the format's label (`"EDF"`, `"EDF+"`, `"BDF"`, `"BDF+"`, or the adapter's); `size` in bytes |
| `broken` | A symbolic link whose target does not exist. | |

These entries are never listed:
- names that start with `.`, unless `hidden=true`;
- entries that lead outside the data roots (symbolic links among them), which are not completed
  either;
- signal files, and other files no built-in reader or installed adapter reads;
- the segments and layout headers of the multi-segment records in the folder (the segments are
  listed with `hidden=true`).

## Errors

Errors are JSON `{"detail": "…"}` with one of these statuses:

| Status | Meaning |
|---|---|
| `400` | A bad request: a host the server does not answer for, a missing, repeated or malformed query parameter, an unknown channel, a bad body, or a path that is neither a folder nor a recording the viewer reads. |
| `401` | Sign-in needed. |
| `403` | The path is not allowed: it lies outside the data roots, which lie inside the viewer's root (a path is checked after its symbolic links are resolved), or a file of the record does; or a request from another site. |
| `404` | Not found. |
| `405` | A known path asked with another method. |
| `413` | A request body over its limit. |
| `415` | A `POST` to the API whose body is not JSON. |
| `429` | A password sent before the delay after wrong ones has passed (the C++ viewer). |
| `422` | A folder over a limit, or without records. |
| `500` | An internal error (a failure of the server while answering). |

Every viewer answers a malformed query parameter with `400`, never `422`.
