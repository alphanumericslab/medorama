// Adapters (adapters.hpp): finding them, running their commands, and their `serve` sessions.
#include "engine/adapters.hpp"

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <thread>

#include "core/paths.hpp"
#include "core/text.hpp"
#include "core/value.hpp"
#include "engine/common.hpp"

#ifdef _WIN32
#include <windows.h>
#else
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
extern char** environ;
#endif

namespace vitalstream::engine::adapters {

using core::Dict;
using core::List;
using core::Value;

namespace {

// --- a child process with pipes --------------------------------------------------------------

// The longest line of an adapter's answer read (longer lines are a protocol error), and the most an
// answer of `describe` may hold.
constexpr std::size_t MAX_LINE = 1 << 20;
constexpr std::size_t MAX_OUTPUT = 16 << 20;
// The bytes of standard output read ahead of the reader (an adapter that writes more waits).
constexpr std::size_t READ_AHEAD = 4 << 20;

using Clock = std::chrono::steady_clock;

Clock::time_point after(double seconds) {
  return Clock::now() + std::chrono::duration_cast<Clock::duration>(std::chrono::duration<double>(seconds));
}

// Seconds as Rust prints them ("30", "0.5").
std::string secs(double s) {
  char buf[64];
  std::snprintf(buf, sizeof buf, "%.17g", s);
  return buf;
}

// Bytes a pipe delivered, read by a thread of its own, so that the reader can wait with a time
// limit.  `keep` > 0: only the last `keep` bytes are kept, and the writer never waits (standard
// error); otherwise the thread waits while READ_AHEAD bytes are not taken.
struct Stream {
  std::mutex m;
  std::condition_variable cv;
  std::string buf;
  bool eof = false;
  std::size_t keep = 0;
};

#ifdef _WIN32
using Pipe = HANDLE;
std::size_t read_pipe(Pipe p, char* out, std::size_t n) {
  DWORD got = 0;
  if (!ReadFile(p, out, static_cast<DWORD>(n), &got, nullptr)) return 0;
  return got;
}
void close_pipe(Pipe p) { CloseHandle(p); }
#else
using Pipe = int;
std::size_t read_pipe(Pipe p, char* out, std::size_t n) {
  for (;;) {
    ssize_t got = ::read(p, out, n);
    if (got < 0 && errno == EINTR) continue;
    return got > 0 ? static_cast<std::size_t>(got) : 0;
  }
}
void close_pipe(Pipe p) { ::close(p); }
#endif

// Start the thread that reads pipe `p` into `s` until its end (it then closes the pipe).
void drain(Pipe p, std::shared_ptr<Stream> s) {
  std::thread([p, s] {
    std::vector<char> tmp(1 << 16);
    for (;;) {
      std::size_t n = read_pipe(p, tmp.data(), tmp.size());
      std::unique_lock<std::mutex> l(s->m);
      if (n == 0) {
        s->eof = true;
        s->cv.notify_all();
        break;
      }
      s->buf.append(tmp.data(), n);
      if (s->keep && s->buf.size() > s->keep) s->buf.erase(0, s->buf.size() - s->keep);
      s->cv.notify_all();
      if (!s->keep) s->cv.wait(l, [&] { return s->buf.size() < READ_AHEAD; });
    }
    close_pipe(p);
  }).detach();
}

// The last line of `text` that is not blank, at most 200 characters, control characters removed:
// for an error message (never printed by the viewer).
std::string last_line(const std::string& raw) {
  std::string text = core::utf8_valid(raw) ? raw : [&] {
    std::string fixed;
    for (char32_t c : core::utf8_decode(raw)) core::utf8_append(fixed, c);
    return fixed;
  }();
  std::string line;
  std::size_t a = 0;
  while (a <= text.size()) {
    std::size_t b = text.find('\n', a);
    if (b == std::string::npos) b = text.size();
    std::string l = trim(text.substr(a, b - a));
    if (!l.empty()) line = l;
    a = b + 1;
  }
  std::string out;
  std::size_t k = 0;
  for (char32_t c : core::utf8_decode(line)) {
    if (c < 0x20 || (c >= 0x7f && c <= 0x9f)) continue;
    if (k++ == 200) break;
    core::utf8_append(out, c);
  }
  return out;
}

// Whether a child gets the environment variable `name`.
bool env_allowed(std::string name) {
#ifdef _WIN32
  for (char& c : name) c = static_cast<char>(std::toupper(static_cast<unsigned char>(c)));
#endif
  if (name.rfind("LC_", 0) == 0) return true;
  return std::any_of(std::begin(ENV_ALLOWED), std::end(ENV_ALLOWED), [&](const char* x) { return name == x; });
}

// What the program is called in messages (its file name).
std::string name_of(const fs::path& program) { return core::path_utf8(program.filename()); }

// A program run with its standard output and error as pipes, and its standard input a pipe or
// nothing; with the allowed environment only.
class Child {
 public:
  Child(const fs::path& program, const std::vector<std::string>& args, bool pipe_in);
  ~Child() { kill(); }
  Child(const Child&) = delete;
  Child& operator=(const Child&) = delete;

