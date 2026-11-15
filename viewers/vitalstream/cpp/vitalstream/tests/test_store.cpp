// The index store (index format 3, engine/store.hpp; as libs/engine/tests/store.rs and the pipe
// test of viewers/vitalstream/rust/tests/viewer.rs): an index packed into a store answers as the
// same index in a folder (format 2) does, across chunk boundaries, encrypted or not; without its
// key nothing is read; an index is kept, replaced and removed whole; a cancelled build leaves
// nothing; the viewer takes stores and keys on a pipe; and a store the Rust viewer wrote holds the
// same bytes as the one this engine writes, each opening the other's.
#include <gtest/gtest.h>
#include <httplib.h>
#include <sqlite3.h>

#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <map>
#include <thread>

#include "core/value.hpp"
#include "engine/dataset.hpp"
#include "engine/index.hpp"
#include "engine/store.hpp"
#include "needs_wfdb.hpp"
#include "server.hpp"
#include "settings.hpp"

using namespace vitalstream::engine;
using namespace vitalstream::viewer;
namespace fs = std::filesystem;

namespace {

fs::path testdata() { return fs::path(VS_TESTDATA); }

fs::path temp_dir(const char* name) {
  fs::path d = fs::temp_directory_path() / name;
  fs::remove_all(d);
  fs::create_directories(d);
  return fs::canonical(d);
}

StoreKey key_of(unsigned char b) {
  StoreKey k;
  k.fill(b);
  return k;
}

std::string hex_of(unsigned char b) {
  static const char* HEX = "0123456789abcdef";
  std::string out;
  for (int i = 0; i < 32; ++i) out += std::string{HEX[b >> 4], HEX[b & 15]};
  return out;
}

// The recordings indexed: an EDF+ file, and WFDB folders when this build reads them.
std::vector<fs::path> recordings() {
  std::vector<fs::path> out{testdata() / "edf" / "psg_plus.edf"};
  if (wfdb_supported()) {
    out.push_back(testdata() / "collection");
    out.push_back(testdata() / "session");
  }
  return out;
}

// meta without its version, key and informative fields.
std::string stripped(Dict m, bool keep_version_and_key = false) {
  std::vector<const char*> keys{"engine", "indexed_s"};
  if (!keep_version_and_key) {
    keys.push_back("index_version");
    keys.push_back("key");
  }
  for (const char* k : keys) m.erase(k);
  return Value(std::move(m)).to_json();
}

// The same queries on two datasets of one recording give the same answers.
void same_answers(const Dataset& a, const Dataset& b) {
  auto info = [](const Dataset& d) {
    Value v = d.info();
    if (Dict* m = v.as_dict()) m->erase("key");
    return v.to_json();
  };
  ASSERT_EQ(info(a), info(b));
  Value i = a.info();
  const List* range = i.get("t_range") ? i.get("t_range")->as_list() : nullptr;
  double t0 = range && range->size() == 2 ? *f64_of(&(*range)[0]) : 0.0;
  double t1 = range && range->size() == 2 ? *f64_of(&(*range)[1]) : t0 + 60.0;
  std::vector<std::int64_t> channels;
  for (std::size_t c = 0; c < a.channel_count(); ++c) channels.push_back(static_cast<std::int64_t>(c));
  // the whole recording, a short window, and a window across a piece's middle
  for (auto [x, y, px] : {std::tuple{t0, t1, 800}, std::tuple{t0, t0 + 10.0, 1200},
                          std::tuple{t0 + (t1 - t0) / 3.0, t0 + (t1 - t0) / 2.0, 50}}) {
    EXPECT_EQ(a.window_json(channels, x, y, px), b.window_json(channels, x, y, px));
    EXPECT_EQ(a.markers(channels, x, y).to_json(), b.markers(channels, x, y).to_json());
  }
}

std::string file_bytes(const fs::path& p) {
  std::ifstream f(p, std::ios::binary);
  return std::string((std::istreambuf_iterator<char>(f)), std::istreambuf_iterator<char>());
}

bool contains(const std::string& hay, const std::string& needle) { return hay.find(needle) != std::string::npos; }

// The rows of a store's table (each row its columns' bytes, | between them), with its key.
std::vector<std::string> rows(const fs::path& store, const std::optional<StoreKey>& key, const char* sql) {
  sqlite3* db = nullptr;
  std::vector<std::string> out;
  if (sqlite3_open(store.string().c_str(), &db) != SQLITE_OK) return out;
  if (key) {
    std::string hex;
    static const char* HEX = "0123456789abcdef";
    for (unsigned char b : *key) hex += std::string{HEX[b >> 4], HEX[b & 15]};
    sqlite3_exec(db, ("PRAGMA key = \"x'" + hex + "'\";").c_str(), nullptr, nullptr, nullptr);
  }
  sqlite3_stmt* st = nullptr;
  if (sqlite3_prepare_v2(db, sql, -1, &st, nullptr) == SQLITE_OK) {
    while (sqlite3_step(st) == SQLITE_ROW) {
      std::string row;
      for (int c = 0; c < sqlite3_column_count(st); ++c) {
        const void* b = sqlite3_column_blob(st, c);
        row += (c ? "|" : "") + (b ? std::string(static_cast<const char*>(b), sqlite3_column_bytes(st, c)) : "");
      }
      out.push_back(row);
    }
  }
  sqlite3_finalize(st);
  sqlite3_close(db);
  return out;
}

void exec(const fs::path& store, const char* sql) {
  sqlite3* db = nullptr;
  ASSERT_EQ(sqlite3_open(store.string().c_str(), &db), SQLITE_OK);
  EXPECT_EQ(sqlite3_exec(db, sql, nullptr, nullptr, nullptr), SQLITE_OK) << sqlite3_errmsg(db);
  sqlite3_close(db);
}

std::size_t entries(const fs::path& dir) {
  std::error_code ec;
  std::size_t n = 0;
  for (fs::directory_iterator it(dir, ec), end; !ec && it != end; it.increment(ec)) ++n;
  return n;
}

}  // namespace

