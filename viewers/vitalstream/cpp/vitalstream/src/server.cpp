// The HTTP API and the web page (server.hpp), with cpp-httplib.
#include "server.hpp"

#include <httplib.h>

#include <algorithm>
#include <atomic>
#include <cctype>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <map>
#include <mutex>
#include <deque>
#include <regex>
#include <set>
#include <thread>

#include "engine/adapters.hpp"
#include "auth.hpp"
#include "engine/dataset.hpp"
#include "engine/index.hpp"
#include "engine/sources.hpp"
#include "core/paths.hpp"
#include "core/text.hpp"
#include "core/pyfmt.hpp"

#ifndef VS_VIEWER_VERSION
#define VS_VIEWER_VERSION "0.0.0"
#endif

namespace vitalstream::web {
const std::map<std::string, std::string_view>& files();
}

namespace vitalstream::viewer {

using namespace engine;

namespace {

constexpr std::size_t MAX_LOADED = 4;
constexpr std::size_t MAX_ENTRIES = 5000;
constexpr std::size_t MAX_COMPLETIONS = 50;
// The most channels of one window or markers request, and series of one vitals request.
constexpr std::size_t MAX_CHANNELS = 64;
constexpr std::size_t MAX_SERIES = 512;
// The largest request bodies: login (a password), and the API's (open, cancel).
constexpr std::size_t MAX_LOGIN_BODY = 4 * 1024;
constexpr std::size_t MAX_API_BODY = 64 * 1024;
// Wrong passwords: the delay before the next password from the same address is checked doubles
// with each (1, 3, 7, 15 s, at most 30 s), for FAILURES_KEPT.
constexpr std::uint64_t MAX_LOGIN_DELAY_S = 30;
constexpr auto FAILURES_KEPT = std::chrono::minutes(15);
// The Content-Security-Policy of the page and the sign-in page.
const char* const CSP =
    "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; object-src 'none'; "
    "base-uri 'none'; frame-ancestors 'self'; form-action 'self'";

const char* LOGIN_PAGE = R"(<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>VitalStream</title>
<link rel="stylesheet" href="static/login.css"></head><body>
<form method="post" action="login"><h1>VitalStream</h1>{error}
<input type="password" name="password" placeholder="Password" autofocus autocomplete="current-password">
<button>Sign in</button></form></body></html>)";

struct ApiError {
  int code;
  std::string msg;
};

std::string login_page(const std::string& error) {
  std::string s = LOGIN_PAGE;
  s.replace(s.find("{error}"), 7, error);
  return s;
}

struct Job {
  enum class State { Indexing, Error, Cancelled, Done } state = State::Indexing;
  std::shared_ptr<std::atomic<double>> fraction;
  std::shared_ptr<std::atomic<bool>> cancel;
  std::chrono::steady_clock::time_point started;
  std::string error;
};

// The name of a Host header or an origin's host, lower case, without the port and the brackets
// of an IPv6 address ("[::1]:8060" -> "::1").
std::string host_name(const std::string& host) {
  std::string h;
  for (char c : trim(host)) h += static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  if (!h.empty() && h[0] == '[') {
    std::size_t e = h.find(']');
    return e == std::string::npos ? std::string() : h.substr(1, e - 1);
  }
  // a bare IPv6 address (the configured host): every colon is part of it
  if (std::count(h.begin(), h.end(), ':') > 1) return h;
  return h.substr(0, h.find(':'));
}

}  // namespace

class App {
 public:
  App(const Settings& s, std::optional<bool> protected_)
      : settings(s), creds(load_auth(s.auth_file)), revoked_(revoked(s.auth_file)) {
    prot = protected_.value_or(creds.has_value());
    if (prot && !creds) throw std::runtime_error("password protection requested but no password is set");
    cookie = "vitalstream_" + std::to_string(s.port);
    // the index builders: index_workers threads, served in turn from a queue
    for (std::size_t i = 0; i < std::max<std::size_t>(s.index_workers, 1); ++i) builders_.emplace_back([this] { build_loop(); });
  }

  ~App() {
    {
      std::lock_guard<std::mutex> l(m);
      stopping_ = true;
      for (auto& [key, job] : jobs)
        if (job.cancel) job.cancel->store(true);
    }
    queued_.notify_all();
    for (auto& t : builders_) t.join();
  }

  // The value of this server's sign-in cookie in a request, if any.
  std::optional<std::string> cookie_of(const httplib::Request& req) const {
    auto range = req.headers.equal_range("Cookie");
    for (auto it = range.first; it != range.second; ++it) {
      const std::string& h = it->second;
      std::size_t a = 0;
      while (a <= h.size()) {
        std::size_t b = h.find(';', a);
        if (b == std::string::npos) b = h.size();
        std::string kv = trim(h.substr(a, b - a));
        std::size_t eq = kv.find('=');
        if (eq != std::string::npos && kv.substr(0, eq) == cookie) return kv.substr(eq + 1);
        a = b + 1;
      }
    }
    return std::nullopt;
  }

  bool logged_in(const httplib::Request& req) {
    if (!prot) return true;
    auto value = cookie_of(req);
    if (value) {
      std::size_t dot = value->find('.');
      std::lock_guard<std::mutex> l(m);
      if (dot != std::string::npos && revoked_.count(value->substr(dot + 1))) return false;
    }
    return creds && cookie_ok(*creds, value);
  }

