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

# --- services / live pipeline (Phase 1) -------------------------------------

REDIS_URL ?= redis://127.0.0.1:6379
API_WS    ?= ws://127.0.0.1:8080
GTFS      := $(GTFS_DIR)

.PHONY: services-up
services-up: data ## Start api + matcher against a local redis (docker)
	docker rm -f tabaani-redis >/dev/null 2>&1 || true
	docker run -d --name tabaani-redis -p 6379:6379 redis:7-alpine >/dev/null
	@sleep 4
	REDIS_URL=$(REDIS_URL) ./target/debug/api > /tmp/tabaani-api.log 2>&1 &
	@sleep 2
	REDIS_URL=$(REDIS_URL) GTFS_DIR=$(GTFS) ./target/debug/matcher > /tmp/tabaani-matcher.log 2>&1 &
	@sleep 2
	@echo "api + matcher started (logs: /tmp/tabaani-{api,matcher}.log)"

.PHONY: services-down
services-down: ## Stop api + matcher
	@-taskkill /F /IM api.exe >/dev/null 2>&1 || true
	@-taskkill /F /IM matcher.exe >/dev/null 2>&1 || true

.PHONY: e2e
e2e: traces ## Live end-to-end check: ingest -> matcher -> /v1/live
	cargo run -q -p e2e

.PHONY: roadmap
roadmap: ## Verify every due ROADMAP.md goal
	cargo run -q -p roadmap

# --- Postgres / PostGIS (data layer) ----------------------------------------

POSTGRES_URL ?= postgres://tabaani:tabaani@127.0.0.1:55432/tabaani
export DATABASE_URL ?= $(POSTGRES_URL)

.PHONY: pg-up
pg-up: ## Start the PostGIS container on port 55432 (5432 may be taken locally)
	docker rm -f tabaani-pg >/dev/null 2>&1 || true
	docker run -d --name tabaani-pg -e POSTGRES_USER=tabaani \
	  -e POSTGRES_PASSWORD=tabaani -e POSTGRES_DB=tabaani -p 55432:5432 \
	  postgis/postgis:16-3.4 >/dev/null
	@for i in $$(seq 1 30); do \
	  docker exec tabaani-pg pg_isready -U tabaani -d tabaani >/dev/null 2>&1 && break; \
	  sleep 2; \
	done
	@echo "postgres+postgis ready on 127.0.0.1:55432"

.PHONY: pg-down
pg-down: ## Stop the PostGIS container (keeps data)
	docker stop tabaani-pg >/dev/null 2>&1 || true

.PHONY: db-migrate
db-migrate: ## Apply schema migrations
	cargo run -q -p db -- migrate

.PHONY: db-load
db-load: db-migrate ## Build GTFS and load it into Postgres
	cargo run -q -p gtfs-builder -- $(SEED) data/gtfs/build
	cargo run -q -p db -- load-gtfs $(GTFS_DIR)

.PHONY: db-status
db-status: ## Show dataset version and row counts
	cargo run -q -p db -- status

.PHONY: stack-up
stack-up: pg-up db-load services-up ## Full local stack: postgres, redis, api, matcher, data

.PHONY: stack-down
stack-down: services-down pg-down ## Stop everything
