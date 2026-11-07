# A program built with a given VS_ROOT (cmake -DVS_ROOT=...): it prints VS_ROOT_PATH as the
# generated header states it (check.cmake compares it with the root given).
cmake_minimum_required(VERSION 3.24)
project(vs_build_paths_check CXX)
set(VS_ROOT "" CACHE STRING "")
include(${CMAKE_CURRENT_SOURCE_DIR}/../../../cmake/build_paths.cmake)
vs_build_paths("${CMAKE_CURRENT_BINARY_DIR}/generated/build_paths.hpp" "${VS_ROOT}" "${CMAKE_CURRENT_SOURCE_DIR}")
add_executable(print_root main.cpp)
target_include_directories(print_root PRIVATE ${CMAKE_CURRENT_BINARY_DIR}/generated)
