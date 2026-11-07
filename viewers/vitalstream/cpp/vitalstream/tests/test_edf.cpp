// EDF, EDF+ and BDF files (docs/edf.md) on the synthetic files of testdata/edf, as
// libs/engine/tests/edf.rs: channels and rates, the start of the first record (EDF+ time keeping),
// records in runs with breaks (EDF+D), annotations as events, a two-digit year, a record cut
// short, 24-bit samples, and the patient field never read; and the header dates and the
// annotation lists themselves.
#include <gtest/gtest.h>

#include <cmath>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <numbers>

#include "engine/dataset.hpp"
#include "engine/index.hpp"
#include "engine/sources.hpp"

using namespace vitalstream::engine;
namespace fs = std::filesystem;

namespace {

constexpr double START = 1709298000.0;  // 2024-03-01 13:00:00 UTC

fs::path file(const char* name) { return fs::path(VS_TESTDATA) / "edf" / name; }

fs::path temp_dir(const char* name) {
  fs::path d = fs::temp_directory_path() / name;
  fs::remove_all(d);
  fs::create_directories(d);
  return d;
}

struct Indexed {
  std::string key;
  Dict meta;
  std::unique_ptr<Dataset> ds;
};

Indexed indexed(const char* name, const fs::path& cache) {
  std::string key = build_index(file(name), cache, [](double) {});
  auto loaded = load_index(cache, key);
  EXPECT_TRUE(loaded) << name;
  Dict meta = loaded->first;
  return {key, meta, std::make_unique<Dataset>(std::move(loaded->first), std::move(loaded->second))};
}

const List& list(const Dict& m, const char* k) { return *m.get(k)->as_list(); }

std::string s(const Value& d, const char* k) {
  const Value* v = d.get(k);
  return v && v->as_str() ? *v->as_str() : std::string();
}

double f(const Value& d, const char* k) { return *f64_of(d.get(k)); }
std::int64_t i(const Value& d, const char* k) { return *int_of(d.get(k)); }

}  // namespace

