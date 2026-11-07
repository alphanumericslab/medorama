// The engine's limits and checks, as libs/engine/tests/{wfdb,edf,roots,adapters}.rs: JSON nesting,
// numbers in files, marker names and pieces bounded, a header stating more samples than its file
// holds, rates that are not finite, a collection that skips what it cannot read, files outside the
// data roots, writers of one key, an index that disagrees with its arrays, many signals read in
// chunks, a folder's key with links, the settings file, and adapters held to their limits
// (docs/adapters.md, "Limits and environment").
#include <gtest/gtest.h>

#include <chrono>
#include <cmath>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <functional>
#include <mutex>
#include <thread>

#ifndef _WIN32
#include <unistd.h>
#endif

#include "core/text.hpp"
#include "core/value.hpp"
#include "engine/adapters.hpp"
#include "engine/common.hpp"
#include "engine/dataset.hpp"
#include "engine/index.hpp"
#include "engine/sources.hpp"
#include "settings.hpp"
#include "needs_wfdb.hpp"

using namespace vitalstream::engine;
using namespace vitalstream::viewer;
namespace fs = std::filesystem;

namespace {

fs::path temp_dir(const char* name) {
  fs::path d = fs::temp_directory_path() / name;
  fs::remove_all(d);
  fs::create_directories(d);
  return fs::canonical(d);
}

void write(const fs::path& p, const std::string& text) {
  fs::create_directories(p.parent_path());
  std::ofstream(p, std::ios::binary) << text;
}

// A single-signal record name in dir: its header line head and 16-bit samples, with their checksum
// (wfdb-x leaves out a last sample that does not match it).
void record(const fs::path& dir, const std::string& name, const std::string& head, const std::vector<std::int16_t>& x,
            const std::string& dat = "") {
  std::int16_t sum = 0;
  for (auto v : x) sum = static_cast<std::int16_t>(sum + v);
  std::string file = dat.empty() ? name + ".dat" : dat;
  write(dir / (name + ".hea"), head + "\n" + file + " 16 200/mV 16 0 0 " + std::to_string(sum) + " 0 II\n");
  if (dat.empty()) {
    std::string bytes;
    for (auto v : x) {
      bytes += static_cast<char>(v & 0xff);
      bytes += static_cast<char>((v >> 8) & 0xff);
    }
    write(dir / file, bytes);
  }
}

Dict meta_of(const fs::path& path, const fs::path& cache) {
  std::string key = build_index(path, cache, [](double) {});
  auto loaded = load_index(cache, key);
  EXPECT_TRUE(loaded);
  return loaded ? loaded->first : Dict();
}

const List& list(const Dict& m, const char* k) { return *m.get(k)->as_list(); }

// An EDF+D file of `runs` data records of one second, each starting a second after the end of the
// one before: one signal of spr samples per record (as libs/engine/tests/edf.rs).
void discontinuous(const fs::path& path, std::size_t runs, std::size_t spr, const std::string& duration) {
  auto field = [](const std::string& s, std::size_t w) { return s + std::string(w > s.size() ? w - s.size() : 0, ' '); };
  const std::size_t ann = 16;
  std::string h = field("0", 8) + field("X X X X", 80) + field("Startdate 01-MAR-2024 X X X", 80) + "01.03.2413.00.00" +
                  field("768", 8) + field("EDF+D", 44) + field(std::to_string(runs), 8) + field(duration, 8) +
                  field("2", 4) + field("ECG", 16) + field("EDF Annotations", 16) + field("", 160) + field("mV", 8) +
                  field("", 8) + field("-1", 8) + field("-1", 8) + field("1", 8) + field("1", 8) + field("-32768", 8) +
                  field("-32768", 8) + field("32767", 8) + field("32767", 8) + field("", 160) +
                  field(std::to_string(spr), 8) + field(std::to_string(ann), 8) + field("", 64);
  std::string b = h;
  for (std::size_t r = 0; r < runs; ++r) {
    for (std::size_t i = 0; i < spr; ++i) {
      b += static_cast<char>(i & 0xff);
      b += static_cast<char>(0);
    }
    std::string tal = "+" + std::to_string(2 * r) + "\x14\x14";
    tal += '\0';
    tal.resize(2 * ann, '\0');
    b += tal;
  }
  write(path, b);
}

}  // namespace

