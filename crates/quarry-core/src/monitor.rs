// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Server monitoring (L3-L5). Optional: a driver offers it through `DatabaseSession::monitor`.

use crate::session::{DbError, QueryResult};
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerView {
    /// Running queries. The first column is the backend id `cancel`/`terminate` take.
    Activity,
    /// Statement statistics, slowest first
    Statements,
    /// Per-database counters
    Database,
    Locks,
    /// Server log file, read through `Monitor::logs`
    Logs,
}

impl ServerView {
    pub fn label(self) -> &'static str {
        match self {
            ServerView::Activity => "Active queries",
            ServerView::Statements => "Statements",
            ServerView::Database => "Database",
            ServerView::Locks => "Locks",
            ServerView::Logs => "Logs",
        }
    }
}

/// A view's rows, or what the server needs before it can show them
#[derive(Debug, Clone)]
pub enum Snapshot {
    Rows(QueryResult),
    Unavailable(String),
}

pub struct LogPage {
    /// time / pid / level / message, newest first
    pub rows: Snapshot,
    /// Log files on the server, newest first
    pub files: Vec<String>,
}

#[async_trait]
pub trait Monitor: Send + Sync {
    /// The views this database can show, in tab order
    fn views(&self) -> &'static [ServerView];

    /// One snapshot of a view other than `Logs`
    async fn snapshot(&self, view: ServerView) -> Result<Snapshot, DbError>;

    /// The last `pages` × 64 KB of `file` (or the newest log)
    async fn logs(&self, _file: Option<String>, _pages: u32) -> Result<LogPage, DbError> {
        Ok(LogPage {
            rows: Snapshot::Unavailable("This database doesn't expose its logs.".into()),
            files: Vec::new(),
        })
    }

    /// Cancels the running query of backend `id`
    async fn cancel(&self, id: i64) -> Result<(), DbError>;

    /// Closes backend `id`'s connection
    async fn terminate(&self, id: i64) -> Result<(), DbError>;
}
