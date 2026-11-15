// HTTP API (version 1, under api/v1/; docs/viewer-api.md) and the web page (as the
// Rust viewer's server.rs).
//
// Access: when a password is set, every API request needs the sign-in cookie that POST login
// sets.  The page uses relative URLs, so it also works behind a reverse proxy that serves it
// under a path prefix.  Only paths allowed by the settings are served.  Patient names and dates
// of birth are never read by any source.
#pragma once

#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include "engine/store.hpp"
#include "settings.hpp"

namespace httplib {
class Server;
}

namespace vitalstream::viewer {

class App;

// The index stores a viewer reads, and the one it writes (viewers/vitalstream/docs/index-format.md,
// "Version 3: the index store"): those given on standard input (--keys-on-stdin: Medorama's
// databases'), looked in first, and the viewer's own, where it writes.  The keys are kept in
// memory only, and never written or logged.
class Stores {
 public:
  // The viewer's own store: cache_dir/index.sqlite, encrypted with the key in index_key_file when
  // one is set.  Throws std::runtime_error.
  static std::shared_ptr<Stores> own_from(const Settings& settings);
  // One line of the pipe: {"store": PATH, "key": HEX} (with "own": true, the store the viewer
  // writes), or {"forget": PATH}.  Throws std::runtime_error (never with the line's text).
  void apply(const std::string& line);
  // Forget every store (standard input closed).
  void forget_all();
  // The store holding the index of key, if any.
  std::shared_ptr<engine::Store> holding(const std::string& key) const;
  // The paths of every store held.
  std::vector<std::string> paths() const;
  // The store the viewer writes, if any.
  std::shared_ptr<engine::Store> own() const;

 private:
  mutable std::mutex m_;
  std::vector<std::shared_ptr<engine::Store>> given_;
  std::shared_ptr<engine::Store> own_;
};

// The application state behind the routes, with its own index store (Stores::own_from).
// protected_: nothing = protected when a password is set.  Throws std::runtime_error when
// protection is requested but no password is set, or when the store cannot be opened.
std::shared_ptr<App> make_app(const Settings& settings, std::optional<bool> protected_);
// make_app with the stores given (launched by Medorama: an empty Stores, filled by apply).
std::shared_ptr<App> make_app(const Settings& settings, std::optional<bool> protected_,
                              std::shared_ptr<Stores> stores);
bool app_protected(const App& app);

// Register every route of the viewer on a server.
void add_routes(httplib::Server& server, std::shared_ptr<App> app);

}  // namespace vitalstream::viewer
