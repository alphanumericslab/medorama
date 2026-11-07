// WFDB records and annotation files through wfdb-x (see wfdb_c.hpp).
#include "core/wfdb_c.hpp"

#include <wfdbx/record.hpp>
#include <wfdbx/wfdb.hpp>

#include <cmath>
#include <cstdio>
#include <mutex>
#include <stdexcept>

#include "core/paths.hpp"
#include "core/pyfmt.hpp"
#include "core/sha256.hpp"

namespace vitalstream::core::wfdbc {

namespace fs = std::filesystem;

namespace {

std::mutex& lock() {
  static std::mutex m;
  return m;
}

// Holds the library lock for the scope (the C++ library keeps process-wide state).  Messages are
// not printed: errors come back as return values, wfdberror() and exceptions.
class Locked {
 public:
  Locked() : guard_(lock()) { wfdbx::wfdbquiet(); }
  ~Locked() { wfdbx::wfdbquit(); }
  Locked(const Locked&) = delete;
  Locked& operator=(const Locked&) = delete;

 private:
  std::lock_guard<std::mutex> guard_;
};

// Output files are created in `dir` for the scope; the previous setting is restored.
class OutputDir {
 public:
  explicit OutputDir(const fs::path& dir) : saved_(wfdbx::wfdbx_set_output_dir(path_utf8(dir).c_str())) {}
  ~OutputDir() { wfdbx::wfdbx_set_output_dir(saved_ ? saved_->c_str() : nullptr); }
  OutputDir(const OutputDir&) = delete;
  OutputDir& operator=(const OutputDir&) = delete;

