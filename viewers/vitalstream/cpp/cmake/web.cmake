# The viewer page (<repo>/web/, shared by the three viewers), embedded into vitalstream-cpp:
# cmake/embed_web.cmake turns every file into a byte array in a generated source file,
# regenerated whenever a file of web/ changes.

set(VS_WEB_DIR "${VS_REPO_ROOT}/web")
set(VS_WEB_SCRIPT "${CMAKE_CURRENT_LIST_DIR}/embed_web.cmake")
set(VS_WEB_CPP "${CMAKE_BINARY_DIR}/generated/web_files.cpp")
file(GLOB_RECURSE VS_WEB_FILES CONFIGURE_DEPENDS "${VS_WEB_DIR}/*")
list(FILTER VS_WEB_FILES EXCLUDE REGEX "README\\.md$")
# The command that generates it (call where the target using the file is defined: CMake runs
# a custom command only for targets of the same directory).
function(vs_embed_web)
add_custom_command(
  OUTPUT "${VS_WEB_CPP}"
  COMMAND ${CMAKE_COMMAND} -DWEB_DIR=${VS_WEB_DIR} -DOUT=${VS_WEB_CPP}
          -P ${VS_WEB_SCRIPT}
  DEPENDS ${VS_WEB_FILES} ${VS_WEB_SCRIPT}
  COMMENT "Embedding the viewer page (web/)")
endfunction()
