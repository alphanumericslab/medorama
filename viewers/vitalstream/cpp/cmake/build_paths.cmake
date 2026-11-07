# The paths fixed when the viewer is built, written into a generated header as raw string
# literals: any character a folder name may hold (spaces, backslashes, quotes) reaches the program
# as it is.  On Windows they are written with '/' between folders (file(TO_CMAKE_PATH): a
# backslash is a separator there, never an escape); the program turns them into its system's
# separators.  Elsewhere a backslash is part of a name, and the paths are written as given.
#
#   vs_build_paths(<header> <root> <viewer source folder>)
set(VS_BUILD_PATHS_TEMPLATE "${CMAKE_CURRENT_LIST_DIR}/build_paths.hpp.in")

function(vs_build_paths header root viewer_dir)
  if(WIN32)
    file(TO_CMAKE_PATH "${root}" VS_ROOT_CMAKE)
    file(TO_CMAKE_PATH "${viewer_dir}" VS_VIEWER_SOURCE)
  else()
    set(VS_ROOT_CMAKE "${root}")
    set(VS_VIEWER_SOURCE "${viewer_dir}")
  endif()
  foreach(_p IN ITEMS "${VS_ROOT_CMAKE}" "${VS_VIEWER_SOURCE}")
    string(FIND "${_p}" ")vs_path\"" _end)
    if(NOT _end EQUAL -1)
      message(FATAL_ERROR "A build path may not contain )vs_path\": ${_p}")
    endif()
  endforeach()
  configure_file("${VS_BUILD_PATHS_TEMPLATE}" "${header}" @ONLY)
endfunction()
