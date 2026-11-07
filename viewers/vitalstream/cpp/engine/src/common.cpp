// Building blocks shared by all sources (common.hpp).
#include "engine/common.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <stdexcept>
#include <limits>

#include "core/text.hpp"

namespace vitalstream::engine {

namespace {
const float NaNf = std::numeric_limits<float>::quiet_NaN();
const double NaN = std::numeric_limits<double>::quiet_NaN();

bool rust_space(char32_t c) {
  return c == ' ' || (c >= 0x9 && c <= 0xd) || c == 0x85 || c == 0xa0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200a) ||
         c == 0x2028 || c == 0x2029 || c == 0x202f || c == 0x205f || c == 0x3000;
}
}  // namespace

std::size_t bucket_size(std::size_t level) {
  std::size_t s = LEVEL0;
  for (std::size_t i = 0; i < level; ++i) s *= FANOUT;
  return s;
}

float fmin32(float a, float b) {
  if (std::isnan(a)) return b;
  if (std::isnan(b) || a <= b) return a;
  return b;
}
float fmax32(float a, float b) {
  if (std::isnan(a)) return b;
  if (std::isnan(b) || a >= b) return a;
  return b;
}
double fmin64(double a, double b) {
  if (std::isnan(a)) return b;
  if (std::isnan(b) || a <= b) return a;
  return b;
}
double fmax64(double a, double b) {
  if (std::isnan(a)) return b;
  if (std::isnan(b) || a >= b) return a;
  return b;
}

std::optional<double> f64_of(const Value* v) {
  double x;
  if (v && v->as_f64(x)) return x;
  return std::nullopt;
}

std::optional<std::int64_t> int_of(const Value* v) {
  std::int64_t x;
  if (v && v->as_int(x)) return x;
  return std::nullopt;
}

const Value* get(const Dict& d, const char* k) { return d.get(k); }
Value get_or_none(const Dict& d, const char* k) {
  const Value* v = d.get(k);
  return v ? *v : Value();
}
Value get_or_none(const Value& d, const char* k) {
  const Value* v = d.get(k);
  return v ? *v : Value();
}

void Envelope::add(const float* x, std::size_t n) {
  for (std::size_t i = 0; i < n; ++i) {
    rest_.push_back(x[i]);
    if (rest_.size() == LEVEL0) flush_bucket();
  }
}

void Envelope::flush_bucket() {
  float lo = NaNf, hi = NaNf;
  for (float v : rest_) {
    lo = fmin32(lo, v);
    hi = fmax32(hi, v);
  }
  mins_.push_back(lo);
  maxs_.push_back(hi);
  rest_.clear();
}

std::vector<std::pair<std::vector<float>, std::vector<float>>> Envelope::finish() {
  if (!rest_.empty()) flush_bucket();
  std::vector<std::pair<std::vector<float>, std::vector<float>>> levels;
  levels.emplace_back(std::move(mins_), std::move(maxs_));
  while (levels.back().first.size() > 1) {
    const auto& [lo, hi] = levels.back();
    std::size_t n = (lo.size() + FANOUT - 1) / FANOUT;
    std::vector<float> nlo, nhi;
    nlo.reserve(n);
    nhi.reserve(n);
    for (std::size_t k = 0; k < n; ++k) {
      float a = NaNf, b = NaNf;
      for (std::size_t j = k * FANOUT; j < std::min((k + 1) * FANOUT, lo.size()); ++j) {
        a = fmin32(a, lo[j]);
        b = fmax32(b, hi[j]);
      }
      nlo.push_back(a);
      nhi.push_back(b);
    }
    levels.emplace_back(std::move(nlo), std::move(nhi));
  }
  return levels;
}

std::size_t Channels::get(const std::string& key, const std::string& label, const std::string& unit,
                          const std::string& modality, const char* extra_key, Value extra) {
  if (const std::size_t* id = ids_.get(key)) return *id;
  std::size_t id = items.size();
  Dict d{{"id", Value(id)}, {"label", Value(label)}, {"unit", Value(unit)}, {"modality", Value(modality)}};
  if (extra_key) d.insert(extra_key, std::move(extra));
  items.push_back(std::move(d));
  ids_.insert(key, id);
  return id;
}

std::size_t Pieces::start(std::size_t ch, double fs, const Dict& info) {
  std::size_t id = items.size();
  Dict d{{"id", Value(id)}, {"ch", Value(ch)}, {"fs", Value(fs)}, {"n", Value(0)}};
  for (const auto& e : info) d.insert(e.key, e.value);
  items.push_back(std::move(d));
  n_.push_back(0);
  envelopes_.emplace_back();
  return id;
}

void Pieces::room(std::size_t more) const {
  if (more > MAX_PIECES || items.size() > MAX_PIECES - more)
    throw std::runtime_error("the recording has more than " + std::to_string(MAX_PIECES) +
                                " pieces (stretches of one signal): it is not indexed");
}

