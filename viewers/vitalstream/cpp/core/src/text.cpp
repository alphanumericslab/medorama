// Text helpers with Python's semantics (text.hpp).
#include "core/text.hpp"

#include <cmath>
#include <limits>

#include "core/pyfmt.hpp"
#include "core/value.hpp"

namespace vitalstream::core {

namespace {
const double NaN = std::numeric_limits<double>::quiet_NaN();
const double INF = std::numeric_limits<double>::infinity();
bool is_digit(char c) { return c >= '0' && c <= '9'; }
std::string lower_ascii(std::string_view s) {
  std::string out(s);
  for (char& c : out)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return out;
}
// Rust char::is_whitespace (Unicode White_Space) plus U+001C..U+001F, as Python's str.strip().
bool py_space(char32_t c) {
  if (c >= 0x09 && c <= 0x0d) return true;
  if (c >= 0x1c && c <= 0x20) return true;
  switch (c) {
    case 0x85: case 0xa0: case 0x1680: case 0x2028: case 0x2029: case 0x202f: case 0x205f: case 0x3000:
      return true;
    default:
      return c >= 0x2000 && c <= 0x200a;
  }
}

// Rust char::is_whitespace (Unicode White_Space): what float() strips, without U+001C..U+001F.
bool unicode_space(char32_t c) { return c != 0x1c && c != 0x1d && c != 0x1e && c != 0x1f && py_space(c); }

}  // namespace

std::string py_strip(std::string_view s) {
  auto cps = utf8_decode(s);
  std::size_t a = 0, b = cps.size();
  while (a < b && py_space(cps[a])) ++a;
  while (b > a && py_space(cps[b - 1])) --b;
  std::string out;
  for (std::size_t i = a; i < b; ++i) utf8_append(out, cps[i]);
  return out;
}

std::string hex(const std::uint8_t* b, std::size_t n) {
  static const char* H = "0123456789abcdef";
  std::string s;
  s.reserve(2 * n);
  for (std::size_t i = 0; i < n; ++i) {
    s += H[b[i] >> 4];
    s += H[b[i] & 15];
  }
  return s;
}

std::optional<double> py_float(std::string_view s) {
  // surrounding whitespace (not the separators U+001C..U+001F, which float() refuses although
  // str.strip() removes them), then one optional sign
  auto cps = utf8_decode(s);
  std::size_t a = 0, b = cps.size();
  while (a < b && unicode_space(cps[a])) ++a;
  while (b > a && unicode_space(cps[b - 1])) --b;
  std::string t;
  for (std::size_t i = a; i < b; ++i) utf8_append(t, cps[i]);
  if (t.empty()) return std::nullopt;
  std::string lower = lower_ascii(t);
  std::string_view body = lower;
  if (body[0] == '+' || body[0] == '-') body.remove_prefix(1);
  if (body == "inf" || body == "infinity" || body == "nan") {
    double v = body == "nan" ? NaN : INF;
    return lower[0] == '-' ? -v : v;
  }
  if (t.find('_') != std::string::npos) {
    for (std::size_t i = 0; i < t.size(); ++i)
      if (t[i] == '_' && !(i > 0 && i + 1 < t.size() && is_digit(t[i - 1]) && is_digit(t[i + 1])))
        return std::nullopt;
    std::string u;
    for (char c : t)
      if (c != '_') u += c;
    t = u;
  }
  for (char c : t)
    if (!(is_digit(c) || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')) return std::nullopt;
  if (t.size() > 1 && t[0] == '+' && (t[1] == '+' || t[1] == '-')) return std::nullopt;
  double x;
  if (parse_double(t, x)) return x;
  return std::nullopt;
}

}  // namespace vitalstream::core
