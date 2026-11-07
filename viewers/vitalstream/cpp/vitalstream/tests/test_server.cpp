// The viewer's HTTP server (docs/viewer-api.md), as viewers/vitalstream/rust/tests/viewer.rs: the
// checks on every request (Host, Origin, Sec-Fetch-Site, JSON bodies and their limits, 405), the
// query grammar, sign-in (bodies, the delay after wrong passwords, logout ending the sign-in,
// Secure cookies), a recording that cannot be indexed, and a record whose signal file leads
// outside the data roots.
#include <gtest/gtest.h>
#include <httplib.h>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <functional>
#include <thread>

#include "auth.hpp"
#include "core/pyfmt.hpp"
#include "core/value.hpp"
#include "engine/sources.hpp"
#include "server.hpp"
#include "settings.hpp"
#include "needs_wfdb.hpp"

using namespace vitalstream::engine;
using namespace vitalstream::viewer;
namespace fs = std::filesystem;

namespace {

fs::path temp_dir(const char* name) {
  fs::path d = fs::temp_directory_path() / name;
  fs::remove_all(d);
  fs::create_directories(d);
  return fs::canonical(d);
}

void write(const fs::path& p, const std::string& text) {
  fs::create_directories(p.parent_path());
  std::ofstream(p, std::ios::binary) << text;
}

// A viewer listening on a free port of this machine, stopped at the end of the scope.
class Running {
 public:
  Running(const fs::path& dir, bool protect, const std::function<void(Settings&)>& more = {}) {
    Settings s = Settings::defaults();
    s.auth_file = dir / "auth.json";
    s.cache_dir = dir / "cache";
    if (more) more(s);
    s.finish();
    add_routes(svr_, make_app(s, protect));
    port_ = svr_.bind_to_any_port("127.0.0.1");
    thread_ = std::thread([this] { svr_.listen_after_bind(); });
    svr_.wait_until_ready();
  }
  ~Running() {
    svr_.stop();
    thread_.join();
  }
  // A client asking as a browser of this machine does ("Host: 127.0.0.1:<port>" unless given).
  httplib::Client client() const {
    httplib::Client c("127.0.0.1", port_);
    c.set_read_timeout(60, 0);
    return c;
  }

 private:
  httplib::Server svr_;
  std::thread thread_;
  int port_ = 0;
};

Value json(const httplib::Result& r) { return Value::from_json(r->body); }

std::string detail(const httplib::Result& r) { return json(r).get("detail")->py_str(); }

std::string open_body(const fs::path& p) { return "{\"path\": " + Value(p.string()).to_json() + "}"; }

// Open path and wait for its index: the dataset's final state.
Value opened(httplib::Client& c, const fs::path& path) {
  auto r = c.Post("/api/v1/open", open_body(path), "application/json");
  EXPECT_EQ(r->status, 200) << r->body;
  std::string key = json(r).get("key")->py_str();
  for (int i = 0; i < 400; ++i) {
    auto d = json(c.Get("/api/v1/datasets/" + key));
    if (d.get("state")->py_str() != "indexing") return d;
    std::this_thread::sleep_for(std::chrono::milliseconds(25));
  }
  ADD_FAILURE() << "still indexing";
  return Value();
}

}  // namespace

