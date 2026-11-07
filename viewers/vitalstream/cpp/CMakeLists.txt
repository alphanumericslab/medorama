# VitalStream, C++ implementation: the core library (vs_core, vitalstream::core), the engine
# (vs_engine, vitalstream::engine) and the viewer (vitalstream-cpp). WFDB records are read and
# written with wfdb-x (its C++ library, fetched at a pinned commit: cmake/deps.cmake), unless
# VS_WFDB=OFF (a build without WFDB support and without wfdb-x). Adapters
# written in C++ (docs/adapters.md) build on this project: add it with VS_BUILD_VIEWER=OFF and
# link vs_engine.
#
#   cmake --preset release && cmake --build --preset release      # -> build/release/bin/
#   ctest --preset release                                       # unit tests
#   cmake --preset nowfdb && cmake --build --preset nowfdb        # without WFDB: build/nowfdb/bin/
#
# See README.md for the toolchain, the options and the per-system instructions.

cmake_minimum_required(VERSION 3.24)

# macOS: CMake 4 no longer picks the SDK by itself; without it, a compiler from the Command Line
# Tools uses their incomplete C++ headers.  Use the SDK xcrun reports.
if(APPLE AND NOT CMAKE_OSX_SYSROOT AND NOT DEFINED ENV{SDKROOT})
  execute_process(COMMAND xcrun --show-sdk-path OUTPUT_VARIABLE _vs_sdk OUTPUT_STRIP_TRAILING_WHITESPACE
                  ERROR_QUIET)
  if(_vs_sdk)
    set(CMAKE_OSX_SYSROOT "${_vs_sdk}" CACHE PATH "macOS SDK")
  endif()
endif()
project(vitalstream_cpp VERSION 0.1.0 DESCRIPTION "VitalStream (C++)" LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_C_STANDARD 99)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/lib)
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/lib)
if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE Release)
endif()

# C++20 as this code uses it (defaulted comparisons, floating-point std::to_chars/from_chars, ...):
# stop at once with the fix, rather than with compiler errors later.
# macOS: some Command Line Tools installations (e.g. after an update) leave an incomplete copy of
# the C++ standard headers in their own include folder, which Apple Clang searches before the
# SDK's complete one: then even <cstddef> is "not found".  Detect that and use the SDK's headers.
if(APPLE AND CMAKE_CXX_COMPILER_ID MATCHES "Clang" AND CMAKE_OSX_SYSROOT)
  include(CheckCXXSourceCompiles)
  check_cxx_source_compiles("#include <cstddef>\n#include <exception>\nint main() { return 0; }" VS_CXX_HEADERS_OK)
  set(_vs_sdk_cxx "${CMAKE_OSX_SYSROOT}/usr/include/c++/v1")
  if(NOT VS_CXX_HEADERS_OK AND EXISTS "${_vs_sdk_cxx}/cstddef")
    message(STATUS "The Command Line Tools' C++ headers are incomplete: using the SDK's (${_vs_sdk_cxx}). "
                   "To repair them: sudo rm -rf /Library/Developer/CommandLineTools && xcode-select --install")
    add_compile_options("$<$<COMPILE_LANGUAGE:CXX>:-nostdinc++>"
                        "$<$<COMPILE_LANGUAGE:CXX>:SHELL:-isystem ${_vs_sdk_cxx}>")
  endif()
endif()

# CMake keeps the compiler a build folder was first configured with: say so when CXX now names
# another one (e.g. after `module load gcc` in a folder configured before).
if(DEFINED ENV{CXX} AND NOT "$ENV{CXX}" STREQUAL "")
  find_program(_vs_env_cxx NAMES "$ENV{CXX}" NO_CACHE)
  if(_vs_env_cxx)
    file(REAL_PATH "${_vs_env_cxx}" _vs_env_cxx_real)
    file(REAL_PATH "${CMAKE_CXX_COMPILER}" _vs_cxx_real)
    if(NOT _vs_env_cxx_real STREQUAL _vs_cxx_real)
      set(_vs_stale_compiler TRUE)
      message(WARNING
        "  CXX is ${_vs_env_cxx}, but this build folder was configured with ${CMAKE_CXX_COMPILER},\n"
        "  which CMake keeps. To use CXX: delete the build folder (rm -rf build/release) and configure again.\n")
    endif()
  endif()
endif()

