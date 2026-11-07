// HTTP API (version 1, under api/v1/; docs/viewer-api.md) and the web page (as the
// Rust viewer's server.rs).
//
// Access: when a password is set, every API request needs the sign-in cookie that POST login
// sets.  The page uses relative URLs, so it also works behind a reverse proxy that serves it
// under a path prefix.  Only paths allowed by the settings are served.  Patient names and dates
// of birth are never read by any source.
#pragma once

#include <memory>
#include <optional>
#include <string>

#include "settings.hpp"

namespace httplib {
class Server;
}

namespace vitalstream::viewer {

class App;

// The application state behind the routes.  protected_: nothing = protected when a password is
// set.  Throws std::runtime_error when protection is requested but no password is set.
std::shared_ptr<App> make_app(const Settings& settings, std::optional<bool> protected_);
bool app_protected(const App& app);

// Register every route of the viewer on a server.
void add_routes(httplib::Server& server, std::shared_ptr<App> app);

}  // namespace vitalstream::viewer
