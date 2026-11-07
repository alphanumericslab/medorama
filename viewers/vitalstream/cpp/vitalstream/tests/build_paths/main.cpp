// Prints the root the program was built with (the generated build_paths.hpp).
#include <cstdio>

#include "build_paths.hpp"

int main() {
  std::fputs(VS_ROOT_PATH, stdout);
  return 0;
}
