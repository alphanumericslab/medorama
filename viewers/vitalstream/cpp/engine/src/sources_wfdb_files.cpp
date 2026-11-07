// WFDB records as files, without reading them (no wfdb-x): which paths are records and WFDB
// folders, the segment table of a multi-segment record, the format labels, the limits on a folder
// of records, and a folder's files for the cache key.  Built in both configurations: with the
// reader (sources_wfdb.cpp, VS_WFDB=ON) and without it (sources_wfdb_none.cpp, VS_WFDB=OFF), so
// that a build without WFDB support still recognises a WFDB record and refuses it (NO_WFDB)
// rather than taking it for something else.  Mirrors the Rust engine's sources/wfdb_files.rs.
#include "wfdb_files.hpp"

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <mutex>
#include <tuple>

#include "core/paths.hpp"
#include "core/pyfmt.hpp"
#include "core/text.hpp"
#include "engine/sources.hpp"

namespace vitalstream::engine {

namespace wfdb_files {

std::optional<std::string> read_text(const fs::path& p) {
  std::FILE* f = core::open_file(core::path_utf8(p), "rb");
  if (!f) return std::nullopt;
  std::string text;
  char buf[65536];
  std::size_t n;
  while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) text.append(buf, n);
  std::fclose(f);
  return text;
}

std::vector<std::string> split(const std::string& s, char sep) {
  std::vector<std::string> out;
  std::size_t a = 0;
  while (true) {
    std::size_t b = s.find(sep, a);
    out.push_back(s.substr(a, b == std::string::npos ? std::string::npos : b - a));
    if (b == std::string::npos) break;
    a = b + 1;
  }
  return out;
}

bool parse_i64(const std::string& s, std::int64_t& out, bool is_unsigned) {
  std::size_t i = !s.empty() && (s[0] == '+' || (s[0] == '-' && !is_unsigned)) ? 1 : 0;
  if (i == s.size()) return false;
  for (std::size_t k = i; k < s.size(); ++k)
    if (!std::isdigit(static_cast<unsigned char>(s[k]))) return false;
  return core::parse_int(s[0] == '+' ? s.substr(1) : s, out);
}

std::string lossy(const std::string& s) {
  if (core::utf8_valid(s)) return s;
  std::string out;
  for (char32_t c : core::utf8_decode(s)) core::utf8_append(out, c);
  return out;
}

bool ends_with(const std::string& s, const std::string& suffix) {
  return s.size() >= suffix.size() && s.compare(s.size() - suffix.size(), suffix.size(), suffix) == 0;
}

fs::path with_extension(const fs::path& p, const char* ext) {
  fs::path out = p;
  out.replace_extension(ext);
  return out;
}

fs::path with_ext(const fs::path& rec, const char* ext) {
  fs::path out = rec;
  out += ext;
  return out;
}

fs::path join(const fs::path& base, const std::string& name) { return base / core::utf8_path(name); }

bool is_file(const fs::path& p) {
  std::error_code ec;
  return fs::is_regular_file(p, ec);
}

std::vector<std::string> header_lines(const fs::path& hea) {
  auto text = read_text(hea);
  std::vector<std::string> out;
  if (!text) return out;
  for (const auto& l : split(lossy(*text), '\n')) {
    std::string t = trim(l);
    if (!t.empty() && t[0] != '#') out.push_back(std::move(t));
  }
  return out;
}

}  // namespace wfdb_files

using namespace wfdb_files;

namespace {

std::mutex& limits_lock() {
  static std::mutex m;
  return m;
}

std::optional<Limits>& limits_set() {
  static std::optional<Limits> l;
  return l;
}

}  // namespace

void set_limits(const Limits& limits) {
  std::lock_guard<std::mutex> l(limits_lock());
  limits_set() = limits;
}

Limits limits() {
  std::lock_guard<std::mutex> l(limits_lock());
  return limits_set().value_or(Limits{});
}

bool is_wfdb_folder(const fs::path& p, std::size_t scan_limit) {
  std::error_code ec;
  if (!fs::is_directory(p, ec)) return false;
  if (fs::is_regular_file(p / "index.csv", ec) || fs::is_regular_file(p / "RECORDS", ec)) return true;
  std::size_t k = 0;
  for (const auto& e : fs::directory_iterator(p, ec)) {
    if (k++ >= scan_limit) break;
    if (ends_with(core::path_utf8(e.path().filename()), ".hea")) return true;
  }
  return false;
}

