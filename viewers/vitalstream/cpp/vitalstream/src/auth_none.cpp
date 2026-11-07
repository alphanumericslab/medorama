// A build without password support (CMake option VS_PASSWORD=OFF; the Rust viewer's
// auth_none.rs): no password can be set, the page never asks for one, and the viewer refuses to
// start where a password is required (require_password, which Medorama sets for the viewer it
// runs).  The same functions as auth.cpp, so the server is the same.
#include <stdexcept>

#include "auth.hpp"

namespace vitalstream::viewer {

namespace {

const char* const NONE = "this viewer was built without password support";

}  // namespace

std::optional<Auth> load_auth(const std::filesystem::path&) { return std::nullopt; }

void set_password(const std::filesystem::path&, const std::string&) { throw std::runtime_error(NONE); }

void clear_password(const std::filesystem::path&) { throw std::runtime_error(NONE); }

bool check_password(const Auth&, const std::string&) { return false; }

std::string make_cookie(const Auth&) { return std::string(); }

bool cookie_ok(const Auth&, const std::optional<std::string>&) { return false; }

std::map<std::string, std::uint64_t> revoked(const std::filesystem::path&) { return {}; }

void save_revoked(const std::filesystem::path&, const std::map<std::string, std::uint64_t>&) {
  throw std::runtime_error(NONE);
}

}  // namespace vitalstream::viewer