TEST(Store, AStoreAnswersAsAFolderDoes) {
  for (const fs::path& path : recordings()) {
    fs::path dir = temp_dir("vs_store_answers");
    std::string key2 = build_index(path, dir / "cache", [](double) {});
    auto v2 = load_index(dir / "cache", key2);
    ASSERT_TRUE(v2) << path;
    // a plain store, and an encrypted one, cut into small chunks
    for (auto key : {std::optional<StoreKey>(), std::optional<StoreKey>(key_of(7))}) {
      Store store(dir / (key ? "encrypted.sqlite" : "plain.sqlite"), key, 100);
      EXPECT_EQ(store.chunk_values(), 100u);
      std::string key3 = build_into_store(path, store, [](double) {});
      EXPECT_NE(key3, key2) << "another version, another key";
      EXPECT_EQ(key3, cache_key_of(path, 3));
      std::string why;
      auto v3 = store.load(key3, why);
      ASSERT_TRUE(v3) << path << ": " << why;
      EXPECT_EQ(stripped(v2->first), stripped(v3->first)) << path;
      EXPECT_EQ(int_of(v3->first.get("index_version")), std::optional<std::int64_t>(3));
      EXPECT_EQ(*v3->first.get("key")->as_str(), key3);
      // the store holds meta.json's text as written
      EXPECT_EQ(*store.meta(key3), Value(v3->first).to_json());
      Dataset a(v2->first, v2->second), b(std::move(v3->first), std::move(v3->second));
      same_answers(a, b);
      // nothing of the build is left next to the store
      EXPECT_EQ(entries(store.tmp_dir()), 0u);
    }
    fs::remove_all(dir);
  }
}

