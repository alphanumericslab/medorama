# Third-party dependencies, pinned by version and SHA-256 (see THIRD_PARTY_LICENSES.md).
#
# Each archive is taken from ${VS_DEPS_DIR} when it is there (tools/fetch-cpp-deps.sh fills that
# folder, for builds without internet access), else downloaded from its URL.  The hash is
# checked either way.

include(FetchContent)
set(FETCHCONTENT_QUIET ON)

function(vs_dependency name file url sha256)
  set(local "${VS_DEPS_DIR}/${file}")
  if(EXISTS "${local}")
    set(src "${local}")
  else()
    set(src "${url}")
  endif()
  FetchContent_Declare(${name} URL "${src}" URL_HASH SHA256=${sha256} DOWNLOAD_EXTRACT_TIMESTAMP ON
                       ${ARGN})
endfunction()

# wfdb-x 1.2.1, commit 4a91916 (its library: GNU LGPL v2.1 or later): WFDB headers, records and
# annotation files (https://github.com/alphanumericslab/wfdb-x; its C++ part, the library only).
# Not fetched at all with VS_WFDB=OFF.
if(VS_WFDB)
  set(WFDBX_BUILD_TOOLS OFF CACHE BOOL "" FORCE)
  set(WFDBX_BUILD_TESTS OFF CACHE BOOL "" FORCE)
  vs_dependency(wfdbx wfdb-x-4a91916f5282fba44ca3056802fbc7def298af08.tar.gz
    https://github.com/alphanumericslab/wfdb-x/archive/4a91916f5282fba44ca3056802fbc7def298af08.tar.gz
    1221990620cd1758b020a2cfe89754d7aa0db4c43a5c70503c6a2521c77749df
    SOURCE_SUBDIR cpp)
  FetchContent_MakeAvailable(wfdbx)
endif()

# SQLCipher 4.20.0 (BSD-3-Clause), based on SQLite 3.54.1 (public domain), with libtomcrypt
# 1.18.2 (public domain or WTFPL) as its crypto provider: the index store (index format 3,
# engine/src/store.cpp), encrypted or plain.  Taken as the crates.io package sqlcipher-src
# 420.0.0, which ships the SQLCipher amalgamation and libtomcrypt's sources as released (MIT for
# its own few files): no OpenSSL, no Tcl, nothing of the system.  Built here as vs_sqlite.
vs_dependency(sqlcipher sqlcipher-src-420.0.0.tar.gz
  https://static.crates.io/crates/sqlcipher-src/sqlcipher-src-420.0.0+sqlcipher-4.20.0-sqlite-3.54.1-libtomcrypt-1.18.2.crate
  fd6b5280ed2593d07b2816ec531821c15c306903bcca224716f1fbcc2bbfedfe
  DOWNLOAD_NAME sqlcipher-src-420.0.0.tar.gz SOURCE_SUBDIR do-not-add)
FetchContent_MakeAvailable(sqlcipher)
set(_vs_sc "${sqlcipher_SOURCE_DIR}/sqlcipher")
# every libtomcrypt source but the tables other sources include
file(GLOB_RECURSE _vs_ltc "${_vs_sc}/libtomcrypt/*.c")
list(FILTER _vs_ltc EXCLUDE REGEX "tab\\.c$")
set_source_files_properties(${_vs_ltc} PROPERTIES COMPILE_DEFINITIONS LTC_SOURCE)
add_library(vs_sqlite STATIC "${_vs_sc}/sqlcipher.c" ${_vs_ltc})
target_include_directories(vs_sqlite SYSTEM PUBLIC "${_vs_sc}" PRIVATE "${_vs_sc}/libtomcrypt/headers")
target_compile_definitions(vs_sqlite
  PUBLIC SQLITE_HAS_CODEC
  PRIVATE SQLCIPHER_CRYPTO_LIBTOMCRYPT SQLITE_THREADSAFE=1 SQLITE_TEMP_STORE=2 SQLITE_OMIT_LOAD_EXTENSION
          SQLITE_EXTRA_INIT=sqlcipher_extra_init SQLITE_EXTRA_SHUTDOWN=sqlcipher_extra_shutdown
          SQLCIPHER_OMIT_LOG
          # libtomcrypt: only what SQLCipher uses (one configuration for every unit, so that its
          # structures agree), seeded from the system's generator (/dev/urandom, or Windows')
          LTC_NOTHING LTC_RIJNDAEL LTC_CBC_MODE LTC_SHA1 LTC_SHA256 LTC_SHA512 LTC_HASH_HELPERS LTC_HMAC
          LTC_FORTUNA LTC_PKCS_5 LTC_RNG_GET_BYTES LTC_DEVRANDOM LTC_TRY_URANDOM_FIRST LTC_NO_TEST LTC_NO_ASM
          LTC_NO_MATH LTC_CLEAN_STACK)
