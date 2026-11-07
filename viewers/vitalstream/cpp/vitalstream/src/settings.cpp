#include "settings.hpp"

#include <toml++/toml.hpp>

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <stdexcept>

#include "build_paths.hpp"
#include "engine/adapters.hpp"
#include "core/paths.hpp"

#if defined(_WIN32)
#include <windows.h>
#elif defined(__APPLE__)
// From <mach-o/dyld.h>, declared here: Apple's Mach headers use Clang-only constructs that GCC
// cannot compile with recent SDKs.
extern "C" int _NSGetExecutablePath(char* buf, std::uint32_t* bufsize);
#endif

namespace vitalstream::viewer {

using namespace engine;

#ifdef _WIN32
const char* const SEP = "\\";
#else
const char* const SEP = "/";
#endif

namespace {

std::optional<std::string> env(const char* name) {
#ifdef _WIN32
  // _wgetenv: environment strings in UTF-16
  std::wstring wname(name, name + std::char_traits<char>::length(name));
  const wchar_t* v = _wgetenv(wname.c_str());
  if (!v) return std::nullopt;
  return core::path_utf8(fs::path(v));
#else
  const char* v = std::getenv(name);
  if (!v) return std::nullopt;
  return std::string(v);
#endif
}

std::string trim_end_seps(const std::string& s) {
  std::size_t n = s.size();
  while (n > 0 && is_sep(s[n - 1])) --n;
  return s.substr(0, n);
}

bool within(const std::string& path, const std::string& root) {
  std::string r = trim_end_seps(root);
  return path == root || r.empty() ||
         (path.size() > r.size() && path.compare(0, r.size(), r) == 0 && is_sep(path[r.size()]));
}

std::string realpath(const std::string& p) {
  std::error_code ec;
  fs::path c = fs::canonical(core::utf8_path(p), ec);
  return ec ? p : path_string(c);
}

std::string relative_to(const fs::path& base, const std::string& p) {
  fs::path e = core::utf8_path(expand(p));
  if (e.is_absolute()) return core::path_utf8(e);
  return core::path_utf8(base / e);
}

}  // namespace

std::string path_string(const fs::path& p) {
  std::string s = core::path_utf8(p);
  if (s.rfind("\\\\?\\UNC\\", 0) == 0) return "\\\\" + s.substr(8);
  if (s.rfind("\\\\?\\", 0) == 0) return s.substr(4);
  return s;
}

std::string home() {
#ifdef _WIN32
  const char* first = "USERPROFILE";
  const char* second = "HOME";
#else
  const char* first = "HOME";
  const char* second = "USERPROFILE";
#endif
  if (auto v = env(first)) return *v;
  if (auto v = env(second)) return *v;
  return fs_root();
}

std::string fs_root() {
#ifdef _WIN32
  std::string h = env("USERPROFILE").value_or("");
  fs::path root = core::utf8_path(h).root_path();
  std::string r = core::path_utf8(root);
  return r.empty() ? "C:\\" : r;
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

std::string expand(const std::string& p) {
  if (p == "~") return home();
  if (p.size() >= 2 && p[0] == '~' && is_sep(p[1])) return trim_end_seps(home()) + SEP + p.substr(2);
  return p;
}

std::string absolute(const std::string& p) {
  fs::path x = core::utf8_path(expand(p));
  if (!x.is_absolute()) {
    std::error_code ec;
    x = fs::current_path(ec) / x;
  }
  fs::path out;
  for (const auto& c : x) {
    if (c == "..") {
      if (out.has_relative_path()) out = out.parent_path();
    } else if (c == "." || c.empty()) {
      continue;
    } else {
      out /= c;
    }
  }
  std::string s = core::path_utf8(out);
  return s.empty() ? fs_root() : s;
}

std::optional<fs::path> executable_path() {
#if defined(_WIN32)
  std::wstring buf(32768, L'\0');
  DWORD n = GetModuleFileNameW(nullptr, buf.data(), static_cast<DWORD>(buf.size()));
  if (n == 0 || n >= buf.size()) return std::nullopt;
  buf.resize(n);
  return fs::path(buf);
#elif defined(__APPLE__)
  std::uint32_t size = 0;
  _NSGetExecutablePath(nullptr, &size);
  std::string buf(size, '\0');
  if (_NSGetExecutablePath(buf.data(), &size) != 0) return std::nullopt;
  buf.resize(std::char_traits<char>::length(buf.c_str()));
  std::error_code ec;
  fs::path p = fs::canonical(buf, ec);
  return ec ? fs::path(buf) : p;
#else
  std::error_code ec;
  fs::path p = fs::read_symlink("/proc/self/exe", ec);
  if (ec) return std::nullopt;
  return p;
#endif
}

std::string built_root() {
  // written by CMake with '/' between folders (file(TO_CMAKE_PATH)): this system's separators
  const std::string r = core::path_utf8(core::utf8_path(VS_ROOT_PATH).make_preferred());
  return r.empty() ? fs_root() : absolute(r);
}

std::optional<fs::path> repo_root() {
  std::error_code ec;
  fs::path viewer = fs::canonical(core::utf8_path(VS_VIEWER_SOURCE_DIR), ec);
  if (ec || !viewer.has_parent_path()) return std::nullopt;
  fs::path clone = viewer.parent_path().parent_path();
  auto exe = executable_path();
  if (!exe) return std::nullopt;
  fs::path e = fs::canonical(*exe, ec);
  if (ec) return std::nullopt;
  // the executable lies inside the clone (component by component)
  auto [c, x] = std::mismatch(clone.begin(), clone.end(), e.begin(), e.end());
  bool inside = c == clone.end() && x != e.end();
  bool ok = inside && fs::is_regular_file(viewer / "cpp" / "CMakeLists.txt", ec) && fs::is_directory(viewer / "web", ec) &&
            fs::is_directory(viewer / "sample_data", ec);
  if (!ok) return std::nullopt;
  return viewer;
}

Settings Settings::defaults() {
  Settings s;
  auto r = repo_root();
  s.cache_dir = r ? *r / "sample_data" / "outputs" / "index" : core::utf8_path(expand("~/.cache/vitalstream"));
  s.data_roots = {built_root()};
  s.auth_file = core::utf8_path(expand("~/.config/vitalstream/auth.json"));
  s.adapters_dir = adapters::default_folder();
  return s;
}

void Settings::finish() {
  const std::string top = built_root();
  const std::string real_top = realpath(top);
  // built without a root: the whole file system, every drive on Windows (where built_root() is
  // only the home folder's drive, the folder it shows first)
  const bool whole = core::utf8_path(VS_ROOT_PATH).empty();
  std::vector<std::string> roots;
  for (const auto& given : data_roots) {
    std::string r = absolute(given);
    std::string real = realpath(r);
    std::string kept;
    if (within(real, real_top) || (whole && !within(real_top, real))) {
      kept = r;
    } else if (within(real_top, real)) {
      kept = top;
    } else {
      outside.push_back(r);
      continue;
    }
    if (std::find(roots.begin(), roots.end(), kept) == roots.end()) roots.push_back(kept);
  }
  if (roots.empty())
    throw std::runtime_error("no data root inside this viewer's root, " + top + " (set when it was built)");
  data_roots = roots;
  real_roots_.clear();
  for (const auto& r : data_roots) real_roots_.push_back(realpath(r));
  if (!valid_timezone(timezone)) throw std::runtime_error("unknown time zone \"" + timezone + "\"");
  if (index_workers == 0) index_workers = 1;
}

bool Settings::allowed(const std::string& path) const {
  std::string r = realpath(absolute(path));
  for (const auto& root : real_roots_)
    if (within(r, root)) return true;
  return false;
}

Settings load_settings(const std::optional<std::string>& path) {
  Settings s = Settings::defaults();
  std::vector<fs::path> candidates;
  if (path) {
    candidates.push_back(core::utf8_path(expand(*path)));
  } else {
    const char* name = "vitalstream.toml";
    if (auto r = repo_root()) candidates.push_back(*r / name);
    candidates.push_back(core::utf8_path(expand(std::string("~/.config/vitalstream/") + name)));
  }
  for (const auto& c : candidates) {
    std::error_code ec;
    std::string shown = core::path_utf8(c);
    if (!fs::is_regular_file(c, ec)) {
      if (path) throw std::runtime_error(shown + ": no such settings file");
      continue;
    }
    toml::table table;
    try {
      table = toml::parse_file(shown);
    } catch (const toml::parse_error& e) {
      throw std::runtime_error(shown + ": " + std::string(e.description()));
    }
    // this viewer's section overrides the shared keys; the other viewers' sections are ignored
    toml::table merged;
    for (const auto& [k, v] : table)
      if (k != "py" && k != "rs" && k != "cpp") merged.insert_or_assign(k, v);
    if (const auto* own = table.get("cpp")) {
      const toml::table* t = own->as_table();
      if (!t) throw std::runtime_error(shown + ": [cpp] must be a table");
      for (const auto& [k, v] : *t) merged.insert_or_assign(k, v);
    }
    fs::path base = fs::canonical(c, ec).parent_path();
    std::vector<std::string> unknown;
    for (const auto& [k, v] : merged) {
      std::string key(k.str());
      auto bad = [&]() { return std::runtime_error(shown + ": bad value for " + key); };
      if (key == "timezone") {
        if (!v.is_string()) throw bad();
        s.timezone = *v.value<std::string>();
      } else if (key == "data_roots") {
        const toml::array* a = v.as_array();
        if (!a) throw bad();
        s.data_roots.clear();
        for (const auto& x : *a) {
          if (!x.is_string()) throw bad();
          s.data_roots.push_back(relative_to(base, *x.value<std::string>()));
        }
      } else if (key == "cache_dir") {
        if (!v.is_string()) throw bad();
        s.cache_dir = core::utf8_path(relative_to(base, *v.value<std::string>()));
      } else if (key == "auth_file") {
        if (!v.is_string()) throw bad();
        s.auth_file = core::utf8_path(relative_to(base, *v.value<std::string>()));
      } else if (key == "adapters_dir") {
        if (!v.is_string()) throw bad();
        s.adapters_dir = core::utf8_path(relative_to(base, *v.value<std::string>()));
      } else if (key == "host") {
        if (!v.is_string()) throw bad();
        s.host = *v.value<std::string>();
      } else if (key == "port") {
        auto p = v.value<std::int64_t>();
        if (!v.is_integer() || !p || *p < 0 || *p > 65535) throw bad();
        s.port = static_cast<int>(*p);
      } else if (key == "index_workers") {
        auto p = v.value<std::int64_t>();
        if (!v.is_integer() || !p || *p < 0) throw bad();
        s.index_workers = static_cast<std::size_t>(*p);
      } else if (key == "qc_verbose") {
        if (!v.is_boolean()) throw bad();
        s.qc_verbose = *v.value<bool>();
      } else if (key == "require_password") {
        if (!v.is_boolean()) throw bad();
        s.require_password = *v.value<bool>();
      } else if (key == "secure_cookie") {
        if (!v.is_boolean()) throw bad();
        s.secure_cookie = *v.value<bool>();
      } else if (key == "allowed_hosts") {
        const toml::array* a = v.as_array();
        if (!a) throw bad();
        s.allowed_hosts.clear();
        for (const auto& x : *a) {
          auto h = x.value<std::string>();
          if (!x.is_string() || !h || trim(*h).empty()) throw bad();
          s.allowed_hosts.push_back(*h);
        }
      } else if (key == "max_records" || key == "max_depth") {
        auto p = v.value<std::int64_t>();
        if (!v.is_integer() || !p || *p < 0) throw bad();
        (key == "max_records" ? s.limits.max_records : s.limits.max_depth) = static_cast<std::size_t>(*p);
      } else if (key == "max_scan_s") {
        // a float or an integer, not negative (inf: no time limit)
        std::optional<double> x;
        if (v.is_floating_point()) x = v.value<double>();
        if (v.is_integer()) x = static_cast<double>(*v.value<std::int64_t>());
        if (!x || !(*x >= 0.0)) throw bad();
        s.limits.max_scan_s = *x;
      } else {
        unknown.push_back(key);
      }
    }
    if (!unknown.empty()) {
      std::sort(unknown.begin(), unknown.end());
      std::string list;
      for (std::size_t i = 0; i < unknown.size(); ++i) list += (i ? ", " : "") + unknown[i];
      throw std::runtime_error("unknown settings: " + list);
    }
    fs::path real = fs::canonical(c, ec);
    s.source = ec ? c : real;
    break;
  }
  return s;
}

}  // namespace vitalstream::viewer
