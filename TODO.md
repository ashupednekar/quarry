# TODO

Deferred from the OOUX pass. Each item names where it plugs in.

## Editor
- [x] Squiggles refresh when the connection or catalog changes (`completion::lint_editor`)
- [x] Fuzzy matching in completions: prefix, then substring, then letters in order (`SchemaCompleter::complete`)
- [x] A failed run's error is a squiggle on the statement, at Postgres' error position (`completion::mark_run_error`)
- [x] Code actions (⌥↵ / ⌘.): format, expand `*`, add `LIMIT`, `EXPLAIN`, wrap in a transaction (`quarry_sql::rewrites`)
- [x] Go to definition: ⌘-click a table or alias to open it (`completion::TableDefinitions`)
- [x] Editor toolbar: Explain, Format
- [x] Subquery aliases: `x.` after `FROM (...) x` should complete the subquery's columns; today inner tables are in scope but `x` is unknown (no false squiggles) (`quarry_sql::table_refs`)
- [x] ▶ on every statement, placed from line height + scroll offset; with soft wrap on, statements below a line that may wrap are skipped (`editor::run_gutter_buttons`)
- [x] Editor toolbar: per-tab connection picker (`sync_tab_connection`)
- [x] Editor toolbar: schema picker (Postgres `search_path`, set per run), row limit (`quarry_sql::with_limit`)
- [x] Reconnect a query tab's connection on tab switch (`sync_tab_connection`)
- [x] ▶ hover highlight (covered by a ▶ per statement)
- [x] Results header: Expand button
- [x] Session saved when a tab opens (empty tabs were only lost on a killed process)
- [x] ⌃Tab: live preview of the highlighted tab while cycling

## Queries (sidebar)
- [x] Right-click: Open, Rename…, Duplicate, Reveal in Finder, Move to Trash, Copy Path
- [x] Folder listing cached by the folder's mtime (`files::saved_queries`)
- [x] Recent: .sql files opened/saved outside the queries folder (`files::recent_files`)
- [x] Rename inline in the sidebar

## Objects
- [x] Object menu: Export, Truncate…, Drop… (open as a query to review; the prod check applies) (`object_menu.rs`)
- [x] Connections: Disconnect, Edit…, Remove (also clears the keychain password)
- [x] Object menu on table tab headers (right-click)
- [x] Command palette: a table's Structure/DDL, and pinned queries
- [x] Column header menu: sort, hide/show columns, copy name (`ResultSet::open_header_menu`)
- [x] Peek card stays open while hovered; Open, Structure, New Query, Copy Name

## Table detail view
- [x] Postgres DDL: checks, uniques, exclusions, comments; constraint-backed indexes not repeated
- [x] Structure: indexes as a table (name, columns, unique)
- [x] Click a FK / "referenced by" entry to open that table
- [x] Table tabs (and their view) restored once their connection is connected

## Server logs
- [x] Split lines into time / pid / level / message (default `log_line_prefix`)
- [x] Pick the log file; Load older pages back 64 KB at a time
- [x] Lines split by the server's own `log_line_prefix` (`server_logs::parse_line`); ERROR/FATAL/PANIC rows in red

## Modals
- [x] Enter triggers the primary button (add connection, export, import, params, command palette)
- [x] Keep Tab focus inside the open modal: Tab / ⇧Tab cycle within it, and focus moves in on open (`ui::modal_backdrop`, one shared `ModalFocus` handle)
- [ ] ~~One `Option<ActiveModal>` instead of separate `*_open` flags~~ won't do: 46 call sites across bool flags and stateful modals, for no user-facing change; one-modal-at-a-time hasn't caused a bug

## Layout
- [x] Maximise the editor (⇧⎋): hides sidebar and bottom panel, again to restore
- [x] Tab headers: staging/prod underline on table tabs, unsaved dot (already existed), right-click menu
- [x] Empty states with a primary button (sidebar, no tabs open)
- [x] Panes remember their size across restarts (`layout.rs`)
- [ ] ~~In-memory SQLite connections kept across restarts~~ won't do: the data can't come back, and the demo reseeds from "Open the demo database"