TEST(Limits, JsonNestingIsBounded) {
  auto deep = [](std::size_t n) { return std::string(n, '[') + "1" + std::string(n, ']'); };
  EXPECT_NO_THROW(Value::from_json(deep(128)));
  try {
    Value::from_json(deep(129));
    FAIL();
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("nested too deeply"), std::string::npos) << e.what();
  }
  // deep enough to overflow the stack without the limit
  EXPECT_THROW(Value::from_json(std::string(1000000, '[')), std::runtime_error);
  std::string objects;
  for (int i = 0; i < 200000; ++i) objects += "{\"a\":";
  EXPECT_THROW(Value::from_json(objects), std::runtime_error);
}

TEST(Limits, NumbersInFiles) {
  // one sign at most, and the separators 0x1c-0x1f are not whitespace here (as in Python)
  EXPECT_TRUE(std::isnan(*vitalstream::core::py_float("+nan")));
  EXPECT_EQ(vitalstream::core::py_float("+Infinity"), HUGE_VAL);
  EXPECT_EQ(vitalstream::core::py_float("\xc2\xa0" "2\n"), 2.0);
  for (const char* bad : {"+-nan", "--inf", "-+inf", "1\x1c", "\x1f" "1", "0x10", "--1"})
    EXPECT_FALSE(vitalstream::core::py_float(bad)) << bad;
}

TEST(Limits, MarkerNamesAndPiecesAreBounded) {
  Pieces p;
  std::size_t id = p.start(0, 1.0, Dict());
  for (std::size_t k = 0; k < MAX_MARKER_NAMES + 10; ++k)
    p.marker(id, static_cast<std::int64_t>(k), "m" + std::to_string(k));
  EXPECT_TRUE(p.markers_capped());
  auto fin = p.finish();
  ASSERT_EQ(fin.marker_names.size(), MAX_MARKER_NAMES);
  EXPECT_EQ(fin.marker_names.back(), OTHER_MARKERS);
  EXPECT_EQ(fin.arrays.mk_code[MAX_MARKER_NAMES - 2], static_cast<std::int16_t>(MAX_MARKER_NAMES - 2));
  for (std::size_t k = MAX_MARKER_NAMES - 1; k < fin.arrays.mk_code.size(); ++k) EXPECT_EQ(fin.arrays.mk_code[k], 32767);
  Pieces q;
  EXPECT_NO_THROW(q.room(MAX_PIECES));
  EXPECT_THROW(q.room(MAX_PIECES + 1), std::runtime_error);
  // a NaN rate (an index written before rates were checked) does not break the sort
  std::vector<Dict> pieces;
  for (auto [t0, fs] : {std::pair{5.0, std::nan("")}, {0.0, 1.0}, {std::nan(""), 1.0}})
    pieces.push_back(Dict{{"t0", Value(t0)}, {"n", Value(10)}, {"fs", Value(fs)}, {"timed", Value(false)}});
  EXPECT_FALSE(untimed_spans(pieces).empty());
}

TEST(Limits, AHeaderStatingTooMuchIsReadToTheEndOfItsData) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_limits_toomuch");
  record(dir, "r", "r 1 250 1000000000000", {1, 2, 3, 4, 5, 6, 7, 8});
  auto t = std::chrono::steady_clock::now();
  Dict m = meta_of(dir / "r.hea", dir / "cache");
  EXPECT_LT(std::chrono::steady_clock::now() - t, std::chrono::seconds(10));
  EXPECT_EQ(int_of(list(m, "pieces")[0].get("n")), 8);
  Dict scanned = scan(dir / "r.hea", Limits{});
  EXPECT_EQ(f64_of(scanned.get("duration_s")), 8.0 / 250.0);
  fs::remove_all(dir);
}

