# BetterShot for Linux. Run from linux/ (or `make -C linux <target>`).
SHELL := /bin/bash -o pipefail

.PHONY: help check fmt lint test build release clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'

check: ## Everything CI runs: format check, clippy, tests
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	BETTERSHOT_TESTING=1 cargo test --workspace

fmt: ## Format the code
	cargo fmt --all

lint: ## Clippy with warnings as errors
	cargo clippy --workspace --all-targets -- -D warnings

test: ## Run the tests with isolated storage
	BETTERSHOT_TESTING=1 cargo test --workspace

build: ## Debug build
	cargo build --workspace

release: ## Optimized build
	cargo build --workspace --release

clean: ## Remove build artifacts
	cargo clean
