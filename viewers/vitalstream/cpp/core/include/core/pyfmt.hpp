// Text formatting identical to Python's, so that every implementation writes the same bytes:
// repr(float), '%.Ng' % x, round(x, n), f'{x:.Nf}', the shortest float32 text, UTC ISO times and
// CSV rows (Python's csv module, excel dialect). The counterpart of the Rust crate signal-core's
// pyfmt.
//
// Everything is built on std::to_chars / std::from_chars: correctly rounded, independent of
// the C locale, the same on every platform.
#pragma once

#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

namespace vitalstream::core {

std::string repr(double x);                       // Python repr(float)
std::string g(double x, int precision);           // Python '%.{precision}g' % x
std::string fixed(double x, int decimals);        // Python f'{x:.{decimals}f}'
double round_to(double x, int ndigits);           // round(x, ndigits) (correctly rounded)
std::string f32_short(double x);                  // shortest text that reads back as the same float32
std::string utc_iso(double unix_time);            // datetime.fromtimestamp(t, timezone.utc).isoformat()
void csv_field(std::string_view s, std::string& out);
std::string csv_row(const std::vector<std::string>& fields);  // with Python's "\r\n" line end

// Parsers (C locale, whole string): false when the text is not a number.
bool parse_double(std::string_view s, double& out);
bool parse_int(std::string_view s, std::int64_t& out);

// Civil date from days since 1970-01-01 (proleptic Gregorian).
struct CivilDate {
  int year;
  unsigned month;
  unsigned day;
};
CivilDate date_from_days(std::int64_t days);
std::int64_t days_from_date(int year, unsigned month, unsigned day);

}  // namespace vitalstream::core
