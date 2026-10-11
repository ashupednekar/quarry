# Roadmap

Quarry stays small: native, fast and light on RAM. Each item below has to earn its place.

## 1. Pluggable drivers

**Goal:** adding a database means one new module in `quarry-core` and one cargo feature. The app never names a database.

### Today
- The app compares `ConnectionTargetKind` (`state.rs:36`) in about 25 files: `is_pg`, `is_sqlite`, and a `postgres: bool` flag.
- Postgres SQL is spread through the UI: `bottom_panel.rs`, `table_detail.rs`, `export_dialog.rs`, the EXPLAIN and search_path code in `main.rs`, and `grid.rs`.
- `quote_ident`, `to_sql_literal` and `LIMIT/OFFSET` are global and assume one dialect.
- The splitter, params parser and completer assume Postgres.

### Design (all in `quarry-core::driver`)

```rust
/// Static description of a database: registered once, used to build the connect form
pub trait Driver: Send + Sync {
    fn id(&self) -> &'static str;                 // "postgres", stored in profiles
    fn name(&self) -> &'static str;               // "PostgreSQL"
    fn fields(&self) -> &'static [Field];         // connect form; `secret` fields go to the keychain
    fn parse_address(&self, s: &str) -> Option<Params>; // pasted URL / path → params
    fn tunnel_target(&self, p: &Params) -> Option<(String, u16)>; // None = no SSH
    async fn connect(&self, p: &Params) -> Result<Arc<dyn DatabaseSession>, DbError>;
}

pub trait Dialect: Send + Sync {
    fn quote_ident(&self, s: &str) -> String;
    fn literal(&self, v: &DbValue) -> String;
    fn limit(&self, sql: &str, limit: u64, offset: u64) -> String; // LIMIT / FETCH FIRST / TOP
    fn explain(&self, sql: &str, analyze: bool) -> Option<String>;
    fn set_schema(&self, schema: Option<&str>) -> Option<String>;
    fn statement_timeout(&self, secs: u64) -> Option<String>;
    fn type_name(&self, t: &ColumnType) -> String;               // import CREATE TABLE
    fn split_rules(&self) -> SplitRules; // dollar quotes, backticks, `#` comments, `/` and `GO` terminators
    fn keywords(&self) -> &'static [&'static str];               // extra completer keywords
}
```

`DatabaseSession` gains `fn dialect(&self) -> &dyn Dialect`. Features that only some databases support are **optional sub-traits**: the capability is the `Option`, so there are no flags to keep in sync.

```rust
fn monitor(&self) -> Option<&dyn Monitor> { None }        // sessions, locks, top queries, cancel/kill
fn server_logs(&self) -> Option<&dyn ServerLogs> { None }
fn object_detail(&self) -> Option<&dyn ObjectDetail> { None } // DDL, indexes, constraints
fn dumper(&self) -> Option<&dyn Dump> { None }            // pg_dump / built-in sqlite dump
fn plan(&self) -> Option<&dyn PlanParser> { None }        // EXPLAIN output → plan tree
```

The registry is a feature-gated slice with no dynamic loading:

```rust
pub fn drivers() -> &'static [&'static dyn Driver] { &[
    #[cfg(feature = "sqlite")] &sqlite::DRIVER,
    #[cfg(feature = "postgres")] &postgres::DRIVER,
    #[cfg(feature = "oracle")] &oracle::DRIVER,
] }
```

Saved profiles change from `kind + path_or_url` to `driver: String, params: BTreeMap<String, String>`. A one-time migration in `connections.rs` converts old profiles. Passwords stay in the keychain under any field marked `secret`.

### Status
- [x] `Dialect`: SQL built through `session.dialect()`
- [x] Optional capabilities: `Monitor`, `Dumper`, `table_info`
- [x] Driver registry; profiles store a driver id (old files still load); drivers are cargo features
- [ ] Dialect-aware splitter/params/completer: built with the first driver that needs it (MySQL backticks, Oracle `/`)
- [x] Conformance suite: `quarry_core::conformance::check`, passing for Postgres and SQLite
- [x] `DRIVERS.md`

Left in the app by design: the SQLite demo playground, and treating a dropped `.db` file as SQLite.

## 2. Databases

| Order | Database | Crate | Status |
|---|---|---|---|
| 1 | MySQL / MariaDB | `mysql_async` | ✓ passes conformance (MySQL 8.4) |
| 2 | SQL Server | `tiberius` | ✓ passes conformance (SQL Server 2022) |
| 3 | **Oracle** | `oracle` (ODPI-C) | ✓ passes conformance (Oracle 23 Free) |
| 4 | DuckDB | `duckdb` | Large compile, so it's opt-in |

**Oracle specifics.** None of these should touch the app:
- ✓ **Instant Client:** ODPI-C loads it at connect time. If it's missing, the connect form offers a pinned, checksummed Basic Lite download (`oracle/client.rs`); the binary ships without it.
- **Async:** the crate is blocking, so wrap calls in `spawn_blocking`.
- **Identifiers:** schemas are users, unquoted identifiers fold to UPPERCASE (the completer has to respect this), and there's no `BOOLEAN` before 23ai.
- **Dialect:** `FETCH FIRST n ROWS ONLY`; `EXPLAIN PLAN FOR` plus `DBMS_XPLAN`; PL/SQL blocks end with a `/` on its own line (`SplitRules`).
- **Session timeout:** none at the session level, so `statement_timeout` returns `None` and Quarry cancels the call client-side.

## 3. Features

Done: visual EXPLAIN (Postgres JSON plans and SQLite query plans as a tree), functions/sequences/triggers/types in the sidebar (loaded per schema), history search across everything in the palette, Find Usages / Rename for tables, schema diff between connections, and a Diagram view (a table with its FK neighbours).

Earlier: WHERE bar on table tabs, following FKs from cells, JSON/long-text cell editor, query history.

## 4. Staying light
- ✓ **Query results are capped** at `MAX_ROWS` (100k), with the full count shown and "All rows" in Export.
- **Introspect lazily per schema** on large databases. Today the whole database is loaded on connect. Needs completion, squiggles and go-to-definition to handle schemas that aren't loaded yet, or every table in one is flagged unknown.
- **Keep heavy drivers opt-in** behind cargo features.
