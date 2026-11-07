// vitalstream-cpp: the VitalStream viewer (C++).  Starts the web server; --index PATH only
// indexes.  Same options, settings keys, API and password as the Python and Rust viewers.

#include <CLI/CLI.hpp>
#include <httplib.h>

#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <thread>

#include "engine/adapters.hpp"
#include "auth.hpp"
#include "engine/index.hpp"
#include "engine/sources.hpp"
#include "server.hpp"
#include "settings.hpp"
#include "core/paths.hpp"

#ifdef _WIN32
#include <windows.h>
#include <shellapi.h>
#else
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
extern char** environ;
#endif

#ifndef VS_VIEWER_VERSION
#define VS_VIEWER_VERSION "0.0.0"
#endif

using namespace vitalstream::engine;
using namespace vitalstream::viewer;

namespace {

std::atomic<bool> g_stop{false};

void on_signal(int) { g_stop = true; }

void open_browser(const std::string& url) {
  std::thread([url] {
    std::this_thread::sleep_for(std::chrono::seconds(1));
#ifdef _WIN32
    ShellExecuteA(nullptr, "open", url.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
#else
#ifdef __APPLE__
    const char* cmd = "open";
#else
    const char* cmd = "xdg-open";
#endif
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_addopen(&fa, 1, "/dev/null", O_WRONLY, 0);
    posix_spawn_file_actions_addopen(&fa, 2, "/dev/null", O_WRONLY, 0);
    std::string a0 = cmd, a1 = url;
    char* argv[] = {a0.data(), a1.data(), nullptr};
    pid_t pid;
    if (posix_spawnp(&pid, cmd, &fa, nullptr, argv, environ) == 0) waitpid(pid, nullptr, 0);
    posix_spawn_file_actions_destroy(&fa);
#endif
  }).detach();
}

}  // namespace

int main(int argc, char** argv) {
  CLI::App cli{"Web viewer for physiological recordings (WFDB folders, and the formats of installed adapters)", "vitalstream-cpp"};
  argv = cli.ensure_utf8(argv);
  cli.set_version_flag("-V,--version", std::string("vitalstream-cpp ") + VS_VIEWER_VERSION);
  std::optional<std::string> config, host, timezone, cache_dir, adapters_dir, index_path;
  std::optional<int> port;
  std::vector<std::string> roots;
  bool no_browser = false, set_pw = false, clear_pw = false, no_auth = false, qc_verbose = false;
  cli.add_option("--config", config,
                 "settings file (TOML); default: <viewer>/vitalstream.toml (run from the clone's build folder) "
                 "or ~/.config/vitalstream/vitalstream.toml if present (never one in the working directory); "
                 "keys of its [cpp] section apply here");
  cli.add_option("--host", host, "address to listen on (default 127.0.0.1: this machine only)");
  cli.add_option("--port", port, "port to listen on (default 8070)")->check(CLI::Range(0, 65535));
  cli.add_option("--timezone", timezone, "display time zone, e.g. America/New_York");
  cli.add_option("--root", roots,
                 "folder the viewer may open (repeatable), inside the viewer's root (set when it was built); "
                 "default: the viewer's root")
      ->expected(1)
      ->multi_option_policy(CLI::MultiOptionPolicy::TakeAll);
  cli.add_option("--cache-dir", cache_dir, "index cache folder");
  cli.add_option("--adapters-dir", adapters_dir,
                 "folder of adapters, programs that read other recording formats (docs/adapters.md)");
  cli.add_flag("--qc-verbose", qc_verbose,
               "also mark the irregularities a reader repaired (data blocks put back in order)");
  cli.add_flag("--no-browser", no_browser, "do not open a browser window");
  cli.add_flag("--set-password", set_pw, "set (or change) the password the page asks for, then exit");
  cli.add_flag("--clear-password", clear_pw, "remove the password, then exit");
  cli.add_flag("--no-auth", no_auth, "run without asking for the password");
  cli.add_option("--index", index_path, "index PATH into the cache and exit")->type_name("PATH");
  CLI11_PARSE(cli, argc, argv);

  Settings s;
  try {
    s = load_settings(config);
    if (host) s.host = *host;
    if (port) s.port = *port;
    if (timezone) s.timezone = *timezone;
    if (!roots.empty()) s.data_roots = roots;
    if (cache_dir) s.cache_dir = vitalstream::core::utf8_path(absolute(*cache_dir));
    if (adapters_dir) s.adapters_dir = vitalstream::core::utf8_path(absolute(*adapters_dir));
    if (qc_verbose) s.qc_verbose = true;
    s.finish();
  } catch (const std::exception& e) {
    std::fprintf(stderr, "vitalstream-cpp: %s\n", e.what());
    return 1;
  }
  adapters::set_folders({s.adapters_dir});
  set_limits(s.limits);
  // the readers open no file outside the data roots (a signal file a header names, a link)
  set_allowed_roots(s.data_roots);
  if (s.source)
    std::fprintf(stderr, "vitalstream-cpp: settings from %s\n", vitalstream::core::path_utf8(*s.source).c_str());
  else
    std::fprintf(stderr, "vitalstream-cpp: no settings file (defaults)\n");
  for (const auto& r : s.outside)
    std::fprintf(stderr, "vitalstream-cpp: %s is outside this viewer's root (%s): left out\n", r.c_str(),
                 built_root().c_str());
  std::string auth_path = vitalstream::core::path_utf8(s.auth_file);
#if !VS_PASSWORD
  if (set_pw) {
    std::fprintf(stderr, "vitalstream-cpp: this viewer was built without password support\n");
    return 1;
  }
#else
  if (set_pw) {
    auto pw = prompt_password("New VitalStream password: ");
    if (!pw) return 1;
    if (vitalstream::core::utf8_decode(*pw).size() < 8) {
      std::printf("Use at least 8 characters.\n");
      return 1;
    }
    auto again = prompt_password("Repeat it: ");
    if (!again || *again != *pw) {
      std::printf("The two entries differ; nothing changed.\n");
      return 1;
    }
    try {
      set_password(s.auth_file, *pw);
    } catch (const std::exception& e) {
      std::fprintf(stderr, "vitalstream-cpp: %s: %s\n", auth_path.c_str(), e.what());
      return 1;
    }
    std::printf("Password set (%s). Restart the viewer for it to take effect.\n", auth_path.c_str());
    return 0;
  }
#endif
  if (clear_pw) {
    try {
      clear_password(s.auth_file);
    } catch (const std::exception& e) {
      std::fprintf(stderr, "vitalstream-cpp: %s\n", e.what());
      return 1;
    }
    std::printf("Password removed.\n");
    return 0;
  }
  if (index_path) {
    try {
      std::string p = absolute(*index_path);
      std::string key = build_index(vitalstream::core::utf8_path(p), s.cache_dir, [](double) {});
      std::printf("indexed %s -> %s/%s/\n", index_path->c_str(), vitalstream::core::path_utf8(s.cache_dir).c_str(),
                  key.c_str());
      return 0;
    } catch (const std::exception& e) {
      std::fprintf(stderr, "vitalstream-cpp: %s\n", e.what());
      return 1;
    }
  }
  bool protected_ = !no_auth && load_auth(s.auth_file).has_value();
  if (s.require_password && !protected_) {
    std::fprintf(stderr, "vitalstream-cpp: a password is required here (require_password)%s\n",
                 PASSWORD_SUPPORTED ? "; set one with --set-password"
                                    : ", and this viewer was built without password support");
    return 1;
  }
  std::shared_ptr<App> app;
  try {
    app = make_app(s, protected_);
  } catch (const std::exception& e) {
    std::fprintf(stderr, "vitalstream-cpp: %s\n", e.what());
    return 1;
  }
  httplib::Server svr;
  add_routes(svr, app);
  if (!svr.bind_to_port(s.host, s.port)) {
    std::fprintf(stderr, "vitalstream-cpp: cannot listen on %s:%d\n", s.host.c_str(), s.port);
    return 1;
  }
  // an IPv6 address is written in brackets in a URL
  std::string url_host = s.host.find(':') != std::string::npos ? "[" + s.host + "]" : s.host;
  std::string url = "http://" + url_host + ":" + std::to_string(s.port) + "/";
  std::printf("VitalStream (C++): open %s\n", url.c_str());
  std::string roots_text;
  for (std::size_t i = 0; i < s.data_roots.size(); ++i) roots_text += (i ? ", " : "") + s.data_roots[i];
  std::printf("  root: %s; data roots: %s; cache: %s; time zone %s\n", built_root().c_str(), roots_text.c_str(),
              vitalstream::core::path_utf8(s.cache_dir).c_str(), s.timezone.c_str());
  if (!vitalstream::engine::wfdb_supported())
    std::printf("  built without WFDB support: WFDB records and folders are refused\n");
  if (protected_)
    std::printf("  password protected (vitalstream-cpp --set-password to change it)\n");
  else if (!PASSWORD_SUPPORTED)
    std::printf(
        "  built without password support: anyone who can connect to this port can read the files under its root\n");
  else
    std::printf(
        "  no password: anyone who can connect to this port can read your files through it.\n  On a shared machine, "
        "set one once with: vitalstream-cpp --set-password\n");
  if ((s.host == "0.0.0.0" || s.host == "::") && s.allowed_hosts.empty())
    std::printf(
        "  listening on every address: only requests to localhost are answered until the names\n  this machine is "
        "reached by are listed in allowed_hosts\n");
  std::fflush(stdout);
  if (!no_browser) open_browser(url);
  std::signal(SIGINT, on_signal);
  std::signal(SIGTERM, on_signal);
  std::thread watcher([&svr] {
    while (!g_stop) std::this_thread::sleep_for(std::chrono::milliseconds(200));
    svr.stop();
  });
  bool ok = svr.listen_after_bind();
  g_stop = true;
  watcher.join();
  return ok ? 0 : 1;
}
