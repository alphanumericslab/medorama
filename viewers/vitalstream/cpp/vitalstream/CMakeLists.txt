# The viewer vitalstream-cpp: the HTTP API over the engine (library vs_viewer, also used by the
# tests) and the command, with the page of <viewer>/web embedded.

vs_embed_web()
# the password (auth.cpp), or none (auth_none.cpp: VS_PASSWORD=OFF)
if(VS_PASSWORD)
  set(_vs_auth src/auth.cpp)
else()
  set(_vs_auth src/auth_none.cpp)
endif()
add_library(vs_viewer STATIC
  ${_vs_auth} src/server.cpp src/settings.cpp src/timezones.cpp ${VS_WEB_CPP})
target_include_directories(vs_viewer PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/src)
target_compile_definitions(vs_viewer PUBLIC VS_VIEWER_VERSION="${PROJECT_VERSION}"
                           VS_PASSWORD=$<BOOL:${VS_PASSWORD}>)
# VS_ROOT and the viewer's folder, in a generated header (any folder name, Windows' included)
vs_build_paths("${CMAKE_CURRENT_BINARY_DIR}/generated/build_paths.hpp" "${VS_ROOT}" "${VS_REPO_ROOT}")
target_include_directories(vs_viewer PRIVATE ${CMAKE_CURRENT_BINARY_DIR}/generated)
target_link_libraries(vs_viewer PUBLIC vs_engine vs_httplib vs_toml)
if(WIN32 AND VS_PASSWORD)
  target_link_libraries(vs_viewer PUBLIC bcrypt)
endif()
vs_warnings(vs_viewer)

add_executable(vitalstream-cpp src/main.cpp)
target_link_libraries(vitalstream-cpp PRIVATE vs_viewer vs_cli11)
if(WIN32)
  target_link_libraries(vitalstream-cpp PRIVATE shell32)
endif()
vs_warnings(vitalstream-cpp)
install(TARGETS vitalstream-cpp RUNTIME DESTINATION bin)

if(VS_BUILD_TESTS)
  add_subdirectory(tests)
endif()
