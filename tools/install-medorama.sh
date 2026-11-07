#!/usr/bin/env bash
# Medorama with ONE VitalStream waveform viewer, nothing else (macOS or Linux, no administrator rights).
#
#   tools/install-medorama.sh                  Medorama + the C++ viewer (vitalstream-cpp)
#   tools/install-medorama.sh --viewer rs      Medorama + the Rust viewer (vitalstream-rs)
#   tools/install-medorama.sh --viewer py      Medorama + the Python viewer (vitalstream-py)
#   tools/install-medorama.sh --viewer none    Medorama alone
# Other options:
#   --demo         also make the demo: invented patients that `medorama init` loads into a cohort
#                  DEMO, for trying Medorama (off by default; docs/administration.md, "The demo")
#   --no-rustup    fail rather than install Rust if cargo is missing
#   --no-tools     leave out the database tools (below)
#   --no-wfdb      build Medorama and the viewer without WFDB support: no wfdb-x code in them (a
#                  WFDB recording is refused, not added; every other format is read; not with
#                  --demo, whose recordings are WFDB records; docs/building.md, "Without WFDB")
#
# Medorama (medorama/, Rust) always; the viewer you choose; the database tools, which build
# databases to import and the demo's DICOM examples: muse-py (GE MUSE ECGs) and dicom-py (DICOM
# studies, with pydicom), into .venv.  Nothing else (the other viewers, the libraries' Python
# packages).  The programs go into .venv/bin, where Medorama finds the viewer; the choice is
# recorded in .venv/bin/medorama.viewer (docs/administration.md, "The waveform viewer"):
#   cpp   CMake and Ninja into .venv, a C++20 compiler (GCC 12+ / Clang 15+), viewers/vitalstream/cpp/build/viewer
#   rs    the Rust viewer, target/release (the same toolchain as Medorama)
#   py    the Python viewer and its packages into .venv
# Compilers: CC and CXX are used when set (`module load gcc`, a conda environment).
# Licensed recording formats need their add-on, installed afterwards with
# `medorama addons install PROGRAM` (docs/adapters.md).
# For every implementation and the tests, use tools/install.sh instead.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
VIEWER=cpp RUSTUP=1 TOOLS=1 DEMO=0 WFDB=1
while [ $# -gt 0 ]; do
  case "$1" in
    --viewer) VIEWER="${2:?--viewer needs a value: cpp, rs, py or none}"; shift 2 ;;
    --viewer=*) VIEWER="${1#--viewer=}"; shift ;;
    --no-rustup) RUSTUP=0; shift ;;
    --no-tools) TOOLS=0; shift ;;
    --demo) DEMO=1; shift ;;
    --no-wfdb) WFDB=0; shift ;;
    -h|--help) sed -n '2,/^# For every/p' "$0"; exit 0 ;;
    *) echo "unknown option $1 (see --help)"; exit 2 ;;
  esac
done
case "$VIEWER" in
  cpp|rs|py|none) ;;
  *) echo "--viewer: cpp, rs, py or none (not '$VIEWER')"; exit 2 ;;
esac
if [ "$WFDB" = 0 ] && [ "$DEMO" = 1 ]; then
  echo "--demo needs WFDB support (the demo's recordings are WFDB records): leave out --no-wfdb or --demo"; exit 2
fi
step() { printf '\n==> %s\n' "$*"; }

# macOS: Xcode installed but its license not accepted makes cc fail: use the Command Line Tools.
if [ "$(uname)" = Darwin ] && [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Library/Developer/CommandLineTools ] \
   && ! cc --version >/dev/null 2>&1; then
  export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
need_compiler() {  # $1: the compiler command
  if ! "$1" --version >/dev/null 2>&1; then
    echo "The C/C++ compiler ($1) is missing."
    if [ "$(uname)" = Darwin ]; then
      echo "Run 'xcode-select --install' (Command Line Tools; asks for an administrator once), then rerun."
    else
      echo "With administrator rights: sudo apt install build-essential (Debian, Ubuntu) or"
      echo "sudo dnf install gcc gcc-c++ (Fedora, RHEL). Without: load a compiler module"
      echo "(module load gcc), or install GCC 13 in your home folder with tools/install-gcc.sh."
    fi
    exit 1
  fi
}

