// WFDB records (docs/wfdb-collection.md), read with wfdb-x (core/wfdb_c.hpp): one record (its
// .hea file, single- or multi-segment), a WFDB collection (records described by index.csv), or a
// folder of plain WFDB records, subfolders included, read as one session.  Mirrors the Rust
// engine's sources/wfdb_folder.rs.
//
// Each signal of each record (each segment of a multi-segment record) is a piece.  Start times
// come from index.csv (UTC) or from the headers (base date and time, taken as UTC); records
// without one are placed after the data before them and marked "time not stated" (records of the
// same stretch share that start).  The segments of a multi-segment record follow each other from
// the record's start; its null segments are breaks of known length.  A piece that would overlap
// another piece of its channel goes into a second lane of that channel ("II (2)"), so overlapping
// records are never drawn on top of each other.  Vital signs, alarms, breaks and markers come
// from vitals.csv, events.csv, gaps.csv and the .mrk annotation files.  Nothing is assumed that a
// header does not state: a record without a sampling frequency is left out, an uncalibrated
// signal (gain 0) is shown in ADC units ("adu"), and signals without units have none.
//
// Built with VS_WFDB=ON (the default); with VS_WFDB=OFF, sources_wfdb_none.cpp takes its place.
// What needs no wfdb-x (which paths are records, segment tables, limits, a folder's key) is in
// sources_wfdb_files.cpp, built in both.
#include <algorithm>
#include <cctype>
#include <charconv>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <mutex>
#include <set>
#include <stdexcept>

#include "engine/sources.hpp"
#include "core/paths.hpp"
#include "core/text.hpp"
#include "core/pyfmt.hpp"
#include "core/wfdb_c.hpp"
#include "wfdb_files.hpp"