bool is_record(const fs::path& p) { return is_file(p) && p.extension() == ".hea"; }

std::optional<Segments> segment_table(const fs::path& record) {
  auto lines = header_lines(with_ext(record, ".hea"));
  if (lines.empty()) return std::nullopt;
  auto first = split_whitespace(lines[0]);
  if (first.empty()) return std::nullopt;
  std::size_t slash = first[0].find('/');
  if (slash == std::string::npos) return std::nullopt;
  std::int64_t nseg;
  if (!parse_i64(first[0].substr(slash + 1), nseg, true)) return std::nullopt;
  Segments out;
  for (std::size_t i = 1; i < lines.size() && static_cast<std::int64_t>(i) <= nseg; ++i) {
    auto w = split_whitespace(lines[i]);
    std::int64_t n;
    if (w.size() >= 2 && parse_i64(w[1], n)) out.emplace_back(w[0], n);
  }
  return out;
}

const char* format_label(const fs::path& path) {
  if (is_record(path))
    return segment_table(with_extension(path, "")) ? "WFDB multi-segment record" : "WFDB record";
  if (is_file(path / "index.csv")) return "WFDB collection";
  return "WFDB records";
}

namespace {

using FileStat = std::tuple<std::string, std::uint64_t, std::int64_t>;

// size and mtime of a file, following links
std::optional<std::pair<std::uint64_t, std::int64_t>> stat_of(const fs::path& p) {
  std::error_code ec;
  fs::file_status st = fs::status(p, ec);
  if (ec || !fs::exists(st)) return std::nullopt;
  std::uintmax_t size = fs::is_regular_file(st) ? fs::file_size(p, ec) : 0;
  if (ec) size = 0;
  return std::make_pair(static_cast<std::uint64_t>(size), mtime_ns(p));
}

// Every file below dir (names in order, max_depth levels of subfolders, at most
// MAX_FINGERPRINT_FILES files); a link to a file counts, links to folders and broken links do not.
void walk_files(const fs::path& dir, const std::string& rel, std::size_t depth, std::size_t max_depth,
                std::vector<FileStat>& out) {
  std::error_code ec;
  fs::directory_iterator it(dir, ec);
  if (ec) return;
  struct Entry {
    std::string name;
    fs::path full;
    fs::file_status st;
  };
  std::vector<Entry> entries;
  for (; it != fs::directory_iterator(); it.increment(ec)) {
    if (ec) break;
    std::error_code ec2;
    fs::file_status st = it->symlink_status(ec2);
    if (ec2) continue;
    std::string name = core::path_utf8(it->path().filename());
    if (!name.empty() && name[0] == '.') continue;
    entries.push_back({std::move(name), it->path(), st});
  }
  std::sort(entries.begin(), entries.end(), [](const Entry& a, const Entry& b) { return a.name < b.name; });
  for (const auto& e : entries) {
    if (out.size() >= MAX_FINGERPRINT_FILES) return;
    std::string r = rel.empty() ? e.name : rel + "/" + e.name;
    if (fs::is_directory(e.st)) {
      if (depth < max_depth) walk_files(e.full, r, depth + 1, max_depth, out);
    } else if (fs::is_symlink(e.st) && !is_file(e.full)) {
      // a link to a folder, or a broken link
    } else if (auto s = stat_of(e.full)) {
      out.emplace_back(std::move(r), s->first, s->second);
    }
  }
}

}  // namespace

std::vector<std::tuple<std::string, std::uint64_t, std::int64_t>> fingerprint(const fs::path& path) {
  std::vector<FileStat> out;
  if (is_record(path)) {
    fs::path base = path.parent_path();
    for (const auto& f : record_files(base, core::path_utf8(path.stem())))
      if (auto s = stat_of(join(base, f))) out.emplace_back(f, s->first, s->second);
    return out;
  }
  walk_files(path, "", 0, limits().max_depth, out);
  std::sort(out.begin(), out.end());
  return out;
}

}  // namespace vitalstream::engine
