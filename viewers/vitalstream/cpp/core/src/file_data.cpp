// FileData (file_data.hpp): a file memory-mapped, read-only.
#include "core/file_data.hpp"

#include <cerrno>
#include <cstring>
#include <stdexcept>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace vitalstream::core {

#ifdef _WIN32
FileData::FileData(const std::string& path) {
  int wn = MultiByteToWideChar(CP_UTF8, 0, path.c_str(), -1, nullptr, 0);
  std::wstring wpath(static_cast<std::size_t>(wn > 0 ? wn : 1), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, path.c_str(), -1, wpath.data(), wn);
  HANDLE f = CreateFileW(wpath.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING,
                         FILE_ATTRIBUTE_NORMAL, nullptr);
  if (f == INVALID_HANDLE_VALUE) throw std::runtime_error(path + ": cannot open file");
  LARGE_INTEGER size;
  if (!GetFileSizeEx(f, &size)) {
    CloseHandle(f);
    throw std::runtime_error(path + ": cannot read file size");
  }
  file_ = f;
  size_ = static_cast<std::size_t>(size.QuadPart);
  if (size_ == 0) return;
  HANDLE m = CreateFileMappingW(f, nullptr, PAGE_READONLY, 0, 0, nullptr);
  if (!m) {
    CloseHandle(f);
    throw std::runtime_error(path + ": cannot map file");
  }
  mapping_ = m;
  data_ = static_cast<const std::uint8_t*>(MapViewOfFile(m, FILE_MAP_READ, 0, 0, 0));
  if (!data_) {
    CloseHandle(m);
    CloseHandle(f);
    throw std::runtime_error(path + ": cannot map file");
  }
}

FileData::~FileData() {
  if (data_) UnmapViewOfFile(data_);
  if (mapping_) CloseHandle(static_cast<HANDLE>(mapping_));
  if (file_) CloseHandle(static_cast<HANDLE>(file_));
}
#else
FileData::FileData(const std::string& path) {
  int fd = ::open(path.c_str(), O_RDONLY);
  if (fd < 0) throw std::runtime_error(path + ": " + std::strerror(errno));
  struct stat st;
  if (::fstat(fd, &st) != 0) {
    int e = errno;
    ::close(fd);
    throw std::runtime_error(path + ": " + std::strerror(e));
  }
  if (S_ISDIR(st.st_mode)) {
    ::close(fd);
    throw std::runtime_error(path + ": Is a directory");
  }
  size_ = static_cast<std::size_t>(st.st_size);
  if (size_ > 0) {
    void* p = ::mmap(nullptr, size_, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) {
      int e = errno;
      ::close(fd);
      throw std::runtime_error(path + ": " + std::strerror(e));
    }
    data_ = static_cast<const std::uint8_t*>(p);
  }
  ::close(fd);  // the mapping stays valid
}

FileData::~FileData() {
  if (data_) ::munmap(const_cast<std::uint8_t*>(data_), size_);
}
#endif

}  // namespace vitalstream::core
