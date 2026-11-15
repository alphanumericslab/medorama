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

The archives, as `tools/fetch-cpp-deps.sh` names them here:

| File | What | SHA-256 |
|---|---|---|
| `wfdb-x-4a91916f5282fba44ca3056802fbc7def298af08.tar.gz` | wfdb-x 1.2.1 (not needed with `VS_WFDB=OFF`) | `1221990620cd1758b020a2cfe89754d7aa0db4c43a5c70503c6a2521c77749df` |
| `sqlcipher-src-420.0.0.tar.gz` | SQLCipher 4.20.0 (SQLite 3.54.1) and libtomcrypt 1.18.2: the crates.io package `sqlcipher-src` 420.0.0+sqlcipher-4.20.0-sqlite-3.54.1-libtomcrypt-1.18.2 (a `.crate` file is a `.tar.gz` archive), saved under this shorter name | `fd6b5280ed2593d07b2816ec531821c15c306903bcca224716f1fbcc2bbfedfe` |
| `CLI11-v2.4.2.tar.gz` | CLI11 2.4.2 | `f2d893a65c3b1324c50d4e682c0cdc021dd0477ae2c048544f39eed6654b699a` |
| `tomlplusplus-v3.4.0.tar.gz` | toml++ 3.4.0 | `8517f65938a4faae9ccf8ebb36631a38c1cadfb5efa85d9a72e15b9e97d25155` |
| `cpp-httplib-v0.65.0.tar.gz` | cpp-httplib 0.65.0 | `6fda8f23ff811a751c2b6e694fe2f1aa1625df53a317110329ef57a18f9fa5fb` |
| `googletest-1.15.2.tar.gz` | GoogleTest 1.15.2 (the tests only) | `7b42b4d6ed48810c5362c265a17faebe90dc2373c885e5216439d37927f02926` |

To fetch one by hand, take the URL next to its name in `../cmake/deps.cmake` and save the
download under the name above.
