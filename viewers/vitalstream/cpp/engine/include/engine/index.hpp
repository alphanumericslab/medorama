// Index cache: per source a folder <key>/ with meta.json and one little-endian binary file per
// array (envelopes, markers, vital-sign series), memory-mapped when loaded.  The key derives
// from the source's real path, size and modification time (viewers/vitalstream/docs/index-format.md).
#pragma once

#include <filesystem>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>

#include "engine/common.hpp"
#include "core/file_data.hpp"

namespace vitalstream::engine {

namespace fs = std::filesystem;

std::string cache_key(const fs::path& path);

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

// A memory-mapped array file (empty files are not mapped).
template <class T>
class Mapped {
 public:
  Mapped() = default;
  explicit Mapped(const fs::path& p);
  const T* data() const { return file_ ? reinterpret_cast<const T*>(file_->data()) : nullptr; }
  std::size_t size() const { return file_ ? file_->size() / sizeof(T) : 0; }

 private:
  std::shared_ptr<const core::FileData> file_;
};

struct Loaded {
  fs::path dir;  // the index folder
  std::vector<Mapped<float>> env_min, env_max;
  Mapped<std::int32_t> mk_piece;
  Mapped<std::int64_t> mk_sample;
  Mapped<std::int16_t> mk_code;
  Mapped<double> vit_t, vit_v;
};

// (meta, arrays) from the cache, or nothing: no index under key, one of another version, or one
// whose meta.json and arrays do not agree (check).
std::optional<std::pair<Dict, Loaded>> load_index(const fs::path& cache_dir, const std::string& key);
// load_index of the index folder dir; why says what is wrong when nothing is returned.
std::optional<std::pair<Dict, Loaded>> load_from(const fs::path& dir, std::string& why);
// Why meta.json does not agree with the arrays (viewers/vitalstream/docs/index-format.md,
// "Loading an index"), or "" when every offset and length lies inside its array.
std::string check(const Dict& meta, const Loaded& a);

// Unix modification time in nanoseconds (0 when unknown).
std::int64_t mtime_ns(const fs::path& p);

}  // namespace vitalstream::engine
