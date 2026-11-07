#!/usr/bin/env bash
# Install a C++20 compiler for VitalStream's C++ code without administrator rights (Linux):
# GCC 13, CMake, Ninja and Python from conda-forge, in a Miniforge installation in your home
# folder.  For machines whose system compiler is too old (e.g. GCC 8.5 on RHEL/Rocky/Alma 8)
# and where no newer compiler module is available.
#
#   tools/install-gcc.sh                  Miniforge in ~/miniforge3, environment "vs-build"
#   tools/install-gcc.sh --prefix DIR     Miniforge in DIR (an existing Miniforge or Anaconda
#                                         installation there is reused)
#
# About 1 GB and a few minutes.  Afterwards, in every new shell that builds VitalStream:
#   source ~/miniforge3/bin/activate vs-build
# (this sets CC and CXX to the new GCC and puts CMake and Ninja on the PATH).
# To remove it all: rm -rf ~/miniforge3
set -euo pipefail

PREFIX="$HOME/miniforge3"
ENV_NAME=vs-build
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="${2:?--prefix needs a folder}"; shift 2 ;;
    --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option $1 (see --help)"; exit 2 ;;
  esac
done

if [ "$(uname)" != Linux ]; then
  echo "This script is for Linux.  On macOS use Apple's Command Line Tools (xcode-select --install);"
  echo "on Windows see viewers/vitalstream/cpp/README.md, 'Build and install: Windows'."
  exit 1
fi

if [ ! -x "$PREFIX/bin/conda" ]; then
  # the latest release, by its version, for the installer's published SHA-256
  url=$(curl -fsSL --retry 3 -o /dev/null -w '%{url_effective}' https://github.com/conda-forge/miniforge/releases/latest)
  version=${url##*/}
  installer="Miniforge3-$version-Linux-$(uname -m).sh"
  echo "==> Downloading Miniforge ($installer)"
  tmp=$(mktemp -d)
  base="https://github.com/conda-forge/miniforge/releases/download/$version"
  curl -fL --retry 3 -o "$tmp/$installer" "$base/$installer"
  curl -fsSL --retry 3 -o "$tmp/$installer.sha256" "$base/$installer.sha256"
  expected=$(awk '{print $1}' "$tmp/$installer.sha256")
  actual=$(sha256sum "$tmp/$installer" | awk '{print $1}')
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "The download's SHA-256 ($actual) is not the published one (${expected:-none}): not installing it."
    rm -rf "$tmp"
    exit 1
  fi
  echo "==> Installing Miniforge into $PREFIX (your home folder; no administrator rights needed)"
  bash "$tmp/$installer" -b -p "$PREFIX"
  rm -rf "$tmp"
else
  echo "==> Using the conda installation in $PREFIX"
fi

if "$PREFIX/bin/conda" env list | awk '{print $1}' | grep -qx "$ENV_NAME"; then
  echo "==> Updating the environment $ENV_NAME"
  "$PREFIX/bin/conda" install -y -n "$ENV_NAME" -c conda-forge "gxx=13" cmake ninja "python=3.12"
else
  echo "==> Creating the environment $ENV_NAME: GCC 13, CMake, Ninja, Python 3.12"
  "$PREFIX/bin/conda" create -y -n "$ENV_NAME" -c conda-forge "gxx=13" cmake ninja "python=3.12"
fi
# CC and CXX, set by every activation of the environment: conda-forge's gxx package puts gcc and
# g++ in it, and CMake would otherwise take the system's c++ first
ENV_DIR=$("$PREFIX/bin/conda" env list | awk -v n="$ENV_NAME" '$1 == n {print $NF}')
[ -n "$ENV_DIR" ] || ENV_DIR="$PREFIX/envs/$ENV_NAME"
"$PREFIX/bin/conda" env config vars set -n "$ENV_NAME" CC="$ENV_DIR/bin/gcc" CXX="$ENV_DIR/bin/g++" > /dev/null

set +u   # conda's activation scripts read unset variables
# shellcheck disable=SC1091
source "$PREFIX/bin/activate" "$ENV_NAME"
set -u
echo
echo "==> Installed:"
"${CXX:-g++}" --version | head -1
cmake --version | head -1
echo "ninja $(ninja --version)"
cat <<EOF

Next, in every new shell that builds VitalStream:
    source $PREFIX/bin/activate $ENV_NAME
then, in the repository's viewers/vitalstream/cpp/ folder (an old build folder must go first):
    rm -rf build/release
    cmake --preset release && cmake --build --preset release && ctest --preset release
To run the programs outside this environment (or copy them to another machine), add
    -DCMAKE_EXE_LINKER_FLAGS="-static-libstdc++ -static-libgcc"
to the cmake --preset command.  Or let the repository's installer do the rest:
    tools/install.sh --cpp
EOF