TEST(Store, WithoutItsKeyAStoreIsNotRead) {
  fs::path dir = temp_dir("vs_store_key");
  fs::path file = dir / "index.sqlite";
  fs::path rec = testdata() / "edf" / "psg_plus.edf";
  std::string key;
  {
    Store store(file, key_of(1));
    key = build_into_store(rec, store, [](double) {});
  }
  EXPECT_THROW(Store(file, std::nullopt), std::runtime_error) << "no key";
  try {
    Store(file, key_of(2));
    ADD_FAILURE() << "another key";
  } catch (const std::runtime_error& e) {
    EXPECT_TRUE(contains(e.what(), "its key is wrong")) << e.what();
  }
  // the file holds no readable text of the index, nor the key
  std::string bytes = file_bytes(file);
  for (const char* wal : {"-wal", "-shm"}) bytes += file_bytes(fs::path(file.string() + wal));
  EXPECT_FALSE(contains(bytes, key));
  EXPECT_FALSE(contains(bytes, "\"pieces\":"));
  EXPECT_FALSE(contains(bytes, hex_of(1)));
  EXPECT_FALSE(contains(bytes, std::string(32, '\x01')));
  EXPECT_TRUE(Store(file, key_of(1)).load(key));
  fs::remove_all(dir);
}

TEST(Store, AnIndexIsKeptReplacedAndRemovedWhole) {
  fs::path dir = temp_dir("vs_store_whole");
  fs::path file = dir / "index.sqlite";
  Store store(file, std::nullopt);
  fs::path rec = testdata() / "edf" / "psg_plus.edf";
  std::string key = build_into_store(rec, store, [](double) {});
  // built again by the same engine: the index there is kept
  auto written = rows(file, std::nullopt, "SELECT written_at FROM indexes");
  EXPECT_EQ(build_into_store(rec, store, [](double) {}), key);
  EXPECT_EQ(rows(file, std::nullopt, "SELECT written_at FROM indexes"), written);
  EXPECT_EQ(store.keys(), std::vector<std::string>{key});
  // by another engine: replaced, whole
  exec(file, "UPDATE indexes SET engine = 'another'");
  exec(file, "INSERT INTO chunks (key, array, n, data) SELECT key, 'left_over.f32', 0, x'' FROM indexes");
  EXPECT_EQ(build_into_store(rec, store, [](double) {}), key);
  EXPECT_EQ(store.engine_of(key), std::optional<std::string>(engine_version()));
  EXPECT_TRUE(rows(file, std::nullopt, "SELECT * FROM chunks WHERE array = 'left_over.f32'").empty());
  // a cancelled build leaves nothing
  std::string e;
  try {
    build_into_store(testdata() / "edf" / "plain.edf", store, [](double) {}, [] { return true; });
  } catch (const std::exception& x) {
    e = x.what();
  }
  EXPECT_TRUE(contains(e, "cancelled")) << e;
  EXPECT_EQ(store.keys(), std::vector<std::string>{key});
  EXPECT_EQ(entries(store.tmp_dir()), 0u);
  // removed: every row of the key
  EXPECT_TRUE(store.remove(key));
  EXPECT_FALSE(store.remove(key));
  EXPECT_FALSE(store.load(key));
  EXPECT_TRUE(store.keys().empty());
  for (const char* t : {"SELECT * FROM chunks", "SELECT * FROM arrays", "SELECT * FROM indexes"})
    EXPECT_TRUE(rows(file, std::nullopt, t).empty()) << t;
  fs::remove_all(dir);
}

