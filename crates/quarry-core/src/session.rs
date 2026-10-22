// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Database session traits, query result containers, streaming cursors, and error handling.
//!
//! Defines [`DatabaseSession`], the primary async trait implemented by database drivers, along
//! with [`QueryResult`], [`QueryChunk`] for streaming queries, [`TxState`] for transaction tracking,
//! and [`DbError`] for classified operational errors.

use crate::dialect::Dialect;
use crate::edits::PendingChanges;
use crate::row_buffer::RowBuffer;
use crate::schema::{SchemaCatalog, SchemaObject, TableInfo, TableSchema};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use thiserror::Error;

/// Leads a driver's raw connect error with what it means and what to check.
/// Text matching because each driver words these differently; unknown errors pass through as is.
pub fn explain_connect_error(raw: &str) -> String {
    let m = raw.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    let hint = if has(&[
        "password authentication failed",
        "access denied for user",
        "login failed for user",
        "ora-01017",
        "invalid username/password",
    ]) {
        "Wrong user name or password."
    } else if has(&["role \"", "ora-01918"]) && m.contains("does not exist") {
        "That user doesn't exist on this server."
    } else if has(&["unknown database", "cannot open database", "ora-12514"])
        || (m.contains("database \"") && m.contains("does not exist"))
    {
        "That database (or Oracle service) doesn't exist on this server. Check its name."
    } else if has(&["no pg_hba.conf entry"]) {
        "The server doesn't allow connections from this machine or user (pg_hba.conf)."
    } else if has(&[
        "connection refused",
        "ora-12541",
        "os error 61",
        "os error 111",
        "actively refused",
    ]) {
        "Nothing is accepting connections at that host and port. Is the server running, and is the port right?"
    } else if has(&[
        "failed to lookup address",
        "nodename nor servname",
        "name or service not known",
        "no such host",
        "ora-12545",
    ]) {
        "Couldn't find that host. Check the host name."
    } else if has(&["timed out", "timeout", "ora-12170"]) {
        "The server didn't answer in time. Check the host, port, and any firewall or VPN."
    } else if has(&["tls", "ssl", "certificate"]) {
        "The server and Quarry couldn't agree on encryption (SSL/TLS)."
    } else if has(&["too many connections", "ora-12516", "ora-12519"]) {
        "The server has no free connections right now. Try again shortly."
    } else {
        return raw.to_string();
    };
    format!("{hint}\n{raw}")
}

#[derive(Error, Debug, Clone)]
pub enum DbError {
    #[error("{}", explain_connect_error(.0))]
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

/// Complete in-memory representation of an executed query result set.
///
/// Holds the list of column names, the columnar [`RowBuffer`] storing up to [`MAX_ROWS`] rows,
/// affected row counts (or total server rows matched), query execution latency, and the
/// executed SQL statement string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Ordered list of column names returned by the database.
    pub columns: Vec<String>,
    /// Columnar storage holding up to [`MAX_ROWS`] returned rows.
    pub rows: RowBuffer,
    /// Total count of rows affected by a DML statement, or total row count scanned/returned.
    pub rows_affected: u64,
    /// Total wall-clock time spent executing the query against the database engine.
    pub execution_time: Duration,
    /// Original SQL statement string that generated this result.
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

/// Asynchronous database session interface implemented by database drivers.
///
/// Encapsulates connection lifecycle, live schema introspection, SQL execution (eager and streaming),
/// transactional change commits, metadata inspection, and optional server monitor/dump extensions.
#[async_trait]
pub trait DatabaseSession: Send + Sync {
    /// Tests connectivity to the database and measures round-trip latency.
    ///
    /// # Returns
    ///
    /// The round-trip wall-clock [`Duration`] elapsed during the ping probe.
    ///
    /// # Errors
    ///
    /// * [`DbError::ConnectionLost`] if the server is unreachable or socket dropped.
    /// * [`DbError::QueryError`] if the underlying heartbeat probe fails.
    async fn ping(&self) -> Result<Duration, DbError>;

    /// Introspects the live database schema catalog.
    ///
    /// Retrieves all visible schemas, tables, views, columns, primary keys, and foreign keys.
    ///
    /// # Returns
    ///
    /// A fully populated [`SchemaCatalog`] representing the database structure.
    ///
    /// # Errors
    ///
    /// * [`DbError::QueryError`] if schema catalog query execution fails.
    /// * [`DbError::ConnectionLost`] if disconnected during introspection.
    async fn introspect(&self) -> Result<SchemaCatalog, DbError>;

    /// Loads the initial catalog for a newly opened connection.
    ///
    /// Drivers for large databases may load only the default schema initially (see `SchemaCatalog::lazy`)
    /// and fetch the remainder on-demand through [`introspect_schema`](Self::introspect_schema).
    ///
    /// # Returns
    ///
    /// Initial [`SchemaCatalog`] ready for immediate autocompletion and sidebar presentation.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if catalog initialization fails.
    async fn load_catalog(&self) -> Result<SchemaCatalog, DbError> {
        self.introspect().await
    }

    /// Introspects table structures within a specific schema.
    ///
    /// # Arguments
    ///
    /// * `schema` - Unquoted or quoted schema name to inspect.
    ///
    /// # Returns
    ///
    /// A vector of [`TableSchema`] items belonging to the requested schema.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if schema query fails.
    async fn introspect_schema(&self, schema: &str) -> Result<Vec<TableSchema>, DbError> {
        Ok(self
            .introspect()
            .await?
            .tables
            .into_iter()
            .filter(|t| t.schema_name.eq_ignore_ascii_case(schema))
            .collect())
    }

