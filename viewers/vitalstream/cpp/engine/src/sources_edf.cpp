// EDF, EDF+ and BDF files (European Data Format, https://www.edfplus.info/specs/): one file is one
// recording (docs/edf.md, "Reading rules (for implementers)").  Mirrors the Rust engine's
// sources/edf.rs.
//
// - Each ordinary signal is a channel (its label and physical dimension), at
//   samples per data record / duration of a data record Hz; physical values are
//   pmin + (digital - dmin) * (pmax - pmin) / (dmax - dmin) (the digital values themselves when
//   dmin == dmax).  BDF and BDF+ files hold 24-bit samples, EDF and EDF+ 16-bit.
// - Data records follow each other from the header's start time (EDF), or from the onsets the
//   EDF+ time-keeping annotations state (EDF+C and EDF+D): a record that does not start where the
//   one before it ends begins a new piece of every channel, with a break between them.
// - The annotations of EDF+ and BDF+ files (EDF Annotations, BDF Annotations signals) are events
//   with their onset, duration and text.
// - The header's patient and recording identification fields are never part of the index: they
//   hold the patient's identity.
#include <algorithm>
#include <charconv>
#include <cmath>
#include <cstdio>
#include <stdexcept>
#include <unordered_map>

#include "engine/sources.hpp"
#include "core/paths.hpp"
#include "core/text.hpp"

namespace vitalstream::engine {

namespace {

// Records read at once while indexing (at most this many bytes).
constexpr std::size_t BATCH_BYTES = 1 << 22;
// Records closer than this (seconds) to where the one before ends follow it without a break.
constexpr double CONTIGUOUS_S = 1e-6;

struct Signal {
  std::string label, unit;
  double pmin = 0, pmax = 0, dmin = 0, dmax = 0;
  std::size_t spr = 0;       // samples per data record
  bool annotations = false;  // an EDF Annotations (BDF Annotations) signal
  std::size_t offset = 0;    // where its samples start in a data record (bytes)

  double physical(std::int32_t d) const {
    if (dmax == dmin) return static_cast<double>(d);
    double gain = (pmax - pmin) / (dmax - dmin);
    return pmin + (static_cast<double>(d) - dmin) * gain;
  }
};

// What the header says (the identification fields left out).
struct Header {
  bool bdf = false;                 // 24-bit samples (BDF, BDF+)
  std::optional<char> plus;         // EDF+ or BDF+ ('C' continuous, 'D' discontinuous)
  std::optional<double> start;      // Unix seconds, the header's clock time taken as UTC
  std::uint64_t header_bytes = 0;
  std::int64_t declared = 0;        // data records stated in the header (-1: unknown)
  std::uint64_t records = 0;        // data records in the file (whole ones)
  std::uint64_t rest = 0;           // bytes after the last whole record
  double duration = 0;              // seconds per data record
  std::vector<Signal> signals;
  std::size_t record_bytes = 0;
};

// A file read at given offsets.
class File {
 public:
  explicit File(const fs::path& p) : name_(core::path_utf8(p)), f_(core::open_file(name_, "rb")) {
    if (!f_) throw std::runtime_error(name_ + ": cannot open");
  }
  ~File() { std::fclose(f_); }
  File(const File&) = delete;
  File& operator=(const File&) = delete;

  // Up to n bytes from offset (fewer at the end of the file).
  std::size_t read(std::uint64_t offset, std::uint8_t* out, std::size_t n) {
#ifdef _WIN32
    int bad = _fseeki64(f_, static_cast<__int64>(offset), SEEK_SET);
#else
    int bad = fseeko(f_, static_cast<off_t>(offset), SEEK_SET);
#endif
    if (bad) throw std::runtime_error(name_ + ": cannot seek");
    return n ? std::fread(out, 1, n, f_) : 0;
  }

