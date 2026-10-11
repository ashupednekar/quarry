# Adding a database

A driver lives entirely in `crates/quarry-core`. The app never names a database: it builds its
connect form, SQL and feature list from what the driver provides. If you find yourself editing
`quarry-app` to support your database, that's a gap in the driver API. Open an issue instead.

## What you write

| Piece | Trait | Required? | What it covers |
|---|---|---|---|
| Driver | `driver::Driver` | yes | Connect form fields, URL ↔ fields, password for the keychain, display address, SSH target, `connect` |
| Session | `session::DatabaseSession` | yes | `execute`, `introspect`, `apply_edits`, transactions, read-only |
| Dialect | `dialect::Dialect` | yes | Quoting, literals, `LIMIT`/`FETCH FIRST`, `EXPLAIN`, schema switching, statement timeout, import types |
| Table info | `DatabaseSession::table_info` | no | DDL and indexes. The default rebuilds `CREATE TABLE` from the catalog. |
| Schema objects | `DatabaseSession::objects` / `object_ddl` | no | Functions, procedures, sequences, triggers, types in the sidebar |
| Query plan | `Dialect::plan` | no | Reads EXPLAIN output as a tree for the plan view |
| Monitor | `monitor::Monitor` via `session.monitor()` | no | Active queries, locks, stats, logs, cancel/kill |
| Dump | `dump::Dumper` via `session.dumper()` | no | Whole-database SQL script |

Optional pieces return `None` by default, and the UI hides what a session doesn't offer.
The `Dialect` methods have ANSI defaults, so override only what your database does differently.

## Steps

1. **Module.** Add `src/<db>.rs` (or `src/<db>/mod.rs` once it grows, like `postgres/`).
2. **Feature.** In `crates/quarry-core/Cargo.toml`, make the client crate optional and add a feature:
   ```toml
   [features]
   mysql = ["dep:mysql_async"]
   ```
   Then gate the module in `lib.rs` with `#[cfg(feature = "mysql")]`.
3. **Register.** Add `&crate::mysql::DRIVER` to `DRIVERS` in `driver.rs`. Order matters for
   pasted-address detection: keep SQLite last, since it accepts any path.
4. **Conform.** Call `conformance::check(&session, "<schema>")` from a test, against a real server:
   ```rust
   #[test]
   fn mysql_conforms() {
       let Ok(url) = std::env::var("QUARRY_MYSQL_URL") else { return };
       let rt = tokio::runtime::Runtime::new().unwrap();
       let s = rt.block_on(MysqlSession::connect(&url, false)).unwrap();
       rt.block_on(quarry_core::conformance::check(&s, "test"));
   }
   ```
   `driver::tests::every_driver_round_trips_its_own_sample` also checks your form fields.
5. **Document.** Add a row to the table below.

## Conventions

- **Values.** Decode into `DbValue` without losing precision: exact numerics go in `Decimal(String)`, and anything without a matching variant stays as its exact `Text`.
- **Big results.** Keep at most `session::MAX_ROWS` rows, and set `rows_affected` to the full count so the UI can say the result was cut off.
- **URLs.** `conn_url` parses and builds `scheme://user:pw@host:port/db`. Use it rather than writing another parser.
- **Splitting.** Override `Dialect::split_rules` if something hides a `;` that isn't covered by quotes, `--` or `/* */`.
- **Error positions.** Append ` (at character N)` (1-based) to an error message when the server reports one, and the editor underlines it (`session::error_position`).
- **Blocking clients** (e.g. `oracle`) run inside `tokio::task::spawn_blocking`. Never block the runtime.
- **Native client libraries** (Oracle Instant Client) load at connect time. If one is missing, return a `DbError::ConnectionFailed` that says what to install. Don't crash.
- **Monitor views.** In the `Activity` view, put the backend id in the first column, because that's what cancel/terminate receive.
- **No new dependencies** in `quarry-app` for a driver.

## Running the tests

```bash
cargo test --workspace                                   # unit tests, SQLite conformance
docker run -d --rm --name quarry-pg -e POSTGRES_PASSWORD=postgres -p 55432:5432 postgres:16
QUARRY_PG_URL=postgres://postgres:postgres@localhost:55432/postgres cargo test -p quarry-core
docker run -d --rm --name quarry-mysql -e MYSQL_ROOT_PASSWORD=mysql -e MYSQL_DATABASE=quarry_test -p 53306:3306 mysql:8.4
QUARRY_MYSQL_URL=mysql://root:mysql@127.0.0.1:53306/quarry_test cargo test -p quarry-core
docker run -d --rm --name quarry-oracle -e ORACLE_PASSWORD=oracle -e APP_USER=quarry -e APP_USER_PASSWORD=quarry -p 51521:1521 gvenzl/oracle-free:23-slim-faststart
ORACLE_CLIENT_LIB_DIR=~/instantclient_23_3 QUARRY_ORACLE_URL=oracle://quarry:quarry@127.0.0.1:51521/FREEPDB1 cargo test -p quarry-core
# SQL Server is amd64-only: on Apple Silicon it needs Rosetta (Docker Desktop, or `colima start --vm-type vz --vz-rosetta`)
docker run -d --rm --platform linux/amd64 --name quarry-mssql -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD=Quarry_test1 -p 51433:1433 mcr.microsoft.com/mssql/server:2022-latest
QUARRY_MSSQL_URL=sqlserver://sa:Quarry_test1@127.0.0.1:51433/master cargo test -p quarry-core
cargo test -p quarry-core --no-default-features --features sqlite   # one driver alone
```

## Drivers

| Database | Feature | Client | Monitor | Dump | Notes |
|---|---|---|---|---|---|
| PostgreSQL | `postgres` | tokio-postgres | ✓ | pg_dump | SSH tunnel, `search_path` per tab |
| SQLite | `sqlite` | rusqlite (bundled) | n/a | built in | `:memory:` connections aren't saved |
| MySQL / MariaDB | `mysql` | mysql_async | processlist, statements | mysqldump | Databases are schemas; `#` comments and `\'` escapes split correctly |
| Oracle | `oracle` | rust-oracle (ODPI-C) | sessions, SQL, locks | n/a | Needs Instant Client at connect time (`ORACLE_CLIENT_LIB_DIR`); users are schemas; PL/SQL blocks end at `/` |
| SQL Server | `mssql` | tiberius (TDS 7.3, no TLS yet) | requests, query stats, locks | n/a | `GO` batches; schemas of the connected database |