TEST(Store, EnvelopesAreReadAcrossChunks) {
  fs::path dir = temp_dir("vs_store_chunks");
  fs::path rec = testdata() / "edf" / "psg_plus.edf";
  std::string key2 = build_index(rec, dir / "cache", [](double) {});
  auto v2 = load_index(dir / "cache", key2);
  ASSERT_TRUE(v2);
  fs::path file = dir / "index.sqlite";
  constexpr std::size_t C = 7;
  std::string key;
  {
    Store store(file, std::nullopt, C);
    key = build_into_store(rec, store, [](double) {});
  }
  // an existing store keeps its chunk size
  Store store(file, std::nullopt, 1000);
  EXPECT_EQ(store.chunk_values(), C);
  auto v3 = store.load(key);
  ASSERT_TRUE(v3);
  ASSERT_EQ(v3->second.env_min.size(), v2->second.env_min.size());
  for (std::size_t lvl = 0; lvl < v2->second.env_min.size(); ++lvl) {
    const Env& file_env = v2->second.env_min[lvl];
    const Env& chunked = v3->second.env_min[lvl];
    ASSERT_EQ(file_env.size(), chunked.size());
    std::size_t len = chunked.size();
    // every range starting and ending around chunk boundaries
    for (std::size_t o = 0; o < std::min<std::size_t>(len, 4 * C); ++o)
      for (std::size_t n = 0; o + n <= len && n < 3 * C; ++n) ASSERT_EQ(file_env.range(o, n), chunked.range(o, n));
    EXPECT_EQ(file_env.range(0, len), chunked.range(0, len));
    EXPECT_FALSE(chunked.range(len, 1));
    EXPECT_FALSE(chunked.range(0, len + 1));
    // the array's rows: its length, and its chunks of C values (the last shorter)
    std::string name = "env_min_" + std::to_string(lvl) + ".f32";
    auto n = rows(file, std::nullopt, ("SELECT n_values FROM arrays WHERE array = '" + name + "'").c_str());
    ASSERT_EQ(n.size(), 1u);
    EXPECT_EQ(std::stoull(n[0]), len);
    auto sizes = rows(file, std::nullopt, ("SELECT length(data) FROM chunks WHERE array = '" + name + "' ORDER BY n").c_str());
    EXPECT_EQ(sizes.size(), len == 0 ? 1 : (len + C - 1) / C);
  }
  // an empty array has one chunk of no bytes
  auto mk = rows(file, std::nullopt, "SELECT length(data) FROM chunks WHERE array = 'vit_t.f64'");
  ASSERT_EQ(mk.size(), 1u);
  EXPECT_EQ(mk[0], "0");
  // a chunk of another length than its array implies: that read fails
  exec(file, "UPDATE chunks SET data = substr(data, 1, 4) WHERE array = 'env_min_0.f32' AND n = 1");
  {
    Store again(file, std::nullopt);
    auto v = again.load(key);
    ASSERT_TRUE(v);
    EXPECT_TRUE(v->second.env_min[0].range(0, C));
    EXPECT_FALSE(v->second.env_min[0].range(C, 1));
  }
  // an array whose length disagrees with its chunks: not loaded
  exec(file, "UPDATE arrays SET n_values = n_values + 1 WHERE array = 'mk_code.i16'");
  std::string why;
  EXPECT_FALSE(Store(file, std::nullopt).load(key, why));
  EXPECT_TRUE(contains(why, "mk_code.i16")) << why;
  fs::remove_all(dir);
}

TEST(Store, EnvelopesAreReadWhileAnIndexIsPacked) {
  // another program holds the store's write lock: this program's pack waits for it, holding its
  // own connection; the envelopes' chunks are read meanwhile, on their own connection
  fs::path dir = temp_dir("vs_store_busy");
  fs::path rec = testdata() / "edf" / "psg_plus.edf";
  fs::path file = dir / "index.sqlite";
  Store store(file, std::nullopt, 7);
  std::string key = build_into_store(rec, store, [](double) {});
  auto v = store.load(key);
  ASSERT_TRUE(v);
  sqlite3* other = nullptr;
  ASSERT_EQ(sqlite3_open(file.string().c_str(), &other), SQLITE_OK);
  ASSERT_EQ(sqlite3_exec(other, "BEGIN IMMEDIATE", nullptr, nullptr, nullptr), SQLITE_OK);
  fs::create_directories(dir / "other");
  std::ofstream(dir / "other" / "meta.json") << "{\"engine\":\"other\"}";
  std::thread packing([&] { store.pack("other", dir / "other"); });
  std::this_thread::sleep_for(std::chrono::milliseconds(200));  // pack is waiting for the lock
  auto t = std::chrono::steady_clock::now();
  auto got = v->second.env_min[0].range(0, 20);
  auto waited = std::chrono::steady_clock::now() - t;
  sqlite3_exec(other, "COMMIT", nullptr, nullptr, nullptr);
  sqlite3_close(other);
  packing.join();
  EXPECT_TRUE(got);
  EXPECT_LT(waited, std::chrono::seconds(5));
  EXPECT_TRUE(store.meta("other"));
  fs::remove_all(dir);
}