namespace vitalstream::engine {

using namespace wfdb_files;

namespace {

using Row = OrderedMap<std::string, std::string>;

// Samples (frames x signals) decoded at once; at least MIN_CHUNK_FRAMES frames.
constexpr std::int64_t CHUNK_SAMPLES = 1 << 20;
constexpr std::int64_t MIN_CHUNK_FRAMES = 4096;
const char* const SPLIT_REASONS[4] = {"channel_absent", "channel_set_change", "label_change", "packet_count_change"};

// Rows of a CSV file (Python's csv module, excel dialect) as dicts of strings.
std::vector<Row> read_csv(const fs::path& path) {
  auto text = read_text(path);
  if (!text || !core::utf8_valid(*text)) return {};
  std::vector<std::vector<std::string>> rows;
  std::string field;
  std::vector<std::string> row;
  bool quoted = false, any = false;
  const std::string& t = *text;
  for (std::size_t i = 0; i < t.size(); ++i) {
    char c = t[i];
    if (quoted) {
      if (c == '"') {
        if (i + 1 < t.size() && t[i + 1] == '"') {
          field += '"';
          ++i;
        } else {
          quoted = false;
        }
      } else {
        field += c;
      }
      continue;
    }
    switch (c) {
      case '"':
        quoted = true;
        any = true;
        break;
      case ',':
        row.push_back(std::move(field));
        field.clear();
        any = true;
        break;
      case '\r': break;
      case '\n':
        row.push_back(std::move(field));
        field.clear();
        rows.push_back(std::move(row));
        row.clear();
        any = false;
        break;
      default:
        field += c;
        any = true;
    }
  }
  if (any || !field.empty()) {
    row.push_back(field);
    rows.push_back(row);
  }
  std::vector<Row> out;
  if (rows.empty()) return out;
  const auto head = rows[0];
  for (std::size_t r = 1; r < rows.size(); ++r) {
    if (rows[r].size() == 1 && rows[r][0].empty()) continue;
    Row m;
    for (std::size_t k = 0; k < head.size(); ++k) m.insert(head[k], k < rows[r].size() ? rows[r][k] : std::string());
    out.push_back(std::move(m));
  }
  return out;
}

const std::string* at(const Row& r, const char* k) { return r.get(k); }

std::optional<double> fl(const std::string* v) {
  if (!v) return std::nullopt;
  auto x = core::py_float(*v);
  if (x && std::isfinite(*x)) return x;
  return std::nullopt;
}

Value sv(const std::string* v) { return v ? Value(*v) : Value(); }

Value int_value(const std::string* v) {
  auto f = fl(v);
  return f ? Value(static_cast<std::int64_t>(std::trunc(*f))) : Value();
}

// Rust's text of an f64 ({} in format!): the shortest digits that read back, never an exponent.
std::string rust_f64(double x) {
  char buf[512];
  auto r = std::to_chars(buf, buf + sizeof buf, x, std::chars_format::fixed);
  return std::string(buf, r.ptr);
}

std::string shown(const fs::path& p) { return core::path_utf8(p); }

// The home folder (HOME, else USERPROFILE; on Windows the other way round).
std::string home_folder() {
#ifdef _WIN32
  const char* names[2] = {"USERPROFILE", "HOME"};
#else
  const char* names[2] = {"HOME", "USERPROFILE"};
#endif
  for (const char* n : names)
    if (const char* v = std::getenv(n)) return v;
#ifdef _WIN32
  return "C:\\";
#else
  return "/";
#endif
}

bool is_sep(char c) {
#ifdef _WIN32
  return c == '/' || c == '\\';
#else
  return c == '/';
#endif
}

// The file a signal file name of a header designates, as wfdb-x opens it: ~ expanded, an absolute
// name as given, any other next to the header.  Nothing for a signal without a file.  record: the
// record's path without .hea.
std::optional<fs::path> signal_file(const fs::path& record, const std::string& fname) {
  if (fname.empty() || fname == "-" || fname == "~") return std::nullopt;
  std::string f = fname;
  if (f.size() >= 2 && f[0] == '~' && is_sep(f[1])) {
    std::string h = home_folder();
    while (!h.empty() && is_sep(h.back())) h.pop_back();
#ifdef _WIN32
    f = h + "\\" + f.substr(2);
#else
    f = h + "/" + f.substr(2);
#endif
  }
  fs::path p = core::utf8_path(f);
  if (p.is_absolute()) return p;
  return record.parent_path() / p;
}

// Three times the bits a sample of WFDB storage format fmt takes, for the formats of fixed size
// (nothing for the others: compressed, or unknown).
std::optional<std::uint64_t> bits3(int fmt) {
  switch (fmt) {
    case 8: case 80: return 24;
    case 16: case 61: case 160: return 48;
    case 212: return 36;
    case 310: case 311: return 32;
    case 24: return 72;
    case 32: return 96;
    default: return std::nullopt;
  }
}

// The most frames the signal files of a record can hold: for each file of signals of a fixed size,
// ceil(8 x bytes / bits per frame), the least of them (every frame stated when no file can be
// measured).  A header that states more samples is read up to this many.
std::int64_t frames_held(const fs::path& record, const core::wfdbc::HeaderInfo& h) {
  OrderedMap<std::string, std::optional<std::uint64_t>> per_file;  // file -> 3 x bits per frame
  std::vector<fs::path> files;
  for (const auto& sig : h.signals) {
    auto f = signal_file(record, sig.file_name);
    if (!f) continue;
    std::string key = core::path_utf8(*f);
    std::optional<std::uint64_t> bits;
    if (auto b = bits3(sig.fmt)) bits = *b * static_cast<std::uint64_t>(std::max(sig.spf, 1));
    if (!per_file.get(key)) {
      per_file.insert(key, std::optional<std::uint64_t>(0));
      files.push_back(*f);
    }
    auto* e = per_file.get(key);
    *e = (*e && bits) ? std::optional<std::uint64_t>(**e + *bits) : std::nullopt;
  }
  std::int64_t held = INT64_MAX;
  for (const auto& f : files) {
    auto bits = *per_file.get(core::path_utf8(f));
    std::error_code ec;
    std::uintmax_t len = fs::file_size(f, ec);
    if (!bits || ec || *bits == 0) continue;
    // 24 x bytes / (3 x bits), rounded up, in parts that do not overflow (saturating)
    const std::uint64_t q = len / *bits, r = len % *bits;
    const std::uint64_t whole = q > static_cast<std::uint64_t>(INT64_MAX) / 24 ? static_cast<std::uint64_t>(INT64_MAX) : q * 24;
    const std::uint64_t part = r <= UINT64_MAX / 24 ? (r * 24 + *bits - 1) / *bits : 24;
    const std::uint64_t frames = whole > static_cast<std::uint64_t>(INT64_MAX) - part ? static_cast<std::uint64_t>(INT64_MAX) : whole + part;
    held = std::min(held, static_cast<std::int64_t>(frames));
  }
  return held;
}

// The header of a record (no signal file is opened); an error for a record that states no
// sampling frequency (or one that is not a finite positive number).  The number of samples is at
// most what the signal files can hold (frames_held).  record: the record's path without .hea.
core::wfdbc::HeaderInfo header(const fs::path& record) {
  auto h = core::wfdbc::read_header(record.parent_path(), core::path_utf8(record.filename()));
  if (!(h.fs > 0 && std::isfinite(h.fs)))
    throw std::runtime_error(shown(record) + ": the header states no sampling frequency");
  h.sig_len = std::min(std::max<std::int64_t>(h.sig_len, 0), frames_held(record, h));
  return h;
}

// Frames decoded at once for a record of nsig signals.
std::int64_t chunk_frames(std::size_t nsig) {
  return std::max<std::int64_t>(CHUNK_SAMPLES / static_cast<std::int64_t>(std::max<std::size_t>(nsig, 1)),
                                MIN_CHUNK_FRAMES);
}

// A record name from index.csv that stays inside its folder: relative, without "..".
bool safe_name(const std::string& name) {
  if (name.empty() || name[0] == '/' || name[0] == '\\' || name[0] == '~') return false;
  fs::path p = core::utf8_path(name);
  if (p.has_root_name() || p.has_root_directory()) return false;
  for (const auto& c : p)
    if (c == "..") return false;
  return true;
}

// `name` of a segment, next to its record ("dir/rec" -> "dir/seg"), with '/' between folders.
std::string beside(const std::string& record, const std::string& name) {
  std::size_t i = record.rfind('/');
  return i == std::string::npos ? name : record.substr(0, i) + "/" + name;
}

// Name and unit of each signal: the description, and the units ("adu" for an uncalibrated
// signal; empty when none are stated).
std::vector<std::pair<std::string, std::string>> names_units(const core::wfdbc::HeaderInfo& h) {
  std::vector<std::pair<std::string, std::string>> out;
  for (const auto& s : h.signals) out.emplace_back(s.description, s.gain == 0.0 ? std::string("adu") : s.units);
  return out;
}

// The refusal for a folder over a limit, with what to do instead.
std::runtime_error over_limit(const std::string& what) {
  return std::runtime_error(what +
                            ": open a subfolder, or describe the records in an index.csv (a WFDB collection, "
                            "docs/wfdb-collection.md)");
}

using Clock = std::chrono::steady_clock;

// Header files below `folder`, as record names (relative, without .hea, '/' between folders),
// sorted.  Hidden entries ('.' names) and symbolic links are skipped.  Over a limit (records,
// depth, or `deadline` for the scan) it is an error.
void walk_records(const fs::path& dir, const std::string& rel, std::size_t depth, const Limits& limits,
                  std::optional<Clock::time_point> deadline, std::vector<std::string>& out) {
  // reached, not passed: a clock that ticks every 15 ms (Windows) still ends a 0 s limit
  if (deadline && Clock::now() >= *deadline)
    throw over_limit("counting the records took more than " + rust_f64(limits.max_scan_s) + " s");
  std::error_code ec;
  fs::directory_iterator it(dir, ec);
  if (ec) return;
  std::vector<std::pair<std::string, bool>> entries;  // name, folder
  for (; it != fs::directory_iterator(); it.increment(ec)) {
    if (ec) break;
    std::error_code ec2;
    fs::file_status st = it->symlink_status(ec2);
    if (ec2) continue;
    std::string name = core::path_utf8(it->path().filename());
    if (!name.empty() && name[0] == '.') continue;
    if (fs::is_symlink(st)) continue;
    entries.emplace_back(std::move(name), fs::is_directory(st));
  }
  std::sort(entries.begin(), entries.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
  for (const auto& [name, folder] : entries) {
    std::string r = rel.empty() ? name : rel + "/" + name;
    if (folder) {
      if (depth + 1 > limits.max_depth)
        throw over_limit("the folder has records nested more than " + std::to_string(limits.max_depth) +
                         " levels deep");
      walk_records(dir / core::utf8_path(name), r, depth + 1, limits, deadline, out);
    } else if (ends_with(r, ".hea")) {
      out.push_back(r.substr(0, r.size() - 4));
      if (out.size() > limits.max_records)
        throw over_limit("the folder holds more than " + std::to_string(limits.max_records) + " WFDB records");
    }
  }
}

std::vector<std::string> walk(const fs::path& folder, const Limits& limits, std::optional<Clock::time_point> deadline) {
  std::vector<std::string> out;
  walk_records(folder, "", 0, limits, deadline, out);
  std::sort(out.begin(), out.end());
  return out;
}

// The records of a folder that are not segments (or layouts) of a multi-segment record in it.
std::vector<std::string> plain_records(const fs::path& folder, const std::vector<std::string>& names) {
  std::set<std::string> claimed;
  for (const auto& n : names)
    for (const auto& [seg, samples] : segment_table(join(folder, n)).value_or(Segments{}))
      if (seg != "~") claimed.insert(beside(n, seg));
  std::vector<std::string> out;
  for (const auto& n : names)
    if (!claimed.count(n)) out.push_back(n);
  return out;
}

// The single-segment record holding samples of a record: itself, or one segment of a
// multi-segment record, `offset` samples after the record's start.
struct Part {
  std::string rec;
  std::int64_t offset = 0;
  core::wfdbc::HeaderInfo h;
};

// A record to read: index.csv's row (or one made from the header), and its parts.
struct Unit {
  std::string name;  // the record's name, relative to the folder opened
  Row row;
  double fs = 0;
  std::int64_t n = 0;
  std::vector<Part> parts;
  std::vector<std::pair<std::int64_t, std::int64_t>> nulls;  // null segments: (offset, samples)
  std::optional<core::wfdbc::HeaderInfo> master;             // of a multi-segment record
};

// The record `name` (relative to base), with `row` from index.csv or nothing.
Unit unit(const fs::path& base, const std::string& name, std::optional<Row> row) {
  check_files(base, name);
  fs::path path = join(base, name);
  auto h = header(path);
  Unit u;
  u.name = name;
  if (row) {
    u.row = std::move(*row);
  } else {
    u.row.insert("record", name);
    u.row.insert("modality", "wfdb");
    u.row.insert("start_unix", h.start_unix ? core::repr(*h.start_unix) : std::string());
  }
  u.fs = h.fs;
  auto segs = segment_table(path);
  if (!segs) {
    u.n = std::max<std::int64_t>(h.sig_len, 0);
    u.parts.push_back(Part{name, 0, std::move(h)});
    return u;
  }
  std::int64_t offset = 0;
  for (const auto& [seg, n] : *segs) {
    if (seg == "~") {
      if (n > 0) u.nulls.emplace_back(offset, n);
    } else if (n > 0) {
      std::string rec = beside(name, seg);
      u.parts.push_back(Part{rec, offset, header(join(base, rec))});
    }
    std::int64_t k = std::max<std::int64_t>(n, 0);  // a layout segment has no samples
    offset = offset > INT64_MAX - k ? INT64_MAX : offset + k;
  }
  u.n = offset;
  u.master = std::move(h);
  return u;
}

// What is opened: the records and how they were found.
struct Opened {
  fs::path base;  // the folder record names are relative to
  std::vector<Unit> units;
  bool collection = false;                                 // index.csv describes the records
  bool single = false;                                     // one record (a .hea file)
  std::vector<std::pair<std::string, std::string>> skipped;  // records left out, with the reason
};

// read_csv of a file the readers may open (none otherwise: allowed_file).
std::vector<Row> read_csv_within(const fs::path& path) { return allowed_file(path) ? read_csv(path) : std::vector<Row>(); }

Opened open(const fs::path& path, const Limits& limits, std::optional<Clock::time_point> deadline) {
  Opened o;
  if (is_record(path)) {
    o.base = path.parent_path();
    o.units.push_back(unit(o.base, core::path_utf8(path.stem()), std::nullopt));
    o.single = true;
    return o;
  }
  o.base = path;
  auto rows = read_csv_within(o.base / "index.csv");
  o.collection = !rows.empty();
  if (o.collection) {
    // a record that cannot be read is left out, as in a folder of plain records
    for (std::size_t i = 0; i < rows.size(); ++i) {
      const std::string* rec = rows[i].get("record");
      if (!rec) throw std::runtime_error(shown(path) + ": index.csv has no record column");
      std::string name = *rec;
      if (!safe_name(name)) {
        o.skipped.emplace_back(name.empty() ? "row " + std::to_string(i + 1) : name,
                               "index.csv names a record outside its folder (an absolute name, or one with ..)");
        continue;
      }
      try {
        o.units.push_back(unit(o.base, name, std::move(rows[i])));
      } catch (const std::exception& e) {
        o.skipped.emplace_back(name, e.what());
      }
    }
  } else {
    for (const auto& name : plain_records(o.base, walk(o.base, limits, deadline))) {
      try {
        o.units.push_back(unit(o.base, name, std::nullopt));
      } catch (const std::exception& e) {
        o.skipped.emplace_back(name, e.what());
      }
    }
  }
  if (o.units.empty()) throw std::runtime_error("no WFDB records in or below " + shown(path));
  return o;
}

std::string rec_path(const std::vector<Unit>& units, const std::string& name) {
  for (const auto& u : units) {
    const std::string& rec = u.name;
    if (rec == name || ends_with(rec, "/" + name)) return rec;
  }
  return name;
}

// Start of each record on the time axis, and whether it is stated: records in order; those
// without a start follow the data before them, after the break gaps.csv states before them
// (gap_s), if any; records of the same stretch start together.
std::vector<std::pair<double, bool>> placement(const std::vector<Unit>& units, const std::vector<Row>& gap_rows) {
  OrderedMap<std::string, double> counted;
  for (const auto& g : gap_rows) {
    auto gs = fl(at(g, "gap_s"));
    const std::string* scope = at(g, "scope");
    if (gs && scope && *scope == "all") {
      const std::string* after = at(g, "records_after");
      for (const auto& name : split(after ? *after : std::string(), ';')) counted.insert(rec_path(units, name), *gs);
    }
  }
  std::vector<std::pair<double, bool>> out;
  out.reserve(units.size());
  std::optional<double> last_end;
  OrderedMap<std::string, double> untimed_group;
  for (std::size_t i = 0; i < units.size(); ++i) {
    const Row& r = units[i].row;
    double dur = static_cast<double>(units[i].n) / units[i].fs;
    std::pair<double, bool> s;
    if (auto t = fl(at(r, "start_unix"))) {
      s = {*t, true};
    } else {
      const std::string* st = at(r, "stretch");
      std::string g = (st && !st->empty()) ? *st : "record " + std::to_string(i);
      const double* c = counted.get(units[i].name);
      double& t0 = untimed_group.entry(g, last_end.value_or(0.0) + (c ? *c : 0.0));
      s = {t0, false};
    }
    out.push_back(s);
    double end = s.first + dur;
    last_end = last_end ? std::max(*last_end, end) : end;
  }
  return out;
}

// Lanes of each channel: the time spans taken in each, to put a piece in the first lane it does
// not overlap.
struct Lanes {
  OrderedMap<std::string, std::vector<std::vector<std::pair<double, double>>>> taken;

  // The lane (1, 2, ...) of a piece of channel `key` over [t0, t1).
  std::size_t place(const std::string& key, double t0, double t1) {
    auto& lanes = taken.entry(key, {});
    for (std::size_t k = 0; k < lanes.size(); ++k) {
      bool free = std::all_of(lanes[k].begin(), lanes[k].end(),
                              [&](const auto& ab) { return t1 <= ab.first + 1e-9 || t0 >= ab.second - 1e-9; });
      if (free) {
        lanes[k].emplace_back(t0, t1);
        return k + 1;
      }
    }
    lanes.push_back({{t0, t1}});
    return lanes.size();
  }
};

}  // namespace

bool wfdb_supported() { return true; }

Built build_wfdb_folder(const fs::path& path, const Progress& progress, const Cancelled& cancelled) {
  const Limits lim = limits();
  Opened opened = open(path, lim, std::nullopt);
  const fs::path& base = opened.base;
  const auto& units = opened.units;
  Channels channels;
  Pieces pieces;
  Vitals vitals;
  std::uint64_t total_n = 0;  // saturating
  for (const auto& u : units)
    for (const auto& p : u.parts) {
      auto n = static_cast<std::uint64_t>(std::max<std::int64_t>(p.h.sig_len, 0));
      std::uint64_t k = p.h.signals.empty() || n <= UINT64_MAX / p.h.signals.size() ? n * p.h.signals.size() : UINT64_MAX;
      total_n = total_n > UINT64_MAX - k ? UINT64_MAX : total_n + k;
    }
  const double total = static_cast<double>(std::max<std::uint64_t>(total_n, 1));
  // the CSV files beside the records (a collection's; a folder of plain records may have them too)
  auto csv = [&](const char* name) { return opened.single ? std::vector<Row>() : read_csv_within(base / name); };
  auto gap_rows = csv("gaps.csv");
  auto starts = placement(units, gap_rows);

  std::uint64_t done = 0;
  Lanes lanes;
  OrderedMap<std::string, std::vector<std::size_t>> rec_pieces;
  std::vector<std::pair<std::string, std::size_t>> piece_info;
  List null_gaps;
  for (std::size_t i = 0; i < units.size(); ++i) {
    const Unit& u = units[i];
    auto [t0, timed] = starts[i];
    const std::string* mod = at(u.row, "modality");
    std::string modality = (mod && !mod->empty()) ? *mod : "wfdb";
    std::vector<std::size_t> all;
    // per part: its pieces, and the index of each of its signal names
    std::vector<std::pair<std::vector<std::size_t>, OrderedMap<std::string, std::size_t>>> part_pieces;
    for (const auto& part : u.parts) {
      double tp = t0 + static_cast<double>(part.offset) / u.fs;
      double tend = tp + static_cast<double>(std::max<std::int64_t>(part.h.sig_len, 0)) / part.h.fs;
      std::vector<std::size_t> ps;
      OrderedMap<std::string, std::size_t> by_name;
      auto nu = names_units(part.h);
      pieces.room(nu.size());
      for (std::size_t j = 0; j < nu.size(); ++j) {
        const auto& [name, unit_] = nu[j];
        std::string key = "wfdb\x01" + modality + "\x01" + name + "\x01" + unit_;
        std::size_t lane = lanes.place(key, tp, tend);
        std::string label = name;
        if (lane != 1) {
          key += "\x01" + std::to_string(lane);
          label = name + " (" + std::to_string(lane) + ")";
        }
        std::size_t cid = channels.get(key, label, unit_, modality);
        ps.push_back(pieces.start(cid, part.h.fs, Dict{{"t0", Value(tp)}, {"timed", Value(timed)}}));
        piece_info.emplace_back(part.rec, j);
        by_name.entry(name, j);
      }
      all.insert(all.end(), ps.begin(), ps.end());
      fs::path ppath = join(base, part.rec);
      const std::int64_t n = std::max<std::int64_t>(part.h.sig_len, 0);
      const std::int64_t chunk = chunk_frames(part.h.signals.size());
      std::vector<float> xf;
      for (std::int64_t a = 0; a < n;) {
        if (cancelled()) throw std::runtime_error("indexing cancelled");
        std::int64_t b = n - a > chunk ? a + chunk : n;
        auto x = core::wfdbc::read_physical(ppath.parent_path(), core::path_utf8(ppath.filename()), a, b);
        std::size_t got = x.empty() ? 0 : x[0].size();
        for (std::size_t j = 0; j < ps.size() && j < x.size(); ++j) {
          xf.assign(x[j].begin(), x[j].end());
          pieces.add(ps[j], xf.data(), xf.size());
        }
        done += static_cast<std::uint64_t>(got) * part.h.signals.size();
        progress(static_cast<double>(done) / total);
        // the end of the data, whatever the header states
        if (static_cast<std::int64_t>(got) < b - a) break;
        a = b;
      }
      part_pieces.emplace_back(std::move(ps), std::move(by_name));
    }
    rec_pieces.insert(u.name, all);
    // markers: the record's .mrk annotations (of a multi-segment record: the record's, with
    // sample numbers from its start); a marker's name is its text, else the annotation code's
    // mnemonic
    fs::path rec = join(base, u.name);
    if (is_file(with_ext(rec, ".mrk"))) {
      std::vector<core::wfdbc::Annotation> list;
      bool read = true;
      try {
        list = core::wfdbc::read_annotations(rec.parent_path(), core::path_utf8(rec.filename()), "mrk");
      } catch (const std::exception&) {
        read = false;
      }
      std::optional<std::vector<std::pair<std::string, std::string>>> master_names;
      if (read && u.master) master_names = names_units(*u.master);
      for (const auto& a : list) {
        auto c = static_cast<std::size_t>(a.chan);
        std::string mname = trim(a.aux);
        if (mname.empty()) mname = trim(core::wfdbc::mnemonic(a.code));
        std::int64_t s = a.time;
        if (!master_names) {
          const auto& ps = part_pieces[0].first;
          if (c < ps.size()) pieces.marker(ps[c], s, mname);
        } else {
          if (c >= master_names->size()) continue;
          const std::string& sig = (*master_names)[c].first;
          std::optional<std::size_t> k;
          for (std::size_t q = 0; q < u.parts.size() && !k; ++q) {
            const Part& p = u.parts[q];
            if (p.offset <= s && s < p.offset + std::max<std::int64_t>(p.h.sig_len, 0)) k = q;
          }
          if (!k) continue;
          const auto& [ps, by_name] = part_pieces[*k];
          if (const std::size_t* j = by_name.get(sig)) pieces.marker(ps[*j], s - u.parts[*k].offset, mname);
        }
      }
    }
    for (const auto& [off, n] : u.nulls) {
      double a = t0 + static_cast<double>(off) / u.fs, b = t0 + static_cast<double>(off + n) / u.fs;
      null_gaps.push_back(Value(Dict{{"t0", Value(a)},
                                     {"t1", Value(b)},
                                     {"reason", Value("null segment")},
                                     {"gap_s", Value(b - a)},
                                     {"exact", Value(true)},
                                     {"measured_by", Value()},
                                     {"notes", Value("")}}));
    }
  }
  const bool markers_capped = pieces.markers_capped();
  auto fin = pieces.finish();
  auto& items = fin.items;
  for (std::size_t p = 0; p < items.size(); ++p) {
    items[p].insert("record", Value(piece_info[p].first));
    items[p].insert("sig", Value(piece_info[p].second));
  }
  auto end_of = [&](std::size_t p) {
    const Dict& d = items[p];
    return f64_of(d.get("t0")).value_or(0.0) + f64_of(d.get("n")).value_or(0.0) / f64_of(d.get("fs")).value_or(1.0);
  };
  std::optional<double> t_lo, t_hi;
  for (std::size_t p = 0; p < items.size(); ++p) {
    if (auto t = f64_of(items[p].get("t0"))) t_lo = t_lo ? std::min(*t_lo, *t) : *t;
    t_hi = t_hi ? std::max(*t_hi, end_of(p)) : end_of(p);
  }

  // vital signs (vitals.csv: time, name or label, value, unit; rows with a non-zero validity are
  // left out). A label measured by several sources keeps its name for the first source in the
  // file; the others are named with their source ("HR [import]").
  std::vector<Row> vital_rows;
  for (auto& v : csv("vitals.csv")) {
    const std::string* validity = at(v, "validity");
    if (!validity || validity->empty() || *validity == "0") vital_rows.push_back(std::move(v));
  }
  auto label_of = [](const Row& v) {
    for (const char* k : {"label", "name"})
      if (const std::string* s = at(v, k); s && !s->empty()) return *s;
    return std::string();
  };
  OrderedMap<std::string, std::vector<std::string>> sources_of;
  for (const auto& v : vital_rows) {
    const std::string* src = at(v, "source");
    auto& e = sources_of.entry(label_of(v), {});
    std::string s = src ? *src : std::string();
    if (std::find(e.begin(), e.end(), s) == e.end()) e.push_back(s);
  }
  for (const auto& v : vital_rows) {
    std::string label = label_of(v);
    const std::string* src = at(v, "source");
    bool other = src && !src->empty() && sources_of.get(label)->front() != *src;
    std::string name = other ? label + " [" + *src + "]" : label;
    const std::string* value = at(v, "value");
    if (!value) value = at(v, "value1");
    vitals.add(name, sv(at(v, "unit")), sv(at(v, "time")), sv(value));
  }
  // alarms and other events with a duration (events.csv: start_unix, end_unix, level, text, kind)
  std::vector<Dict> alarms;
  for (const auto& e : csv("events.csv")) {
    auto t0 = plausible_time(sv(at(e, "start_unix")), t_lo, t_hi);
    if (!t0) continue;
    auto t1 = plausible_time(sv(at(e, "end_unix")), t_lo, t_hi);
    if (!t1 || *t1 < *t0) t1 = t0;
    const std::string* tx = at(e, "text");
    auto words = split_whitespace(tx ? *tx : std::string());
    std::string text;
    for (std::size_t i = 0; i < words.size(); ++i) text += (i ? " " : "") + words[i];
    const std::string* k = at(e, "kind");
    alarms.push_back(Dict{{"t0", Value(*t0)},
                          {"t1", Value(*t1)},
                          {"text", Value(text)},
                          {"level", int_value(at(e, "level"))},
                          {"kind", Value(k && !k->empty() ? *k : std::string("event"))}});
  }
  sort_by_t0(alarms);

  List gaps;
  for (const auto& g : gap_rows) {
    const std::string* rs = at(g, "reason");
    std::string reason = rs ? *rs : std::string();
    if (std::find(std::begin(SPLIT_REASONS), std::end(SPLIT_REASONS), reason) != std::end(SPLIT_REASONS))
      continue;  // a new record, not a break in the data
    auto collect = [&](const char* k) {
      std::vector<std::size_t> out;
      const std::string* v = at(g, k);
      for (const auto& r : split(v ? *v : std::string(), ';')) {
        if (r.empty()) continue;
        if (const auto* ps = rec_pieces.get(rec_path(units, r))) out.insert(out.end(), ps->begin(), ps->end());
      }
      return out;
    };
    auto before = collect("records_before"), after = collect("records_after");
    auto t0 = fl(at(g, "end_before_unix"));
    auto t1 = fl(at(g, "start_after_unix"));
    if (!t0 && !before.empty())
      for (auto p : before) t0 = t0 ? std::max(*t0, end_of(p)) : end_of(p);
    if (!t1 && !after.empty())
      for (auto p : after)
        if (auto t = f64_of(items[p].get("t0"))) t1 = t1 ? std::min(*t1, *t) : *t;
    if (!t0 && !t1) continue;
    const std::string* sc = at(g, "scope");
    std::string scope = (sc && !sc->empty()) ? *sc : "all";
    const std::string* ex = at(g, "exact");
    const std::string* notes = at(g, "notes");
    Dict row{{"t0", core::opt_value(t0 ? t0 : t1)},
             {"t1", core::opt_value(t1 ? t1 : t0)},
             {"reason", Value(scope == "all" ? reason : scope + ": " + reason)},
             {"gap_s", core::opt_value(fl(at(g, "gap_s")))},
             {"exact", Value(ex && *ex == "True")},
             {"measured_by", sv(at(g, "measured_by"))},
             {"notes", Value(notes ? *notes : std::string())}};
    if (scope != "all") {
      std::vector<std::int64_t> chs;
      for (auto p : before) chs.push_back(int_of(items[p].get("ch")).value_or(0));
      for (auto p : after) chs.push_back(int_of(items[p].get("ch")).value_or(0));
      std::sort(chs.begin(), chs.end());
      chs.erase(std::unique(chs.begin(), chs.end()), chs.end());
      row.insert("channels", Value::list_of(chs));
    }
    gaps.push_back(Value(std::move(row)));
  }
  for (auto& g : null_gaps) gaps.push_back(std::move(g));

  auto vf = vitals.finish();
  fin.arrays.vit_t = std::move(vf.t);
  fin.arrays.vit_v = std::move(vf.v);
  List notes;
  if (!opened.collection) notes.push_back(Value("The headers' start times are taken as UTC."));
  for (const auto& p : items) {
    const Value* t = p.get("timed");
    if (t && t->as_bool() && !*t->as_bool()) {
      notes.push_back(Value(
          "Some records have no start time; they are placed after the data before them and marked \"time not "
          "stated\"."));
      break;
    }
  }
  for (const auto& [key, l] : lanes.taken)
    if (l.size() > 1) {
      notes.push_back(Value(
          "Some records overlap in time: a signal recorded twice at once is shown in a second lane (\"II (2)\")."));
      break;
    }
  if (markers_capped)
    notes.push_back(Value("More than " + std::to_string(MAX_MARKER_NAMES - 1) + " distinct marker names: the others are shown as \"" +
                          OTHER_MARKERS + "\"."));
  if (!opened.skipped.empty()) {
    std::size_t k = opened.skipped.size();
    notes.push_back(Value(k == 1 ? std::string("1 record could not be read and was left out.")
                                 : std::to_string(k) + " records could not be read and were left out."));
  }
  Dict skipped;
  for (const auto& [n, why] : opened.skipped) skipped.insert(n, Value(why));
  List untimed = untimed_spans(items);
  Dict meta{{"channels", dict_list(std::move(channels.items))},
            {"pieces", dict_list(std::move(items))},
            {"vitals", dict_list(std::move(vf.meta))},
            {"alarms", dict_list(std::move(alarms))},
            {"gaps", Value(std::move(gaps))},
            {"untimed", Value(std::move(untimed))},
            {"marker_names", Value::list_of(fin.marker_names)},
            {"skipped", Value(std::move(skipped))},
            {"notes", Value(std::move(notes))},
            {"time_resolution_s", Value()},
            {"reader", Value(Dict())}};
  return {std::move(meta), std::move(fin.arrays)};
}

Dict scan(const fs::path& path, const Limits& limits) {
  const auto t_start = Clock::now();
  // no deadline for a limit too large to state (an infinite one: no time limit)
  std::optional<Clock::time_point> deadline;
  double secs = std::max(limits.max_scan_s, 0.0);
  if (secs < 9.0e9)
    deadline = t_start + std::chrono::duration_cast<Clock::duration>(std::chrono::duration<double>(secs));
  Opened opened = open(path, limits, deadline);
  auto starts = placement(opened.units, {});
  std::vector<std::string> signals;
  std::set<std::string> seen;
  std::size_t segments = 0, untimed = 0;
  double samples_s = 0;
  std::optional<double> lo, hi;
  std::vector<std::string> files;
  std::set<std::string> seen_files;
  for (std::size_t i = 0; i < opened.units.size(); ++i) {
    const Unit& u = opened.units[i];
    auto [t0, timed] = starts[i];
    if (u.master) segments += u.parts.size();
    double dur = static_cast<double>(u.n) / u.fs;
    samples_s += dur;
    if (timed) {
      lo = lo ? std::min(*lo, t0) : t0;
      hi = hi ? std::max(*hi, t0 + dur) : t0 + dur;
    } else {
      ++untimed;
    }
    for (const auto& p : u.parts)
      for (const auto& [name, unit_] : names_units(p.h))
        if (seen.insert(name).second) signals.push_back(name);
    for (auto& f : record_files(opened.base, u.name))
      if (seen_files.insert(f).second) files.push_back(f);
  }
  std::uint64_t bytes = 0;
  for (const auto& f : files) {
    std::error_code ec;
    std::uintmax_t n = fs::file_size(join(opened.base, f), ec);
    if (!ec) bytes += n;
  }
  double elapsed = std::chrono::duration<double>(Clock::now() - t_start).count();
  return Dict{{"path", Value(core::path_utf8(path))},
              {"format", Value(format_label(path))},
              {"records", Value(static_cast<unsigned long long>(opened.units.size()))},
              {"segments", Value(static_cast<unsigned long long>(segments))},
              {"signals", Value::list_of(signals)},
              {"duration_s", Value(samples_s)},
              {"t_range", lo && hi ? Value(List{Value(*lo), Value(*hi)}) : Value()},
              {"untimed", Value(static_cast<unsigned long long>(untimed))},
              {"bytes", Value(static_cast<unsigned long long>(bytes))},
              {"skipped", Value(static_cast<unsigned long long>(opened.skipped.size()))},
              {"scan_s", Value(core::round_to(elapsed, 2))}};
}

namespace {

void add_record_files(const fs::path& base, const std::string& name, std::set<std::string>& out) {
  std::size_t i = name.rfind('/');
  std::string dir = i == std::string::npos ? std::string() : name.substr(0, i + 1);
  out.insert(name + ".hea");
  fs::path p = join(base, name);
  try {
    for (const auto& s : core::wfdbc::read_header(p.parent_path(), core::path_utf8(p.filename())).signals)
      if (!s.file_name.empty() && s.file_name != "-" && s.file_name != "~") out.insert(dir + s.file_name);
  } catch (const std::exception&) {
  }
  if (is_file(join(base, name + ".mrk"))) out.insert(name + ".mrk");
}

}  // namespace

std::vector<std::string> record_files(const fs::path& base, const std::string& name) {
  std::set<std::string> out;
  add_record_files(base, name, out);
  for (const auto& [seg, n] : segment_table(join(base, name)).value_or(Segments{}))
    if (seg != "~") add_record_files(base, beside(name, seg), out);
  return {out.begin(), out.end()};
}

std::vector<fs::path> files_read(const fs::path& base, const std::string& name) {
  std::vector<fs::path> out;
  std::set<std::string> seen;
  auto put = [&](const fs::path& f) {
    if (seen.insert(core::path_utf8(f)).second) out.push_back(f);
  };
  auto add = [&](const std::string& rec_name) {
    fs::path rec = join(base, rec_name);
    put(join(base, rec_name + ".hea"));
    try {
      for (const auto& s : core::wfdbc::read_header(rec.parent_path(), core::path_utf8(rec.filename())).signals)
        if (auto f = signal_file(rec, s.file_name)) put(*f);
    } catch (const std::exception&) {
    }
    fs::path mrk = join(base, rec_name + ".mrk");
    std::error_code ec;
    if (fs::exists(fs::symlink_status(mrk, ec))) put(mrk);
  };
  add(name);
  for (const auto& [seg, n] : segment_table(join(base, name)).value_or(Segments{}))
    if (seg != "~") add(beside(name, seg));
  return out;
}

void check_files(const fs::path& base, const std::string& name) {
  for (const auto& f : files_read(base, name))
    if (!allowed_file(f))
      throw std::runtime_error(core::path_utf8(f) + " lies outside the data roots: the record " + name + " is not read");
}

namespace {

class WfdbReader : public Reader {
 public:
  WfdbReader(const fs::path& path, const Dict& meta) : folder_(is_record(path) ? path.parent_path() : path) {
    const Value* pv = meta.get("pieces");
    if (const List* l = pv ? pv->as_list() : nullptr)
      for (const auto& p : *l) {
        const Value* r = p.get("record");
        pieces_.emplace_back(r && r->as_str() ? *r->as_str() : std::string(),
                             static_cast<std::size_t>(int_of(p.get("sig")).value_or(0)));
      }
  }

