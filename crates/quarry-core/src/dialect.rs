// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! The SQL a database speaks. The app builds every statement through a session's dialect, so a
//! new driver overrides only what differs from the ANSI defaults here.

use crate::plan::Plan;
use crate::schema::ColumnType;
use crate::session::QueryResult;
use crate::value::DbValue;

/// What can hide a `;` from the statement splitter, beyond quotes, `--` and `/* */` comments
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SplitRules {
    /// `# comment` (MySQL). Off elsewhere: `#` is an operator in Postgres.
    pub hash_comments: bool,
    /// `'it\'s'`: backslash escapes the next character inside strings (MySQL)
    pub backslash_escapes: bool,
    /// Batches end at a line that's just `GO`, and `;` doesn't split (SQL Server scripts)
    pub go_batches: bool,
    /// PL/SQL blocks (`BEGIN`, `DECLARE`, `CREATE PROCEDURE …`) keep their `;`s and end at a
    /// line that's just `/` (Oracle)
    pub slash_blocks: bool,
}

/// A PL/SQL block rather than one SQL statement: it keeps its semicolons, and its final `;` is part of it
pub fn is_plsql_block(sql: &str) -> bool {
    let code = sql
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("--"))
        .collect::<Vec<_>>()
        .join(" ");
    let words: Vec<String> = code
        .split_whitespace()
        .take(6)
        .map(|w| w.to_ascii_uppercase())
        .collect();
    let w = |i: usize| words.get(i).map(String::as_str).unwrap_or("");
    if matches!(w(0), "BEGIN" | "DECLARE") {
        return true;
    }
    if w(0) != "CREATE" {
        return false;
    }
    // CREATE [OR REPLACE] [EDITIONABLE | NONEDITIONABLE] <kind>
    let mut i = 1;
    if w(i) == "OR" && w(i + 1) == "REPLACE" {
        i += 2;
    }
    if matches!(w(i), "EDITIONABLE" | "NONEDITIONABLE") {
        i += 1;
    }
    matches!(
        w(i),
        "PROCEDURE" | "FUNCTION" | "PACKAGE" | "TRIGGER" | "TYPE"
    )
}

/// Engine-specific SQL dialect translation interface.
///
/// Encapsulates identifier quoting, qualification, pagination limits, parameter placeholders,
/// EXPLAIN query syntax, type name generation, and date formatting across database engines.
pub trait Dialect: Send + Sync {
    /// Short identifier shown in the UI and used in tests (e.g. `"postgres"`, `"mysql"`, `"sqlite"`).
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

    /// `sql` limited to `limit` rows after skipping `offset`. Override when the clause depends on
    /// the statement (SQL Server's OFFSET needs an ORDER BY).
    fn limit(&self, sql: &str, limit: u64, offset: u64) -> String {
        let sep = if sql.ends_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        format!("{sql}{sep}{}", self.limit_clause(limit, offset))
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

    fn split_rules(&self) -> SplitRules {
        SplitRules::default()
    }

    /// Reads the output of `explain` as a plan tree, when the format is one this dialect knows
    fn plan(&self, _result: &QueryResult) -> Option<Plan> {
        None
    }

    /// Run before a statement to resolve unqualified names in `schema` (`None` = connection default).
    /// `None` when the database has no per-session schema to switch.
    fn set_schema(&self, _schema: Option<&str>) -> Option<String> {
        None
    }

    /// The editor offers a schema picker. Defaults to whether `set_schema(None)` can reset it.
    fn switches_schema(&self) -> bool {
        self.set_schema(None).is_some()
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

    /// Whether this dialect supports the `RELEASE SAVEPOINT <name>` SQL statement.
    /// False for T-SQL (SQL Server) and Oracle.
    fn supports_release_savepoint(&self) -> bool {
        true
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
        // JSON so the plan view can read it; the grid still shows it as one cell
        Some(if analyze {
            format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}")
        } else {
            format!("EXPLAIN (FORMAT JSON) {sql}")
        })
    }

    fn plan(&self, result: &QueryResult) -> Option<Plan> {
        crate::plan::from_postgres_json(result)
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

    fn plan(&self, result: &QueryResult) -> Option<Plan> {
        crate::plan::from_sqlite_rows(result)
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

pub struct Mysql;

impl Dialect for Mysql {
    fn name(&self) -> &'static str {
        "mysql"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }

    fn literal(&self, v: &DbValue) -> String {
        // Backslash is an escape in MySQL strings (unless NO_BACKSLASH_ESCAPES), so it's doubled too
        let quoted = |s: &str| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"));
        match v {
            DbValue::Text(s) => quoted(s),
            DbValue::Json(j) => quoted(&j.to_string()),
            // DATETIME takes no zone suffix; values are UTC
            DbValue::Timestamp(ts) => format!("'{}'", ts.format("%Y-%m-%d %H:%M:%S%.6f")),
            v => v.to_sql_literal(),
        }
    }

    /// JSON so the plan view can read it. ANALYZE only comes as a text tree, shown as is.
    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        Some(if analyze {
            format!("EXPLAIN ANALYZE {sql}")
        } else {
            format!("EXPLAIN FORMAT=JSON {sql}")
        })
    }

    fn plan(&self, result: &QueryResult) -> Option<Plan> {
        crate::plan::from_mysql_json(result)
    }

    /// A database is MySQL's schema; `USE` switches it, but there's no "back to the default"
    fn set_schema(&self, schema: Option<&str>) -> Option<String> {
        schema.map(|s| format!("USE {}", self.quote_ident(s)))
    }

    fn switches_schema(&self) -> bool {
        true
    }

    fn split_rules(&self) -> SplitRules {
        SplitRules {
            hash_comments: true,
            backslash_escapes: true,
            ..SplitRules::default()
        }
    }

    /// Applies to SELECT only; MySQL has no limit for writes
    fn statement_timeout(&self, secs: u64) -> Option<String> {
        Some(format!("SET SESSION max_execution_time = {}", secs * 1000))
    }

    fn type_name(&self, t: &ColumnType) -> Option<&'static str> {
        Some(match t {
            ColumnType::Integer => "INT",
            ColumnType::BigInt => "BIGINT",
            ColumnType::Float => "DOUBLE",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Date => "DATE",
            ColumnType::Timestamp => "DATETIME(6)",
            ColumnType::Text => "TEXT",
            ColumnType::Json => "JSON",
            _ => return None,
        })
    }

    /// MySQL skips duplicates with `INSERT IGNORE`, a prefix, which also hides other errors
    fn insert_ignore_suffix(&self) -> Option<&'static str> {
        None
    }
}

