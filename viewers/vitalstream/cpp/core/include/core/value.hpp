// Dynamic values: records as the Python implementations hold them (dicts in insertion order,
// lists, numbers, strings, None), with JSON and str()/repr() text identical to Python's, so that
// the Rust, C++ and Python implementations write the same bytes (the C++ counterpart of the Rust
// crate signal-core). Large sample arrays use the compact array alternatives (DoubleArray,
// Int64Array), which print exactly like lists of numbers.
#pragma once

#include <cstdint>
#include <initializer_list>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

namespace vitalstream::core {

class Value;
struct Entry;

using List = std::vector<Value>;
using DoubleArray = std::vector<double>;
using Int64Array = std::vector<std::int64_t>;

// A dict in insertion order (Python dict).
class Dict {
 public:
  // Special members are defined in value.cpp, where Entry is complete.
  Dict();
  Dict(std::initializer_list<Entry> items);
  Dict(const Dict&);
  Dict(Dict&&) noexcept;
  Dict& operator=(const Dict&);
  Dict& operator=(Dict&&) noexcept;
  ~Dict();

  bool contains(std::string_view key) const;
  const Value* get(std::string_view key) const;  // nullptr when absent
  Value* get(std::string_view key);
  Value& operator[](std::string_view key);         // inserts None when absent
  void insert(std::string key, Value v);           // replaces an existing key in place
  bool erase(std::string_view key);
  std::size_t size() const;
  bool empty() const;
  std::vector<Entry>::const_iterator begin() const;
  std::vector<Entry>::const_iterator end() const;
  std::vector<Entry>::iterator begin();
  std::vector<Entry>::iterator end();
  bool operator==(const Dict& o) const;

 private:
  std::vector<Entry> items_;
};

class Value {
 public:
  using Storage = std::variant<std::monostate, bool, std::int64_t, std::uint64_t, double, std::string, List, Dict,
                               DoubleArray, Int64Array>;

  Value() = default;
  Value(std::nullptr_t) {}
  Value(bool b) : v_(b) {}
  Value(int x) : v_(static_cast<std::int64_t>(x)) {}
  Value(long x) : v_(static_cast<std::int64_t>(x)) {}
  Value(long long x) : v_(static_cast<std::int64_t>(x)) {}
  Value(unsigned x) : v_(static_cast<std::int64_t>(x)) {}
  Value(unsigned long x) { set_unsigned(x); }
  Value(unsigned long long x) { set_unsigned(x); }
  Value(double x) : v_(x) {}
  Value(float x) : v_(static_cast<double>(x)) {}
  Value(const char* s) : v_(std::string(s)) {}
  Value(std::string s) : v_(std::move(s)) {}
  Value(std::string_view s) : v_(std::string(s)) {}
  Value(List l) : v_(std::move(l)) {}
  Value(Dict d) : v_(std::move(d)) {}
  Value(DoubleArray a) : v_(std::move(a)) {}
  Value(Int64Array a) : v_(std::move(a)) {}
  template <class T>
  static Value list_of(const std::vector<T>& xs) {
    List l;
    l.reserve(xs.size());
    for (const auto& x : xs) l.emplace_back(x);
    return Value(std::move(l));
  }

  bool is_none() const { return std::holds_alternative<std::monostate>(v_); }
  bool is_bool() const { return std::holds_alternative<bool>(v_); }
  bool is_int() const { return std::holds_alternative<std::int64_t>(v_) || std::holds_alternative<std::uint64_t>(v_); }
  bool is_float() const { return std::holds_alternative<double>(v_); }
  bool is_str() const { return std::holds_alternative<std::string>(v_); }
  bool is_list() const { return std::holds_alternative<List>(v_); }
  bool is_dict() const { return std::holds_alternative<Dict>(v_); }

  const Dict* as_dict() const { return std::get_if<Dict>(&v_); }
  Dict* as_dict() { return std::get_if<Dict>(&v_); }
  const List* as_list() const { return std::get_if<List>(&v_); }
  List* as_list() { return std::get_if<List>(&v_); }
  const std::string* as_str() const { return std::get_if<std::string>(&v_); }
  const bool* as_bool() const { return std::get_if<bool>(&v_); }
  bool as_int(std::int64_t& out) const;    // integers (not bools)
  bool as_f64(double& out) const;          // floats and integers
  const Storage& storage() const { return v_; }
  Storage& storage() { return v_; }

  // d.get(key) for a dict (nullptr for anything else).
  const Value* get(std::string_view key) const;

  bool truthy() const;  // Python truthiness

  // json.dumps(v, separators=(',', ':')): ASCII output, NaN/Infinity as Python writes them.
  std::string to_json() const;
  void write_json(std::string& out) const;
  std::string py_str() const;   // str(v)
  std::string py_repr() const;  // repr(v)

  // Parse JSON text (Python's json: NaN, Infinity and -Infinity accepted; integers stay
  // integers).  Throws std::runtime_error.
  static Value from_json(std::string_view text);

  bool operator==(const Value& o) const { return v_ == o.v_; }

 private:
  void set_unsigned(unsigned long long x) {
    if (x <= 0x7fffffffffffffffULL)
      v_ = static_cast<std::int64_t>(x);
    else
      v_ = static_cast<std::uint64_t>(x);
  }
  Storage v_;
};

struct Entry {
  std::string key;
  Value value;
  bool operator==(const Entry& o) const { return key == o.key && value == o.value; }
};

// An optional as a value (None when empty).
template <class T>
Value opt_value(const std::optional<T>& o) {
  return o ? Value(*o) : Value();
}

std::string json_float(double f);
void json_string(std::string_view utf8, std::string& out);
std::string str_repr(std::string_view utf8);  // Python repr(str)

// UTF-8 helpers.
std::vector<char32_t> utf8_decode(std::string_view s);  // invalid bytes -> U+FFFD
void utf8_append(std::string& out, char32_t c);
bool utf8_valid(std::string_view s, std::size_t* valid_up_to = nullptr);

}  // namespace vitalstream::core
