# Warnings for our own targets (not for the dependencies).
option(VS_WARNINGS_AS_ERRORS "warnings are errors (CI)" OFF)

function(vs_warnings target)
  if(MSVC)
    target_compile_options(${target} PRIVATE /W4 /permissive- /utf-8 /wd4244 /wd4267)
    target_compile_definitions(${target} PRIVATE _CRT_SECURE_NO_WARNINGS)
  else()
    target_compile_options(${target} PRIVATE -Wall -Wextra -Wpedantic -Wno-unused-parameter)
    if(CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
      # GCC's note that the ARM calling convention for some structs changed in GCC 10.1: informational
      target_compile_options(${target} PRIVATE -Wno-psabi)
      if(CMAKE_CXX_COMPILER_VERSION VERSION_LESS 13)
        # GCC 12 warns, wrongly, of overlapping copies in std::string's operator+ when optimizing
        # (GCC bug 105329, fixed in GCC 13)
        target_compile_options(${target} PRIVATE -Wno-restrict)
      endif()
    endif()
  endif()
  if(VS_WARNINGS_AS_ERRORS)
    target_compile_options(${target} PRIVATE $<IF:$<CXX_COMPILER_ID:MSVC>,/WX,-Werror>)
  endif()
endfunction()