TEST(Server, RequestsFromOtherSites) {
  fs::path dir = temp_dir("vs_server_sites");
  {
  Running viewer(dir, false);
  auto c = viewer.client();
  // DNS rebinding: a name this server does not answer for
  for (auto [host, want] : std::vector<std::pair<std::string, int>>{{"localhost:8060", 200},
                                                                    {"127.0.0.1:8060", 200},
                                                                    {"[::1]:8060", 200},
                                                                    {"LOCALHOST", 200},
                                                                    {"evil.example:8060", 400},
                                                                    {"localhost.evil.example", 400},
                                                                    {"", 400}}) {
    auto r = c.Get("/api/v1/config", httplib::Headers{{"Host", host}});
    EXPECT_EQ(r->status, want) << host;
  }
  {
    Running proxied(dir, false, [](Settings& s) { s.allowed_hosts = {"portal.example.org"}; });
    auto p = proxied.client();
    EXPECT_EQ(p.Get("/api/v1/config", httplib::Headers{{"Host", "portal.example.org"}})->status, 200);
    EXPECT_EQ(p.Get("/api/v1/config", httplib::Headers{{"Host", "other.example.org"}})->status, 400);
  }
  // a request that changes something: JSON, from no other site
  std::string body = open_body(fs::canonical(fs::path(VS_TESTDATA) / "edf" / "plain.edf"));
  EXPECT_EQ(c.Post("/api/v1/open", body, "text/plain")->status, 415);
  EXPECT_EQ(c.Post("/api/v1/open", body, "application/x-www-form-urlencoded")->status, 415);
  EXPECT_EQ(c.Post("/api/v1/open", {{"Origin", "http://evil.example"}}, body, "application/json")->status, 403);
  EXPECT_EQ(c.Post("/api/v1/open", {{"Origin", "null"}}, body, "application/json")->status, 403);
  EXPECT_EQ(c.Post("/api/v1/cancel", {{"Sec-Fetch-Site", "cross-site"}}, "{\"key\":\"0\"}", "application/json")->status,
            403);
  EXPECT_EQ(c.Post("/api/v1/open", {{"Origin", "http://localhost:8060"}}, body, "application/json; charset=utf-8")->status,
            200);
  // fields of the wrong type, and bodies that are not a JSON object
  std::string typed = body.substr(0, body.size() - 1) + ", \"reindex\": \"yes\"}";
  EXPECT_EQ(c.Post("/api/v1/open", typed, "application/json")->status, 400);
  EXPECT_EQ(c.Post("/api/v1/open", "{\"path\": 3}", "application/json")->status, 400);
  EXPECT_EQ(c.Post("/api/v1/open", "[]", "application/json")->status, 400);
  // a body nested deeply: refused, the server goes on
  auto deep = c.Post("/api/v1/open", std::string(20000, '['), "application/json");
  EXPECT_EQ(deep->status, 400);
  EXPECT_NE(detail(deep).find("nested too deeply"), std::string::npos);
  // bodies: at most 64 KiB
  auto big = c.Post("/api/v1/cancel", std::string(70000, ' '), "application/json");
  EXPECT_EQ(big->status, 413);
  EXPECT_EQ(detail(big), "the request body is larger than 65536 bytes");
  // a method a known path does not take: 405, in JSON; an unknown path: 404
  auto del = c.Delete("/api/v1/config");
  EXPECT_EQ(del->status, 405);
  EXPECT_EQ(detail(del), "Method Not Allowed");
  EXPECT_EQ(c.Get("/api/v1/open")->status, 405);
  EXPECT_EQ(c.Get("/api/v1/nothing")->status, 404);
  // the page: its Content-Security-Policy, nosniff on every answer, no inline style or script
  auto page = c.Get("/");
  std::string csp = page->get_header_value("Content-Security-Policy");
  EXPECT_TRUE(csp.find("script-src 'self'") != std::string::npos && csp.find("frame-ancestors 'self'") != std::string::npos &&
              csp.find("unsafe") == std::string::npos)
      << csp;
  EXPECT_EQ(c.Get("/api/v1/config")->get_header_value("X-Content-Type-Options"), "nosniff");
  EXPECT_EQ(c.Get("/static/login.css")->status, 200);
  }
  fs::remove_all(dir);
}

