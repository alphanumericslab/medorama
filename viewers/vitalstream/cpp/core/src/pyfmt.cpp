// Number and time formatting identical to Python's (pyfmt.hpp; signal-core's pyfmt.rs).
#include "core/pyfmt.hpp"

#include <charconv>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <system_error>

namespace vitalstream::core {

namespace {

std::string to_chars_str(double x, std::chars_format fmt, int precision = -1) {
  char buf[512];
  std::to_chars_result r = precision < 0 ? std::to_chars(buf, buf + sizeof buf, x, fmt)
                                         : std::to_chars(buf, buf + sizeof buf, x, fmt, precision);
  return std::string(buf, r.ptr);
}

std::string strip_zeros(std::string s) {
  if (s.find('.') == std::string::npos) return s;
  while (!s.empty() && s.back() == '0') s.pop_back();
  if (!s.empty() && s.back() == '.') s.pop_back();
  return s;
}

}  // namespace

bool parse_double(std::string_view s, double& out) {
  if (s.empty()) return false;
#if defined(__cpp_lib_to_chars) && __cpp_lib_to_chars >= 201611L
  const char* first = s.data();
  if (*first == '+') ++first;  // from_chars rejects a leading '+'; Python's float() accepts it
  auto r = std::from_chars(first, s.data() + s.size(), out);
  if (r.ec == std::errc::result_out_of_range) {
    // Python: overflow -> inf, underflow -> 0 (with the sign)
    std::string t(s);
    out = std::strtod(t.c_str(), nullptr);
    return true;
  }
  return r.ec == std::errc() && r.ptr == s.data() + s.size();
#else
  std::string t(s);
  char* end = nullptr;
  out = std::strtod(t.c_str(), &end);  // the program never changes the C locale
  return end == t.c_str() + t.size();
#endif
}

bool parse_int(std::string_view s, std::int64_t& out) {
  if (s.empty()) return false;
  const char* first = s.data();
  if (*first == '+') ++first;
  auto r = std::from_chars(first, s.data() + s.size(), out);
  return r.ec == std::errc() && r.ptr == s.data() + s.size();
}

std::string repr(double x) {
  if (std::isnan(x)) return "nan";
  if (std::isinf(x)) return x > 0 ? "inf" : "-inf";
  if (x == 0.0) return std::signbit(x) ? "-0.0" : "0.0";
  // Shortest round-trip digits (correctly rounded, ties to even) in scientific form.
  std::string s = to_chars_str(x, std::chars_format::scientific);
  bool neg = s[0] == '-';
  std::size_t e = s.find('e');
  std::string mant = s.substr(neg ? 1 : 0, e - (neg ? 1 : 0));
  int exp = std::atoi(s.c_str() + e + 1);
  std::string digits;
  for (char c : mant)
    if (c != '.') digits += c;
  while (digits.size() > 1 && digits.back() == '0') digits.pop_back();
  int decpt = exp + 1;  // value = 0.d1d2... x 10^decpt
  std::string out = neg ? "-" : "";
  int n = static_cast<int>(digits.size());
  if (-4 < decpt && decpt <= 16) {
    if (decpt <= 0) {
      out += "0.";
      out.append(static_cast<std::size_t>(-decpt), '0');
      out += digits;
    } else if (decpt >= n) {
      out += digits;
      out.append(static_cast<std::size_t>(decpt - n), '0');
      out += ".0";
    } else {
      out += digits.substr(0, decpt);
      out += '.';
      out += digits.substr(decpt);
    }
  } else {
    out += digits[0];
    if (n > 1) {
      out += '.';
      out += digits.substr(1);
    }
    int ee = decpt - 1;
    char buf[16];
    std::snprintf(buf, sizeof buf, "e%c%02d", ee < 0 ? '-' : '+', ee < 0 ? -ee : ee);
    out += buf;
  }
  return out;
}

std::string g(double x, int precision) {
  if (std::isnan(x)) return "nan";
  if (std::isinf(x)) return x > 0 ? "inf" : "-inf";
  int p = precision < 1 ? 1 : precision;
  // Exponent after rounding to p significant digits, as printf decides %g.
  std::string e_text = to_chars_str(x, std::chars_format::scientific, p - 1);
  std::size_t epos = e_text.find('e');
  int exp = std::atoi(e_text.c_str() + epos + 1);
  if (exp < -4 || exp >= p) {
    std::string mant = strip_zeros(e_text.substr(0, epos));
    char buf[16];
    std::snprintf(buf, sizeof buf, "e%c%02d", exp < 0 ? '-' : '+', exp < 0 ? -exp : exp);
    return mant + buf;
  }
  int decimals = p - 1 - exp;
  if (decimals < 0) decimals = 0;
  return strip_zeros(to_chars_str(x, std::chars_format::fixed, decimals));
}

std::string fixed(double x, int decimals) {
  if (std::isnan(x)) return "nan";
  if (std::isinf(x)) return x > 0 ? "inf" : "-inf";
  return to_chars_str(x, std::chars_format::fixed, decimals);
}

double round_to(double x, int ndigits) {
  if (!std::isfinite(x)) return x;
  double v;
  return parse_double(fixed(x, ndigits), v) ? v : x;
}

std::string f32_short(double x) {
  if (std::isnan(x)) return "NaN";
  if (std::isinf(x)) return x > 0 ? "INF" : "-INF";
  float target = static_cast<float>(x);
  std::uint32_t tb;
  std::memcpy(&tb, &target, 4);
  for (int p = 6; p < 10; ++p) {
    std::string s = g(x, p);
    double v;
    if (parse_double(s, v)) {
      float f = static_cast<float>(v);
      std::uint32_t fb;
      std::memcpy(&fb, &f, 4);
      if (fb == tb) return s;
    }
  }
  return repr(x);
}

CivilDate date_from_days(std::int64_t days) {
  std::int64_t z = days + 719468;
  std::int64_t era = (z >= 0 ? z : z - 146096) / 146097;
  std::int64_t doe = z - era * 146097;
  std::int64_t yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
  std::int64_t y = yoe + era * 400;
  std::int64_t doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  std::int64_t mp = (5 * doy + 2) / 153;
  std::int64_t d = doy - (153 * mp + 2) / 5 + 1;
  std::int64_t m = mp < 10 ? mp + 3 : mp - 9;
  return CivilDate{static_cast<int>(y + (m <= 2)), static_cast<unsigned>(m), static_cast<unsigned>(d)};
}

std::int64_t days_from_date(int year, unsigned month, unsigned day) {
  std::int64_t y = year - (month <= 2);
  std::int64_t era = (y >= 0 ? y : y - 399) / 400;
  std::int64_t yoe = y - era * 400;
  std::int64_t mp = month > 2 ? month - 3 : month + 9;
  std::int64_t doy = (153 * mp + 2) / 5 + day - 1;
  std::int64_t doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  return era * 146097 + doe - 719468;
}

std::string utc_iso(double t) {
  double secs = std::trunc(t);
  double us = std::nearbyint((t - secs) * 1e6);  // default rounding mode: half to even
  if (us >= 1e6) {
    us -= 1e6;
    secs += 1.0;
  } else if (us < 0.0) {
    us += 1e6;
    secs -= 1.0;
  }
  auto s = static_cast<std::int64_t>(secs);
  std::int64_t day = s >= 0 ? s / 86400 : -((-s + 86399) / 86400);
  std::int64_t sod = s - day * 86400;
  CivilDate d = date_from_days(day);
  char buf[64];
  std::snprintf(buf, sizeof buf, "%04d-%02u-%02uT%02lld:%02lld:%02lld", d.year, d.month, d.day,
                static_cast<long long>(sod / 3600), static_cast<long long>(sod / 60 % 60),
                static_cast<long long>(sod % 60));
  std::string out = buf;
  if (us != 0.0) {
    std::snprintf(buf, sizeof buf, ".%06lld", static_cast<long long>(us));
    out += buf;
  }
  return out + "+00:00";
}

void csv_field(std::string_view s, std::string& out) {
  if (s.find_first_of(",\"\r\n") != std::string_view::npos) {
    out += '"';
    for (char c : s) {
      if (c == '"') out += '"';
      out += c;
    }
    out += '"';
  } else {
    out += s;
  }
}

std::string csv_row(const std::vector<std::string>& fields) {
  if (fields.size() == 1 && fields[0].empty()) return "\"\"\r\n";
  std::string out;
  for (std::size_t i = 0; i < fields.size(); ++i) {
    if (i) out += ',';
    csv_field(fields[i], out);
  }
  out += "\r\n";
  return out;
}

}  // namespace vitalstream::core
