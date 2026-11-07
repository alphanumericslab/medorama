// The index cache (index.hpp).
#include "engine/index.hpp"

#include <atomic>
#include <cctype>
#include <chrono>
#include <cmath>
#include <map>
#include <mutex>
#include <cstdio>
#include <cstring>
#include <stdexcept>

#include <sys/stat.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

#include "engine/sources.hpp"
#include "core/paths.hpp"
#include "core/pyfmt.hpp"
#include "core/sha256.hpp"

#ifndef VS_ENGINE_VERSION
#define VS_ENGINE_VERSION "0.0.0"
#endif

namespace vitalstream::engine {

using core::path_utf8;

std::int64_t mtime_ns(const fs::path& p) {
#ifdef _WIN32
  WIN32_FILE_ATTRIBUTE_DATA a;
  if (!GetFileAttributesExW(p.c_str(), GetFileExInfoStandard, &a)) return 0;
  ULARGE_INTEGER u;
  u.LowPart = a.ftLastWriteTime.dwLowDateTime;
  u.HighPart = a.ftLastWriteTime.dwHighDateTime;
  // 100 ns intervals since 1601-01-01
  return (static_cast<std::int64_t>(u.QuadPart) - 116444736000000000LL) * 100;
#else
  struct stat st;
  if (::stat(p.c_str(), &st) != 0) return 0;
#if defined(__APPLE__)
  return static_cast<std::int64_t>(st.st_mtimespec.tv_sec) * 1000000000LL + st.st_mtimespec.tv_nsec;
#else
  return static_cast<std::int64_t>(st.st_mtim.tv_sec) * 1000000000LL + st.st_mtim.tv_nsec;
#endif
#endif
}

std::string cache_key(const fs::path& path) {
  std::error_code ec;
  fs::path p = fs::canonical(path, ec);
  if (ec) p = path;
  Value fp;
  if (fs::is_directory(p, ec) || is_record(p)) {
    List l;
    for (const auto& [name, size, mt] : fingerprint(p))
      l.push_back(Value(List{Value(name), Value(size), Value(mt)}));
    fp = Value(std::move(l));
  } else {
    std::uintmax_t size = fs::file_size(p, ec);
    if (ec) size = 0;
    fp = Value(List{Value(static_cast<unsigned long long>(size)), Value(mtime_ns(p))});
  }
  std::string text = Value(List{Value(INDEX_VERSION), Value(path_utf8(p)), fp}).to_json();
  core::Sha256 h;
  h.update(text.data(), text.size());
  return h.hex_digest().substr(0, 20);
}

namespace {

template <class T>
void write_array(const fs::path& dir, const std::string& name, const std::vector<T>& v) {
  std::string path = path_utf8(dir / name);
  std::FILE* f = core::open_file(path, "wb");
  if (!f) throw std::runtime_error(path + ": cannot create file");
  // the platforms supported are little-endian: the bytes are written as they are in memory
  bool ok = v.empty() || std::fwrite(v.data(), sizeof(T), v.size(), f) == v.size();
  ok = std::fclose(f) == 0 && ok;
  if (!ok) throw std::runtime_error(path + ": write failed");
}

void write_text(const fs::path& p, const std::string& text) {
  std::string path = path_utf8(p);
  std::FILE* f = core::open_file(path, "wb");
  if (!f) throw std::runtime_error(path + ": cannot create file");
  bool ok = std::fwrite(text.data(), 1, text.size(), f) == text.size();
  ok = std::fclose(f) == 0 && ok;
  if (!ok) throw std::runtime_error(path + ": write failed");
}

long process_id() {
#ifdef _WIN32
  return static_cast<long>(GetCurrentProcessId());
#else
  return static_cast<long>(getpid());
#endif
}

}  // namespace

void write_arrays(const fs::path& dir, const Arrays& a) {
  for (std::size_t lvl = 0; lvl < a.env_min.size(); ++lvl) {
    write_array(dir, "env_min_" + std::to_string(lvl) + ".f32", a.env_min[lvl]);
    write_array(dir, "env_max_" + std::to_string(lvl) + ".f32", a.env_max[lvl]);
  }
  write_array(dir, "mk_piece.i32", a.mk_piece);
  write_array(dir, "mk_sample.i64", a.mk_sample);
  write_array(dir, "mk_code.i16", a.mk_code);
  write_array(dir, "vit_t.f64", a.vit_t);
  write_array(dir, "vit_v.f64", a.vit_v);
}

std::string build_index(const fs::path& path, const fs::path& cache_dir, const Progress& progress) {
  return build_index_until(path, cache_dir, progress, [] { return false; });
}

namespace {

// The writers of each key in this process, one at a time.
std::shared_ptr<std::mutex> key_lock(const std::string& key) {
  static std::mutex m;
  static std::map<std::string, std::weak_ptr<std::mutex>> locks;
  std::lock_guard<std::mutex> l(m);
  for (auto it = locks.begin(); it != locks.end();) it = it->second.expired() ? locks.erase(it) : std::next(it);
  auto& w = locks[key];
  auto lock = w.lock();
  if (!lock) {
    lock = std::make_shared<std::mutex>();
    w = lock;
  }
  return lock;
}

// A folder of cache_dir named uniquely for this writer: <key>.<process id>.<n>.<suffix>.
fs::path unique(const fs::path& cache_dir, const std::string& key, const char* suffix) {
  static std::atomic<std::uint64_t> n{0};
  return cache_dir / (key + "." + std::to_string(process_id()) + "." + std::to_string(n++) + "." + suffix);
}

// Removes a folder when it goes out of scope (an index being built, on every way out).
struct Removed {
  fs::path path;
  ~Removed() {
    std::error_code ec;
    fs::remove_all(path, ec);
  }
};

}  // namespace

std::string build_index_until(const fs::path& path, const fs::path& cache_dir, const Progress& progress,
                              const Cancelled& cancelled) {
  fs::create_directories(cache_dir);
  std::string key = cache_key(path);
  auto lock = key_lock(key);
  std::lock_guard<std::mutex> writing(*lock);
  auto t_start = std::chrono::steady_clock::now();
  progress(0.0);
  fs::path folder = cache_dir / key;
  Removed tmp{unique(cache_dir, key, "tmp")};
  std::error_code ec;
  fs::create_directories(tmp.path);
  Building b = build_source(path, tmp.path, progress, cancelled);
  auto& [meta, a] = b.built;
  if (!b.by_adapter) write_arrays(tmp.path, a);  // otherwise the adapter wrote the arrays into tmp
  fs::path real = fs::canonical(path, ec);
  if (ec) real = path;
  meta.insert("path", Value(path_utf8(real)));
  meta.insert("name", Value(path_utf8(path.filename())));
  meta.insert("key", Value(key));
  meta.insert("index_version", Value(INDEX_VERSION));
  meta.insert("engine", Value(VS_ENGINE_VERSION));
  double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - t_start).count();
  meta.insert("indexed_s", Value(core::round_to(secs, 1)));
  write_text(tmp.path / "meta.json", clean(Value(std::move(meta))).to_json());
  // what a reader would refuse is not kept (times or rates that are not finite numbers, ...)
  std::string why;
  if (!load_from(tmp.path, why))
    throw std::runtime_error(path_utf8(path) + ": the index would be inconsistent: " + why);
  // an index already there, complete and written by this engine, is kept (the same content);
  // any other is moved aside first, so that readers never see a folder half replaced
  bool current = false;
  if (auto there = load_from(folder, why)) {
    const Value* e = there->first.get("engine");
    current = e && e->as_str() && *e->as_str() == VS_ENGINE_VERSION;
  }
  if (!current) {
    Removed old{unique(cache_dir, key, "old")};
    if (fs::exists(folder, ec)) fs::rename(folder, old.path);
    fs::rename(tmp.path, folder, ec);
    // another program moved its index of this key in first
    if (ec && !load_from(folder, why)) throw fs::filesystem_error("cannot rename", tmp.path, folder, ec);
  }
  progress(1.0);
  return key;
}

