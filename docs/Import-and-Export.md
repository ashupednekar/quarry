# Moving Data In and Out

Sooner or later someone hands you a spreadsheet. Sooner or later someone asks for one.

## In

Marketing sent `leads.csv`:

```csv
name,email,signed_up
Linus,linus@example.com,2026-03-01
Margaret,margaret@example.com,2026-03-04
```

Open the import wizard from the command bar (`⌘K`, "import"), pick the file, and Quarry takes a look.
It detects CSV or JSON, the delimiter and the encoding, and guesses types per column: `signed_up`
looks like a date, so it's a date.

Import into an existing table, or let Quarry create one from the guessed types. Creating the table
and loading the rows happen in one transaction, so a bad row on line 9,000 doesn't leave you with half
a table.

What about duplicates? Pick one:

- **Error**: a plain `INSERT`. A conflict stops the import. The safe default.
- **Skip duplicates**: `ON CONFLICT DO NOTHING`. Rows already there stay as they are.

## Out

Run a query, then `⇧⌘E`. You get three formats:

- **CSV** for the spreadsheet people.
- **JSON** for the script people.
- **SQL** `INSERT` statements, for moving rows to another database.

Export streams. It doesn't build the whole file in memory first, so it works for results much larger
than the grid will show you (the grid stops at 100,000 rows).

## Whole databases

For a full dump, `⌘K` → "Dump Database to SQL…". Postgres and MySQL use `pg_dump` and `mysqldump` if you have
them; SQLite has its own built in.
