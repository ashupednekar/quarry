# Writing Queries

A SQL editor has one job: get out of the way between your idea and the result. Here's how Quarry's tries.

## Completion that has read your schema

Type `SELECT * FROM cu` and you get `customers`. That much is table stakes. The more interesting part
is what happens next:

```sql
SELECT * FROM customers c JOIN |
```

Quarry knows `orders.customer_id` references `customers.id`, so it offers the whole thing:

```sql
JOIN orders o ON o.customer_id = c.id
```

It also knows about the tables you invented a few lines up:

```sql
WITH big_spenders AS (
  SELECT customer_id, sum(total) AS spent FROM orders GROUP BY customer_id
)
SELECT spent FROM big_spenders WHERE sp|
```

`spent` completes, even though no such table exists in the database. Same for `FROM (SELECT ...) alias`.

Matching is forgiving. `cid` finds `customer_id`. When two candidates tie, the one you actually use
more wins, because Quarry remembers which identifiers you run.

## Parameters

Debugging a query you copied out of application code? It's full of placeholders:

```sql
SELECT * FROM orders WHERE customer_id = :id AND total > $1;
```

Run it as is. Quarry asks for `:id` and `$1`, then fills them in. It's smart enough to leave
`'12:30'`, `-- :comment` and Postgres's `::int` casts alone.

## Code actions

Put the cursor on a query and press `⌥↵`:

- **Expand `*`** into the real column list. Handy right before you trim it down.
- **Add `LIMIT`**, for when you're about to `SELECT *` from a table with forty million rows.
- **Wrap in a transaction**, for when you're about to do something you might regret.

`⇧⌥F` formats the SQL. `⌥⌘E` shows the query plan as a tree.

## Statements, not files

`⌘↵` runs the statement under the cursor, which means Quarry has to know where statements end.
That's harder than splitting on `;`:

```sql
SELECT 'a;b';                                    -- the ; is in a string
CREATE FUNCTION f() RETURNS int AS $$ SELECT 1; $$ LANGUAGE sql;   -- Postgres dollar quotes
SELECT 1 # MySQL comment; still one statement
```

SQL Server scripts split on `GO`, Oracle PL/SQL blocks end on a line with a lone `/`. Each driver
tells the splitter its rules, so you don't have to think about it.

## Vim

Settings (`⌘,`) → Vim mode. Normal, Insert and Visual, with an optional block cursor.