TEST(Store, WhatAKilledProgramLeftIsRemovedADayLater) {
  fs::path dir = temp_dir("vs_store_left");
  fs::path file = dir / "index.sqlite";
  { Store s(file, std::nullopt); }
  fs::path tmp = dir / "index.tmp";
  for (const char* name : {"0123.1.0.tmp", "0123.1.1.read", "0123.1.2.tmp"}) {
    fs::create_directories(tmp / name);
    std::ofstream(tmp / name / "meta.json") << "{}";
  }
  auto old = fs::file_time_type::clock::now() - std::chrono::hours(25);
  fs::last_write_time(tmp / "0123.1.0.tmp", old);
  fs::last_write_time(tmp / "0123.1.1.read", old);
  { Store s(file, std::nullopt); }
  EXPECT_FALSE(fs::exists(tmp / "0123.1.0.tmp"));
  EXPECT_FALSE(fs::exists(tmp / "0123.1.1.read"));
  EXPECT_TRUE(fs::exists(tmp / "0123.1.2.tmp")) << "a build of today is left alone";
  fs::remove_all(dir);
}

TEST(Store, AStoreOfAnotherFormatIsRefused) {
  fs::path dir = temp_dir("vs_store_format");
  fs::path file = dir / "index.sqlite";
  { Store s(file, std::nullopt); }
  exec(file, "UPDATE store SET value = '4' WHERE name = 'format'");
  try {
    Store s(file, std::nullopt);
    ADD_FAILURE() << "opened";
  } catch (const std::runtime_error& e) {
    EXPECT_TRUE(contains(e.what(), "another index store format")) << e.what();
  }
  fs::remove_all(dir);
}

namespace {

// A viewer with the stores given, listening on a free port of this machine.
class Running {
 public:
  Running(const Settings& s, std::shared_ptr<Stores> stores) {
    add_routes(svr_, make_app(s, false, std::move(stores)));
    port_ = svr_.bind_to_any_port("127.0.0.1");
    thread_ = std::thread([this] { svr_.listen_after_bind(); });
    svr_.wait_until_ready();
  }
  ~Running() {
    svr_.stop();
    thread_.join();
  }
  httplib::Client client() const {
    httplib::Client c("127.0.0.1", port_);
    c.set_read_timeout(60, 0);
    return c;
  }

 private:
  httplib::Server svr_;
  std::thread thread_;
  int port_ = 0;
};

// Open path and wait for its index: (key, the dataset's final state).
std::pair<std::string, std::string> opened(httplib::Client& c, const fs::path& path) {
  auto r = c.Post("/api/v1/open", "{\"path\": " + Value(path.string()).to_json() + "}", "application/json");
  EXPECT_EQ(r->status, 200) << r->body;
  std::string key = Value::from_json(r->body).get("key")->py_str();
  for (int i = 0; i < 400; ++i) {
    std::string state = Value::from_json(c.Get("/api/v1/datasets/" + key)->body).get("state")->py_str();
    if (state != "indexing") return {key, state};
    std::this_thread::sleep_for(std::chrono::milliseconds(25));
  }
  return {key, "still indexing"};
}

std::string line(const fs::path& p, unsigned char k, bool own) {
  return "{\"store\":" + Value(p.string()).to_json() + ",\"key\":\"" + hex_of(k) + "\",\"own\":" +
         (own ? "true" : "false") + "}";
}

}  // namespace

