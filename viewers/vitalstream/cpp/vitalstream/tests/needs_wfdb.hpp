// The tests on WFDB records are skipped in a build without WFDB support (VS_WFDB=OFF), which
// refuses them; test_server.cpp checks that refusal (Server.WithoutWfdbAWfdbRecordIsRefused).
#pragma once

#include <gtest/gtest.h>

#include "engine/sources.hpp"

#define VS_NEEDS_WFDB()                       \
  if (!vitalstream::engine::wfdb_supported()) \
  GTEST_SKIP() << "WFDB records: a build without WFDB support (VS_WFDB=OFF) refuses them"
