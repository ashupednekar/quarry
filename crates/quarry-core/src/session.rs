// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::dialect::Dialect;
use crate::edits::PendingChanges;
use crate::schema::{SchemaCatalog, SchemaObject, TableInfo, TableSchema};
use crate::value::DbValue;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum DbError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Connection lost")]
    ConnectionLost,

    #[error("SQL error: {0}")]
    SqlError(String),

    #[error("Cannot modify read-only connection")]
    ReadOnlyViolation,

    #[error("Table has no primary key")]
    MissingPrimaryKey,

    #[error("A row was changed or removed by someone else since it was loaded. Nothing was saved; refresh and try again.")]
    ConcurrentModification,

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Most rows a result keeps in memory. The rest are counted but not kept: page with LIMIT,
/// or export, to see them all.
pub const MAX_ROWS: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DbValue>>,
    pub rows_affected: u64,
    pub execution_time: Duration,
    pub statement: String,
}

impl QueryResult {
    /// Rows were dropped at `MAX_ROWS`; `rows_affected` holds the full count
    pub fn truncated(&self) -> bool {
        !self.columns.is_empty() && (self.rows.len() as u64) < self.rows_affected
    }
}

/// Transaction state shared by the drivers (E9). `open` tracks whether a transaction is in progress.
#[derive(Default)]
pub struct TxState {
    manual: AtomicBool,
    open: AtomicBool,
}

impl TxState {
    pub fn manual(&self) -> bool {
        self.manual.load(Ordering::Relaxed)
    }
    pub fn set_manual(&self, on: bool) {
        self.manual.store(on, Ordering::Relaxed)
    }
    pub fn open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
    pub fn set_open(&self, open: bool) {
        self.open.store(open, Ordering::Relaxed)
    }
}

/// Where in the statement an error is, when the driver knows: drivers append `(at character N)`
/// (1-based) to the message, and the editor underlines from there
pub fn error_position(message: &str) -> Option<usize> {
    let (_, tail) = message.rsplit_once("(at character ")?;
    tail.strip_suffix(')')?.parse().ok()
}

#[derive(Debug, PartialEq)]
pub enum TxVerb {
    Begin,
    End,
}

