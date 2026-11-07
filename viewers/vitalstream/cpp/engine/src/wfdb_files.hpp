// WFDB records as files, without reading them (no wfdb-x): the helpers that sources_wfdb.cpp (the
// reader, VS_WFDB=ON) and sources_wfdb_none.cpp (a build without WFDB support, VS_WFDB=OFF) share
// with sources_wfdb_files.cpp, which defines them.  Internal to the engine.  Mirrors the Rust
// engine's sources/wfdb_files.rs.
#pragma once

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <vector>

namespace vitalstream::engine::wfdb_files {

namespace fs = std::filesystem;

std::optional<std::string> read_text(const fs::path& p);
std::vector<std::string> split(const std::string& s, char sep);
// Rust's str::parse of an integer: an optional sign ('+' only for an unsigned one), then digits.
bool parse_i64(const std::string& s, std::int64_t& out, bool is_unsigned = false);
std::string lossy(const std::string& s);  // String::from_utf8_lossy
bool ends_with(const std::string& s, const std::string& suffix);
// p with its extension replaced (Rust's Path::with_extension: "" removes it).
fs::path with_extension(const fs::path& p, const char* ext);
// a record's file: its name with an extension added ("a.v1" -> "a.v1.hea"; a record name may
// hold a dot, which with_extension would take for the start of an extension)
fs::path with_ext(const fs::path& rec, const char* ext);
fs::path join(const fs::path& base, const std::string& name);
bool is_file(const fs::path& p);
// The lines of a header file that are neither blank nor comments, trimmed.
std::vector<std::string> header_lines(const fs::path& hea);

}  // namespace vitalstream::engine::wfdb_files