pub struct Mssql;

impl Dialect for Mssql {
    fn name(&self) -> &'static str {
        "mssql"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("[{}]", ident.replace(']', "]]"))
    }

    fn literal(&self, v: &DbValue) -> String {
        // N'' keeps non-Latin text intact whatever the column's collation
        let quoted = |s: &str| format!("N'{}'", s.replace('\'', "''"));
        match v {
            DbValue::Text(s) => quoted(s),
            DbValue::Json(j) => quoted(&j.to_string()),
            DbValue::Boolean(b) => if *b { "1" } else { "0" }.into(),
            DbValue::Blob(b) => format!("0x{}", hex(b)),
            DbValue::Timestamp(ts) => format!("'{}'", ts.format("%Y-%m-%dT%H:%M:%S%.f")),
            v => v.to_sql_literal(),
        }
    }

    /// OFFSET … FETCH only follows an ORDER BY; without one, any order will do
    fn limit(&self, sql: &str, limit: u64, offset: u64) -> String {
        // ponytail: looks for ORDER BY anywhere, so one only inside a subquery is missed; the server then says so
        let ordered = sql
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|w| w == ["order", "by"]);
        let order = if ordered {
            ""
        } else {
            " ORDER BY (SELECT NULL)"
        };
        format!(
            "{}{order} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY",
            sql.trim_end()
        )
    }

    fn limit_clause(&self, limit: u64, offset: u64) -> String {
        format!("OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY")
    }

    /// Plans come from SET SHOWPLAN_XML, a session mode of its own rather than a prefix
    fn explain(&self, _sql: &str, _analyze: bool) -> Option<String> {
        None
    }

    /// No statement timeout per session; the closest is how long to wait on a lock
    fn statement_timeout(&self, secs: u64) -> Option<String> {
        Some(format!("SET LOCK_TIMEOUT {}", secs * 1000))
    }

    fn type_name(&self, t: &ColumnType) -> Option<&'static str> {
        Some(match t {
            ColumnType::Integer => "INT",
            ColumnType::BigInt => "BIGINT",
            ColumnType::Float => "FLOAT",
            ColumnType::Boolean => "BIT",
            ColumnType::Date => "DATE",
            ColumnType::Timestamp => "DATETIME2",
            ColumnType::Text | ColumnType::Json => "NVARCHAR(MAX)",
            _ => return None,
        })
    }

    fn insert_ignore_suffix(&self) -> Option<&'static str> {
        None
    }

    fn supports_release_savepoint(&self) -> bool {
        false
    }

    fn split_rules(&self) -> SplitRules {
        SplitRules {
            go_batches: true,
            ..SplitRules::default()
        }
    }
}

pub struct Oracle;

