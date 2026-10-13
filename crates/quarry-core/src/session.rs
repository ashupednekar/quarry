// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::dialect::Dialect;
use crate::edits::PendingChanges;
use crate::row_buffer::RowBuffer;
use crate::schema::{SchemaCatalog, SchemaObject, TableInfo, TableSchema};
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
    pub rows: RowBuffer,
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

/// Default batch size for streaming query cursors.
pub const STREAM_CHUNK_SIZE: usize = 1000;

/// A batch of rows streamed progressively from an in-flight query cursor.
#[derive(Debug, Clone)]
pub struct QueryChunk {
    /// Zero-based statement index in the multi-statement execution batch.
    pub statement_index: usize,
    /// Column names for the result set.
    pub columns: Vec<String>,
    /// Columnar rows contained in this chunk.
    pub rows: RowBuffer,
    /// Total rows affected or rows seen so far.
    pub rows_affected: u64,
    /// True if this is the final chunk for this statement.
    pub is_last: bool,
    /// Execution duration up to this chunk.
    pub execution_time: Duration,
    /// Original SQL statement text.
    pub statement: String,
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
    crate::lexer::SqlLexer::new(sql)
        .filter_map(|tok| match tok.kind {
            crate::lexer::TokenKind::Word(w) => Some(w.to_ascii_uppercase()),
            crate::lexer::TokenKind::Punctuation(c) if matches!(c, '(' | ')' | ',' | ';') => {
                Some(c.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Recognises statements that start or end a transaction, safely skipping comments and literals.
pub fn tx_verb(sql: &str) -> Option<TxVerb> {
    let mut words = crate::lexer::SqlLexer::new(sql).filter_map(|tok| match tok.kind {
        crate::lexer::TokenKind::Word(w) => Some(w),
        _ => None,
    });
    let first = words.next()?;
    if first.eq_ignore_ascii_case("BEGIN") || first.eq_ignore_ascii_case("START") {
        Some(TxVerb::Begin)
    } else if first.eq_ignore_ascii_case("ROLLBACK") {
        if words.next().is_some_and(|w| w.eq_ignore_ascii_case("TO")) {
            None
        } else {
            Some(TxVerb::End)
        }
    } else if first.eq_ignore_ascii_case("COMMIT")
        || first.eq_ignore_ascii_case("END")
        || first.eq_ignore_ascii_case("ABORT")
    {
        Some(TxVerb::End)
    } else {
        None
    }
}

/// Plain reads don't open a transaction in manual-commit mode.
/// Correctly inspects CTEs and handles statements preceded by comments.
pub fn is_read(sql: &str) -> bool {
    let mut tokens = crate::lexer::SqlLexer::new(sql)
        .filter(|t| !t.is_comment())
        .peekable();

    let first = match tokens.peek() {
        Some(tok) if tok.is_word() => tok.text,
        _ => return false,
    };

    if first.eq_ignore_ascii_case("SELECT")
        || first.eq_ignore_ascii_case("EXPLAIN")
        || first.eq_ignore_ascii_case("SHOW")
        || first.eq_ignore_ascii_case("PRAGMA")
        || first.eq_ignore_ascii_case("VALUES")
        || first.eq_ignore_ascii_case("TABLE")
    {
        return true;
    }

    // CTE handling: WITH ...
    if first.eq_ignore_ascii_case("WITH") {
        let all_tokens: Vec<crate::lexer::Token> = tokens.collect();
        if all_tokens.iter().any(|t| match t.kind {
            crate::lexer::TokenKind::Word(w) => is_mutation_verb(w),
            _ => false,
        }) {
            return false;
        }
        return has_outer_read_verb_tokens(&all_tokens);
    }

    false
}

/// Detects whether `sql` is a data-modifying or DDL/administrative mutation.
/// Accurately analyzes statements with leading comments, CTEs, and procedural calls.
pub fn is_mutation(sql: &str) -> bool {
    let mut tokens = crate::lexer::SqlLexer::new(sql)
        .filter(|t| !t.is_comment())
        .peekable();

    let first = match tokens.peek() {
        Some(tok) if tok.is_word() => tok.text,
        _ => return false,
    };

    if is_mutation_verb(first) {
        return true;
    }

    // CTE handling: WITH ... (can contain mutating CTEs like WITH d AS (DELETE ...) or outer INSERT)
    if first.eq_ignore_ascii_case("WITH") {
        return tokens.any(|t| match t.kind {
            crate::lexer::TokenKind::Word(w) => is_mutation_verb(w),
            _ => false,
        });
    }

    false
}

fn is_mutation_verb(verb: &str) -> bool {
    let upper = verb.to_ascii_uppercase();
    matches!(
        upper.as_str(),
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

fn has_outer_read_verb_tokens(tokens: &[crate::lexer::Token]) -> bool {
    let mut depth = 0;
    let mut in_cte_head = true;
    for token in tokens.iter().skip(1) {
        if token.is_punct('(') {
            depth += 1;
        } else if token.is_punct(')') {
            if depth > 0 {
                depth -= 1;
            }
            if depth == 0 {
                in_cte_head = false;
            }
        } else if token.is_punct(',') && depth == 0 {
            in_cte_head = true;
        } else if depth == 0 && !in_cte_head {
            if let crate::lexer::TokenKind::Word(w) = token.kind {
                if w.eq_ignore_ascii_case("SELECT")
                    || w.eq_ignore_ascii_case("VALUES")
                    || w.eq_ignore_ascii_case("TABLE")
                {
                    return true;
                }
            }
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

    /// Executes a statement and streams row chunks via `tx` as they arrive from the database cursor.
    /// Drivers can stream directly from their wire protocol / cursor; the default implementation
    /// runs `execute(sql)` and emits the result as chunk(s).
    async fn execute_stream(
        &self,
        sql: &str,
        chunk_size: usize,
        tx: tokio::sync::mpsc::Sender<Result<QueryChunk, DbError>>,
    ) -> Result<(), DbError> {
        let res = self.execute(sql).await?;
        if res.rows.is_empty() {
            let chunk = QueryChunk {
                statement_index: 0,
                columns: res.columns,
                rows: RowBuffer::default(),
                rows_affected: res.rows_affected,
                is_last: true,
                execution_time: res.execution_time,
                statement: res.statement,
            };
            let _ = tx.send(Ok(chunk)).await;
            return Ok(());
        }

        let total_rows = res.rows.len();
        let num_cols = res.columns.len();
        let mut offset = 0;
        while offset < total_rows {
            let end = (offset + chunk_size).min(total_rows);
            let mut chunk_buf = RowBuffer::new(num_cols);
            for r in offset..end {
                chunk_buf.push_row(res.rows.row(r).iter().cloned());
            }
            let is_last = end == total_rows;
            let chunk = QueryChunk {
                statement_index: 0,
                columns: res.columns.clone(),
                rows: chunk_buf,
                rows_affected: res.rows_affected,
                is_last,
                execution_time: res.execution_time,
                statement: res.statement.clone(),
            };
            if tx.send(Ok(chunk)).await.is_err() {
                break;
            }
            offset = end;
        }
        Ok(())
    }

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
