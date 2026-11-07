// Building blocks shared by all sources (as the Rust engine's common.rs and the Python viewer's
// common.py).
//
// - channel: one signal as the user sees it (label, unit, modality);
// - piece: a stretch of a channel the source proves contiguous: n samples at fs from t0 (Unix
//   seconds, UTC); timed = false when the file states no time for it;
// - envelope: per piece, min and max of every block of LEVEL0 samples, and of every FANOUT
//   blocks above that (a min/max pyramid).
#pragma once

#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "core/ordered_map.hpp"
#include "core/value.hpp"

namespace vitalstream::engine {

using core::Dict;
using core::List;
using core::OrderedMap;
using core::Value;

constexpr std::size_t LEVEL0 = 32;
constexpr std::size_t FANOUT = 4;
constexpr std::int64_t INDEX_VERSION = 2;  // bump when the cached index layout changes
constexpr std::size_t CHUNK = 1 << 16;
// The most pieces a recording may have: a reader refuses a recording with more
// (viewers/vitalstream/docs/index-format.md, "Limits").
constexpr std::size_t MAX_PIECES = 100000;
// The most distinct marker names (mk_code is 16-bit): names after the first MAX_MARKER_NAMES - 1
// share the last code, named OTHER_MARKERS.
constexpr std::size_t MAX_MARKER_NAMES = 32768;
inline constexpr const char* OTHER_MARKERS = "(other markers)";

std::size_t bucket_size(std::size_t level);

// NaN-ignoring min / max (NumPy fmin / fmax).
float fmin32(float a, float b);
float fmax32(float a, float b);
double fmin64(double a, double b);
double fmax64(double a, double b);

// Value accessors (nothing when absent or of another type).
std::optional<double> f64_of(const Value* v);         // floats and integers
std::optional<std::int64_t> int_of(const Value* v);   // integers
const Value* get(const Dict& d, const char* k);       // nullptr when absent
Value get_or_none(const Dict& d, const char* k);
Value get_or_none(const Value& d, const char* k);

// Streaming min/max pyramid of one piece (NaN only where a whole bucket is NaN).
class Envelope {
 public:
  void add(const float* x, std::size_t n);
  // (min, max) per level, level 0 first, down to a single bucket.
  std::vector<std::pair<std::vector<float>, std::vector<float>>> finish();

 private:
  void flush_bucket();
  std::vector<float> mins_, maxs_, rest_;
};

// Channel registry: one id per key, first-seen order.
class Channels {
 public:
  std::size_t get(const std::string& key, const std::string& label, const std::string& unit,
                  const std::string& modality, const char* extra_key = nullptr, Value extra = Value());
  std::vector<Dict> items;

 private:
  OrderedMap<std::string, std::size_t> ids_;
};

// Arrays of an index (see index.hpp for the files).
struct Arrays {
  std::vector<std::vector<float>> env_min, env_max;
  std::vector<std::int32_t> mk_piece;
  std::vector<std::int64_t> mk_sample;
  std::vector<std::int16_t> mk_code;
  std::vector<double> vit_t, vit_v;
};

// Pieces with their envelopes and markers.
class Pieces {
 public:
  // A new piece of channel ch at fs; info: source-specific fields.  A reader checks room()
  // first (MAX_PIECES).
  std::size_t start(std::size_t ch, double fs, const Dict& info);
  // Throws std::runtime_error unless `more` pieces still fit within MAX_PIECES.
  void room(std::size_t more) const;
  // Whether more marker names were found than codes exist (the rest share one name).
  bool markers_capped() const { return marker_names_.size() == MAX_MARKER_NAMES; }
  std::int64_t n(std::size_t p) const { return n_[p]; }
  void add(std::size_t p, const float* values, std::size_t count);
  void marker(std::size_t p, std::int64_t sample, const std::string& name);
  struct Finished {
    std::vector<Dict> items;
    Arrays arrays;
    std::vector<std::string> marker_names;
  };
  Finished finish();
  std::vector<Dict> items;

 private:
  std::vector<std::int64_t> n_;
  std::vector<Envelope> envelopes_;
  struct Marker {
    std::int32_t piece;
    std::int64_t sample;
    std::int16_t code;
  };
  std::vector<Marker> markers_;
  std::vector<std::string> marker_names_;
  OrderedMap<std::string, std::size_t> names_;
};

// Python float(x) for values from a file: numbers and numeric strings.
std::optional<double> to_float(const Value& v);

// Vital-sign series: name -> times and values (finite values only).
class Vitals {
 public:
  void add(const std::string& name, const Value& unit, const Value& t, const Value& value);
  struct Finished {
    std::vector<Dict> meta;
    std::vector<double> t, v;
  };
  Finished finish();

 private:
  struct Series {
    std::string unit;
    std::vector<double> t, v;
  };
  OrderedMap<std::string, Series> series_;
};

double t0_of(const Dict& d);
void sort_by_t0(std::vector<Dict>& v);

// t if it lies within one day of [lo, hi], else nothing (e.g. .NET zero dates).
std::optional<double> plausible_time(const Value& t, std::optional<double> lo, std::optional<double> hi);

// Merged [t0, t1] spans covered by pieces whose time is not stated.
List untimed_spans(const std::vector<Dict>& pieces);

// JSON-safe copy: NaN and infinities become null.
Value clean(const Value& v);

// Unicode-aware helpers matching Rust's str methods.
std::string trim(const std::string& s);                      // str::trim
std::vector<std::string> split_whitespace(const std::string& s);
std::string to_lower(const std::string& s);                  // str::to_lowercase (common scripts)

Value dict_list(std::vector<Dict> v);

}  // namespace vitalstream::engine