  bool write_all(const std::string& s);
  // More output, waiting until `deadline`: false on timeout.  With nothing more to come, true
  // and `eof()`.
  bool wait_output(Clock::time_point deadline);
  // Bytes of output read and not taken (0: none so far).
  std::size_t pending();
  // Take every byte read so far.
  std::string take();
  bool eof();
  // The exit status once it has exited, waiting until `deadline` (it is killed after it).
  std::optional<int> wait_until(Clock::time_point deadline);
  bool running();
  void kill();  // stop it at once, and collect it
  std::string stderr_line() {
    std::lock_guard<std::mutex> l(err_->m);
    return last_line(err_->buf);
  }

 private:
  std::shared_ptr<Stream> out_ = std::make_shared<Stream>();
  std::shared_ptr<Stream> err_ = std::make_shared<Stream>();
#ifdef _WIN32
  HANDLE process_ = nullptr, in_ = nullptr;
#else
  pid_t pid_ = -1;
  int in_ = -1;
#endif
  bool waited_ = false;
  int status_ = -1;
};

bool Child::wait_output(Clock::time_point deadline) {
  std::unique_lock<std::mutex> l(out_->m);
  return out_->cv.wait_until(l, deadline, [&] { return !out_->buf.empty() || out_->eof; });
}

std::size_t Child::pending() {
  std::lock_guard<std::mutex> l(out_->m);
  return out_->buf.size();
}

std::string Child::take() {
  std::string b;
  {
    std::lock_guard<std::mutex> l(out_->m);
    b.swap(out_->buf);
  }
  out_->cv.notify_all();
  return b;
}

bool Child::eof() {
  std::lock_guard<std::mutex> l(out_->m);
  return out_->eof && out_->buf.empty();
}

std::optional<int> Child::wait_until(Clock::time_point deadline) {
  while (running()) {
    if (Clock::now() >= deadline) {
      kill();
      return std::nullopt;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  return status_;
}

#ifdef _WIN32
std::wstring wide(const std::string& s) {
  int n = MultiByteToWideChar(CP_UTF8, 0, s.c_str(), -1, nullptr, 0);
  std::wstring w(static_cast<std::size_t>(n > 0 ? n : 1), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, s.c_str(), -1, w.data(), n);
  w.resize(std::wcslen(w.c_str()));
  return w;
}

// One argument quoted as CommandLineToArgvW reads it back.
std::wstring quote(const std::wstring& a) {
  if (!a.empty() && a.find_first_of(L" \t\"") == std::wstring::npos) return a;
  std::wstring out = L"\"";
  std::size_t slashes = 0;
  for (wchar_t c : a) {
    if (c == L'\\') {
      ++slashes;
    } else if (c == L'"') {
      out.append(slashes * 2 + 1, L'\\');
      out += c;
      slashes = 0;
    } else {
      out.append(slashes, L'\\');
      out += c;
      slashes = 0;
    }
  }
  out.append(slashes * 2, L'\\');
  return out + L"\"";
}

// The allowed variables of this process's environment, as an environment block.
std::wstring environment_block() {
  std::wstring block;
  if (wchar_t* env = GetEnvironmentStringsW()) {
    for (const wchar_t* p = env; *p; p += std::wcslen(p) + 1) {
      std::wstring entry(p);
      std::size_t eq = entry.find(L'=', 1);  // "=C:=C:\..." entries start with '='
      if (eq == std::wstring::npos) continue;
      std::string name = core::path_utf8(fs::path(entry.substr(0, eq)));
      if (env_allowed(name)) block += entry + L'\0';
    }
    FreeEnvironmentStringsW(env);
  }
  block += L'\0';
  if (block.size() == 1) block += L'\0';
  return block;
}

Child::Child(const fs::path& program, const std::vector<std::string>& args, bool pipe_in) {
  SECURITY_ATTRIBUTES sa{sizeof sa, nullptr, TRUE};
  HANDLE child_in = nullptr, child_out = nullptr, child_err = nullptr, out = nullptr, err = nullptr;
  if (pipe_in) {
    if (!CreatePipe(&child_in, &in_, &sa, 0)) throw std::runtime_error("cannot create a pipe");
    SetHandleInformation(in_, HANDLE_FLAG_INHERIT, 0);
  } else {
    child_in = CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ, &sa, OPEN_EXISTING, 0, nullptr);
  }
  if (!CreatePipe(&out, &child_out, &sa, 0) || !CreatePipe(&err, &child_err, &sa, 0))
    throw std::runtime_error("cannot create a pipe");
  SetHandleInformation(out, HANDLE_FLAG_INHERIT, 0);
  SetHandleInformation(err, HANDLE_FLAG_INHERIT, 0);
  std::wstring cmd = quote(program.wstring());
  for (const auto& a : args) cmd += L" " + quote(wide(a));
  std::wstring env = environment_block();
  STARTUPINFOW si{};
  si.cb = sizeof si;
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = child_in;
  si.hStdOutput = child_out;
  si.hStdError = child_err;
  PROCESS_INFORMATION pi{};
  BOOL ok = CreateProcessW(program.wstring().c_str(), cmd.data(), nullptr, nullptr, TRUE,
                           CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT, env.data(), nullptr, &si, &pi);
  if (child_in) CloseHandle(child_in);
  CloseHandle(child_out);
  CloseHandle(child_err);
  if (!ok) {
    CloseHandle(out);
    CloseHandle(err);
    if (in_) CloseHandle(in_);
    in_ = nullptr;
    throw std::runtime_error(core::path_utf8(program) + ": cannot start");
  }
  CloseHandle(pi.hThread);
  process_ = pi.hProcess;
  err_->keep = STDERR_KEPT;
  drain(out, out_);
  drain(err, err_);
}

bool Child::write_all(const std::string& s) {
  const char* p = s.data();
  std::size_t left = s.size();
  while (left) {
    DWORD n = 0;
    if (!in_ || !WriteFile(in_, p, static_cast<DWORD>(std::min<std::size_t>(left, 1 << 20)), &n, nullptr) || n == 0)
      return false;
    p += n;
    left -= n;
  }
  return true;
}

bool Child::running() {
  if (waited_ || !process_) return false;
  if (WaitForSingleObject(process_, 0) == WAIT_TIMEOUT) return true;
  DWORD code = 1;
  GetExitCodeProcess(process_, &code);
  status_ = static_cast<int>(code);
  waited_ = true;
  return false;
}

void Child::kill() {
  if (in_) CloseHandle(in_);
  in_ = nullptr;
  if (process_) {
    if (!waited_) {
      TerminateProcess(process_, 1);
      WaitForSingleObject(process_, INFINITE);
      running();
    }
    CloseHandle(process_);
    process_ = nullptr;
  }
  take();  // the reading thread waits for no one
}
#else
Child::Child(const fs::path& program, const std::vector<std::string>& args, bool pipe_in) {
  // writing to an adapter that has ended must be an error, not the end of this program
  static std::once_flag no_sigpipe;
  std::call_once(no_sigpipe, [] { std::signal(SIGPIPE, SIG_IGN); });
  int pin[2] = {-1, -1}, pout[2] = {-1, -1}, perr[2] = {-1, -1};
  if ((pipe_in && ::pipe(pin) != 0) || ::pipe(pout) != 0 || ::pipe(perr) != 0)
    throw std::runtime_error("cannot create a pipe");
  for (int fd : {pin[1], pout[0], perr[0]})
    if (fd >= 0) ::fcntl(fd, F_SETFD, FD_CLOEXEC);  // not inherited by other children
  posix_spawn_file_actions_t fa;
  posix_spawn_file_actions_init(&fa);
  if (pipe_in) {
    posix_spawn_file_actions_adddup2(&fa, pin[0], 0);
  } else {
    posix_spawn_file_actions_addopen(&fa, 0, "/dev/null", O_RDONLY, 0);
  }
  posix_spawn_file_actions_adddup2(&fa, pout[1], 1);
  posix_spawn_file_actions_adddup2(&fa, perr[1], 2);
  std::string prog = core::path_utf8(program);
  std::vector<std::string> argv_s{prog};
  argv_s.insert(argv_s.end(), args.begin(), args.end());
  std::vector<char*> argv;
  for (auto& a : argv_s) argv.push_back(a.data());
  argv.push_back(nullptr);
  std::vector<char*> envp;
  for (char** e = environ; *e; ++e) {
    const char* eq = std::strchr(*e, '=');
    if (eq && env_allowed(std::string(*e, static_cast<std::size_t>(eq - *e)))) envp.push_back(*e);
  }
  envp.push_back(nullptr);
  int rc = posix_spawn(&pid_, prog.c_str(), &fa, nullptr, argv.data(), envp.data());
  posix_spawn_file_actions_destroy(&fa);
  for (int fd : {pin[0], pout[1], perr[1]})
    if (fd >= 0) ::close(fd);
  in_ = pin[1];
  if (rc != 0) {
    for (int fd : {pin[1], pout[0], perr[0]})
      if (fd >= 0) ::close(fd);
    in_ = -1;
    pid_ = -1;
    throw std::runtime_error(prog + ": cannot start: " + std::strerror(rc));
  }
  err_->keep = STDERR_KEPT;
  drain(pout[0], out_);
  drain(perr[0], err_);
}

bool Child::write_all(const std::string& s) {
  const char* p = s.data();
  std::size_t left = s.size();
  while (left && in_ >= 0) {
    ssize_t n = ::write(in_, p, left);
    if (n < 0 && errno == EINTR) continue;
    if (n <= 0) return false;
    p += n;
    left -= static_cast<std::size_t>(n);
  }
  return left == 0;
}

bool Child::running() {
  if (waited_ || pid_ <= 0) return false;
  int st = 0;
  pid_t r = ::waitpid(pid_, &st, WNOHANG);
  if (r == pid_) {
    waited_ = true;
    status_ = WIFEXITED(st) ? WEXITSTATUS(st) : 128 + (WIFSIGNALED(st) ? WTERMSIG(st) : 0);
    return false;
  }
  return r == 0;
}

void Child::kill() {
  if (in_ >= 0) ::close(in_);
  in_ = -1;
  if (pid_ > 0 && !waited_) {
    ::kill(pid_, SIGKILL);
    int st = 0;
    while (::waitpid(pid_, &st, 0) < 0 && errno == EINTR) {
    }
    waited_ = true;
    status_ = 128 + SIGKILL;
  }
  take();  // the reading thread waits for no one
}
#endif

// --- the protocol --------------------------------------------------------------------------

std::mutex mu;
std::vector<fs::path> folders_ = {default_folder()};
std::optional<std::vector<Adapter>> found_;
std::mutex timeouts_mu;
Timeouts timeouts_;

Dict json_object(const std::string& line) {
  Value v = Value::from_json(line);
  const Dict* d = v.as_dict();
  if (!d) throw std::runtime_error("an adapter answered with something other than a JSON object");
  return *d;
}

std::string text(const Dict& d, const char* k) {
  const Value* v = d.get(k);
  return v && v->as_str() ? *v->as_str() : std::string();
}

std::string exit_text(int status) { return "exit status: " + std::to_string(status); }

// Run `program` with `args` within `limit` seconds; its standard output.  The error says why, with
// the last line of its standard error.
std::string run(const fs::path& program, const std::vector<std::string>& args, double limit) {
  const auto deadline = after(limit);
  const std::string name = name_of(program);
  Child c(program, args, false);
  std::string out;
  for (;;) {
    if (!c.wait_output(deadline)) {
      c.kill();
      throw std::runtime_error(name + " did not answer within " + secs(limit) + " s: stopped");
    }
    out += c.take();
    if (out.size() > MAX_OUTPUT) {
      c.kill();
      throw std::runtime_error(name + " answered with more than 16 MiB");
    }
    if (c.eof()) break;
  }
  auto status = c.wait_until(deadline);
  if (!status) throw std::runtime_error(name + " did not exit within " + secs(limit) + " s: stopped");
  if (*status != 0) {
    std::string why = c.stderr_line();
    throw std::runtime_error(name + " failed (" + exit_text(*status) + ")" + (why.empty() ? "" : ": " + why));
  }
  return out;
}

Adapter describe_program(const fs::path& program) {
  Dict d = json_object(run(program, {"describe"}, timeouts().describe));
  std::int64_t protocol = 0;
  if (const Value* p = d.get("protocol")) p->as_int(protocol);
  if (protocol != PROTOCOL)
    throw std::runtime_error("speaks protocol " + std::to_string(protocol) + "; this program speaks " +
                             std::to_string(PROTOCOL));
  Adapter a{text(d, "name"), text(d, "version"), program, {}};
  if (const Value* fv = d.get("formats"); fv && fv->as_list()) {
    for (const Value& f : *fv->as_list()) {
      const Dict* fd = f.as_dict();
      if (!fd) continue;
      Format fmt{text(*fd, "kind"), text(*fd, "label"), {}};
      if (const Value* ev = fd->get("extensions"); ev && ev->as_list())
        for (const Value& e : *ev->as_list())
          if (const std::string* s = e.as_str()) {
            std::string x = *s;
            if (!x.empty() && x[0] == '.') x.erase(0, 1);
            for (char& c : x) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
            fmt.extensions.push_back(x);
          }
      if (!fmt.kind.empty()) a.formats.push_back(std::move(fmt));
    }
  }
  return a;
}

std::string lower_extension(const fs::path& p) {
  std::string e = core::path_utf8(p.extension());
  if (!e.empty() && e[0] == '.') e.erase(0, 1);
  for (char& c : e) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  return e;
}

bool is_adapter_name(const std::string& n) {
  std::string l = n;
  for (char& c : l) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  auto ends = [&](const std::string& s) { return l.size() >= s.size() && l.compare(l.size() - s.size(), s.size(), s) == 0; };
  return ends("-adapter") || ends("-adapter.exe");
}

// Why a session's answer was not samples.
enum class Failure {
  None,
  Answer,    // the adapter answered {"error": ...}: the session stays in step
  Ended,     // the session ended before answering
  Protocol,  // a malformed answer, or none in time: the session is out of step
};

// A running `serve` session.
class Session {
 public:
  explicit Session(const fs::path& program) : child_(program, {"serve"}, true) {}
  bool ended() { return !child_.running(); }

  // The samples answered to `request`, at most `want` of them; else why not (in `why`).
  Failure read(const std::string& request, std::size_t want, std::vector<double>& out, std::string& why) {
    if (!buf_.empty() || child_.pending()) {
      why = "the adapter wrote more than it was asked for";
      return Failure::Protocol;
    }
    if (!child_.write_all(request + "\n")) return Failure::Ended;
    std::string line;
    for (;;) {
      std::size_t nl = buf_.find('\n');
      if (nl != std::string::npos) {
        line = buf_.substr(0, nl);
        buf_.erase(0, nl + 1);
        break;
      }
      if (buf_.size() > MAX_LINE) {
        why = "an answer line longer than 1 MiB";
        return Failure::Protocol;
      }
      if (Failure f = more(why); f != Failure::None) return f == Failure::Ended && !buf_.empty() ? inside(why) : f;
    }
    Dict d;
    try {
      d = json_object(line);
    } catch (const std::exception& e) {
      why = e.what();
      return Failure::Protocol;
    }
    if (const Value* e = d.get("error"); e && e->as_str()) {
      why = *e->as_str();
      return Failure::Answer;
    }
    std::int64_t n = -1;
    const Value* nv = d.get("n");
    if (!nv || !nv->as_int(n) || n < 0 || static_cast<std::uint64_t>(n) > want) {
      why = "an answer of " + (nv ? nv->py_str() : std::string()) + " samples, for " + std::to_string(want) + " asked";
      return Failure::Protocol;
    }
    const std::size_t bytes = static_cast<std::size_t>(n) * 8;  // n <= READ_CHUNK
    while (buf_.size() < bytes)
      if (Failure f = more(why); f != Failure::None) return f == Failure::Ended ? inside(why) : f;
    out.resize(static_cast<std::size_t>(n));
    std::memcpy(out.data(), buf_.data(), bytes);  // little-endian doubles (the platforms supported)
    buf_.erase(0, bytes);
    if (!buf_.empty() || child_.pending()) {
      why = "the adapter wrote more than it was asked for";
      return Failure::Protocol;
    }
    return Failure::None;
  }

 private:
  // More bytes of the answer, waiting at most Timeouts::serve_stall.
  Failure more(std::string& why) {
    const double stall = timeouts().serve_stall;
    if (!child_.wait_output(after(stall))) {
      why = "no answer for " + secs(stall) + " s";
      return Failure::Protocol;
    }
    if (child_.eof()) return Failure::Ended;
    buf_ += child_.take();
    return Failure::None;
  }
  static Failure inside(std::string& why) {
    why = "the session ended inside an answer";
    return Failure::Protocol;
  }

  Child child_;
  std::string buf_;
};

// The serve session of each adapter, each behind a lock of its own.
struct Slot {
  std::mutex m;
  std::unique_ptr<Session> session;
};
std::mutex sessions_mu;
std::map<std::string, std::shared_ptr<Slot>> sessions;

std::vector<double> read_chunk(const Adapter& a, const fs::path& index_dir, std::size_t piece, std::int64_t from,
                               std::int64_t to) {
  Dict req{{"op", Value("read")},
           {"index", Value(core::path_utf8(index_dir))},
           {"piece", Value(static_cast<std::int64_t>(piece))},
           {"a", Value(from)},
           {"b", Value(to)}};
  std::string request = Value(std::move(req)).to_json();
  std::shared_ptr<Slot> slot;
  {
    // this adapter's session only: the others are not held up while it answers
    std::lock_guard<std::mutex> lock(sessions_mu);
    auto& s = sessions[a.name];
    if (!s) s = std::make_shared<Slot>();
    slot = s;
  }
  std::lock_guard<std::mutex> lock(slot->m);
  // a session that ended (the adapter was updated, or failed) is started again, once
  for (int attempt = 0; attempt < 2; ++attempt) {
    if (!slot->session || slot->session->ended()) slot->session = std::make_unique<Session>(a.program);
    std::vector<double> out;
    std::string why;
    switch (slot->session->read(request, static_cast<std::size_t>(to - from), out, why)) {
      case Failure::None:
        return out;
      case Failure::Answer:
        throw std::runtime_error(why);
      case Failure::Ended:
        slot->session.reset();  // gone: never used again
        if (attempt == 1) throw std::runtime_error(a.name + ": its session ended");
        break;
      case Failure::Protocol:
        slot->session.reset();  // out of step: never used again
        throw std::runtime_error(a.name + ": " + why + "; its session was stopped");
    }
  }
  throw std::runtime_error(a.name + ": its session could not be started");
}

}  // namespace

fs::path default_folder() {
#ifdef _WIN32
  if (const char* l = std::getenv("LOCALAPPDATA")) return core::utf8_path(l) / "medorama" / "adapters";
#endif
  const char* h = std::getenv("HOME");
  return core::utf8_path(h ? h : "") / ".local" / "share" / "medorama" / "adapters";
}

void set_folders(std::vector<fs::path> dirs) {
  std::lock_guard<std::mutex> lock(mu);
  folders_ = std::move(dirs);
  found_.reset();
}

std::vector<Adapter> installed() {
  std::lock_guard<std::mutex> lock(mu);
  if (found_) return *found_;
  std::vector<Adapter> out;
  for (const auto& dir : folders_) {
    std::error_code ec;
    std::vector<fs::path> programs;
    for (fs::directory_iterator it(dir, ec), end; !ec && it != end; it.increment(ec))
      if (it->is_regular_file(ec) && is_adapter_name(core::path_utf8(it->path().filename())))
        programs.push_back(it->path());
    std::sort(programs.begin(), programs.end());
    for (const auto& p : programs) {
      try {
        Adapter a = describe(p);
        if (std::none_of(out.begin(), out.end(), [&](const Adapter& x) { return x.name == a.name; }))
          out.push_back(std::move(a));
      } catch (const std::exception& e) {
        std::fprintf(stderr, "adapter %s: %s\n", core::path_utf8(p).c_str(), e.what());
      }
    }
  }
  found_ = out;
  return out;
}

std::optional<std::pair<Adapter, Format>> for_file(const fs::path& path) {
  std::string ext = lower_extension(path);
  if (ext.empty()) return std::nullopt;
  for (const auto& a : installed())
    for (const auto& f : a.formats)
      if (std::find(f.extensions.begin(), f.extensions.end(), ext) != f.extensions.end()) return std::make_pair(a, f);
  return std::nullopt;
}

std::optional<Adapter> named(const std::string& name) {
  for (const auto& a : installed())
    if (a.name == name) return a;
  return std::nullopt;
}

void set_timeouts(const Timeouts& t) {
  std::lock_guard<std::mutex> lock(timeouts_mu);
  timeouts_ = t;
}

Timeouts timeouts() {
  std::lock_guard<std::mutex> lock(timeouts_mu);
  return timeouts_;
}

Adapter describe(const fs::path& program) { return describe_program(program); }

void index(const Adapter& a, const fs::path& path, const fs::path& dir, const std::function<void(double)>& progress,
           const std::function<bool()>& cancelled) {
  const double stall = timeouts().index_stall;
  auto fail = [&](const std::string& why) {
    return std::runtime_error(a.name + " could not index " + core::path_utf8(path) + ": " + why);
  };
  Child c(a.program, {"index", core::path_utf8(path), core::path_utf8(dir)}, false);
  bool done = false;
  auto deadline = after(stall);
  std::string buf;
  for (;;) {
    if (cancelled()) {
      c.kill();
      throw std::runtime_error("indexing cancelled");
    }
    // the lines read so far
    for (std::size_t nl; (nl = buf.find('\n')) != std::string::npos;) {
      std::string line = buf.substr(0, nl);
      buf.erase(0, nl + 1);
      if (line.size() > MAX_LINE) {
        c.kill();
        throw fail("an adapter wrote a line longer than 1 MiB");
      }
      deadline = after(stall);
      Dict d;
      try {
        d = json_object(line);
      } catch (const std::exception&) {
        continue;
      }
      double f;
      if (const Value* p = d.get("progress"); p && p->as_f64(f)) progress(std::isnan(f) ? 0.0 : std::clamp(f, 0.0, 1.0));
      if (const Value* v = d.get("done"); v && v->as_bool() && *v->as_bool()) done = true;
    }
    if (buf.size() > MAX_LINE) {
      c.kill();
      throw fail("an adapter wrote a line longer than 1 MiB");
    }
    if (c.eof()) break;
    auto wait = std::min(deadline, Clock::now() + std::chrono::milliseconds(100));
    if (c.wait_output(wait)) {
      buf += c.take();
    } else if (Clock::now() >= deadline) {
      c.kill();
      throw fail("no answer for " + secs(stall) + " s: stopped");
    }
  }
  if (!buf.empty() && buf.size() <= MAX_LINE) {  // a last line without its end
    try {
      Dict d = json_object(buf);
      if (const Value* v = d.get("done"); v && v->as_bool() && *v->as_bool()) done = true;
    } catch (const std::exception&) {
    }
  }
  auto status = c.wait_until(after(stall));
  if (!status) throw fail("it did not exit within " + secs(stall) + " s: stopped");
  if (*status != 0 || !done) {
    std::string why = c.stderr_line();
    throw fail(why.empty() ? "it failed (" + exit_text(*status) + ")" : why);
  }
}

std::vector<double> read(const Adapter& a, const fs::path& index_dir, std::size_t piece, std::int64_t from,
                         std::int64_t to) {
  std::vector<double> out;
  for (std::int64_t at = std::max<std::int64_t>(from, 0); at < to;) {
    std::int64_t end = to - at > READ_CHUNK ? at + READ_CHUNK : to;
    auto x = read_chunk(a, index_dir, piece, at, end);
    bool short_ = static_cast<std::int64_t>(x.size()) < end - at;
    out.insert(out.end(), x.begin(), x.end());
    if (short_) break;  // the piece ends here
    at = end;
  }
  return out;
}

}  // namespace vitalstream::engine::adapters
