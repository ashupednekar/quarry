.DEFAULT_GOAL := help
.PHONY: help dev run build release test lint fmt check bench live-test package clean

help: ## Show available commands
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'

dev: ## Run the app (debug build, fast compile)
	cargo run -p quarry-app

run: ## Run the app (release build, smooth UI)
	cargo run -p quarry-app --release

build: ## Build the workspace (debug)
	cargo build --workspace

release: ## Build the app (release)
	cargo build -p quarry-app --release

test: ## Run all tests
	cargo test --workspace

lint: ## Check formatting and clippy
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format the code
	cargo fmt --all

check: lint test ## Lint and test

bench: ## Run benchmarks
	cargo bench --workspace

live-test: ## Tests against a live Postgres on :55432
	QUARRY_PG_URL="postgres://postgres:postgres@localhost:55432/postgres" \
	cargo test -p quarry-core --test live

# Packaging builds for the machine it runs on; OS picks the formats
# Windows itself exports OS=Windows_NT
ifeq ($(OS),Windows_NT)
override OS := windows
endif
OS ?= $(if $(filter Darwin,$(shell uname -s)),macos,$(if $(filter Linux,$(shell uname -s)),linux,windows))

package: ## Package the app: OS=macos (dmg) | linux (deb, AppImage) | windows (nsis); default: this OS
ifeq ($(OS),macos)
	./scripts/package-macos.sh
else ifeq ($(OS),linux)
	cd crates/quarry-app && cargo packager --release -f deb -f appimage
else ifeq ($(OS),windows)
	cd crates/quarry-app && cargo packager --release -f nsis
else
	$(error OS must be macos, linux or windows)
endif

clean: ## Remove build artifacts
	cargo clean
