// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Built-in SQLite dump (X3): schema + data as a replayable SQL script.

use crate::edits::quote_ident;
use crate::session::DatabaseSession;
use crate::value::DbValue;
use std::io::Write;

/// Writes `BEGIN; CREATE TABLE …; INSERT …; CREATE INDEX/VIEW/TRIGGER …; COMMIT;`. Returns rows written.
pub async fn sqlite_dump(session: &dyn DatabaseSession, out: &mut impl Write) -> Result<usize, String> {
    let objects = session
        .execute("SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type = 'table' DESC, rowid")
        .await
        .map_err(|e| e.to_string())?;
    let text = |v: &DbValue| v.to_display_string();
    let io = |e: std::io::Error| e.to_string();

    writeln!(out, "-- Quarry SQLite dump\nPRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;").map_err(io)?;
    let mut rows = 0;
    // Tables (with their data) first, so indexes, views and triggers can refer to them
    for obj in objects.rows.iter().filter(|o| text(&o[0]) == "table") {
        let (name, sql) = (text(&obj[1]), text(&obj[2]));
        writeln!(out, "{sql};").map_err(io)?;
        let data = session.execute(&format!("SELECT * FROM {}", quote_ident(&name))).await.map_err(|e| e.to_string())?;
        let cols: Vec<String> = data.columns.iter().map(|c| quote_ident(c)).collect();
        for row in &data.rows {
            let values: Vec<String> = row.iter().map(|v| v.to_sql_literal()).collect();
            writeln!(out, "INSERT INTO {} ({}) VALUES ({});", quote_ident(&name), cols.join(","), values.join(",")).map_err(io)?;
        }
        rows += data.rows.len();
    }
    for obj in objects.rows.iter().filter(|o| text(&o[0]) != "table") {
        writeln!(out, "{};", text(&obj[2])).map_err(io)?;
    }
    writeln!(out, "COMMIT;").map_err(io)?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteSession;

    #[tokio::test]
    async fn dump_replays_into_an_identical_database() {
        let src = SqliteSession::open_in_memory().unwrap();
        for sql in [
            "CREATE TABLE a (id INTEGER PRIMARY KEY, note TEXT, data BLOB, score REAL)",
            "INSERT INTO a VALUES (1, 'it''s; fine', X'00FF', 1.5), (2, NULL, NULL, NULL), (3, '', NULL, -0.25)",
            "CREATE INDEX a_note ON a (note)",
            "CREATE VIEW v AS SELECT id FROM a WHERE note IS NOT NULL",
        ] {
            src.execute(sql).await.unwrap();
        }
        let mut buf = Vec::new();
        assert_eq!(sqlite_dump(&src, &mut buf).await.unwrap(), 3);

        let dst = SqliteSession::open_in_memory().unwrap();
        let script = String::from_utf8(buf).unwrap();
        for stmt in script.split(";\n").map(str::trim).filter(|s| !s.is_empty() && !s.starts_with("--")) {
            dst.execute(stmt).await.unwrap_or_else(|e| panic!("{stmt}: {e}"));
        }
        let q = "SELECT id, note, data, score FROM a ORDER BY id";
        assert_eq!(src.execute(q).await.unwrap().rows, dst.execute(q).await.unwrap().rows);
        assert_eq!(dst.execute("SELECT count(*) FROM v").await.unwrap().rows[0][0], DbValue::Int(2));
    }
}