## Performance & Memory (Zed-grade scale)
- [x] Non-blocking SQLite: offload queries to Tokio blocking pool so 100k-row scans don't starve async worker threads (`quarry_core::sqlite::SqliteSession`)
- [x] Zero-copy cell display: replace `DbValue::to_display_string()` heap cloning with `Cow<'static, str>` / borrowed formatting in `delegate.rs` and fast-path unmodified cells to eliminate 60k-120k string allocations/sec during 60-120 FPS grid scrolling (`quarry_core::value::DbValue`, `grid::delegate`, `grid::mod`)
- [ ] Flat columnar result buffer: replace `Vec<Vec<DbValue>>` with flat column-oriented chunks (`quarry_core::session::QueryResult`, `grid::ResultSet`) to eliminate millions of pointer indirections and heap allocations on 100k-row queries
- [ ] Streaming query cursor: stream database rows in chunked batches (e.g. 1,000 rows) via async channels to the UI grid instead of buffering the entire dataset in memory before rendering (`quarry_core`, `execution.rs`)
- [x] Zero-copy completion context: slice only the active statement around the caret instead of cloning the entire editor rope into a heap string on every keystroke (`completion::SchemaCompletions`)
- [x] Asynchronous autocomplete cancellation: compute schema completions on GPUI's background executor with cooperative cancellation tokens on subsequent keystrokes (`completion.rs`, `editor.rs`)
- [x] Allocation-free fuzzy search: rewrite `fuzzy_score` in `command_bar.rs` to match via `FuzzyQuery` and stack-allocated Damerau-Levenshtein matrix without allocating `Vec<char>` per candidate per keystroke (`command_bar.rs`)
- [x] Retained ER diagram layout: cache BFS graph topology and card coordinates in component state instead of re-running graph traversal on every single frame render (`er_view.rs`)
- [x] Consolidate SQL lexers: share tokenization state machines between `quarry_core::session::sql_tokens` and `quarry_sql::splitter` (`quarry_core::dialect`, `quarry_sql`)
- [x] Driver TLS support (`rustls`): secure direct connections to cloud databases (RDS, Neon, Supabase) without requiring SSH tunnels (`quarry-core/Cargo.toml`, `postgres`, `mysql`)
- [x] Microbenchmarks suite (`criterion`): benchmark SQL statement splitting throughput, completion lookup on 1,000+ tables, fuzzy matching, and grid row decoding (`benches/`)

## Editor Intelligence & Power Editing
- [x] Tree-sitter SQL integration: use Tree-sitter queries (already enabled in `gpui-kit`) for AST-based statement boundaries and syntax folding (`quarry_sql::splitter`, `editor.rs`)
- [x] Multi-cursor selection: support secondary carets (`⌥⌘↑`/`⌥⌘↓` or `⌘D` for next occurrence) in SQL editor (`editor::QueryEditor`)
- [x] Vim mode toggle: optional modal editing (`settings: vim_mode`) for normal/insert/visual command navigation (`settings.rs`, `editor.rs`)

## Configuration & Tooling
- [x] Atomic connection profile permissions: create `connections.json` with `0o600` mode atomically on Unix to prevent TOCTOU permission leaks (`connections::save`)
- [x] Structured diagnostics & startup tracing: configure `tracing-subscriber` with `EnvFilter` and default to `info` (`main.rs`, `execution.rs`, `state.rs`)
- [x] Release build optimization: enable thin LTO, single codegen unit, and symbol stripping (`Cargo.toml`)
- [x] Developer tooling configs: add standardized `Makefile`, `rustfmt.toml`, and `clippy.toml`
- [x] Fast test & lint validation pass: clean fmt, clippy, and all 106 tests passing cleanly
- [x] Live configuration reload: watch `~/.config/quarry/` (`settings.json`, `keymap.json`) via mtime check so edits apply immediately without restarting (`settings.rs`)
- [x] Modularize `main.rs`: extract application menu generation and global command registrations out of the 2,100-line entrypoint (`main.rs`, `menus.rs`)

