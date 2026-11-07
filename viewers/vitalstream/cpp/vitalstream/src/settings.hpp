// Settings: defaults, overridden by a TOML file, overridden by command-line options (the same
// keys as the Python and Rust viewers').
//
// - The viewer's root, fixed when it is built (the CMake cache variable VS_ROOT: install.sh sets
//   it, to ~, the home folder of whoever runs the viewer, unless --root DIR says otherwise;
//   tools/install-medorama.sh to the whole file system, since Medorama applies its own rules; a
//   build without it, such as a developer's cmake --preset release, has the whole file system):
//   it never opens anything outside it.
// - data_roots: folders the viewer may open, inside its root.  Default: its root.  A root that
//   contains the viewer's root is narrowed to it, and one outside it is left out (the settings
//   narrow the root, never widen it).  A path is allowed when the file or folder it leads to,
//   every symbolic link resolved, lies inside a data root.
// - max_records, max_depth, max_scan_s: the limits on a folder of WFDB records
//   (docs/wfdb-collection.md, "Limits").
// - require_password: refuse to start without a password (Medorama sets it for the viewer it
//   runs; a build without password support then never starts).
// - cache_dir: the index cache, shared by the three viewers (they write the same index).
//   Default, run from a clone of the repository: <viewer>/sample_data/outputs/index; elsewhere
//   ~/.cache/vitalstream.
// - adapters_dir: the folder of adapters, programs that read formats the viewer does not read
//   itself (docs/adapters.md).  Default: ~/.local/share/medorama/adapters.
// - auth_file: the password (shared by the three viewers), default ~/.config/vitalstream/auth.json.
// - qc_verbose: also mark the irregularities a reader repaired (data blocks put back in order);
//   default false.
// - allowed_hosts: host names the server answers for besides localhost, 127.0.0.1, ::1 and host
//   (a reverse proxy's public name; "*": any), docs/viewer-api.md, "Requests".
// - secure_cookie: send the sign-in cookie only over HTTPS (behind an HTTPS proxy).
//
// The settings file: --config FILE, else the first that exists of <viewer>/vitalstream.toml (only
// for a viewer run from the build folder of the clone it was built from) and
// ~/.config/vitalstream/vitalstream.toml.  A vitalstream.toml in the working directory is never
// read on its own (a shared data folder could hold one).  Relative paths in a settings file are
// relative to its folder.  Top-level keys apply to all viewers, the keys of a [cpp] section only
// to this one ([py] and [rs] are ignored).  A key of the wrong type is an error (data_roots must
// be a list of strings, port an integer from 0 to 65535, ...).
#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <vector>

#include "engine/sources.hpp"

namespace vitalstream::viewer {

namespace fs = std::filesystem;

extern const char* const SEP;  // the path separator of this system ("/", or "\" on Windows)

std::string home();       // the home folder (on Windows USERPROFILE)
std::string fs_root();    // "/", or on Windows the root of the home folder's drive
bool is_sep(char c);      // '/', and on Windows also '\'
std::string expand(const std::string& p);    // ~ expanded
std::string absolute(const std::string& p);  // absolute, normalized, ~ expanded; links not resolved
// The viewer's folder of the clone this program was built from (viewers/vitalstream), when it runs
// from inside that clone (its build folder); never from the working directory.
std::optional<fs::path> repo_root();
std::optional<fs::path> executable_path();   // this program
// The root fixed when this viewer was built (VS_ROOT; ~ is the home folder of whoever runs it),
// else the whole file system; absolute.
std::string built_root();

bool valid_timezone(const std::string& name);  // an IANA time zone name

struct Settings {
  std::string timezone = "America/New_York";
  std::vector<std::string> data_roots;
  fs::path cache_dir;
  std::string host = "127.0.0.1";
  int port = 8070;
  std::size_t index_workers = 2;
  fs::path auth_file;
  fs::path adapters_dir;
  bool qc_verbose = false;
  engine::Limits limits;          // the limits on a folder of WFDB records
  bool require_password = false;  // refuse to start without a password
  std::vector<std::string> allowed_hosts;  // host names answered for besides the local ones and host ("*": any)
  bool secure_cookie = false;              // the sign-in cookie only over HTTPS
  std::optional<fs::path> source;          // the settings file read, if any
  std::vector<std::string> outside;  // data roots left out because they lie outside the viewer's root

  static Settings defaults();
  // Absolute roots inside the viewer's root, validated time zone.  Throws std::runtime_error.
  void finish();
  // True if the file or folder path leads to (every symbolic link resolved) lies inside a data
  // root.
  bool allowed(const std::string& path) const;

 private:
  std::vector<std::string> real_roots_;
};

// Settings from path (or the first default file that exists).  Throws std::runtime_error.
Settings load_settings(const std::optional<std::string>& path);

std::string path_string(const fs::path& p);  // UTF-8, without Windows' \\?\ prefix

}  // namespace vitalstream::viewer
