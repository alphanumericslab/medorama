# Building

This page explains how to build every program in this repository from source, on Linux, macOS
and Windows, with or without administrator rights or internet access. It is for administrators
who install Medorama and for contributors. For a first installation with the defaults, see
[Install](install.md), which gives the commands for each operating system, and
[getting-started.md](getting-started.md); to set up and run Medorama once it is built, see the
[administrator's guide](administration.md).

## Contents

- [What there is to build](#what-there-is-to-build)
- [Install with the installers](#install-with-the-installers)
- [Prerequisites](#prerequisites)
- [The Rust workspace](#the-rust-workspace)
- [The C++ viewer](#the-c-viewer)
- [The Python packages](#the-python-packages)
- [Without WFDB](#without-wfdb)
- [Without administrator rights](#without-administrator-rights)
- [Without internet access](#without-internet-access)
- [Check the build](#check-the-build)
- [Update and uninstall](#update-and-uninstall)
- [Troubleshooting](#troubleshooting)

## What there is to build

| Program | Language | Built from | Result |
|---|---|---|---|
| `medorama` | Rust | The workspace (`cargo build -p medorama`) | One binary of about 18 MB, which holds the database engine, the web page, and the ECG and PACS viewers. |
| `vitalstream-rs` | Rust | The workspace (`-p vitalstream-rs`) | One binary, which holds its web page. |
| `vitalstream-cpp` | C++20 | [`viewers/vitalstream/cpp`](../viewers/vitalstream/cpp) (CMake) | One binary, which holds its web page. |
| `vitalstream-py` | Python | [`viewers/vitalstream/python/vitalstream`](../viewers/vitalstream/python/vitalstream) | A Python package. |
| `muse-py`, `dicom-py` | Python | [`viewers/ecg/python/muse`](../viewers/ecg/python/muse), [`viewers/pacs/python/dicom`](../viewers/pacs/python/dicom) | The viewers' Python tools, which build databases to import and serve the ECG and PACS viewers on their own. |

WFDB records are read with [wfdb-x](https://github.com/alphanumericslab/wfdb-x) in each
language. Cargo, CMake and pip fetch it from GitHub at one pinned commit (`4a91916`), like any
other dependency. EDF, EDF+, BDF and BDF+ files are read by code in this repository and need no
further dependency. Every program can also be built without WFDB support, and then contains no
wfdb-x code ([Without WFDB](#without-wfdb)).

The ECG and PACS viewers are static pages, so there is nothing to build for them. Add-ons
(adapters) are built in their own repositories ([adapters.md](adapters.md)).

Every Rust program is a member of one Cargo workspace (`Cargo.toml` at the root of the
repository), which shares one `Cargo.lock` and one `target/` folder. Each program builds and
deploys on its own.

## Install with the installers

The installers build the programs and install them inside the clone. They need no administrator
rights, except on some systems for the compiler ([Prerequisites](#prerequisites)).

| Installer | What it builds |
|---|---|
| `tools/install-medorama.sh [--viewer cpp\|rs\|py\|none] [--no-tools] [--no-rustup] [--demo] [--no-wfdb]` (Windows: `tools\install-medorama.ps1 [-Viewer …] [-NoTools] [-NoRustup] [-Demo] [-NoWfdb]`) | Medorama, **one** VitalStream viewer (by default `cpp`) and the database tools, into `.venv/`. The viewer it builds is the one Medorama runs. |
| `viewers/vitalstream/install.sh [--viewer cpp\|rs\|py] [--prefix DIR] [--root DIR] [--no-password] [--no-wfdb] [--no-rustup]` (Windows: `install.ps1 [-Viewer …] [-Prefix …] [-Root …] [-NoPassword] [-NoWfdb]`) | VitalStream alone, without Medorama, into `~/.local/bin` (Windows: `%LOCALAPPDATA%\vitalstream\bin`), with a root folder that it never leaves (by default the home folder). |
| `tools/install.sh [--py] [--rs] [--cpp] [--no-rustup] [--demo]` (Windows: `tools\install.ps1 [-Py] [-Rs] [-Cpp] [-NoRustup] [-Demo]`) | Everything, for development: the Python packages, the Rust workspace, the C++ viewer and its tests, and the git hooks. |

| Option | Installers | Effect |
|---|---|---|
| `--viewer cpp\|rs\|py\|none` (`-Viewer`) | `install-medorama` | The viewer Medorama runs. The default, `cpp`, needs a C++20 compiler; `rs` needs only Rust. |
| `--no-tools` (`-NoTools`) | `install-medorama` | Leaves out the database tools (`muse-py`, `dicom-py`). |
| `--py`, `--rs`, `--cpp` (`-Py`, `-Rs`, `-Cpp`) | `install` | The Python packages are always installed. `--py` installs them alone, `--rs` adds the Rust workspace, `--cpp` the C++ viewer, and `--rs --cpp` both. Without any of these options, all three are installed. |
| `--no-rustup` (`-NoRustup`) | `install-medorama.sh` and `.ps1`, `install.sh` and `.ps1`, VitalStream's `install.sh` | Fails instead of installing Rust when `cargo` is missing. VitalStream's `install.ps1` has no `-NoRustup`: it never installs Rust, and `-Viewer rs` fails when `cargo` is missing. |
| `--root DIR` (`-Root`) | VitalStream's `install` | The viewer's root folder, fixed in the program: the viewer never opens anything outside it. The default is `~`, the home folder of whoever runs the viewer; `/` allows the whole file system. Medorama's installer gives its viewer the whole file system, because Medorama's own rules decide what each user may open. |
| `--no-password` (`-NoPassword`) | VitalStream's `install` | Builds the viewer without password support: no password can be set, and the viewer refuses to run where one is required, as under Medorama. By default the viewer is built with password support, and a password is optional. |
| `--demo` (`-Demo`) | `install-medorama`, `install` | Also makes **the demo**, invented patients that `medorama init` loads into a cohort named DEMO. It is **off by default** ([administrator's guide, The demo](administration.md#the-demo)). |
| `--no-wfdb` (`-NoWfdb`) | `install-medorama`, VitalStream's `install` | Builds the programs without WFDB support, so that they contain no wfdb-x code: a WFDB recording is refused, and every other format is read ([Without WFDB](#without-wfdb)). `install-medorama` builds both Medorama and its viewer this way, and refuses `--demo` with it, because the demo's recordings are WFDB records. |

To clone the repository, run the installer, and check the result:

```bash
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
tools/install-medorama.sh
source .venv/bin/activate              # in every new terminal (Windows: .venv\Scripts\Activate.ps1)
medorama --version
```

The programs go into `.venv/bin` (Windows: `.venv\Scripts`), which is not on the `PATH` by
default. Activate the environment in each new terminal, or call `.venv/bin/medorama` directly.

### The build options of a standalone viewer

A viewer installed on its own has two options that are fixed when it is built: its root folder,
and whether it supports a password. The installers set them; a build by hand sets them as
follows. (WFDB support is a third build option, for every program: [Without WFDB](#without-wfdb).)

| Program | Root folder (`~` is allowed and means the home folder of whoever runs it; unset means the whole file system) | Without password support |
|---|---|---|
| `vitalstream-rs` | `VITALSTREAM_ROOT=DIR cargo build --release -p vitalstream-rs --target-dir target/standalone` | `--no-default-features --features wfdb` (the Cargo feature `password` left out, `wfdb` kept) |
| `vitalstream-cpp` | `cmake --preset viewer -B build/standalone -DVS_ROOT=DIR` | `-DVS_PASSWORD=OFF` |
| `vitalstream-py` | `VITALSTREAM_ROOT=DIR pip install './viewers/vitalstream/python/vitalstream[wfdb]'` | `VITALSTREAM_PASSWORD=0` |
| `muse-py serve`, `dicom-py serve` (the ECG and PACS viewers, served) | `VIEWER_ROOT=DIR pip install ./viewers/ecg/python/muse` (or `…/pacs/python/dicom`) | `VIEWER_PASSWORD=0` |

At run time, the settings (`data_roots`, `--root`) can narrow the root further, but never widen
it. The standalone installer builds in folders of its own (`target/standalone`,
`viewers/vitalstream/cpp/build/standalone`), so that the viewer built for Medorama
(`target/release`, `build/viewer`) keeps its own options.

## Prerequisites

| Requirement | Linux | macOS | Windows 10 and 11 |
|---|---|---|---|
| Git, Python 3.9 or later | `sudo apt install git python3 python3-venv` (Debian, Ubuntu) or `sudo dnf install git python3` (Fedora, RHEL) | `xcode-select --install` (the Command Line Tools: Git, Python and compilers) | `winget install Git.Git` and `winget install Python.Python.3.12` |
| A C compiler, for Rust (SQLite is compiled in) | `sudo apt install build-essential` or `sudo dnf install gcc` | The Command Line Tools | The Microsoft C++ Build Tools: `winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` |
| A C++20 compiler, for the C++ viewer | GCC 12 or later, or Clang 15 or later (`sudo apt install g++` on Debian 12 and Ubuntu 22.04 or later) | Apple Clang 15 or later (the Command Line Tools) | Visual Studio 2022 (the Build Tools above) |
| Rust 1.88 or later | [rustup](https://rustup.rs), in your user account (the installers install it when it is missing) | The same | The same |
| CMake 3.24 or later and Ninja, for the C++ viewer | `pip install "cmake>=3.24" ninja` (the installers do this, into `.venv`) | The same | The same |

## The Rust workspace

To install Rust in your user account, run the line for your system, then open a new shell and
check the version:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal   # Linux, macOS
winget install Rustlang.Rustup                                                             # Windows
rustc --version                                                                            # 1.88 or later
```

Rust 1.88 is the oldest version that builds the workspace with the versions of its `Cargo.lock`
(`rust-version` in `Cargo.toml`; CI checks it). Development and CI use the current stable Rust;
the released programs are built with a pinned version ([developing.md](developing.md#releases)).

To build, run these commands from the root of the clone. The first builds every program, the
second one program, and the `cargo install` lines optionally copy the programs into
`~/.cargo/bin`, which is on the `PATH`:

```bash
cargo build --release --locked                       # medorama, vitalstream-rs -> target/release/
cargo build --release --locked -p medorama           # one program
cargo install --path medorama --locked               # optional: into ~/.cargo/bin (on the PATH)
cargo install --path viewers/vitalstream/rust --locked
```

**For an older Linux server.** A binary built on a recent distribution needs a C library at least
as recent. A fully static build runs on any Linux of the same architecture:

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools                          # Fedora: sudo dnf install musl-gcc
cargo build --release --locked --target x86_64-unknown-linux-musl -p medorama
# -> target/x86_64-unknown-linux-musl/release/medorama
```

**Database encryption.** This build encrypts the catalog's SQLite database with SQLCipher. It
builds OpenSSL from source, and so needs Perl and `make`:

```bash
cargo build --release --locked -p medorama --features encryption
```

**macOS.** If the build stops with `linking with cc failed: exit status: 69` or "You have not
agreed to the Xcode license agreements", Xcode is installed but its license was never accepted.
Run `sudo xcodebuild -license accept`, or build with the Command Line Tools:
`DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo build --release --locked`.

**Windows.** The workspace builds in an ordinary PowerShell window once the Build Tools are
installed. The programs are `target\release\*.exe`.

## The C++ viewer

These commands, run from `viewers/vitalstream/cpp` with a C++20 compiler, CMake 3.24 or later
and Ninja, build the viewer, run its tests, and optionally install it:

```bash
cmake --preset release && cmake --build --preset release     # -> build/release/bin/vitalstream-cpp
ctest --preset release                                       # the tests
cmake --install build/release --prefix ~/.local              # optional: ~/.local/bin/vitalstream-cpp
```

| Preset | What it builds | Folder |
|---|---|---|
| `release` | The viewer and its tests. | `build/release` |
| `debug` | The same, unoptimized, with debug information. | `build/debug` |
| `viewer` | The viewer only, as the installers use it. | `build/viewer` |
| `nowfdb` | The viewer and its tests without WFDB support (`VS_WFDB=OFF`; [Without WFDB](#without-wfdb)). | `build/nowfdb` |

| Option (`-D…` when configuring) | Default | Effect |
|---|---|---|
| `VS_BUILD_VIEWER` | `ON` | Builds the viewer. With `OFF`, only the libraries `vs_core` and `vs_engine` are built, for adapters written in C++. |
| `VS_BUILD_TESTS` | `ON` | Builds the unit tests (GoogleTest). |
| `VS_ROOT` | Empty (the whole file system) | The viewer's root folder ([The build options of a standalone viewer](#the-build-options-of-a-standalone-viewer)). |
| `VS_PASSWORD` | `ON` | Builds the viewer with password support. |
| `VS_WFDB` | `ON` | Builds the viewer with WFDB support (wfdb-x). With `OFF`, wfdb-x is neither downloaded nor built in, and a WFDB recording is refused ([Without WFDB](#without-wfdb)). |
| `VS_DEPS_DIR` | `cpp/deps` | The folder of pre-downloaded dependency archives ([Without internet access](#without-internet-access)). |

The first configuration downloads five pinned archives (wfdb-x, CLI11, toml++ and cpp-httplib,
and GoogleTest for the tests; four with `VS_WFDB=OFF`), and checks each against its SHA-256
([`cmake/deps.cmake`](../viewers/vitalstream/cpp/cmake/deps.cmake)). Without Ninja, configure
with `cmake -S . -B build/make -G "Unix Makefiles" -DCMAKE_BUILD_TYPE=Release` and build with
`cmake --build build/make`. On Windows, use `cmake -S . -B build/vs -G "Visual Studio 17 2022"`
and `cmake --build build/vs --config Release`.

**Linux.** `g++ --version` must report 12 or later. On older systems, install a newer GCC
(`sudo apt install g++-12`; on RHEL 8, `sudo dnf install gcc-toolset-12`) and select it with
`export CC=gcc-12 CXX=g++-12` (on RHEL 8, `scl enable gcc-toolset-12 bash`), or use Clang 15 or
later (`export CC=clang-15 CXX=clang++-15`). Without administrator rights, see
[below](#without-administrator-rights). To run a program built against a newer C++ runtime on
other machines, link the runtime in with
`-DCMAKE_EXE_LINKER_FLAGS="-static-libstdc++ -static-libgcc"`.

**macOS.** Use Apple Clang 15 or later from the Command Line Tools (`xcode-select --install`,
once, with an administrator's password). With GCC from Homebrew (`CC=gcc-14 CXX=g++-14`), add
`-DVS_BUILD_TESTS=OFF`, because GoogleTest does not compile with GCC against the macOS 26 SDK. If
a standard header such as `cstddef` is not found, the Command Line Tools are damaged: run
`sudo rm -rf /Library/Developer/CommandLineTools && xcode-select --install`. Until then, the
build detects this case and uses the SDK's headers.

**Windows.** Build in a **Developer PowerShell for VS 2022** (in the Start menu), which sets up
the compiler:

```powershell
cd viewers\vitalstream\cpp
cmake --preset release; cmake --build --preset release      # -> build\release\bin\vitalstream-cpp.exe
```

In an ordinary PowerShell window, `cmake --preset release` stops with "No CMAKE_CXX_COMPILER
could be found"; `tools\install.ps1 -Cpp` sets the compiler up by itself. Visual Studio can also
open the `cpp` folder directly, because it reads `CMakePresets.json`.

## The Python packages

These commands create a virtual environment and install the Python viewer and the viewers'
Python tools into it:

```bash
python3 -m venv .venv
.venv/bin/pip install './viewers/vitalstream/python/vitalstream[wfdb]'   # vitalstream-py
.venv/bin/pip install ./viewers/ecg/python/muse ./viewers/pacs/python/dicom   # muse-py, dicom-py (with `serve`: the ECG and PACS viewers, served)
```

The `[wfdb]` extra installs wfdb-x from GitHub, from the archive of the pinned commit, so `git`
is not needed. Without the extra, `vitalstream-py` has no WFDB support
([Without WFDB](#without-wfdb)); the installers always add it, except with `--no-wfdb`.

Each package carries the licence: building it copies the repository's `LICENSE` (with its
Required Notice) and `COMMERCIAL.md` beside its `pyproject.toml`, and they are installed with the
package (in its `.dist-info/licenses/`). A package builds from a clone, or from its source
distribution: `python -m build viewers/vitalstream/python/vitalstream` (and
`viewers/ecg/python/muse`, `viewers/pacs/python/dicom`) writes an sdist that carries the
viewer's page and the licence, and a wheel built from that sdist alone. Building needs
setuptools 77 or later, which pip fetches into the isolated build environment.

For development, install the packages in editable mode
(`pip install -e … --config-settings editable_mode=compat`) with their `[test]` extras (and
`[test,wfdb]` for `vitalstream-py`), as `tools/install.sh` does.

## Without WFDB

WFDB support is the only part of the programs that uses wfdb-x. Each program can be built without
it, and such a build contains no wfdb-x code. It reads every other format as usual: EDF, EDF+,
BDF and BDF+ files, the formats of installed adapters (such as STP, GE MUSE and DICOM), CLIF
tables, and the databases Medorama imports or connects to. A WFDB recording (a `.hea` file, a
multi-segment record, a folder of records or a WFDB collection) is still recognised, and refused
with one message, never skipped in silence:

```text
this build has no WFDB support (built without the `wfdb` feature)
```

The C++ viewer ends the message with `(built with VS_WFDB=OFF)`, and the Python viewer with
`(wfdb-x is not installed: pip install 'vitalstream_py[wfdb]')`. Medorama does not add such a
recording: an import lists it among the files it skipped, with this reason. A viewer lists it
when browsing, and refuses to open or scan it (HTTP 422, with the message). At start-up, every
program built this way says so.

| Program | Built without WFDB support | The default |
|---|---|---|
| `medorama` | `cargo build --release --locked -p medorama --no-default-features` (add `--features encryption` for database encryption) | The Cargo feature `wfdb`, on |
| `vitalstream-rs` | `cargo build --release --locked -p vitalstream-rs --no-default-features --features password` (leave out `--features password` for no password support either) | The Cargo features `password` and `wfdb`, on |
| `recording-engine` (the library, for adapters written in Rust) | `recording-engine = { …, default-features = false }` in the adapter's `Cargo.toml` | The Cargo feature `wfdb`, on |
| `vitalstream-cpp` | From `viewers/vitalstream/cpp`: `cmake --preset nowfdb && cmake --build --preset nowfdb` (into `build/nowfdb`), or `-DVS_WFDB=OFF` with any preset | `VS_WFDB=ON` |
| `vitalstream-py` | `pip install ./viewers/vitalstream/python/vitalstream`, without the `[wfdb]` extra, in an environment that does not hold wfdb-x (`pip uninstall wfdb-x` removes it) | The installers install the `[wfdb]` extra |

The installers do the same with one option: `tools/install-medorama.sh --no-wfdb` (Windows:
`-NoWfdb`) builds both Medorama and its viewer this way, and `viewers/vitalstream/install.sh
--no-wfdb` one viewer alone. Medorama and its viewer are built separately, so a deployment
without wfdb-x builds both without WFDB support. The demo (`--demo`) needs WFDB support, because
its recordings are WFDB records written with wfdb-x.

To check a build, `cargo tree --locked -p medorama --no-default-features | grep wfdbx` prints
nothing (likewise for `vitalstream-rs`), a C++ build in `build/nowfdb` has no `_deps/wfdbx-src`
folder, and `make test-nowfdb` runs the tests of these builds (the Python tests with wfdb-x made
unimportable). The tests on WFDB records are skipped there, and a test of each program checks
the refusal.

Two parts read WFDB headers without wfdb-x, so they work in either build: the ECG viewer, which
shows a WFDB record attached to an item with its own reader in the page, and Privacy Mode, which
de-identifies the text of a WFDB header ([protecting-patient-data.md](protecting-patient-data.md)).
In the C++ libraries, `core/wfdb_c.hpp` (WFDB headers, records and annotations, for adapters
written in C++) exists only with `VS_WFDB=ON`.

## Without administrator rights

Only the compiler can need an administrator. Rust, CMake, Ninja, the Python packages and every
program built here install in your own account.

**Linux (a shared server or a cluster).** If `g++ --version` reports 12 or later, nothing more is
needed. Otherwise, use the first of these that your machine offers:

| Way | How to check | If it is available |
|---|---|---|
| A compiler module | `module avail 2>&1 \| grep -i gcc` lists GCC 12 or later. | `module load <that name>`, then `export CC=gcc CXX=g++`. |
| gcc-toolset (RHEL, Rocky and Alma 8 and 9) | `ls /opt/rh` lists `gcc-toolset-12` or later. | `scl enable gcc-toolset-13 bash` |
| GCC 13 in your home folder (anywhere) | | `tools/install-gcc.sh`, then `source ~/miniforge3/bin/activate vs-build` in every new shell. |

`tools/install-gcc.sh` installs Miniforge (conda-forge) into `~/miniforge3`, with an environment
named `vs-build` that holds GCC 13, CMake, Ninja and Python 3.12 (about 1 GB). The installers use
the compiler in `CC` and `CXX` when these are set. All three ways apply to the current shell only:
repeat the `module load`, `scl` or `source` line in every new shell, or add it to `~/.bashrc`.
CMake remembers the compiler that a build folder was first configured with, so after switching
compilers, delete the build folder (the installers do this).

**CMake and Ninja in your account.** Run `python3 -m pip install --user "cmake>=3.24" ninja`, or
unpack the official archives from <https://github.com/Kitware/CMake/releases> and
<https://github.com/ninja-build/ninja/releases> into `~/.local`.

**Windows.** For Rust, the self-contained GNU toolchain needs no Visual Studio:
`rustup-init.exe --default-host x86_64-pc-windows-gnu` (SQLite then needs GCC from MSYS2). For
C++, [MSYS2](https://www.msys2.org), unpacked into your user folder, provides GCC, CMake and
Ninja. These commands download and unpack it, update it, and install the tools:

```powershell
Invoke-WebRequest https://github.com/msys2/msys2-installer/releases/download/nightly-x86_64/msys2-base-x86_64-latest.sfx.exe -OutFile msys2.exe
.\msys2.exe -y -o"$env:USERPROFILE"; Remove-Item msys2.exe
& "$env:USERPROFILE\msys64\usr\bin\bash.exe" -lc "pacman -Syu --noconfirm"
& "$env:USERPROFILE\msys64\usr\bin\bash.exe" -lc "pacman -S --noconfirm --needed git mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-ninja"
```

Then, in the UCRT64 shell, build with `cmake --preset release -DCMAKE_EXE_LINKER_FLAGS=-static &&
cmake --build --preset release`. The `-static` flag makes a program that needs no MSYS2 DLLs. CI
builds the C++ viewer this way (MSYS2); the GNU Rust toolchain is not tested in CI.

**macOS.** The Command Line Tools need an administrator once.

## Without internet access

**Copy the programs.** Build on a machine with the same operating system and architecture (on
Linux, preferably the static musl build) and copy the single program files. Nothing else is
needed to run them.

**Build on the offline machine.** First, on a machine with internet access, run these commands
from the root of the clone. They put every Rust dependency into `vendor/`, the C++ dependencies
into `viewers/vitalstream/cpp/deps/`, and, if needed, the Python viewer's dependencies into
`../wheels`:

```bash
mkdir -p .cargo && cargo vendor --locked vendor > .cargo/config.toml
tools/fetch-cpp-deps.sh
pip download -d ../wheels './viewers/vitalstream/python/vitalstream[wfdb]' "setuptools>=77" wheel
```

`cargo vendor` includes wfdb-x, and `fetch-cpp-deps.sh` its C++ archive. Copy the whole clone,
and `../wheels`, to the offline machine, which needs Rust and a compiler installed (rustup's
offline installers are listed at
<https://forge.rust-lang.org/infra/other-installation-methods.html>). Then build as usual, with
`cargo build --release --locked --offline` and `cmake --preset release`. For the Python viewer,
these commands install wfdb-x from the archive that `pip download` saved (named after the
commit), and then the viewer:

```bash
pip install --no-index --find-links ../wheels fastapi uvicorn numpy \
    "wfdb-x @ file://$PWD/../wheels/4a91916f5282fba44ca3056802fbc7def298af08.tar.gz#subdirectory=python"
pip install --no-index --find-links ../wheels --no-deps ./viewers/vitalstream/python/vitalstream
```

Git ignores `vendor/`, `.cargo/` and `viewers/vitalstream/cpp/deps/`.

**The demo, offline.** The installers' `--demo` option needs NumPy and wfdb-x in `.venv`, and
`pip download` above saved both in `../wheels`. These commands install them from there, then make
and load the demo by hand
([administrator's guide, The demo](administration.md#load-it-later-or-into-another-catalog)):

```bash
.venv/bin/pip install --no-index --find-links ../wheels numpy \
    "wfdb-x @ file://$PWD/../wheels/4a91916f5282fba44ca3056802fbc7def298af08.tar.gz#subdirectory=python"
.venv/bin/python tools/make-demo-data.py ~/medorama-demo
medorama demo add ~/medorama-demo
```

## Check the build

These commands run the tests of each part, check that the three VitalStream viewers agree, and
list every other check:

```bash
cargo test --release --locked                    # every Rust test of the workspace (Medorama's take a few minutes)
(cd viewers/vitalstream/cpp && ctest --preset release)
.venv/bin/python -m pytest -q viewers/vitalstream/python/vitalstream/tests
make parity                                      # the three viewers agree (index and answers)
make test-nowfdb                                 # the builds without WFDB support (Without WFDB)
make help                                        # every check
```

[developing.md](developing.md#tests) describes the tests.

## Update and uninstall

**Update.** Run `git pull`, then the installer again with the same options (or
`cargo build --release --locked` and `cmake --build --preset release`). Without `--demo`,
catalogs created afterwards start empty; a catalog that already holds the demo keeps it. Restart
the programs. Medorama updates its database when it starts, so back the database up first
([administrator's guide](administration.md#backup-update-uninstall)).

**Uninstall.** Delete the clone (the programs, `.venv/` and the builds are inside it), or the
installed copies (`cargo uninstall medorama`, `~/.local/bin/vitalstream-*`). A standalone
viewer's index folder is `~/.cache/vitalstream`, and its settings and password are in
`~/.config/vitalstream/`. Medorama's home and results folders are where `init` put them.
Recordings are never touched.

## Troubleshooting

| Message | Cause and remedy |
|---|---|
| `command not found: medorama` | The program is in `.venv/bin`. Run `source .venv/bin/activate`, call `.venv/bin/medorama`, or run `cargo install --path medorama --locked`. |
| `linker 'cc' not found` (Linux) | No C toolchain is installed. Install `build-essential` or `gcc` ([Prerequisites](#prerequisites)), or see [Without administrator rights](#without-administrator-rights). |
| `linker 'link.exe' not found`, `No CMAKE_CXX_COMPILER could be found` (Windows) | The Build Tools are missing, or lack **Desktop development with C++**. For C++, use a Developer PowerShell. |
| `exit status: 69`, `You have not agreed to the Xcode license agreements` (macOS) | Run `sudo xcodebuild -license accept`, or `export DEVELOPER_DIR=/Library/Developer/CommandLineTools`. |
| `feature 'edition2024' is required`, `requires rustc 1.88` | Rust is too old. Run `rustup update`; a distribution's packaged Rust is often old. |
| `cargo: command not found` after installing Rust | Open a new shell, or run `. "$HOME/.cargo/env"`. |
| `failed to get … from crates.io` | There is no internet access. [Vendor the dependencies](#without-internet-access). |
| `The C++ compiler is too old` | GCC 12 or Clang 15 or later is needed ([The C++ viewer](#the-c-viewer), [Without administrator rights](#without-administrator-rights)). Then delete the build folder. |
| `CXX is …, but this build folder was configured with …` | CMake keeps the first compiler. Run `rm -rf build/release` and configure again. |
| `The current CMakeCache.txt directory … is different` | The clone moved after a build. Delete `build/` and configure again. |
| `CMake 3.24 or higher is required`, no `Ninja` | Run `pip install --user "cmake>=3.24" ninja`. |
| `Failed to download`, `HASH mismatch` while configuring | There is no internet access, or a proxy interferes. Run `tools/fetch-cpp-deps.sh` on another machine ([Without internet access](#without-internet-access)). |
| `version 'GLIBC_2.xx' not found` or `'GLIBCXX_3.4.xx' not found` for a copied program | It was built on a newer Linux. Use the static musl build (Rust), or link the C++ runtime statically (C++). |
| "running scripts is disabled on this system" (Windows) | Start the installers as shown (`powershell -ExecutionPolicy Bypass -File …`). For `Activate.ps1`, run `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`. |
