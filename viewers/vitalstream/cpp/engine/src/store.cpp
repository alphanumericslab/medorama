// The index store (store.hpp), with SQLite and SQLCipher (cmake/deps.cmake).
#include "engine/store.hpp"

#include <sqlite3.h>

#include <algorithm>
#include <chrono>
#include <cstring>
#include <fstream>
#include <iterator>
#include <map>
#include <stdexcept>

#include "core/paths.hpp"

namespace vitalstream::engine {

using core::path_utf8;

// A database connection, used by one thread at a time.
struct Connection {
  sqlite3* db = nullptr;
  std::mutex m;
  ~Connection() {
    if (db) sqlite3_close_v2(db);
  }
};

namespace {

// A prepared statement, finalized when it goes out of scope.
class Stmt {
 public:
  Stmt(sqlite3* db, const char* sql) {
    if (sqlite3_prepare_v2(db, sql, -1, &st_, nullptr) != SQLITE_OK) {
      sqlite3_finalize(st_);
      st_ = nullptr;
    }
  }
  ~Stmt() { sqlite3_finalize(st_); }
  Stmt(const Stmt&) = delete;
  Stmt& operator=(const Stmt&) = delete;
  bool ok() const { return st_ != nullptr; }
  Stmt& text(int i, const std::string& v) {
    if (st_) sqlite3_bind_text(st_, i, v.data(), static_cast<int>(v.size()), SQLITE_TRANSIENT);
    return *this;
  }
  Stmt& integer(int i, std::int64_t v) {
    if (st_) sqlite3_bind_int64(st_, i, v);
    return *this;
  }
  Stmt& real(int i, double v) {
    if (st_) sqlite3_bind_double(st_, i, v);
    return *this;
  }
  // A blob of n bytes (an empty one is a blob of no bytes, not NULL).
  Stmt& blob(int i, const void* data, std::size_t n) {
    static const char none = 0;
    if (st_) sqlite3_bind_blob64(st_, i, n ? data : &none, n, SQLITE_TRANSIENT);
    return *this;
  }
  // The next row: SQLITE_ROW, SQLITE_DONE or an error.
  int step() { return st_ ? sqlite3_step(st_) : SQLITE_ERROR; }
  void reset() {
    if (st_) {
      sqlite3_reset(st_);
      sqlite3_clear_bindings(st_);
    }
  }
  std::string column_text(int i) const {
    const unsigned char* t = sqlite3_column_text(st_, i);
    return t ? std::string(reinterpret_cast<const char*>(t), static_cast<std::size_t>(sqlite3_column_bytes(st_, i)))
             : std::string();
  }
  std::int64_t column_int(int i) const { return sqlite3_column_int64(st_, i); }
  std::string column_blob(int i) const {
    const void* b = sqlite3_column_blob(st_, i);
    auto n = static_cast<std::size_t>(sqlite3_column_bytes(st_, i));
    return b ? std::string(static_cast<const char*>(b), n) : std::string();
  }

