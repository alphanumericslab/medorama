# The viewer's core library (vitalstream::core): values, Python-compatible formatting, text
# helpers, paths, SHA-256, memory-mapped files and WFDB records through wfdb-x (core/wfdb_c.hpp,
# only with VS_WFDB=ON). The C++ counterpart of the Rust crate signal-core; adapters written in
# C++ use it too.

add_library(vs_core STATIC
  src/file_data.cpp src/pyfmt.cpp src/sha256.cpp src/text.cpp src/value.cpp)
target_include_directories(vs_core PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_compile_features(vs_core PUBLIC cxx_std_20)
if(VS_WFDB)
  target_sources(vs_core PRIVATE src/wfdb_c.cpp)
  target_link_libraries(vs_core PUBLIC wfdbx::wfdbx)
endif()
vs_warnings(vs_core)
