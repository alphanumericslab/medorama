// The viewer's building blocks (envelopes, settings, password scheme), the
// synthetic recordings of testdata/ indexed and queried, WFDB records (one record, a
// multi-segment record, a folder of records read as one session, the limits on a folder, the key
// of a record, cancelling; as libs/engine/tests/wfdb.rs), and the adapter protocol with the mock
// adapter of the engine's tests.  Parity with the other viewers is checked by
// tests/parity/viewer_parity.py.
#include <gtest/gtest.h>

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <limits>

#include "engine/adapters.hpp"
#include "auth.hpp"
#include "engine/common.hpp"
#include "engine/dataset.hpp"
#include "engine/index.hpp"
#include "settings.hpp"
#include "engine/sources.hpp"
#include "core/text.hpp"
#include "needs_wfdb.hpp"

using namespace vitalstream::engine;
using namespace vitalstream::viewer;
namespace fs = std::filesystem;

TEST(Common, EnvelopeLevels) {
  std::vector<float> x(10007);
  for (std::size_t i = 0; i < x.size(); ++i) x[i] = static_cast<float>((i * 7919) % 1000) - 500.0f;
  for (std::size_t i = 100; i < 140; ++i) x[i] = std::nanf("");
  Envelope env;
  for (std::size_t a = 0; a < x.size(); a += 999) env.add(x.data() + a, std::min<std::size_t>(999, x.size() - a));
  auto levels = env.finish();
  ASSERT_EQ(levels[0].first.size(), (x.size() + LEVEL0 - 1) / LEVEL0);
  for (std::size_t b = 0; b < levels[0].first.size(); ++b) {
    float lo = std::numeric_limits<float>::infinity();
    bool any = false;
    for (std::size_t i = b * LEVEL0; i < std::min((b + 1) * LEVEL0, x.size()); ++i)
      if (!std::isnan(x[i])) {
        lo = std::min(lo, x[i]);
        any = true;
      }
    if (any)
      EXPECT_EQ(levels[0].first[b], lo);
    else
      EXPECT_TRUE(std::isnan(levels[0].first[b]));
  }
  EXPECT_EQ(levels.back().first.size(), 1u);
}

TEST(Settings, RootsAndLinks) {
  // inside the home folder: the viewer's root may be ~ (VS_ROOT)
  fs::path dir = fs::path(home()) / ".vs_settings_test";
  fs::remove_all(dir);
  fs::create_directories(dir / "data" / "a");
  fs::create_directories(dir / "outside");
  Settings s = Settings::defaults();
  s.data_roots = {(dir / "data").string()};
  s.finish();
  EXPECT_TRUE(s.allowed((dir / "data" / "a").string()));
  EXPECT_FALSE(s.allowed((dir / "outside").string()));
  EXPECT_FALSE(s.allowed((dir / "data" / ".." / "outside").string()));
#ifndef _WIN32
  // a link inside a root that leads outside it: refused
  fs::create_directory_symlink(dir / "outside", dir / "data" / "link");
  EXPECT_FALSE(s.allowed((dir / "data" / "link").string()));
  EXPECT_EQ(absolute("/a/b/../c/./d"), "/a/c/d");
#endif
  fs::remove_all(dir);
  EXPECT_TRUE(valid_timezone("America/New_York"));
  EXPECT_FALSE(valid_timezone("Mars/Olympus"));
}

TEST(Settings, TheViewersRoot) {
  // the default data root is the viewer's root; a wider one is narrowed to it, one outside it is
  // left out
  const std::string top = built_root();
  Settings s = Settings::defaults();
  s.finish();
  EXPECT_EQ(s.data_roots, std::vector<std::string>{top});
  EXPECT_TRUE(s.allowed(top));
  s = Settings::defaults();
  s.data_roots = {fs_root()};
  s.finish();
  EXPECT_EQ(s.data_roots, std::vector<std::string>{top});
  if (top != fs_root()) {
    std::string outside = (fs::temp_directory_path() / "vitalstream-outside").string();
    std::error_code ec;
    std::string real_tmp = fs::canonical(fs::temp_directory_path(), ec).string();
    std::string real_top = fs::canonical(top, ec).string();
    if (real_tmp.rfind(real_top + "/", 0) != 0) {
      Settings o = Settings::defaults();
      o.data_roots = {outside, top};
      o.finish();
      EXPECT_EQ(o.data_roots, std::vector<std::string>{top});
      EXPECT_EQ(o.outside, std::vector<std::string>{absolute(outside)});
      EXPECT_FALSE(o.allowed(outside));
    }
  }
}