void Pieces::add(std::size_t p, const float* values, std::size_t count) {
  envelopes_[p].add(values, count);
  n_[p] += static_cast<std::int64_t>(count);
}

void Pieces::marker(std::size_t p, std::int64_t sample, const std::string& name) {
  std::size_t i;
  if (const std::size_t* x = names_.get(name)) {
    i = *x;
  } else if (marker_names_.size() == MAX_MARKER_NAMES - 1) {
    // every code taken: the rest share the last one
    marker_names_.push_back(OTHER_MARKERS);
    names_.insert(OTHER_MARKERS, MAX_MARKER_NAMES - 1);
    i = MAX_MARKER_NAMES - 1;
  } else if (marker_names_.size() == MAX_MARKER_NAMES) {
    i = MAX_MARKER_NAMES - 1;
  } else {
    marker_names_.push_back(name);
    i = marker_names_.size() - 1;
    names_.insert(name, i);
  }
  markers_.push_back(Marker{static_cast<std::int32_t>(p), sample, static_cast<std::int16_t>(i)});
}

Pieces::Finished Pieces::finish() {
  Finished out;
  out.items = std::move(items);
  for (std::size_t p = 0; p < envelopes_.size(); ++p) {
    out.items[p].insert("n", Value(n_[p]));
    auto levels = envelopes_[p].finish();
    List offs;
    for (std::size_t lvl = 0; lvl < levels.size(); ++lvl) {
      auto& [lo, hi] = levels[lvl];
      if (out.arrays.env_min.size() <= lvl) {
        out.arrays.env_min.emplace_back();
        out.arrays.env_max.emplace_back();
      }
      offs.push_back(Value(List{Value(out.arrays.env_min[lvl].size()), Value(lo.size())}));
      out.arrays.env_min[lvl].insert(out.arrays.env_min[lvl].end(), lo.begin(), lo.end());
      out.arrays.env_max[lvl].insert(out.arrays.env_max[lvl].end(), hi.begin(), hi.end());
    }
    out.items[p].insert("env", Value(std::move(offs)));
  }
  auto mk = std::move(markers_);
  std::stable_sort(mk.begin(), mk.end(), [](const Marker& a, const Marker& b) {
    return a.piece != b.piece ? a.piece < b.piece : a.sample < b.sample;
  });
  for (const auto& m : mk) {
    out.arrays.mk_piece.push_back(m.piece);
    out.arrays.mk_sample.push_back(m.sample);
    out.arrays.mk_code.push_back(m.code);
  }
  out.marker_names = std::move(marker_names_);
  return out;
}

std::optional<double> to_float(const Value& v) {
  if (const auto* f = std::get_if<double>(&v.storage())) return *f;
  if (const bool* b = v.as_bool()) return *b ? 1.0 : 0.0;
  if (v.is_int()) return f64_of(&v);
  if (const std::string* s = v.as_str()) return core::py_float(*s);
  return std::nullopt;
}

void Vitals::add(const std::string& name, const Value& unit, const Value& t, const Value& value) {
  auto tt = to_float(t), vv = to_float(value);
  if (!tt || !vv || !std::isfinite(*tt) || !std::isfinite(*vv)) return;
  Series* s = series_.get(name);
  if (!s) {
    series_.insert(name, Series{unit.truthy() ? unit.py_str() : std::string(), {}, {}});
    s = series_.get(name);
  }
  s->t.push_back(*tt);
  s->v.push_back(*vv);
}

Vitals::Finished Vitals::finish() {
  std::vector<const std::string*> names;
  for (const auto& [k, s] : series_) names.push_back(&k);
  std::stable_sort(names.begin(), names.end(), [&](const std::string* a, const std::string* b) {
    std::size_t na = series_.get(*a)->t.size(), nb = series_.get(*b)->t.size();
    if (na != nb) return na > nb;
    return *a < *b;
  });
  Finished out;
  for (const std::string* name : names) {
    const Series& s = *series_.get(*name);
    std::vector<std::size_t> order(s.t.size());
    for (std::size_t i = 0; i < order.size(); ++i) order[i] = i;
    std::stable_sort(order.begin(), order.end(), [&](std::size_t a, std::size_t b) { return s.t[a] < s.t[b]; });
    out.meta.push_back(Dict{{"name", Value(*name)},
                            {"unit", Value(s.unit)},
                            {"offset", Value(out.t.size())},
                            {"n", Value(s.t.size())}});
    for (std::size_t i : order) {
      out.t.push_back(s.t[i]);
      out.v.push_back(s.v[i]);
    }
  }
  return out;
}

double t0_of(const Dict& d) { return f64_of(d.get("t0")).value_or(NaN); }

void sort_by_t0(std::vector<Dict>& v) {
  // Rust's stable sort with partial_cmp (NaN compares equal to everything)
  std::stable_sort(v.begin(), v.end(), [](const Dict& a, const Dict& b) { return t0_of(a) < t0_of(b); });
}

