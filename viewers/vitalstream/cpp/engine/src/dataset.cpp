// An indexed recording and its API responses (dataset.hpp).
#include "engine/dataset.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>
#include <tuple>

#include "core/pyfmt.hpp"

namespace vitalstream::engine {

namespace {

constexpr double RAW_PER_PX = 2.0;          // send samples as they are up to this many per pixel
constexpr std::int64_t MAX_RAW = 2000000;   // never decode more samples than this for one window
constexpr double VITAL_MAX_AGE_S = 600.0;   // vitals_at: values older than this are not reported
const double NaN = std::numeric_limits<double>::quiet_NaN();

std::string json_array(const std::vector<double>& x, int digits) {
  std::string s;
  s.reserve(x.size() * static_cast<std::size_t>(digits + 2) + 2);
  s += '[';
  for (std::size_t i = 0; i < x.size(); ++i) {
    if (i) s += ',';
    if (std::isfinite(x[i]))
      s += core::g(x[i], digits);
    else
      s += "null";
  }
  s += ']';
  return s;
}

// Rust's `as i64` of a float: saturating, NaN -> 0.
std::int64_t sat_i64(double x) {
  if (std::isnan(x)) return 0;
  if (x >= 9223372036854775807.0) return INT64_MAX;
  if (x <= -9223372036854775808.0) return INT64_MIN;
  return static_cast<std::int64_t>(x);
}

// Rust's f64::total_cmp as an integer key (NaN after +inf).
std::int64_t total_key(double x) {
  std::int64_t i;
  std::memcpy(&i, &x, sizeof i);
  return i ^ static_cast<std::int64_t>(static_cast<std::uint64_t>(i >> 63) >> 1);
}

// Index of the first element > x (right) or >= x (left).
std::size_t search(const double* v, std::size_t n, double x, bool right) {
  const double* e = v + n;
  return static_cast<std::size_t>((right ? std::upper_bound(v, e, x) : std::lower_bound(v, e, x)) - v);
}

}  // namespace

Dataset::Dataset(Dict meta_, Loaded arrays_) : meta(std::move(meta_)), arrays(std::move(arrays_)) {
  if (const Value* pv = meta.get("pieces"); pv && pv->as_list()) {
    for (const auto& p : *pv->as_list()) {
      Piece x;
      x.id = static_cast<std::size_t>(int_of(p.get("id")).value_or(0));
      x.ch = int_of(p.get("ch")).value_or(0);
      // load_index checks that every piece states its rate and start
      x.fs = f64_of(p.get("fs")).value_or(NaN);
      x.n = int_of(p.get("n")).value_or(0);
      x.t0 = f64_of(p.get("t0")).value_or(0.0);
      const Value* timed = p.get("timed");
      x.timed = !(timed && timed->as_bool() && !*timed->as_bool());
      if (const Value* e = p.get("env"); e && e->as_list())
        for (const auto& y : *e->as_list()) {
          const List* l = y.as_list();
          if (!l || l->size() < 2) continue;
          auto a = int_of(&(*l)[0]), b = int_of(&(*l)[1]);
          if (a && b) x.env.emplace_back(static_cast<std::size_t>(*a), static_cast<std::size_t>(*b));
        }
      pieces.push_back(std::move(x));
    }
  }
  for (std::size_t i = 0; i < pieces.size(); ++i) by_channel_.entry(pieces[i].ch).order.push_back(i);
  for (auto& [ch, c] : by_channel_) {
    std::stable_sort(c.order.begin(), c.order.end(), [&](std::size_t a, std::size_t b) {
      return total_key(pieces[a].t0) != total_key(pieces[b].t0) ? total_key(pieces[a].t0) < total_key(pieces[b].t0)
                                                                : a < b;
    });
    double latest = -std::numeric_limits<double>::infinity();
    for (std::size_t i : c.order) {
      latest = std::fmax(latest, pieces[i].end());
      c.max_end.push_back(latest);
    }
    for (std::size_t i : c.order) c.margin = std::fmax(c.margin, 3.0 / pieces[i].fs);
  }
  if (const Value* vv = meta.get("vitals"); vv && vv->as_list())
    for (const auto& v : *vv->as_list()) {
      auto s = [&](const char* k) {
        const Value* x = v.get(k);
        return x ? x->py_str() : std::string();
      };
      auto i = [&](const char* k) { return static_cast<std::size_t>(int_of(v.get(k)).value_or(0)); };
      vitals_.insert(s("name"), Series{s("unit"), i("offset"), i("n")});
    }
  reader_ = make_reader(meta, this->arrays.dir);
}

Value Dataset::m(const char* k) const { return get_or_none(meta, k); }

std::size_t Dataset::channel_count() const {
  const Value* c = meta.get("channels");
  return c && c->as_list() ? c->as_list()->size() : 0;
}

std::pair<std::int64_t, std::int64_t> Piece::span(double w0, double w1) const {
  double nf = static_cast<double>(n);
  double a = std::clamp(std::floor((w0 - t0) * fs) - 1.0, 0.0, nf);
  double b = std::clamp(std::ceil((w1 - t0) * fs) + 2.0, 0.0, nf);
  return {sat_i64(a), sat_i64(b)};
}

std::vector<std::size_t> Dataset::pieces_in(std::int64_t ch, double t0, double t1, double px) const {
  const ChannelPieces* c = by_channel_.get(ch);
  if (!c) return {};
  auto hi = static_cast<std::size_t>(
      std::partition_point(c->order.begin(), c->order.end(), [&](std::size_t i) { return pieces[i].t0 < t1 + c->margin; }) -
      c->order.begin());
  auto lo = static_cast<std::size_t>(
      std::partition_point(c->max_end.begin(), c->max_end.begin() + static_cast<std::ptrdiff_t>(hi),
                           [&](double e) { return e + c->margin <= t0; }) -
      c->max_end.begin());
  std::vector<std::size_t> found;
  for (std::size_t k = lo; k < hi; ++k) {
    auto [a, b] = pieces[c->order[k]].span(t0, t1);
    if (b > a) found.push_back(c->order[k]);
  }
  if (static_cast<double>(found.size()) > px) {
    std::vector<std::size_t> kept;
    std::optional<std::int64_t> last;
    for (std::size_t i : found) {
      std::int64_t col = sat_i64(std::floor((std::fmax(pieces[i].t0, t0) - t0) / (t1 - t0) * px));
      if (last != col) kept.push_back(i);
      last = col;
    }
    found = std::move(kept);
  }
  std::sort(found.begin(), found.end());
  return found;
}

std::vector<double> Dataset::samples(std::size_t piece, std::int64_t a, std::int64_t b) const {
  return reader_->read(piece, a, b);
}

Value Dataset::info() const {
  std::vector<double> starts, ends;
  for (const auto& p : pieces) {
    starts.push_back(p.t0);
    ends.push_back(p.t0 + static_cast<double>(p.n) / p.fs);
  }
  if (arrays.vit_t.size() > 0) {
    const double* t = arrays.vit_t.data();
    double lo = std::numeric_limits<double>::infinity(), hi = -std::numeric_limits<double>::infinity();
    for (std::size_t i = 0; i < arrays.vit_t.size(); ++i) {
      lo = std::fmin(lo, t[i]);
      hi = std::fmax(hi, t[i]);
    }
    starts.push_back(lo);
    ends.push_back(hi);
  }
  // f64::min / f64::max (NaN ignored)
  auto reduce = [](const std::vector<double>& v, bool lo) -> Value {
    if (v.empty()) return Value();
    double r = v[0];
    for (std::size_t i = 1; i < v.size(); ++i) r = lo ? std::fmin(r, v[i]) : std::fmax(r, v[i]);
    return Value(r);
  };
  List ps;
  for (const auto& p : pieces)
    ps.push_back(Value(Dict{{"id", Value(p.id)},
                            {"ch", Value(p.ch)},
                            {"t0", Value(p.t0)},
                            {"fs", Value(p.fs)},
                            {"n", Value(p.n)},
                            {"timed", Value(p.timed)}}));
  List vit;
  for (const auto& [k, s] : vitals_) vit.push_back(Value(Dict{{"name", Value(k)}, {"unit", Value(s.unit)}, {"n", Value(s.n)}}));
  const Value* skipped = meta.get("skipped");
  const Value* notes = meta.get("notes");
  const Value* late = meta.get("late_blocks");
  return Value(Dict{{"key", m("key")},
                    {"name", m("name")},
                    {"path", m("path")},
                    {"kind", m("kind")},
                    {"format", m("format")},
                    {"description", m("description")},
                    {"t_range", Value(List{reduce(starts, true), reduce(ends, false)})},
                    {"time_resolution_s", m("time_resolution_s")},
                    {"channels", m("channels")},
                    {"pieces", Value(std::move(ps))},
                    {"vitals", Value(std::move(vit))},
                    {"alarms", m("alarms")},
                    {"gaps", m("gaps")},
                    {"untimed", m("untimed")},
                    {"late_blocks", late ? *late : Value(List())},
                    {"markers", Value(Dict{{"names", m("marker_names")}, {"count", Value(arrays.mk_piece.size())}})},
                    {"skipped", skipped ? *skipped : Value(Dict())},
                    {"notes", notes ? *notes : Value(List())},
                    {"indexed_s", m("indexed_s")}});
}

std::string Dataset::window_json(const std::vector<std::int64_t>& channels, double t0, double t1,
                                 std::int64_t px) const {
  double pxf = static_cast<double>(std::clamp<std::int64_t>(px, 16, 8000));
  double span = t1 - t0;
  // how each block is made: samples decoded (raw), or the index's envelope
  struct Plan {
    std::size_t piece;
    bool raw;
    std::int64_t a, b;
    double per_px;
  };
  std::vector<std::vector<Plan>> plans;
  std::vector<std::tuple<std::size_t, std::int64_t, std::int64_t>> raw;
  for (std::int64_t ch : channels) {
    std::vector<Plan> blocks;
    for (std::size_t i : pieces_in(ch, t0, t1, pxf)) {
      const Piece& p = pieces[i];
      auto [a, b] = p.span(t0, t1);
      double per_px = p.fs * span / pxf;
      bool r = per_px < static_cast<double>(LEVEL0) && b - a <= MAX_RAW;
      blocks.push_back(Plan{i, r, a, b, per_px});
      if (r) raw.emplace_back(p.id, a, b);
    }
    plans.push_back(std::move(blocks));
  }
  // the samples of every raw block, read together
  auto samples = reader_->read_many(raw);
  std::size_t next = 0;
  std::string out = "{\"t0\":" + core::repr(t0) + ",\"t1\":" + core::repr(t1) + ",\"channels\":{";
  for (std::size_t c = 0; c < channels.size(); ++c) {
    if (c) out += ',';
    out += "\"" + std::to_string(channels[c]) + "\":[";
    for (std::size_t k = 0; k < plans[c].size(); ++k) {
      const Plan& plan = plans[c][k];
      if (k) out += ',';
      if (plan.raw) {
        std::vector<double> none;
        out += raw_block(pieces[plan.piece], plan.a, next < samples.size() ? samples[next] : none, plan.per_px);
        ++next;
      } else {
        out += env_block(pieces[plan.piece], plan.a, plan.b, plan.per_px);
      }
    }
    out += ']';
  }
  out += "}}";
  return out;
}

// A block of samples y from sample a: as they are, or min/max of every per_px.
std::string Dataset::raw_block(const Piece& p, std::int64_t a, const std::vector<double>& y, double per_px) const {
  double fs = p.fs;
  std::string id = std::to_string(p.id);
  if (per_px <= RAW_PER_PX)
    return "{\"piece\":" + id + ",\"kind\":\"raw\",\"t0\":" + core::repr(p.t0 + static_cast<double>(a) / fs) +
           ",\"dt\":" + core::repr(1.0 / fs) + ",\"y\":" + json_array(y, 8) + "}";
  std::size_t step = std::max<std::size_t>(static_cast<std::size_t>(per_px), 1);
  std::vector<double> lo, hi;
  for (std::size_t k = 0; k < y.size(); k += step) {
    double l = NaN, h = NaN;
    for (std::size_t j = k; j < std::min(k + step, y.size()); ++j) {
      l = fmin64(l, y[j]);
      h = fmax64(h, y[j]);
    }
    lo.push_back(l);
    hi.push_back(h);
  }
  return "{\"piece\":" + id + ",\"kind\":\"env\",\"t0\":" + core::repr(p.t0 + static_cast<double>(a) / fs) +
         ",\"dt\":" + core::repr(static_cast<double>(step) / fs) + ",\"min\":" + json_array(lo, 7) +
         ",\"max\":" + json_array(hi, 7) + "}";
}

// A block of the index's envelope over samples [a, b), at the coarsest level whose buckets hold at
// most per_px samples.
std::string Dataset::env_block(const Piece& p, std::int64_t a, std::int64_t b, double per_px) const {
  double fs = p.fs;
  std::string id = std::to_string(p.id);
  std::size_t lvl = 0;
  while (lvl + 1 < p.env.size() && static_cast<double>(bucket_size(lvl + 1)) <= per_px) ++lvl;
  auto size = static_cast<std::int64_t>(bucket_size(lvl));
  auto [off, length] = lvl < p.env.size() ? p.env[lvl] : std::pair<std::size_t, std::size_t>(0, 0);
  auto i0 = static_cast<std::size_t>(a / size);
  auto i1 = static_cast<std::size_t>(std::min<std::int64_t>(static_cast<std::int64_t>(length), (b + size - 1) / size));
  std::vector<double> lo, hi;
  if (i1 > i0 && lvl < arrays.env_min.size()) {
    const float* mn = arrays.env_min[lvl].data();
    const float* mx = arrays.env_max[lvl].data();
    for (std::size_t i = off + i0; i < off + i1; ++i) {
      lo.push_back(static_cast<double>(mn[i]));
      hi.push_back(static_cast<double>(mx[i]));
    }
  }
  return "{\"piece\":" + id + ",\"kind\":\"env\",\"t0\":" +
         core::repr(p.t0 + static_cast<double>(static_cast<std::int64_t>(i0) * size) / fs) +
         ",\"dt\":" + core::repr(static_cast<double>(size) / fs) + ",\"min\":" + json_array(lo, 7) +
         ",\"max\":" + json_array(hi, 7) + "}";
}

Value Dataset::vitals_window(const std::vector<std::string>& names, double t0, double t1, std::int64_t px) const {
  Dict out;
  for (const auto& name : names) {
    const Series* s = vitals_.get(name);
    if (!s) continue;
    const double* t = arrays.vit_t.data() + s->offset;
    const double* v = arrays.vit_v.data() + s->offset;
    std::size_t n = s->n;
    std::size_t i0 = search(t, n, t0, false);
    i0 = i0 == 0 ? 0 : i0 - 1;
    std::size_t i1 = std::min(search(t, n, t1, true) + 1, n);
    i1 = std::max(i1, i0);
    std::size_t len = i1 - i0;
    double pxf = static_cast<double>(px);
    if (static_cast<double>(len) > 3.0 * pxf && t1 > t0) {
      std::vector<double> ts, lo, hi;
      std::optional<std::int64_t> last_col;
      for (std::size_t k = 0; k < len; ++k) {
        double tk = t[i0 + k], vk = v[i0 + k];
        auto col = sat_i64(std::floor((tk - t0) / (t1 - t0) * pxf));
        if (last_col != std::optional<std::int64_t>(col) || k == 0) {
          ts.push_back(tk);
          lo.push_back(vk);
          hi.push_back(vk);
          last_col = col;
        } else {
          std::size_t j = lo.size() - 1;
          lo[j] = (vk < lo[j] || std::isnan(vk)) ? vk : lo[j];
          hi[j] = (vk > hi[j] || std::isnan(vk)) ? vk : hi[j];
        }
      }
      out.insert(name, Value(Dict{{"unit", Value(s->unit)},
                                  {"kind", Value("env")},
                                  {"t", Value(core::DoubleArray(ts))},
                                  {"min", Value(core::DoubleArray(lo))},
                                  {"max", Value(core::DoubleArray(hi))}}));
    } else {
      out.insert(name, Value(Dict{{"unit", Value(s->unit)},
                                  {"kind", Value("points")},
                                  {"t", Value(core::DoubleArray(t + i0, t + i1))},
                                  {"v", Value(core::DoubleArray(v + i0, v + i1))}}));
    }
  }
  return Value(std::move(out));
}

Value Dataset::vitals_at(double t) const {
  List out;
  for (const auto& [name, s] : vitals_) {
    const double* ts = arrays.vit_t.data() + s.offset;
    const double* vs = arrays.vit_v.data() + s.offset;
    std::size_t i = search(ts, s.n, t, true);
    if (i > 0 && t - ts[i - 1] <= VITAL_MAX_AGE_S)
      out.push_back(Value(Dict{{"name", Value(name)}, {"unit", Value(s.unit)}, {"value", Value(vs[i - 1])}, {"t", Value(ts[i - 1])}}));
  }
  return Value(std::move(out));
}

Value Dataset::markers(const std::vector<std::int64_t>& channels, double t0, double t1) const {
  constexpr std::size_t LIMIT = 20000;
  const std::int32_t* mk_p = arrays.mk_piece.data();
  const std::int64_t* mk_s = arrays.mk_sample.data();
  const std::int16_t* mk_c = arrays.mk_code.data();
  std::size_t total = arrays.mk_piece.size();
  List items;
  Value names = m("marker_names");
  for (const auto& p : pieces) {
    if (std::find(channels.begin(), channels.end(), p.ch) == channels.end()) continue;
    // saturating: any finite times
    auto a = sat_i64(std::floor((t0 - p.t0) * p.fs));
    auto b = sat_i64(std::ceil((t1 - p.t0) * p.fs));
    if (b < 0 || a > p.n) continue;
    auto id = static_cast<std::int64_t>(p.id);
    std::size_t lo = static_cast<std::size_t>(
        std::partition_point(mk_p, mk_p + total, [&](std::int32_t x) { return static_cast<std::int64_t>(x) < id; }) - mk_p);
    std::size_t hi = static_cast<std::size_t>(
        std::partition_point(mk_p, mk_p + total, [&](std::int32_t x) { return static_cast<std::int64_t>(x) <= id; }) - mk_p);
    std::size_t j0 = static_cast<std::size_t>(
        std::partition_point(mk_s + lo, mk_s + hi, [&](std::int64_t x) { return x < a; }) - mk_s);
    std::size_t j1 = static_cast<std::size_t>(
        std::partition_point(mk_s + lo, mk_s + hi, [&](std::int64_t x) { return x <= b; }) - mk_s);
    for (std::size_t k = j0; k < j1; ++k) {
      items.push_back(Value(List{Value(p.ch), Value(p.t0 + static_cast<double>(mk_s[k]) / p.fs), Value(static_cast<int>(mk_c[k]))}));
      if (items.size() >= LIMIT)
        return Value(Dict{{"names", names}, {"items", Value(std::move(items))}, {"truncated", Value(true)}});
    }
  }
  return Value(Dict{{"names", names}, {"items", Value(std::move(items))}, {"truncated", Value(false)}});
}

}  // namespace vitalstream::engine