  // Whether a request names a host this server answers for (DNS rebinding: docs/viewer-api.md,
  // "Requests"): localhost, 127.0.0.1, ::1, the host it listens on, or one of allowed_hosts (*:
  // any), with any port.
  bool host_allowed(const std::string& host) const {
    std::string name = host_name(host);
    if (name.empty()) return false;
    std::string configured = host_name(settings.host);
    if (name == "localhost" || name == "127.0.0.1" || name == "::1") return true;
    if (name == configured && configured != "0.0.0.0" && configured != "::" && !configured.empty()) return true;
    return std::any_of(settings.allowed_hosts.begin(), settings.allowed_hosts.end(),
                       [&](const std::string& h) { return h == "*" || host_name(h) == name; });
  }

  // Absolute path as given (links not resolved), if allowed and existing.
  std::string check_path(const std::string& path) const {
    std::string p = absolute(path);
    if (!settings.allowed(p)) throw ApiError{403, "path outside the configured data roots"};
    std::error_code ec;
    if (!fs::exists(core::utf8_path(p), ec)) throw ApiError{404, "no such file or folder: " + p};
    return p;
  }

  // 403 unless every file a WFDB record reads (its header, the signal files it names, its
  // segments, its .mrk file) leads inside the data roots, links resolved.
  void check_record(const fs::path& hea) const {
    fs::path base = hea.parent_path();
    for (const auto& f : files_read(base, core::path_utf8(hea.stem()))) {
      std::error_code ec;
      if (fs::exists(fs::symlink_status(f, ec)) && !settings.allowed(core::path_utf8(f)))
        throw ApiError{403, core::path_utf8(f.filename()) + ", a file of this record, lies outside the data roots"};
    }
  }

  std::shared_ptr<Dataset> dataset(const std::string& key) {
    {
      std::lock_guard<std::mutex> l(m);
      if (auto* d = loaded.get(key)) return *d;
    }
    auto li = load_index(settings.cache_dir, key);
    if (!li) return nullptr;
    auto ds = std::make_shared<Dataset>(std::move(li->first), std::move(li->second));
    const Value* pv = ds->meta.get("path");
    std::string path = pv && pv->as_str() ? *pv->as_str() : std::string();
    std::lock_guard<std::mutex> l(m);
    if (!opened.count(key) && !settings.allowed(path)) throw ApiError{403, "path outside the configured data roots"};
    loaded.insert(key, ds);
    while (loaded.size() > MAX_LOADED) {
      std::string oldest = loaded.begin()->first;
      loaded.shift_remove(oldest);
    }
    return ds;
  }

  std::shared_ptr<Dataset> need(const std::string& key) {
    auto ds = dataset(key);
    if (!ds) throw ApiError{404, "dataset not indexed"};
    return ds;
  }

  std::string start_folder() const {
    std::string h = home();
    std::error_code ec;
    if (settings.allowed(h) && fs::is_directory(core::utf8_path(h), ec)) return h;
    return settings.data_roots.at(0);
  }

  // Build the index of `path` under `key` in the background (the caller holds m and has made
  // the job).
  void enqueue(const std::string& key, const std::string& path) {
    queue_.emplace_back(key, path);
    queued_.notify_one();
  }

  // The delay before a password from `client` is checked: none, then 1, 3, 7, 15 s ... (at most
  // MAX_LOGIN_DELAY_S) after wrong passwords in the last FAILURES_KEPT; how long it still lasts.
  std::chrono::seconds login_wait(const std::string& client) {
    std::lock_guard<std::mutex> l(m);
    auto now = std::chrono::steady_clock::now();
    for (auto it = failures_.begin(); it != failures_.end();)
      it = now - it->second.second >= FAILURES_KEPT ? failures_.erase(it) : std::next(it);
    auto it = failures_.find(client);
    if (it == failures_.end()) return std::chrono::seconds(0);
    std::uint64_t n = std::min<std::uint64_t>(it->second.first, 6);
    auto delay = std::chrono::seconds(std::min<std::uint64_t>((std::uint64_t{1} << n) - 1, MAX_LOGIN_DELAY_S));
    auto left = std::chrono::ceil<std::chrono::seconds>(it->second.second + delay - now);
    return std::max(left, std::chrono::seconds(0));
  }

  void login_failed(const std::string& client) {
    std::lock_guard<std::mutex> l(m);
    if (failures_.size() >= 10000 && !failures_.count(client)) failures_.clear();  // many addresses: start again
    auto& e = failures_[client];
    e = {e.first + 1, std::chrono::steady_clock::now()};
  }

  void login_succeeded(const std::string& client) {
    std::lock_guard<std::mutex> l(m);
    failures_.erase(client);
  }

  // End the sign-in of a cookie on the server (kept in the auth file).
  void revoke(const std::string& signature, std::uint64_t expires) {
    std::map<std::string, std::uint64_t> all;
    {
      std::lock_guard<std::mutex> l(m);
      revoked_[signature] = expires;
      all = revoked_;
    }
    try {
      save_revoked(settings.auth_file, all);
    } catch (const std::exception&) {
    }
  }

  std::string cookie_attributes() const {
    return settings.secure_cookie ? "HttpOnly; Path=/; SameSite=lax; Secure" : "HttpOnly; Path=/; SameSite=lax";
  }

  Settings settings;
  bool prot = false;
  std::optional<Auth> creds;
  std::string cookie;
  std::mutex m;
  std::map<std::string, Job> jobs;
  OrderedMap<std::string, std::shared_ptr<Dataset>> loaded;
  std::set<std::string> opened;
  std::mutex checking;  // one password checked at a time (scrypt takes 16 MiB)

