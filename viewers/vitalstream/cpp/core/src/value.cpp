// JSON and str()/repr() text exactly as Python writes them (value.hpp; the Rust crate signal-core's value.rs).
#include "core/value.hpp"

#include <cctype>
#include <cmath>
#include <cstdio>
#include <stdexcept>

#include "core/pyfmt.hpp"

namespace vitalstream::core {

// --- Dict ---------------------------------------------------------------------------------

Dict::Dict() = default;
Dict::Dict(const Dict&) = default;
Dict::Dict(Dict&&) noexcept = default;
Dict& Dict::operator=(const Dict&) = default;
Dict& Dict::operator=(Dict&&) noexcept = default;
Dict::~Dict() = default;
std::size_t Dict::size() const { return items_.size(); }
bool Dict::empty() const { return items_.empty(); }

Dict::Dict(std::initializer_list<Entry> items) {
  for (const auto& e : items) insert(e.key, e.value);
}

bool Dict::contains(std::string_view key) const { return get(key) != nullptr; }

const Value* Dict::get(std::string_view key) const {
  for (const auto& e : items_)
    if (e.key == key) return &e.value;
  return nullptr;
}

Value* Dict::get(std::string_view key) {
  for (auto& e : items_)
    if (e.key == key) return &e.value;
  return nullptr;
}

Value& Dict::operator[](std::string_view key) {
  if (Value* v = get(key)) return *v;
  items_.push_back(Entry{std::string(key), Value()});
  return items_.back().value;
}

void Dict::insert(std::string key, Value v) {
  if (Value* old = get(key)) {
    *old = std::move(v);
    return;
  }
  items_.push_back(Entry{std::move(key), std::move(v)});
}

bool Dict::erase(std::string_view key) {
  for (auto it = items_.begin(); it != items_.end(); ++it)
    if (it->key == key) {
      items_.erase(it);
      return true;
    }
  return false;
}

std::vector<Entry>::const_iterator Dict::begin() const { return items_.begin(); }
std::vector<Entry>::const_iterator Dict::end() const { return items_.end(); }
std::vector<Entry>::iterator Dict::begin() { return items_.begin(); }
std::vector<Entry>::iterator Dict::end() { return items_.end(); }
bool Dict::operator==(const Dict& o) const { return items_ == o.items_; }

// --- Value --------------------------------------------------------------------------------

bool Value::as_int(std::int64_t& out) const {
  if (auto p = std::get_if<std::int64_t>(&v_)) {
    out = *p;
    return true;
  }
  if (auto p = std::get_if<std::uint64_t>(&v_)) {
    out = static_cast<std::int64_t>(*p);
    return true;
  }
  return false;
}

bool Value::as_f64(double& out) const {
  if (auto p = std::get_if<double>(&v_)) {
    out = *p;
    return true;
  }
  if (auto p = std::get_if<std::int64_t>(&v_)) {
    out = static_cast<double>(*p);
    return true;
  }
  if (auto p = std::get_if<std::uint64_t>(&v_)) {
    out = static_cast<double>(*p);
    return true;
  }
  return false;
}

const Value* Value::get(std::string_view key) const {
  const Dict* d = as_dict();
  return d ? d->get(key) : nullptr;
}

bool Value::truthy() const {
  return std::visit(
      [](const auto& x) -> bool {
        using T = std::decay_t<decltype(x)>;
        if constexpr (std::is_same_v<T, std::monostate>) return false;
        else if constexpr (std::is_same_v<T, bool>) return x;
        else if constexpr (std::is_same_v<T, std::int64_t> || std::is_same_v<T, std::uint64_t>) return x != 0;
        else if constexpr (std::is_same_v<T, double>) return x != 0.0;
        else return !x.empty();
      },
      v_);
}

std::string json_float(double f) {
  if (std::isnan(f)) return "NaN";
  if (std::isinf(f)) return f > 0 ? "Infinity" : "-Infinity";
  return repr(f);
}

void Value::write_json(std::string& out) const {
  std::visit(
      [&out](const auto& x) {
        using T = std::decay_t<decltype(x)>;
        if constexpr (std::is_same_v<T, std::monostate>) {
          out += "null";
        } else if constexpr (std::is_same_v<T, bool>) {
          out += x ? "true" : "false";
        } else if constexpr (std::is_same_v<T, std::int64_t> || std::is_same_v<T, std::uint64_t>) {
          out += std::to_string(x);
        } else if constexpr (std::is_same_v<T, double>) {
          out += json_float(x);
        } else if constexpr (std::is_same_v<T, std::string>) {
          json_string(x, out);
        } else if constexpr (std::is_same_v<T, List>) {
          out += '[';
          for (std::size_t i = 0; i < x.size(); ++i) {
            if (i) out += ',';
            x[i].write_json(out);
          }
          out += ']';
        } else if constexpr (std::is_same_v<T, Dict>) {
          out += '{';
          bool first = true;
          for (const auto& e : x) {
            if (!first) out += ',';
            first = false;
            json_string(e.key, out);
            out += ':';
            e.value.write_json(out);
          }
          out += '}';
        } else if constexpr (std::is_same_v<T, DoubleArray>) {
          out += '[';
          for (std::size_t i = 0; i < x.size(); ++i) {
            if (i) out += ',';
            out += json_float(x[i]);
          }
          out += ']';
        } else {  // Int64Array
          out += '[';
          for (std::size_t i = 0; i < x.size(); ++i) {
            if (i) out += ',';
            out += std::to_string(x[i]);
          }
          out += ']';
        }
      },
      v_);
}

std::string Value::to_json() const {
  std::string out;
  write_json(out);
  return out;
}

std::string Value::py_str() const {
  if (auto s = as_str()) return *s;
  return py_repr();
}

std::string Value::py_repr() const {
  return std::visit(
      [](const auto& x) -> std::string {
        using T = std::decay_t<decltype(x)>;
        if constexpr (std::is_same_v<T, std::monostate>) {
          return "None";
        } else if constexpr (std::is_same_v<T, bool>) {
          return x ? "True" : "False";
        } else if constexpr (std::is_same_v<T, std::int64_t> || std::is_same_v<T, std::uint64_t>) {
          return std::to_string(x);
        } else if constexpr (std::is_same_v<T, double>) {
          return repr(x);
        } else if constexpr (std::is_same_v<T, std::string>) {
          return str_repr(x);
        } else if constexpr (std::is_same_v<T, List>) {
          std::string s = "[";
          for (std::size_t i = 0; i < x.size(); ++i) {
            if (i) s += ", ";
            s += x[i].py_repr();
          }
          return s + "]";
        } else if constexpr (std::is_same_v<T, Dict>) {
          std::string s = "{";
          bool first = true;
          for (const auto& e : x) {
            if (!first) s += ", ";
            first = false;
            s += str_repr(e.key) + ": " + e.value.py_repr();
          }
          return s + "}";
        } else if constexpr (std::is_same_v<T, DoubleArray>) {
          std::string s = "[";
          for (std::size_t i = 0; i < x.size(); ++i) {
            if (i) s += ", ";
            s += repr(x[i]);
          }
          return s + "]";
        } else {
          std::string s = "[";
          for (std::size_t i = 0; i < x.size(); ++i) {
            if (i) s += ", ";
            s += std::to_string(x[i]);
          }
          return s + "]";
        }
      },
      v_);
}

// --- UTF-8 --------------------------------------------------------------------------------

namespace {

// Decode one code point at s[i]; returns its length (0: invalid byte).
std::size_t utf8_next(std::string_view s, std::size_t i, char32_t& c) {
  const auto b = [&](std::size_t k) { return static_cast<unsigned char>(s[k]); };
  unsigned char c0 = b(i);
  if (c0 < 0x80) {
    c = c0;
    return 1;
  }
  std::size_t n;
  char32_t min;
  if ((c0 & 0xe0) == 0xc0) {
    n = 2, c = c0 & 0x1f, min = 0x80;
  } else if ((c0 & 0xf0) == 0xe0) {
    n = 3, c = c0 & 0x0f, min = 0x800;
  } else if ((c0 & 0xf8) == 0xf0) {
    n = 4, c = c0 & 0x07, min = 0x10000;
  } else {
    return 0;
  }
  if (i + n > s.size()) return 0;
  for (std::size_t k = 1; k < n; ++k) {
    if ((b(i + k) & 0xc0) != 0x80) return 0;
    c = (c << 6) | (b(i + k) & 0x3f);
  }
  if (c < min || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff)) return 0;
  return n;
}

}  // namespace