    /// Executes an arbitrary SQL query or command eagerly.
    ///
    /// Buffers up to [`MAX_ROWS`] rows in a columnar [`RowBuffer`] and computes execution duration.
    ///
    /// # Arguments
    ///
    /// * `sql` - SQL statement string to execute against the session.
    ///
    /// # Returns
    ///
    /// A complete [`QueryResult`] containing columns, rows, rows affected, and duration.
    ///
    /// # Errors
    ///
    /// * [`DbError::QueryError`] if the database returns an execution error.
    /// * [`DbError::ConnectionLost`] if the connection terminates mid-execution.
    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError>;

    /// Executes a statement and streams row chunks via an mpsc channel as they arrive from the cursor.
    ///
    /// Drivers can stream directly from their wire protocol cursor; the default implementation
    /// runs `execute(sql)` eagerly and emits the result as chunk(s).
    ///
    /// # Arguments
    ///
    /// * `sql` - Statement string to execute.
    /// * `chunk_size` - Maximum number of rows to package into each [`QueryChunk`].
    /// * `tx` - Asynchronous channel sender receiving yielded chunks or stream errors.
    ///
    /// # Returns
    ///
    /// `Ok(())` when execution and streaming finish successfully.
    ///
    /// # Errors
    ///
    /// * [`DbError::QueryError`] if execution fails.
    /// * [`DbError::ConnectionLost`] if the network cursor drops during streaming.
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

    /// Executes a SQL statement quietly without appending it to the user activity history.
    ///
    /// Suitable for internal background polling (e.g. server monitor heartbeats or transaction status queries).
    ///
    /// # Arguments
    ///
    /// * `sql` - SQL statement string to execute quietly.
    ///
    /// # Returns
    ///
    /// A [`QueryResult`] containing the query outcome.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if execution fails.
    async fn execute_quiet(&self, sql: &str) -> Result<QueryResult, DbError> {
        self.execute(sql).await
    }

    /// Applies in-memory pending grid edits atomically using optimistic concurrency checks.
    ///
    /// Emits parameterized UPDATE, INSERT, and DELETE statements wrapped in a transaction.
    ///
    /// # Arguments
    ///
    /// * `table` - Quoted, schema-qualified target table name.
    /// * `changes` - Staged cell edits, inserted rows, and deleted rows.
    ///
    /// # Returns
    ///
    /// The total count of modified rows upon successful commit.
    ///
    /// # Errors
    ///
    /// * [`DbError::MissingPrimaryKey`] if target table lacks a unique primary key identifier.
    /// * [`DbError::ConcurrentModification`] if a row was updated or removed by an external actor.
    /// * [`DbError::QueryError`] if a database constraint check fails.
    async fn apply_edits(&self, table: &str, changes: &PendingChanges) -> Result<usize, DbError>;

    /// Fetches DDL and index metadata for structure and schema inspect views.
    ///
    /// # Arguments
    ///
    /// * `table` - Target table schema reference.
    ///
    /// # Returns
    ///
    /// A [`TableInfo`] struct populated with DDL strings, index descriptors, and constraint rules.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if database metadata queries fail.
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        Ok(TableInfo {
            ddl: table.create_sql(self.dialect()),
            indexes: Vec::new(),
            ..Default::default()
        })
    }

    /// Fetches auxiliary schema objects (functions, sequences, triggers, types) in a schema.
    ///
    /// # Arguments
    ///
    /// * `schema` - Target schema name.
    ///
    /// # Returns
    ///
    /// A vector of [`SchemaObject`] descriptors.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if object catalog query fails.
    async fn objects(&self, _schema: &str) -> Result<Vec<SchemaObject>, DbError> {
        Ok(Vec::new())
    }

    /// Fetches SQL DDL script to recreate a specific schema object.
    ///
    /// # Arguments
    ///
    /// * `schema` - Parent schema name.
    /// * `obj` - Target schema object reference.
    ///
    /// # Returns
    ///
    /// SQL source DDL defining the object.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if no definition is available or query fails.
    async fn object_ddl(&self, _schema: &str, obj: &SchemaObject) -> Result<String, DbError> {
        Err(DbError::Internal(format!(
            "no definition available for {}",
            obj.name
        )))
    }

    /// Returns an optional database dumper interface for whole-database exports.
    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        None
    }

    /// Returns an optional server monitor interface (active connections, locks, storage metrics).
    fn monitor(&self) -> Option<&dyn crate::monitor::Monitor> {
        None
    }

    /// Returns a reference to the session's transaction state tracker.
    fn tx(&self) -> &TxState;

    /// Returns true if an uncommitted transaction is currently open on this session.
    fn in_transaction(&self) -> bool {
        self.tx().open()
    }

    /// Commits the active transaction.
    ///
    /// # Returns
    ///
    /// `Ok(())` upon successful commit.
    ///
    /// # Errors
    ///
    /// * [`DbError`] if commit fails.
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

#[cfg(test)]
mod explain_tests {
    use super::explain_connect_error as x;

    #[test]
    fn explains_common_connect_failures() {
        assert!(x("password authentication failed for user \"bob\"").starts_with("Wrong user name"));
        assert!(x("Access denied for user 'root'@'localhost'").starts_with("Wrong user name"));
        assert!(x("role \"bob\" does not exist").starts_with("That user doesn't"));
        assert!(x("database \"nope\" does not exist").starts_with("That database"));
        assert!(x("ORA-12541: Cannot connect. No listener").starts_with("Nothing is accepting"));
        assert!(
            x("error connecting to server: Connection refused (os error 61)")
                .starts_with("Nothing is accepting")
        );
        // Unknown errors pass through untouched
        assert_eq!(x("something odd"), "something odd");
    }
}