TEST(Store, StoresAndKeysOnAPipe) {
  // launched by Medorama: a database's store and the viewer's own, each with its key
  fs::path dir = temp_dir("vs_store_pipe");
  fs::path db_store = dir / "db-0001" / "index.sqlite", own = dir / "viewer" / "index.sqlite";
  fs::path rec = testdata() / "edf" / "psg_plus.edf";
  {
    // Medorama indexed one recording into its database's store
    Store st(db_store, key_of(1));
    build_into_store(rec, st, [](double) {});
  }
  auto stores = std::make_shared<Stores>();
  stores->apply(line(db_store, 1, false));
  stores->apply(line(own, 2, true));
  try {
    stores->apply(line(db_store, 9, false));
    ADD_FAILURE() << "a wrong key is accepted";
  } catch (const std::runtime_error& e) {
    EXPECT_FALSE(contains(e.what(), hex_of(9))) << "the key is never repeated";
  }
  EXPECT_THROW(stores->apply("not json"), std::runtime_error);
  EXPECT_THROW(stores->apply("{\"store\": \"/x\", \"key\": \"00\"}"), std::runtime_error);
  Settings s = Settings::defaults();
  s.auth_file = dir / "auth.json";
  s.cache_dir = dir / "cache";
  s.finish();
  {
    Running r(s, stores);
    auto c = r.client();
    // found in the database's store, nothing written to the viewer's
    EXPECT_EQ(opened(c, rec).second, "ready");
    EXPECT_TRUE(Store(own, key_of(2)).keys().empty());
    // another recording: indexed into the viewer's own store, encrypted with its key
    auto [key, state] = opened(c, testdata() / "edf" / "plain.edf");
    EXPECT_EQ(state, "ready");
    EXPECT_EQ(Store(own, key_of(2)).keys(), std::vector<std::string>{key});
    EXPECT_THROW(Store(own, std::nullopt), std::runtime_error);
    EXPECT_FALSE(fs::exists(dir / "cache" / "index.sqlite")) << "no store of its own in cache_dir";
    // forgotten: the database's store is no longer read
    stores->apply("{\"forget\":" + Value(db_store.string()).to_json() + "}");
    auto paths = stores->paths();
    EXPECT_EQ(std::find(paths.begin(), paths.end(), db_store.string()), paths.end());
    EXPECT_EQ(paths.size(), 1u);
    stores->forget_all();
    EXPECT_TRUE(stores->paths().empty());
  }
  fs::remove_all(dir);
}

TEST(Store, TheViewersOwnStoreUsesItsKeyFile) {
  fs::path dir = temp_dir("vs_store_own");
  Settings s = Settings::defaults();
  s.cache_dir = dir / "cache";
  s.index_key_file = dir / "index.key";
  std::ofstream(s.index_key_file) << hex_of(5) << "\n";
  auto own = Stores::own_from(s)->own();
  ASSERT_TRUE(own);
  EXPECT_EQ(own->path(), dir / "cache" / "index.sqlite");
  EXPECT_THROW(Store(own->path(), std::nullopt), std::runtime_error);
  EXPECT_NO_THROW(Store(own->path(), key_of(5)));
  std::ofstream(s.index_key_file) << "not a key";
  EXPECT_THROW(Stores::own_from(s), std::runtime_error);
  // without a key file: a plain store
  s.index_key_file.clear();
  s.cache_dir = dir / "plain";
  Stores::own_from(s);
  EXPECT_NO_THROW(Store(dir / "plain" / "index.sqlite", std::nullopt));
  fs::remove_all(dir);
}

