add_executable(viewer_tests test_viewer.cpp test_edf.cpp test_limits.cpp test_server.cpp)
target_link_libraries(viewer_tests PRIVATE vs_viewer GTest::gtest_main)
# the synthetic recordings (viewers/vitalstream/testdata) and the mock adapters of the engine's
# tests (libs/engine/tests/fake_adapter.py, misbehaving_adapter.py)
get_filename_component(VS_TESTDATA "${CMAKE_CURRENT_SOURCE_DIR}/../../../testdata" ABSOLUTE)
get_filename_component(VS_FAKE_ADAPTER "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../libs/engine/tests/fake_adapter.py"
                       ABSOLUTE)
get_filename_component(VS_MISBEHAVING_ADAPTER
                       "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../libs/engine/tests/misbehaving_adapter.py" ABSOLUTE)
target_compile_definitions(viewer_tests PRIVATE VS_TESTDATA="${VS_TESTDATA}" VS_FAKE_ADAPTER="${VS_FAKE_ADAPTER}"
                           VS_MISBEHAVING_ADAPTER="${VS_MISBEHAVING_ADAPTER}")
vs_warnings(viewer_tests)
include(GoogleTest)
gtest_discover_tests(viewer_tests DISCOVERY_TIMEOUT 60)

# VS_ROOT with spaces, quotes and backslashes reaches the program as given (cmake/build_paths.cmake)
add_test(NAME BuildPaths.AnyRoot
         COMMAND ${CMAKE_COMMAND} -DSRC=${CMAKE_CURRENT_SOURCE_DIR}/build_paths
                 -DBIN=${CMAKE_CURRENT_BINARY_DIR}/build_paths_check "-DGEN=${CMAKE_GENERATOR}"
                 "-DCXX=${CMAKE_CXX_COMPILER}" "-DCXXFLAGS=${CMAKE_CXX_FLAGS}" "-DSYSROOT=${CMAKE_OSX_SYSROOT}"
                 -P ${CMAKE_CURRENT_SOURCE_DIR}/build_paths/check.cmake)