template <class T>
Mapped<T>::Mapped(const fs::path& p) {
  // cache files are written once (atomically renamed into place) and never modified
  file_ = std::make_shared<const core::FileData>(path_utf8(p));
  if (file_->size() % sizeof(T) != 0) throw std::runtime_error(path_utf8(p) + ": not a whole number of values");
}

template class Mapped<float>;
template class Mapped<std::int32_t>;
template class Mapped<std::int64_t>;
template class Mapped<std::int16_t>;
template class Mapped<double>;

namespace {

// A list of meta.json (missing: empty); nullptr for another type.
const List* list_of(const Dict& meta, const char* k) {
  static const List empty;
  const Value* v = meta.get(k);
  if (!v) return &empty;
  return v->as_list();
}

// An integer (not a bool, not beyond 64 bits), else nothing.
std::optional<std::int64_t> whole(const Value* v) {
  if (!v) return std::nullopt;
  if (const auto* u = std::get_if<std::uint64_t>(&v->storage()))
    if (*u > static_cast<std::uint64_t>(INT64_MAX)) return std::nullopt;
  return int_of(v);
}

// An integer or a finite float, else nothing.
std::optional<double> finite(const Value* v) {
  if (!v || !(v->is_int() || v->is_float())) return std::nullopt;
  auto x = f64_of(v);
  if (!x || !std::isfinite(*x)) return std::nullopt;
  return x;
}

}  // namespace

