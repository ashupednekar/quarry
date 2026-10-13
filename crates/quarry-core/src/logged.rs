// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Client activity log (L1): a session wrapper that records every statement the app runs.

use crate::edits::PendingChanges;
use crate::monitor::{LogPage, Monitor, ServerView, Snapshot};
use crate::schema::{SchemaCatalog, TableSchema};
use crate::session::{DatabaseSession, DbError, QueryResult, TxState};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    pub connection: String,
    pub sql: String,
    pub duration: Duration,
    pub rows: u64,
    pub error: Option<String>,
}

/// Shared, bounded log. Oldest entries drop off first.
#[derive(Clone, Default)]
pub struct ActivityLog(Arc<Mutex<(VecDeque<LogEntry>, u64)>>);

impl ActivityLog {
    pub const CAPACITY: usize = 10_000;

    pub fn push(&self, entry: LogEntry) {
        let mut guard = self.0.lock().expect("activity log poisoned");
        let (log, version) = &mut *guard;
        if log.len() == Self::CAPACITY {
            log.pop_front();
        }
        log.push_back(entry);
        *version += 1;
    }

    /// Changes on every push, so viewers can skip rebuilding when nothing happened
    pub fn version(&self) -> u64 {
        self.0.lock().expect("activity log poisoned").1
    }

    /// Newest first
    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.0
            .lock()
            .expect("activity log poisoned")
            .0
            .iter()
            .rev()
            .cloned()
            .collect()
    }
}

pub struct LoggedSession {
    inner: Arc<dyn DatabaseSession>,
    connection: String,
    log: ActivityLog,
}

impl LoggedSession {
    pub fn new(
        inner: Arc<dyn DatabaseSession>,
        connection: impl Into<String>,
        log: ActivityLog,
    ) -> Self {
        Self {
            inner,
            connection: connection.into(),
            log,
        }
    }

    fn record<T>(
        &self,
        sql: String,
        start: Instant,
        res: &Result<T, DbError>,
        rows: impl Fn(&T) -> u64,
    ) {
        self.log.push(LogEntry {
            at: Utc::now(),
            connection: self.connection.clone(),
            sql,
            duration: start.elapsed(),
            rows: res.as_ref().map(&rows).unwrap_or(0),
            error: res.as_ref().err().map(|e| e.to_string()),
        });
    }
}

#[async_trait]
impl DatabaseSession for LoggedSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        self.inner.ping().await
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        let start = Instant::now();
        let res = self.inner.introspect().await;
        self.record("-- read schema".into(), start, &res, |c| {
            c.tables.len() as u64
        });
        res
    }

    async fn load_catalog(&self) -> Result<SchemaCatalog, DbError> {
        let start = Instant::now();
        let res = self.inner.load_catalog().await;
        self.record("-- read schema".into(), start, &res, |c| {
            c.tables.len() as u64
        });
        res
    }

    async fn introspect_schema(&self, schema: &str) -> Result<Vec<TableSchema>, DbError> {
        let start = Instant::now();
        let res = self.inner.introspect_schema(schema).await;
        self.record(format!("-- read schema {schema}"), start, &res, |t| {
            t.len() as u64
        });
        res
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        let start = Instant::now();
        let res = self.inner.execute(sql).await;
        self.record(sql.to_string(), start, &res, |r| r.rows_affected);
        res
    }

    async fn execute_quiet(&self, sql: &str) -> Result<QueryResult, DbError> {
        self.inner.execute(sql).await
    }

    async fn apply_edits(&self, table: &str, changes: &PendingChanges) -> Result<usize, DbError> {
        let start = Instant::now();
        let res = self.inner.apply_edits(table, changes).await;
        self.record(
            changes.generate_sql(table, self.dialect()).join("\n"),
            start,
            &res,
            |n| *n as u64,
        );
        res
    }

    async fn table_info(
        &self,
        table: &crate::schema::TableSchema,
    ) -> Result<crate::schema::TableInfo, DbError> {
        self.inner.table_info(table).await
    }

    async fn objects(&self, schema: &str) -> Result<Vec<crate::schema::SchemaObject>, DbError> {
        self.inner.objects(schema).await
    }

    async fn object_ddl(
        &self,
        schema: &str,
        obj: &crate::schema::SchemaObject,
    ) -> Result<String, DbError> {
        self.inner.object_ddl(schema, obj).await
    }

    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        self.inner.dumper()
    }

    fn monitor(&self) -> Option<&dyn Monitor> {
        // Wrapped so cancel/terminate show up in the activity log
        self.inner.monitor().map(|_| self as &dyn Monitor)
    }

    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }

    fn tx(&self) -> &TxState {
        self.inner.tx()
    }

    fn dialect(&self) -> &'static dyn crate::Dialect {
        self.inner.dialect()
    }
}

