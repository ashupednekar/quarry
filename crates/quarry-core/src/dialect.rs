// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! The SQL a database speaks. The app builds every statement through a session's dialect, so a
//! new driver overrides only what differs from the ANSI defaults here.

use crate::schema::ColumnType;
use crate::value::DbValue;

pub trait Dialect: Send + Sync {
    /// Short name shown in the UI and used in tests, e.g. "postgres"
    fn name(&self) -> &'static str;

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    /// `"schema"."table"`
    fn qualified(&self, schema: &str, table: &str) -> String {
        format!("{}.{}", self.quote_ident(schema), self.quote_ident(table))
    }

    fn literal(&self, v: &DbValue) -> String {
        v.to_sql_literal()
    }

    /// Appended after a SELECT to page it. `offset` 0 means no OFFSET.
    fn limit_clause(&self, limit: u64, offset: u64) -> String {
        match offset {
            0 => format!("LIMIT {limit}"),
            o => format!("LIMIT {limit} OFFSET {o}"),
        }
    }

    /// The statement that shows `sql`'s plan. `analyze` runs it for real timings where supported.
    fn explain(&self, sql: &str, _analyze: bool) -> Option<String> {
        Some(format!("EXPLAIN {sql}"))
    }

    /// Run before a statement to resolve unqualified names in `schema` (`None` = connection default).
    /// `None` when the database has no per-session schema to switch.
    fn set_schema(&self, _schema: Option<&str>) -> Option<String> {
        None
    }

    /// Session setting that cancels statements running longer than `secs`, if there is one
    fn statement_timeout(&self, _secs: u64) -> Option<String> {
        None
    }

    /// `UPDATE … SET col = DEFAULT` works; otherwise edits write the column's default expression
    fn supports_default_keyword(&self) -> bool {
        true
    }

    /// Column type for `CREATE TABLE` (imports). `None` means "store it as text".
    fn type_name(&self, t: &ColumnType) -> Option<&'static str> {
        Some(match t {
            ColumnType::Integer => "INTEGER",
            ColumnType::BigInt => "BIGINT",
            ColumnType::Float => "DOUBLE PRECISION",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Date => "DATE",
            ColumnType::Timestamp => "TIMESTAMP",
            ColumnType::Text => "TEXT",
            _ => return None,
        })
    }

    /// `INSERT … <this>` skips rows that hit a unique constraint
    fn insert_ignore_suffix(&self) -> Option<&'static str> {
        Some(" ON CONFLICT DO NOTHING")
    }
}

pub struct Postgres;

impl Dialect for Postgres {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn literal(&self, v: &DbValue) -> String {
        match v {
            // `X'..'` is a bit string in Postgres; bytea takes its hex escape form
            DbValue::Blob(b) => format!("'\\x{}'::bytea", hex(b)),
            v => v.to_sql_literal(),
        }
    }

    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        Some(if analyze { format!("EXPLAIN (ANALYZE, BUFFERS) {sql}") } else { format!("EXPLAIN {sql}") })
    }

    fn set_schema(&self, schema: Option<&str>) -> Option<String> {
        Some(match schema {
            Some(s) => format!("SET search_path TO {}", self.quote_ident(s)),
            None => "SET search_path TO DEFAULT".into(),
        })
    }

    fn statement_timeout(&self, secs: u64) -> Option<String> {
        Some(format!("SET statement_timeout = {}", secs * 1000))
    }
}

pub struct Sqlite;

impl Dialect for Sqlite {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    // SQLite has no ANALYZE variant; the query plan is the useful view
    fn explain(&self, sql: &str, _analyze: bool) -> Option<String> {
        Some(format!("EXPLAIN QUERY PLAN {sql}"))
    }

    // SQLite can't time out a single statement; the closest is how long to wait on a lock
    fn statement_timeout(&self, secs: u64) -> Option<String> {
        Some(format!("PRAGMA busy_timeout = {}", secs * 1000))
    }

    fn supports_default_keyword(&self) -> bool {
        false
    }

    fn type_name(&self, t: &ColumnType) -> Option<&'static str> {
        Some(match t {
            ColumnType::Integer | ColumnType::BigInt => "INTEGER",
            ColumnType::Float => "REAL",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Text => "TEXT",
            // Dates are stored as text, so a typed column adds nothing
            _ => return None,
        })
    }
}

/// Plain ANSI SQL, for code that builds SQL with no session at hand
pub struct Ansi;

impl Dialect for Ansi {
    fn name(&self) -> &'static str {
        "ansi"
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialects_differ_where_databases_do() {
        assert_eq!(Postgres.literal(&DbValue::Blob(vec![0, 255])), "'\\x00FF'::bytea");
        assert_eq!(Sqlite.literal(&DbValue::Blob(vec![0, 255])), "X'00FF'");
        assert_eq!(Postgres.qualified("public", "a\"b"), r#""public"."a""b""#);
        assert_eq!(Ansi.limit_clause(10, 0), "LIMIT 10");
        assert_eq!(Ansi.limit_clause(10, 20), "LIMIT 10 OFFSET 20");
        assert_eq!(Sqlite.explain("SELECT 1", true).unwrap(), "EXPLAIN QUERY PLAN SELECT 1");
        assert_eq!(Postgres.set_schema(None).unwrap(), "SET search_path TO DEFAULT");
        assert_eq!(Sqlite.set_schema(Some("main")), None);
        assert_eq!(Sqlite.type_name(&ColumnType::Date), None);
    }
}
