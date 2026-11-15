// Index cache: per source a folder <key>/ with meta.json and one little-endian binary file per
// array (envelopes, markers, vital-sign series), memory-mapped when loaded (index format 2); or
// an index store (format 3, engine/store.hpp), built the same way and packed.  The key derives
// from the source's real path, size and modification time (viewers/vitalstream/docs/index-format.md).
#pragma once

#include <filesystem>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include "engine/common.hpp"
#include "core/file_data.hpp"

namespace vitalstream::engine {

namespace fs = std::filesystem;

std::string cache_key(const fs::path& path);
// The key of path in index format version (2: a folder, 3: a store).
std::string cache_key_of(const fs::path& path, std::int64_t version);

using Progress = std::function<void(double)>;
using Cancelled = std::function<bool()>;

// Write the array files of an index into dir (viewers/vitalstream/docs/index-format.md, "Files of one index");
// adapters written in C++ use it for their `index` command.  Throws.
void write_arrays(const fs::path& dir, const Arrays& a);

// Index path into cache_dir; returns the key.  progress gets fractions 0..1.  Throws.  Built in a
// folder of its own (<key>.<process id>.<n>.tmp, removed on every way out), checked as a reader
// checks it, then kept unless a complete index of this engine is already there; builds of one
// key in this process wait for each other (index-format.md, "Writing and sharing").
std::string build_index(const fs::path& path, const fs::path& cache_dir, const Progress& progress);
// build_index that stops (an error, nothing left behind) once cancelled returns true.
std::string build_index_until(const fs::path& path, const fs::path& cache_dir, const Progress& progress,
                              const Cancelled& cancelled);

// Write the index files of path (its arrays and meta.json, index format version, under key) into
// the new folder dir, and check them as a reader would.  Throws.
void build_files(const fs::path& path, const fs::path& dir, const std::string& key, std::int64_t version,
                 const Progress& progress, const Cancelled& cancelled);

// An array read whole: a version 2 file memory-mapped (empty files are not mapped), or values
// held (a store's).
template <class T>
class Values {
 public:
  Values() = default;
  explicit Values(const fs::path& p);
  explicit Values(std::vector<T> held) : held_(std::make_shared<const std::vector<T>>(std::move(held))) {}
  const T* data() const {
    if (held_) return held_->data();
    return file_ ? reinterpret_cast<const T*>(file_->data()) : nullptr;
  }
  std::size_t size() const {
    if (held_) return held_->size();
    return file_ ? file_->size() / sizeof(T) : 0;
  }

 private:
  std::shared_ptr<const core::FileData> file_;
  std::shared_ptr<const std::vector<T>> held_;
};

class Chunked;  // engine/store.hpp

// An envelope array: a version 2 file memory-mapped, or a store's array read by range.
class Env {
 public:
  explicit Env(Values<float> file) : file_(std::move(file)) {}
  explicit Env(std::shared_ptr<const Chunked> chunked) : chunked_(std::move(chunked)) {}
  std::size_t size() const;
  // The values [o, o+n), or nothing when they cannot be read.
  std::optional<std::vector<float>> range(std::size_t o, std::size_t n) const;

 private:
  Values<float> file_;
  std::shared_ptr<const Chunked> chunked_;
};

struct Loaded {
  // The index folder (a store's index of an adapter's recording: a private folder holding its
  // meta.json, for the adapter to read samples again).
  fs::path dir;
  std::vector<Env> env_min, env_max;
  Values<std::int32_t> mk_piece;
  Values<std::int64_t> mk_sample;
  Values<std::int16_t> mk_code;
  Values<double> vit_t, vit_v;
  // That private folder, removed when the last copy of this goes.
  std::shared_ptr<const void> reading;
};

// (meta, arrays) from the cache, or nothing: no index under key, one of another version, or one
// whose meta.json and arrays do not agree (check).
std::optional<std::pair<Dict, Loaded>> load_index(const fs::path& cache_dir, const std::string& key);
// load_index of the index folder dir; why says what is wrong when nothing is returned.
std::optional<std::pair<Dict, Loaded>> load_from(const fs::path& dir, std::string& why);
// load_from of index format version.
std::optional<std::pair<Dict, Loaded>> load_files(const fs::path& dir, std::int64_t version, std::string& why);
// Why meta.json does not agree with the arrays (viewers/vitalstream/docs/index-format.md,
// "Loading an index"), or "" when every offset and length lies inside its array.
std::string check(const Dict& meta, const Loaded& a);

// Unix modification time in nanoseconds (0 when unknown).
std::int64_t mtime_ns(const fs::path& p);

// The writers of each key in this process, one at a time (a key, or a store's path and a key).
std::shared_ptr<std::mutex> key_lock(const std::string& key);
// A folder of dir named uniquely for this writer: <key>.<process id>.<n>.<suffix>.
fs::path unique_dir(const fs::path& dir, const std::string& key, const char* suffix);
// A folder only its owner may read (made if missing).  Throws.
void private_dir(const fs::path& p);
// Removes a folder when it goes out of scope (an index being built, on every way out).
struct Removed {
  fs::path path;
  ~Removed();
};

}  // namespace vitalstream::engine
