#!/usr/bin/env bash
# Medorama and the VitalStream viewers, for development: everything into this clone (macOS or
# Linux, no administrator rights).
#
#   tools/install.sh                 all of it: the Python packages, the Rust workspace, the C++ project
#   tools/install.sh --py            the Python packages only (no compiler needed)
#   tools/install.sh --rs            Python + Rust
#   tools/install.sh --cpp           Python + C++
# Other options:
#   --demo         also make the demo: invented patients that `medorama init` loads into a cohort
#                  DEMO, for trying Medorama (off by default; docs/administration.md, "The demo")
#   --no-rustup    fail rather than install Rust if cargo is missing
#
# What it does:
#   1. .venv/ with the Python packages, editable: the Python viewer vitalstream_py and the
#      database tools muse_py and dicom_py, with the test tools.
#   2. Rust (--rs): installs rustup in your user account if cargo is missing, builds the
#      workspace (medorama, vitalstream-rs; target/release, links in .venv/bin).
#   3. C++ (--cpp): installs CMake and Ninja into .venv, builds vitalstream-cpp and its tests
#      (viewers/vitalstream/cpp/build/release/bin, link in .venv/bin).
#   4. Enables the repository's git hooks (privacy checks before each commit).
# Compilers: CC and CXX are used when set (e.g. after `module load gcc` or in a conda environment
# with gxx), so no administrator rights are needed when such a compiler is available.
# Afterwards: source .venv/bin/activate (or call .venv/bin/<command>).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
RS=0 CPP=0 ANY=0 RUSTUP=1 DEMO=0
for a in "$@"; do
  case "$a" in
    --py) ANY=1 ;;
    --rs) RS=1 ANY=1 ;;
    --cpp) CPP=1 ANY=1 ;;
    --no-rustup) RUSTUP=0 ;;
    --demo) DEMO=1 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "unknown option $a (see --help)"; exit 2 ;;
  esac
done
if [ "$ANY" = 0 ]; then RS=1 CPP=1; fi
step() { printf '\n==> %s\n' "$*"; }

# macOS: Xcode installed but its license not accepted makes cc fail: use the Command Line Tools.
if [ "$(uname)" = Darwin ] && [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Library/Developer/CommandLineTools ] \
   && ! cc --version >/dev/null 2>&1; then
  export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
# The compilers: CC and CXX when set (an HPC module, a conda environment), else cc and c++.
need_compiler() {  # $1: the compiler command
  if ! "$1" --version >/dev/null 2>&1; then
    echo "The C/C++ compiler ($1) is missing."
    if [ "$(uname)" = Darwin ]; then
      echo "Run 'xcode-select --install' (Command Line Tools; asks for an administrator once), then rerun."
    else
      echo "With administrator rights: sudo apt install build-essential (Debian, Ubuntu) or"
      echo "sudo dnf install gcc gcc-c++ (Fedora, RHEL). Without: load a compiler module"
      echo "(module load gcc), or install GCC 13 in your home folder with tools/install-gcc.sh and"
      echo "'source ~/miniforge3/bin/activate vs-build', then rerun (docs/building.md)."
    fi
    exit 1
  fi
}

# --- Python -----------------------------------------------------------------------------
PY=${PYTHON:-python3}
command -v "$PY" >/dev/null || { echo "python3 not found: install Python 3.9 or later"; exit 1; }
"$PY" -c 'import sys; sys.exit(sys.version_info < (3, 9))' || { echo "Python 3.9 or later is needed ($("$PY" -V))"; exit 1; }
step "Python environment (.venv) and the Python packages"
[ -d .venv ] || "$PY" -m venv .venv
.venv/bin/python -m pip install --quiet --upgrade pip
.venv/bin/pip install --quiet -e "./viewers/vitalstream/python/vitalstream[test,wfdb]" -e ./viewers/ecg/python/muse \
  -e ./viewers/pacs/python/dicom --config-settings editable_mode=compat
.venv/bin/pip install --quiet pytest httpx
COMMANDS="vitalstream-py muse-py dicom-py"

# --- Rust -------------------------------------------------------------------------------
if [ "$RS" = 1 ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
  # the builds in this clone's target/, where the links below point (not CARGO_TARGET_DIR or a
  # build.target-dir set elsewhere)
  export CARGO_TARGET_DIR="$ROOT/target"
  if ! command -v cargo >/dev/null; then
    if [ "$RUSTUP" = 0 ]; then
      echo "cargo not found and --no-rustup given: install Rust (https://rustup.rs), or leave out --rs"
      exit 1
    fi
    step "Installing Rust in your user account (rustup; ~/.rustup and ~/.cargo)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
  fi
  need_compiler "${CC:-cc}"
  step "Building the Rust workspace (medorama, vitalstream-rs)"
  cargo build --release --locked
  for b in medorama vitalstream-rs; do ln -sf "$ROOT/target/release/$b" ".venv/bin/$b"; done
  COMMANDS="$COMMANDS medorama vitalstream-rs"
fi

# --- C++ --------------------------------------------------------------------------------
if [ "$CPP" = 1 ]; then
  need_compiler "${CXX:-c++}"
  # C++20: GCC 12+ or Clang 15+ (Apple Clang 15+)
  "${CXX:-c++}" --version | head -1
  step "CMake and Ninja (into .venv)"
  .venv/bin/pip install --quiet "cmake>=3.24" ninja
  export PATH="$ROOT/.venv/bin:$PATH"
  step "Building the C++ viewer (vitalstream-cpp) and its tests"
  # CMake keeps the compiler a build folder was first configured with, whatever CC/CXX say now:
  # start afresh when that is not the compiler selected now (e.g. after `module load gcc`).
  cxx_now=$(command -v "${CXX:-c++}")
  cache=viewers/vitalstream/cpp/build/release/CMakeCache.txt
  if [ -f "$cache" ]; then
    cxx_cached=$(sed -n 's/^CMAKE_CXX_COMPILER:[A-Z]*=//p' "$cache")
    real() { readlink -f "$1" 2>/dev/null || python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"; }
    if [ -n "$cxx_cached" ] && [ "$(real "$cxx_cached")" != "$(real "$cxx_now")" ]; then
      echo "viewers/vitalstream/cpp/build/release was configured with $cxx_cached; now $cxx_now: removing it to start afresh"
      rm -rf viewers/vitalstream/cpp/build/release
    fi
  fi
  (cd viewers/vitalstream/cpp && cmake --preset release >/dev/null && cmake --build --preset release)
  ln -sf "$ROOT/viewers/vitalstream/cpp/build/release/bin/vitalstream-cpp" .venv/bin/vitalstream-cpp
  COMMANDS="$COMMANDS vitalstream-cpp"
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
echo "Commands in .venv/bin: $COMMANDS"
echo "Activate with:  source .venv/bin/activate"
echo "Check with:     make test"
if [ "$DEMO" = 1 ]; then
  echo "Then:           medorama init --results DIR   (loads the demo: the cohort DEMO), medorama serve"
fi
