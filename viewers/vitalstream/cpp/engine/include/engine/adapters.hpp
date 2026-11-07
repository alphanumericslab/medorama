// Adapters: separate programs that read recording formats the viewer does not read itself
// (licensed or vendor formats), as specified in docs/adapters.md (the Rust engine's adapters.rs
// speaks the same protocol). An adapter is an executable named `<name>-adapter` in an adapter
// folder; the viewer asks it to describe itself, to index a recording into the index format, and
// (in a `serve` session kept open) for the samples of an indexed recording.
#pragma once

#include <cstdint>
#include <filesystem>
#include <functional>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace vitalstream::engine::adapters {

namespace fs = std::filesystem;

// The protocol version this viewer speaks.
constexpr int PROTOCOL = 1;

// Every child runs with a cleared environment (ENV_ALLOWED, and the LC_* variables), its standard
// error kept (the last STDERR_KEPT bytes) but never printed, and within the time limits of
// Timeouts (docs/adapters.md, "Limits and environment").
inline constexpr const char* ENV_ALLOWED[] = {"PATH",   "HOME",        "USERPROFILE", "TMP",    "TEMP",
                                              "TMPDIR", "LANG",        "SYSTEMROOT",  "SYSTEMDRIVE",
                                              "WINDIR", "COMSPEC",     "PATHEXT",     "LOCALAPPDATA", "APPDATA"};
constexpr std::size_t STDERR_KEPT = 64 * 1024;
// The samples asked of a session in one request; longer reads are split.
constexpr std::int64_t READ_CHUNK = std::int64_t{1} << 20;

// Time limits on an adapter's commands, in seconds.
struct Timeouts {
  double describe = 30;           // describe: the whole command
  double index_stall = 300;       // index: the longest time without a line of output (or to exit after it)
  double serve_stall = 60;        // serve: the longest wait for the next bytes of an answer
};
void set_timeouts(const Timeouts& t);  // from now on (tests; the defaults otherwise)
Timeouts timeouts();

struct Format {
  std::string kind;   // the recording kind written in the index (meta.kind)
  std::string label;
  std::vector<std::string> extensions;  // lower case, without the dot
};

struct Adapter {
  std::string name, version;
  fs::path program;
  std::vector<Format> formats;
};

// ~/.local/share/medorama/adapters (Windows: %LOCALAPPDATA%\medorama\adapters).
fs::path default_folder();
// The folders searched for adapters, in order (the setting adapters_dir).
void set_folders(std::vector<fs::path> dirs);
// The adapters installed, each described once.
std::vector<Adapter> installed();
// An adapter's description (describe), checked against the protocol.  Throws std::runtime_error.
Adapter describe(const fs::path& program);
// The adapter and format for a file, by its extension.
std::optional<std::pair<Adapter, Format>> for_file(const fs::path& path);
std::optional<Adapter> named(const std::string& name);

// Write the index files of `path` into `dir` (the adapter's `index`), reporting progress.  The
// adapter is stopped once `cancelled` returns true (checked ten times a second: "indexing
// cancelled"), and when it writes no line for Timeouts::index_stall.  Throws std::runtime_error.
void index(const Adapter& a, const fs::path& path, const fs::path& dir, const std::function<void(double)>& progress,
           const std::function<bool()>& cancelled = [] { return false; });

// Physical samples [a, b) of a piece of the recording indexed in `index_dir`, from the adapter's
// `serve` session (started once, kept open; in requests of at most READ_CHUNK samples).  The
// sessions of different adapters answer side by side; one that answers out of step is stopped.
// Throws std::runtime_error.
std::vector<double> read(const Adapter& a, const fs::path& index_dir, std::size_t piece, std::int64_t from,
                         std::int64_t to);

}  // namespace vitalstream::engine::adapters