 private:
  sqlite3_stmt* st_ = nullptr;
};

std::runtime_error failure(const fs::path& p, const std::string& why) {
  return std::runtime_error(path_utf8(p) + ": " + why);
}

std::string db_error(sqlite3* db) { return db ? sqlite3_errmsg(db) : "out of memory"; }

void exec(sqlite3* db, const fs::path& p, const char* sql) {
  char* msg = nullptr;
  if (sqlite3_exec(db, sql, nullptr, nullptr, &msg) != SQLITE_OK) {
    std::string why = msg ? msg : db_error(db);
    sqlite3_free(msg);
    throw failure(p, why);
  }
}

// The value size of an array file, by its extension; 0 for a file that is not an array.
std::size_t value_size(const fs::path& f) {
  std::string x = path_utf8(f.extension());
  if (x == ".i16") return 2;
  if (x == ".f32" || x == ".i32") return 4;
  if (x == ".f64" || x == ".i64") return 8;
  return 0;
}

std::size_t value_size(const std::string& array) { return value_size(fs::path(array)); }

// The bytes chunk i of an array of len values holds (chunk c values each; an empty array has one
// chunk of no bytes).
std::size_t chunk_bytes(std::size_t len, std::size_t c, std::size_t i, std::size_t size) {
  if (i * c >= len) return 0;
  return std::min(c, len - i * c) * size;
}

std::size_t chunk_count(std::size_t len, std::size_t c) { return len == 0 ? 1 : (len + c - 1) / c; }

template <class T>
std::vector<T> values_of(const std::string& bytes) {
  // the platforms supported are little-endian: the values are the bytes as they are
  std::vector<T> v(bytes.size() / sizeof(T));
  if (!v.empty()) std::memcpy(v.data(), bytes.data(), v.size() * sizeof(T));
  return v;
}

std::string read_file(const fs::path& p) {
  std::ifstream f(p, std::ios::binary);
  if (!f) throw failure(p, "cannot open");
  std::string out((std::istreambuf_iterator<char>(f)), std::istreambuf_iterator<char>());
  if (f.bad()) throw failure(p, "cannot read");
  return out;
}

}  // namespace

std::optional<StoreKey> store_key_of(const std::string& hex) {
  if (hex.size() != 64) return std::nullopt;
  StoreKey k{};
  auto digit = [](char c) -> int {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
  };
  for (std::size_t i = 0; i < 32; ++i) {
    int hi = digit(hex[2 * i]), lo = digit(hex[2 * i + 1]);
    if (hi < 0 || lo < 0) return std::nullopt;
    k[i] = static_cast<unsigned char>(hi * 16 + lo);
  }
  return k;
}

namespace {

// A connection to the store at path, its key given first, checked.
std::shared_ptr<Connection> connect(const fs::path& path, const std::optional<StoreKey>& key) {
  auto conn = std::make_shared<Connection>();
  sqlite3*& db = conn->db;
  if (sqlite3_open_v2(path_utf8(path).c_str(), &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, nullptr) != SQLITE_OK)
    throw failure(path, db_error(db));
  if (key) {
    // a raw key, the first statement on the connection (no key derivation); the text is cleared
    static const char* HEX = "0123456789abcdef";
    std::string sql = "PRAGMA key = \"x'";
    for (unsigned char b : *key) {
      sql += HEX[b >> 4];
      sql += HEX[b & 15];
    }
    sql += "'\";";
    int rc = sqlite3_exec(db, sql.c_str(), nullptr, nullptr, nullptr);
    std::fill(sql.begin(), sql.end(), '\0');
    if (rc != SQLITE_OK) throw failure(path, "the key cannot be set");
  }
  sqlite3_busy_timeout(db, 30000);
  if (sqlite3_exec(db, "SELECT COUNT(*) FROM sqlite_master", nullptr, nullptr, nullptr) != SQLITE_OK)
    throw failure(path, "cannot be read: it is encrypted, or its key is wrong");
  return conn;
}

}  // namespace

Store::Store(const fs::path& path, const std::optional<StoreKey>& key, std::size_t chunk_values) : path_(path) {
  std::error_code ec;
  if (path.has_parent_path()) fs::create_directories(path.parent_path(), ec);
  conn_ = connect(path, key);
  sqlite3* db = conn_->db;
  exec(db, path, "PRAGMA journal_mode = WAL");
  exec(db, path,
       "CREATE TABLE IF NOT EXISTS store (name TEXT PRIMARY KEY, value TEXT NOT NULL);"
       "CREATE TABLE IF NOT EXISTS indexes (key TEXT PRIMARY KEY, meta TEXT NOT NULL, engine TEXT NOT NULL,"
       " written_at REAL NOT NULL);"
       "CREATE TABLE IF NOT EXISTS arrays (key TEXT NOT NULL, array TEXT NOT NULL, n_values INTEGER NOT NULL,"
       " PRIMARY KEY (key, array)) WITHOUT ROWID;"
       "CREATE TABLE IF NOT EXISTS chunks (key TEXT NOT NULL, array TEXT NOT NULL, n INTEGER NOT NULL,"
       " data BLOB NOT NULL, PRIMARY KEY (key, array, n)) WITHOUT ROWID;");
  {
    Stmt st(db, "INSERT OR IGNORE INTO store (name, value) VALUES ('format', ?1), ('chunk_values', ?2)");
    st.text(1, std::to_string(STORE_FORMAT)).text(2, std::to_string(std::max<std::size_t>(chunk_values, 1)));
    if (st.step() != SQLITE_DONE) throw failure(path, db_error(db));
  }
  auto get = [&](const char* name) -> std::optional<std::string> {
    Stmt st(db, "SELECT value FROM store WHERE name = ?1");
    st.text(1, name);
    if (st.step() != SQLITE_ROW) return std::nullopt;
    return st.column_text(0);
  };
  if (get("format") != std::to_string(STORE_FORMAT)) throw failure(path, "another index store format");
  auto c = get("chunk_values");
  std::size_t n = 0;
  if (c && !c->empty() && c->size() < 19 && std::all_of(c->begin(), c->end(), [](char x) { return x >= '0' && x <= '9'; }))
    n = static_cast<std::size_t>(std::stoull(*c));
  if (n == 0) throw failure(path, "no chunk size");
  chunk_ = n;
  // a connection of its own for the envelopes' chunks (Chunked), so that windows are read while an
  // index is packed (the store is in WAL mode: a reader is not blocked by a writer); it only reads
  chunks_ = connect(path, key);
  exec(chunks_->db, path, "PRAGMA query_only = ON");
  // what a writer or reader that ended without cleaning up left in the temporary folder: entries
  // untouched for a day (no build takes that long)
  auto now = std::chrono::duration_cast<std::chrono::nanoseconds>(
                 std::chrono::system_clock::now().time_since_epoch()).count();
  constexpr std::int64_t DAY_NS = 86400LL * 1000000000LL;
  std::vector<fs::path> old;
  for (fs::directory_iterator it(tmp_dir(), ec), end; !ec && it != end; it.increment(ec)) {
    std::int64_t m = mtime_ns(it->path());
    if (m > 0 && now - m > DAY_NS) old.push_back(it->path());
  }
  for (const auto& p : old) fs::remove_all(p, ec);
}

fs::path Store::tmp_dir() const {
  fs::path dir = path_.parent_path();
  return (dir.empty() ? fs::path(".") : dir) / "index.tmp";
}

std::optional<std::string> Store::engine_of(const std::string& key) const {
  std::lock_guard<std::mutex> l(conn_->m);
  Stmt st(conn_->db, "SELECT engine FROM indexes WHERE key = ?1");
  st.text(1, key);
  if (st.step() != SQLITE_ROW) return std::nullopt;
  return st.column_text(0);
}

std::optional<std::string> Store::meta(const std::string& key) const {
  std::lock_guard<std::mutex> l(conn_->m);
  Stmt st(conn_->db, "SELECT meta FROM indexes WHERE key = ?1");
  st.text(1, key);
  if (st.step() != SQLITE_ROW) return std::nullopt;
  return st.column_text(0);
}

std::vector<std::string> Store::keys() const {
  std::lock_guard<std::mutex> l(conn_->m);
  Stmt st(conn_->db, "SELECT key FROM indexes ORDER BY key");
  std::vector<std::string> out;
  while (st.step() == SQLITE_ROW) out.push_back(st.column_text(0));
  return out;
}

namespace {

// Delete the rows of an index (within a transaction); how many indexes rows went.
int delete_rows(sqlite3* db, const fs::path& p, const std::string& key) {
  for (const char* sql : {"DELETE FROM chunks WHERE key = ?1", "DELETE FROM arrays WHERE key = ?1"}) {
    Stmt st(db, sql);
    st.text(1, key);
    if (st.step() != SQLITE_DONE) throw failure(p, db_error(db));
  }
  Stmt st(db, "DELETE FROM indexes WHERE key = ?1");
  st.text(1, key);
  if (st.step() != SQLITE_DONE) throw failure(p, db_error(db));
  return sqlite3_changes(db);
}

// A transaction, rolled back unless committed.
class Transaction {
 public:
  Transaction(sqlite3* db, const fs::path& p) : db_(db), p_(p) { exec(db, p, "BEGIN IMMEDIATE"); }
  ~Transaction() {
    if (!done_) sqlite3_exec(db_, "ROLLBACK", nullptr, nullptr, nullptr);
  }
  void commit() {
    exec(db_, p_, "COMMIT");
    done_ = true;
  }