std::vector<char32_t> utf8_decode(std::string_view s) {
  std::vector<char32_t> out;
  out.reserve(s.size());
  for (std::size_t i = 0; i < s.size();) {
    char32_t c;
    std::size_t n = utf8_next(s, i, c);
    if (n == 0) {
      out.push_back(0xfffd);
      ++i;
    } else {
      out.push_back(c);
      i += n;
    }
  }
  return out;
}

bool utf8_valid(std::string_view s, std::size_t* valid_up_to) {
  for (std::size_t i = 0; i < s.size();) {
    char32_t c;
    std::size_t n = utf8_next(s, i, c);
    if (n == 0) {
      if (valid_up_to) *valid_up_to = i;
      return false;
    }
    i += n;
  }
  if (valid_up_to) *valid_up_to = s.size();
  return true;
}

void utf8_append(std::string& out, char32_t c) {
  if (c < 0x80) {
    out += static_cast<char>(c);
  } else if (c < 0x800) {
    out += static_cast<char>(0xc0 | (c >> 6));
    out += static_cast<char>(0x80 | (c & 0x3f));
  } else if (c < 0x10000) {
    out += static_cast<char>(0xe0 | (c >> 12));
    out += static_cast<char>(0x80 | ((c >> 6) & 0x3f));
    out += static_cast<char>(0x80 | (c & 0x3f));
  } else {
    out += static_cast<char>(0xf0 | (c >> 18));
    out += static_cast<char>(0x80 | ((c >> 12) & 0x3f));
    out += static_cast<char>(0x80 | ((c >> 6) & 0x3f));
    out += static_cast<char>(0x80 | (c & 0x3f));
  }
}

