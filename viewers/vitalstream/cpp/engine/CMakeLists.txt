# The engine (vitalstream::engine): reading recordings (WFDB folders unless VS_WFDB=OFF, EDF files,
# and other formats through adapters), their index (docs/index-format.md) and the queries the viewer answers from it.
# The C++ counterpart of the Rust crate recording-engine; adapters written in C++ build on it too.

# WFDB records: read with wfdb-x (sources_wfdb.cpp), or refused (sources_wfdb_none.cpp: VS_WFDB=OFF)
if(VS_WFDB)
  set(_vs_wfdb src/sources_wfdb.cpp)
else()
  set(_vs_wfdb src/sources_wfdb_none.cpp)
endif()
add_library(vs_engine STATIC
  src/adapters.cpp src/common.cpp src/dataset.cpp src/index.cpp src/sources.cpp src/sources_edf.cpp
  src/sources_wfdb_files.cpp ${_vs_wfdb})
target_include_directories(vs_engine PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_compile_definitions(vs_engine PRIVATE VS_ENGINE_VERSION="${PROJECT_VERSION}")
target_link_libraries(vs_engine PUBLIC vs_core)
vs_warnings(vs_engine)
