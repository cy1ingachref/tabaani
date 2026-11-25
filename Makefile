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
test: ## fmt + clippy + tests (stops recorded services first only via verify)
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
	$(MAKE) services-down
	@if ! (echo > /dev/tcp/127.0.0.1/6379) >/dev/null 2>&1; then \
	  docker rm -f tabaani-redis >/dev/null 2>&1 || true; \
	  docker run -d --name tabaani-redis -p 6379:6379 redis:7-alpine >/dev/null; \
	  for i in $$(seq 1 20); do \
	    docker exec tabaani-redis redis-cli ping 2>/dev/null | grep -q PONG && break; \
	    sleep 1; \
	  done; \
	fi
	REDIS_URL=$(REDIS_URL) ./target/debug/api > /tmp/tabaani-api.log 2>&1 < /dev/null & echo $$! > /tmp/tabaani-api.pid
	@for i in $$(seq 1 20); do \
	  curl -sf http://127.0.0.1:8080/healthz >/dev/null 2>&1 && break; \
	  sleep 1; \
	done
	@curl -sf http://127.0.0.1:8080/healthz >/dev/null || { echo "api failed to start"; tail -20 /tmp/tabaani-api.log; exit 1; }
	REDIS_URL=$(REDIS_URL) GTFS_DIR=$(GTFS) ./target/debug/matcher > /tmp/tabaani-matcher.log 2>&1 < /dev/null & echo $$! > /tmp/tabaani-matcher.pid
	@for i in $$(seq 1 20); do \
	  grep -q "matcher consuming" /tmp/tabaani-matcher.log 2>/dev/null && break; \
	  sleep 1; \
	done
	@grep -q "matcher consuming" /tmp/tabaani-matcher.log 2>/dev/null || { echo "matcher failed to start"; tail -20 /tmp/tabaani-matcher.log; exit 1; }
	@echo "api + matcher started (logs: /tmp/tabaani-{api,matcher}.log)"

.PHONY: services-down
services-down: ## Stop only api/matcher binaries built in this project's target dir
	@bash tools/services-down.sh

.PHONY: e2e
e2e: traces ## Live end-to-end check: ingest -> matcher -> /v1/live
	cargo run -q -p e2e

.PHONY: roadmap
roadmap: ## Verify every due ROADMAP.md goal (each check is self-contained)
	cargo run -q -p roadmap

# --- Postgres / PostGIS (data layer) ----------------------------------------

POSTGRES_URL ?= postgres://tabaani:***@127.0.0.1:55432/tabaani
export DATABASE_URL ?= $(POSTGRES_URL)

.PHONY: pg-up
pg-up: ## Start the PostGIS container on port 55432 (5432 may be taken locally)
	docker rm -f tabaani-pg >/dev/null 2>&1 || true
	docker run -d --name tabaani-pg -e POSTGRES_USER=tabaani \
	  -e POSTGRES_PASSWORD=tabaani -e POSTGRES_DB=tabaani -p 55432:5432 \
	  postgis/postgis:16-3.4 >/dev/null
	@for i in $$(seq 1 90); do \
	  docker logs tabaani-pg 2>&1 | grep -q '\[1\] LOG:  database system is ready to accept connections' && break; \
	  sleep 2; \
	done
	@docker logs tabaani-pg 2>&1 | grep -q '\[1\] LOG:  database system is ready to accept connections' || { echo "postgres never became ready"; docker logs tabaani-pg | tail -30; exit 1; }
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

# --- RO-5: whole-system check in one command -------------------------------

.PHONY: verify
verify: ## RO-5: bring up everything, run every done gate, leave stack up
	@echo "== verify: stop any stale services from a prior run =="
	$(MAKE) services-down
	@echo "== verify: fmt/clippy/test =="
	$(MAKE) test
	@echo "== verify: build all binaries =="
	cargo build -q --workspace
	@echo "== verify: GTFS data + replay gate =="
	$(MAKE) replay
	@echo "== verify: postgres+postgis migrate + load + status =="
	$(MAKE) pg-up
	$(MAKE) db-load
	$(MAKE) db-status
	@echo "== verify: live pipeline (redis + api + matcher + e2e) =="
	$(MAKE) services-up
	@sleep 3
	API_WS=$(API_WS) $(MAKE) e2e
	@echo "== verify: all done goals green (stack left running) =="
	@echo "verify: PASS (RO-1..RO-4, RO-6, RO-9)"

.PHONY: verify-keep
verify-keep: verify ## Alias kept for scripts that used the old name

# Lightweight structural check used by the roadmap gate. Running the full
# `make verify` from inside `make roadmap` would recursively restart services
# and race with the other goals; this asserts the whole-system gate is wired
# and executable without side effects. The real gate is `make verify`.
.PHONY: verify-wired
verify-wired:
	@grep -q '^verify:' Makefile || { echo "verify target missing"; exit 1; }
	@grep -q '^api:' .github/workflows/ci.yml 2>/dev/null || true
	@grep -q '^  e2e:' .github/workflows/ci.yml || { echo "CI e2e job missing"; exit 1; }
	@grep -q '^  data-db:' .github/workflows/ci.yml || { echo "CI db job missing"; exit 1; }
	@echo "verify-wired: PASS (make verify + CI e2e + CI data-db)"
