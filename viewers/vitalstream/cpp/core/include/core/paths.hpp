// File names are UTF-8 strings everywhere in the library (as in Python and Rust); these helpers
// convert them for the file system (wide names on Windows).
#pragma once

#include <cstdio>
#include <filesystem>
#include <string>

namespace vitalstream::core {

inline std::filesystem::path utf8_path(const std::string& s) {
  return std::filesystem::path(std::u8string(s.begin(), s.end()));
}

inline std::string path_utf8(const std::filesystem::path& p) {
  std::u8string u = p.u8string();
  return std::string(u.begin(), u.end());
}

// fopen with a UTF-8 file name.
inline std::FILE* open_file(const std::string& utf8_name, const char* mode) {
#ifdef _WIN32
  std::wstring wmode(mode, mode + std::char_traits<char>::length(mode));
  return _wfopen(utf8_path(utf8_name).c_str(), wmode.c_str());
#else
  return std::fopen(utf8_name.c_str(), mode);
#endif
}

}  // namespace vitalstream::core
