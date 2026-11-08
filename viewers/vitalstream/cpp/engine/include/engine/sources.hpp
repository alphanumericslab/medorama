// Reading recordings: WFDB records in the viewer itself (sources_wfdb.cpp, with wfdb-x: one
// record, a collection, or a folder of records), EDF, EDF+ and BDF files (sources_edf.cpp), every
// other format through an installed adapter (adapters.hpp). Mirrors the Rust engine's sources/ module: building the index of a recording
// (build(path, progress, cancelled)), and a reader of the physical samples [a, b) of a piece.
//
// WFDB support is the CMake option VS_WFDB (ON by default), the only part of the engine that uses
// wfdb-x.  With VS_WFDB=OFF, sources_wfdb_none.cpp takes the place of sources_wfdb.cpp: WFDB
// records and folders are still recognised (kind_of says "wfdb"), and refused with NO_WFDB when
// they are opened (unreadable, build_source); every other format is read as in the default build.
#pragma once

#include <filesystem>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <tuple>
#include <utility>
#include <vector>

#include "engine/common.hpp"
#include "engine/index.hpp"

namespace vitalstream::engine {

using Built = std::pair<Dict, Arrays>;

// Whether this build reads WFDB records (VS_WFDB).
bool wfdb_supported();

// The refusal of a WFDB record or folder by a build without WFDB support (VS_WFDB=OFF).
extern const char* const NO_WFDB;

// Why this build cannot read the recording at p, if it cannot: a WFDB record or a folder (which
// opens as WFDB records, build_source) in a build without WFDB support (NO_WFDB).  The viewer
// refuses such a recording when it is opened, with this message.
std::optional<std::string> unreadable(const fs::path& p);

// The kind of recording at p: "wfdb" for a WFDB record (.hea file) or a WFDB folder (one that
// holds records itself, or a collection), "edf" for an EDF or BDF file, an adapter's kind for a
// file it reads (by extension), or nothing.  Any folder can also be opened as the records below it (build_source); a folder
// that holds none itself is not a recording when a tree is searched.
std::optional<std::string> kind_of(const fs::path& p);

// The label of the format a recording at p is read as ("WFDB record", "EDF+", an adapter's
// label), or an empty string.
std::string source_format_label(const fs::path& p);

// The index of a recording: WFDB records (a folder, or a .hea file) and EDF files are built here
// (meta and arrays); a file an adapter reads is indexed by the adapter into dir (its meta.json is
// returned, the arrays stay on disk, `by_adapter` is set).  Stops with "indexing cancelled" once
// `cancelled` returns true.  Throws.
struct Building {
  Built built;
  bool by_adapter = false;
};
Building build_source(const fs::path& path, const fs::path& dir, const Progress& progress, const Cancelled& cancelled);

// What a folder of plain records may hold before it is read: records (header files, the
// segments of multi-segment records included), folder levels below it, and seconds for the scan
// that counts them (docs/wfdb-collection.md, "Limits").  A collection (index.csv) lists its
// records itself and is not limited.
struct Limits {
  std::size_t max_records = 5000;
  std::size_t max_depth = 20;
  double max_scan_s = 10.0;
};
void set_limits(const Limits& limits);  // for every folder read from now on (the settings)
Limits limits();                        // the limits in force

// WFDB records (sources_wfdb.cpp): one record (its .hea file), a collection or a folder of
// plain records, subfolders included, read as one session.  Throws (NO_WFDB without VS_WFDB).
Built build_wfdb_folder(const fs::path& path, const Progress& progress, const Cancelled& cancelled);

// A WFDB collection (index.csv), or a folder holding WFDB records (RECORDS, or a .hea file among
// its first scan_limit entries).
bool is_wfdb_folder(const fs::path& p, std::size_t scan_limit);

// A WFDB header file (.hea): one record.
bool is_record(const fs::path& p);

// The segments of a multi-segment record, (name, samples) in order ("~" names a null segment);
// nothing for a single-segment record.  record: the record's path without .hea.
using Segments = std::vector<std::pair<std::string, std::int64_t>>;
std::optional<Segments> segment_table(const fs::path& record);

// The format label of what path opens as: "WFDB record", "WFDB multi-segment record", "WFDB
// collection" or "WFDB records".
const char* format_label(const fs::path& path);

// Whether p is named as a WFDB signal file is (.dat, or .mat, a MATLAB file a header names): not
// a record of its own, but read with the header that names it.
bool is_signal_file(const fs::path& p);
// For a WFDB signal file (.dat, .mat) opened or added as a recording: what to open in its place,
// its header (with the name of the header next to it that names it, when there is one).  Nothing
// for any other path.
std::optional<std::string> signal_file_note(const fs::path& p);
// Why p is not opened as a recording: for a WFDB signal file, to open its header in its place;
// else that it is not a recording this program reads.
std::string not_a_recording(const fs::path& p);

// What opening path would read, from the headers alone (no signal file is opened), within
// limits (the scan's time limit too).  Throws over a limit (docs/wfdb-collection.md, "Limits"),
// and NO_WFDB without VS_WFDB.
Dict scan(const fs::path& path, const Limits& limits);

// The files of record `name` (relative to base, '/' between folders): its header, the signal
// files its header names, its .mrk file if there is one, and for a multi-segment record those of
// each segment; sorted, each once.
std::vector<std::string> record_files(const fs::path& base, const std::string& name);

// The most files a folder's key is computed from (fingerprint).
constexpr std::size_t MAX_FINGERPRINT_FILES = 200000;

// (name, size, mtime ns) of each file, for the cache key: every file below a folder (hidden
// entries and links to folders skipped; a link to a file counts with its target's size and time),
// or the files of one record (record_files); name relative to the folder (the record's folder),
// with '/' between components on every system (viewers/vitalstream/docs/index-format.md).  A
// folder's walk visits names in order, enters at most max_depth levels of subfolders (limits())
// and stops after MAX_FINGERPRINT_FILES files.
std::vector<std::tuple<std::string, std::uint64_t, std::int64_t>> fingerprint(const fs::path& path);

// Every file reading record `name` (relative to base) opens: its header, the signal files its
// header names (an absolute name as given, a name starting with ~ in the home folder, any other
// next to the header), its .mrk file, and those of each segment of a multi-segment record.
std::vector<fs::path> files_read(const fs::path& base, const std::string& name);
// Throws std::runtime_error naming the first file of record `name` that lies outside the folders
// the readers may open (set_allowed_roots).
void check_files(const fs::path& base, const std::string& name);

// Restrict every file the readers open (a header, the signal files it names, a segment, a .mrk or
// CSV file) to these folders, symbolic links resolved: a program's data roots
// (viewers/vitalstream/docs/index-format.md, "Files outside the data roots").  Nothing (the
// default) allows any file.
void set_allowed_roots(const std::optional<std::vector<std::string>>& roots);
// Whether a reader may open the file at p: its real path lies inside an allowed root.  A file
// that does not exist is allowed (nothing can be read from it); a broken link is not.
bool allowed_file(const fs::path& p);

// EDF, EDF+, BDF and BDF+ files (sources_edf.cpp; docs/edf.md): one file, one recording.
bool is_edf(const fs::path& p);                 // a file named .edf or .bdf (any case)
const char* edf_format_label(const fs::path& p);  // "EDF", "EDF+", "BDF" or "BDF+"
Built build_edf(const fs::path& path, const Progress& progress, const Cancelled& cancelled);  // throws

namespace edf {
// The start of a file (Unix seconds) from its header's date (dd.mm.yy) and time (hh.mm.ss) and,
// for EDF+ and BDF+, its recording field (Startdate dd-MMM-yyyy gives the year in full); nothing
// when they state no valid one.
std::optional<double> start_of(const std::string& date, const std::string& time, const std::string* recording);
// The time-stamped annotation lists of an annotation signal's bytes:
// +onset[\x15duration]\x14text\x14...\x14\x00 (texts with their whitespace runs collapsed).
struct Tal {
  double onset;
  std::optional<double> duration;
  std::vector<std::string> texts;
};
std::vector<Tal> tals(std::string_view bytes);
}  // namespace edf

class Reader {
 public:
  virtual ~Reader() = default;
  // Physical samples [a, b) of a piece (NaN for missing).  Throws.
  virtual std::vector<double> read(std::size_t piece, std::int64_t a, std::int64_t b) const = 0;
  // read() of several (piece, a, b) at once (a WFDB record's signals over the same samples are
  // decoded once).  Throws.
  virtual std::vector<std::vector<double>> read_many(
      const std::vector<std::tuple<std::size_t, std::int64_t, std::int64_t>>& wanted) const {
    std::vector<std::vector<double>> out;
    for (const auto& [p, a, b] : wanted) out.push_back(read(p, a, b));
    return out;
  }
};

// The reader of the recording indexed in dir (its meta).
std::unique_ptr<Reader> make_reader(const Dict& meta, const fs::path& dir);
// The reader of the records at path (a folder, or a record's .hea file).
std::unique_ptr<Reader> make_wfdb_reader(const fs::path& path, const Dict& meta);
// The reader of the EDF or BDF file at path.
std::unique_ptr<Reader> make_edf_reader(const fs::path& path, const Dict& meta);

}  // namespace vitalstream::engine