# third-party C: no warnings from it
if(MSVC)
  target_compile_options(vs_sqlite PRIVATE /w)
else()
  target_compile_options(vs_sqlite PRIVATE -w)
endif()
find_package(Threads REQUIRED)
target_link_libraries(vs_sqlite PUBLIC Threads::Threads)
if(WIN32)
  target_link_libraries(vs_sqlite PUBLIC advapi32)
elseif(NOT APPLE)
  target_link_libraries(vs_sqlite PUBLIC m)
endif()

# CLI11 2.4.2 (BSD-3-Clause, header-only): command lines.
vs_dependency(cli11 CLI11-v2.4.2.tar.gz
  https://github.com/CLIUtils/CLI11/archive/refs/tags/v2.4.2.tar.gz
  f2d893a65c3b1324c50d4e682c0cdc021dd0477ae2c048544f39eed6654b699a
  SOURCE_SUBDIR do-not-add)
FetchContent_MakeAvailable(cli11)
add_library(vs_cli11 INTERFACE)
target_include_directories(vs_cli11 SYSTEM INTERFACE ${cli11_SOURCE_DIR}/include)

if(VS_BUILD_VIEWER)
  # toml++ 3.4.0 (MIT, header-only): the settings file vitalstream.toml.
  vs_dependency(tomlplusplus tomlplusplus-v3.4.0.tar.gz
    https://github.com/marzer/tomlplusplus/archive/refs/tags/v3.4.0.tar.gz
    8517f65938a4faae9ccf8ebb36631a38c1cadfb5efa85d9a72e15b9e97d25155
    SOURCE_SUBDIR do-not-add)
  FetchContent_MakeAvailable(tomlplusplus)
  add_library(vs_toml INTERFACE)
  target_include_directories(vs_toml SYSTEM INTERFACE ${tomlplusplus_SOURCE_DIR}/include)

  # cpp-httplib 0.65.0 (MIT, header-only): the viewer's HTTP server (no TLS, no compression).
  vs_dependency(httplib cpp-httplib-v0.65.0.tar.gz
    https://github.com/yhirose/cpp-httplib/archive/refs/tags/v0.65.0.tar.gz
    6fda8f23ff811a751c2b6e694fe2f1aa1625df53a317110329ef57a18f9fa5fb
    SOURCE_SUBDIR do-not-add)
  FetchContent_MakeAvailable(httplib)
  add_library(vs_httplib INTERFACE)
  target_include_directories(vs_httplib SYSTEM INTERFACE ${httplib_SOURCE_DIR})
  # what a request may hold: a request line of up to 64 KiB (long queries: many channels or
  # vital-sign names), header lines of 8 KiB, 100 headers (the bodies: add_routes)
  target_compile_definitions(vs_httplib INTERFACE CPPHTTPLIB_REQUEST_URI_MAX_LENGTH=65536
                             CPPHTTPLIB_MAX_LINE_LENGTH=65536 CPPHTTPLIB_HEADER_MAX_LENGTH=8192
                             CPPHTTPLIB_HEADER_MAX_COUNT=100)
  find_package(Threads REQUIRED)
  target_link_libraries(vs_httplib INTERFACE Threads::Threads)
  if(WIN32)
    target_link_libraries(vs_httplib INTERFACE ws2_32)
  endif()
endif()

if(VS_BUILD_TESTS)
  # GoogleTest 1.15.2 (BSD-3-Clause): unit tests.
  vs_dependency(googletest googletest-1.15.2.tar.gz
    https://github.com/google/googletest/releases/download/v1.15.2/googletest-1.15.2.tar.gz
    7b42b4d6ed48810c5362c265a17faebe90dc2373c885e5216439d37927f02926)
  set(gtest_force_shared_crt ON CACHE BOOL "" FORCE)
  set(INSTALL_GTEST OFF CACHE BOOL "" FORCE)
  set(BUILD_GMOCK OFF CACHE BOOL "" FORCE)
  FetchContent_MakeAvailable(googletest)
  # its headers are third-party code: no warnings from them in the tests
  foreach(t gtest gtest_main)
    get_target_property(_inc ${t} INTERFACE_INCLUDE_DIRECTORIES)
    set_target_properties(${t} PROPERTIES INTERFACE_SYSTEM_INCLUDE_DIRECTORIES "${_inc}")
  endforeach()
endif()
