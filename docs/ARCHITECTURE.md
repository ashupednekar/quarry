# Architecture

This page follows one query through Quarry, from the keypress to the pixels, and stops at each
interesting idea along the way. If you want to change Quarry, this is the map. If you just want the
five-minute version, read [How Quarry Works](How-Quarry-Works) instead.

Our running example is the query from [Getting Started](Getting-Started):

```sql
SELECT c.name, sum(o.total) AS spent
FROM customers c
JOIN orders o ON o.customer_id = c.id
GROUP BY c.name;
```

You press `⌘↵`. What happens?

## The lay of the land

Quarry is a Cargo workspace with three crates, layered like a cake:

```
┌──────────────────────────────────────────────┐
│ quarry-app    windows, tabs, the grid, GPUI  │
├──────────────────────────────────────────────┤
│ quarry-sql    reads SQL text: split,         │
│               complete, parameters, rewrites │
├──────────────────────────────────────────────┤
│ quarry-core   talks to databases: drivers,   │
│               values, results, edits, I/O    │
└──────────────────────────────────────────────┘
```

Each layer only knows about the ones below it. `quarry-core` has no idea there is a window. That's not
purity for its own sake: it's what lets you [add a database](Adding-a-Database) without opening a UI
file, and test the whole engine with `cargo test` and no display.

A quick lookup table for "where does X live?":

| You're looking for | Go to |
|---|---|
| A database's wire-level code | `crates/quarry-core/src/<db>/` (`sqlite.rs` for SQLite) |
| The traits every driver implements | `quarry-core/src/driver.rs`, `session.rs`, `dialect.rs` |
| How a result is stored | `quarry-core/src/row_buffer.rs`, `value.rs` |
| Staged grid edits and their SQL | `quarry-core/src/edits.rs` |
| Statement splitting | `quarry-sql/src/splitter.rs` |
| Autocomplete | `quarry-sql/src/completer.rs` |
| Running a query from the UI | `quarry-app/src/execution.rs`, `runtime.rs` |
| The data grid | `quarry-app/src/grid/` |

## Step 1: which statement?

Your editor might hold twenty queries. `⌘↵` should run the one under the cursor, so first Quarry has to
cut the text into statements. Splitting on `;` is the obvious answer, and the obvious answer is wrong:

```sql
SELECT 'a;b';                                  -- ; inside a string
SELECT 1; -- ; inside a comment ;
CREATE FUNCTION f() RETURNS int AS $$
  SELECT 1;                                    -- ; inside a Postgres dollar quote
$$ LANGUAGE sql;
```

So the splitter (`quarry-sql/src/splitter.rs`) doesn't look at characters, it looks at *tokens*. The
lexer in `quarry-core/src/lexer.rs` turns text into words, strings, comments and punctuation. Each
token is a slice of the original text, so nothing is copied. A `;` only ends a statement if it's a
punctuation token, not part of a string or comment.

Databases disagree about the rest, so each driver's `Dialect` hands the splitter a small set of rules:
MySQL treats `#` as a comment, SQL Server scripts split on `GO`, Oracle PL/SQL blocks end on a line with
a lone `/`. One splitter, many dialects, no `if mysql` scattered around.

(The little run buttons in the editor gutter use a Tree-sitter parse to find statement starts. Running
a query always uses the lexer-based splitter.)

## Step 2: hand it off, never wait

Now we have our `SELECT`. The single most important rule in Quarry is this:

> The thread that draws the window never waits for a database.

Databases are slow, networks are slower, and a client that freezes while a query runs feels broken.
So Quarry runs two worlds side by side:

- **The GPUI main thread** handles input and draws frames. Nothing else.
- **`DB_RUNTIME`**, a Tokio runtime with 4 to 16 worker threads, does all network and disk work.
  It lives in `quarry-app/src/runtime.rs`.

