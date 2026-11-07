// A build without WFDB support (CMake option VS_WFDB=OFF; the Rust engine's wfdb_none.rs): no
// wfdb-x code at all.  WFDB records are still recognised by their files (a .hea file, a folder of
// records, a collection: sources_wfdb_files.cpp), so that opening one is refused with NO_WFDB
// rather than taken for something else; every other format is read as in the default build.  The
// same functions as sources_wfdb.cpp, so the callers are the same.
#include <stdexcept>

#include "engine/sources.hpp"
#include "wfdb_files.hpp"

namespace vitalstream::engine {

namespace {

[[noreturn]] void refuse() { throw std::runtime_error(NO_WFDB); }

class NoWfdbReader : public Reader {
 public:
  std::vector<double> read(std::size_t, std::int64_t, std::int64_t) const override { refuse(); }
};

}  // namespace

bool wfdb_supported() { return false; }

Built build_wfdb_folder(const fs::path&, const Progress&, const Cancelled&) { refuse(); }

Dict scan(const fs::path&, const Limits&) { refuse(); }

// The files of record `name` known without reading its header: the header itself (the signal
// files it names are known only to wfdb-x).
std::vector<std::string> record_files(const fs::path&, const std::string& name) { return {name + ".hea"}; }

std::vector<fs::path> files_read(const fs::path& base, const std::string& name) {
  return {wfdb_files::with_ext(wfdb_files::join(base, name), ".hea")};
}

// The record is not read at all.
void check_files(const fs::path&, const std::string&) { refuse(); }

std::unique_ptr<Reader> make_wfdb_reader(const fs::path&, const Dict&) { return std::make_unique<NoWfdbReader>(); }

}  // namespace vitalstream::engine