/// Extracts SQL keywords and structure tokens, safely skipping comments, whitespace, and string literals.
pub fn sql_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Skip whitespace
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Single-line comment: -- or #
        if (i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-') || bytes[i] == b'#' {
            i = if bytes[i] == b'#' { i + 1 } else { i + 2 };
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Multi-line comment: /* ... */
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            let mut depth = 1;
            while i + 1 < len && depth > 0 {
                if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Single or double quote or backtick strings
        if bytes[i] == b'\'' || bytes[i] == b'"' || bytes[i] == b'`' {
            let q = bytes[i];
            i += 1;
            while i < len {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == q {
                    if i + 1 < len && bytes[i + 1] == q {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Dollar-quoted strings in Postgres ($$ or $tag$)
        if bytes[i] == b'$' {
            if let Some((tag, tag_len)) = parse_dollar_tag(&bytes[i..]) {
                i += tag_len;
                while i < len {
                    if bytes[i] == b'$' && bytes[i..].starts_with(tag.as_bytes()) {
                        i += tag.len();
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }
        // Identifier / keyword word
        if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(sql[start..i].to_ascii_uppercase());
            continue;
        }
        // Structural punctuation: (, ), ,, ;
        if matches!(bytes[i], b'(' | b')' | b',' | b';') {
            tokens.push((bytes[i] as char).to_string());
        }
        i += 1;
    }
    tokens
}

fn parse_dollar_tag(slice: &[u8]) -> Option<(String, usize)> {
    if slice.is_empty() || slice[0] != b'$' {
        return None;
    }
    if slice.len() >= 2 && slice[1] == b'$' {
        return Some(("$$".to_string(), 2));
    }
    let mut i = 1;
    while i < slice.len() {
        let b = slice[i];
        if b == b'$' {
            let tag = String::from_utf8_lossy(&slice[0..=i]).to_string();
            return Some((tag, i + 1));
        }
        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        i += 1;
    }
    None
}

/// Recognises statements that start or end a transaction, safely skipping comments.
pub fn tx_verb(sql: &str) -> Option<TxVerb> {
    let tokens = sql_tokens(sql);
    let mut iter = tokens.into_iter();
    match iter.next()?.as_str() {
        "BEGIN" | "START" => Some(TxVerb::Begin),
        // ROLLBACK TO SAVEPOINT keeps the transaction open
        "ROLLBACK" if iter.next().as_deref() == Some("TO") => None,
        "COMMIT" | "ROLLBACK" | "END" | "ABORT" => Some(TxVerb::End),
        _ => None,
    }
}

/// Plain reads don't open a transaction in manual-commit mode.
/// Correctly inspects CTEs and handles statements preceded by comments.
pub fn is_read(sql: &str) -> bool {
    let tokens = sql_tokens(sql);
    let first = match tokens.first() {
        Some(w) => w.as_str(),
        None => return false,
    };

    if matches!(
        first,
        "SELECT" | "EXPLAIN" | "SHOW" | "PRAGMA" | "VALUES" | "TABLE"
    ) {
        return true;
    }

    // CTE handling: WITH ...
    if first == "WITH" {
        // If any token in the entire query is a mutation verb, it is NOT a pure read.
        if tokens.iter().any(|t| is_mutation_verb(t.as_str())) {
            return false;
        }
        // At depth 0 after the CTE definition, check that the main statement is SELECT / VALUES / TABLE
        return has_outer_read_verb(&tokens);
    }

    false
}

/// Detects whether `sql` is a data-modifying or DDL/administrative mutation.
/// Accurately analyzes statements with leading comments, CTEs, and procedural calls.
pub fn is_mutation(sql: &str) -> bool {
    let tokens = sql_tokens(sql);
    let first = match tokens.first() {
        Some(w) => w.as_str(),
        None => return false,
    };

    if is_mutation_verb(first) {
        return true;
    }

    // CTE handling: WITH ... (can contain mutating CTEs like WITH d AS (DELETE ...) or outer INSERT)
    if first == "WITH" {
        return tokens.iter().any(|t| is_mutation_verb(t.as_str()));
    }

    false
}

fn is_mutation_verb(verb: &str) -> bool {
    matches!(
        verb,
        "INSERT"
            | "UPDATE"
            | "DELETE"
            | "CREATE"
            | "DROP"
            | "ALTER"
            | "TRUNCATE"
            | "MERGE"
            | "REPLACE"
            | "UPSERT"
            | "GRANT"
            | "REVOKE"
            | "VACUUM"
            | "CALL"
            | "EXEC"
            | "EXECUTE"
            | "DO"
            | "ATTACH"
            | "DETACH"
    )
}

fn has_outer_read_verb(tokens: &[String]) -> bool {
    let mut depth = 0;
    let mut in_cte_head = true;
    for token in tokens.iter().skip(1) {
        match token.as_str() {
            "(" => depth += 1,
            ")" => {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    in_cte_head = false;
                }
            }
            "," if depth == 0 => in_cte_head = true,
            verb if depth == 0 && !in_cte_head => {
                if matches!(verb, "SELECT" | "VALUES" | "TABLE") {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[async_trait]
pub trait DatabaseSession: Send + Sync {
    /// Tests connectivity and returns latency
    async fn ping(&self) -> Result<Duration, DbError>;

    /// Returns the live schema tree (schemas, tables, columns, PKs)
    async fn introspect(&self) -> Result<SchemaCatalog, DbError>;

    /// The catalog to start a connection with. Drivers for big databases load only the default
    /// schema here (see `SchemaCatalog::lazy`) and the rest through `introspect_schema`.
    async fn load_catalog(&self) -> Result<SchemaCatalog, DbError> {
        self.introspect().await
    }

    /// One schema's tables
    async fn introspect_schema(&self, schema: &str) -> Result<Vec<TableSchema>, DbError> {
        Ok(self
            .introspect()
            .await?
            .tables
            .into_iter()
            .filter(|t| t.schema_name.eq_ignore_ascii_case(schema))
            .collect())
    }

    /// Executes an arbitrary SQL query or statement
    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError>;

    /// Like `execute`, but not recorded in the activity log (background monitoring polls)
    async fn execute_quiet(&self, sql: &str) -> Result<QueryResult, DbError> {
        self.execute(sql).await
    }

    /// Applies pending edits atomically (E4-E6). `table` is the quoted, schema-qualified name.
    /// Fails with `ConcurrentModification` if any statement doesn't affect exactly one row.
    async fn apply_edits(&self, table: &str, changes: &PendingChanges) -> Result<usize, DbError>;

    /// DDL and indexes for a table's Structure and DDL views. The default rebuilds `CREATE TABLE`
    /// from the catalog; drivers that can do better (stored DDL, indexes, constraints) override it.
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        Ok(TableInfo {
            ddl: table.create_sql(self.dialect()),
            indexes: Vec::new(),
            ..Default::default()
        })
    }

    /// Functions, sequences, triggers and types in `schema`. Fetched when the schema is expanded.
    async fn objects(&self, _schema: &str) -> Result<Vec<SchemaObject>, DbError> {
        Ok(Vec::new())
    }

    /// The definition of one of `objects`, as SQL that would recreate it
    async fn object_ddl(&self, _schema: &str, obj: &SchemaObject) -> Result<String, DbError> {
        Err(DbError::Internal(format!(
            "no definition available for {}",
            obj.name
        )))
    }

    /// Whole-database dump, when the database supports one
    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        None
    }

    /// Server monitoring (activity, locks, logs), when the database has a server to watch
    fn monitor(&self) -> Option<&dyn crate::monitor::Monitor> {
        None
    }

    fn tx(&self) -> &TxState;

    fn in_transaction(&self) -> bool {
        self.tx().open()
    }

    async fn commit(&self) -> Result<(), DbError> {
        self.execute("COMMIT").await.map(|_| ())
    }

    async fn rollback(&self) -> Result<(), DbError> {
        self.execute("ROLLBACK").await.map(|_| ())
    }

    /// Returns whether this connection is operating in read-only mode
    fn is_read_only(&self) -> bool;

    /// The SQL this database speaks; the app builds statements through it
    fn dialect(&self) -> &'static dyn Dialect;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_transaction_statements() {
        assert_eq!(tx_verb("begin;"), Some(TxVerb::Begin));
        assert_eq!(
            tx_verb("/* comment */ START TRANSACTION"),
            Some(TxVerb::Begin)
        );
        assert_eq!(tx_verb("commit"), Some(TxVerb::End));
        assert_eq!(tx_verb("-- note\nROLLBACK TO SAVEPOINT a"), None);
        assert_eq!(tx_verb("select 1"), None);
    }

    #[test]
    fn accurately_identifies_mutations_and_reads() {
        // Leading comments do not bypass mutation detection
        assert!(is_mutation("/* harmless */ DELETE FROM users;"));
        assert!(is_mutation("-- safe\nDROP TABLE prod;"));
        assert!(is_mutation(
            "/* multi\nline */ UPDATE accounts SET balance = 0;"
        ));
        assert!(is_mutation("CALL wipe_data();"));
        assert!(is_mutation(
            "MERGE INTO target USING src ON (id) WHEN MATCHED THEN UPDATE;"
        ));

        // CTEs with mutations are recognized
        assert!(is_mutation(
            "WITH deleted AS (DELETE FROM users RETURNING *) SELECT * FROM deleted;"
        ));
        assert!(is_mutation(
            "WITH data AS (SELECT * FROM src) INSERT INTO dest SELECT * FROM data;"
        ));

        // Pure reads
        assert!(is_read("/* note */ SELECT * FROM users;"));
        assert!(is_read("-- comment\nEXPLAIN SELECT 1;"));
        assert!(is_read(
            "WITH summary AS (SELECT count(*) as c FROM users) SELECT * FROM summary;"
        ));
        assert!(!is_mutation(
            "WITH summary AS (SELECT count(*) as c FROM users) SELECT * FROM summary;"
        ));
    }
}