void json_string(std::string_view utf8, std::string& out) {
  static const char* H = "0123456789abcdef";
  auto u4 = [&](unsigned u) {
    out += "\\u";
    out += H[(u >> 12) & 15];
    out += H[(u >> 8) & 15];
    out += H[(u >> 4) & 15];
    out += H[u & 15];
  };
  out += '"';
  for (char32_t c : utf8_decode(utf8)) {
    switch (c) {
      case '"': out += "\\\""; break;
      case '\\': out += "\\\\"; break;
      case '\n': out += "\\n"; break;
      case '\r': out += "\\r"; break;
      case '\t': out += "\\t"; break;
      case 0x08: out += "\\b"; break;
      case 0x0c: out += "\\f"; break;
      default:
        if (c < 0x20 || c > 0x7e) {
          if (c >= 0x10000) {
            char32_t v = c - 0x10000;
            u4(0xd800 + (v >> 10));
            u4(0xdc00 + (v & 0x3ff));
          } else {
            u4(c);
          }
        } else {
          out += static_cast<char>(c);
        }
    }
  }
  out += '"';
}

std::string str_repr(std::string_view utf8) {
  auto cps = utf8_decode(utf8);
  bool has_single = false, has_double = false;
  for (char32_t c : cps) {
    has_single |= c == '\'';
    has_double |= c == '"';
  }
  char32_t quote = (has_single && !has_double) ? '"' : '\'';
  static const char* H = "0123456789abcdef";
  std::string out;
  out += static_cast<char>(quote);
  for (char32_t c : cps) {
    if (c == '\\') out += "\\\\";
    else if (c == '\n') out += "\\n";
    else if (c == '\r') out += "\\r";
    else if (c == '\t') out += "\\t";
    else if (c == quote) {
      out += '\\';
      out += static_cast<char>(c);
    } else if (c < 0x20 || c == 0x7f || (c >= 0x80 && c < 0xa0)) {
      out += "\\x";
      out += H[(c >> 4) & 15];
      out += H[c & 15];
    } else {
      utf8_append(out, c);
    }
  }
  out += static_cast<char>(quote);
  return out;
}

// --- JSON parser --------------------------------------------------------------------------

namespace {

// Arrays and objects nested deeper than this are refused (the parser recurses, and so does the
// destructor of what it builds).
constexpr std::size_t MAX_DEPTH = 128;

struct JsonParser {
  std::string_view b;
  std::size_t i = 0;
  std::size_t depth = 0;  // arrays and objects open around the current position

  [[noreturn]] void err(const std::string& what) const {
    throw std::runtime_error("JSON: " + what + " at byte " + std::to_string(i));
  }
  void ws() {
    while (i < b.size() && (b[i] == ' ' || b[i] == '\t' || b[i] == '\n' || b[i] == '\r')) ++i;
  }
  bool starts(std::string_view w) const { return b.substr(i, w.size()) == w; }
  Value lit(std::string_view w, Value v) {
    if (!starts(w)) err("unexpected character");
    i += w.size();
    return v;
  }

  Value value() {
    ws();
    if (i < b.size() && (b[i] == '{' || b[i] == '[')) {
      if (depth >= MAX_DEPTH) err("arrays and objects nested too deeply");
      ++depth;
      Value v = scalar_or_container();
      --depth;
      return v;
    }
    return scalar_or_container();
  }