#if VS_PASSWORD
TEST(Auth, ScryptTestVectors) {
  // RFC 7914, section 12
  auto a = scrypt("", {}, 16, 1, 1, 64);
  EXPECT_EQ(vitalstream::core::hex(a.data(), a.size()),
            "77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442"
            "fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906");
  std::string salt = "NaCl";
  auto b = scrypt("password", std::vector<std::uint8_t>(salt.begin(), salt.end()), 1024, 8, 16, 64);
  EXPECT_EQ(vitalstream::core::hex(b.data(), b.size()),
            "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b373162"
            "2eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640");
}

TEST(Auth, PasswordAndCookie) {
  fs::path p = fs::temp_directory_path() / "vs_auth_test" / "auth.json";
  fs::remove_all(p.parent_path());
  EXPECT_FALSE(load_auth(p));
  set_password(p, "correct horse");
  auto a = load_auth(p);
  ASSERT_TRUE(a);
  EXPECT_TRUE(check_password(*a, "correct horse"));
  EXPECT_FALSE(check_password(*a, "wrong"));
  auto c = make_cookie(*a);
  EXPECT_TRUE(cookie_ok(*a, c));
  EXPECT_FALSE(cookie_ok(*a, std::string("123.abc")));
  EXPECT_FALSE(cookie_ok(*a, std::nullopt));
  clear_password(p);
  EXPECT_FALSE(load_auth(p));
  fs::remove_all(p.parent_path());
}
#else
TEST(Auth, NoPasswordSupport) {
  // a build without password support: no password can be set, none is ever loaded
  fs::path p = fs::temp_directory_path() / "vs_auth_test" / "auth.json";
  EXPECT_FALSE(PASSWORD_SUPPORTED);
  EXPECT_THROW(set_password(p, "correct horse"), std::runtime_error);
  EXPECT_THROW(clear_password(p), std::runtime_error);
  EXPECT_FALSE(load_auth(p));
}
#endif

namespace {

fs::path temp_dir(const char* name) {
  fs::path d = fs::temp_directory_path() / name;
  fs::remove_all(d);
  fs::create_directories(d);
  return d;
}

std::vector<std::string> names_of(const Value& list) {
  std::vector<std::string> out;
  for (const auto& v : *list.as_list()) out.push_back(*v.get("name")->as_str());
  return out;
}

}  // namespace

TEST(Recordings, IndexAndQueryEveryRecording) {
  VS_NEEDS_WFDB();
  fs::path cache = temp_dir("vs_index_test");
  for (const char* name : {"collection", "plain"}) {
    fs::path path = fs::path(VS_TESTDATA) / name;
    std::string key = build_index(path, cache, [](double) {});
    auto loaded = load_index(cache, key);
    ASSERT_TRUE(loaded) << name;
    Dataset ds(std::move(loaded->first), std::move(loaded->second));
    Value info = ds.info();
    const List& range = *info.get("t_range")->as_list();
    double t0 = 0, t1 = 0;
    ASSERT_TRUE(range[0].as_f64(t0) && range[1].as_f64(t1));
    auto n_ch = static_cast<std::int64_t>(info.get("channels")->as_list()->size());
    std::vector<std::int64_t> chs;
    for (std::int64_t c = 0; c < n_ch; ++c) chs.push_back(c);
    for (auto [a, b] : {std::pair{t0, t1}, std::pair{t0, t0 + 2.0}, std::pair{t0 + 1.0, t0 + 1.2}}) {
      Value w = Value::from_json(ds.window_json(chs, a, b, 800));
      EXPECT_EQ(static_cast<std::int64_t>(w.get("channels")->as_dict()->size()), n_ch);
    }
    for (std::size_t p = 0; p < ds.pieces.size(); ++p)
      EXPECT_EQ(static_cast<std::int64_t>(ds.samples(p, 0, ds.pieces[p].n).size()), ds.pieces[p].n) << name << p;
    auto vitals = names_of(*info.get("vitals"));
    ds.vitals_window(vitals, t0, t1, 100);
    ds.vitals_at((t0 + t1) / 2);
    ds.markers(chs, t0, t1);
    EXPECT_EQ(*info.get("format")->as_str(), std::string(name) == "collection" ? "WFDB collection" : "WFDB records");
    if (std::string(name) == "plain") {
      // nothing assumed that a header does not state: ADC units for an uncalibrated signal, no
      // unit when none is stated, and the record without a sampling frequency left out
      std::vector<std::string> units;
      for (const auto& c : *info.get("channels")->as_list()) units.push_back(*c.get("unit")->as_str());
      std::sort(units.begin(), units.end());
      EXPECT_EQ(units, (std::vector<std::string>{"", "adu", "mV", "mmHg"}));
    }
    if (std::string(name) == "collection") {
      // three stretches; the last, without a start time, after the 0.5 s break stated before it
      std::vector<double> starts;
      for (const auto& p : ds.pieces)
        if (p.ch == 0) starts.push_back(p.t0);
      EXPECT_EQ(starts, (std::vector<double>{1709287200.0, 1709287205.5, 1709287209.0}));
      EXPECT_EQ(vitals, (std::vector<std::string>{"HR", "SpO2", "HR [import]"}))
          << "the invalid row left out; a second source named";
      const List& alarms = *info.get("alarms")->as_list();
      ASSERT_EQ(alarms.size(), 2u);
      EXPECT_EQ(*alarms[0].get("text")->as_str(), "HR HIGH 151 > 150");
      EXPECT_EQ(info.get("gaps")->as_list()->size(), 2u);
      std::string marks = ds.markers({0}, t0, t1).py_str();
      EXPECT_TRUE(marks.find("R wave") != std::string::npos && marks.find("'V'") != std::string::npos) << marks;
    }
  }
  fs::remove_all(cache);
}