TEST(Limits, ARateThatIsNotANumberIsRefused) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_limits_nan");
  record(dir / "f", "a", "a 1 -nan 4", {1, 2, 3, 4});
  record(dir / "f", "b", "b 1 250 4", {1, 2, 3, 4});
  try {
    build_index(dir / "f" / "a.hea", dir / "cache", [](double) {});
    FAIL();
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("no sampling frequency"), std::string::npos) << e.what();
  }
  Dict m = meta_of(dir / "f", dir / "cache");
  EXPECT_EQ(list(m, "pieces").size(), 1u);
  EXPECT_TRUE(m.get("skipped")->as_dict()->get("a"));
  // an EDF rate that is not finite, and more pieces than a recording may have
  discontinuous(dir / "tiny.edf", 2, 4, "1e-320");
  try {
    build_index(dir / "tiny.edf", dir / "cache", [](double) {});
    FAIL();
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("no finite sampling rate"), std::string::npos) << e.what();
  }
  discontinuous(dir / "too_many.edf", MAX_PIECES + 1, 1, "1");
  try {
    build_index(dir / "too_many.edf", dir / "cache", [](double) {});
    FAIL();
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("more than 100000 pieces"), std::string::npos) << e.what();
  }
  fs::remove_all(dir);
}

TEST(Limits, ManyBreaksAreBounded) {
  // more pieces than px in a window: one block per pixel column at most; a narrow window: every
  // piece in it, in order
  fs::path dir = temp_dir("vs_limits_runs");
  discontinuous(dir / "runs.edf", 3000, 4, "1");
  std::string key = build_index(dir / "runs.edf", dir / "cache", [](double) {});
  auto loaded = load_index(dir / "cache", key);
  ASSERT_TRUE(loaded);
  Dataset ds(std::move(loaded->first), std::move(loaded->second));
  ASSERT_EQ(ds.pieces.size(), 3000u);
  double t0 = ds.pieces[0].t0;
  Value w = Value::from_json(ds.window_json({0}, t0, t0 + 6000.0, 100));
  std::size_t blocks = w.get("channels")->get("0")->as_list()->size();
  EXPECT_TRUE(blocks <= 100 && blocks >= 90) << blocks;
  w = Value::from_json(ds.window_json({0}, t0 + 10.0, t0 + 20.0, 400));
  std::vector<std::int64_t> ids;
  for (const auto& b : *w.get("channels")->get("0")->as_list()) ids.push_back(*int_of(b.get("piece")));
  EXPECT_EQ(ids, (std::vector<std::int64_t>{5, 6, 7, 8, 9, 10}));
  // times far outside the recording: no data, no failure
  EXPECT_NO_THROW(ds.window_json({0}, -1e308, 1e308, 1200));
  EXPECT_NO_THROW(ds.markers({0}, -1e308, 1e308));
  fs::remove_all(dir);
}

TEST(Limits, ACollectionSkipsWhatItCannotRead) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_limits_coll");
  fs::path c = dir / "c";
  record(c, "good", "good 1 250 4", {1, 2, 3, 4});
  record(dir / "elsewhere", "far", "far 1 250 4", {1, 2, 3, 4});
  write(c / "index.csv", "name,start_unix\ngood,0\n");
  try {
    build_index(c, dir / "cache", [](double) {});
    FAIL();
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("no record column"), std::string::npos) << e.what();
  }
  std::string far = (dir / "elsewhere" / "far").string();
  write(c / "index.csv", "record,start_unix\ngood,0\nmissing,0\n../elsewhere/far,0\n" + far + ",0\n,0\n");
  Dict m = meta_of(c, dir / "cache");
  EXPECT_EQ(list(m, "pieces").size(), 1u);
  std::vector<std::string> names;
  for (const auto& e : *m.get("skipped")->as_dict()) names.push_back(e.key);
  EXPECT_EQ(names, (std::vector<std::string>{"missing", "../elsewhere/far", far, "row 5"}));
  EXPECT_NE(m.get("skipped")->get("../elsewhere/far")->py_str().find("outside its folder"), std::string::npos);
  bool noted = false;
  for (const auto& n : list(m, "notes")) noted = noted || n.py_str().find("4 records could not be read") != std::string::npos;
  EXPECT_TRUE(noted);
  fs::remove_all(dir);
}