 private:
  std::string name_;
  std::FILE* f_;
};

// Rust's str::fmt::Debug, for the messages: the text in double quotes, control characters escaped.
std::string debug_text(const std::string& s) {
  static const char* H = "0123456789abcdef";
  std::string out = "\"";
  for (char32_t c : core::utf8_decode(s)) {
    switch (c) {
      case '"': out += "\\\""; break;
      case '\\': out += "\\\\"; break;
      case '\n': out += "\\n"; break;
      case '\r': out += "\\r"; break;
      case '\t': out += "\\t"; break;
      case 0: out += "\\0"; break;
      default:
        if (c < 0x20 || (c >= 0x7f && c < 0xa0)) {
          out += "\\u{";
          if (c >= 0x10) out += H[c >> 4];
          out += H[c & 15];
          out += "}";
        } else {
          core::utf8_append(out, c);
        }
    }
  }
  return out + "\"";
}

// Rust's f64 Display (the shortest digits, never an exponent), for the messages.
std::string display(double x) {
  char buf[64];
  auto r = std::to_chars(buf, buf + sizeof buf, x, std::chars_format::scientific);
  std::string s(buf, r.ptr);
  bool neg = !s.empty() && s[0] == '-';
  if (neg) s.erase(0, 1);
  std::size_t e = s.find('e');
  int exp = std::atoi(s.c_str() + e + 1);
  std::string digits;
  for (char c : s.substr(0, e))
    if (c != '.') digits += c;
  long point = exp + 1;  // digits before the decimal point
  auto nd = static_cast<long>(digits.size());
  std::string out;
  if (point <= 0)
    out = "0." + std::string(static_cast<std::size_t>(-point), '0') + digits;
  else if (point >= nd)
    out = digits + std::string(static_cast<std::size_t>(point - nd), '0');
  else
    out = digits.substr(0, static_cast<std::size_t>(point)) + "." + digits.substr(static_cast<std::size_t>(point));
  return (neg ? "-" : "") + out;
}

// A header field: its bytes as Latin-1 text (here in UTF-8), without the spaces around it.
std::string field(const std::uint8_t* b, std::size_t n) {
  std::string s;
  for (std::size_t i = 0; i < n; ++i) core::utf8_append(s, b[i]);
  return core::py_strip(s);
}

double number(const std::uint8_t* b, std::size_t n, const char* what) {
  std::string s = field(b, n);
  auto v = core::py_float(s);
  if (!v || !std::isfinite(*v))
    throw std::runtime_error(std::string("EDF header: ") + what + " is not a number (" + debug_text(s) + ")");
  return *v;
}

// Rust's str::parse::<i64>(): an optional sign, then ASCII digits only.
std::optional<std::int64_t> parse_i64(const std::string& s) {
  std::size_t i = (!s.empty() && (s[0] == '+' || s[0] == '-')) ? 1 : 0;
  if (i == s.size()) return std::nullopt;
  for (std::size_t k = i; k < s.size(); ++k)
    if (s[k] < '0' || s[k] > '9') return std::nullopt;
  const char* a = s.data() + (s[0] == '+' ? 1 : 0);
  std::int64_t v = 0;
  auto r = std::from_chars(a, s.data() + s.size(), v);
  if (r.ec != std::errc() || r.ptr != s.data() + s.size()) return std::nullopt;
  return v;
}

std::int64_t integer(const std::uint8_t* b, std::size_t n, const char* what) {
  std::string s = field(b, n);
  auto v = parse_i64(s);
  if (!v)
    throw std::runtime_error(std::string("EDF header: ") + what + " is not a whole number (" + debug_text(s) + ")");
  return *v;
}

// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
std::int64_t days_from_civil(std::int64_t y, std::int64_t m, std::int64_t d) {
  y = m <= 2 ? y - 1 : y;
  std::int64_t era = (y >= 0 ? y : y - 399) / 400;
  std::int64_t yoe = y - era * 400;
  std::int64_t mp = (m + 9) % 12;
  std::int64_t doy = (153 * mp + 2) / 5 + d - 1;
  std::int64_t doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  return era * 146097 + doe - 719468;
}

bool digit(char c) { return c >= '0' && c <= '9'; }

// Three two-digit numbers aa?bb?cc (any separators), as EDF writes dates and times; the third may
// be anything when loose (the year of an EDF+ date after 2084 is yy).  s: UTF-8, read as bytes.
struct Three {
  std::int64_t a, b;
  std::optional<std::int64_t> c;
};
std::optional<Three> three(const std::string& s, bool loose) {
  auto digits = [&](std::size_t i) { return digit(s[i]) && digit(s[i + 1]); };
  if (s.size() != 8 || !digits(0) || !digits(3) || digit(s[2]) || digit(s[5])) return std::nullopt;
  auto n = [&](std::size_t i) { return static_cast<std::int64_t>((s[i] - '0') * 10 + (s[i + 1] - '0')); };
  if (digits(6)) return Three{n(0), n(3), n(6)};
  if (loose) return Three{n(0), n(3), std::nullopt};
  return std::nullopt;
}

// The four-digit year of an EDF+ recording field ("Startdate 02-MAR-2002 ..."), if it states one.
std::optional<std::int64_t> plus_year(const std::string& recording) {
  std::size_t sp = recording.find(' ');
  if (sp == std::string::npos || recording.compare(0, sp, "Startdate") != 0) return std::nullopt;
  std::size_t end = recording.find(' ', sp + 1);
  std::string date = recording.substr(sp + 1, end == std::string::npos ? std::string::npos : end - sp - 1);
  if (date.size() != 11) return std::nullopt;
  for (std::size_t i = 7; i < 11; ++i)
    if (!digit(date[i])) return std::nullopt;
  return std::stoll(date.substr(7));
}

std::uint64_t size_of(const fs::path& p) {
  std::error_code ec;
  std::uintmax_t n = fs::file_size(p, ec);
  if (ec) throw std::runtime_error(core::path_utf8(p) + ": " + ec.message());
  return static_cast<std::uint64_t>(n);
}

}  // namespace

namespace edf {

std::optional<double> start_of(const std::string& date, const std::string& time, const std::string* recording) {
  std::optional<std::int64_t> full = recording ? plus_year(*recording) : std::nullopt;
  auto dmy = three(date, full.has_value());
  if (!dmy) return std::nullopt;
  auto hms = three(time, false);
  if (!hms || !hms->c) return std::nullopt;
  std::int64_t d = dmy->a, m = dmy->b, hh = hms->a, mi = hms->b, ss = *hms->c;
  std::int64_t year;
  if (full)
    year = *full;
  else if (dmy->c)
    year = *dmy->c >= 85 ? 1900 + *dmy->c : 2000 + *dmy->c;
  else
    return std::nullopt;
  if (m < 1 || m > 12 || d < 1 || d > 31 || hh > 23 || mi > 59 || ss > 59) return std::nullopt;
  return static_cast<double>(days_from_civil(year, m, d) * 86400 + hh * 3600 + mi * 60 + ss);
}

namespace {

// Unicode White_Space plus U+001C..U+001F: what Python's str.split() splits on.
bool space(char32_t c) {
  if (c >= 0x09 && c <= 0x0d) return true;
  if (c >= 0x1c && c <= 0x20) return true;
  switch (c) {
    case 0x85: case 0xa0: case 0x1680: case 0x2028: case 0x2029: case 0x202f: case 0x205f: case 0x3000:
      return true;
    default:
      return c >= 0x2000 && c <= 0x200a;
  }
}

// UTF-8 decoded with each invalid sequence (its maximal subpart) replaced by U+FFFD, as Rust's
// String::from_utf8_lossy and Python's bytes.decode('utf-8', 'replace').
std::vector<char32_t> lossy(std::string_view s) {
  std::vector<char32_t> out;
  const auto b = [&](std::size_t k) { return static_cast<unsigned char>(s[k]); };
  for (std::size_t i = 0; i < s.size();) {
    unsigned char c0 = b(i);
    if (c0 < 0x80) {
      out.push_back(c0);
      ++i;
      continue;
    }
    std::size_t n;
    unsigned char lo = 0x80, hi = 0xbf;  // the range of the second byte
    char32_t c;
    if (c0 >= 0xc2 && c0 <= 0xdf) {
      n = 2, c = c0 & 0x1f;
    } else if (c0 >= 0xe0 && c0 <= 0xef) {
      n = 3, c = c0 & 0x0f;
      if (c0 == 0xe0) lo = 0xa0;
      if (c0 == 0xed) hi = 0x9f;
    } else if (c0 >= 0xf0 && c0 <= 0xf4) {
      n = 4, c = c0 & 0x07;
      if (c0 == 0xf0) lo = 0x90;
      if (c0 == 0xf4) hi = 0x8f;
    } else {
      out.push_back(0xfffd);
      ++i;
      continue;
    }
    std::size_t k = 1;
    for (; k < n && i + k < s.size(); ++k) {
      unsigned char x = b(i + k);
      if (k == 1 ? (x < lo || x > hi) : (x < 0x80 || x > 0xbf)) break;
      c = (c << 6) | (x & 0x3f);
    }
    out.push_back(k == n ? c : 0xfffd);
    i += k;
  }
  return out;
}

// Python's ' '.join(text.split()) of the UTF-8 decoded text (invalid bytes replaced).
std::string words(std::string_view bytes) {
  std::string out;
  bool gap = false;
  for (char32_t c : lossy(bytes)) {
    if (space(c)) {
      gap = !out.empty();
      continue;
    }
    if (gap) out += ' ';
    gap = false;
    core::utf8_append(out, c);
  }
  return out;
}

std::optional<double> tal_number(std::string_view b) {
  if (!core::utf8_valid(b)) return std::nullopt;
  auto v = core::py_float(b);
  if (!v || !std::isfinite(*v)) return std::nullopt;
  return v;
}

}  // namespace

std::vector<Tal> tals(std::string_view bytes) {
  std::vector<Tal> out;
  std::size_t i = 0;
  while (i <= bytes.size()) {
    std::size_t z = bytes.find('\0', i);
    if (z == std::string_view::npos) z = bytes.size();
    std::string_view tal = bytes.substr(i, z - i);
    i = z + 1;
    if (tal.empty()) continue;
    std::vector<std::string_view> parts;
    for (std::size_t a = 0;;) {
      std::size_t e = tal.find('\x14', a);
      parts.push_back(tal.substr(a, e == std::string_view::npos ? std::string_view::npos : e - a));
      if (e == std::string_view::npos) break;
      a = e + 1;
    }
    std::string_view time = parts[0];
    std::size_t d = time.find('\x15');
    auto onset = tal_number(time.substr(0, d));
    if (!onset) continue;
    std::optional<double> duration;
    if (d != std::string_view::npos) {
      std::string_view rest = time.substr(d + 1);
      duration = tal_number(rest.substr(0, rest.find('\x15')));
    }
    Tal t{*onset, duration, {}};
    for (std::size_t k = 1; k < parts.size(); ++k) t.texts.push_back(words(parts[k]));
    out.push_back(std::move(t));
  }
  return out;
}

}  // namespace edf

namespace {

using edf::Tal;

Header header(const fs::path& path) {
  const std::string name = core::path_utf8(path);
  std::uint64_t size = size_of(path);
  File f(path);
  std::uint8_t fixed[256];
  if (f.read(0, fixed, 256) != 256) throw std::runtime_error("not an EDF or BDF file: " + name);
  Header h;
  h.bdf = fixed[0] == 0xFF && std::string_view(reinterpret_cast<const char*>(fixed + 1), 7) == "BIOSEMI";
  if (!h.bdf && field(fixed, 8) != "0") throw std::runtime_error("not an EDF or BDF file: " + name);
  // the signal fields, when the number of signals is one (read before the fields are checked)
  std::vector<std::uint8_t> rest;
  {
    std::string t = field(fixed + 252, 4);
    std::string digits = !t.empty() && t[0] == '+' ? t.substr(1) : t;
    if (!digits.empty() && digits.size() <= 4 &&
        std::all_of(digits.begin(), digits.end(), [](char c) { return c >= '0' && c <= '9'; })) {
      std::size_t ns = std::stoul(digits);
      if (ns >= 1) {
        rest.resize(256 * ns);
        if (f.read(256, rest.data(), rest.size()) != rest.size())
          throw std::runtime_error("EDF header cut short: " + name);
      }
    }
  }
  std::string reserved = field(fixed + 192, 44);
  for (const char* p : {"EDF+C", "EDF+D", "BDF+C", "BDF+D"})
    if (reserved.rfind(p, 0) == 0) {
      h.plus = p[4];
      break;
    }
  std::string recording = field(fixed + 88, 80);
  h.start = edf::start_of(field(fixed + 168, 8), field(fixed + 176, 8), h.plus ? &recording : nullptr);
  std::int64_t header_bytes = integer(fixed + 184, 8, "the number of header bytes");
  h.declared = integer(fixed + 236, 8, "the number of data records");
  h.duration = number(fixed + 244, 8, "the duration of a data record");
  std::int64_t ns = integer(fixed + 252, 4, "the number of signals");
  if (ns < 1 || ns > 9999 || header_bytes != 256 * (ns + 1))
    throw std::runtime_error("EDF header: " + std::to_string(ns) + " signals do not fit its " +
                             std::to_string(header_bytes) + " header bytes: " + name);
  auto n = static_cast<std::size_t>(ns);
  if (rest.size() != 256 * n) throw std::runtime_error("EDF header cut short: " + name);
  // the signal fields, each for every signal in turn
  const std::size_t widths[10] = {16, 80, 8, 8, 8, 8, 8, 80, 8, 32};
  std::size_t starts[10];
  for (std::size_t c = 0, at = 0; c < 10; ++c) {
    starts[c] = at;
    at += widths[c] * n;
  }
  auto col = [&](std::size_t c, std::size_t k) { return rest.data() + starts[c] + k * widths[c]; };
  const std::size_t width = h.bdf ? 3 : 2;
  std::size_t offset = 0;
  for (std::size_t k = 0; k < n; ++k) {
    Signal s;
    s.label = field(col(0, k), 16);
    std::int64_t spr = integer(col(8, k), 8, "a number of samples per data record");
    if (spr < 0)
      throw std::runtime_error("EDF header: signal " + std::to_string(k + 1) + " has " + std::to_string(spr) +
                               " samples per data record");
    s.annotations = s.label == "EDF Annotations" || s.label == "BDF Annotations";
    s.unit = field(col(2, k), 8);
    s.pmin = number(col(3, k), 8, "a physical minimum");
    s.pmax = number(col(4, k), 8, "a physical maximum");
    s.dmin = number(col(5, k), 8, "a digital minimum");
    s.dmax = number(col(6, k), 8, "a digital maximum");
    s.spr = static_cast<std::size_t>(spr);
    s.offset = offset;
    offset += s.spr * width;
    h.signals.push_back(std::move(s));
  }
  h.record_bytes = offset;
  h.header_bytes = static_cast<std::uint64_t>(header_bytes);
  std::uint64_t body = size > h.header_bytes ? size - h.header_bytes : 0;
  if (h.record_bytes == 0) {
    h.records = 0;
    h.rest = body;
  } else {
    h.records = body / h.record_bytes;
    h.rest = body % h.record_bytes;
  }
  if (h.declared >= 0) h.records = std::min(h.records, static_cast<std::uint64_t>(h.declared));
  bool ordinary =
      std::any_of(h.signals.begin(), h.signals.end(), [](const Signal& s) { return !s.annotations && s.spr > 0; });
  if (!(h.duration > 0.0) && ordinary)
    throw std::runtime_error("EDF header: a data record lasts " + display(h.duration) + " s: " + name);
  // a sampling rate that is not a finite number (a data record of 1e-320 s)
  for (const Signal& s : h.signals)
    if (!s.annotations && s.spr > 0 && !std::isfinite(static_cast<double>(s.spr) / h.duration))
      throw std::runtime_error("EDF header: signal " + s.label + " has no finite sampling rate (" +
                               std::to_string(s.spr) + " samples in " + display(h.duration) + " s): " + name);
  return h;
}

// A record's samples of one signal, as digital values.
void digital(const std::uint8_t* rec, const Signal& s, bool bdf, std::vector<std::int32_t>& out) {
  out.resize(s.spr);
  const std::uint8_t* b = rec + s.offset;
  if (bdf) {
    for (std::size_t i = 0; i < s.spr; ++i) {
      auto u = static_cast<std::uint32_t>(b[3 * i]) | (static_cast<std::uint32_t>(b[3 * i + 1]) << 8) |
               (static_cast<std::uint32_t>(b[3 * i + 2]) << 16);
      out[i] = static_cast<std::int32_t>(u ^ 0x800000u) - 0x800000;
    }
  } else {
    for (std::size_t i = 0; i < s.spr; ++i)
      out[i] = static_cast<std::int16_t>(static_cast<std::uint16_t>(b[2 * i] | (b[2 * i + 1] << 8)));
  }
}

std::vector<std::uint8_t> read_records(File& f, const Header& h, std::uint64_t first, std::uint64_t count,
                                       const std::string& name) {
  std::vector<std::uint8_t> buf(static_cast<std::size_t>(count) * h.record_bytes);
  if (f.read(h.header_bytes + first * h.record_bytes, buf.data(), buf.size()) != buf.size())
    throw std::runtime_error(name + ": failed to fill whole buffer");
  return buf;
}

}  // namespace

bool is_edf(const fs::path& p) {
  std::error_code ec;
  if (!fs::is_regular_file(p, ec)) return false;
  std::string e = core::path_utf8(p.extension());
  for (char& c : e)
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
  return e == ".edf" || e == ".bdf";
}

const char* edf_format_label(const fs::path& path) {
  try {
    Header h = header(path);
    if (h.bdf) return h.plus ? "BDF+" : "BDF";
    return h.plus ? "EDF+" : "EDF";
  } catch (const std::exception&) {
    return "EDF";
  }
}

Built build_edf(const fs::path& path, const Progress& progress, const Cancelled& cancelled) {
  const std::string name = core::path_utf8(path);
  Header h = header(path);
  std::vector<std::size_t> data, notes_sig;
  for (std::size_t k = 0; k < h.signals.size(); ++k) {
    if (!h.signals[k].annotations && h.signals[k].spr > 0) data.push_back(k);
    if (h.signals[k].annotations) notes_sig.push_back(k);
  }
  if (data.empty()) throw std::runtime_error(name + " holds no signals, only annotations");
  if (h.records == 0) throw std::runtime_error(name + " holds no data records");
  const double start = h.start.value_or(0.0);
  const bool timed = h.start.has_value();
  const std::size_t width = h.bdf ? 3 : 2;

  // channels, in the order of the signals; a label repeated gets its count ("ECG (2)")
  Channels channels;
  std::unordered_map<std::string, std::size_t> seen;
  std::vector<std::size_t> ch;
  for (std::size_t k : data) {
    const Signal& s = h.signals[k];
    std::size_t n = ++seen[s.label];
    std::string label = n == 1 ? s.label : s.label + " (" + std::to_string(n) + ")";
    ch.push_back(channels.get("edf\x01" + std::to_string(k), label, s.unit, "edf"));
  }

  Pieces pieces;
  std::vector<std::pair<std::size_t, std::uint64_t>> info;  // per piece: signal, first data record
  std::vector<std::size_t> current;                          // the pieces of the current run
  std::vector<std::pair<double, double>> runs;               // [start, end) of each run, from the file's start
  struct Event {
    double t, d;
    std::string text;
  };
  std::vector<Event> events;
  File file(path);
  const std::uint64_t batch = std::max<std::uint64_t>(BATCH_BYTES / std::max<std::size_t>(h.record_bytes, 1), 1);
  std::vector<std::int32_t> dig;
  std::vector<float> x;
  for (std::uint64_t k = 0; k < h.records;) {
    if (cancelled()) throw std::runtime_error("indexing cancelled");
    std::uint64_t count = std::min(batch, h.records - k);
    auto buf = read_records(file, h, k, count, name);
    for (std::uint64_t j = 0; j < count; ++j) {
      const std::uint8_t* rec = buf.data() + j * h.record_bytes;
      std::uint64_t r = k + j;
      // the record's onset: its time-keeping annotation (EDF+), else where it follows on
      std::optional<double> onset;
      for (std::size_t i = 0; i < notes_sig.size(); ++i) {
        const Signal& s = h.signals[notes_sig[i]];
        auto list = edf::tals(std::string_view(reinterpret_cast<const char*>(rec + s.offset), s.spr * width));
        for (std::size_t n = 0; n < list.size(); ++n) {
          Tal& t = list[n];
          if (i == 0 && n == 0 && h.plus) onset = t.onset;
          for (auto& text : t.texts)
            if (!text.empty())
              events.push_back({t.onset, t.duration && *t.duration > 0.0 ? *t.duration : 0.0, std::move(text)});
        }
      }
      std::optional<double> follows = runs.empty() ? std::nullopt : std::optional<double>(runs.back().second);
      double on = onset ? *onset : follows.value_or(0.0);
      if (!follows || std::abs(on - *follows) > CONTIGUOUS_S) {
        pieces.room(data.size());
        runs.emplace_back(on, on);
        current.clear();
        for (std::size_t q = 0; q < data.size(); ++q) {
          info.emplace_back(data[q], r);
          current.push_back(pieces.start(ch[q], static_cast<double>(h.signals[data[q]].spr) / h.duration,
                                         Dict{{"t0", Value(start + on)}, {"timed", Value(timed)}}));
        }
      }
      runs.back().second = on + h.duration;
      for (std::size_t q = 0; q < data.size(); ++q) {
        const Signal& s = h.signals[data[q]];
        digital(rec, s, h.bdf, dig);
        x.resize(dig.size());
        for (std::size_t i = 0; i < dig.size(); ++i) x[i] = static_cast<float>(s.physical(dig[i]));
        pieces.add(current[q], x.data(), x.size());
      }
    }
    k += count;
    progress(static_cast<double>(k) / static_cast<double>(h.records));
  }

  auto fin = pieces.finish();
  auto& items = fin.items;
  for (std::size_t p = 0; p < items.size(); ++p) {
    items[p].insert("sig", Value(info[p].first));
    items[p].insert("data_record", Value(static_cast<std::int64_t>(info[p].second)));
  }
  List gaps;
  for (std::size_t i = 1; i < runs.size(); ++i) {
    double a = start + runs[i - 1].second, b = start + runs[i].first;
    if (b > a)
      gaps.push_back(Value(Dict{{"t0", Value(a)},
                                {"t1", Value(b)},
                                {"reason", Value("not recorded")},
                                {"gap_s", Value(b - a)},
                                {"exact", Value(true)},
                                {"measured_by", Value()},
                                {"notes", Value("")}}));
  }
  std::vector<Dict> alarms;
  for (auto& e : events)
    alarms.push_back(Dict{{"t0", Value(start + e.t)},
                          {"t1", Value(start + e.t + e.d)},
                          {"text", Value(std::move(e.text))},
                          {"level", Value()},
                          {"kind", Value("annotation")}});
  sort_by_t0(alarms);

  List notes;
  notes.push_back(Value(timed ? "The file's start time is taken as UTC."
                              : "The file states no valid start time: times are counted from its start, and marked "
                                "\"time not stated\"."));
  if (h.declared >= 0 && static_cast<std::uint64_t>(h.declared) > h.records)
    notes.push_back(Value("The header states " + std::to_string(h.declared) + " data records; the file holds " +
                          std::to_string(h.records) + "."));
  else if (h.rest > 0)
    notes.push_back(Value("The file ends inside a data record: its last " + std::to_string(h.rest) +
                          " bytes are left out."));
  List untimed = untimed_spans(items);
  Dict meta{{"channels", dict_list(std::move(channels.items))},
            {"pieces", dict_list(std::move(items))},
            {"vitals", Value(List())},
            {"alarms", dict_list(std::move(alarms))},
            {"gaps", Value(std::move(gaps))},
            {"untimed", Value(std::move(untimed))},
            {"marker_names", Value::list_of(fin.marker_names)},
            {"skipped", Value(Dict())},
            {"notes", Value(std::move(notes))},
            {"time_resolution_s", Value()},
            {"reader", Value(Dict())}};
  return {std::move(meta), std::move(fin.arrays)};
}

namespace {

class EdfReader : public Reader {
 public:
  EdfReader(const fs::path& path, const Dict& meta) : path_(path) {
    const Value* pv = meta.get("pieces");
    if (const List* l = pv ? pv->as_list() : nullptr)
      for (const auto& p : *l)
        pieces_.push_back({static_cast<std::size_t>(int_of(p.get("sig")).value_or(0)),
                           static_cast<std::uint64_t>(int_of(p.get("data_record")).value_or(0)),
                           int_of(p.get("n")).value_or(0)});
  }

