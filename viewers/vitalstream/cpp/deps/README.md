# C++ dependency archives

This folder holds the archives of the C and C++ dependencies of the
[C++ viewer](../README.md), so that it can be built without internet access. It is for
administrators who build on an offline machine. Git ignores everything here except this file.

To build offline:
1. On a machine with internet access, fill this folder with `tools/fetch-cpp-deps.sh` (Windows:
   `tools\fetch-cpp-deps.ps1`).
2. Copy the clone, with this folder, to the machine without internet access, and build as
   usual.

The build takes each archive from this folder when it is present, and downloads it otherwise.
Either way, it checks the archive's SHA-256 against the value pinned in
[`../cmake/deps.cmake`](../cmake/deps.cmake). The versions and licenses are listed in
[`THIRD_PARTY_LICENSES.md`](../../../../THIRD_PARTY_LICENSES.md).
