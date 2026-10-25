# How Quarry Works

A short tour, for the curious and for contributors. The long version is [Architecture](ARCHITECTURE).

## Three crates

```
quarry-core   talks to databases. No UI.
quarry-sql    understands SQL text: splitting, completion, parameters, rewrites.
quarry-app    the window. Built on GPUI, the GPU UI framework from Zed.
```

The arrow only points one way: the app knows about the core, the core has never heard of the app.
That's why [adding a database](Adding-a-Database) never touches UI code.

## The UI thread never waits

The rule: the thread drawing the window does nothing but draw the window. Queries run on a separate
Tokio runtime. When you press `⌘↵`:

1. The UI thread finds the statement under the cursor and hands it off.
2. A worker sends it to the database and reads rows back 1,000 at a time.
3. Each batch goes over a channel to the UI, which appends it and repaints.

So the first rows show up while the rest are still on the wire, and a slow query never freezes the
window. You can scroll, switch tabs, or cancel.

## Columns, not rows

The obvious way to hold a result is a list of rows, each a list of values. For 100,000 rows that's
100,000 little allocations. Quarry stores it the other way around, one vector per column:

```
id     [  1,     2,       3 ]
name   [ "Ada", "Grace", "Linus" ]
total  [ 42.5,   19.0,    99.95 ]
```

Twenty columns, twenty allocations. Summing a column is a walk down one vector. And sorting doesn't
move any data: Quarry sorts a list of row indices, `[2, 0, 1]`, and reads through it.

The grid only draws the rows and columns you can see, so 100,000 rows scroll like 100.

## Drivers are three traits

- `Driver`: the connect form, URL parsing, connecting.
- `DatabaseSession`: run queries, read the schema, apply edits.
- `Dialect`: how this database's SQL differs: quoting, `LIMIT`, `EXPLAIN`.

The UI asks the driver what it can do and hides what it can't. A new driver can start small.

## Edits are data

A cell edit isn't SQL until you press `⌘S`. Until then it's a record: the row's primary key, the
column, the old value, the new one. That's what makes undo trivial, the review dialog honest, and the
[concurrency check](Editing-Data-Safely#why-the-extra-condition) possible.
