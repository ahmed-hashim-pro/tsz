# Thin wrappers over cargo, plus the targets that regenerate what the README shows.
CARGO ?= cargo
TSZ := target/release/tsz

.PHONY: build test lint soak fuzz bench bench-sizes check demo data clean

build:
	$(CARGO) build --release

test:
	$(CARGO) test

lint:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings

# The README's round-trip claim at full size: a million random series, every codec.
soak:
	TSZ_SOAK=1000000 $(CARGO) test --release --test roundtrip soak -- --ignored

# Needs nightly and cargo-fuzz (cargo install cargo-fuzz). FUZZ_SECONDS per target.
FUZZ_SECONDS ?= 60
fuzz:
	cd fuzz && cargo +nightly fuzz run -O decode -- -max_total_time=$(FUZZ_SECONDS) -rss_limit_mb=2048
	cd fuzz && cargo +nightly fuzz run -O roundtrip -- -max_total_time=$(FUZZ_SECONDS) -rss_limit_mb=2048

# Sizes and speeds against other crates; rewrites docs/bench.{svg,json}.
bench:
	cd bench && $(CARGO) run --release -- --runs 7 --svg ../docs/bench.svg --json ../docs/bench.json

# Sizes only (deterministic), and the chart: what CI checks is current.
bench-sizes:
	cd bench && $(CARGO) run --release -- --sizes-only --svg ../docs/bench.svg

check: lint test soak bench-sizes

# Re-record docs/demo.gif from the real CLI. Needs Docker (VHS); the binary is
# built on Debian bookworm glibc, as the VHS image is.
demo: build
	docker run --rm -u $$(id -u):$$(id -g) -e HOME=/tmp -v $(CURDIR):/vhs \
	  -v $(CURDIR)/target/release:/opt/tsz:ro ghcr.io/charmbracelet/vhs:v0.10.0 docs/demo.tape

# Re-download data/ from the sources (see data/README.md).
data:
	python3 scripts/fetch_data.py

clean:
	$(CARGO) clean