# --- .venv: where the programs go (and, for some viewers, their tools) ---------------------
PY=${PYTHON:-python3}
command -v "$PY" >/dev/null || { echo "python3 not found: install Python 3.9 or later"; exit 1; }
"$PY" -c 'import sys; sys.exit(sys.version_info < (3, 9))' || { echo "Python 3.9 or later is needed ($("$PY" -V))"; exit 1; }
step "Environment .venv (the programs are linked into .venv/bin)"
[ -d .venv ] || "$PY" -m venv .venv
.venv/bin/python -m pip install --quiet --upgrade pip
COMMANDS=""

# --- Rust (Medorama; the Rust viewer) ---------------------------------------------------
export PATH="$HOME/.cargo/bin:$PATH"
# the builds in this clone's target/, where the links below point (not CARGO_TARGET_DIR or a
# build.target-dir set elsewhere)
export CARGO_TARGET_DIR="$ROOT/target"
if ! command -v cargo >/dev/null; then
  if [ "$RUSTUP" = 0 ]; then
    echo "cargo not found and --no-rustup given: install Rust (https://rustup.rs)"
    exit 1
  fi
  step "Installing Rust in your user account (rustup; ~/.rustup and ~/.cargo)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
fi
need_compiler "${CC:-cc}"

step "Building Medorama (medorama)"
cargo build --release --locked -p medorama $([ "$WFDB" = 1 ] || echo --no-default-features)
ln -sf "$ROOT/target/release/medorama" .venv/bin/medorama
COMMANDS="medorama"

# --- the viewer ----------------------------------------------------------------------------
case "$VIEWER" in
  cpp)
    need_compiler "${CXX:-c++}"
    "${CXX:-c++}" --version | head -1
    step "CMake and Ninja (into .venv)"
    .venv/bin/pip install --quiet "cmake>=3.24" ninja
    export PATH="$ROOT/.venv/bin:$PATH"
    step "Building the C++ viewer (vitalstream-cpp)"
    # CMake keeps the compiler a build folder was first configured with: start afresh when
    # that is not the compiler selected now (e.g. after `module load gcc`).
    cache=viewers/vitalstream/cpp/build/viewer/CMakeCache.txt
    if [ -f "$cache" ]; then
      cxx_cached=$(sed -n 's/^CMAKE_CXX_COMPILER:[A-Z]*=//p' "$cache")
      real() { readlink -f "$1" 2>/dev/null || python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"; }
      if [ -n "$cxx_cached" ] && [ "$(real "$cxx_cached")" != "$(real "$(command -v "${CXX:-c++}")")" ]; then
        echo "viewers/vitalstream/cpp/build/viewer was configured with $cxx_cached: removing it to start afresh"
        rm -rf viewers/vitalstream/cpp/build/viewer
      fi
    fi
    # the whole file system as the viewer's root, and its password: Medorama's own rules apply
    # (its gateway; docs/architecture.md)
    (cd viewers/vitalstream/cpp && cmake --preset viewer -DVS_ROOT= -DVS_PASSWORD=ON \
       -DVS_WFDB=$([ "$WFDB" = 1 ] && echo ON || echo OFF) >/dev/null && cmake --build --preset viewer)
    ln -sf "$ROOT/viewers/vitalstream/cpp/build/viewer/bin/vitalstream-cpp" .venv/bin/vitalstream-cpp
    COMMANDS="$COMMANDS vitalstream-cpp"
    ;;
  rs)
    step "Building the Rust viewer (vitalstream-rs)"
    # the whole file system as its root
    env -u VITALSTREAM_ROOT cargo build --release --locked -p vitalstream-rs \
      $([ "$WFDB" = 1 ] || echo --no-default-features --features password)
    ln -sf "$ROOT/target/release/vitalstream-rs" .venv/bin/vitalstream-rs
    COMMANDS="$COMMANDS vitalstream-rs"
    ;;
  py)
    step "The Python viewer (vitalstream-py) and its packages"
    unset CONDA_PREFIX || true
    env -u VITALSTREAM_ROOT -u VITALSTREAM_PASSWORD \
      .venv/bin/pip install --quiet -e "./viewers/vitalstream/python/vitalstream$([ "$WFDB" = 1 ] && echo '[wfdb]')" \
      --config-settings editable_mode=compat
    if [ "$WFDB" = 0 ] && .venv/bin/python -c 'import wfdbx' 2>/dev/null; then
      echo "note: .venv already holds wfdb-x (installed earlier), which the Python viewer then uses;"
      echo "      .venv/bin/pip uninstall wfdb-x leaves it out"
    fi
    COMMANDS="$COMMANDS vitalstream-py"
    ;;
  none) ;;