TEST(Server, QueriesAreCheckedAndBounded) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_server_queries");
  {
  Running viewer(dir, false);
  auto c = viewer.client();
  Value v = opened(c, fs::canonical(fs::path(VS_TESTDATA) / "collection"));
  const Value* ds = v.get("dataset");
  ASSERT_TRUE(ds);
  std::string key = ds->get("key")->py_str();
  double t0 = *f64_of(&(*ds->get("t_range")->as_list())[0]);
  auto num = [](double x) { return vitalstream::core::repr(x); };
  std::string t1 = num(t0 + 1), t2 = num(t0 + 2), w = "/api/v1/datasets/" + key + "/";
  // a channel asked for many times is answered once
  std::string many;
  for (int i = 0; i < 3000; ++i) many += i ? ",0" : "0";
  auto r = c.Get(w + "window?t0=" + t1 + "&t1=" + t2 + "&ch=" + many);
  ASSERT_EQ(r->status, 200);
  EXPECT_EQ(json(r).get("channels")->as_dict()->size(), 1u);
  for (const std::string& q :
       {"window?t0=" + t1 + "&t1=" + t2 + "&ch=0,99", "window?t0=" + t1 + "&t1=" + t2 + "&ch=-1",
        "window?t0=-inf&t1=" + t2 + "&ch=0", "window?t0=nan&t1=" + t2 + "&ch=0", "window?t0=0x10&t1=" + t2 + "&ch=0",
        "window?t0=1_0&t1=" + t2 + "&ch=0", "window?t0=1e400&t1=" + t2 + "&ch=0",
        "window?t0=%20" + t1 + "&t1=" + t2 + "&ch=0", "window?t0=" + t1 + "&t1=" + t2 + "&px=1.5&ch=0",
        "window?t0=" + t1 + "&t0=" + t1 + "&t1=" + t2 + "&ch=0", "window?t0=" + t2 + "&t1=" + t1 + "&ch=0",
        "vitals?t0=" + t2 + "&t1=" + t1 + "&names=HR", "vitals?t0=" + t1 + "&t1=inf&names=HR",
        "markers?t0=" + t1 + "&t1=Infinity&ch=0", std::string("vitals_at?t=-inf")}) {
    auto a = c.Get(w + q);
    EXPECT_EQ(a->status, 400) << q << ": " << a->body;
  }
  // times far outside the recording: no data, no failure
  for (const char* q : {"window?t0=-1e308&t1=1e308&ch=0,1", "window?t0=1e300&t1=1.5e300&ch=0",
                        "markers?t0=-1e308&t1=1e308&ch=0"}) {
    auto a = c.Get(w + q);
    EXPECT_EQ(a->status, 200) << q << ": " << a->body;
  }
  std::string names;
  for (int i = 0; i < 2000; ++i) names += i ? "|HR" : "HR";
  auto a = c.Get(w + "vitals?t0=" + num(t0) + "&t1=" + t2 + "&names=" + names);
  ASSERT_EQ(a->status, 200);
  EXPECT_EQ(json(a).as_dict()->size(), 1u);
  std::string distinct;
  for (int i = 0; i < 600; ++i) distinct += (i ? "|n" : "n") + std::to_string(i);
  EXPECT_EQ(c.Get(w + "vitals?t0=0&t1=1&names=" + distinct)->status, 400);
  }
  fs::remove_all(dir);
}

TEST(Server, ARecordingThatCannotBeIndexedIsAnError) {
  VS_NEEDS_WFDB();
  // index.csv without a record column: the state is "error", with a reason, and nothing is left
  // behind in the index folder
  fs::path dir = temp_dir("vs_server_error");
  write(dir / "c" / "index.csv", "name\nx\n");
  {
    Running viewer(dir, false);
    auto c = viewer.client();
    Value v = opened(c, dir / "c");
    EXPECT_EQ(v.get("state")->py_str(), "error");
    EXPECT_NE(v.get("error")->py_str().find("no record column"), std::string::npos);
  }
  EXPECT_TRUE(fs::is_empty(dir / "cache"));
  fs::remove_all(dir);
}

#ifndef _WIN32
TEST(Server, ASignalFileOutsideTheRootsIsRefused) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_server_outside");
  fs::path root = dir / "root", outside = dir / "outside";
  write(outside / "secret.dat", std::string("\1\0\2\0", 4));
  write(root / "r.hea", "r 1 250 2\nr.dat 16 200/mV 16 0 0 3 0 II\n");
  fs::create_symlink(outside / "secret.dat", root / "r.dat");
  {
    Running viewer(dir, false, [&](Settings& s) { s.data_roots = {root.string()}; });
    auto c = viewer.client();
    auto r = c.Post("/api/v1/open", open_body(root / "r.hea"), "application/json");
    EXPECT_EQ(r->status, 403) << r->body;
    EXPECT_EQ(r->body.find("outside/"), std::string::npos) << "the target is not shown";
  }
  fs::remove_all(dir);
}
#endif

