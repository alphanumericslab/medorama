#!/usr/bin/env bash
# Download the C/C++ dependencies of viewers/vitalstream/cpp/ (pinned in viewers/vitalstream/cpp/cmake/deps.cmake: archive, URL, SHA-256)
# into viewers/vitalstream/cpp/deps/, checking each hash.  A build then needs no internet access: copy the clone,
# with viewers/vitalstream/cpp/deps/, to the machine without it.
#
#   tools/fetch-cpp-deps.sh            all dependencies (the libraries, the viewer, the tests)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEPS="$ROOT/viewers/vitalstream/cpp/deps"
mkdir -p "$DEPS"
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
# vs_dependency(<name> <file>\n <url>\n <sha256> ...): the three fields after the name
tr '\n' ' ' < "$ROOT/viewers/vitalstream/cpp/cmake/deps.cmake" | grep -oE 'vs_dependency\([a-z0-9_]+ +[^ ]+ +https://[^ ]+ +[0-9a-f]{64}' |
while read -r _ file url hash; do
  out="$DEPS/$file"
  if [ -f "$out" ] && [ "$(sha "$out")" = "$hash" ]; then
    echo "ok        $file"
    continue
  fi
  echo "download  $file"
  curl -fsSL --retry 3 -o "$out.part" "$url"
  got=$(sha "$out.part")
  if [ "$got" != "$hash" ]; then
    rm -f "$out.part"
    echo "SHA-256 mismatch for $file: expected $hash, got $got" >&2
    exit 1
  fi
  mv "$out.part" "$out"
done
echo "viewers/vitalstream/cpp/deps/ is complete: builds now work without internet access."