  Value scalar_or_container() {
    if (i >= b.size()) err("unexpected end");
    char c = b[i];
    if (c == '{') {
      ++i;
      Dict d;
      ws();
      if (i < b.size() && b[i] == '}') {
        ++i;
        return Value(std::move(d));
      }
      for (;;) {
        ws();
        std::string k = string();
        ws();
        if (i >= b.size() || b[i] != ':') err("expected ':'");
        ++i;
        Value v = value();
        d.insert(std::move(k), std::move(v));
        ws();
        if (i < b.size() && b[i] == ',') {
          ++i;
        } else if (i < b.size() && b[i] == '}') {
          ++i;
          return Value(std::move(d));
        } else {
          err("expected ',' or '}'");
        }
      }
    }
    if (c == '[') {
      ++i;
      List l;
      ws();
      if (i < b.size() && b[i] == ']') {
        ++i;
        return Value(std::move(l));
      }
      for (;;) {
        l.push_back(value());
        ws();
        if (i < b.size() && b[i] == ',') {
          ++i;
        } else if (i < b.size() && b[i] == ']') {
          ++i;
          return Value(std::move(l));
        } else {
          err("expected ',' or ']'");
        }
      }
    }
    if (c == '"') return Value(string());
    if (c == 't') return lit("true", Value(true));
    if (c == 'f') return lit("false", Value(false));
    if (c == 'n') return lit("null", Value());
    if (c == 'N') return lit("NaN", Value(std::nan("")));
    if (c == 'I') return lit("Infinity", Value(HUGE_VAL));
    if (c == '-' && starts("-Infinity")) return lit("-Infinity", Value(-HUGE_VAL));
    return number();
  }

  Value number() {
    std::size_t s = i;
    while (i < b.size() && (std::isdigit(static_cast<unsigned char>(b[i])) || b[i] == '-' || b[i] == '+' ||
                            b[i] == '.' || b[i] == 'e' || b[i] == 'E'))
      ++i;
    std::string_view t = b.substr(s, i - s);
    if (t.empty()) err("unexpected character");
    if (t.find_first_of(".eE") == std::string_view::npos) {
      std::int64_t v;
      if (parse_int(t, v)) return Value(static_cast<long long>(v));
    }
    double d;
    if (!parse_double(t, d)) throw std::runtime_error("JSON: bad number " + std::string(t));
    return Value(d);
  }

  static unsigned hex4(std::string_view h, bool& ok) {
    unsigned u = 0;
    ok = h.size() == 4;
    for (char ch : h) {
      u <<= 4;
      if (ch >= '0' && ch <= '9') u |= ch - '0';
      else if (ch >= 'a' && ch <= 'f') u |= ch - 'a' + 10;
      else if (ch >= 'A' && ch <= 'F') u |= ch - 'A' + 10;
      else ok = false;
    }
    return u;
  }

  std::string string() {
    if (i >= b.size() || b[i] != '"') err("expected a string");
    ++i;
    std::string out;
    std::vector<unsigned> units;
    auto flush_units = [&] {
      for (std::size_t k = 0; k < units.size(); ++k) {
        unsigned u = units[k];
        if (u >= 0xd800 && u < 0xdc00 && k + 1 < units.size() && units[k + 1] >= 0xdc00 && units[k + 1] < 0xe000) {
          utf8_append(out, 0x10000 + ((u - 0xd800) << 10) + (units[k + 1] - 0xdc00));
          ++k;
        } else if (u >= 0xd800 && u < 0xe000) {
          utf8_append(out, 0xfffd);
        } else {
          utf8_append(out, u);
        }
      }
      units.clear();
    };
    for (;;) {
      if (i >= b.size()) err("unterminated string");
      char c = b[i];
      if (c == '\\' && i + 1 < b.size() && b[i + 1] == 'u') {
        bool ok;
        unsigned u = hex4(b.substr(i + 2, 4), ok);
        if (!ok) throw std::runtime_error("JSON: bad \\u escape");
        units.push_back(u);
        i += 6;
        continue;
      }
      if (!units.empty()) flush_units();
      if (c == '"') {
        ++i;
        return out;
      }
      if (c == '\\') {
        char e = i + 1 < b.size() ? b[i + 1] : '?';
        switch (e) {
          case 'n': out += '\n'; break;
          case 't': out += '\t'; break;
          case 'r': out += '\r'; break;
          case 'b': out += '\b'; break;
          case 'f': out += '\f'; break;
          case '/': out += '/'; break;
          case '"': out += '"'; break;
          case '\\': out += '\\'; break;
          default: err("bad escape");
        }
        i += 2;
      } else {
        std::size_t start = i;
        while (i < b.size() && b[i] != '"' && b[i] != '\\') ++i;
        std::string_view piece = b.substr(start, i - start);
        if (!utf8_valid(piece)) throw std::runtime_error("JSON: invalid UTF-8");
        out += piece;
      }
    }
  }
};

}  // namespace

Value Value::from_json(std::string_view text) {
  JsonParser p{text};
  Value v = p.value();
  p.ws();
  if (p.i != text.size()) throw std::runtime_error("trailing characters at byte " + std::to_string(p.i));
  return v;
}

}  // namespace vitalstream::core