  std::vector<double> read(std::size_t piece, std::int64_t a, std::int64_t b) const override {
    if (piece >= pieces_.size()) throw std::runtime_error("no piece " + std::to_string(piece) + " in this recording");
    const Piece& p = pieces_[piece];
    a = std::clamp<std::int64_t>(a, 0, p.n);
    b = std::clamp<std::int64_t>(b, 0, p.n);
    if (b <= a) return {};
    const std::string name = core::path_utf8(path_);
    Header h = header(path_);
    if (p.sig >= h.signals.size() || h.signals[p.sig].spr == 0)
      throw std::runtime_error("the file has changed since it was indexed");
    const Signal& s = h.signals[p.sig];
    auto spr = static_cast<std::int64_t>(s.spr);
    std::int64_t r0 = a / spr, r1 = (b - 1) / spr;
    if (p.first + static_cast<std::uint64_t>(r1) >= h.records)
      throw std::runtime_error("the file has changed since it was indexed");
    File f(path_);
    auto buf = read_records(f, h, p.first + static_cast<std::uint64_t>(r0), static_cast<std::uint64_t>(r1 - r0 + 1),
                            name);
    std::vector<double> out;
    out.reserve(static_cast<std::size_t>(b - a));
    std::vector<std::int32_t> dig;
    for (std::int64_t j = 0; j <= r1 - r0; ++j) {
      digital(buf.data() + static_cast<std::size_t>(j) * h.record_bytes, s, h.bdf, dig);
      std::int64_t base = (r0 + j) * spr;
      for (std::size_t i = 0; i < dig.size(); ++i) {
        std::int64_t at = base + static_cast<std::int64_t>(i);
        if (at >= a && at < b) out.push_back(s.physical(dig[i]));
      }
    }
    return out;
  }

 private:
  struct Piece {
    std::size_t sig;
    std::uint64_t first;
    std::int64_t n;
  };
  fs::path path_;
  std::vector<Piece> pieces_;
};

}  // namespace

std::unique_ptr<Reader> make_edf_reader(const fs::path& path, const Dict& meta) {
  return std::make_unique<EdfReader>(path, meta);
}

}  // namespace vitalstream::engine
