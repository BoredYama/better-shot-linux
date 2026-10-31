# BetterShot for Linux. Run from linux/ (or `make -C linux <target>`).
SHELL := /bin/bash -o pipefail

.PHONY: help check check-ui fmt lint test build release clean install uninstall run

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

check-ui: build ## Drive the app under Xvfb with a fixture screen (needs Xvfb, Openbox, xdotool, ImageMagick)
	bash scripts/check-ui.sh

run: build ## Run from the source tree with isolated test storage
	BETTERSHOT_TESTING=1 BETTERSHOT_TEST_ROOT=$(CURDIR)/target/dev-profile ./target/debug/bettershot

clean: ## Remove build artifacts
	cargo clean

PREFIX ?= /usr/local
DESTDIR ?=
APP_ID = site.bettershot.BetterShot

install: release ## Install to $(PREFIX) (default /usr/local)
	install -Dm755 target/release/bettershot $(DESTDIR)$(PREFIX)/bin/bettershot
	install -Dm644 data/$(APP_ID).desktop $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	for size in 16 32 64 128 256 512; do \
		install -Dm644 data/icons/hicolor/$${size}x$${size}/apps/$(APP_ID).png \
			$(DESTDIR)$(PREFIX)/share/icons/hicolor/$${size}x$${size}/apps/$(APP_ID).png; \
	done
	@# Packagers (DESTDIR set) leave cache updates to the package manager.
	if [ -z "$(DESTDIR)" ]; then \
		gtk-update-icon-cache -qtf $(PREFIX)/share/icons/hicolor || true; \
		update-desktop-database -q $(PREFIX)/share/applications || true; \
	fi

uninstall: ## Remove an installation from $(PREFIX)
	rm -f $(DESTDIR)$(PREFIX)/bin/bettershot
	rm -f $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	rm -f $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/*/apps/$(APP_ID).png
	-update-desktop-database -q $(DESTDIR)$(PREFIX)/share/applications