 private:
  std::optional<std::string> saved_;
};

[[noreturn]] void fail(const std::string& what) {
  std::string msg = what, e = wfdbx::wfdberror();
  while (!e.empty() && (e.back() == '\n' || e.back() == '\r')) e.pop_back();
  if (!e.empty()) msg += ": " + e;
  throw std::runtime_error(msg);
}

std::string record_path(const fs::path& dir, const std::string& record) { return path_utf8(dir / record); }

std::int64_t round_half_away(double x) { return static_cast<std::int64_t>(std::copysign(std::floor(std::fabs(x) + 0.5), x)); }

wfdbx::Header header_of(const std::string& path) {
  try {
    return wfdbx::read_header(path);
  } catch (const wfdbx::Error& e) {
    throw std::runtime_error(path + ": " + e.what());
  }
}

}  // namespace

void write_header(const fs::path& dir, const std::string& record, double fs_, std::int64_t sig_len,
                  const std::vector<SignalSpec>& signals, std::optional<double> start_unix,
                  const std::vector<std::string>& info) {
  Locked in;
  OutputDir out(dir);
  if (wfdbx::setsampfreq(fs_) < 0) fail("setsampfreq");
  if (start_unix) {
    std::int64_t total_ms = round_half_away(*start_unix * 1000.0);
    std::int64_t days = total_ms >= 0 ? total_ms / 86400000 : -((-total_ms + 86399999) / 86400000);
    std::int64_t ms = total_ms - days * 86400000;
    CivilDate d = date_from_days(days);
    char buf[96];
    std::snprintf(buf, sizeof buf, "%d:%02d:%02d.%03d %02u/%02u/%d", static_cast<int>(ms / 3600000),
                  static_cast<int>(ms / 60000 % 60), static_cast<int>(ms / 1000 % 60), static_cast<int>(ms % 1000),
                  d.day, d.month, d.year);
    if (wfdbx::setbasetime(buf) < 0) fail("setbasetime");
  } else {
    wfdbx::setbasetime("0");  // not a time: base time and date are both cleared
  }
  std::vector<wfdbx::WFDB_Siginfo> si(signals.size());
  for (std::size_t k = 0; k < signals.size(); ++k) {
    const SignalSpec& s = signals[k];
    si[k].fname = s.file_name;
    si[k].desc = s.description;
    si[k].units = s.units;
    si[k].gain = s.gain;
    si[k].initval = s.init_value;
    si[k].group = 0;
    si[k].fmt = s.fmt;
    si[k].spf = 1;
    si[k].bsize = s.block_size;
    si[k].adcres = s.adc_res;
    si[k].adczero = s.adc_zero;
    si[k].baseline = s.baseline;
    si[k].nsamp = static_cast<long>(sig_len);
    si[k].cksum = s.checksum;
  }
  if (wfdbx::setheader(record.c_str(), si.data(), static_cast<unsigned>(si.size())) < 0) fail(record + ".hea");
  for (const auto& line : info)
    if (wfdbx::putinfo(line.c_str()) < 0) fail(record + ".hea");
}

void write_annotations(const fs::path& dir, const std::string& record, const std::string& ext,
                       const std::vector<Annotation>& annotations, double fs_, const std::vector<Label>& labels) {
  Locked in;
  for (const auto& l : labels) {
    wfdbx::setannstr(l.code, l.symbol.c_str());  // positive codes: written to the file's table
    wfdbx::setanndesc(l.code, l.description.c_str());
  }
  wfdbx::Annotations a;
  for (const auto& x : annotations) {
    if (x.aux.size() > 255) throw std::runtime_error(record + "." + ext + ": aux field longer than 255 bytes");
    a.samples.push_back(x.time);
    a.codes.push_back(x.code);
    a.subtypes.push_back(x.subtype);
    a.chans.push_back(x.chan);
    a.nums.push_back(x.num);
    a.aux.push_back(x.aux.empty() ? std::nullopt : std::optional<std::string>(x.aux));
  }
  try {
    // written as given, never sorted: the converters write annotations in canonical order
    wfdbx::write_annotations(record, ext, a, fs_, path_utf8(dir), false);
  } catch (const wfdbx::Error& e) {
    throw std::runtime_error(record + "." + ext + ": " + e.what());
  }
}

RecordCheck read_record(const fs::path& dir, const std::string& record) {
  Locked in;
  wfdbx::ReadOptions opt;
  opt.checksum_warning = false;  // the checksums are compared by the caller
  wfdbx::Record r;
  try {
    r = wfdbx::read_record(record_path(dir, record), opt);
  } catch (const wfdbx::Error& e) {
    throw std::runtime_error(record + ": " + e.what());
  }
  RecordCheck out;
  const auto n = static_cast<std::size_t>(r.nsig);
  out.fs = r.fs;
  out.sig_len = n > 0 ? r.siginfo[0].nsamp : 0;
  for (const auto& s : r.siginfo) {
    out.descriptions.push_back(s.desc.value_or(""));
    out.header_checksums.push_back(static_cast<short>(s.cksum & 0xffff));
  }
  std::vector<long> sums(n, 0);
  std::vector<std::uint8_t> bytes;
  bytes.reserve(r.samples.size() * 2);
  for (std::size_t i = 0; i < r.samples.size(); ++i) {
    int v = r.samples[i];
    auto x = static_cast<std::uint16_t>(static_cast<std::int16_t>(v));
    bytes.push_back(static_cast<std::uint8_t>(x & 0xff));
    bytes.push_back(static_cast<std::uint8_t>(x >> 8));
    sums[i % n] += v;
  }
  Sha256 sha;
  sha.update(bytes.data(), bytes.size());
  out.frames_read = static_cast<std::int64_t>(r.nsamples());
  for (long s : sums) out.sample_checksums.push_back(static_cast<short>(s & 0xffff));
  out.sha256 = sha.hex_digest();
  return out;
}

HeaderInfo read_header(const fs::path& dir, const std::string& record) {
  Locked in;
  const std::string path = record_path(dir, record);
  wfdbx::Header h = header_of(path);
  HeaderInfo out;
  out.fs = h.fs;
  out.sig_len = h.nsamples;
  for (const auto& s : h.siginfo)
    out.signals.push_back(
        SignalInfo{s.desc.value_or(""), s.units.value_or(""), s.gain, s.baseline, s.fname.value_or(""), s.fmt, s.spf});
  if (h.base_date && h.base_time_ms) {
    const auto& d = *h.base_date;
    out.start_unix =
        static_cast<double>(days_from_date(d[0], static_cast<unsigned>(d[1]), static_cast<unsigned>(d[2]))) * 86400.0 +
        static_cast<double>(*h.base_time_ms) / 1000.0;
  }
  return out;
}

std::vector<std::vector<double>> read_physical(const fs::path& dir, const std::string& record, std::int64_t from,
                                               std::int64_t to, const std::vector<std::size_t>& signals) {
  Locked in;
  const std::string path = record_path(dir, record);
  wfdbx::Header h = header_of(path);
  const auto n = static_cast<std::size_t>(h.nsig);
  std::vector<std::size_t> wanted = signals;
  if (wanted.empty())
    for (std::size_t k = 0; k < n; ++k) wanted.push_back(k);
  for (auto k : wanted)
    if (k >= n) throw std::runtime_error(record + ": no signal " + std::to_string(k));
  if (h.nsamples > 0 && to > h.nsamples) to = h.nsamples;
  std::vector<std::vector<double>> out(wanted.size());
  if (to <= from) return out;
  wfdbx::ReadOptions opt;
  opt.from = static_cast<wfdbx::WFDB_Time>(from);
  opt.to = static_cast<wfdbx::WFDB_Time>(to);
  opt.checksum_warning = false;
  wfdbx::Record r;
  try {
    r = wfdbx::read_record(path, opt);
  } catch (const wfdbx::Error& e) {
    throw std::runtime_error(record + ": " + e.what());
  }
  const std::size_t frames = r.nsamples();
  for (std::size_t j = 0; j < wanted.size(); ++j) {
    const auto& s = r.siginfo[wanted[j]];
    out[j].reserve(frames);
    for (std::size_t i = 0; i < frames; ++i) {
      int x = r.samples[i * n + wanted[j]];
      // the stored value for an uncalibrated signal (gain 0): no gain is assumed
      out[j].push_back(x == wfdbx::WFDB_INVALID_SAMPLE ? std::nan("")
                       : s.gain == 0.0 ? static_cast<double>(x)
                                       : (static_cast<double>(x) - static_cast<double>(s.baseline)) / s.gain);
    }
  }
  return out;
}

std::vector<Annotation> read_annotations(const fs::path& dir, const std::string& record, const std::string& ext) {
  Locked in;
  wfdbx::Annotations a;
  try {
    a = wfdbx::read_annotations(record_path(dir, record), ext);
  } catch (const wfdbx::Error& e) {
    throw std::runtime_error(record + "." + ext + ": " + e.what());
  }
  std::vector<Annotation> out;
  out.reserve(a.size());
  for (std::size_t i = 0; i < a.size(); ++i) {
    Annotation x;
    x.time = a.samples[i];
    x.code = a.codes[i];
    x.subtype = a.subtypes[i];
    x.chan = a.chans[i];
    x.num = a.nums[i];
    x.aux = a.aux[i].value_or("");
    out.push_back(std::move(x));
  }
  return out;
}

std::string mnemonic(int code) {
  Locked in;
  return wfdbx::ecgstr(code).value_or("[" + std::to_string(code) + "]");
}

}  // namespace vitalstream::core::wfdbc
