// SHA-256 (FIPS 180-4): checksums of files and record samples, the index key, and the viewer's
// password hashing (HMAC-SHA-256, scrypt).
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <string>

namespace vitalstream::core {

class Sha256 {
 public:
  Sha256();
  void update(const void* data, std::size_t n);
  std::array<std::uint8_t, 32> digest() const;  // of everything so far (the state is kept)
  std::string hex_digest() const;

 private:
  void block(const std::uint8_t* p);
  std::array<std::uint32_t, 8> h_;
  std::array<std::uint8_t, 64> buf_{};
  std::size_t used_ = 0;
  std::uint64_t total_ = 0;
};

// SHA-256 of a whole file, as lower-case hex.  Throws std::runtime_error.
std::string sha256_file(const std::string& utf8_path);

}  // namespace vitalstream::core