#ifndef _WIN32
namespace {

// The Rust viewer, when built (VS_RUST_VIEWER, else the repository's target/release or
// target/debug): it writes the stores this engine reads.
std::optional<fs::path> rust_viewer() {
  if (const char* e = std::getenv("VS_RUST_VIEWER"); e && *e) return fs::path(e);
  fs::path repo = testdata() / ".." / ".." / "..";
  for (const char* b : {"release", "debug"}) {
    fs::path p = repo / "target" / b / "vitalstream-rs";
    if (fs::exists(p)) return fs::canonical(p);
  }
  return std::nullopt;
}

// The Rust viewer's --index of path into the store of cache_dir (encrypted with key k); whether
// it succeeded.
bool rust_index(const fs::path& viewer, const fs::path& dir, const fs::path& cache_dir, unsigned char k,
                const fs::path& path) {
  fs::path key_file = dir / "rust.key", config = dir / "rust.toml";
  std::ofstream(key_file) << hex_of(k);
  std::ofstream(config) << "cache_dir = " << Value(cache_dir.string()).to_json()
                        << "\nindex_key_file = " << Value(key_file.string()).to_json() << "\n";
  std::string cmd = "'" + viewer.string() + "' --config '" + config.string() + "' --index '" + path.string() +
                    "' > '" + (dir / "rust.out").string() + "' 2>&1";
  return std::system(cmd.c_str()) == 0;
}

}  // namespace

TEST(Store, TheRustViewersStoreHoldsTheSameBytes) {
  auto viewer = rust_viewer();
  if (!viewer) GTEST_SKIP() << "no Rust viewer built (cargo build --release -p vitalstream-rs, or VS_RUST_VIEWER)";
  for (const fs::path& path : recordings()) {
    fs::path dir = temp_dir("vs_store_rust");
    // the Rust viewer writes an encrypted store, this engine another with the same chunk size
    ASSERT_TRUE(rust_index(*viewer, dir, dir / "rust", 4, path)) << file_bytes(dir / "rust.out");
    fs::path rust_file = dir / "rust" / "index.sqlite", cpp_file = dir / "cpp" / "index.sqlite";
    Store rust(rust_file, key_of(4));
    Store cpp(cpp_file, key_of(6), rust.chunk_values());
    std::string key = build_into_store(path, cpp, [](double) {});
    ASSERT_EQ(rust.keys(), std::vector<std::string>{key}) << path;
    // the same meta.json text but for the informative fields, the same arrays, the same chunks
    std::string why;
    auto a = rust.load(key, why);
    ASSERT_TRUE(a) << why;
    auto b = cpp.load(key);
    ASSERT_TRUE(b);
    EXPECT_EQ(stripped(a->first, true), stripped(b->first, true)) << path;
    for (const char* sql : {"SELECT array, n_values FROM arrays ORDER BY array",
                            "SELECT array, n, data FROM chunks ORDER BY array, n", "SELECT * FROM store ORDER BY name"}) {
      auto ra = rows(rust_file, key_of(4), sql), rb = rows(cpp_file, key_of(6), sql);
      EXPECT_FALSE(ra.empty()) << sql;
      EXPECT_TRUE(ra == rb) << path << ": " << sql;
    }
    Dataset da(std::move(a->first), std::move(a->second)), db(std::move(b->first), std::move(b->second));
    same_answers(da, db);
    // the other way: the Rust viewer opens this engine's store (and keeps its index, written by
    // the same engine version, or replaces it)
    ASSERT_TRUE(rust_index(*viewer, dir, dir / "cpp", 6, path)) << file_bytes(dir / "rust.out");
    EXPECT_TRUE(contains(file_bytes(dir / "rust.out"), key)) << file_bytes(dir / "rust.out");
    EXPECT_EQ(Store(cpp_file, key_of(6)).keys(), std::vector<std::string>{key});
    EXPECT_TRUE(Store(cpp_file, key_of(6)).load(key));
    fs::remove_all(dir);
  }
}
#endif
