#!/usr/bin/env bash
# VitalStream on its own, without Medorama: build one viewer and install it (macOS or Linux, no
# administrator rights).
#
#   viewers/vitalstream/install.sh                 the C++ viewer (vitalstream-cpp)
#   viewers/vitalstream/install.sh --viewer rs     the Rust viewer (vitalstream-rs)
#   viewers/vitalstream/install.sh --viewer py     the Python viewer (vitalstream-py)
# Other options:
#   --prefix DIR   where it goes: DIR/bin (default ~/.local; the Python viewer's environment in
#                  DIR/share/vitalstream/venv)
#   --no-rustup    fail rather than install Rust if cargo is missing (rs)
#   --root DIR     the viewer's root, fixed in the program: it never opens anything outside DIR
#                  (default ~, the home folder of whoever runs it; / for the whole file system)
#   --no-password  build it without password support (no password can be set; it then refuses
#                  to run where one is required, as under Medorama)
#   --no-wfdb      build it without WFDB support: no wfdb-x code in it (a WFDB record or folder
#                  is refused; every other format is read; docs/building.md at the repository root)
#
# Then: vitalstream-cpp (or -rs, -py) opens the viewer in a browser; --help lists its options.
# Licensed recording formats are read through their add-on, an adapter installed into
# ~/.local/share/medorama/adapters (docs/adapters.md at the repository root).
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
VIEWER=cpp PREFIX="$HOME/.local" RUSTUP=1 VROOT='~' PASSWORD=1 WFDB=1
while [ $# -gt 0 ]; do
  case "$1" in
    --viewer) VIEWER="${2:?--viewer needs a value: cpp, rs or py}"; shift 2 ;;
    --prefix) PREFIX="${2:?--prefix needs a folder}"; shift 2 ;;
    --no-rustup) RUSTUP=0; shift ;;
    --root) VROOT="${2:?--root needs a folder}"; shift 2 ;;
    --no-password) PASSWORD=0; shift ;;
    --no-wfdb) WFDB=0; shift ;;
    -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option $1 (see --help)"; exit 2 ;;
  esac
done
step() { printf '\n==> %s\n' "$*"; }
# absolute folders: the Python viewer's link and the root fixed in the program must not depend on
# the folder this script was started from (a root starting with ~ stays as it is: the home folder
# of whoever runs the viewer)
case "$PREFIX" in /*) ;; *) PREFIX="$(pwd)/$PREFIX" ;; esac
case "$VROOT" in /*|'~'|'~/'*) ;; *) VROOT="$(pwd)/$VROOT" ;; esac
mkdir -p "$PREFIX/bin"
PREFIX=$(cd "$PREFIX" && pwd)
# Copy a program into place without writing over the one there: on macOS, a program changed in
# place is killed when it starts (its signature no longer matches); a new file is not.
install_program() {
  rm -f "$2.new"
  cp "$1" "$2.new"
  mv -f "$2.new" "$2"
}

# macOS: Xcode installed but its license not accepted makes cc fail: use the Command Line Tools.
if [ "$(uname)" = Darwin ] && [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Library/Developer/CommandLineTools ] \
   && ! cc --version >/dev/null 2>&1; then
  export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
PY=${PYTHON:-python3}

case "$VIEWER" in
  cpp)
    "${CXX:-c++}" --version >/dev/null 2>&1 || { echo "a C++20 compiler is needed (GCC 12+, Clang 15+; docs/building.md)"; exit 1; }
    if ! command -v cmake >/dev/null || ! command -v ninja >/dev/null; then
      step "CMake and Ninja (in a build environment, $HERE/cpp/build/tools)"
      "$PY" -m venv "$HERE/cpp/build/tools"
      "$HERE/cpp/build/tools/bin/pip" install --quiet "cmake>=3.24" ninja
      export PATH="$HERE/cpp/build/tools/bin:$PATH"
    fi
    step "Building vitalstream-cpp"
    # its own build folder: Medorama's viewer (build/viewer) keeps its settings
    (cd "$HERE/cpp" && cmake --preset viewer -B build/standalone -DVS_ROOT="$VROOT" \
       -DVS_PASSWORD=$([ "$PASSWORD" = 1 ] && echo ON || echo OFF) -DVS_WFDB=$([ "$WFDB" = 1 ] && echo ON || echo OFF) \
       >/dev/null && cmake --build build/standalone)
    install_program "$HERE/cpp/build/standalone/bin/vitalstream-cpp" "$PREFIX/bin/vitalstream-cpp"
    PROGRAM=vitalstream-cpp
    ;;
  rs)
    export PATH="$HOME/.cargo/bin:$PATH"
    if ! command -v cargo >/dev/null; then
      [ "$RUSTUP" = 1 ] || { echo "cargo not found and --no-rustup given: install Rust (https://rustup.rs)"; exit 1; }
      step "Installing Rust in your user account (rustup)"
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
    fi
    step "Building vitalstream-rs"
    # its own target folder: the workspace's (target/release, which Medorama's viewer is) keeps its
    # build; --target-dir wins over a CARGO_TARGET_DIR or build.target-dir set elsewhere
    # the features: password and wfdb by default; --no-password and --no-wfdb leave one out
    FEATURES=$( { [ "$PASSWORD" = 0 ] || echo password; [ "$WFDB" = 0 ] || echo wfdb; } | paste -sd, -)
    (cd "$ROOT" && VITALSTREAM_ROOT="$VROOT" cargo build --release --locked -p vitalstream-rs \
       --target-dir "$ROOT/target/standalone" --no-default-features ${FEATURES:+--features "$FEATURES"})
    install_program "$ROOT/target/standalone/release/vitalstream-rs" "$PREFIX/bin/vitalstream-rs"
    PROGRAM=vitalstream-rs
    ;;
  py)
    step "The Python viewer and its packages ($PREFIX/share/vitalstream/venv)"
    "$PY" -m venv "$PREFIX/share/vitalstream/venv"
    "$PREFIX/share/vitalstream/venv/bin/pip" install --quiet --upgrade pip
    VITALSTREAM_ROOT="$VROOT" VITALSTREAM_PASSWORD="$PASSWORD" \
      "$PREFIX/share/vitalstream/venv/bin/pip" install --quiet --force-reinstall \
      "$HERE/python/vitalstream$([ "$WFDB" = 1 ] && echo '[wfdb]')"
    ln -sf "$PREFIX/share/vitalstream/venv/bin/vitalstream-py" "$PREFIX/bin/vitalstream-py"
    PROGRAM=vitalstream-py
    ;;
  *) echo "--viewer: cpp, rs or py (not '$VIEWER')"; exit 2 ;;
esac

# the program installed, and one that runs (not a link to a missing build)
[ -x "$PREFIX/bin/$PROGRAM" ] || { echo "error: $PREFIX/bin/$PROGRAM is missing or cannot be run"; exit 1; }

step "Done"
echo "Installed $PREFIX/bin/$PROGRAM (root: $VROOT$([ "$PASSWORD" = 1 ] || echo '; no password support')$([ "$WFDB" = 1 ] || echo '; no WFDB support'))"
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Add $PREFIX/bin to PATH to run it by name." ;; esac
if [ "$PASSWORD" = 1 ]; then
  echo "Start it with: $PROGRAM   (a password for shared machines: $PROGRAM --set-password)"
else
  echo "Start it with: $PROGRAM"
fi
