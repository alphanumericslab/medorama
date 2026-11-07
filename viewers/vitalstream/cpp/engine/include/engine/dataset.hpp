// A loaded source: answers window, vital-sign and marker queries from its index (the Python
// viewer's dataset.py and the Rust engine's dataset.rs; responses are the same JSON).
#pragma once

#include <memory>
#include <string>
#include <vector>

#include "engine/common.hpp"
#include "engine/index.hpp"
#include "engine/sources.hpp"

namespace vitalstream::engine {

struct Piece {
  std::size_t id = 0;
  std::int64_t ch = 0;
  double fs = 1.0;
  std::int64_t n = 0;
  double t0 = 0.0;
  bool timed = true;
  std::vector<std::pair<std::size_t, std::size_t>> env;

  double end() const { return t0 + static_cast<double>(n) / fs; }
  // Samples [a, b) of the piece that a window [t0, t1] takes: one sample more before, two after;
  // within [0, n] (any finite times).
  std::pair<std::int64_t, std::int64_t> span(double w0, double w1) const;
};

class Dataset {
 public:
  Dataset(Dict meta, Loaded arrays);

  Value info() const;  // GET datasets/{key}
  // The number of channels (their ids are 0 to this, excluded).
  std::size_t channel_count() const;
  // Per channel, blocks of samples (raw) or min/max (env) covering [t0, t1] at about px points
  // across (JSON text); t0 < t1, both finite.  Throws.
  std::string window_json(const std::vector<std::int64_t>& channels, double t0, double t1, std::int64_t px) const;
  Value vitals_window(const std::vector<std::string>& names, double t0, double t1, std::int64_t px) const;
  Value vitals_at(double t) const;
  Value markers(const std::vector<std::int64_t>& channels, double t0, double t1) const;
  // Physical samples [a, b) of a piece.  Throws.
  std::vector<double> samples(std::size_t piece, std::int64_t a, std::int64_t b) const;

  Dict meta;
  Loaded arrays;
  std::vector<Piece> pieces;

 private:
  // The channel's pieces that lie in [t0, t1], in order of id; with more than px of them, only
  // the first (by start, then id) to start in each pixel column.
  std::vector<std::size_t> pieces_in(std::int64_t ch, double t0, double t1, double px) const;
  std::string raw_block(const Piece& p, std::int64_t a, const std::vector<double>& y, double per_px) const;
  std::string env_block(const Piece& p, std::int64_t a, std::int64_t b, double per_px) const;
  Value m(const char* k) const;
  // The pieces of one channel, for finding those in a window quickly: in order of their start
  // (then id), the latest end of the pieces up to each, and the margin around a piece's span that
  // a window may still take samples from (three samples at its lowest rate).
  struct ChannelPieces {
    std::vector<std::size_t> order;
    std::vector<double> max_end;
    double margin = 0.0;
  };
  struct Series {
    std::string unit;
    std::size_t offset, n;
  };
  OrderedMap<std::int64_t, ChannelPieces> by_channel_;
  std::unique_ptr<Reader> reader_;
  OrderedMap<std::string, Series> vitals_;
};

}  // namespace vitalstream::engine
