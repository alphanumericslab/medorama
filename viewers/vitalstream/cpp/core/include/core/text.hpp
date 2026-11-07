// Text helpers with Python's semantics, shared by the viewer and the readers.
#pragma once

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace vitalstream::core {

using Bytes = std::vector<std::uint8_t>;

// Lower-case hex text of bytes (Python's bytes.hex()).
std::string hex(const std::uint8_t* b, std::size_t n);
inline std::string hex(const Bytes& b) { return hex(b.data(), b.size()); }
// Python's str.strip() (Unicode whitespace) on UTF-8 text.
std::string py_strip(std::string_view s);
// Python's float(str): surrounding whitespace allowed (not U+001C..U+001F), one optional sign,
// inf/nan words, underscores between digits.
std::optional<double> py_float(std::string_view s);

}  // namespace vitalstream::core