set(_vs_min_GNU 12)
set(_vs_min_Clang 15)
set(_vs_min_AppleClang 15)
set(_vs_min_MSVC 19.34)
foreach(_lang CXX)   # the C parts (the WFDB library) build with any C compiler
  set(_id "${CMAKE_${_lang}_COMPILER_ID}")
  set(_ver "${CMAKE_${_lang}_COMPILER_VERSION}")
  if(DEFINED _vs_min_${_id} AND _ver VERSION_LESS _vs_min_${_id})
    if(_vs_stale_compiler)
      message(FATAL_ERROR
        "  \n"
        "  The C++ compiler of this build folder is too old: ${_id} ${_ver} (${CMAKE_${_lang}_COMPILER}).\n"
        "  CXX names another compiler ($ENV{CXX}), but CMake keeps the one this folder was first\n"
        "  configured with. Delete the build folder and configure again:\n"
        "       rm -rf build/release\n"
        "       cmake --preset release && cmake --build --preset release\n")
    endif()
    message(FATAL_ERROR
      "  \n"
      "  The C++ compiler is too old: ${_id} ${_ver} (${CMAKE_${_lang}_COMPILER}).\n"
      "  VitalStream's C++ code needs GCC ${_vs_min_GNU}+ (or Clang ${_vs_min_Clang}+, Apple Clang ${_vs_min_AppleClang}+, Visual Studio 2022).\n"
      "  A newer compiler can be set up without administrator rights, in one of these ways:\n"
      "  \n"
      "  A. A compiler module (clusters). See what is there:   module avail 2>&1 | grep -i gcc\n"
      "     If it lists gcc 12 or later:   module load <name it lists> && export CC=gcc CXX=g++\n"
      "  \n"
      "  B. gcc-toolset (RHEL, Rocky, Alma 8 and 9). See what is there:   ls /opt/rh\n"
      "     If it lists gcc-toolset-12 or later:   scl enable gcc-toolset-13 bash   (a number it lists)\n"
      "  \n"
      "  C. Neither: GCC 13 in your home folder (about 1 GB, a few minutes):\n"
      "       ../tools/install-gcc.sh\n"
      "       source ~/miniforge3/bin/activate vs-build\n"
      "  \n"
      "  A, B and C last for the current shell only: repeat the 'module load', 'scl' or 'source' line in\n"
      "  every new shell (or add it to ~/.bashrc). Then, in this folder (cpp/), start again with an empty\n"
      "  build folder, since CMake remembers the old compiler:\n"
      "       rm -rf build/release\n"
      "       cmake --preset release && cmake --build --preset release\n"
      "  \n"
      "  Details: cpp/README.md, 'Build and install: Linux', B.\n")
  endif()
endforeach()

# Floating-point results identical to the Rust and Python implementations: no fused multiply-add
# (GCC and Clang contract a * b + c into one instruction by default on some processors).
if(NOT MSVC)
  add_compile_options($<$<COMPILE_LANGUAGE:C,CXX>:-ffp-contract=off>)
endif()

if(WIN32)
  # <windows.h> without the min/max macros and the rarely used APIs; Windows 10 APIs (cpp-httplib
  # requires them, and MinGW's default may be older)
  add_compile_definitions(NOMINMAX WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0A00)
endif()

get_filename_component(VS_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/.." ABSOLUTE)

option(VS_BUILD_TESTS "Build the unit tests" ON)
option(VS_BUILD_VIEWER "Build the viewer vitalstream-cpp" ON)
# The viewer's root: it never opens anything outside it (empty: the whole file system; ~ is the
# home folder of whoever runs the viewer, expanded when it runs).  install.sh sets it to ~.
set(VS_ROOT "" CACHE STRING "The folder the viewer may open at most (empty: the whole file system; ~ allowed)")
# The password the page can ask for (--set-password).  OFF: no password code at all, and the
# viewer refuses to run where one is required (require_password, set by Medorama).
option(VS_PASSWORD "Build the viewer with password support" ON)
# WFDB records and folders, read with wfdb-x (fetched at a pinned commit: cmake/deps.cmake).  OFF:
# no wfdb-x code at all (not even fetched); a WFDB record or folder is refused ("this build has no
# WFDB support"), every other format is read as usual.
option(VS_WFDB "Build with WFDB support (wfdb-x)" ON)
set(VS_DEPS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/deps" CACHE PATH
    "Folder with pre-downloaded dependency archives (for builds without internet access)")

include(cmake/warnings.cmake)
include(cmake/build_paths.cmake)
include(cmake/deps.cmake)
include(cmake/web.cmake)

if(VS_BUILD_TESTS)
  enable_testing()
endif()

add_subdirectory(core)
add_subdirectory(engine)
if(VS_BUILD_VIEWER)
  add_subdirectory(vitalstream)
endif()
