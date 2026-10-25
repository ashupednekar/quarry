# Quarry

It's 4:55 on a Friday. Support pings you: a customer's email address is wrong and they can't log in.
You open your database client. It takes eleven seconds to start. It wants to index something. A fan spins up.

Quarry is the client for that moment. It opens fast, it shows you the row, you fix the cell, and before
anything touches the database it shows you the exact SQL it's about to run. Then you go home.

That's the whole pitch: the parts of a database IDE you use every day, without the IDE.

## Pages

- [Getting Started](Getting-Started): install it, connect, run your first query.
- [Writing Queries](Writing-Queries): the editor, completion, parameters, and the little tricks.
- [Editing Data Safely](Editing-Data-Safely): how a cell edit becomes an `UPDATE`, and why it won't clobber your colleague's.
- [Moving Data In and Out](Import-and-Export): CSV and JSON in, CSV, JSON and SQL out.
- [Connections and Production](Connections-and-Production): SSH tunnels, the keychain, and the guard rail on prod.
- [How Quarry Works](How-Quarry-Works): a short tour of the internals.
- [Adding a Database](Adding-a-Database): write a driver without touching the UI.

For the long-form internals reference, see [Architecture](ARCHITECTURE).