namespace {

constexpr double T0 = 1709287200.0;

fs::path testdata() { return fs::path(VS_TESTDATA); }

Dict meta_of(const fs::path& path, const fs::path& cache) {
  std::string key = build_index(path, cache, [](double) {});
  auto loaded = load_index(cache, key);
  EXPECT_TRUE(loaded) << path;
  return loaded ? loaded->first : Dict();
}

const List& list_of(const Dict& m, const char* k) { return *m.get(k)->as_list(); }

std::string s_of(const Value& d, const char* k) {
  const Value* v = d.get(k);
  return v && v->as_str() ? *v->as_str() : std::string();
}

double f_of(const Value& d, const char* k) { return *f64_of(d.get(k)); }

std::vector<std::string> labels_of(const Dict& m) {
  std::vector<std::string> out;
  for (const auto& c : list_of(m, "channels")) out.push_back(s_of(c, "label"));
  return out;
}

}  // namespace

TEST(Wfdb, AFolderOfRecordsIsOneSession) {
  VS_NEEDS_WFDB();
  fs::path cache = temp_dir("vs_wfdb_session");
  Dict m = meta_of(testdata() / "session", cache);
  EXPECT_EQ(s_of(Value(m), "format"), "WFDB records");
  EXPECT_EQ(labels_of(m), (std::vector<std::string>{"ART", "II", "II (2)", "V"}));
  const List& pieces = list_of(m, "pieces");
  std::vector<std::string> recs;
  for (const auto& p : pieces) recs.push_back(s_of(p, "record"));
  // subfolders in name order; the segments of the multi-segment records, never on their own; the
  // hidden folder left out
  EXPECT_EQ(recs, (std::vector<std::string>{"abp/abp_a", "ecg/hr_a", "ecg/hr_b", "ecg/hr_c", "ms/ms_fix_0001",
                                            "ms/ms_fix_0002", "ms/ms_rec_0001", "ms/ms_rec_0001",
                                            "ms/ms_rec_0002"}));
  std::vector<double> t;
  for (const auto& p : pieces) t.push_back(f_of(p, "t0") - T0);
  EXPECT_EQ(t, (std::vector<double>{7200.0, 7200.0, 7201.0, 7204.0, 7205.0, 7206.0, 7210.0, 7210.0, 7212.5}));
  // the overlapping record is in a second lane
  EXPECT_EQ(int_of(pieces[2].get("ch")), std::optional<std::int64_t>(2));
  // without a start time: placed after the data before them
  std::vector<bool> timed;
  for (const auto& p : pieces) {
    const Value* v = p.get("timed");
    timed.push_back(v && v->as_bool() && *v->as_bool());
  }
  EXPECT_EQ(timed, (std::vector<bool>{true, true, true, false, false, false, true, true, true}));
  const List& gaps = list_of(m, "gaps");
  ASSERT_EQ(gaps.size(), 1u);
  EXPECT_EQ(s_of(gaps[0], "reason"), "null segment");
  EXPECT_EQ(f_of(gaps[0], "t0") - T0, 7212.0);
  EXPECT_EQ(f_of(gaps[0], "t1") - T0, 7212.5);
  bool lane_note = false;
  for (const auto& n : list_of(m, "notes"))
    if (n.as_str() && n.as_str()->find("second lane") != std::string::npos) lane_note = true;
  EXPECT_TRUE(lane_note);
  fs::remove_all(cache);
}

