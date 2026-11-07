// Optional password protection, with the same scheme and file as the Python and Rust viewers
// (as the Rust viewer's auth.rs): a salted scrypt hash (N = 2^14, r = 8, p = 1, 32
// bytes) and a random signing key in an auth file readable only by its owner (default
// ~/.config/vitalstream/auth.json).  The login is a cookie <expiry>.<HMAC-SHA256(key, expiry)>
// valid for LOGIN_DAYS.  A password set with any of the viewers works for all of them.  The
// sign-ins ended by logout are kept in the auth file (revoked: {signature: expiry}) until they
// would have expired, so that a copy of a cookie stops working.
//
// Everything about passwords lives here, behind these few functions, so that the scheme can be
// replaced (e.g. by an external authentication service) without touching the server.  A build
// without password support (CMake option VS_PASSWORD=OFF, as the Rust viewer's auth_none.rs)
// compiles auth_none.cpp instead of auth.cpp: the same functions, but no password can be set.
#pragma once

#include <cstdint>
#include <filesystem>
#include <map>
#include <optional>
#include <string>
#include <vector>

namespace vitalstream::viewer {

constexpr std::uint64_t LOGIN_DAYS = 30;
// Whether this build can protect the viewer with a password (VS_PASSWORD).
#if VS_PASSWORD
constexpr bool PASSWORD_SUPPORTED = true;
#else
constexpr bool PASSWORD_SUPPORTED = false;
#endif

struct Auth {
  std::vector<std::uint8_t> salt;
  std::string hash;
  std::vector<std::uint8_t> secret;
};

// The auth file's contents, or nothing when no password is set.
std::optional<Auth> load_auth(const std::filesystem::path& path);
// Set (or change) the password; a new signing key ends earlier logins.  Throws.
void set_password(const std::filesystem::path& path, const std::string& password);
void clear_password(const std::filesystem::path& path);  // throws (a missing file is fine)
bool check_password(const Auth& auth, const std::string& password);
std::string make_cookie(const Auth& auth);
bool cookie_ok(const Auth& auth, const std::optional<std::string>& value);
// The sign-ins ended by logout that have not expired (signature -> expiry).
std::map<std::string, std::uint64_t> revoked(const std::filesystem::path& path);
// Write the sign-ins ended by logout into the auth file (the expired ones left out), its other
// keys unchanged.  Throws.
void save_revoked(const std::filesystem::path& path, const std::map<std::string, std::uint64_t>& all);

#if VS_PASSWORD
// Read a password from the terminal without echoing it; nothing at end of input.
std::optional<std::string> prompt_password(const char* prompt);

// Primitives (exposed for the tests).
std::vector<std::uint8_t> hmac_sha256(const std::vector<std::uint8_t>& key, const std::string& message);
std::vector<std::uint8_t> scrypt(const std::string& password, const std::vector<std::uint8_t>& salt, std::uint64_t n,
                                 std::uint32_t r, std::uint32_t p, std::size_t dklen);
std::vector<std::uint8_t> random_bytes(std::size_t n);  // from the operating system
#endif

}  // namespace vitalstream::viewer
