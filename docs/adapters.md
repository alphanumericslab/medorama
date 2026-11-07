# Adapters: reading other recording formats

This page explains adapters: separate programs that let Medorama and the VitalStream viewers read
recording formats they do not read themselves. It is written for administrators who install an
add-on, and for developers who write an adapter for a new format.

Medorama and the VitalStream viewers read WFDB records and EDF, EDF+, BDF and BDF+ files
themselves ([wfdb-collection.md](wfdb-collection.md), [edf.md](edf.md)). Every other recording
format is read by an **adapter**: a program, installed beside them, that turns a recording into
the [index](../viewers/vitalstream/docs/index-format.md) the viewers draw from, and returns its
samples when a view zooms in. Formats that are licensed, or that only some institutions may
read, are distributed this way, as **add-ons**, outside this repository. How adapters fit among
the other readers is explained in [readers.md](readers.md).

## Contents

- [What an adapter is](#what-an-adapter-is)
- [Installing an add-on](#installing-an-add-on)
- [How the programs use adapters](#how-the-programs-use-adapters)
- [The protocol](#the-protocol)
- [Limits and environment](#limits-and-environment)
- [Writing an adapter](#writing-an-adapter)
- [Security](#security)

## What an adapter is

| Property | Rule |
|---|---|
| A program | An executable named `<name>-adapter` (`stp-adapter`; on Windows, `stp-adapter.exe`). |
| Where | An **adapter folder**: `~/.local/share/medorama/adapters` by default (Windows: `%LOCALAPPDATA%\medorama\adapters`). The setting `adapters_dir` of Medorama (`medorama.toml`) and of the viewers (`vitalstream.toml`, or `--adapters-dir`) names another. |
| What it reads | The formats it lists when asked (`describe`): each a recording kind, a label and file extensions. |
| What it writes | Only the index files, into the folder it is given; never anything next to a recording. |
| How it is called | By Medorama and by each viewer, as a child process, with the commands below. Nothing runs as a server or listens on a port. |

The **stp-adapter** (`.Stp` files of BedMasterEx: GE Unity and Carescape, Philips PIICiX and
Viridia) is such an add-on. It is private: it is licensed separately and released, as a prebuilt
program, to institutions under agreement.

Medorama and the viewers name no add-on's format. Until its adapter is installed, a file of that
format is like any other file they do not read: the file browsers do not list it, and opening it
is refused ("not a recording this program reads"). The built-in readers take precedence over an
adapter that lists the same extensions (`.hea`, `.edf`, `.bdf`).

## Installing an add-on

An add-on arrives as an archive for one system: `<name>-adapter-<version>-<os>-<arch>.tar.gz`
(Linux, macOS) or `.zip` (Windows), holding the program, its license and `INSTALL.txt`. An
administrator installs it on the server, never through the page, because an adapter is a program
that runs with the catalog's rights.

1. **Unpack it, and allow the system to run it.** Programs downloaded from the internet are
   blocked until you allow them. The following commands unpack the archive on Linux and macOS,
   and remove macOS's quarantine mark (otherwise Gatekeeper refuses to run the program):

   ```bash
   tar -xzf stp-adapter-0.1.0-linux-x86_64.tar.gz
   xattr -d com.apple.quarantine stp-adapter-0.1.0-*/stp-adapter   # macOS only
   ```

   On Windows, the following commands unpack the archive and unblock the program:

   ```powershell
   Expand-Archive stp-adapter-0.1.0-windows-amd64.zip .
   Unblock-File stp-adapter-0.1.0-windows-amd64\stp-adapter.exe
   ```

2. **Install it, as the account that runs the catalog** (or with `--home` naming the catalog), so
   that it goes into that catalog's adapter folder. The following commands install it and list
   the installed adapters (name, version, implementation, formats, program):

   ```bash
   medorama addons install stp-adapter-0.1.0-linux-x86_64/stp-adapter   # Windows: …\stp-adapter.exe
   medorama addons list
   ```

   `install` checks the program first: it must be named `<name>-adapter` (`.exe` on Windows),
   answer `describe`, and speak this protocol's version. An installed adapter of the same name is
   replaced. Installing and removing are written to the audit log (`addon_install`,
   `addon_remove`); `medorama addons remove stp` removes an adapter by name.

3. **Restart Medorama** (`systemctl restart medorama`, or stop and start `medorama serve`).
   Programs list their adapters when they start. Stopping Medorama also stops the VitalStream
   viewer it runs, so the next one sees the add-on too. **Admin** › **Add-ons** then lists it.

4. **Import again the tables that named such files before.** An import skips a recording that no
   installed reader reads and no viewer opens ("line N: FILE: not a recording Medorama reads. A
   format read by an add-on needs it installed …"), and keeps nothing of it. Once the add-on is
   installed, remove that earlier import (**Import** › **Sources**, or `medorama imports list`
   and then `medorama imports remove ID`) and import the table again: the files come in as
   recordings, linked to their patients and indexed by the workers. Importing the table again
   without removing the first import makes a second copy of the database.

**The viewers on their own** (without Medorama): copy the program into their adapter folder
(by default `~/.local/share/medorama/adapters`, on Windows `%LOCALAPPDATA%\medorama\adapters`, or
their `adapters_dir` setting), make it executable on Linux and macOS (`chmod 755`), unblock it as
in step 1, and restart the viewer.

| Problem | Cause and remedy |
|---|---|
| `install`: "is named <name>-adapter" | The file was renamed. Install the program as it came in the archive. |
| `install`: describe fails, "cannot be opened", "permission denied" | The system blocks the program. Repeat step 1 (`xattr` on macOS, `Unblock-File` on Windows), or run `chmod 755`. |
| `install`: another protocol version | The add-on was built for another Medorama version. Ask for the release that matches yours. |
| `addons list` shows it, but `.Stp` files are still refused | The server was not restarted (step 3), or it runs as another account whose `adapters_dir` is elsewhere: run `medorama addons list` as that account. |
| Recordings named by an earlier import are missing | They were skipped before the add-on was installed. Remove that import and import the table again (step 4). |

## How the programs use adapters

1. **At start**, each program lists its adapter folder and asks every `*-adapter` program to
   `describe` itself, once. When two adapters have the same name, the first in name order is
   used.
2. **Browsing:** a file whose extension an adapter lists is a recording of that adapter's kind.
3. **Indexing:** the program creates a temporary index folder and runs `index FILE FOLDER`,
   showing the progress lines. It then adds the fields it owns (`path`, `name`, `key`,
   `index_version`, `engine`, `indexed_s`, and `kind`, `format`, `adapter`) to the `meta.json`
   the adapter wrote, and renames the folder into place
   ([index-format.md, Writing and sharing](../viewers/vitalstream/docs/index-format.md#writing-and-sharing)).
4. **Viewing:** envelopes, markers and vital signs come from the index. Raw samples, when a view
   zooms in close enough, are read from the recording itself through the adapter's `serve`
   session: one session per adapter, started at the first request and kept open, and started
   again once if it has ended. The sessions of different adapters answer side by side; one
   adapter's requests are answered in turn.
5. **Identifiers** (Medorama only): the built-in action `identify` asks the adapter for the
   identifiers the file states (`identify FILE`), and keeps them in the catalog's database only
   ([administration.md, Identifiers in recordings](administration.md#identifiers-in-recordings)).

Indexes written through an adapter are shared like the others: the three viewers and Medorama
read and write the same index folders, and the three implementations of the stp-adapter write
the same bytes.

## The protocol

This is version 1 of the protocol. Every command writes its answer on standard output as JSON
(UTF-8, one object per line), and its errors on standard error, with a non-zero exit status.
Paths are given as arguments, as the operating system names them.

| Command | Answer |
|---|---|
| `describe` | One object: `{"name": "stp", "version": "0.1.0", "protocol": 1, "implementation": "rs", "formats": [{"kind": "stp", "label": "BedMasterEx .Stp", "extensions": ["stp"]}]}`. `implementation` is informative. |
| `detect PATH…` | One line per path: `{"path": …, "kind": "stp"}`, with `kind` null when the file is not of its formats. |
| `index PATH DIR` | Writes the index files of the recording at `PATH` into the existing, empty folder `DIR`; prints `{"progress": f}` lines (0 to 1, a few per second at most), then `{"done": true}`. |
| `identify PATH…` | One line per path: `{"path": …, "identifiers": {…} or null, "others": [{…}, …]}`, or `{"path": …, "error": "…"}`. A set of identifiers has the fields `full_name`, `last_name`, `first_name`, `patient_id`, `encounter_id`, `birth_date` (`YYYY-MM-DD`) and `sex`, each a string (empty when not stated). `identifiers` is the first set the file states; `others` lists the different sets found after it (at most 10). |
| `serve` | A session on standard input and output, until standard input closes. |
| `--version` | `<program> <version>` on one line. |

**The index an adapter writes** follows the [index format](../viewers/vitalstream/docs/index-format.md)
in everything but the fields the calling program adds (step 3 above): `meta.json` with
`channels`, `pieces`, `vitals`, `alarms`, `gaps`, `untimed`, `marker_names`, `skipped`, `notes`,
`time_resolution_s` and `reader`, and every array file. An adapter may add its own keys
(`vendor`, and `description`, a sentence shown with the recording), and keep in `reader` whatever
it needs to read samples later; the programs never interpret `reader`. An adapter that puts back
in order data that the file stored out of order lists those blocks in `late_blocks` (index
format): the viewers do not shade them as breaks, and mark them only with their `qc_verbose`
setting.

**The session (`serve`).** Each request is one line of JSON:

```json
{"op": "read", "index": "/path/to/the/index/folder", "piece": 3, "a": 1000, "b": 3500}
```

It asks for the physical samples `a` to `b - 1` of piece `piece` of the recording indexed in
`index`, whose `meta.json` holds the recording's `path`. The answer is one line, `{"n": k}`,
followed at once by `k` little-endian 64-bit floats (NaN for samples the recording marks
missing), or one line `{"error": "…"}` with nothing after it. An adapter answers requests in
order, may keep recordings open between them, and ends when standard input closes.

## Limits and environment

The calling program holds every adapter to these limits, so that a faulty or stuck adapter makes
one recording unreadable and never holds up the program. The Rust engine
([`libs/engine/src/adapters.rs`](../libs/engine/src/adapters.rs)) defines them; the C++ and
Python viewers apply the same.

| Command | Limit | When it is exceeded |
|---|---|---|
| `describe` | 30 s for the whole command; at most 16 MiB of output. | The adapter is stopped (killed) and not listed. |
| `identify` | 60 s per path of the command; at most 16 MiB of output. The paths are passed in commands whose arguments total at most 24,000 bytes (one path per command at least), far below the 32,767 characters of a Windows command line. | The adapter is stopped; the identifiers are not read. |
| `index` | 300 s without a line of output (a progress line resets the time), and 300 s for the adapter to exit once its output ends; lines of at most 1 MiB. A cancelled index stops the adapter within a tenth of a second, without waiting for its next line. | The adapter is stopped; the index fails, and nothing is left in the index folder. |
| `serve` | 60 s without a byte of an answer; an answer line of at most 1 MiB. The program asks for at most 1,048,576 samples (2^20) in one request and splits longer reads. The answer's `n` must be a whole number from 0 to `b - a`; fewer samples than asked mean that the piece ends there. | See below. |

**A session out of step is stopped.** After an answer that breaks the protocol (not a JSON
object, an `n` outside `0 … b - a`, bytes beyond the samples announced, the session ending
inside an answer, or no answer in time), the program stops the session and starts a new one at
the next request; the request fails. An answer `{"error": "…"}` keeps the session. A session
that has ended before a request is started again once for that request.

**Standard error** is read while the adapter runs, so an adapter that writes much there never
blocks; the program keeps its last 64 KiB and never prints them in its log. When `index` fails,
the error shown with the recording ends with the last non-blank line of the adapter's standard
error (at most 200 characters, control characters removed); when `describe` fails, the program's
log shows the same line. What `identify` and `serve` write there is not shown at all, since it
could carry identifiers.

**The environment** of an adapter is cleared, except for `PATH`, `HOME`, `USERPROFILE`, `TMP`,
`TEMP`, `TMPDIR`, `LANG`, the `LC_*` variables, and on Windows `SYSTEMROOT`, `SYSTEMDRIVE`,
`WINDIR`, `COMSPEC`, `PATHEXT`, `LOCALAPPDATA` and `APPDATA` (names compared ignoring case on
Windows). Medorama keeps secrets in its environment (the database password, its key); an
adapter never sees them. An adapter must not depend on any other variable: what it needs to know
comes in its arguments and requests.

## Writing an adapter

An adapter can be written in any language. The engines' building blocks make it short in the
three languages this repository uses:

| Language | Building blocks | Example |
|---|---|---|
| Rust | The crates `recording-engine` (`common`: channels, pieces, envelopes, vital-sign series; `index::write_arrays`) and `signal-core` (values, Python-compatible JSON) | The test adapter of [`libs/engine/tests/adapters.rs`](../libs/engine/tests/adapters.rs) |
| C++ | The C++ project's `vs_engine` and `vs_core` libraries (`engine/common.hpp`, `engine/index.hpp`: `write_arrays`). Add `viewers/vitalstream/cpp` to a CMake project with `VS_BUILD_VIEWER=OFF`. | |
| Python | `vitalstream_py.common` and `vitalstream_py.index.write_arrays` | [`libs/engine/tests/fake_adapter.py`](../libs/engine/tests/fake_adapter.py), the mock adapter every engine's tests use |

An adapter must meet these rules:

- It writes only into the folder that `index` is given, and nothing at all for the other
  commands.
- It never prints identifiers except in answer to `identify` (on standard output; never on
  standard error), and never writes them into the index.
- It answers within the [limits](#limits-and-environment).
- Times are Unix seconds (UTC), as in the index format.
- The same recording gives the same index bytes every time: no time stamps of the indexing, no
  absolute paths of temporary files.
- It is tested with the viewers: `viewers/vitalstream/tests/parity/index_parity.py --adapters-dir
  DIR FILE…` indexes a recording with every built viewer through the adapter and checks that
  the results are byte-identical.

When to write an adapter rather than a built-in reader is discussed in
[readers.md, Recordings](readers.md#recordings-a-built-in-reader-or-an-adapter).

## Security

- **An adapter is code that runs with the rights of the program that calls it** (the catalog's
  service account, or a viewer's user). Install only adapters from their publisher, and keep the
  adapter folder writable by its owner only (the default folder is in that user's home).
- **It reads recordings, which may hold identifiers.** It must not send anything over the
  network, and it writes only into the index folder it is given. The stp-adapter opens no
  network connection and writes no other file.
- **Identifiers it reads** (`identify`) go only to Medorama's database, as other identifiers do
  ([security.md, Privacy Mode and the privacy gate](security.md#7-privacy-mode-and-the-privacy-gate));
  the index holds none.
- **A failing adapter** (a crash, a malformed answer) makes the recording unreadable, never the
  program: the indexing fails with the adapter's message, and a session that ended is started
  again once. An adapter that stops answering is stopped after a time limit
  ([Limits and environment](#limits-and-environment)).
- **It gets none of the program's secrets:** its environment is cleared except for a short
  allow-list, and its standard error never reaches the program's log.
