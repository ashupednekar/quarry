// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! pg_stat_* views, server logs and backend control (L3-L5)

use super::PostgresSession;
use crate::monitor::{LogPage, Monitor, ServerView, Snapshot};
use crate::session::{DbError, QueryResult};
use crate::value::DbValue;
use async_trait::async_trait;
use std::time::Duration;

const STATEMENTS_HELP: &str =
    "pg_stat_statements isn't enabled on this server. Add it to shared_preload_libraries \
in postgresql.conf, restart Postgres, then run: CREATE EXTENSION pg_stat_statements;";
const LOGS_OFF_HELP: &str = "This server doesn't write log files (logging_collector is off). Set logging_collector = on \
in postgresql.conf and restart Postgres. Managed services (RDS, Cloud SQL, Supabase…) show logs in their own console.";
const LOGS_PERMISSION_HELP: &str =
    "Reading server logs needs a superuser, or a role with pg_monitor and pg_read_server_files: \
GRANT pg_monitor, pg_read_server_files TO your_role;";

const LOG_COLUMNS: [&str; 4] = ["time", "pid", "level", "message"];

fn view_sql(view: ServerView) -> &'static str {
    match view {
        ServerView::Activity => {
            "SELECT pid, usename AS user, datname AS database, client_addr::text AS client, state, \
             wait_event_type AS waiting_on, (now() - query_start)::text AS running_for, left(query, 500) AS query \
             FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND backend_type = 'client backend' \
             ORDER BY query_start NULLS LAST"
        }
        ServerView::Statements => {
            "SELECT calls, round(total_exec_time::numeric, 1) AS total_ms, round(mean_exec_time::numeric, 2) AS mean_ms, \
             rows, left(query, 500) AS query FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 200"
        }
        ServerView::Database => {
            "SELECT datname AS database, numbackends AS connections, xact_commit AS commits, xact_rollback AS rollbacks, \
             blks_hit, blks_read, tup_returned, tup_fetched, tup_inserted, tup_updated, tup_deleted, deadlocks \
             FROM pg_stat_database WHERE datname IS NOT NULL ORDER BY datname"
        }
        ServerView::Locks => {
            "SELECT l.pid, l.locktype, l.mode, l.granted, l.relation::regclass::text AS relation, a.state, \
             left(a.query, 300) AS query FROM pg_locks l LEFT JOIN pg_stat_activity a USING (pid) \
             WHERE l.pid <> pg_backend_pid() ORDER BY l.granted, l.pid"
        }
        ServerView::Logs => unreachable!("logs are read through Monitor::logs"),
    }
}

impl PostgresSession {
    /// First column of every row, as text
    async fn column(&self, sql: &str) -> Result<Vec<String>, DbError> {
        let r = self.run_statement(sql).await?;
        Ok(r.rows
            .iter()
            .filter_map(|row| row.first().map(|v| v.to_display_string()))
            .collect())
    }
}

#[async_trait]
impl Monitor for PostgresSession {
    fn views(&self) -> &'static [ServerView] {
        &[
            ServerView::Activity,
            ServerView::Statements,
            ServerView::Database,
            ServerView::Locks,
            ServerView::Logs,
        ]
    }

    async fn snapshot(&self, view: ServerView) -> Result<Snapshot, DbError> {
        if view == ServerView::Statements
            && self
                .column("SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'")
                .await?
                .is_empty()
        {
            return Ok(Snapshot::Unavailable(STATEMENTS_HELP.into()));
        }
        self.run_statement(view_sql(view)).await.map(Snapshot::Rows)
    }

    async fn logs(&self, file: Option<String>, pages: u32) -> Result<LogPage, DbError> {
        let unavailable = |help: &str| LogPage {
            rows: Snapshot::Unavailable(help.into()),
            files: Vec::new(),
        };
        if self
            .column("SELECT current_setting('logging_collector')")
            .await?
            .first()
            .is_some_and(|v| v != "on")
        {
            return Ok(unavailable(LOGS_OFF_HELP));
        }
        match self.read_logs(file, pages).await {
            Err(e) if e.to_string().contains("permission denied") => {
                Ok(unavailable(LOGS_PERMISSION_HELP))
            }
            other => other,
        }
    }

    async fn cancel(&self, pid: i64) -> Result<(), DbError> {
        self.client
            .execute("SELECT pg_cancel_backend($1)", &[&(pid as i32)])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;
        Ok(())
    }

    async fn terminate(&self, pid: i64) -> Result<(), DbError> {
        self.client
            .execute("SELECT pg_terminate_backend($1)", &[&(pid as i32)])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;
        Ok(())
    }
}

impl PostgresSession {
    /// The chosen (or newest) log file's last `pages` × 64 KB, newest line first, split by the server's `log_line_prefix`
    async fn read_logs(&self, file: Option<String>, pages: u32) -> Result<LogPage, DbError> {
        let files = self
            .column("SELECT name FROM pg_ls_logdir() ORDER BY modification DESC")
            .await?;
        let Some(name) = file
            .filter(|f| files.contains(f))
            .or_else(|| files.first().cloned())
        else {
            return Ok(LogPage {
                rows: Snapshot::Rows(result(Vec::new())),
                files,
            });
        };
        let quoted = name.replace('\'', "''");
        let bytes = 65536 * pages as i64;
        let sql = format!(
            "SELECT pg_read_file(current_setting('log_directory') || '/{quoted}', \
             greatest((pg_stat_file(current_setting('log_directory') || '/{quoted}')).size - {bytes}, 0), {bytes}), \
             current_setting('log_line_prefix')"
        );
        let r = self.run_statement(&sql).await?;
        let cell = |i: usize| {
            r.rows
                .first()
                .and_then(|row| row.get(i))
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        };
        let (body, prefix) = (cell(0), cell(1));
        let rows = body
            .lines()
            .rev()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (t, p, lvl, m) = super::logs::parse_line(&prefix, l);
                vec![
                    DbValue::Text(t),
                    DbValue::Text(p),
                    DbValue::Text(lvl),
                    DbValue::Text(m),
                ]
            })
            .collect();
        Ok(LogPage {
            rows: Snapshot::Rows(result(rows)),
            files,
        })
    }
}

fn result(rows: Vec<Vec<DbValue>>) -> QueryResult {
    QueryResult {
        columns: LOG_COLUMNS.iter().map(|c| c.to_string()).collect(),
        rows_affected: rows.len() as u64,
        rows: rows.into(),
        execution_time: Duration::ZERO,
        statement: String::new(),
    }
}