esac

# --- the database tools (muse-py, dicom-py; pydicom) ------------------------------------------
if [ "$TOOLS" = 1 ]; then
  step "The database tools (muse-py, dicom-py)"
  .venv/bin/pip install --quiet -e ./viewers/ecg/python/muse -e ./viewers/pacs/python/dicom --config-settings editable_mode=compat
  COMMANDS="$COMMANDS muse-py dicom-py"
fi

# --- the demo (--demo): invented patients, loaded by `medorama init` (docs/administration.md) --
DEMO_DIR="$ROOT/.venv/share/medorama/demo"
if [ "$DEMO" = 1 ]; then
  step "The demo: invented patients and their files, in .venv/share/medorama/demo"
  # tools/make-demo-data.py writes its WFDB records with wfdb-x (the version the Python viewer
  # pins) and NumPy: installed when .venv lacks them (offline: docs/building.md)
  if ! .venv/bin/python -c 'import numpy, wfdbx' 2>/dev/null; then
    wfdbx=$(sed -n 's/^ *"\(wfdb-x @ [^"]*\)",*$/\1/p' viewers/vitalstream/python/vitalstream/pyproject.toml)
    .venv/bin/pip install --quiet numpy "$wfdbx"
  fi
  .venv/bin/python tools/make-demo-data.py "$DEMO_DIR" | tail -1
  # `medorama init` loads the demo from the folder named here (next to the programs)
  echo "$DEMO_DIR" > .venv/bin/medorama.demo
else
  # without --demo, new catalogs start empty (a catalog that holds the demo keeps it)
  rm -f .venv/bin/medorama.demo
fi

# The viewer Medorama starts (it reads this file next to itself; `viewer` in medorama.toml
# overrides it): the choice made here, even if other viewers are installed too.
echo "$VIEWER" > .venv/bin/medorama.viewer

# --- git hooks (privacy checks before each commit) ------------------------------------------
# only in a clone of this repository (also a worktree), never in a repository around it
top=$(git rev-parse --show-toplevel 2>/dev/null || true)
if [ -n "$top" ] && [ "$(cd "$top" && pwd -P)" = "$(pwd -P)" ]; then
  current=$(git config --local --get core.hooksPath || true)
  if [ -n "$current" ] && [ "$current" != tools/git-hooks ]; then
    echo "warning: core.hooksPath is '$current' in this clone, so the privacy checks of tools/git-hooks do not run:"
    echo "  set it with 'git config core.hooksPath tools/git-hooks', or call tools/git-hooks/check-files from your hooks (PRIVACY.md)"
  else
    current=$(git config --get core.hooksPath || true)
    if [ -n "$current" ] && [ "$current" != tools/git-hooks ]; then
      echo "note: this clone now uses tools/git-hooks, not the hooks of your global core.hooksPath ('$current')"
    fi
    git config core.hooksPath tools/git-hooks
  fi
fi

# every program linked or installed, not a link to a missing build
for c in $COMMANDS; do
  [ -x ".venv/bin/$c" ] || { echo "error: .venv/bin/$c is missing or does not run (a link to a build that is not there?)"; exit 1; }
done

step "Done"
echo "Installed in .venv/bin: $COMMANDS"
echo "Next, in each new terminal:  source .venv/bin/activate"
if [ "$DEMO" = 1 ]; then
  echo "  medorama init --results DIR      (once; docs/administration.md): it loads the demo, the cohort DEMO"
else
  echo "  medorama init --results DIR      (once; docs/administration.md)"
fi
echo "  medorama serve                   (http://127.0.0.1:8090)"
echo "Licensed formats: medorama addons install PROGRAM   (docs/adapters.md)"