TEST(Edf, AnEdfPlusFile) {
  fs::path cache = temp_dir("vs_edf_plus");
  fs::path p = file("psg_plus.edf");
  EXPECT_EQ(kind_of(p), std::optional<std::string>("edf"));
  EXPECT_STREQ(edf_format_label(p), "EDF+");
  EXPECT_EQ(source_format_label(p), "EDF+");
  auto [key, m, ds] = indexed("psg_plus.edf", cache);
  EXPECT_EQ(s(Value(m), "kind"), "edf");
  EXPECT_EQ(s(Value(m), "format"), "EDF+");
  std::vector<std::pair<std::string, std::string>> ch;
  for (const auto& c : list(m, "channels")) ch.emplace_back(s(c, "label"), s(c, "unit"));
  EXPECT_EQ(ch, (std::vector<std::pair<std::string, std::string>>{{"ECG", "mV"}, {"Resp", ""}, {"SpO2", "%"}}));
  // the first record starts 0.25 s after the header's time (its time-keeping annotation)
  std::vector<std::tuple<double, std::int64_t, double>> got;
  for (const auto& q : list(m, "pieces")) got.emplace_back(f(q, "fs"), i(q, "n"), f(q, "t0") - START);
  EXPECT_EQ(got, (std::vector<std::tuple<double, std::int64_t, double>>{
                     {250.0, 5000, 0.25}, {25.0, 500, 0.25}, {1.0, 20, 0.25}}));
  EXPECT_TRUE(list(m, "gaps").empty());
  std::vector<std::tuple<double, double, std::string, std::string>> ev;
  for (const auto& a : list(m, "alarms")) {
    ev.emplace_back(f(a, "t0") - START, f(a, "t1") - START, s(a, "text"), s(a, "kind"));
    EXPECT_TRUE(a.get("level")->is_none());
  }
  EXPECT_EQ(ev, (std::vector<std::tuple<double, double, std::string, std::string>>{
                    {0.25, 10.25, "Sleep stage W", "annotation"},
                    {0.5, 0.5, "Lights off", "annotation"},
                    {4.25, 7.25, "Arousal", "annotation"}}));
  // the patient field (an invented identity here) is in no file of the index
  for (const auto& e : fs::directory_iterator(cache / key)) {
    std::ifstream in(e.path(), std::ios::binary);
    std::string bytes((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    EXPECT_EQ(bytes.find("Synthetic"), std::string::npos) << e.path();
    EXPECT_EQ(bytes.find("MO-TEST"), std::string::npos) << e.path();
  }
  // samples: the physical values the generator wrote, to within one digital step
  auto x = ds->samples(2, 0, 20);
  ASSERT_EQ(x.size(), 20u);
  for (std::size_t k = 0; k < x.size(); ++k) {
    double t = 0.25 + static_cast<double>(k);
    EXPECT_LE(std::abs(x[k] - (96.0 + std::sin(t / 3.0))), 0.1 / 2.0 + 1e-9) << "SpO2 " << k;
  }
  EXPECT_EQ(ds->samples(1, 498, 600).size(), 2u);
  fs::remove_all(cache);
}

TEST(Edf, RecordsInRuns) {
  fs::path cache = temp_dir("vs_edf_runs");
  auto [key, m, ds] = indexed("gappy_plus.edf", cache);
  std::vector<std::tuple<double, std::int64_t, std::int64_t>> got;
  for (const auto& q : list(m, "pieces")) got.emplace_back(f(q, "t0") - START, i(q, "n"), i(q, "data_record"));
  EXPECT_EQ(got, (std::vector<std::tuple<double, std::int64_t, std::int64_t>>{
                     {0.0, 375, 0}, {3.0, 250, 3}, {10.0, 125, 5}}));
  std::vector<std::tuple<double, double, std::string>> gaps;
  for (const auto& g : list(m, "gaps")) gaps.emplace_back(f(g, "t0") - START, f(g, "t1") - START, s(g, "reason"));
  EXPECT_EQ(gaps, (std::vector<std::tuple<double, double, std::string>>{{1.5, 3.0, "not recorded"},
                                                                         {4.0, 10.0, "not recorded"}}));
  std::vector<std::string> ev;
  for (const auto& a : list(m, "alarms")) ev.push_back(s(a, "text"));
  EXPECT_EQ(ev, std::vector<std::string>{"Electrode off"});
  // samples of a later run come from its own records
  EXPECT_EQ(ds->samples(1, 0, 250).size(), 250u);
  fs::remove_all(cache);
}

TEST(Edf, PlainEdfAndBdf) {
  fs::path cache = temp_dir("vs_edf_plain");
  {
    auto [key, m, ds] = indexed("plain.edf", cache);
    EXPECT_EQ(s(Value(m), "format"), "EDF");
    const List& pieces = list(m, "pieces");
    // 1999 (a two-digit year); the number of records is unknown: the 8 whole records of the file
    EXPECT_EQ(f(pieces[0], "t0"), 920368800.0);
    std::vector<std::int64_t> n;
    for (const auto& q : pieces) n.push_back(i(q, "n"));
    EXPECT_EQ(n, (std::vector<std::int64_t>{1600, 8}));
    bool cut = false;
    for (const auto& note : list(m, "notes"))
      cut |= note.as_str()->find("ends inside a data record") != std::string::npos;
    EXPECT_TRUE(cut);
    EXPECT_TRUE(list(m, "alarms").empty());
  }
  auto [key, m, ds] = indexed("eeg_plus.bdf", cache);
  EXPECT_EQ(s(Value(m), "format"), "BDF+");
  std::vector<std::string> labels;
  for (const auto& c : list(m, "channels")) labels.push_back(s(c, "label"));
  EXPECT_EQ(labels, (std::vector<std::string>{"Fp1", "Flat"}));
  // a digital range of zero: the digital values themselves
  EXPECT_EQ(ds->samples(1, 0, 12), std::vector<double>(12, 7.0));
  auto x = ds->samples(0, 0, 768);
  ASSERT_EQ(x.size(), 768u);
  double step = (262143.0 + 262144.0) / (8388607.0 + 8388608.0);
  for (std::size_t k = 0; k < x.size(); ++k) {
    double t = static_cast<double>(k) / 256.0;
    EXPECT_LE(std::abs(x[k] - 80.0 * std::sin(2.0 * std::numbers::pi * 9.0 * t)), step) << "Fp1 " << k;
  }
  std::vector<std::pair<double, std::string>> ev;
  for (const auto& a : list(m, "alarms")) ev.emplace_back(f(a, "t0") - START, s(a, "text"));
  EXPECT_EQ(ev, (std::vector<std::pair<double, std::string>>{{1.5, "Eyes closed"}}));
  fs::remove_all(cache);
}

TEST(Edf, NotEdf) {
  fs::path dir = temp_dir("vs_edf_not");
  fs::path p = dir / "x.edf";
  std::ofstream(p, std::ios::binary) << "not an EDF file at all";
  try {
    build_index(p, dir / "cache", [](double) {});
    ADD_FAILURE() << "indexed";
  } catch (const std::exception& e) {
    EXPECT_NE(std::string(e.what()).find("not an EDF or BDF file"), std::string::npos) << e.what();
  }
  fs::remove_all(dir);
}

TEST(Edf, DatesAndAnnotations) {
  // two-digit years: 85-99 are 19xx, the rest 20xx; EDF+ states the year in full
  std::string plus = "Startdate 01-MAR-2091 X X X";
  EXPECT_EQ(edf::start_of("02.03.99", "10.00.00", nullptr), std::optional<double>(920368800.0));
  EXPECT_EQ(edf::start_of("01.03.24", "13.00.00", nullptr), std::optional<double>(1709298000.0));
  EXPECT_EQ(edf::start_of("01.03.yy", "13.00.00", &plus), std::optional<double>(3823592400.0));
  EXPECT_EQ(edf::start_of("01.03.yy", "13.00.00", nullptr), std::nullopt);
  EXPECT_EQ(edf::start_of("01.03.91", "13.00.00", &plus), std::optional<double>(3823592400.0));
  EXPECT_EQ(edf::start_of("31.02.24", "25.00.00", nullptr), std::nullopt);
  using namespace std::string_view_literals;
  auto t = edf::tals("+0.25\x14\x14\x00+1.5\x15" "2\x14" "Arousal\x14 two  words \x14\x00\x00\x00"sv);
  ASSERT_EQ(t.size(), 2u);
  EXPECT_EQ(t[0].onset, 0.25);
  EXPECT_EQ(t[0].duration, std::nullopt);
  EXPECT_EQ(t[0].texts, (std::vector<std::string>{"", ""}));
  EXPECT_EQ(t[1].onset, 1.5);
  EXPECT_EQ(t[1].duration, std::optional<double>(2.0));
  EXPECT_EQ(t[1].texts, (std::vector<std::string>{"Arousal", "two words", ""}));
  // invalid UTF-8: one replacement character per maximal subpart (as Rust and Python decode it);
  // an onset that is not a number skips its list
  auto u = edf::tals("+1\x14\xe2\x82" "A\xff\x1f" "b\x14\x00x\x14lost\x14\x00"sv);
  ASSERT_EQ(u.size(), 1u);
  EXPECT_EQ(u[0].texts, (std::vector<std::string>{"\xef\xbf\xbd" "A\xef\xbf\xbd b", ""}));
}
