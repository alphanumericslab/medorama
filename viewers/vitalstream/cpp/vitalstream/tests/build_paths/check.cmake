# The viewer's root (VS_ROOT) reaches the program as it was given, whatever it holds: spaces,
# quotes, backslashes (Windows' separators), a closing parenthesis (install.ps1 -Root, install.sh
# --root).  Builds the program of this folder with such a root and compares what it prints.
#
#   cmake -DSRC=<this folder> -DBIN=<a build folder> -DGEN=<generator> -DCXX=<compiler>
#         -DCXXFLAGS=<flags> [-DSYSROOT=<macOS SDK>] -P check.cmake
if(WIN32)
  set(root [[C:\Data\new dir "x" (y)]])
  set(expected [[C:/Data/new dir "x" (y)]])
else()
  set(root [[/data/new dir/a\b "x" (y)]])
  set(expected "${root}")
endif()
file(REMOVE_RECURSE "${BIN}")
set(args -S "${SRC}" -B "${BIN}" -G "${GEN}" "-DVS_ROOT=${root}" "-DCMAKE_CXX_COMPILER=${CXX}"
    "-DCMAKE_CXX_FLAGS=${CXXFLAGS}")
if(SYSROOT)
  list(APPEND args "-DCMAKE_OSX_SYSROOT=${SYSROOT}")
endif()
execute_process(COMMAND ${CMAKE_COMMAND} ${args} RESULT_VARIABLE rc OUTPUT_QUIET ERROR_VARIABLE err)
if(NOT rc EQUAL 0)
  message(FATAL_ERROR "configuring with VS_ROOT=${root} failed:\n${err}")
endif()
execute_process(COMMAND ${CMAKE_COMMAND} --build "${BIN}" RESULT_VARIABLE rc OUTPUT_VARIABLE out ERROR_VARIABLE err)
if(NOT rc EQUAL 0)
  message(FATAL_ERROR "building with VS_ROOT=${root} failed:\n${out}\n${err}")
endif()
find_program(program print_root PATHS "${BIN}" "${BIN}/Debug" "${BIN}/Release" NO_DEFAULT_PATH)
execute_process(COMMAND "${program}" OUTPUT_VARIABLE printed RESULT_VARIABLE rc)
file(REMOVE_RECURSE "${BIN}")
if(NOT printed STREQUAL expected)
  message(FATAL_ERROR "VS_ROOT=${root} reached the program as ${printed}, not ${expected}")
endif()
message(STATUS "VS_ROOT=${root}: ${printed}")
