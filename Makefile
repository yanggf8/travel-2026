# travel-2026 — npm-free build entry (root package.json retired; CLI is Rust).
# The Cloudflare Workers under workers/ keep their own wrangler/npm setup and
# are built/deployed separately — they are NOT covered here.

CARGO      := cargo
RUST_DIR   := rust
BIN_DIR    := bin
TARGET_REL := $(RUST_DIR)/target/release

.DEFAULT_GOAL := build

# Build the release binaries and stage them into ./bin/ (gitignored).
.PHONY: build
build:
	cd $(RUST_DIR) && $(CARGO) build --release -p travel-cli -p chromeport
	mkdir -p $(BIN_DIR)
	cp $(TARGET_REL)/travel $(BIN_DIR)/travel
	cp $(TARGET_REL)/chromeport $(BIN_DIR)/chromeport
	@echo "Built ./bin/travel and ./bin/chromeport"

# Fast debug build (no ./bin staging) — for local iteration.
.PHONY: dev
dev:
	cd $(RUST_DIR) && $(CARGO) build -p travel-cli

# Install git hooks (replaces the old npm postinstall).
.PHONY: hooks
hooks:
	cp scripts/hooks/* .git/hooks/
	chmod +x .git/hooks/*
	@echo "Installed git hooks."

# Run the Rust test suite (real Turso; ported from the retired vitest suite).
.PHONY: test
test:
	cd $(RUST_DIR) && $(CARGO) test

# Compile check (the old `npm run typecheck` equivalent).
.PHONY: check
check:
	cd $(RUST_DIR) && $(CARGO) build -p travel-cli --quiet

# Data integrity check (the old `npm run validate:data`).
.PHONY: validate
validate: dev
	./$(RUST_DIR)/target/debug/travel validate data

# Full system health check (the old `npm run doctor`).
.PHONY: doctor
doctor: dev
	./$(RUST_DIR)/target/debug/travel doctor

# First-time setup: build binaries + install hooks.
.PHONY: setup
setup: build hooks

.PHONY: clean
clean:
	cd $(RUST_DIR) && $(CARGO) clean
	rm -rf $(BIN_DIR)