Two small functions bridge them. `run_db` is for one-shot jobs (ping, load a table's DDL): run a
future on the runtime, then call back into the UI with the answer. `stream_db` is for queries:

```rust
pub fn stream_db<T, C, R, F, Fut>(
    window: &Window,
    cx: &mut Context<T>,
    make_fut: F,          // the work, given a channel Sender to push chunks into
    on_chunk: impl FnMut(&mut T, C, &mut Window, &mut Context<T>),
    on_done: impl FnOnce(&mut T, R, &mut Window, &mut Context<T>),
)
```

Inside, it's a bounded channel of 64 chunks. The worker pushes, the UI pulls each chunk, appends it,
and repaints:

```
 DB_RUNTIME worker                         GPUI main thread
 ─────────────────                         ────────────────
 send SQL ──────────► database
 rows 1–1000    ◄──── ...
 decode → chunk ──┐
                  │  channel (64 slots)
                  └──────────────────────► append, repaint
 rows 1001–2000 ...                        (you're already looking at row 1)
```

Two nice properties fall out of this for free:

- **Backpressure.** If the UI falls behind, the channel fills up and the worker waits. Memory can't
  balloon because the database is faster than the screen.
- **Clean shutdown.** If the view that asked for the rows is gone, the UI side stops reading and drops
  its end of the channel. The worker's next send fails, and it stops. No flag to check, no thread to
  kill.

## Step 3: the driver

The worker doesn't know about Postgres or SQLite. It knows about three traits:

- **`Driver`** (`driver.rs`) is everything *before* a connection exists: the fields on the connect
  form, parsing a pasted URL, pulling the password out so it can go to the keychain, and `connect()`.
- **`DatabaseSession`** (`session.rs`) is a live connection: run SQL, stream results, read the schema,
  apply edits, manage transactions.
- **`Dialect`** (`dialect.rs`) is how this database's SQL is different: quoting identifiers, writing
  `LIMIT`, building an `EXPLAIN`. It ships with standard SQL defaults, so a driver overrides only
  what it must.

Drivers are plain statics in a list, each behind a Cargo feature:

```rust
static DRIVERS: &[&dyn Driver] = &[
    #[cfg(feature = "postgres")] &crate::postgres::DRIVER,
    #[cfg(feature = "mysql")]    &crate::mysql::DRIVER,
    #[cfg(feature = "mssql")]    &crate::mssql::DRIVER,
    #[cfg(feature = "oracle")]   &crate::oracle::DRIVER,
    #[cfg(feature = "sqlite")]   &crate::sqlite::DRIVER,
];
```

When you paste `mysql://root@localhost/shop`, Quarry asks each driver in turn, "is this yours?"
(`Driver::recognizes`). SQLite goes last because it's the most generous: almost any path looks like a
file to it.

Not every database can do everything. Oracle has no dump yet; SQLite has no server to monitor. The UI
asks the driver what it supports and hides the rest, so a new driver can start with the basics.

Some client libraries are blocking (Oracle's ODPI-C, SQLite). Those drivers wrap their calls in
`tokio::task::spawn_blocking`, so a slow Oracle query can't stall the runtime's other workers.

## Step 4: rows become columns

Results arrive. Each value becomes a `DbValue` (`value.rs`), one enum for every database:

```rust
pub enum DbValue {
    Null, Boolean(bool), Int(i64), Float(f64),
    Decimal(String),   // NUMERIC kept as text: 0.1 + 0.2 stays honest
    Text(String), Blob(Vec<u8>), Timestamp(DateTime<Utc>),
    Json(serde_json::Value),
    Expr(String),      // raw SQL like DEFAULT, for edits
}
```

Note `Decimal(String)`. Money columns are exact in the database, and they stay exact here. Converting
`19.99` to a float to display it is how you get `19.989999999999998` in a spreadsheet.

Then the values go into a `RowBuffer` (`row_buffer.rs`). The textbook layout for a table is a list of
rows. Quarry flips it:

```
Row-major (the textbook)           Column-major (Quarry)
[1, "Ada",   42.5 ]                id     [ 1,     2,       3       ]
[2, "Grace", 19.0 ]                name   [ "Ada", "Grace", "Linus" ]
[3, "Linus", 99.95]                total  [ 42.5,  19.0,    99.95   ]
```

```rust
pub struct RowBuffer {
    row_count: usize,
    columns: Vec<Vec<DbValue>>,
    order: Option<Vec<usize>>,   // order[display_row] = physical_row
}
```

Why bother?

- **Fewer allocations.** 100,000 rows × 20 columns is 100,000 row vectors the textbook way. Here it's 20.
- **Aggregates are a straight walk.** Select a column and the status bar shows sum, avg, min, max.
  That's one pass down one contiguous vector.
- **Sorting moves nothing.** Click a column header and Quarry sorts a list of indices, `order`, and
  leaves the data where it is. Sorting by `total` descending just sets `order = [2, 0, 1]`.

Results are capped at `MAX_ROWS` (100,000) so a careless `SELECT *` can't eat your RAM. The real row
count is still reported, and export streams straight from the database, so the cap never limits what
you can get out. Chunks are `STREAM_CHUNK_SIZE` (1,000) rows. Both constants are in `session.rs`.

## Step 5: pixels

The window is built with [GPUI](https://www.gpui.rs), the GPU-rendered UI framework from Zed. The
grid (`quarry-app/src/grid/`) is virtualized: each frame it draws only the cells you can see. Having
100,000 rows costs memory, not frame time.

UI state lives in GPUI entities. When a background job finishes, it updates the entity and calls
`cx.notify()`, and GPUI redraws what depends on it. That's the whole reactivity story.

## Side quest: editing a cell

Now you notice Grace's email is `grace@exmaple.com` and fix it in the grid. This is the part of
Quarry most worth understanding, because it's the part that protects your data.

**An edit is data, not SQL.** Typing in a cell records a `PendingCellEdit` in `PendingChanges`
(`edits.rs`): the row's primary key, the column, the old value, the new value. Nothing is sent.
Because edits are just records, undo is popping a list, and the review dialog can show you exactly
what will happen.

**Press `⌘S` and the records become SQL**, via `generate_sql`, quoted by the connection's `Dialect`:

```sql
UPDATE "customers" SET "email" = 'grace@example.com'
WHERE "id" = 2 AND "email" = 'grace@exmaple.com';
```

That last condition is the clever bit. The `WHERE` says not just *which row* but *what you saw in
it*. If a colleague changed that email while you were typing, the row no longer matches, and the
`UPDATE` touches zero rows.

**Zero rows means "someone got there first".** `apply_edits` runs all your staged changes in one
transaction and checks every affected-row count. Any mismatch rolls the whole batch back and returns
`DbError::ConcurrentModification`. You refresh, see the new value, and decide. Nobody's work is
silently overwritten.

This pattern is called optimistic concurrency control: don't lock anything, but check before you
write. Every driver implements it, and `conformance::check` tests it against real servers.

## Side quest: autocomplete

While you typed `JOIN o`, Quarry offered `orders o ON o.customer_id = c.id`. Here's how
(`quarry-sql/src/completer.rs`):

1. **Lex the current statement** and find where the cursor is: after `FROM`? in a `WHERE`? in the
   select list?
2. **Collect what's in scope**: tables and their aliases, CTEs from `WITH`, and the columns of
   subqueries in `FROM (SELECT ...) x`. That's why the columns of a CTE you wrote a minute ago
   complete even though no such table exists.
3. **Generate candidates.** After `JOIN`, check foreign keys in the schema catalog to offer complete
   join conditions.
4. **Rank them.** A prefix match beats a substring match, which beats a fuzzy one (`cid` →
   `customer_id`). Ties go to whatever you run most often, counted from your query history.

The schema catalog behind all this is loaded lazily on big servers: if a database has more than
eight schemas (`EAGER_SCHEMAS` in `schema.rs`), Quarry loads only the default one up front and the
rest when you expand them.

## The edges: secrets, tunnels, and prod

**Connections** are plain JSON in `~/.config/quarry/connections.json`. Before saving, the driver's
`take_secret` pulls the password out of the connection string, and it goes to the OS keychain via the
`keyring` crate (`quarry-app/src/connections.rs`). The file is safe to read, diff and back up.

**SSH tunnels** use your system's `ssh` rather than a Rust SSH library (`quarry-app/src/ssh.rs`).
Quarry picks a free local port, runs `ssh -N -L <port>:<db_host>:<db_port> <bastion>`, and the driver's
`through_tunnel` rewrites the connection to point at `localhost:<port>`. Using the real `ssh` means
your `~/.ssh/config`, keys, agent and jump hosts work exactly as they do in your terminal.

**Production** connections run every statement through `is_mutation` (`session.rs`) first. It reads
the first keyword, sees through a `WITH` to find a hidden `DELETE`, and if the statement writes
anything, Quarry asks before running it.

## Where to go next

- [Adding a Database](Adding-a-Database): implement the three traits and run the conformance suite.
- `make test` runs everything that doesn't need a server; `make bench` runs the Criterion benchmarks
  for the core and SQL crates.
- When in doubt, read an existing driver. `sqlite.rs` is the shortest.