std::string check(const Dict& meta, const Loaded& a) {
  const List* channels = list_of(meta, "channels");
  const List* pieces = list_of(meta, "pieces");
  const List* vitals = list_of(meta, "vitals");
  const List* names = list_of(meta, "marker_names");
  if (!channels) return "channels is not a list";
  if (!pieces) return "pieces is not a list";
  for (std::size_t i = 0; i < pieces->size(); ++i) {
    const Value& p = (*pieces)[i];
    std::string bad = "piece " + std::to_string(i) + ": ";
    auto id = whole(p.get("id"));
    if (!id) return bad + "a id that is not a whole number";
    if (*id != static_cast<std::int64_t>(i)) return bad + "its id is not its position";
    auto ch = whole(p.get("ch"));
    if (!ch) return bad + "a ch that is not a whole number";
    if (*ch < 0 || static_cast<std::uint64_t>(*ch) >= channels->size())
      return bad + "channel " + std::to_string(*ch) + " is not among the " + std::to_string(channels->size()) +
             " channels";
    auto fs_ = finite(p.get("fs"));
    if (!fs_) return bad + "a fs that is not a finite number";
    if (!(*fs_ > 0.0)) return bad + "a sampling rate of " + core::repr(*fs_);
    if (!finite(p.get("t0"))) return bad + "a t0 that is not a finite number";
    auto n = whole(p.get("n"));
    if (!n) return bad + "a n that is not a whole number";
    if (*n < 0) return bad + "a negative number of samples";
    static const List none;
    const List* env = &none;
    if (const Value* e = p.get("env")) {
      env = e->as_list();
      if (!env) return bad + "env is not a list";
    }
    if (env->size() > a.env_min.size())
      return bad + std::to_string(env->size()) + " envelope levels; the index has " + std::to_string(a.env_min.size());
    for (std::size_t lvl = 0; lvl < env->size(); ++lvl) {
      const List* pair = (*env)[lvl].as_list();
      std::optional<std::int64_t> off, len;
      if (pair && pair->size() == 2) {
        off = whole(&(*pair)[0]);
        len = whole(&(*pair)[1]);
      }
      if (!off || !len) return bad + "envelope level " + std::to_string(lvl) + " is not [offset, length]";
      auto size = static_cast<std::int64_t>(std::min(a.env_min[lvl].size(), a.env_max[lvl].size()));
      if (*off < 0 || *len < 0 || *off > size || *len > size - *off)
        return bad + "envelope level " + std::to_string(lvl) + " [" + std::to_string(*off) + ", " +
               std::to_string(*len) + "] lies outside its " + std::to_string(size) + " values";
    }
  }
  std::size_t nm = a.mk_piece.size();
  if (a.mk_sample.size() != nm || a.mk_code.size() != nm) return "the marker arrays differ in length";
  for (std::size_t k = 0; k < nm; ++k) {
    std::int32_t p = a.mk_piece.data()[k];
    if (p < 0 || static_cast<std::size_t>(p) >= pieces->size() || (k > 0 && a.mk_piece.data()[k - 1] > p))
      return "mk_piece names a piece that does not exist, or is not in order";
  }
  if (!names) return "marker_names is not a list";
  for (std::size_t k = 0; k < nm; ++k) {
    std::int16_t c = a.mk_code.data()[k];
    if (c < 0 || static_cast<std::size_t>(c) >= names->size()) return "mk_code names a marker name that does not exist";
  }
  auto vt = static_cast<std::int64_t>(a.vit_t.size());
  if (a.vit_v.size() != a.vit_t.size()) return "vit_t and vit_v differ in length";
  if (!vitals) return "vitals is not a list";
  for (const Value& v : *vitals) {
    auto off = whole(v.get("offset"));
    if (!off) return "a offset that is not a whole number";
    auto n = whole(v.get("n"));
    if (!n) return "a n that is not a whole number";
    if (*off < 0 || *n < 0 || *off > vt || *n > vt - *off)
      return "a vital-sign series [" + std::to_string(*off) + ", " + std::to_string(*n) + "] lies outside the " +
             std::to_string(vt) + " values";
  }
  return "";
}