 private:
  void build_loop() {
    for (;;) {
      std::string key, path;
      std::shared_ptr<std::atomic<double>> fraction;
      std::shared_ptr<std::atomic<bool>> cancel;
      {
        std::unique_lock<std::mutex> l(m);
        queued_.wait(l, [&] { return stopping_ || !queue_.empty(); });
        if (stopping_) return;
        std::tie(key, path) = queue_.front();
        queue_.pop_front();
        Job& j = jobs[key];
        fraction = j.fraction;
        cancel = j.cancel;
      }
      std::string err;
      bool ok = true;
      try {
        build_index_until(
            core::utf8_path(path), settings.cache_dir, [&](double x) { fraction->store(std::clamp(x, 0.0, 1.0)); },
            [&] { return cancel->load(); });
      } catch (const std::exception& e) {
        ok = false;
        err = e.what();
      } catch (...) {
        // the index folder is removed on the way out
        ok = false;
        err = "indexing stopped on an internal error";
      }
      std::lock_guard<std::mutex> l(m);
      Job& j = jobs[key];
      j.state = ok ? Job::State::Done : cancel->load() ? Job::State::Cancelled : Job::State::Error;
      j.error = err;
    }
  }

  std::map<std::string, std::uint64_t> revoked_;  // sign-ins ended by logout: signature -> expiry
  std::map<std::string, std::pair<std::uint64_t, std::chrono::steady_clock::time_point>> failures_;
  std::deque<std::pair<std::string, std::string>> queue_;  // (key, path) waiting for a builder
  std::condition_variable queued_;
  bool stopping_ = false;
  std::vector<std::thread> builders_;
};

std::shared_ptr<App> make_app(const Settings& settings, std::optional<bool> protected_) {
  return std::make_shared<App>(settings, protected_);
}

bool app_protected(const App& app) { return app.prot; }

namespace {

void json(httplib::Response& res, int code, const Value& v) {
  res.status = code;
  res.set_content(clean(v).to_json(), "application/json");
}

void error(httplib::Response& res, int code, const std::string& msg) { json(res, code, Value(Dict{{"detail", Value(msg)}})); }

void html(httplib::Response& res, int code, const std::string& body) {
  res.status = code;
  res.set_header("Content-Security-Policy", CSP);
  res.set_content(body, "text/html; charset=utf-8");
}

// Run a handler, turning ApiError into JSON error responses; any other failure is an internal
// error (500), whose text is not shown.
template <class F>
httplib::Server::Handler api(F f) {
  return [f](const httplib::Request& req, httplib::Response& res) {
    try {
      f(req, res);
    } catch (const ApiError& e) {
      error(res, e.code, e.msg);
    } catch (...) {
      error(res, 500, "internal error");
    }
  };
}

std::string url_decode(const std::string& s);

// The query parameters (application/x-www-form-urlencoded) of a request; a parameter given twice
// is a bad request.
class Query {
 public:
  explicit Query(const httplib::Request& req) {
    std::size_t q = req.target.find('?');
    std::string raw = q == std::string::npos ? std::string() : req.target.substr(q + 1);
    std::size_t a = 0;
    while (a <= raw.size()) {
      std::size_t b = raw.find('&', a);
      if (b == std::string::npos) b = raw.size();
      std::string kv = raw.substr(a, b - a);
      a = b + 1;
      if (kv.empty()) continue;
      std::size_t eq = kv.find('=');
      std::string k = url_decode(eq == std::string::npos ? kv : kv.substr(0, eq));
      std::string v = url_decode(eq == std::string::npos ? std::string() : kv.substr(eq + 1));
      if (!values_.emplace(k, v).second) throw ApiError{400, "query parameter " + k + " is given more than once"};
    }
  }
  std::optional<std::string> get(const std::string& k) const {
    auto it = values_.find(k);
    if (it == values_.end()) return std::nullopt;
    return it->second;
  }