/// Only reached through `LoggedSession::monitor`, which checks the inner session has one
#[async_trait]
impl Monitor for LoggedSession {
    fn views(&self) -> &'static [ServerView] {
        self.inner.monitor().map_or(&[], |m| m.views())
    }

    async fn snapshot(&self, view: ServerView) -> Result<Snapshot, DbError> {
        self.inner_monitor()?.snapshot(view).await
    }

    async fn logs(&self, file: Option<String>, pages: u32) -> Result<LogPage, DbError> {
        self.inner_monitor()?.logs(file, pages).await
    }

    async fn cancel(&self, id: i64) -> Result<(), DbError> {
        let start = Instant::now();
        let res = self.inner_monitor()?.cancel(id).await;
        self.record(format!("-- cancel backend {id}"), start, &res, |_| 0);
        res
    }

    async fn terminate(&self, id: i64) -> Result<(), DbError> {
        let start = Instant::now();
        let res = self.inner_monitor()?.terminate(id).await;
        self.record(format!("-- terminate backend {id}"), start, &res, |_| 0);
        res
    }
}

impl LoggedSession {
    fn inner_monitor(&self) -> Result<&dyn Monitor, DbError> {
        self.inner
            .monitor()
            .ok_or_else(|| DbError::Internal("no monitor".into()))
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::SqliteSession;

    #[tokio::test]
    async fn logs_successes_and_failures_newest_first() {
        let log = ActivityLog::default();
        let s = LoggedSession::new(
            Arc::new(SqliteSession::open_in_memory().unwrap()),
            "mem",
            log.clone(),
        );
        s.execute("SELECT 1").await.unwrap();
        assert!(s.execute("SELECT nope FROM nowhere").await.is_err());
        let entries = log.snapshot();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].error.is_some() && entries[0].sql.contains("nowhere"));
        assert_eq!(
            (entries[1].connection.as_str(), entries[1].rows),
            ("mem", 1)
        );
    }

    #[tokio::test]
    async fn manual_mode_keeps_changes_until_commit_or_rollback() {
        let s = SqliteSession::open_in_memory().unwrap();
        s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
            .await
            .unwrap();
        s.tx().set_manual(true);
        s.execute("INSERT INTO t VALUES (1, 'a')").await.unwrap();
        assert!(s.in_transaction());
        s.rollback().await.unwrap();
        // Reads don't reopen a transaction
        s.execute("SELECT 1").await.unwrap();
        assert!(!s.in_transaction());
        assert!(!s.in_transaction());
        let n = s.execute("SELECT count(*) FROM t").await.unwrap();
        assert_eq!(n.rows[(0, 0)], crate::DbValue::Int(0));

        // A failed grid submit inside the transaction only undoes itself
        s.execute("INSERT INTO t VALUES (1, 'a')").await.unwrap();
        let mut stale = PendingChanges::new();
        stale.set_cell(
            vec![("id".into(), crate::DbValue::Int(1))],
            "v",
            crate::DbValue::Text("zzz".into()),
            crate::DbValue::Null,
        );
        assert!(s.apply_edits("t", &stale).await.is_err());
        assert!(s.in_transaction());
        s.commit().await.unwrap();
        let n = s.execute("SELECT count(*) FROM t").await.unwrap();
        assert_eq!(n.rows[(0, 0)], crate::DbValue::Int(1));
    }
}
