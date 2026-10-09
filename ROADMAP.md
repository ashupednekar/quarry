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

### Steps (one PR each; the app keeps working after every step)
1. **Dialect.** Add the trait and implement it for Postgres and SQLite. Route `quote_ident`, `to_sql_literal`, `with_limit`, EXPLAIN, `search_path`, timeout, and the import types through `session.dialect()`.
2. **Optional sub-traits.** Move the `pg_*` SQL out of `bottom_panel.rs`, `table_detail.rs` and `export_dialog.rs` into the drivers. The UI checks `session.monitor().is_some()` instead of the database type.
3. **Registry and profiles.** Build the connect form from `fields()`, write the profile migration, make SSH generic through `tunnel_target`, and remove `ConnectionTargetKind`.
4. **Dialect-aware SQL text.** Parameterise the splitter, params parser and completer with `SplitRules` and the dialect keywords.
5. **Conformance suite.** Generalise `test_sqlite_in_memory_flow` into `driver::conformance(session)`, covering the type round-trip, edit conflicts and transactions. CI runs it against Docker images.
6. **`DRIVERS.md`.** Write the contributor guide plus a capability table.

**Done when:** `grep -rn 'postgres\|sqlite' crates/quarry-app/src` finds only the demo database seed.

## 2. Databases

| Order | Database | Crate | Notes |
|---|---|---|---|
| 1 | MySQL / MariaDB | `mysql_async` | Backticks, `#` comments, `LIMIT`; tests the abstraction |
| 2 | SQL Server | `tiberius` | `TOP` / `OFFSET FETCH`, `GO` batches |
| 3 | **Oracle** | `oracle` (ODPI-C) | See below |
| 4 | DuckDB | `duckdb` | Large compile, so it's opt-in |

**Oracle specifics.** None of these should touch the app:
- **Instant Client:** ODPI-C loads it at connect time. If it's missing, show "install Oracle Instant Client" rather than crashing, and ship the binary without it.
- **Async:** the crate is blocking, so wrap calls in `spawn_blocking`.
- **Identifiers:** schemas are users, unquoted identifiers fold to UPPERCASE (the completer has to respect this), and there's no `BOOLEAN` before 23ai.
- **Dialect:** `FETCH FIRST n ROWS ONLY`; `EXPLAIN PLAN FOR` plus `DBMS_XPLAN`; PL/SQL blocks end with a `/` on its own line (`SplitRules`).
- **Session timeout:** none at the session level, so `statement_timeout` returns `None` and Quarry cancels the call client-side.

## 3. Features

Already shipped (DataGrip parity): raw WHERE bar on table tabs, following FKs from cells, JSON/long-text cell editor, query history (bottom panel and palette).

| # | Feature | Plugs into | Kept small by |
|---|---|---|---|
| 1 | **Visual EXPLAIN** | `PlanParser` → plan tree in the results area | Tree with cost/time bars only; no graph canvas |
| 2 | **More objects in sidebar**: functions, sequences, triggers, enums | `SchemaCatalog` + `ObjectDetail` | Loaded lazily when a schema is expanded |
| 3 | **Searchable history** | `history.rs` + palette | Fuzzy search over the existing jsonl; filter by connection |
| 4 | **Find usages / rename** a table or column in open queries | `quarry_sql::table_refs` | Open tabs and the queries folder only |
| 5 | **Schema diff** between two connections | `SchemaCatalog` compare | Read-only; outputs an ALTER script into a new tab |
| 6 | **Mini ER view** | FKs already in `TableSchema` | One table plus its direct neighbours |

## 4. Staying light
- **Cap query-tab results.** Fetch the first N rows and page the rest, as table tabs already do. Today the whole `QueryResult` is held in memory.
- **Introspect lazily per schema** on large databases. Today the whole database is loaded on connect.
- **Keep heavy drivers opt-in** behind cargo features.