  std::vector<double> read(std::size_t piece, std::int64_t a, std::int64_t b) const override {
    return std::move(read_many({{piece, a, b}})[0]);
  }

  // The signals of one record over the same samples are decoded together, once.
  std::vector<std::vector<double>> read_many(
      const std::vector<std::tuple<std::size_t, std::int64_t, std::int64_t>>& wanted) const override {
    // (record, a, b) -> (position in wanted, signal), in order of first request
    OrderedMap<std::string, std::vector<std::pair<std::size_t, std::size_t>>> groups;
    std::vector<std::tuple<std::string, std::int64_t, std::int64_t>> keys;
    for (std::size_t k = 0; k < wanted.size(); ++k) {
      auto [piece, a, b] = wanted[k];
      if (piece >= pieces_.size()) throw std::runtime_error("no piece " + std::to_string(piece) + " in this recording");
      const auto& [rec, sig] = pieces_[piece];
      a = std::max<std::int64_t>(a, 0);
      b = std::max<std::int64_t>(b, 0);
      std::string key = rec + '\0' + std::to_string(a) + '\0' + std::to_string(b);
      if (!groups.get(key)) {
        groups.insert(key, {});
        keys.emplace_back(rec, a, b);
      }
      groups.get(key)->emplace_back(k, sig);
    }
    std::vector<std::vector<double>> out(wanted.size());
    for (const auto& [rec, a0, b0] : keys) {
      const auto& items = *groups.get(rec + '\0' + std::to_string(a0) + '\0' + std::to_string(b0));
      check_files(folder_, rec);
      fs::path path = join(folder_, rec);
      auto h = header(path);
      std::int64_t a = a0, b = std::min(b0, std::max<std::int64_t>(h.sig_len, 0));
      std::vector<std::size_t> sigs;
      for (const auto& [k, sig] : items)
        if (sig < h.signals.size()) sigs.push_back(sig);
      if (sigs.empty()) continue;
      auto x = core::wfdbc::read_physical(path.parent_path(), core::path_utf8(path.filename()), a, std::max(a, b), sigs);
      std::size_t j = 0;
      for (const auto& [k, sig] : items)
        if (sig < h.signals.size()) out[k] = j < x.size() ? x[j++] : std::vector<double>();
    }
    return out;
  }

 private:
  fs::path folder_;
  std::vector<std::pair<std::string, std::size_t>> pieces_;
};

}  // namespace

std::unique_ptr<Reader> make_wfdb_reader(const fs::path& path, const Dict& meta) {
  return std::make_unique<WfdbReader>(path, meta);
}

}  // namespace vitalstream::engine