 private:
  std::map<std::string, std::string> values_;
};

bool all_digits(std::string_view s) {
  return !s.empty() && std::all_of(s.begin(), s.end(), [](char c) { return c >= '0' && c <= '9'; });
}

// A decimal number as the API takes it: [+-]digits[.digits][e[+-]digits] (or .digits), nothing
// around it.
bool decimal(std::string_view v) {
  if (!v.empty() && (v[0] == '+' || v[0] == '-')) v.remove_prefix(1);
  std::size_t e = v.find_first_of("eE");
  std::string_view mant = v.substr(0, e);
  bool mant_ok;
  std::size_t dot = mant.find('.');
  if (dot != std::string_view::npos) {
    std::string_view i = mant.substr(0, dot), f = mant.substr(dot + 1);
    mant_ok = (all_digits(i) || i.empty()) && (all_digits(f) || f.empty()) && !(i.empty() && f.empty());
  } else {
    mant_ok = all_digits(mant);
  }
  if (e == std::string_view::npos) return mant_ok;
  std::string_view x = v.substr(e + 1);
  if (!x.empty() && (x[0] == '+' || x[0] == '-')) x.remove_prefix(1);
  return mant_ok && all_digits(x);
}

// A required time (or other number): a finite decimal number.
double q_f64(const Query& q, const char* k) {
  auto v = q.get(k);
  if (!v) throw ApiError{400, std::string("missing query parameter ") + k};
  double x;
  std::string t = !v->empty() && (*v)[0] == '+' ? v->substr(1) : *v;
  if (!decimal(*v) || !core::parse_double(t, x) || !std::isfinite(x))
    throw ApiError{400, std::string(k) + " must be a finite number"};
  return x;
}

// t0 and t1: finite, t0 < t1.
std::pair<double, double> q_span(const Query& q) {
  double t0 = q_f64(q, "t0"), t1 = q_f64(q, "t1");
  if (!(t1 > t0)) throw ApiError{400, "t1 must be after t0"};
  return {t0, t1};
}

std::int64_t q_i64(const Query& q, const char* k, std::int64_t def) {
  auto v = q.get(k);
  if (!v) return def;
  std::string_view d = *v;
  if (!d.empty() && (d[0] == '+' || d[0] == '-')) d.remove_prefix(1);
  std::int64_t x;
  std::string t = !v->empty() && (*v)[0] == '+' ? v->substr(1) : *v;
  if (!all_digits(d) || !core::parse_int(t, x)) throw ApiError{400, std::string(k) + " must be an integer"};
  return x;
}

// The channel ids of ch (a comma-separated list), each once, in order: ids of channels of the
// dataset (0 <= id < channels), at most MAX_CHANNELS.
std::vector<std::int64_t> q_channels(const Query& q, std::size_t channels) {
  std::string s = q.get("ch").value_or("");
  std::vector<std::int64_t> out;
  std::size_t a = 0;
  while (a <= s.size()) {
    std::size_t b = s.find(',', a);
    if (b == std::string::npos) b = s.size();
    std::string c = s.substr(a, b - a);
    a = b + 1;
    if (c.empty()) continue;
    std::int64_t x;
    if (!all_digits(c) || !core::parse_int(c, x)) throw ApiError{400, "ch must be a comma-separated list of channel ids"};
    if (static_cast<std::uint64_t>(x) >= channels) throw ApiError{400, "no channel " + std::to_string(x) + " in this dataset"};
    if (std::find(out.begin(), out.end(), x) == out.end()) out.push_back(x);
  }
  if (out.size() > MAX_CHANNELS)
    throw ApiError{400, "at most " + std::to_string(MAX_CHANNELS) + " channels at once"};
  return out;
}

bool q_bool(const Query& q, const char* k) {
  auto v = q.get(k);
  if (!v) return false;
  std::string l;
  for (char c : *v) l += static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  return l == "true" || l == "1" || l == "yes" || l == "on";
}

// The JSON object of a request body; a body that is not one is a bad request.
Dict json_body(const std::string& body) {
  Value v;
  try {
    v = Value::from_json(body);
  } catch (const std::exception& e) {
    throw ApiError{400, std::string("bad request body: ") + e.what()};
  }
  Dict* d = v.as_dict();
  if (!d) throw ApiError{400, "bad request body: expected a JSON object"};
  return std::move(*d);
}

// A string field of a request body (required).
std::string str_field(const Dict& d, const char* k) {
  const Value* v = d.get(k);
  if (!v) throw ApiError{400, std::string("bad request body: missing field `") + k + "`"};
  if (!v->as_str()) throw ApiError{400, std::string("bad request body: invalid type for `") + k + "`: expected a string"};
  return *v->as_str();
}

std::optional<Value> entry(const std::string& dir, const std::string& name) {
  fs::path full = core::utf8_path(dir) / core::utf8_path(name);
  std::string fsp = core::path_utf8(full);
  std::error_code ec;
  bool link = fs::is_symlink(fs::symlink_status(full, ec));
  if (link && !fs::exists(full, ec))
    return Value(Dict{{"name", Value(name)}, {"path", Value(fsp)}, {"type", Value("broken")}, {"link", Value(true)}});
  if (is_record(full)) {
    bool hea = name.size() >= 4 && name.compare(name.size() - 4, 4, ".hea") == 0;
    std::string record = hea ? name.substr(0, name.size() - 4) : name;
    return Value(Dict{{"name", Value(record)},
                      {"path", Value(fsp)},
                      {"type", Value("record")},
                      {"kind", Value("wfdb")},
                      {"label", Value(format_label(full))},
                      {"link", Value(link)}});
  }
  auto k = kind_of(full);
  auto size = [&] {
    std::uintmax_t n = fs::file_size(full, ec);
    return static_cast<unsigned long long>(ec ? 0 : n);
  };
  if (k && *k == "wfdb")
    return Value(Dict{{"name", Value(name)}, {"path", Value(fsp)}, {"type", Value("wfdb")}, {"link", Value(link)}});
  if (k) {
    return Value(Dict{{"name", Value(name)},
                      {"path", Value(fsp)},
                      {"type", Value("file")},
                      {"kind", Value(*k)},
                      {"label", Value(source_format_label(full))},
                      {"size", Value(size())},
                      {"link", Value(link)}});
  }
  if (fs::is_directory(full, ec))
    return Value(Dict{{"name", Value(name)}, {"path", Value(fsp)}, {"type", Value("dir")}, {"link", Value(link)}});
  return std::nullopt;
}

std::optional<std::vector<std::string>> sorted_names(const std::string& dir) {
  std::error_code ec;
  fs::directory_iterator it(core::utf8_path(dir), ec);
  if (ec) return std::nullopt;
  std::vector<std::string> names;
  for (; it != fs::directory_iterator(); it.increment(ec)) {
    if (ec) break;
    names.push_back(core::path_utf8(it->path().filename()));
  }
  std::stable_sort(names.begin(), names.end(),
                   [](const std::string& a, const std::string& b) { return to_lower(a) < to_lower(b); });
  return names;
}

std::string url_decode(const std::string& s) {
  std::string out;
  for (std::size_t i = 0; i < s.size(); ++i) {
    char c = s[i];
    if (c == '+') {
      out += ' ';
    } else if (c == '%' && i + 2 < s.size()) {
      auto hexv = [](char h) -> int {
        if (h >= '0' && h <= '9') return h - '0';
        if (h >= 'a' && h <= 'f') return h - 'a' + 10;
        if (h >= 'A' && h <= 'F') return h - 'A' + 10;
        return -1;
      };
      int a = hexv(s[i + 1]), b = hexv(s[i + 2]);
      if (a >= 0 && b >= 0) {
        out += static_cast<char>(a * 16 + b);
        i += 2;
      } else {
        out += '%';
      }
    } else {
      out += c;
    }
  }
  if (!core::utf8_valid(out)) {  // String::from_utf8_lossy
    std::string fixed;
    for (char32_t c : core::utf8_decode(out)) core::utf8_append(fixed, c);
    return fixed;
  }
  return out;
}

std::string form_value(const std::string& body, const std::string& key) {
  std::size_t a = 0;
  while (a <= body.size()) {
    std::size_t b = body.find('&', a);
    if (b == std::string::npos) b = body.size();
    std::string kv = body.substr(a, b - a);
    a = b + 1;
    std::size_t eq = kv.find('=');
    std::string k = eq == std::string::npos ? kv : kv.substr(0, eq);
    std::string v = eq == std::string::npos ? "" : kv.substr(eq + 1);
    if (url_decode(k) == key) return url_decode(v);
  }
  return "";
}

void redirect_home(httplib::Response& res) {
  res.status = 303;
  res.set_header("Location", "./");
}

const char* mime_of(const std::string& path) {
  std::string ext = path.substr(path.find_last_of('.') == std::string::npos ? path.size() : path.find_last_of('.'));
  for (char& c : ext) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  if (ext == ".html" || ext == ".htm") return "text/html";
  if (ext == ".css") return "text/css";
  if (ext == ".js" || ext == ".mjs") return "text/javascript";
  if (ext == ".json") return "application/json";
  if (ext == ".svg") return "image/svg+xml";
  if (ext == ".png") return "image/png";
  if (ext == ".ico") return "image/x-icon";
  if (ext == ".woff2") return "font/woff2";
  if (ext == ".txt") return "text/plain";
  return "application/octet-stream";
}

std::string parent_of(const std::string& p) {
  fs::path x = core::utf8_path(p);
  if (!x.has_relative_path()) return "";  // a root has no parent
  return core::path_utf8(x.parent_path());
}

}  // namespace

