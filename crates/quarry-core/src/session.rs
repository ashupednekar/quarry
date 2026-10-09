// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::dialect::Dialect;
use crate::edits::PendingChanges;
use crate::schema::{SchemaCatalog, TableInfo, TableSchema};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DbValue>>,
    pub rows_affected: u64,
    pub execution_time: Duration,
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

#[derive(Debug, PartialEq)]
pub enum TxVerb {
    Begin,
    End,
}

/// Recognises statements that start or end a transaction
pub fn tx_verb(sql: &str) -> Option<TxVerb> {
    let mut words = sql.split_whitespace().map(|w| w.trim_end_matches(';').to_ascii_uppercase());
    match words.next()?.as_str() {
        "BEGIN" | "START" => Some(TxVerb::Begin),
        // ROLLBACK TO SAVEPOINT keeps the transaction open
        "ROLLBACK" if words.next().as_deref() == Some("TO") => None,
        "COMMIT" | "ROLLBACK" | "END" | "ABORT" => Some(TxVerb::End),
        _ => None,
    }
}

/// Plain reads don't open a transaction in manual-commit mode. `WITH` can write, so it counts as a write.
pub fn is_read(sql: &str) -> bool {
    let verb = sql.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(verb.as_str(), "SELECT" | "EXPLAIN" | "SHOW" | "PRAGMA" | "VALUES" | "TABLE")
}

pub fn is_mutation(sql: &str) -> bool {
    let verb = sql.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(verb.as_str(), "INSERT" | "UPDATE" | "DELETE" | "CREATE" | "DROP" | "ALTER" | "TRUNCATE")
}

#[async_trait]
pub trait DatabaseSession: Send + Sync {
    /// Tests connectivity and returns latency
    async fn ping(&self) -> Result<Duration, DbError>;

    /// Returns the live schema tree (schemas, tables, columns, PKs)
    async fn introspect(&self) -> Result<SchemaCatalog, DbError>;

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
        Ok(TableInfo { ddl: table.create_sql(self.dialect()), indexes: Vec::new() })
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
        assert_eq!(tx_verb("START TRANSACTION"), Some(TxVerb::Begin));
        assert_eq!(tx_verb("commit"), Some(TxVerb::End));
        assert_eq!(tx_verb("ROLLBACK TO SAVEPOINT a"), None);
        assert_eq!(tx_verb("select 1"), None);
    }
}
