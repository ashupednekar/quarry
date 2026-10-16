// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Process list, statement statistics and KILL (L3-L5)

use super::MysqlSession;
use crate::monitor::{Monitor, ServerView, Snapshot};
use crate::session::DbError;
use async_trait::async_trait;

const STATEMENTS_HELP: &str =
    "Statement statistics come from performance_schema, which is off on this server. \
Set performance_schema = ON in my.cnf and restart MySQL.";

#[async_trait]
impl Monitor for MysqlSession {
    fn views(&self) -> &'static [ServerView] {
        &[ServerView::Activity, ServerView::Statements]
    }

    async fn snapshot(&self, view: ServerView) -> Result<Snapshot, DbError> {
        let sql = match view {
            // Connection id first: cancel/terminate take it
            ServerView::Activity => {
                "SELECT ID, USER, HOST AS client, DB AS `database`, COMMAND, STATE, TIME AS seconds, LEFT(INFO, 500) AS query \
                 FROM information_schema.PROCESSLIST WHERE ID <> CONNECTION_ID() AND COMMAND <> 'Daemon' ORDER BY TIME DESC"
            }
            ServerView::Statements => {
                if self.column("SELECT @@performance_schema").await?.first().is_some_and(|v| v == "0") {
                    return Ok(Snapshot::Unavailable(STATEMENTS_HELP.into()));
                }
                "SELECT COUNT_STAR AS calls, ROUND(SUM_TIMER_WAIT / 1e9, 1) AS total_ms, ROUND(AVG_TIMER_WAIT / 1e9, 2) AS mean_ms, \
                 SUM_ROWS_SENT AS `rows`, LEFT(DIGEST_TEXT, 500) AS query \
                 FROM performance_schema.events_statements_summary_by_digest ORDER BY SUM_TIMER_WAIT DESC LIMIT 200"
            }
            _ => return Ok(Snapshot::Unavailable("Not available for MySQL".into())),
        };
        self.run(sql).await.map(Snapshot::Rows)
    }

    async fn cancel(&self, id: i64) -> Result<(), DbError> {
        self.run(&format!("KILL QUERY {id}")).await.map(|_| ())
    }

    async fn terminate(&self, id: i64) -> Result<(), DbError> {
        self.run(&format!("KILL {id}")).await.map(|_| ())
    }
}