namespace {

// The routes and the methods they take (a GET route answers HEAD too), for the 405 of a known
// path asked with another method.
struct Route {
  std::regex path;
  const char* method;
};

const std::vector<Route>& routes() {
  static const std::vector<Route> r = [] {
    std::vector<Route> v;
    for (const char* p : {"/", "/static/.+", "/api/v1/config", "/api/v1/browse", "/api/v1/complete", "/api/v1/scan",
                          "/api/v1/datasets/[^/]+", "/api/v1/datasets/[^/]+/window", "/api/v1/datasets/[^/]+/vitals",
                          "/api/v1/datasets/[^/]+/vitals_at", "/api/v1/datasets/[^/]+/markers"})
      v.push_back({std::regex(p), "GET"});
    for (const char* p : {"/login", "/logout", "/api/v1/open", "/api/v1/cancel"}) v.push_back({std::regex(p), "POST"});
    return v;
  }();
  return r;
}

// A request body of at most `limit` bytes (413 beyond).
void check_body(const httplib::Request& req, std::size_t limit) {
  if (req.body.size() > limit)
    throw ApiError{413, "the request body is larger than " + std::to_string(limit) + " bytes"};
}

}  // namespace

void add_routes(httplib::Server& svr, std::shared_ptr<App> app) {
  // Limits on what a client may send, and how long it may take (the bodies are checked again per
  // route: login takes less).
  svr.set_payload_max_length(MAX_API_BODY);
  svr.set_read_timeout(30, 0);
  svr.set_write_timeout(60, 0);
  svr.set_keep_alive_timeout(5);
  svr.set_default_headers({{"X-Content-Type-Options", "nosniff"}});
  // a failure while answering is an internal error, whose text is not shown
  svr.set_exception_handler([](const httplib::Request&, httplib::Response& res, std::exception_ptr) {
    error(res, 500, "internal error");
  });

  // Every request (docs/viewer-api.md, "Requests"): a Host this server answers for (400
  // otherwise).  One that changes something (any method but GET and HEAD): no foreign Origin and
  // no cross-site fetch (403), and a POST to the API, a JSON body (415).  Then the sign-in (deny
  // by default: before it only the page, which then shows the sign-in form, login, logout and the
  // static files are served), and a known path asked with another method (405).
  svr.set_pre_routing_handler([app](const httplib::Request& req, httplib::Response& res) {
    using R = httplib::Server::HandlerResponse;
    if (!req.has_header("Host") || !app->host_allowed(req.get_header_value("Host"))) {
      error(res, 400, "this server does not answer for that host (see allowed_hosts)");
      return R::Handled;
    }
    const std::string& p = req.path;
    if (req.method != "GET" && req.method != "HEAD") {
      bool foreign = false;
      if (req.has_header("Origin")) {
        std::string o = req.get_header_value("Origin");
        std::string rest = o.rfind("http://", 0) == 0 ? o.substr(7) : o.rfind("https://", 0) == 0 ? o.substr(8) : "";
        foreign = rest.empty() || rest.find('/') != std::string::npos || !app->host_allowed(rest);
      }
      bool cross_site = req.get_header_value("Sec-Fetch-Site") == "cross-site";
      if (foreign || cross_site) {
        error(res, 403, "a request from another site");
        return R::Handled;
      }
      std::string ct = req.get_header_value("Content-Type");
      ct = to_lower(trim(ct.substr(0, ct.find(';'))));
      if (req.method == "POST" && p.rfind("/api/", 0) == 0 && ct != "application/json") {
        error(res, 415, "the request body must be JSON (Content-Type: application/json)");
        return R::Handled;
      }
    }
    bool open = p == "/" || p == "/login" || p == "/logout" || p.rfind("/static/", 0) == 0;
    if (!open && !app->logged_in(req)) {
      error(res, 401, "sign in required");
      return R::Handled;
    }
    bool known = false, allowed = false;
    for (const auto& r : routes()) {
      if (!std::regex_match(p, r.path)) continue;
      known = true;
      if (req.method == r.method || (req.method == "HEAD" && std::string(r.method) == "GET")) allowed = true;
    }
    if (known && !allowed) {
      error(res, 405, "Method Not Allowed");
      return R::Handled;
    }
    return R::Unhandled;
  });

  svr.Get("/", [app](const httplib::Request& req, httplib::Response& res) {
    if (app->logged_in(req)) {
      const auto& files = web::files();
      auto it = files.find("index.html");
      std::string body = it == files.end() ? std::string() : std::string(it->second);
      res.set_header("Cache-Control", "no-cache");
      return html(res, 200, body);
    }
    html(res, 401, login_page(""));
  });

  svr.Post("/login", [app](const httplib::Request& req, httplib::Response& res) {
    if (!app->prot) return redirect_home(res);
    if (req.body.size() > MAX_LOGIN_BODY)
      return error(res, 413, "the request body is larger than " + std::to_string(MAX_LOGIN_BODY) + " bytes");
    const std::string& client = req.remote_addr;
    // after wrong passwords, the next one from the same address is refused until its delay has
    // passed (no worker waits it out)
    auto wait = app->login_wait(client);
    if (wait.count() > 0) {
      res.set_header("Retry-After", std::to_string(wait.count()));
      return html(res, 429,
                  login_page("<p class=\"err\">Too many wrong passwords: try again in " + std::to_string(wait.count()) +
                             " s.</p>"));
    }
    std::string password = form_value(req.body, "password");
    bool good;
    {
      std::lock_guard<std::mutex> one(app->checking);
      good = check_password(*app->creds, password);
    }
    if (!good) {
      app->login_failed(client);
      return html(res, 401, login_page("<p class=\"err\">Wrong password.</p>"));
    }
    app->login_succeeded(client);
    redirect_home(res);
    res.set_header("Set-Cookie", app->cookie + "=" + make_cookie(*app->creds) + "; Max-Age=" +
                                     std::to_string(LOGIN_DAYS * 86400) + "; " + app->cookie_attributes());
  });

  // Remove the cookie; a valid sign-in presented here is also ended on the server, so that a copy
  // of the cookie no longer works (kept in the auth file: docs/viewer-api.md, "Sign-in").
  svr.Post("/logout", [app](const httplib::Request& req, httplib::Response& res) {
    if (app->prot && app->logged_in(req)) {
      std::string v = app->cookie_of(req).value_or("");
      std::size_t dot = v.find('.');
      std::int64_t expires;
      if (dot != std::string::npos && all_digits(v.substr(0, dot)) && core::parse_int(v.substr(0, dot), expires))
        app->revoke(v.substr(dot + 1), static_cast<std::uint64_t>(expires));
    }
    redirect_home(res);
    res.set_header("Set-Cookie", app->cookie + "=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; " +
                                     app->cookie_attributes());
  });

  svr.Get(R"(/static/(.+))", [](const httplib::Request& req, httplib::Response& res) {
    std::string path = req.matches[1];
    const auto& files = web::files();
    auto it = files.find(path);
    if (it == files.end()) return error(res, 404, "Not Found");
    std::string mime = mime_of(path);
    if (mime == "text/html") res.set_header("Content-Security-Policy", CSP);
    res.set_content(std::string(it->second), mime);
  });

  svr.Get("/api/v1/config", api([app](const httplib::Request&, httplib::Response& res) {
            std::string h = home();
            List places;
            if (app->settings.allowed(h)) places.push_back(Value(Dict{{"name", Value("Home")}, {"path", Value(h)}}));
            if (auto r = repo_root()) {
              std::string sd = core::path_utf8(*r / "sample_data");
              if (app->settings.allowed(sd))
                places.push_back(Value(Dict{{"name", Value("sample_data")}, {"path", Value(sd)}}));
            }
            for (const auto& r : app->settings.data_roots)
              if (r != h && r != fs_root()) places.push_back(Value(Dict{{"name", Value(r)}, {"path", Value(r)}}));
            json(res, 200,
                 Value(Dict{{"timezone", Value(app->settings.timezone)},
                            {"data_roots", Value::list_of(app->settings.data_roots)},
                            {"home", Value(h)},
                            {"start", Value(app->start_folder())},
                            {"places", Value(std::move(places))},
                            {"sep", Value(SEP)},
                            {"version", Value(VS_VIEWER_VERSION)},
                            {"qc_verbose", Value(app->settings.qc_verbose)},
                            {"limits", Value(Dict{{"max_records", Value(app->settings.limits.max_records)},
                                                  {"max_depth", Value(app->settings.limits.max_depth)},
                                                  {"max_scan_s", Value(app->settings.limits.max_scan_s)}})}}));
          }));

  svr.Get("/api/v1/browse", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            auto pp = q.get("path");
            std::string path = (pp && !pp->empty()) ? *pp : app->start_folder();
            std::string p = app->check_path(path);
            bool hidden = q_bool(q, "hidden");
            auto k = kind_of(core::utf8_path(p));
            std::error_code ec;
            bool dir = fs::is_directory(core::utf8_path(p), ec);
            // a recording file (a WFDB record's .hea, or a file an adapter reads)
            if (k && !dir) return json(res, 200, Value(Dict{{"path", Value(p)}, {"type", Value("file")}}));
            if (!dir) throw ApiError{400, "not a folder or a recording this viewer reads: " + p};
            auto names = sorted_names(p);
            if (!names) throw ApiError{403, "permission denied: " + p};
            // the segments and layout headers of the multi-segment records here: part of those
            // records, listed only with the hidden entries
            std::set<std::string> segments;
            for (const auto& n : *names) {
              if (n.size() < 4 || n.compare(n.size() - 4, 4, ".hea") != 0) continue;
              auto table = segment_table(core::utf8_path(p) / core::utf8_path(n.substr(0, n.size() - 4)));
              if (table)
                for (const auto& [seg, samples] : *table) segments.insert(seg + ".hea");
            }
            List entries;
            bool truncated = false;
            for (const auto& name : *names) {
              if (((!name.empty() && name[0] == '.') || segments.count(name)) && !hidden) continue;
              // a symbolic link that leads outside the data roots is not listed
              if (!app->settings.allowed(core::path_utf8(core::utf8_path(p) / core::utf8_path(name)))) continue;
              if (auto e = entry(p, name)) {
                entries.push_back(std::move(*e));
                if (entries.size() >= MAX_ENTRIES) {
                  truncated = true;
                  break;
                }
              }
            }
            auto key_of = [](const Value& e) {
              const Value* t = e.get("type");
              bool file = t && t->as_str() && (*t->as_str() == "file" || *t->as_str() == "record");
              const Value* n = e.get("name");
              return std::make_pair(file, to_lower(n ? n->py_str() : std::string()));
            };
            std::stable_sort(entries.begin(), entries.end(),
                             [&](const Value& a, const Value& b) { return key_of(a) < key_of(b); });
            std::string parent = parent_of(p);
            Value pv;
            if (!parent.empty() && parent != p && app->settings.allowed(parent)) pv = Value(parent);
            json(res, 200,
                 Value(Dict{{"path", Value(p)},
                            {"type", Value("dir")},
                            {"kind", k ? Value(*k) : Value()},
                            {"entries", Value(std::move(entries))},
                            {"truncated", Value(truncated)},
                            {"parent", pv}}));
          }));

  svr.Get("/api/v1/complete", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            std::string prefix = q.get("prefix").value_or("");
            std::string text = prefix.empty() ? app->start_folder() + SEP : expand(prefix);
            std::string folder, part;
            if (!text.empty() && is_sep(text.back())) {
              folder = text;
            } else {
              std::size_t i = std::string::npos;
              for (std::size_t j = 0; j < text.size(); ++j)
                if (is_sep(text[j])) i = j;
              if (i == std::string::npos) {
                part = text;
              } else {
                folder = text.substr(0, i + 1);
                part = text.substr(i + 1);
              }
            }
            folder = folder.empty() ? fs_root() : absolute(folder);
            Value empty_items(Dict{{"items", Value(List())}});
            std::error_code ec;
            if (!app->settings.allowed(folder) || !fs::is_directory(core::utf8_path(folder), ec))
              return json(res, 200, empty_items);
            auto names = sorted_names(folder);
            if (!names) return json(res, 200, empty_items);
            List out;
            std::string lpart = to_lower(part);
            for (const auto& name : *names) {
              std::string lname = to_lower(name);
              if (lname.rfind(lpart, 0) != 0 || (!name.empty() && name[0] == '.' && (part.empty() || part[0] != '.')))
                continue;
              if (!app->settings.allowed(core::path_utf8(core::utf8_path(folder) / core::utf8_path(name)))) continue;
              auto e = entry(folder, name);
              if (!e) continue;
              std::string t = e->get("type")->py_str();
              if (t == "broken") continue;
              std::string p = e->get("path")->py_str();
              if (t == "dir") p += SEP;
              out.push_back(Value(Dict{{"path", Value(p)}, {"type", Value(t)}}));
              if (out.size() >= MAX_COMPLETIONS) break;
            }
            json(res, 200, Value(Dict{{"items", Value(std::move(out))}}));
          }));

  // What opening a folder of records (or one record) would read, from the headers alone, within
  // the limits (docs/viewer-api.md, scan).
  svr.Get("/api/v1/scan", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            std::string p = app->check_path(q.get("path").value_or(""));
            const fs::path path = core::utf8_path(p);
            std::error_code ec;
            if (!fs::is_directory(path, ec) && !is_record(path))
              throw ApiError{400, "not a folder or a WFDB record: " + p};
            Dict d;
            try {
              d = scan(path, app->settings.limits);
            } catch (const std::exception& e) {
              throw ApiError{422, e.what()};
            }
            json(res, 200, Value(std::move(d)));
          }));

  svr.Post("/api/v1/open", api([app](const httplib::Request& req, httplib::Response& res) {
             check_body(req, MAX_API_BODY);
             Dict body = json_body(req.body);
             std::string given = str_field(body, "path");
             const Value* rv = body.get("reindex");
             if (rv && !rv->as_bool()) throw ApiError{400, "bad request body: invalid type for `reindex`: expected a boolean"};
             bool reindex = rv && *rv->as_bool();
             std::string p = app->check_path(given);
             std::error_code ec;
             const fs::path path = core::utf8_path(p);
             const bool dir = fs::is_directory(path, ec);
             if (!dir && !kind_of(path)) throw ApiError{400, "not a recording this viewer reads: " + p};
             // a WFDB record or folder in a build without WFDB support
             if (auto why = unreadable(path)) throw ApiError{422, *why};
             // one record: every file it reads lies inside the data roots (its signal files may be links)
             if (is_record(path)) app->check_record(path);
             // a folder: within the limits on the records it holds, before anything is read
             if (dir && !fs::is_regular_file(path / "index.csv", ec)) {
               try {
                 scan(path, app->settings.limits);
               } catch (const std::exception& e) {
                 throw ApiError{422, e.what()};
               }
             }
             std::string key = cache_key(core::utf8_path(p));
             {
               std::lock_guard<std::mutex> l(app->m);
               app->opened.insert(key);
             }
             if (!reindex && app->dataset(key))
               return json(res, 200, Value(Dict{{"key", Value(key)}, {"state", Value("ready")}}));
             {
               std::lock_guard<std::mutex> l(app->m);
               auto it = app->jobs.find(key);
               if (it == app->jobs.end() || it->second.state != Job::State::Indexing) {
                 Job job;
                 job.fraction = std::make_shared<std::atomic<double>>(0.0);
                 job.started = std::chrono::steady_clock::now();
                 job.cancel = std::make_shared<std::atomic<bool>>(false);
                 app->jobs[key] = job;
                 app->loaded.shift_remove(key);
                 app->enqueue(key, p);
               }
             }
             json(res, 200, Value(Dict{{"key", Value(key)}, {"state", Value("indexing")}}));
           }));

  // Stop an index being built (docs/viewer-api.md, cancel).
  svr.Post("/api/v1/cancel", api([app](const httplib::Request& req, httplib::Response& res) {
             check_body(req, MAX_API_BODY);
             std::string key = str_field(json_body(req.body), "key");
             std::string state;
             {
               std::lock_guard<std::mutex> l(app->m);
               auto it = app->jobs.find(key);
               if (it != app->jobs.end() && it->second.state == Job::State::Indexing) {
                 it->second.cancel->store(true);
                 state = "cancelling";
               } else if (it != app->jobs.end() && it->second.state == Job::State::Cancelled) {
                 state = "cancelled";
               } else {
                 throw ApiError{404, "no index being built under this key"};
               }
             }
             json(res, 200, Value(Dict{{"key", Value(key)}, {"state", Value(state)}}));
           }));

  svr.Get(R"(/api/v1/datasets/([^/]+))", api([app](const httplib::Request& req, httplib::Response& res) {
            std::string key = req.matches[1];
            {
              std::lock_guard<std::mutex> l(app->m);
              auto it = app->jobs.find(key);
              if (it != app->jobs.end() && it->second.state == Job::State::Indexing) {
                double fr = it->second.fraction->load();
                double el = std::chrono::duration<double>(std::chrono::steady_clock::now() - it->second.started).count();
                return json(res, 200,
                            Value(Dict{{"state", Value("indexing")},
                                       {"progress", Value(Dict{{"state", Value("indexing")},
                                                               {"fraction", Value(fr)},
                                                               {"elapsed_s", Value(el)}})}}));
              }
              if (it != app->jobs.end() && it->second.state == Job::State::Error)
                return json(res, 200, Value(Dict{{"state", Value("error")}, {"error", Value(it->second.error)}}));
              if (it != app->jobs.end() && it->second.state == Job::State::Cancelled)
                return json(res, 200, Value(Dict{{"state", Value("cancelled")}}));
            }
            auto ds = app->need(key);
            json(res, 200, Value(Dict{{"state", Value("ready")}, {"dataset", ds->info()}}));
          }));

  svr.Get(R"(/api/v1/datasets/([^/]+)/window)", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            auto [t0, t1] = q_span(q);
            std::int64_t px = q_i64(q, "px", 1200);
            auto ds = app->need(req.matches[1]);
            auto ch = q_channels(q, ds->channel_count());
            std::string text;
            try {
              text = ds->window_json(ch, t0, t1, px);
            } catch (const std::exception& e) {
              throw ApiError{500, e.what()};
            }
            res.set_content(text, "application/json");
          }));

  svr.Get(R"(/api/v1/datasets/([^/]+)/vitals)", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            auto [t0, t1] = q_span(q);
            std::int64_t px = std::clamp<std::int64_t>(q_i64(q, "px", 1200), 16, 8000);
            std::vector<std::string> names;
            std::string s = q.get("names").value_or("");
            std::size_t a = 0;
            while (a <= s.size()) {
              std::size_t b = s.find('|', a);
              if (b == std::string::npos) b = s.size();
              std::string n = s.substr(a, b - a);
              if (!n.empty() && std::find(names.begin(), names.end(), n) == names.end()) names.push_back(n);
              a = b + 1;
            }
            if (names.size() > MAX_SERIES)
              throw ApiError{400, "at most " + std::to_string(MAX_SERIES) + " vital-sign series at once"};
            auto ds = app->need(req.matches[1]);
            json(res, 200, ds->vitals_window(names, t0, t1, px));
          }));

  svr.Get(R"(/api/v1/datasets/([^/]+)/vitals_at)", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            double t = q_f64(q, "t");
            auto ds = app->need(req.matches[1]);
            json(res, 200, ds->vitals_at(t));
          }));

  svr.Get(R"(/api/v1/datasets/([^/]+)/markers)", api([app](const httplib::Request& req, httplib::Response& res) {
            Query q(req);
            auto [t0, t1] = q_span(q);
            auto ds = app->need(req.matches[1]);
            auto ch = q_channels(q, ds->channel_count());
            json(res, 200, ds->markers(ch, t0, t1));
          }));

  // anything else, and errors without a body (a body over its limit): JSON
  svr.set_error_handler([](const httplib::Request& req, httplib::Response& res) {
    if (!res.body.empty()) return;
    if (res.status == 404) return error(res, 404, "Not Found");
    if (res.status == 413) {
      std::size_t limit = req.path == "/login" ? MAX_LOGIN_BODY : MAX_API_BODY;
      return error(res, 413, "the request body is larger than " + std::to_string(limit) + " bytes");
    }
    if (res.status == 500) return error(res, 500, "internal error");
  });
}

}  // namespace vitalstream::viewer