// A build without WFDB support (VS_WFDB=OFF): a WFDB record, a multi-segment record, a folder of
// records and a collection are listed, and refused when opened, with one message (NO_WFDB); an EDF
// file opens as in the default build.
TEST(Server, WithoutWfdbAWfdbRecordIsRefused) {
  if (wfdb_supported()) GTEST_SKIP() << "a build with WFDB support reads WFDB records";
  fs::path dir = temp_dir("vs_server_nowfdb");
  Running viewer(dir, false);
  auto c = viewer.client();
  for (const char* name : {"plain/rec_a.hea", "session/ms/ms_rec.hea", "plain", "collection", "session"}) {
    fs::path p = fs::canonical(fs::path(VS_TESTDATA) / name);
    auto r = c.Post("/api/v1/open", open_body(p), "application/json");
    EXPECT_EQ(r->status, 422) << name;
    EXPECT_EQ(detail(r), NO_WFDB) << name;
    auto sc = c.Get("/api/v1/scan?path=" + p.string());
    EXPECT_EQ(sc->status, 422) << name;
    EXPECT_EQ(detail(sc), NO_WFDB) << name;
  }
  // still listed as a record, so that opening it says why it is not read
  auto b = json(c.Get("/api/v1/browse?path=" + fs::canonical(fs::path(VS_TESTDATA) / "plain").string()));
  bool record = false;
  for (const auto& e : *b.get("entries")->as_list()) record = record || e.get("type")->py_str() == "record";
  EXPECT_TRUE(record);
  Value d = opened(c, fs::canonical(fs::path(VS_TESTDATA) / "edf" / "psg_plus.edf"));
  EXPECT_EQ(d.get("state")->py_str(), "ready");
  fs::remove_all(dir);
}

#if VS_PASSWORD
TEST(Server, SignIn) {
  fs::path dir = temp_dir("vs_server_signin");
  set_password(dir / "auth.json", "correct horse");
  std::string pair;
  {
    Running viewer(dir, true);
    auto c = viewer.client();
    EXPECT_EQ(c.Get("/api/v1/config")->status, 401);
    // a body larger than a password is refused before anything is checked
    EXPECT_EQ(c.Post("/login", std::string(5000, 'x'), "application/x-www-form-urlencoded")->status, 413);
    // a wrong password: 401; another one at once: refused until its delay has passed (429)
    EXPECT_EQ(c.Post("/login", "password=wrong", "application/x-www-form-urlencoded")->status, 401);
    auto again = c.Post("/login", "password=correct+horse", "application/x-www-form-urlencoded");
    EXPECT_EQ(again->status, 429);
    EXPECT_EQ(again->get_header_value("Retry-After"), "1");
    std::this_thread::sleep_for(std::chrono::milliseconds(1100));
    auto ok = c.Post("/login", "password=correct+horse", "application/x-www-form-urlencoded");
    ASSERT_EQ(ok->status, 303);
    std::string cookie = ok->get_header_value("Set-Cookie");
    EXPECT_EQ(cookie.find("Secure"), std::string::npos);
    pair = cookie.substr(0, cookie.find(';'));
    EXPECT_EQ(c.Get("/api/v1/config", httplib::Headers{{"Cookie", pair}})->status, 200);
    // a non-ASCII cookie is no sign-in (and no failure)
    EXPECT_EQ(c.Get("/api/v1/config", httplib::Headers{{"Cookie", "vitalstream_8070=\xc3\xa9.\xc3\xa9"}})->status, 401);
    // logout ends the sign-in on the server: a copy of the cookie no longer works
    auto out = c.Post("/logout", {{"Cookie", pair}}, "", "application/x-www-form-urlencoded");
    EXPECT_EQ(out->status, 303);
    EXPECT_NE(out->get_header_value("Set-Cookie").find("Max-Age=0"), std::string::npos);
    EXPECT_EQ(c.Get("/api/v1/config", httplib::Headers{{"Cookie", pair}})->status, 401);
  }
  // also after a restart (kept in the auth file), the password unchanged
  {
    Running viewer(dir, true);
    auto c = viewer.client();
    EXPECT_EQ(c.Get("/api/v1/config", httplib::Headers{{"Cookie", pair}})->status, 401);
  }
  auto a = load_auth(dir / "auth.json");
  ASSERT_TRUE(a);
  EXPECT_TRUE(check_password(*a, "correct horse"));
  EXPECT_EQ(revoked(dir / "auth.json").size(), 1u);
  // behind HTTPS: the cookie is marked Secure
  {
    Running viewer(dir, true, [](Settings& s) { s.secure_cookie = true; });
    auto c = viewer.client();
    auto ok = c.Post("/login", "password=correct+horse", "application/x-www-form-urlencoded");
    ASSERT_EQ(ok->status, 303);
    EXPECT_NE(ok->get_header_value("Set-Cookie").find("; Secure"), std::string::npos);
  }
  fs::remove_all(dir);
}
#endif
