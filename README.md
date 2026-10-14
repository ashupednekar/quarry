# Quarry ⚡️

A fast, keyboard-first database client with a Zed-like interface. Quarry does the DataGrip core (query, browse, edit, import/export, logs) and skips the IDE baggage.

## Principles

1. **Content First:** Data and SQL get the space. Chrome recedes.
2. **Keyboard First:** Every action reachable via the `⌘K` command bar.
3. **Pure Rust Speed:** Built on GPUI for 120 FPS GPU rendering and sub-100ms startup.
4. **Safe Data Editing:** JetBrains capability with explicit review and commit (`UPDATE`/`INSERT`/`DELETE` diff view).
5. **Driver Philosophy:** Native Rust drivers behind a pluggable registry. Oracle is the one exception: it needs Instant Client, which Quarry can download from the connect form.

## Databases

PostgreSQL, MySQL, SQLite, SQL Server and Oracle. TLS for Postgres and MySQL uses Rustls. To add an engine, see [DRIVERS.md](DRIVERS.md).

## Build and run

```sh
make run     # cargo run -p quarry-app
make test    # workspace tests
make lint    # fmt check + clippy
```

## Packaging

Uses [cargo-packager](https://github.com/crabnebula-dev/cargo-packager). Output lands in `dist/`.

```sh
cargo install cargo-packager --locked
cargo packager --release -f app   # or dmg, deb, appimage, nsis
```

## Architecture

- `crates/quarry-core`: Driver registry and `Dialect` trait, drivers for each engine, unified `DbValue`, row streaming, atomic pending edits generator.
- `crates/quarry-sql`: SQL parser, statement boundary splitting (protecting strings/comments/dollar-quotes), schema-aware keyword and table autocomplete.
- `crates/quarry-app`: GPUI user interface, command palette, tabs, virtualized results grid, and inline review modal.

## License

GNU General Public License v3.0 or later (`GPL-3.0-or-later`).
