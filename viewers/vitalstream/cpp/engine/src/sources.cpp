// Reading recordings (sources.hpp): WFDB records and EDF files here, other formats through adapters.
#include "engine/sources.hpp"

#include <algorithm>
#include <cstdio>
#include <mutex>
#include <stdexcept>

#include "engine/adapters.hpp"
#include "core/paths.hpp"

namespace vitalstream::engine {

namespace {

class AdapterReader : public Reader {
 public:
  AdapterReader(adapters::Adapter a, fs::path dir) : a_(std::move(a)), dir_(std::move(dir)) {}
  std::vector<double> read(std::size_t piece, std::int64_t a, std::int64_t b) const override {
    return adapters::read(a_, dir_, piece, a, b);
  }

 private:
  adapters::Adapter a_;
  fs::path dir_;
};

class NoReader : public Reader {
 public:
  explicit NoReader(std::string why) : why_(std::move(why)) {}
  std::vector<double> read(std::size_t, std::int64_t, std::int64_t) const override {
    throw std::runtime_error(why_);
  }

 private:
  std::string why_;
};

std::string read_text(const fs::path& p) {
  std::string out;
  std::FILE* f = core::open_file(core::path_utf8(p), "rb");
  if (!f) throw std::runtime_error(core::path_utf8(p) + ": cannot open");
  char buf[65536];
  std::size_t n;
  while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) out.append(buf, n);
  std::fclose(f);
  return out;
}

}  // namespace

namespace {

std::mutex& allowed_lock() {
  static std::mutex m;
  return m;
}

std::optional<std::vector<std::string>>& allowed_roots() {
  static std::optional<std::vector<std::string>> roots;
  return roots;
}

// The real path (symbolic links resolved, Windows' \\?\ prefix removed), or nothing when it
// does not exist.
std::optional<std::string> real(const fs::path& p) {
  std::error_code ec;
  fs::path c = fs::canonical(p, ec);
  if (ec) return std::nullopt;
  std::string s = core::path_utf8(c);
  if (s.rfind("\\\\?\\UNC\\", 0) == 0) return "\\\\" + s.substr(8);
  if (s.rfind("\\\\?\\", 0) == 0) return s.substr(4);
  return s;
}

bool sep(char c) {
#ifdef _WIN32
  return c == '/' || c == '\\';
#else
  return c == '/';
#endif
}

bool within(const std::string& path, const std::string& root) {
  std::size_t n = root.size();
  while (n > 0 && sep(root[n - 1])) --n;
  return path == root || n == 0 || (path.size() > n && path.compare(0, n, root, 0, n) == 0 && sep(path[n]));
}

}  // namespace

void set_allowed_roots(const std::optional<std::vector<std::string>>& roots) {
  std::optional<std::vector<std::string>> real_roots;
  if (roots) {
    real_roots.emplace();
    for (const auto& r : *roots) {
      std::error_code ec;
      fs::path a = fs::absolute(core::utf8_path(r), ec).lexically_normal();
      real_roots->push_back(real(a).value_or(core::path_utf8(a)));
    }
  }
  std::lock_guard<std::mutex> l(allowed_lock());
  allowed_roots() = std::move(real_roots);
}

bool allowed_file(const fs::path& p) {
  std::optional<std::vector<std::string>> roots;
  {
    std::lock_guard<std::mutex> l(allowed_lock());
    roots = allowed_roots();
  }
  if (!roots) return true;
  if (auto r = real(p))
    return std::any_of(roots->begin(), roots->end(), [&](const std::string& root) { return within(*r, root); });
  std::error_code ec;
  return !fs::exists(fs::symlink_status(p, ec));  // nothing there (a broken link is something)
}

const char* const NO_WFDB = "this build has no WFDB support (built with VS_WFDB=OFF)";

std::optional<std::string> unreadable(const fs::path& p) {
  std::error_code ec;
  if (!wfdb_supported() && (fs::is_directory(p, ec) || is_record(p))) return std::string(NO_WFDB);
  return std::nullopt;
}

std::optional<std::string> kind_of(const fs::path& p) {
  std::error_code ec;
  if (fs::is_regular_file(p, ec)) {
    if (is_record(p)) return std::string("wfdb");
    if (is_edf(p)) return std::string("edf");
    if (auto found = adapters::for_file(p)) return found->second.kind;
    return std::nullopt;
  }
  if (is_wfdb_folder(p, 2000)) return std::string("wfdb");
  return std::nullopt;
}

std::string source_format_label(const fs::path& p) {
  std::error_code ec;
  if (fs::is_directory(p, ec) || is_record(p)) return format_label(p);
  if (is_edf(p)) return edf_format_label(p);
  auto found = adapters::for_file(p);
  return found ? found->second.label : std::string();
}

Building build_source(const fs::path& path, const fs::path& dir, const Progress& progress,
                      const Cancelled& cancelled) {
  std::error_code ec;
  if (fs::is_directory(path, ec) || is_record(path)) {
    Building b{build_wfdb_folder(path, progress, cancelled), false};
    b.built.first.insert("kind", Value("wfdb"));
    b.built.first.insert("format", Value(format_label(path)));
    return b;
  }
  if (is_edf(path)) {
    Building b{build_edf(path, progress, cancelled), false};
    b.built.first.insert("kind", Value("edf"));
    b.built.first.insert("format", Value(edf_format_label(path)));
    return b;
  }
  auto found = adapters::for_file(path);
  if (!found) throw std::runtime_error("not a recording this program reads: " + core::path_utf8(path));
  const auto& [adapter, format] = *found;
  adapters::index(adapter, path, dir, progress, cancelled);
  Value v = Value::from_json(read_text(dir / "meta.json"));
  Dict* meta = v.as_dict();
  if (!meta) throw std::runtime_error(adapter.name + " wrote no valid meta.json");
  meta->insert("kind", Value(format.kind));
  meta->insert("format", Value(format.label));
  meta->insert("adapter", Value(adapter.name));
  return Building{{std::move(*meta), Arrays{}}, true};
}

std::unique_ptr<Reader> make_reader(const Dict& meta, const fs::path& dir) {
  const Value* pv = meta.get("path");
  fs::path path = core::utf8_path(pv && pv->as_str() ? *pv->as_str() : std::string());
  const Value* kind = meta.get("kind");
  if (kind && kind->as_str() && *kind->as_str() == "wfdb") return make_wfdb_reader(path, meta);
  if (kind && kind->as_str() && *kind->as_str() == "edf") return make_edf_reader(path, meta);
  const Value* name = meta.get("adapter");
  if (name && name->as_str()) {
    if (auto a = adapters::named(*name->as_str())) return std::make_unique<AdapterReader>(std::move(*a), dir);
    return std::make_unique<NoReader>("the " + *name->as_str() + " adapter is not installed: its samples cannot be read");
  }
  return std::make_unique<NoReader>("no reader for this recording");
}

}  // namespace vitalstream::engine