TEST(Wfdb, OneRecord) {
  VS_NEEDS_WFDB();
  fs::path cache = temp_dir("vs_wfdb_record");
  fs::path ms = testdata() / "session" / "ms";
  EXPECT_EQ(kind_of(ms / "ms_rec.hea"), std::optional<std::string>("wfdb"));
  EXPECT_STREQ(format_label(ms / "ms_rec.hea"), "WFDB multi-segment record");
  EXPECT_STREQ(format_label(ms / "ms_rec_0001.hea"), "WFDB record");

  // a multi-segment record: its segments from its start, the null segment a break, the markers
  // of the record in the segments they fall in (the one in the null segment left out)
  Dict m = meta_of(ms / "ms_rec.hea", cache);
  EXPECT_EQ(s_of(Value(m), "format"), "WFDB multi-segment record");
  std::vector<std::tuple<std::string, std::int64_t, double>> got;
  for (const auto& p : list_of(m, "pieces"))
    got.emplace_back(s_of(p, "record"), *int_of(p.get("n")), f_of(p, "t0") - T0);
  EXPECT_EQ(got, (std::vector<std::tuple<std::string, std::int64_t, double>>{
                     {"ms_rec_0001", 500, 7210.0}, {"ms_rec_0001", 500, 7210.0}, {"ms_rec_0002", 250, 7212.5}}));
  std::string key = build_index(ms / "ms_rec.hea", cache, [](double) {});
  auto loaded = load_index(cache, key);
  ASSERT_TRUE(loaded);
  const auto& a = loaded->second;
  EXPECT_EQ(std::vector<std::int32_t>(a.mk_piece.data(), a.mk_piece.data() + a.mk_piece.size()),
            (std::vector<std::int32_t>{0, 1, 2}));
  EXPECT_EQ(std::vector<std::int64_t>(a.mk_sample.data(), a.mk_sample.data() + a.mk_sample.size()),
            (std::vector<std::int64_t>{100, 200, 75}));

  // a segment alone: only that record, and its header states no start time
  m = meta_of(ms / "ms_rec_0001.hea", cache);
  const List& pieces = list_of(m, "pieces");
  EXPECT_EQ(pieces.size(), 2u);
  for (const auto& p : pieces) {
    const Value* v = p.get("timed");
    EXPECT_TRUE(v && v->as_bool() && !*v->as_bool());
  }

  // a record of a folder: that record only
  m = meta_of(testdata() / "plain" / "rec_a.hea", cache);
  EXPECT_EQ(labels_of(m), (std::vector<std::string>{"II", "ART"}));
  fs::remove_all(cache);
}

TEST(Wfdb, TheLimitsOfAFolder) {
  VS_NEEDS_WFDB();
  fs::path session = testdata() / "session";
  Dict scanned = scan(session, Limits{});
  EXPECT_EQ(int_of(scanned.get("records")), std::optional<std::int64_t>(6));
  EXPECT_EQ(int_of(scanned.get("segments")), std::optional<std::int64_t>(4));
  EXPECT_EQ(int_of(scanned.get("untimed")), std::optional<std::int64_t>(2));
  std::vector<std::string> signals;
  for (const auto& x : *scanned.get("signals")->as_list()) signals.push_back(x.py_str());
  EXPECT_EQ(signals, (std::vector<std::string>{"ART", "II", "V"}));
  const List& r = *scanned.get("t_range")->as_list();
  EXPECT_EQ(*f64_of(&r[0]) - T0, 7200.0);
  EXPECT_EQ(*f64_of(&r[1]) - T0, 7213.5);

  // over a limit: refused, with what to do instead (header files are counted, segments too)
  auto error_of = [&](const fs::path& p, const Limits& l) {
    try {
      scan(p, l);
    } catch (const std::exception& e) {
      return std::string(e.what());
    }
    return std::string("no error");
  };
  Limits few;
  few.max_records = 5;
  std::string e = error_of(session, few);
  EXPECT_TRUE(e.find("more than 5 WFDB records") != std::string::npos && e.find("index.csv") != std::string::npos)
      << e;
  Limits shallow;
  shallow.max_depth = 0;
  e = error_of(session, shallow);
  EXPECT_NE(e.find("nested more than 0 levels deep"), std::string::npos) << e;
  Limits quick;
  quick.max_scan_s = 0.0;
  e = error_of(session, quick);
  EXPECT_NE(e.find("took more than 0 s"), std::string::npos) << e;

  // a folder without records
  fs::path dir = temp_dir("vs_wfdb_empty");
  std::ofstream(dir / "notes.txt") << "no records";
  e = error_of(dir, Limits{});
  EXPECT_NE(e.find("no WFDB records"), std::string::npos) << e;
  fs::remove_all(dir);
}

