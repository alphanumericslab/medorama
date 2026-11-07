// Password protection (auth.hpp), with SHA-256 based primitives (HMAC, PBKDF2, scrypt
// as RFC 2104, RFC 8018 and RFC 7914 define them).
#include "auth.hpp"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <stdexcept>

#ifdef _WIN32
#include <windows.h>
#include <bcrypt.h>
#include <conio.h>
#else
#include <fcntl.h>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>
#if defined(__APPLE__)
#include <sys/random.h>
#endif
#endif

#include "core/paths.hpp"
#include "core/text.hpp"
#include "core/sha256.hpp"
#include "core/value.hpp"

namespace vitalstream::viewer {

namespace fs = std::filesystem;
using core::Value;

std::vector<std::uint8_t> hmac_sha256(const std::vector<std::uint8_t>& key_in, const std::string& message) {
  std::vector<std::uint8_t> key = key_in;
  if (key.size() > 64) {
    core::Sha256 h;
    h.update(key.data(), key.size());
    auto d = h.digest();
    key.assign(d.begin(), d.end());
  }
  key.resize(64, 0);
  std::uint8_t ipad[64], opad[64];
  for (int i = 0; i < 64; ++i) {
    ipad[i] = static_cast<std::uint8_t>(key[i] ^ 0x36);
    opad[i] = static_cast<std::uint8_t>(key[i] ^ 0x5c);
  }
  core::Sha256 inner;
  inner.update(ipad, 64);
  inner.update(message.data(), message.size());
  auto id = inner.digest();
  core::Sha256 outer;
  outer.update(opad, 64);
  outer.update(id.data(), id.size());
  auto d = outer.digest();
  return std::vector<std::uint8_t>(d.begin(), d.end());
}

namespace {

// PBKDF2-HMAC-SHA256 with one iteration (all that scrypt needs).
std::vector<std::uint8_t> pbkdf2_1(const std::string& password, const std::vector<std::uint8_t>& salt,
                                   std::size_t dklen) {
  std::vector<std::uint8_t> key(password.begin(), password.end());
  std::vector<std::uint8_t> out;
  for (std::uint32_t block = 1; out.size() < dklen; ++block) {
    std::string msg(salt.begin(), salt.end());
    msg += static_cast<char>(block >> 24);
    msg += static_cast<char>(block >> 16);
    msg += static_cast<char>(block >> 8);
    msg += static_cast<char>(block);
    auto u = hmac_sha256(key, msg);
    out.insert(out.end(), u.begin(), u.end());
  }
  out.resize(dklen);
  return out;
}

inline std::uint32_t rotl(std::uint32_t x, int n) { return (x << n) | (x >> (32 - n)); }

void salsa20_8(std::uint32_t b[16]) {
  std::uint32_t x[16];
  std::memcpy(x, b, sizeof x);
  for (int i = 0; i < 8; i += 2) {
    x[4] ^= rotl(x[0] + x[12], 7);
    x[8] ^= rotl(x[4] + x[0], 9);
    x[12] ^= rotl(x[8] + x[4], 13);
    x[0] ^= rotl(x[12] + x[8], 18);
    x[9] ^= rotl(x[5] + x[1], 7);
    x[13] ^= rotl(x[9] + x[5], 9);
    x[1] ^= rotl(x[13] + x[9], 13);
    x[5] ^= rotl(x[1] + x[13], 18);
    x[14] ^= rotl(x[10] + x[6], 7);
    x[2] ^= rotl(x[14] + x[10], 9);
    x[6] ^= rotl(x[2] + x[14], 13);
    x[10] ^= rotl(x[6] + x[2], 18);
    x[3] ^= rotl(x[15] + x[11], 7);
    x[7] ^= rotl(x[3] + x[15], 9);
    x[11] ^= rotl(x[7] + x[3], 13);
    x[15] ^= rotl(x[11] + x[7], 18);
    x[1] ^= rotl(x[0] + x[3], 7);
    x[2] ^= rotl(x[1] + x[0], 9);
    x[3] ^= rotl(x[2] + x[1], 13);
    x[0] ^= rotl(x[3] + x[2], 18);
    x[6] ^= rotl(x[5] + x[4], 7);
    x[7] ^= rotl(x[6] + x[5], 9);
    x[4] ^= rotl(x[7] + x[6], 13);
    x[5] ^= rotl(x[4] + x[7], 18);
    x[11] ^= rotl(x[10] + x[9], 7);
    x[8] ^= rotl(x[11] + x[10], 9);
    x[9] ^= rotl(x[8] + x[11], 13);
    x[10] ^= rotl(x[9] + x[8], 18);
    x[12] ^= rotl(x[15] + x[14], 7);
    x[13] ^= rotl(x[12] + x[15], 9);
    x[14] ^= rotl(x[13] + x[12], 13);
    x[15] ^= rotl(x[14] + x[13], 18);
  }
  for (int i = 0; i < 16; ++i) b[i] += x[i];
}

// scryptBlockMix: b has 2r blocks of 16 words; y is scratch of the same size.
void block_mix(std::uint32_t* b, std::uint32_t* y, std::uint32_t r) {
  std::uint32_t x[16];
  std::memcpy(x, b + (2 * r - 1) * 16, 64);
  for (std::uint32_t i = 0; i < 2 * r; ++i) {
    for (int k = 0; k < 16; ++k) x[k] ^= b[i * 16 + k];
    salsa20_8(x);
    // even blocks first, then odd ones
    std::uint32_t dst = (i % 2 == 0) ? (i / 2) : (r + i / 2);
    std::memcpy(y + dst * 16, x, 64);
  }
  std::memcpy(b, y, 128 * r);
}

void ro_mix(std::uint8_t* bytes, std::uint64_t n, std::uint32_t r) {
  const std::size_t words = 32 * r;
  std::vector<std::uint32_t> x(words), y(words), v(static_cast<std::size_t>(n) * words);
  for (std::size_t k = 0; k < words; ++k)
    x[k] = std::uint32_t(bytes[4 * k]) | (std::uint32_t(bytes[4 * k + 1]) << 8) |
           (std::uint32_t(bytes[4 * k + 2]) << 16) | (std::uint32_t(bytes[4 * k + 3]) << 24);
  for (std::uint64_t i = 0; i < n; ++i) {
    std::memcpy(v.data() + i * words, x.data(), words * 4);
    block_mix(x.data(), y.data(), r);
  }
  for (std::uint64_t i = 0; i < n; ++i) {
    std::uint64_t j = x[(2 * r - 1) * 16] & (n - 1);  // Integerify (n is a power of 2 below 2^32)
    for (std::size_t k = 0; k < words; ++k) x[k] ^= v[j * words + k];
    block_mix(x.data(), y.data(), r);
  }
  for (std::size_t k = 0; k < words; ++k) {
    bytes[4 * k] = static_cast<std::uint8_t>(x[k]);
    bytes[4 * k + 1] = static_cast<std::uint8_t>(x[k] >> 8);
    bytes[4 * k + 2] = static_cast<std::uint8_t>(x[k] >> 16);
    bytes[4 * k + 3] = static_cast<std::uint8_t>(x[k] >> 24);
  }
}

std::optional<std::vector<std::uint8_t>> unhex(const std::string& s) {
  if (s.size() % 2) return std::nullopt;
  std::vector<std::uint8_t> out;
  auto nib = [](char c) -> int {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
  };
  for (std::size_t i = 0; i < s.size(); i += 2) {
    int a = nib(s[i]), b = nib(s[i + 1]);
    if (a < 0 || b < 0) return std::nullopt;
    out.push_back(static_cast<std::uint8_t>(a * 16 + b));
  }
  return out;
}

std::string hexs(const std::vector<std::uint8_t>& b) { return core::hex(b.data(), b.size()); }

std::string scrypt_hex(const std::string& password, const std::vector<std::uint8_t>& salt) {
  return hexs(scrypt(password, salt, 1u << 14, 8, 1, 32));
}

bool eq_ct(const std::string& a, const std::string& b) {
  if (a.size() != b.size()) return false;
  unsigned char acc = 0;
  for (std::size_t i = 0; i < a.size(); ++i) acc |= static_cast<unsigned char>(a[i] ^ b[i]);
  return acc == 0;
}

std::uint64_t now() {
  return static_cast<std::uint64_t>(
      std::chrono::duration_cast<std::chrono::seconds>(std::chrono::system_clock::now().time_since_epoch()).count());
}

std::string sign(const Auth& auth, std::uint64_t expires) { return hexs(hmac_sha256(auth.secret, std::to_string(expires))); }

}  // namespace

std::vector<std::uint8_t> scrypt(const std::string& password, const std::vector<std::uint8_t>& salt, std::uint64_t n,
                                 std::uint32_t r, std::uint32_t p, std::size_t dklen) {
  if (n < 2 || (n & (n - 1)) != 0) throw std::invalid_argument("scrypt: N must be a power of 2");
  std::vector<std::uint8_t> b = pbkdf2_1(password, salt, static_cast<std::size_t>(p) * 128 * r);
  for (std::uint32_t i = 0; i < p; ++i) ro_mix(b.data() + static_cast<std::size_t>(i) * 128 * r, n, r);
  std::string bs(b.begin(), b.end());
  std::vector<std::uint8_t> bsalt(bs.begin(), bs.end());
  return pbkdf2_1(password, bsalt, dklen);
}

std::vector<std::uint8_t> random_bytes(std::size_t n) {
  std::vector<std::uint8_t> b(n);
#ifdef _WIN32
  if (BCryptGenRandom(nullptr, b.data(), static_cast<ULONG>(n), BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0)
    throw std::runtime_error("system random number generator failed");
#else
  for (std::size_t off = 0; off < n;) {
    std::size_t k = std::min<std::size_t>(256, n - off);
    if (getentropy(b.data() + off, k) != 0) throw std::runtime_error("system random number generator failed");
    off += k;
  }
#endif
  return b;
}

std::optional<Auth> load_auth(const fs::path& path) {
  std::FILE* f = core::open_file(core::path_utf8(path), "rb");
  if (!f) return std::nullopt;
  std::string text;
  char buf[4096];
  std::size_t n;
  while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) text.append(buf, n);
  std::fclose(f);
  try {
    Value v = Value::from_json(text);
    auto s = [&](const char* k) -> std::optional<std::string> {
      const Value* x = v.get(k);
      if (!x || !x->as_str()) return std::nullopt;
      return *x->as_str();
    };
    auto hash = s("hash"), secret = s("secret"), salt = s("salt");
    if (!hash || !secret || !salt || hash->empty() || secret->empty()) return std::nullopt;
    auto salt_b = unhex(*salt), secret_b = unhex(*secret);
    if (!salt_b || !secret_b) return std::nullopt;
    return Auth{*salt_b, *hash, *secret_b};
  } catch (const std::exception&) {
    return std::nullopt;
  }
}

namespace {

std::string read_file(const fs::path& path) {
  std::FILE* f = core::open_file(core::path_utf8(path), "rb");
  if (!f) throw std::runtime_error(core::path_utf8(path) + ": cannot open");
  std::string text;
  char buf[4096];
  std::size_t n;
  while ((n = std::fread(buf, 1, sizeof buf, f)) > 0) text.append(buf, n);
  std::fclose(f);
  return text;
}

// Write text to path readable by its owner only, replacing it at once.
void write_private(const fs::path& path, const std::string& text) {
  fs::path tmp = path;
  tmp.replace_extension("json.tmp");
#ifdef _WIN32
  std::FILE* f = core::open_file(core::path_utf8(tmp), "wb");
#else
  int fd = ::open(tmp.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0600);
  std::FILE* f = fd < 0 ? nullptr : ::fdopen(fd, "wb");
#endif
  if (!f) throw std::runtime_error(core::path_utf8(tmp) + ": cannot create file");
  bool ok = std::fwrite(text.data(), 1, text.size(), f) == text.size();
  ok = std::fclose(f) == 0 && ok;
  if (!ok) throw std::runtime_error(core::path_utf8(tmp) + ": write failed");
  fs::rename(tmp, path);
#ifndef _WIN32
  ::chmod(path.c_str(), 0600);
#endif
}

}  // namespace

void set_password(const fs::path& path, const std::string& password) {
  fs::path dir = path.parent_path();
  if (!dir.empty()) {
    fs::create_directories(dir);
#ifndef _WIN32
    ::chmod(dir.c_str(), 0700);
#endif
  }
  auto salt = random_bytes(16);
  Value data(core::Dict{{"scheme", Value("scrypt")},
                       {"salt", Value(hexs(salt))},
                       {"hash", Value(scrypt_hex(password, salt))},
                       {"secret", Value(hexs(random_bytes(32)))}});
  write_private(path, data.to_json());
}

std::map<std::string, std::uint64_t> revoked(const fs::path& path) {
  std::map<std::string, std::uint64_t> out;
  try {
    Value v = Value::from_json(read_file(path));
    const Value* r = v.get("revoked");
    if (!r || !r->as_dict()) return out;
    std::uint64_t t = now();
    for (const auto& e : *r->as_dict()) {
      std::int64_t x;
      if (e.value.as_int(x) && x > 0 && static_cast<std::uint64_t>(x) > t) out[e.key] = static_cast<std::uint64_t>(x);
    }
  } catch (const std::exception&) {
  }
  return out;
}

void save_revoked(const fs::path& path, const std::map<std::string, std::uint64_t>& all) {
  Value v = Value::from_json(read_file(path));
  core::Dict* d = v.as_dict();
  if (!d) throw std::runtime_error(core::path_utf8(path) + ": not an auth file");
  std::uint64_t t = now();
  core::Dict kept;  // in order of the signatures
  for (const auto& [sig, expiry] : all)
    if (expiry > t) kept.insert(sig, Value(static_cast<std::int64_t>(expiry)));
  d->insert("revoked", Value(std::move(kept)));
  write_private(path, v.to_json());
}

void clear_password(const fs::path& path) {
  std::error_code ec;
  fs::remove(path, ec);
  if (ec && ec != std::errc::no_such_file_or_directory) throw std::runtime_error(core::path_utf8(path) + ": " + ec.message());
}

bool check_password(const Auth& auth, const std::string& password) {
  return eq_ct(scrypt_hex(password, auth.salt), auth.hash);
}

std::string make_cookie(const Auth& auth) {
  std::uint64_t expires = now() + LOGIN_DAYS * 86400;
  return std::to_string(expires) + "." + sign(auth, expires);
}

bool cookie_ok(const Auth& auth, const std::optional<std::string>& value) {
  if (!value) return false;
  std::size_t dot = value->find('.');
  if (dot == std::string::npos || dot == 0) return false;
  std::string e = value->substr(0, dot), sig = value->substr(dot + 1);
  std::uint64_t expires = 0;
  for (char c : e) {
    if (c < '0' || c > '9' || expires > (UINT64_MAX - 9) / 10) return false;
    expires = expires * 10 + static_cast<std::uint64_t>(c - '0');
  }
  return expires > now() && eq_ct(sig, sign(auth, expires));
}

std::optional<std::string> prompt_password(const char* prompt) {
  std::fputs(prompt, stderr);
  std::fflush(stderr);
  std::string line;
#ifdef _WIN32
  HANDLE h = GetStdHandle(STD_INPUT_HANDLE);
  DWORD mode = 0;
  bool console = GetConsoleMode(h, &mode);
  if (console) SetConsoleMode(h, mode & ~ENABLE_ECHO_INPUT);
  bool ok = static_cast<bool>(std::getline(std::cin, line));
  if (console) SetConsoleMode(h, mode);
#else
  termios old{};
  bool tty = isatty(STDIN_FILENO) && tcgetattr(STDIN_FILENO, &old) == 0;
  if (tty) {
    termios t = old;
    t.c_lflag &= static_cast<tcflag_t>(~ECHO);
    tcsetattr(STDIN_FILENO, TCSAFLUSH, &t);
  }
  bool ok = static_cast<bool>(std::getline(std::cin, line));
  if (tty) tcsetattr(STDIN_FILENO, TCSAFLUSH, &old);
#endif
  std::fputc('\n', stderr);
  if (!ok) return std::nullopt;
  if (!line.empty() && line.back() == '\r') line.pop_back();
  return line;
}

}  // namespace vitalstream::viewer