#ifndef _WIN32
TEST(Limits, FilesOutsideTheRootsAreNotRead) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_limits_roots");
  fs::path root = dir / "root", outside = dir / "outside";
  write(outside / "secret.dat", std::string("\1\0\2\0\3\0\4\0", 8));
  record(root, "good", "good 1 250 4", {1, 2, 3, 4});
  // a link to a file outside, and a header naming one by its absolute path
  write(root / "linked.hea", "linked 1 250 4\nlinked.dat 16 200/mV 16 0 0 10 0 II\n");
  fs::create_symlink(outside / "secret.dat", root / "linked.dat");
  write(root / "absolute.hea", "absolute 1 250 4\n" + (outside / "secret.dat").string() + " 16 200/mV 16 0 0 10 0 II\n");
  fs::path cache = dir / "cache";
  set_allowed_roots(std::nullopt);  // a library's default: anything is read
  EXPECT_NO_THROW(build_index(root / "linked.hea", cache, [](double) {}));
  set_allowed_roots(std::vector<std::string>{root.string()});
  for (const char* name : {"linked", "absolute"}) {
    try {
      build_index(root / (std::string(name) + ".hea"), cache, [](double) {});
      ADD_FAILURE() << name;
    } catch (const std::exception& e) {
      EXPECT_NE(std::string(e.what()).find("outside the data roots"), std::string::npos) << e.what();
    }
  }
  std::string key = build_index(root, cache, [](double) {});
  auto loaded = load_index(cache, key);
  ASSERT_TRUE(loaded);
  std::vector<std::string> skipped;
  for (const auto& e : *loaded->first.get("skipped")->as_dict()) skipped.push_back(e.key);
  EXPECT_EQ(skipped, (std::vector<std::string>{"absolute", "linked"}));
  // reading samples checks again: a signal file replaced by a link afterwards is not read
  Dataset ds(std::move(loaded->first), std::move(loaded->second));
  EXPECT_EQ(ds.samples(0, 0, 4).size(), 4u);
  fs::remove(root / "good.dat");
  fs::create_symlink(outside / "secret.dat", root / "good.dat");
  try {
    ds.samples(0, 0, 4);
    ADD_FAILURE();
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("outside the data roots"), std::string::npos) << e.what();
  }
  set_allowed_roots(std::nullopt);
  fs::remove_all(dir);
}

TEST(Limits, AFoldersKeyFollowsLinkedFiles) {
  // a link to a file counts with its target's size and time; links to folders do not
  fs::path dir = temp_dir("vs_limits_links");
  fs::path f = dir / "f";
  record(f, "a", "a 1 250 4", {1, 2, 3, 4});
  write(dir / "target.dat", std::string(8, '\0'));
  fs::create_symlink(dir / "target.dat", f / "b.dat");
  fs::create_directory_symlink(dir, f / "up");
  std::vector<std::string> names;
  for (const auto& x : fingerprint(f)) names.push_back(std::get<0>(x));
  EXPECT_EQ(names, (std::vector<std::string>{"a.dat", "a.hea", "b.dat"}));
  std::string k0 = cache_key(f);
  write(dir / "target.dat", std::string(10, '\0'));
  EXPECT_NE(cache_key(f), k0);
  fs::remove_all(dir);
}
#endif

TEST(Limits, WritersOfOneKeyDoNotCollide) {
  VS_NEEDS_WFDB();
  fs::path cache = temp_dir("vs_limits_writers");
  fs::path session = fs::path(VS_TESTDATA) / "session";
  std::vector<std::string> keys(4);
  std::vector<std::thread> ts;
  for (std::size_t i = 0; i < keys.size(); ++i) ts.emplace_back([&, i] { keys[i] = build_index(session, cache, [](double) {}); });
  for (auto& t : ts) t.join();
  for (const auto& k : keys) EXPECT_EQ(k, keys[0]);
  EXPECT_TRUE(load_index(cache, keys[0]));
  std::vector<std::string> names;
  for (const auto& e : fs::directory_iterator(cache)) names.push_back(e.path().filename().string());
  EXPECT_EQ(names, std::vector<std::string>{keys[0]});
  // building it again keeps the complete index there
  auto before = fs::last_write_time(cache / keys[0] / "meta.json");
  build_index(session, cache, [](double) {});
  EXPECT_EQ(fs::last_write_time(cache / keys[0] / "meta.json"), before);
  // a failure while building leaves nothing
  fs::path other = temp_dir("vs_limits_failure");
  EXPECT_THROW(build_index_until(fs::path(VS_TESTDATA) / "plain", other, [](double) {},
                                 []() -> bool { throw std::logic_error("a reader's bug"); }),
               std::logic_error);
  EXPECT_TRUE(fs::is_empty(other));
  fs::remove_all(cache);
  fs::remove_all(other);
}