TEST(Wfdb, TheKeyOfARecordFollowsItsFiles) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_wfdb_key");
  fs::path ms = dir / "ms";
  fs::copy(testdata() / "session" / "ms", ms, fs::copy_options::recursive);
  fs::path rec = ms / "ms_rec.hea";
  std::string k0 = cache_key(rec);
  std::vector<std::string> names;
  for (const auto& x : fingerprint(rec)) names.push_back(std::get<0>(x));
  EXPECT_EQ(names, (std::vector<std::string>{"ms_rec.hea", "ms_rec.mrk", "ms_rec_0001.dat", "ms_rec_0001.hea",
                                             "ms_rec_0002.dat", "ms_rec_0002.hea", "ms_rec_layout.hea"}));
  // another record of the folder changes: not this one's key
  std::ofstream(ms / "ms_fix_0001.dat", std::ios::binary) << std::string(4, '\0');
  EXPECT_EQ(cache_key(rec), k0);
  // one of its segments changes: a new key
  std::ofstream(ms / "ms_rec_0002.dat", std::ios::binary) << std::string(4, '\0');
  EXPECT_NE(cache_key(rec), k0);
  fs::remove_all(dir);
}

TEST(Wfdb, CancellingLeavesNothing) {
  VS_NEEDS_WFDB();
  fs::path dir = temp_dir("vs_wfdb_cancel");
  fs::path cache = dir / "cache";
  std::string e;
  try {
    build_index_until(testdata() / "session", cache, [](double) {}, [] { return true; });
  } catch (const std::exception& x) {
    e = x.what();
  }
  EXPECT_NE(e.find("cancelled"), std::string::npos) << e;
  EXPECT_TRUE(fs::is_empty(cache));
  fs::remove_all(dir);
}
#ifndef _WIN32
TEST(Adapters, EndToEnd) {
  if (std::system("python3 --version > /dev/null 2>&1") != 0 || !fs::exists(VS_FAKE_ADAPTER))
    GTEST_SKIP() << "no python3 or no mock adapter";
  fs::path dir = temp_dir("vs_adapter_test");
  fs::path folder = dir / "adapters";
  fs::create_directories(folder);
  fs::copy_file(VS_FAKE_ADAPTER, folder / "fake-adapter");
  fs::permissions(folder / "fake-adapter", fs::perms::owner_all);
  std::vector<double> samples;
  List xs;
  for (int i = 0; i < 500; ++i) {
    samples.push_back(std::sin(i / 10.0));
    xs.push_back(Value(samples.back()));
  }
  fs::path rec = dir / "rec.fake";
  {
    std::ofstream f(rec);
    f << Value(Dict{{"fs", Value(100.0)},
                    {"t0", Value(1700000000.0)},
                    {"label", Value("II")},
                    {"unit", Value("mV")},
                    {"patient", Value("Doe^Jane")},
                    {"samples", Value(std::move(xs))}})
             .to_json();
  }
  // without the adapter: a file of an unknown format
  adapters::set_folders({dir / "none"});
  EXPECT_FALSE(kind_of(rec));
  EXPECT_THROW(build_index(rec, dir / "cache", [](double) {}), std::runtime_error);

  adapters::set_folders({folder});
  auto found = adapters::installed();
  ASSERT_EQ(found.size(), 1u);
  EXPECT_EQ(found[0].name, "fake");
  EXPECT_EQ(found[0].formats[0].label, "Fake recording");
  EXPECT_EQ(kind_of(rec), std::optional<std::string>("fake"));
  std::vector<double> progress;
  std::string key = build_index(rec, dir / "cache", [&](double f) { progress.push_back(f); });
  EXPECT_NE(std::find(progress.begin(), progress.end(), 0.5), progress.end());
  auto loaded = load_index(dir / "cache", key);
  ASSERT_TRUE(loaded);
  EXPECT_EQ(*loaded->first.get("adapter")->as_str(), "fake");
  EXPECT_EQ(*loaded->first.get("kind")->as_str(), "fake");
  Dataset ds(std::move(loaded->first), std::move(loaded->second));
  // samples read on demand through the adapter's session, twice (the session is kept)
  EXPECT_EQ(ds.samples(0, 10, 20), std::vector<double>(samples.begin() + 10, samples.begin() + 20));
  EXPECT_EQ(ds.samples(0, 490, 500), std::vector<double>(samples.begin() + 490, samples.end()));
  std::string w = Value::from_json(ds.window_json({0}, 1700000000.0, 1700000001.0, 400)).py_str();
  EXPECT_NE(w.find("'raw'"), std::string::npos) << w;
  fs::remove_all(dir);
}
#endif
