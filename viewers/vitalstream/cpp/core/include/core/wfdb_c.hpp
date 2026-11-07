// WFDB headers, records and annotation files through wfdb-x (https://github.com/alphanumericslab/wfdb-x,
// the C++ library wfdbx): setheader + putinfo write headers, write_annotations writes MIT-format
// annotation files (with their time-resolution and code-definition table), read_header,
// read_record and read_annotations read them.  Signal files (.dat) are written by the converters
// themselves, because the library writes one output record per process at a time and a converter
// writes several at once.
//
// The C++ library keeps process-wide state, so every call here holds one process-wide lock.
// Records are named by their full path; output files are created in the record's folder
// (wfdbx_set_output_dir), so the current directory is never changed.  Nothing is assumed that a
// header does not state: no sampling frequency, gain or units.
#pragma once

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <vector>

namespace vitalstream::core::wfdbc {

struct SignalSpec {
  std::string file_name;
  int fmt = 16;
  double gain = 1.0;
  int baseline = 0;
  std::string units;
  int adc_res = 16;
  int adc_zero = 0;
  int init_value = 0;
  int checksum = 0;
  int block_size = 0;
  std::string description;
};

// <dir>/<record>.hea: record line (base time and date when start_unix is given, millisecond
// resolution, rounded half away from zero), one line per signal, then "#" + each info string.
// Throws std::runtime_error.
void write_header(const std::filesystem::path& dir, const std::string& record, double fs, std::int64_t sig_len,
                  const std::vector<SignalSpec>& signals, std::optional<double> start_unix,
                  const std::vector<std::string>& info);

struct Annotation {
  std::int64_t time = 0;
  int code = 0;
  int subtype = 0;
  int chan = 0;
  int num = 0;
  std::string aux;  // empty: none (at most 255 bytes)
};

struct Label {
  int code;
  std::string symbol, description;
};

// <dir>/<record>.<ext> (MIT format), with the time resolution fs and the custom code labels in
// the file's table.  Annotations in time order.  Throws std::runtime_error.
void write_annotations(const std::filesystem::path& dir, const std::string& record, const std::string& ext,
                       const std::vector<Annotation>& annotations, double fs, const std::vector<Label>& labels);

// A record read back (read_record).
struct RecordCheck {
  double fs = 0.0;
  std::int64_t sig_len = 0;                 // stated in the header
  std::int64_t frames_read = 0;
  std::vector<std::string> descriptions;
  std::vector<int> header_checksums;
  std::vector<int> sample_checksums;        // 16-bit sums of the samples read
  std::string sha256;                       // of the frames as 16-bit little-endian samples
};
RecordCheck read_record(const std::filesystem::path& dir, const std::string& record);

// Header of a record (read_header; no signal file is opened).
struct SignalInfo {
  std::string description, units;  // units: empty when the header states none
  double gain = 0.0;                // 0: uncalibrated (physical values are then ADC units)
  int baseline = 0;
  std::string file_name;            // the signal file the header names ("~": none)
  int fmt = 0;                      // its storage format
  int spf = 1;                      // samples per frame
};
struct HeaderInfo {
  double fs = 0.0;                   // 0 when the header states none
  std::int64_t sig_len = 0;
  std::vector<SignalInfo> signals;
  std::optional<double> start_unix;  // base date and time (UTC), when both are stated
};
// Throws std::runtime_error ("<dir>/<record>: <wfdb-x's message>").
HeaderInfo read_header(const std::filesystem::path& dir, const std::string& record);

// Physical values of frames [from, to) of the signals `signals` (all when empty):
// (stored - baseline) / gain, the stored value for an uncalibrated signal (gain 0), NaN for
// WFDB's invalid sample.  Throws std::runtime_error.
std::vector<std::vector<double>> read_physical(const std::filesystem::path& dir, const std::string& record,
                                               std::int64_t from, std::int64_t to,
                                               const std::vector<std::size_t>& signals = {});

// The standard mnemonic of an annotation code (ecgstr: the WFDB table for codes 0..49, "[code]"
// otherwise).
std::string mnemonic(int code);

// Annotations of <record>.<ext> (read_annotations).
std::vector<Annotation> read_annotations(const std::filesystem::path& dir, const std::string& record,
                                         const std::string& ext);

}  // namespace vitalstream::core::wfdbc
