# Medorama: common tasks.  `make help` lists them.
#
# Everything runs inside this clone: the Python environment is .venv/, the Rust workspace
# builds to target/, the C++ viewer to $(V)/cpp/build/, results to sample_data/outputs/ and
# $(V)/sample_data/outputs/.  The first `make install` needs network access (Python packages,
# Rust crates and rustup if Rust is missing, the C++ dependencies unless $(V)/cpp/deps/ holds
# them).

V       := viewers/vitalstream
VENV    := .venv
PY      := $(VENV)/bin/python
CARGO   := cargo
export PATH := $(CURDIR)/$(VENV)/bin:$(HOME)/.cargo/bin:$(PATH)

.PHONY: help install install-python rust cpp cpp-deps test test-python test-rust test-cpp \
        test-medorama test-medorama-postgres test-viewers test-tools parity coverage fuzz audit \
        lint lint-python msrv clean rust-nowfdb cpp-nowfdb test-nowfdb

help:
	@echo "make install          Python + Rust + C++ into .venv (tools/install.sh)"
	@echo "make install-python   the Python packages only"
	@echo "make rust             build the Rust workspace: medorama, vitalstream-rs (target/release)"
	@echo "make cpp              build the C++ viewer and its tests ($(V)/cpp/build/release/bin)"
	@echo "make cpp-deps         download the C++ dependencies into $(V)/cpp/deps (for offline builds)"
	@echo "make rust-nowfdb      medorama and vitalstream-rs without WFDB support, no wfdb-x code (target/nowfdb/release)"
	@echo "make cpp-nowfdb       the C++ viewer without WFDB support, VS_WFDB=OFF ($(V)/cpp/build/nowfdb/bin)"
	@echo "make test             all tests: Python, Rust, C++, Medorama, viewers, tools, viewer parity"
	@echo "make test-python      the Python packages: vitalstream-py, muse-py, dicom-py (pytest)"
	@echo "make test-rust        the Rust libraries and vitalstream-rs"
	@echo "make test-cpp         the C++ viewer's unit tests (ctest; builds it first)"
	@echo "make test-medorama    Medorama's tests and its page scripts"
	@echo "make test-medorama-postgres   Medorama's tests with the catalog in PostgreSQL (MEDORAMA_TEST_POSTGRES=postgres://...)"
	@echo "make test-viewers     the ECG and PACS viewers, and the VitalStream page's scripts (node --test)"
	@echo "make test-tools       tools/: the privacy check, the screenshot tool's guard, the demo's generator"
	@echo "make parity           the three VitalStream viewers: the same index and the same answers ($(V)/testdata)"
	@echo "make test-nowfdb      the tests of the builds without WFDB support: Rust, C++, Python (wfdb-x not imported)"
	@echo "make lint             cargo fmt --check, clippy and rustdoc"
	@echo "make lint-python      Python: pyflakes' rules (ruff $(RUFF); install: .venv/bin/pip install ruff==$(RUFF))"
	@echo "make msrv             the workspace checked with the oldest Rust it supports (rustup toolchain install $(MSRV))"
	@echo "make coverage         Medorama's test coverage, failing under the floor CI uses (install: cargo install cargo-llvm-cov)"
	@echo "make fuzz             Medorama's parsers fuzzed, every target (cargo fuzz list) 60 s each (cargo +nightly; install: cargo install cargo-fuzz)"
	@echo "make audit            dependencies: RustSec advisories, licences, sources (cargo deny; install: cargo install cargo-deny)"
	@echo "make clean            remove the Rust and C++ builds and Python caches (keeps .venv)"

install:
	tools/install.sh

install-python:
	tools/install.sh --py

rust:
	$(CARGO) build --release --locked

cpp:
	$(VENV)/bin/pip install --quiet 'cmake>=3.24' ninja
	cd $(V)/cpp && cmake --preset release > /dev/null && cmake --build --preset release

cpp-deps:
	tools/fetch-cpp-deps.sh

test: test-python test-rust test-cpp test-medorama test-viewers test-tools parity

test-python:
	$(PY) -m pytest -q $(V)/python/vitalstream/tests viewers/ecg/python/muse/tests viewers/pacs/python/dicom/tests

# the libraries and the Rust viewer (Medorama: test-medorama)
test-rust:
	$(CARGO) test --release --locked -p signal-core -p recording-engine -p vitalstream-rs

