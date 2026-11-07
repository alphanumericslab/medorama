// A file's contents, memory-mapped, read-only: index arrays, and recordings read in place.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

namespace vitalstream::core {

// The file's contents, memory-mapped (read-only; empty files are not mapped).
class FileData {
 public:
  explicit FileData(const std::string& path);  // throws std::runtime_error ("PATH: reason")
  ~FileData();
  FileData(const FileData&) = delete;
  FileData& operator=(const FileData&) = delete;
  const std::uint8_t* data() const { return data_; }
  std::size_t size() const { return size_; }

 private:
  const std::uint8_t* data_ = nullptr;
  std::size_t size_ = 0;
#ifdef _WIN32
  void* file_ = nullptr;
  void* mapping_ = nullptr;
#endif
};

}  // namespace vitalstream::core
