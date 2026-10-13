.PHONY: all build test lint fmt run clean bench live-test

all: lint test

build:
	cargo build --workspace

test:
	cargo test --workspace

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

run:
	cargo run -p quarry-app

clean:
	cargo clean

bench:
	cargo bench --workspace

live-test:
	QUARRY_PG_URL="postgres://postgres:postgres@localhost:55432/postgres" \
	cargo test -p quarry-core --test live
