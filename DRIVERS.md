# Adding a database

Want Quarry to support another database? Great, a new driver is one of the best ways to contribute.

The good news: **everything happens in one crate, `crates/quarry-core`.** The app builds its connect form, SQL and menus from what your driver tells it, so you never touch the UI. If you find you need to, open an issue and we'll fix the driver API instead.

Not sure where to start? [Open an issue](https://github.com/thekodeking/quarry/issues) saying which database you'd like to add, and we'll help you get going.

## The short version

1. **Copy a driver.** Start from the one closest to your database: `mysql/` for a client-server database, `sqlite.rs` for an embedded one. Put yours in `crates/quarry-core/src/<db>/mod.rs`.
2. **Add a feature** in `crates/quarry-core/Cargo.toml`, so your client library is optional:
   ```toml
   [features]
   mydb = ["dep:mydb-client"]
   ```
   Then gate the module in `lib.rs` with `#[cfg(feature = "mydb")]`.
3. **Register it.** Add `&crate::mydb::DRIVER` to the `DRIVERS` list in `driver.rs`. Keep SQLite last.
4. **Test it** against a real server (see [Testing](#testing)).
5. **Add a row** to the [table below](#supported-databases) and open a pull request.

## What a driver needs

Three pieces are required. Everything else is optional, and the UI simply hides features your driver doesn't offer yet, so a small first version is welcome.

| Piece | What it does |
|---|---|
| `Driver` (`driver.rs`) | The connect form, parsing connection URLs, and connecting |
| `DatabaseSession` (`session.rs`) | Running queries, reading the schema, applying grid edits, transactions |
| `Dialect` (`dialect.rs`) | How your SQL differs: quoting, `LIMIT`, `EXPLAIN` and so on. It has standard SQL defaults, so override only what's different |

Optional, add them later if you like: DDL and indexes, functions and other schema objects, query plans, the activity monitor, and database dumps.

## A few rules

- **Don't lose precision.** Exact numbers go in `DbValue::Decimal`. If there's no matching type, keep the value as text.
- **Cap big results** at `session::MAX_ROWS` rows, but report the full row count in `rows_affected`.
- **Don't block the async runtime.** Wrap blocking client libraries in `tokio::task::spawn_blocking`.
- **Fail with a helpful message.** If a native library is missing, return `DbError::ConnectionFailed` saying what to install, instead of crashing.
- **Reuse `conn_url`** to parse `scheme://user:pw@host:port/db` URLs.

Once it works, a few touches make it feel native:

- Append ` (at character N)` to an error when the server reports a position, and the editor underlines it.
- Override `Dialect::split_rules` if your SQL hides a `;` somewhere other than quotes or comments.
- In the monitor's `Activity` view, put the backend id in the first column, since cancel and kill use it.

For anything else, read an existing driver. They're the best reference.

## Testing

`conformance::check` runs the same checks against every driver. Add a test for yours to `crates/quarry-core/tests/live.rs`, following the others there. It only runs when its URL variable is set:

```rust
#[cfg(feature = "mydb")]
#[test]
fn mydb_conforms() {
    let Ok(url) = std::env::var("QUARRY_MYDB_URL") else { return };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(MydbSession::connect(&url, false)).unwrap();
    rt.block_on(quarry_core::conformance::check(&s, "test"));
}
```

Then start a server in Docker and run it:

```bash
cargo test --workspace                       # everything that needs no server
QUARRY_MYDB_URL=mydb://user:pw@localhost:5000/test cargo test -p quarry-core
```

<details>
<summary>Commands for the existing databases</summary>

```bash
# PostgreSQL
docker run -d --rm --name quarry-pg -e POSTGRES_PASSWORD=postgres -p 55432:5432 postgres:16
QUARRY_PG_URL=postgres://postgres:postgres@localhost:55432/postgres cargo test -p quarry-core

# MySQL
docker run -d --rm --name quarry-mysql -e MYSQL_ROOT_PASSWORD=mysql -e MYSQL_DATABASE=quarry_test -p 53306:3306 mysql:8.4
QUARRY_MYSQL_URL=mysql://root:mysql@127.0.0.1:53306/quarry_test cargo test -p quarry-core

# Oracle: Quarry's own Instant Client download (Connect → Download) is found automatically.
# Instant Client 23.3 crashes on macOS 27.
docker run -d --rm --name quarry-oracle -e ORACLE_PASSWORD=oracle -e APP_USER=quarry -e APP_USER_PASSWORD=quarry -p 51521:1521 gvenzl/oracle-free:23-slim-faststart
QUARRY_ORACLE_URL=oracle://quarry:quarry@127.0.0.1:51521/FREEPDB1 cargo test -p quarry-core

# SQL Server is amd64-only. On Apple silicon it needs Rosetta
# (Docker Desktop, or `colima start --vm-type vz --vz-rosetta`).
docker run -d --rm --platform linux/amd64 --name quarry-mssql -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD=Quarry_test1 -p 51433:1433 mcr.microsoft.com/mssql/server:2022-latest
QUARRY_MSSQL_URL=sqlserver://sa:Quarry_test1@127.0.0.1:51433/master cargo test -p quarry-core

# One driver on its own
cargo test -p quarry-core --no-default-features --features sqlite
```

</details>

## Supported databases

| Database | Feature | Client library | Activity monitor | Dump | Notes |
|---|---|---|---|---|---|
| PostgreSQL | `postgres` | tokio-postgres | ✓ | pg_dump | SSH tunnel, `search_path` per tab |
| SQLite | `sqlite` | rusqlite (bundled) | n/a | built in | `:memory:` connections aren't saved |
| MySQL / MariaDB | `mysql` | mysql_async | processlist, statements | mysqldump | Databases are schemas; `#` comments and `\'` escapes split correctly |
| Oracle | `oracle` | rust-oracle (ODPI-C) | sessions, SQL, locks | – | Needs Oracle Instant Client (`ORACLE_CLIENT_LIB_DIR`, a system install, or the download the connect form offers); users are schemas; PL/SQL blocks end at `/` |
| SQL Server | `mssql` | tiberius (TDS 7.3, no TLS yet) | requests, query stats, locks | – | `GO` batches; schemas of the connected database |