 private:
  sqlite3* db_;
  fs::path p_;
  bool done_ = false;
};

}  // namespace

bool Store::remove(const std::string& key) const {
  std::lock_guard<std::mutex> l(conn_->m);
  Transaction tx(conn_->db, path_);
  int n = delete_rows(conn_->db, path_, key);
  tx.commit();
  return n > 0;
}

void Store::pack(const std::string& key, const fs::path& dir) const {
  std::string meta = read_file(dir / "meta.json");
  std::string engine;
  {
    Value v;
    try {
      v = Value::from_json(meta);
    } catch (const std::exception&) {
    }
    const Dict* d = v.as_dict();
    if (!d) throw failure(dir, "meta.json is not a JSON object");
    const Value* e = d->get("engine");
    if (e && e->as_str()) engine = *e->as_str();
  }
  std::vector<fs::path> files;
  for (const auto& entry : fs::directory_iterator(dir))
    if (entry.is_regular_file() && value_size(entry.path())) files.push_back(entry.path());
  std::sort(files.begin(), files.end());
  std::lock_guard<std::mutex> l(conn_->m);
  sqlite3* db = conn_->db;
  Transaction tx(db, path_);
  {
    Stmt st(db, "SELECT engine FROM indexes WHERE key = ?1");
    st.text(1, key);
    if (st.step() == SQLITE_ROW && st.column_text(0) == engine) return;  // the same content: kept
  }
  delete_rows(db, path_, key);
  double written =
      std::chrono::duration<double>(std::chrono::system_clock::now().time_since_epoch()).count();
  {
    Stmt st(db, "INSERT INTO indexes (key, meta, engine, written_at) VALUES (?1, ?2, ?3, ?4)");
    st.text(1, key).text(2, meta).text(3, engine).real(4, written);
    if (st.step() != SQLITE_DONE) throw failure(path_, db_error(db));
  }
  Stmt arrays(db, "INSERT INTO arrays (key, array, n_values) VALUES (?1, ?2, ?3)");
  Stmt chunks(db, "INSERT INTO chunks (key, array, n, data) VALUES (?1, ?2, ?3, ?4)");
  for (const auto& f : files) {
    std::string name = path_utf8(f.filename());
    std::size_t size = value_size(f);
    std::string bytes = read_file(f);
    if (bytes.size() % size != 0) throw failure(f, "not a whole number of values");
    std::size_t len = bytes.size() / size;
    arrays.reset();
    arrays.text(1, key).text(2, name).integer(3, static_cast<std::int64_t>(len));
    if (arrays.step() != SQLITE_DONE) throw failure(path_, db_error(db));
    // every array has a row, an empty one a chunk of no bytes
    for (std::size_t i = 0, n = chunk_count(len, chunk_); i < n; ++i) {
      chunks.reset();
      chunks.text(1, key).text(2, name).integer(3, static_cast<std::int64_t>(i));
      chunks.blob(4, bytes.data() + i * chunk_ * size, chunk_bytes(len, chunk_, i, size));
      if (chunks.step() != SQLITE_DONE) throw failure(path_, db_error(db));
    }
  }
  tx.commit();
}

std::shared_ptr<const std::vector<float>> Chunked::chunk_at(std::size_t n) const {
  std::lock_guard<std::mutex> l(m_);
  for (const auto& [i, v] : cache_)
    if (i == n) return v;
  std::string bytes;
  {
    std::lock_guard<std::mutex> c(conn_->m);
    Stmt st(conn_->db, "SELECT data FROM chunks WHERE key = ?1 AND array = ?2 AND n = ?3");
    st.text(1, key_).text(2, array_).integer(3, static_cast<std::int64_t>(n));
    if (st.step() != SQLITE_ROW) return nullptr;
    bytes = st.column_blob(0);
  }
  // a chunk of another length than its array's implies is not read
  if (bytes.size() != chunk_bytes(len_, chunk_, n, sizeof(float))) return nullptr;
  auto v = std::make_shared<const std::vector<float>>(values_of<float>(bytes));
  if (cache_.size() >= CACHED_CHUNKS) cache_.pop_front();
  cache_.emplace_back(n, v);
  return v;
}

std::optional<std::vector<float>> Chunked::range(std::size_t o, std::size_t n) const {
  if (n == 0) return std::vector<float>();
  if (o > len_ || n > len_ - o) return std::nullopt;
  std::vector<float> out;
  out.reserve(n);
  std::size_t first = o / chunk_, last = (o + n - 1) / chunk_;
  for (std::size_t c = first; c <= last; ++c) {
    auto v = chunk_at(c);
    if (!v) return std::nullopt;
    std::size_t start = c == first ? o - c * chunk_ : 0;
    std::size_t end = c == last ? o + n - c * chunk_ : v->size();
    if (end > v->size() || start > end) return std::nullopt;
    out.insert(out.end(), v->begin() + static_cast<std::ptrdiff_t>(start), v->begin() + static_cast<std::ptrdiff_t>(end));
  }
  return out;
}

std::optional<std::pair<Dict, Loaded>> Store::load(const std::string& key, std::string& why) const {
  try {
    auto text = meta(key);
    if (!text) {
      why = "no index";
      return std::nullopt;
    }
    Value v;
    try {
      v = Value::from_json(*text);
    } catch (const std::exception&) {
    }
    Dict* d = v.as_dict();
    if (!d) {
      why = "meta is not a JSON object";
      return std::nullopt;
    }
    const Value* iv = d->get("index_version");
    if (!iv || !iv->is_int() || int_of(iv) != std::optional<std::int64_t>(STORE_FORMAT)) {
      why = "another index version";
      return std::nullopt;
    }
    // the length of every array, from the arrays table (never from the chunks' lengths)
    std::map<std::string, std::size_t> lengths;
    {
      std::lock_guard<std::mutex> l(conn_->m);
      Stmt st(conn_->db, "SELECT array, n_values FROM arrays WHERE key = ?1");
      st.text(1, key);
      while (st.step() == SQLITE_ROW) {
        std::int64_t n = st.column_int(1);
        if (n < 0) {
          why = st.column_text(0) + ": a negative number of values";
          return std::nullopt;
        }
        lengths[st.column_text(0)] = static_cast<std::size_t>(n);
      }
    }
    // the bytes of a whole array, each chunk of the length its array implies
    auto whole = [&](const std::string& name) -> std::optional<std::string> {
      auto it = lengths.find(name);
      if (it == lengths.end()) {
        why = name + ": missing";
        return std::nullopt;
      }
      std::size_t size = value_size(name), len = it->second;
      std::string out;
      std::size_t i = 0;
      std::lock_guard<std::mutex> l(conn_->m);
      Stmt st(conn_->db, "SELECT n, data FROM chunks WHERE key = ?1 AND array = ?2 ORDER BY n");
      st.text(1, key).text(2, name);
      while (st.step() == SQLITE_ROW) {
        std::string part = st.column_blob(1);
        if (st.column_int(0) != static_cast<std::int64_t>(i) || part.size() != chunk_bytes(len, chunk_, i, size)) {
          why = name + ": chunks that do not hold its " + std::to_string(len) + " values";
          return std::nullopt;
        }
        out += part;
        ++i;
      }
      if (i != chunk_count(len, chunk_)) {
        why = name + ": chunks that do not hold its " + std::to_string(len) + " values";
        return std::nullopt;
      }
      return out;
    };
    Loaded a;
    for (std::size_t lvl = 0;; ++lvl) {
      std::string lo = "env_min_" + std::to_string(lvl) + ".f32", hi = "env_max_" + std::to_string(lvl) + ".f32";
      auto lo_it = lengths.find(lo);
      if (lo_it == lengths.end()) break;
      auto hi_it = lengths.find(hi);
      if (hi_it == lengths.end()) {
        why = hi + ": missing";
        return std::nullopt;
      }
      a.env_min.emplace_back(std::make_shared<const Chunked>(chunks_, key, lo, lo_it->second, chunk_));
      a.env_max.emplace_back(std::make_shared<const Chunked>(chunks_, key, hi, hi_it->second, chunk_));
    }
    auto mk_piece = whole("mk_piece.i32");
    if (!mk_piece) return std::nullopt;
    auto mk_sample = whole("mk_sample.i64");
    if (!mk_sample) return std::nullopt;
    auto mk_code = whole("mk_code.i16");
    if (!mk_code) return std::nullopt;
    auto vit_t = whole("vit_t.f64");
    if (!vit_t) return std::nullopt;
    auto vit_v = whole("vit_v.f64");
    if (!vit_v) return std::nullopt;
    a.mk_piece = Values<std::int32_t>(values_of<std::int32_t>(*mk_piece));
    a.mk_sample = Values<std::int64_t>(values_of<std::int64_t>(*mk_sample));
    a.mk_code = Values<std::int16_t>(values_of<std::int16_t>(*mk_code));
    a.vit_t = Values<double>(values_of<double>(*vit_t));
    a.vit_v = Values<double>(values_of<double>(*vit_v));
    why = check(*d, a);
    if (!why.empty()) return std::nullopt;
    // an adapter reads samples again from its index folder (protocol 1): a private folder holding
    // only its meta.json, for as long as the index is loaded
    a.dir = tmp_dir();
    const Value* ad = d->get("adapter");
    if (ad && ad->as_str() && !ad->as_str()->empty()) {
      private_dir(tmp_dir());
      auto reading = std::make_shared<Removed>(Removed{unique_dir(tmp_dir(), key, "read")});
      private_dir(reading->path);
      std::ofstream f(reading->path / "meta.json", std::ios::binary);
      f << *text;
      f.close();
      if (!f) throw failure(reading->path, "cannot write meta.json");
      a.dir = reading->path;
      a.reading = reading;
    }
    return std::make_pair(std::move(*d), std::move(a));
  } catch (const std::exception& e) {
    why = e.what();
    return std::nullopt;
  }
}

std::string build_into_store(const fs::path& path, const Store& store, const Progress& progress,
                             const Cancelled& cancelled) {
  std::string key = cache_key_of(path, STORE_FORMAT);
  auto lock = key_lock(path_utf8(store.path()) + "\x01" + key);
  std::lock_guard<std::mutex> writing(*lock);
  progress(0.0);
  fs::path tmp_root = store.tmp_dir();
  private_dir(tmp_root);
  Removed tmp{unique_dir(tmp_root, key, "tmp")};
  build_files(path, tmp.path, key, STORE_FORMAT, progress, cancelled);
  store.pack(key, tmp.path);
  progress(1.0);
  return key;
}

}  // namespace vitalstream::engine
