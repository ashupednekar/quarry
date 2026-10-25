# Editing Data Safely

Back to Grace, whose email is `grace@exmaple.com`. Let's fix it, and along the way see why editing a
grid in Quarry is boring in the best way.

## The edit

Open the `customers` table, select the email cell, press `Enter` (or `F2`), type `grace@example.com`, press `Tab`.

Nothing happened to the database. The cell is marked as changed, and that's all. You can keep going:
`⌘N` adds a row, `⌘⌫` deletes one, `⌘Z` undoes, `⌥⌘N` sets a cell to `NULL`.

## The review

Press `⌘S`. Quarry shows you what it's going to run:

```sql
UPDATE "customers"
SET "email" = 'grace@example.com'
WHERE "id" = 2 AND "email" = 'grace@exmaple.com';
```

Read that `WHERE` clause again. It doesn't just say *which row*. It also says *what you thought was
in it*.

## Why the extra condition

Imagine your colleague, at the same moment, opens the same row and fixes the email too, but to
`grace.hopper@example.com`, which is what Grace actually asked for. They save first.

A naive client would run `UPDATE customers SET email = ... WHERE id = 2`, quietly overwrite their fix,
and nobody would know until Grace calls again.

Quarry's `UPDATE` matches zero rows instead, because the email is no longer `grace@exmaple.com`.
Zero rows means someone got there first, so Quarry rolls back the whole batch and tells you. You
refresh, see the new value, and decide what's right. This trick is called optimistic concurrency
control: assume nobody else is editing, but check before you write.

All of your staged changes go in one transaction. Either they all land, or none do.

## Following the thread

Grace has orders. Click a `customer_id` cell in `orders` and press `F4` to jump to the customer it
points at. Foreign keys are links; Quarry treats them that way.

A few other grid niceties:

- `⌥R` flips the grid sideways, one row as a list of fields. Great for wide tables.
- `⌥⌘F` opens the filter builder, for `WHERE` clauses without typing them.
- `⇧↵` opens a bigger editor for long text and JSON.
