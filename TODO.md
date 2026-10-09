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
- [ ] Keep Tab focus inside the open modal; gpui cycles focus across the whole window. Doable by giving `modal_card` a FocusHandle and stepping `focus_next` until focus is back inside, but that touches all 11 modals
- [ ] One `Option<ActiveModal>` instead of separate `*_open` flags (internal cleanup, no user-facing change)

## Layout
- [x] Maximise the editor (⇧⎋): hides sidebar and bottom panel, again to restore
- [x] Tab headers: staging/prod underline on table tabs, unsaved dot (already existed), right-click menu
- [x] Empty states with a primary button (sidebar, no tabs open)
- [x] Panes remember their size across restarts (`layout.rs`)
- [ ] ~~In-memory SQLite connections kept across restarts~~ won't do: the data can't come back, and the demo reseeds from "Open the demo database"