test-cpp: cpp
	cd $(V)/cpp && ctest --preset release

test-medorama:
	$(CARGO) test --release --locked -p medorama
	node --test medorama/tests/web/

# the same tests with the catalog in PostgreSQL: MEDORAMA_TEST_POSTGRES=postgres://user@host/db
test-medorama-postgres:
	@test -n "$(MEDORAMA_TEST_POSTGRES)" || { echo "set MEDORAMA_TEST_POSTGRES=postgres://user@host/db (an empty database)"; exit 1; }
	MEDORAMA_TEST_POSTGRES=$(MEDORAMA_TEST_POSTGRES) $(CARGO) test --release --locked -p medorama

test-viewers:
	node --test viewers/ecg/tests/*.test.js viewers/pacs/tests/*.test.js
	node --test viewers/vitalstream/tests/web/

# the privacy check (tools/git-hooks/check-files) in a scratch repository; the screenshot tool,
# the demo's generator and the website's link script
test-tools:
	bash tools/tests/test-check-files.sh
	python3 -B -m unittest discover -s tools/tests
	node --test tools/tests/

# Builds without WFDB support, which hold no wfdb-x code (docs/building.md, "Without WFDB"): the
# Rust programs in target/nowfdb (the default builds in target/release stay), the C++ viewer in
# $(V)/cpp/build/nowfdb; the Python tests with wfdb-x made unimportable, as in an installation
# without the [wfdb] extra
NOWFDB_RS := --target-dir target/nowfdb -p recording-engine -p medorama -p vitalstream-rs \
             --no-default-features --features vitalstream-rs/password
rust-nowfdb:
	$(CARGO) build --release --locked $(NOWFDB_RS)

cpp-nowfdb:
	$(VENV)/bin/pip install --quiet 'cmake>=3.24' ninja
	cd $(V)/cpp && cmake --preset nowfdb > /dev/null && cmake --build --preset nowfdb

test-nowfdb: cpp-nowfdb
	$(CARGO) test --release --locked $(NOWFDB_RS)
	cd $(V)/cpp && ctest --preset nowfdb
	$(PY) -c "import sys; sys.modules['wfdbx'] = None; import pytest; \
	  sys.exit(pytest.main(['-q', '$(V)/python/vitalstream/tests']))"

# the viewers that are built: byte-identical indexes, the same API answers
parity:
	$(PY) $(V)/tests/parity/index_parity.py
	$(PY) $(V)/tests/parity/viewer_parity.py

lint:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets --release -- -D warnings
	$(CARGO) clippy $(NOWFDB_RS) --all-targets --release -- -D warnings
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps --document-private-items

# the Python code: unused imports, undefined names (CI's checks.yml, the same version)
RUFF ?= 0.17.4
lint-python:
	$(VENV)/bin/ruff check --no-cache --isolated --select F --target-version py39 --exclude sample_data .

# the oldest Rust the workspace supports (rust-version in Cargo.toml; CI's msrv job)
MSRV := $(shell sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)
msrv:
	$(CARGO) +$(MSRV) check --locked --workspace

clean:
	rm -rf target/release target/debug target/nowfdb $(V)/cpp/build
	find . -name __pycache__ -type d -prune -exec rm -rf {} +

# the share of Medorama's lines its tests run (an HTML report in target/llvm-cov/html); the
# floor is CI's (.github/workflows/medorama.yml)
coverage:
	$(CARGO) llvm-cov -p medorama --tests --html --fail-under-lines 76 && $(CARGO) llvm-cov report --summary-only | tail -1

# coverage-guided fuzzing of the parsers (medorama/fuzz/): FUZZ_S seconds each, every target
# (`cargo fuzz list`), on the versions of the workspace's Cargo.lock
FUZZ_S ?= 60
fuzz:
	cp Cargo.lock medorama/fuzz/Cargo.lock
	cd medorama/fuzz && for t in $$($(CARGO) fuzz list); do \
	  mkdir -p corpus/$$t && $(CARGO) +nightly fuzz run $$t corpus/$$t seeds/$$t -- -max_total_time=$(FUZZ_S) || exit 1; done

# dependencies: advisories, licences (deny.toml), sources
audit:
	$(CARGO) deny check
