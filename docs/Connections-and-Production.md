# Connections and Production

Every developer has a story that starts with "I thought I was connected to staging."
This page is about not having one.

## Tag your environments

Each connection is **Local**, **Staging** or **Production**. The title bar is green, amber or red to match, and staging and prod tabs are tinted too, so you
always see where you are.

On a Production connection, Quarry reads each statement before running it. If it changes anything
(`INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `TRUNCATE`, `GRANT` and friends, including a `WITH`
that hides a `DELETE`) it stops and asks, showing the server and the SQL. A `SELECT` runs without
fuss.

It's a speed bump, not a wall. The point is that the dangerous thing takes one more deliberate click.

## Passwords live in the keychain

Connection details are stored as plain JSON in `~/.config/quarry/connections.json`. You can read it,
diff it, copy it to a new laptop. What you won't find in it is a password: those go to the macOS
Keychain, Secret Service on Linux, or Windows Credential Manager.

## SSH tunnels

The database is in a private network and the only way in is through `bastion.example.com`. Fill in
the SSH section of the connection, and Quarry does what you'd do by hand:

```sh
ssh -N -L 54012:db.internal:5432 me@bastion.example.com
```

It picks a free local port, starts your system's `ssh`, points the driver at the local end, and stops
the tunnel when you disconnect. Because it's your `ssh`, your `~/.ssh/config`, keys and agent all just
work.

## Big servers

Some servers have hundreds of schemas. Quarry loads all of them only if there are eight or fewer;
otherwise it loads the default schema and fetches the rest when you expand them in the sidebar.
Connecting stays fast either way.