std::optional<double> plausible_time(const Value& t, std::optional<double> lo, std::optional<double> hi) {
  auto x = to_float(t);
  if (!x || !lo || !hi) return std::nullopt;
  if (std::isfinite(*x) && *lo - 86400.0 <= *x && *x <= *hi + 86400.0) return x;
  return std::nullopt;
}

List untimed_spans(const std::vector<Dict>& pieces) {
  std::vector<std::pair<double, double>> spans;
  for (const auto& p : pieces) {
    const Value* timed = p.get("timed");
    if (!(timed && timed->as_bool() && !*timed->as_bool())) continue;
    double t0 = f64_of(p.get("t0")).value_or(0.0);
    double n = f64_of(p.get("n")).value_or(0.0);
    double fs = f64_of(p.get("fs")).value_or(NaN);
    spans.emplace_back(t0, t0 + n / fs);
  }
  // a total order (Rust's total_cmp: NaN after +inf, -NaN before -inf), so that NaN never
  // breaks the sort
  auto key = [](double x) {
    std::int64_t i;
    std::memcpy(&i, &x, sizeof i);
    return i ^ static_cast<std::int64_t>(static_cast<std::uint64_t>(i >> 63) >> 1);
  };
  std::sort(spans.begin(), spans.end(), [&](const auto& a, const auto& b) {
    return key(a.first) != key(b.first) ? key(a.first) < key(b.first) : key(a.second) < key(b.second);
  });
  std::vector<std::pair<double, double>> merged;
  for (auto [a, b] : spans) {
    if (!merged.empty() && a <= merged.back().second + 1e-9)
      merged.back().second = std::max(merged.back().second, b);
    else
      merged.emplace_back(a, b);
  }
  List out;
  for (auto [a, b] : merged) out.push_back(Value(Dict{{"t0", Value(a)}, {"t1", Value(b)}}));
  return out;
}

Value clean(const Value& v) {
  const auto& s = v.storage();
  if (const auto* f = std::get_if<double>(&s)) return std::isfinite(*f) ? v : Value();
  if (const auto* a = std::get_if<core::DoubleArray>(&s)) {
    if (std::all_of(a->begin(), a->end(), [](double x) { return std::isfinite(x); })) return v;
    List l;
    l.reserve(a->size());
    for (double x : *a) l.push_back(std::isfinite(x) ? Value(x) : Value());
    return Value(std::move(l));
  }
  if (const List* l = v.as_list()) {
    List out;
    out.reserve(l->size());
    for (const auto& x : *l) out.push_back(clean(x));
    return Value(std::move(out));
  }
  if (const Dict* d = v.as_dict()) {
    Dict out;
    for (const auto& e : *d) out.insert(e.key, clean(e.value));
    return Value(std::move(out));
  }
  return v;
}

std::string trim(const std::string& s) {
  auto cs = core::utf8_decode(s);
  std::size_t a = 0, b = cs.size();
  while (a < b && rust_space(cs[a])) ++a;
  while (b > a && rust_space(cs[b - 1])) --b;
  if (a == 0 && b == cs.size()) return s;
  std::string out;
  for (std::size_t i = a; i < b; ++i) core::utf8_append(out, cs[i]);
  return out;
}

std::vector<std::string> split_whitespace(const std::string& s) {
  std::vector<std::string> out;
  std::string cur;
  for (char32_t c : core::utf8_decode(s)) {
    if (rust_space(c)) {
      if (!cur.empty()) out.push_back(std::move(cur));
      cur.clear();
    } else {
      core::utf8_append(cur, c);
    }
  }
  if (!cur.empty()) out.push_back(std::move(cur));
  return out;
}

std::string to_lower(const std::string& s) {
  bool ascii = std::all_of(s.begin(), s.end(), [](char c) { return static_cast<unsigned char>(c) < 0x80; });
  std::string out;
  out.reserve(s.size());
  if (ascii) {
    for (char c : s) out += (c >= 'A' && c <= 'Z') ? static_cast<char>(c - 'A' + 'a') : c;
    return out;
  }
  for (char32_t c : core::utf8_decode(s)) {
    if ((c >= 'A' && c <= 'Z') || (c >= 0xC0 && c <= 0xDE && c != 0xD7) || (c >= 0x391 && c <= 0x3AB && c != 0x3A2) ||
        (c >= 0x410 && c <= 0x42F))
      c += 32;
    else if (c >= 0x400 && c <= 0x40F)
      c += 80;
    core::utf8_append(out, c);
  }
  return out;
}

Value dict_list(std::vector<Dict> v) {
  List l;
  l.reserve(v.size());
  for (auto& d : v) l.emplace_back(std::move(d));
  return Value(std::move(l));
}

}  // namespace vitalstream::engine
