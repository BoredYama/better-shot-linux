# BetterShot for Linux. Run from linux/ (or `make -C linux <target>`).
SHELL := /bin/bash -o pipefail

.PHONY: help check check-ui check-gnome-helper fmt lint test build release clean install uninstall run deb appimage flatpak

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

check-gnome-helper: build ## Check the GNOME Shell helper extension in a headless GNOME Shell (needs gnome-shell)
	bash scripts/check-gnome-helper.sh

run: build ## Run from the source tree with isolated test storage
	BETTERSHOT_TESTING=1 BETTERSHOT_TEST_ROOT=$(CURDIR)/target/dev-profile ./target/debug/bettershot

clean: ## Remove build artifacts
	cargo clean

PREFIX ?= /usr/local
DESTDIR ?=
APP_ID = site.bettershot.BetterShot
HELPER_UUID = bettershot-helper@bettershot.site

install: release ## Install to $(PREFIX) (default /usr/local)
	install -Dm755 target/release/bettershot $(DESTDIR)$(PREFIX)/bin/bettershot
	install -Dm644 data/$(APP_ID).desktop $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	for size in 16 32 64 128 256 512; do \
		install -Dm644 data/icons/hicolor/$${size}x$${size}/apps/$(APP_ID).png \
			$(DESTDIR)$(PREFIX)/share/icons/hicolor/$${size}x$${size}/apps/$(APP_ID).png; \
	done
	for icon in data/icons/hicolor/scalable/actions/*.svg; do \
		install -Dm644 "$$icon" $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/actions/$$(basename "$$icon"); \
	done
	install -Dm644 ../Resources/Onboarding/coast.png $(DESTDIR)$(PREFIX)/share/bettershot/onboarding/coast.png
	@# The GNOME Shell helper; users turn it on in Settings > Capture after logging in again.
	for file in metadata.json extension.js; do \
		install -Dm644 gnome-extension/$(HELPER_UUID)/$$file $(DESTDIR)$(PREFIX)/share/gnome-shell/extensions/$(HELPER_UUID)/$$file; \
	done
	@# Scrolling capture adapts MacShot (GPLv3) and zoom planning adapts Cap (AGPLv3); their notices ship with the app.
	install -Dm644 ../LICENSE $(DESTDIR)$(PREFIX)/share/licenses/bettershot/LICENSE
	install -Dm644 ../Resources/Licenses/MacShot.txt $(DESTDIR)$(PREFIX)/share/licenses/bettershot/MacShot.txt
	install -Dm644 ../Resources/Licenses/Cap.txt $(DESTDIR)$(PREFIX)/share/licenses/bettershot/Cap.txt
	@# Packagers (DESTDIR set) leave cache updates to the package manager.
	if [ -z "$(DESTDIR)" ]; then \
		gtk-update-icon-cache -qtf $(PREFIX)/share/icons/hicolor || true; \
		update-desktop-database -q $(PREFIX)/share/applications || true; \
	fi

deb: ## Build a Debian package into target/debian (needs `cargo install cargo-deb`)
	cargo deb -p bettershot-app

appimage: release ## Build an AppImage into target/appimage (needs appimagetool on PATH or APPIMAGETOOL set)
	bash packaging/appimage/build.sh

flatpak: ## Build and install the Flatpak for the current user (needs flatpak-builder and the GNOME SDK)
	flatpak-builder --user --install --force-clean target/flatpak-build packaging/flatpak/$(APP_ID).yml

uninstall: ## Remove an installation from $(PREFIX)
	rm -f $(DESTDIR)$(PREFIX)/bin/bettershot
	rm -f $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	rm -f $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/*/apps/$(APP_ID).png
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/actions/bettershot-*-symbolic.svg
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/bettershot
	rm -rf $(DESTDIR)$(PREFIX)/share/bettershot
	rm -rf $(DESTDIR)$(PREFIX)/share/gnome-shell/extensions/$(HELPER_UUID)
	-update-desktop-database -q $(DESTDIR)$(PREFIX)/share/applications