std::optional<std::pair<Dict, Loaded>> load_from(const fs::path& dir, std::string& why) {
  try {
    std::FILE* f = core::open_file(path_utf8(dir / "meta.json"), "rb");
    if (!f) {
      why = "meta.json: cannot open";
      return std::nullopt;
    }
    std::string text;
    char buf[65536];
    std::size_t n;
    while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) text.append(buf, n);
    std::fclose(f);
    Value meta;
    try {
      meta = Value::from_json(text);
    } catch (const std::exception&) {
    }
    Dict* d = meta.as_dict();
    if (!d) {
      why = "meta.json is not a JSON object";
      return std::nullopt;
    }
    if (whole(d->get("index_version")) != std::optional<std::int64_t>(INDEX_VERSION)) {
      why = "another index version";
      return std::nullopt;
    }
    Loaded l;
    l.dir = dir;
    std::error_code ec;
    for (int lvl = 0;; ++lvl) {
      fs::path a = dir / ("env_min_" + std::to_string(lvl) + ".f32");
      fs::path b = dir / ("env_max_" + std::to_string(lvl) + ".f32");
      if (!fs::exists(a, ec)) break;
      l.env_min.emplace_back(a);
      l.env_max.emplace_back(b);
    }
    l.mk_piece = Mapped<std::int32_t>(dir / "mk_piece.i32");
    l.mk_sample = Mapped<std::int64_t>(dir / "mk_sample.i64");
    l.mk_code = Mapped<std::int16_t>(dir / "mk_code.i16");
    l.vit_t = Mapped<double>(dir / "vit_t.f64");
    l.vit_v = Mapped<double>(dir / "vit_v.f64");
    why = check(*d, l);
    if (!why.empty()) return std::nullopt;
    return std::make_pair(std::move(*d), std::move(l));
  } catch (const std::exception& e) {
    why = e.what();
    return std::nullopt;
  }
}

std::optional<std::pair<Dict, Loaded>> load_index(const fs::path& cache_dir, const std::string& key) {
  if (key.empty()) return std::nullopt;
  for (char c : key)
    if (!std::isxdigit(static_cast<unsigned char>(c))) return std::nullopt;
  std::string why;
  return load_from(cache_dir / key, why);
}

}  // namespace vitalstream::engine
