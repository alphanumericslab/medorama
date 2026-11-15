// The index store (index format 3, viewers/vitalstream/docs/index-format.md, "Version 3: the
// index store"): one SQLite database of indexes, encrypted with SQLCipher when its owner has a
// key.  An index is built as version 2 files in a temporary folder, then packed into the store in
// one transaction; envelopes are read back by range, a chunk at a time (the Rust engine's
// store.rs).
#pragma once

#include <array>
#include <cstdint>
#include <deque>
#include <filesystem>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "engine/common.hpp"
#include "engine/index.hpp"

namespace vitalstream::engine {

namespace fs = std::filesystem;

// The store's format (store.format).
constexpr std::int64_t STORE_FORMAT = 3;
// Values per chunk in a new store (store.chunk_values); an existing store keeps its own.
constexpr std::size_t CHUNK_VALUES = 4096;
// Chunks of one array kept in memory once read.
constexpr std::size_t CACHED_CHUNKS = 16;

// A raw 256-bit SQLCipher key.
using StoreKey = std::array<unsigned char, 32>;
// A key's 64 hex digits as bytes, or nothing.
std::optional<StoreKey> store_key_of(const std::string& hex);

// The version of this engine, as indexes record it (meta.engine).
const char* engine_version();

struct Connection;  // a database connection and its lock (store.cpp)

// An envelope array of a store read by range, a chunk at a time, the last chunks read kept.
class Chunked {
 public:
  Chunked(std::shared_ptr<Connection> conn, std::string key, std::string array, std::size_t len, std::size_t chunk)
      : conn_(std::move(conn)), key_(std::move(key)), array_(std::move(array)), len_(len), chunk_(chunk) {}
  std::size_t size() const { return len_; }
  // The values [o, o+n), or nothing when the store cannot be read.
  std::optional<std::vector<float>> range(std::size_t o, std::size_t n) const;

 private:
  std::shared_ptr<const std::vector<float>> chunk_at(std::size_t n) const;

  std::shared_ptr<Connection> conn_;
  std::string key_, array_;
  std::size_t len_, chunk_;
  mutable std::mutex m_;
  mutable std::deque<std::pair<std::size_t, std::shared_ptr<const std::vector<float>>>> cache_;
};

// An index store, opened.
class Store {
 public:
  // Open (or make) the store at path; key a raw SQLCipher key (SQLCipher 4's default settings), or
  // nothing for a plain store.  A new store is made with chunk_values values per chunk (tests cut
  // small arrays into several chunks).  Throws std::runtime_error: a wrong key, another format.
  Store(const fs::path& path, const std::optional<StoreKey>& key, std::size_t chunk_values = CHUNK_VALUES);

  const fs::path& path() const { return path_; }
  std::size_t chunk_values() const { return chunk_; }
  // Where indexes are built before they are packed: <store folder>/index.tmp.
  fs::path tmp_dir() const;
  // The engine that wrote the index of key, if there is one.
  std::optional<std::string> engine_of(const std::string& key) const;
  // The meta.json text of the index of key, if there is one.
  std::optional<std::string> meta(const std::string& key) const;
  // The keys of the indexes held, in order.
  std::vector<std::string> keys() const;
  // Remove the index of key; whether there was one.  Throws.
  bool remove(const std::string& key) const;
  // Pack the version 2 files of an index (its meta.json and arrays, in dir) into the store, in one
  // transaction: an index of the key written by the same engine is kept, any other replaced.
  // Throws.
  void pack(const std::string& key, const fs::path& dir) const;
  // (meta, arrays) of the index of key, or nothing: no such index, or one whose meta and arrays do
  // not agree (index-format.md, "Loading an index"); why says what is wrong.
  std::optional<std::pair<Dict, Loaded>> load(const std::string& key, std::string& why) const;
  std::optional<std::pair<Dict, Loaded>> load(const std::string& key) const {
    std::string why;
    return load(key, why);
  }

 private:
  fs::path path_;
  std::shared_ptr<Connection> conn_;
  // the envelopes' chunks are read on a connection of their own: not held up by pack
  std::shared_ptr<Connection> chunks_;
  std::size_t chunk_ = CHUNK_VALUES;
};

// Index path into an index store (format 3): built in a private temporary folder
// (<store folder>/index.tmp/<key>.<process id>.<n>.tmp, removed on every way out), then packed in
// one transaction; returns the key.  Stops (an error, nothing left behind) once cancelled returns
// true.  Throws.
std::string build_into_store(const fs::path& path, const Store& store, const Progress& progress,
                             const Cancelled& cancelled = [] { return false; });

}  // namespace vitalstream::engine