impl Dialect for Oracle {
    fn name(&self) -> &'static str {
        "oracle"
    }

    fn literal(&self, v: &DbValue) -> String {
        match v {
            // 1/0 work for NUMBER(1) flags and for 23ai's BOOLEAN alike
            DbValue::Boolean(b) => if *b { "1" } else { "0" }.into(),
            DbValue::Blob(b) => format!("HEXTORAW('{}')", hex(b)),
            DbValue::Timestamp(ts) => format!("TIMESTAMP '{}'", ts.format("%Y-%m-%d %H:%M:%S%.f")),
            v => v.to_sql_literal(),
        }
    }

    fn limit_clause(&self, limit: u64, offset: u64) -> String {
        match offset {
            0 => format!("FETCH FIRST {limit} ROWS ONLY"),
            o => format!("OFFSET {o} ROWS FETCH NEXT {limit} ROWS ONLY"),
        }
    }

    /// EXPLAIN PLAN writes to PLAN_TABLE, then its newest plan is read back as rows (estimates only)
    fn explain(&self, sql: &str, _analyze: bool) -> Option<String> {
        Some(format!(
            "EXPLAIN PLAN SET STATEMENT_ID = 'quarry' FOR {};\n{}",
            sql.trim().trim_end_matches(';'),
            "SELECT id, parent_id, operation, options, object_name, cost, cardinality FROM plan_table \
             WHERE plan_id = (SELECT MAX(plan_id) FROM plan_table WHERE statement_id = 'quarry') ORDER BY id"
        ))
    }

    fn plan(&self, result: &QueryResult) -> Option<Plan> {
        crate::plan::from_oracle_plan_table(result)
    }

    /// A schema is a user; there's no "back to my own" without knowing who that is
    fn set_schema(&self, schema: Option<&str>) -> Option<String> {
        schema.map(|s| format!("ALTER SESSION SET CURRENT_SCHEMA = {}", self.quote_ident(s)))
    }

    fn switches_schema(&self) -> bool {
        true
    }

    fn type_name(&self, t: &ColumnType) -> Option<&'static str> {
        Some(match t {
            ColumnType::Integer => "NUMBER(10)",
            ColumnType::BigInt => "NUMBER(19)",
            ColumnType::Float => "BINARY_DOUBLE",
            ColumnType::Boolean => "NUMBER(1)",
            ColumnType::Date => "DATE",
            ColumnType::Timestamp => "TIMESTAMP",
            ColumnType::Text => "VARCHAR2(4000)",
            ColumnType::Json => "CLOB",
            _ => return None,
        })
    }

    fn insert_ignore_suffix(&self) -> Option<&'static str> {
        None
    }

    fn supports_release_savepoint(&self) -> bool {
        false
    }

    fn split_rules(&self) -> SplitRules {
        SplitRules {
            slash_blocks: true,
            ..SplitRules::default()
        }
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
        assert_eq!(
            Postgres.literal(&DbValue::Blob(vec![0, 255])),
            "'\\x00FF'::bytea"
        );
        assert_eq!(Sqlite.literal(&DbValue::Blob(vec![0, 255])), "X'00FF'");
        assert_eq!(Postgres.qualified("public", "a\"b"), r#""public"."a""b""#);
        assert_eq!(Ansi.limit_clause(10, 0), "LIMIT 10");
        assert_eq!(Ansi.limit_clause(10, 20), "LIMIT 10 OFFSET 20");
        assert_eq!(
            Sqlite.explain("SELECT 1", true).unwrap(),
            "EXPLAIN QUERY PLAN SELECT 1"
        );
        assert_eq!(
            Postgres.set_schema(None).unwrap(),
            "SET search_path TO DEFAULT"
        );
        assert_eq!(Sqlite.set_schema(Some("main")), None);
        assert_eq!(Sqlite.type_name(&ColumnType::Date), None);
        assert_eq!(Mysql.qualified("shop", "a`b"), "`shop`.`a``b`");
        assert_eq!(
            Mysql.literal(&DbValue::Text(r"C:\new 'x'".into())),
            r"'C:\\new ''x'''"
        );
        assert!(Mysql.switches_schema() && Mysql.set_schema(None).is_none());
        assert_eq!(Mssql.qualified("dbo", "a]b"), "[dbo].[a]]b]");
        assert_eq!(
            Mssql.limit("SELECT * FROM t", 10, 0),
            "SELECT * FROM t ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 10 ROWS ONLY"
        );
        assert_eq!(
            Mssql.limit("SELECT * FROM t ORDER BY id", 5, 20),
            "SELECT * FROM t ORDER BY id OFFSET 20 ROWS FETCH NEXT 5 ROWS ONLY"
        );
        assert_eq!(Ansi.limit("select 1\n", 3, 0), "select 1\nLIMIT 3");
    }
}