TEST(Limits, AnIndexThatDisagreesWithItsArraysIsNotUsed) {
  VS_NEEDS_WFDB();
  fs::path cache = temp_dir("vs_limits_check");
  std::string key = build_index(fs::path(VS_TESTDATA) / "plain", cache, [](double) {});
  fs::path folder = cache / key;
  std::ifstream in(folder / "meta.json", std::ios::binary);
  std::string text((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
  in.close();
  Value meta = Value::from_json(text);
  auto with = [&](const std::function<void(Dict&)>& f) {
    Value m = meta;
    f(*m.as_dict());
    write(folder / "meta.json", m.to_json());
    return load_index(cache, key).has_value();
  };
  auto piece = [](Dict& m, const char* k, Value v) { (*m.get("pieces")->as_list())[0].as_dict()->insert(k, std::move(v)); };
  EXPECT_TRUE(with([](Dict&) {}));
  // an envelope beyond its array, a rate of 0, a missing start, an unknown channel, a vital-sign
  // series beyond its array
  EXPECT_FALSE(with([&](Dict& m) { piece(m, "env", Value(List{Value(List{Value(0), Value(1000000)})})); }));
  EXPECT_FALSE(with([&](Dict& m) { piece(m, "fs", Value(0.0)); }));
  EXPECT_FALSE(with([&](Dict& m) { piece(m, "t0", Value()); }));
  EXPECT_FALSE(with([&](Dict& m) { piece(m, "ch", Value(99)); }));
  EXPECT_FALSE(with([](Dict& m) {
    m.insert("vitals", Value(List{Value(Dict{{"name", Value("x")}, {"offset", Value(5)}, {"n", Value(1)}})}));
  }));
  // an envelope level missing (an index half replaced)
  EXPECT_TRUE(with([](Dict&) {}));
  fs::remove(folder / "env_max_0.f32");
  EXPECT_FALSE(load_index(cache, key));
  fs::remove_all(cache);
}

TEST(Limits, ManySignalsAreReadAChunkAtATime) {
  VS_NEEDS_WFDB();
  // 300 signals, 5000 frames: decoded in chunks of 4096 frames, every value in place
  fs::path dir = temp_dir("vs_limits_wide");
  const std::size_t nsig = 300, n = 5000;
  std::string head = "w " + std::to_string(nsig) + " 100 " + std::to_string(n) + "\n";
  for (std::size_t k = 0; k < nsig; ++k) {
    std::int16_t sum = 0;
    for (std::size_t i = 0; i < n; ++i) sum = static_cast<std::int16_t>(sum + static_cast<std::int16_t>(i + k));
    head += "w.dat 16 1/mV 16 0 0 " + std::to_string(sum) + " 0 s" + std::to_string(k) + "\n";
  }
  write(dir / "w.hea", head);
  std::string bytes;
  for (std::size_t i = 0; i < n; ++i)
    for (std::size_t k = 0; k < nsig; ++k) {
      auto v = static_cast<std::uint16_t>(i + k);
      bytes += static_cast<char>(v & 0xff);
      bytes += static_cast<char>(v >> 8);
    }
  write(dir / "w.dat", bytes);
  std::string key = build_index(dir / "w.hea", dir / "cache", [](double) {});
  auto loaded = load_index(dir / "cache", key);
  ASSERT_TRUE(loaded);
  Dataset ds(std::move(loaded->first), std::move(loaded->second));
  ASSERT_EQ(ds.pieces.size(), nsig);
  auto x = ds.samples(299, 4090, 4100);
  for (std::size_t i = 0; i < x.size(); ++i) EXPECT_EQ(x[i], static_cast<double>(4090 + i + 299));
  // a window of 64 channels reads the record once and answers each
  std::vector<std::int64_t> chs;
  for (std::int64_t c = 0; c < 64; ++c) chs.push_back(c);
  double t0 = ds.pieces[0].t0;
  Value w = Value::from_json(ds.window_json(chs, t0 + 40.0, t0 + 41.0, 400));
  const Value& block = (*w.get("channels")->get("63")->as_list())[0];
  EXPECT_EQ(block.get("kind")->py_str(), "raw");
  EXPECT_EQ(f64_of(&(*block.get("y")->as_list())[1]), 4000.0 + 63);
  fs::remove_all(dir);
}

#ifndef _WIN32
TEST(Limits, TheSettingsFile) {
  // a vitalstream.toml in the working directory is not read (it could run any program through
  // adapters_dir, or open the viewer to the network)
  fs::path dir = temp_dir("vs_limits_settings");
  fs::path toml = dir / "vitalstream.toml";
  write(toml, "adapters_dir = \"ad\"\nhost = \"0.0.0.0\"\n");
  fs::path cwd = fs::current_path();
  fs::current_path(dir);
  Settings s = load_settings(std::nullopt);
  fs::current_path(cwd);
  EXPECT_NE(s.source, std::optional<fs::path>(toml));
  EXPECT_NE(s.adapters_dir, dir / "ad");
  // named with --config, it is read; wrong types are refused
  s = load_settings(toml.string());
  EXPECT_EQ(s.adapters_dir, dir / "ad");
  EXPECT_EQ(s.host, "0.0.0.0");
  EXPECT_EQ(s.source, std::optional<fs::path>(toml));
  write(toml, "allowed_hosts = [\"portal.example.org\"]\nsecure_cookie = true\n");
  s = load_settings(toml.string());
  EXPECT_EQ(s.allowed_hosts, std::vector<std::string>{"portal.example.org"});
  EXPECT_TRUE(s.secure_cookie);
  for (const char* bad : {"data_roots = \"/\"", "port = \"8060\"", "port = 70000", "index_workers = -1",
                          "allowed_hosts = \"x\"", "allowed_hosts = [\" \"]", "qc_verbose = 1", "secure_cookie = \"yes\""}) {
    write(toml, bad);
    try {
      load_settings(toml.string());
      ADD_FAILURE() << bad;
    } catch (const std::exception& e) {
      EXPECT_NE(std::string(e.what()).find("bad value"), std::string::npos) << bad << ": " << e.what();
    }
  }
  fs::remove_all(dir);
}

// --- adapters held to their limits (with libs/engine/tests/misbehaving_adapter.py) ------------

namespace {

bool have_python() { return std::system("python3 -c pass >/dev/null 2>&1") == 0; }

// The misbehaving adapter installed as `name` (its mode) in folder.
fs::path install(const fs::path& folder, const std::string& name) {
  fs::create_directories(folder);
  fs::path p = folder / name;
  fs::copy_file(VS_MISBEHAVING_ADAPTER, p, fs::copy_options::overwrite_existing);
  fs::permissions(p, fs::perms::owner_all, fs::perm_options::add);
  return p;
}

adapters::Timeouts shortened() {
  adapters::Timeouts t;
  t.describe = 2;
  t.index_stall = 1;
  t.serve_stall = 1;
  return t;
}

std::string message(const std::function<void()>& f) {
  try {
    f();
  } catch (const std::exception& e) {
    return e.what();
  }
  return "(no error)";
}

}  // namespace

TEST(Adapters, ANoisyAdapterDoesNotBlock) {
  if (!have_python()) GTEST_SKIP() << "no python3";
  adapters::set_timeouts({});
  fs::path dir = temp_dir("vs_limits_noisy");
  auto a = adapters::describe(install(dir / "ad", "noisy-adapter"));
  // 300 kB on standard error before its first progress line
  write(dir / "r.noisy", "fine");
  fs::create_directories(dir / "out");
  std::vector<double> seen;
  adapters::index(a, dir / "r.noisy", dir / "out", [&](double f) { seen.push_back(f); });
  EXPECT_EQ(seen, std::vector<double>{0.5});
  // a failure: the message ends with the last line of its standard error, nothing more
  write(dir / "r.noisy", "fail");
  std::string e = message([&] { adapters::index(a, dir / "r.noisy", dir / "out", [](double) {}); });
  EXPECT_TRUE(e.size() < 500 && e.size() > 16 && e.compare(e.size() - 16, 16, ": the last words") == 0) << e;
  fs::remove_all(dir);
}

TEST(Adapters, AHangingAdapterIsStopped) {
  if (!have_python()) GTEST_SKIP() << "no python3";
  adapters::set_timeouts(shortened());
  fs::path dir = temp_dir("vs_limits_hang");
  auto t = std::chrono::steady_clock::now();
  std::string e = message([&] { adapters::describe(install(dir / "ad", "hangdescribe-adapter")); });
  EXPECT_NE(e.find("did not answer within 2 s"), std::string::npos) << e;
  auto a = adapters::describe(install(dir / "ad", "hang-adapter"));
  write(dir / "r.hang", "x");
  fs::create_directories(dir / "out");
  e = message([&] { adapters::index(a, dir / "r.hang", dir / "out", [](double) {}); });
  EXPECT_NE(e.find("no answer for 1 s"), std::string::npos) << e;
  // cancelling does not wait for a line from the adapter
  adapters::set_timeouts({});
  auto asked = std::chrono::steady_clock::now();
  e = message([&] {
    adapters::index(a, dir / "r.hang", dir / "out", [](double) {},
                    [&] { return std::chrono::steady_clock::now() - asked > std::chrono::milliseconds(300); });
  });
  EXPECT_EQ(e, "indexing cancelled");
  EXPECT_LT(std::chrono::steady_clock::now() - asked, std::chrono::seconds(5));
  EXPECT_LT(std::chrono::steady_clock::now() - t, std::chrono::seconds(20));
  fs::remove_all(dir);
}

TEST(Adapters, ALyingSessionIsStoppedAndStartedAgain) {
  if (!have_python()) GTEST_SKIP() << "no python3";
  adapters::set_timeouts(shortened());
  fs::path dir = temp_dir("vs_limits_liar");
  auto a = adapters::describe(install(dir / "ad", "liar-adapter"));
  std::vector<double> want;
  for (int i = 10; i < 20; ++i) want.push_back(i);
  EXPECT_EQ(adapters::read(a, dir, 0, 10, 20), want);
  // more samples than asked, an absurd count, no JSON: errors, and the session is stopped
  for (std::size_t piece : {1, 2, 4}) {
    std::string e = message([&] { adapters::read(a, dir, piece, 10, 20); });
    EXPECT_NE(e.find("its session was stopped"), std::string::npos) << piece << ": " << e;
    EXPECT_EQ(adapters::read(a, dir, 0, 10, 20), want) << "after piece " << piece;
  }
  // an error answered: the session stays in step
  EXPECT_EQ(message([&] { adapters::read(a, dir, 3, 0, 5); }), "no such piece");
  EXPECT_EQ(adapters::read(a, dir, 0, 10, 20), want);
  // no answer: stopped after the stall time
  std::string e = message([&] { adapters::read(a, dir, 5, 0, 5); });
  EXPECT_NE(e.find("no answer for 1 s"), std::string::npos) << e;
  EXPECT_EQ(adapters::read(a, dir, 0, 10, 20), want);
  // a long read is asked in requests of at most READ_CHUNK samples
  std::int64_t n = adapters::READ_CHUNK + 3;
  auto x = adapters::read(a, dir, 0, 0, n);
  ASSERT_EQ(static_cast<std::int64_t>(x.size()), n);
  EXPECT_EQ(x.back(), static_cast<double>(n - 1));
  adapters::set_timeouts({});
  fs::remove_all(dir);
}

TEST(Adapters, AnAdapterGetsAClearedEnvironment) {
  if (!have_python()) GTEST_SKIP() << "no python3";
  adapters::set_timeouts({});
  fs::path dir = temp_dir("vs_limits_env");
  // an adapter that states in its version what it sees of the environment
  fs::path p = dir / "ad" / "env-adapter";
  write(p, "#!/usr/bin/env python3\nimport json, os\nprint(json.dumps({'name': 'env', 'protocol': 1, 'formats': [], "
           "'version': os.environ.get('MEDORAMA_KEY', '') + '|' + str('PATH' in os.environ) + '|' + "
           "os.environ.get('LC_ALL', '')}))\n");
  fs::permissions(p, fs::perms::owner_all, fs::perm_options::add);
  ::setenv("MEDORAMA_KEY", "secret", 1);
  ::setenv("LC_ALL", "C", 1);
  auto a = adapters::describe(p);
  ::unsetenv("MEDORAMA_KEY");
  EXPECT_EQ(a.version, "|True|C");
  fs::remove_all(dir);
}
#endif
