# tabaani — one-command entry points.
#
# Every target body is plain shell and works from any POSIX shell if `make` is
# not installed.

SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

.PHONY: help
help: ## List targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN { FS = ":.*?## " }; {printf "  %-12s %s\n", $$1, $$2}'

DATASET_VERSION ?= 0.1.0-dev
SEED := data/tab_seed.json
GTFS_DIR := data/gtfs/build/$(DATASET_VERSION)
TRACES := data/traces
THRESHOLDS := tools/replay/thresholds.toml

.PHONY: data
data: ## Build the versioned GTFS dataset from the seed
	cargo run -q -p gtfs-builder -- $(SEED) data/gtfs/build

.PHONY: traces
traces: data ## Generate simulated rider traces with ground truth
	cargo run -q -p simulator -- $(GTFS_DIR) $(TRACES)

.PHONY: replay
replay: traces ## Score traces through the matcher; fails below thresholds
	cargo run -q -p replay -- $(TRACES) $(THRESHOLDS)

.PHONY: pipeline
pipeline: ## Full data -> traces -> replay gate
	$(MAKE) replay

.PHONY: test
test: ## fmt + clippy + tests
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

.PHONY: lint
lint: ## Formatting + clippy only
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

.PHONY: clean
clean: ## Remove build outputs (target dir, generated data)
	cargo clean
	rm -rf data/gtfs/build data/traces/*.jsonl
