// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Running SQL from tabs: statement collection, execution, transactions and saving grid edits

use super::*;

impl QuarryWindow {
    /// Re-runs the active tab: table tabs reload their first page, query tabs run the statement at the cursor
    pub fn refresh_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_active_tab(false, window, cx);
    }

    pub(crate) fn run_active_tab(
        &mut self,
        run_all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.state.active_tab() {
            Some(Tab::Table { filter, grid, .. }) => {
                let filter = filter.read(cx).value().to_string();
                grid.clone().update(cx, |t, cx| {
                    let rs = t.delegate_mut();
                    if let Some(src) = rs.source.as_mut() {
                        src.filter = filter;
                    }
                    // Pending edits are keyed by primary key, so they survive a reload
                    rs.fetch(Fetch::Reset, window, cx);
                });
            }
            Some(Tab::Query { .. }) => self.run_query_tab(run_all, window, cx),
            None => {}
        }
    }

    /// Runs the selection, the whole script (`run_all`), or the statement at the cursor.
    /// Statements run in order and stop at the first failure (UC3).
    pub(crate) fn run_query_tab(
        &mut self,
        run_all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_run(
            if run_all {
                RunMode::All
            } else {
                RunMode::Current
            },
            window,
            cx,
        );
    }

    /// Collects what to run (selection, statement at cursor, or everything), wraps EXPLAIN,
    /// and asks for parameter values when the SQL has `:name` / `$1` placeholders (Q6, Q7)
    pub fn start_run(&mut self, mode: RunMode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query {
            editor, is_running, ..
        }) = self.state.active_tab()
        else {
            return;
        };
        if *is_running {
            return;
        }
        let ed = editor.read(cx);
        let text = ed.value();
        let sel = ed.selected_range();
        let rules = self.state.dialect().split_rules();
        let mut statements: Vec<String> = if !sel.is_empty() {
            quarry_sql::split_statements_with(&text[sel], rules)
                .into_iter()
                .map(|s| s.sql)
                .collect()
        } else if mode == RunMode::All {
            quarry_sql::split_statements_with(&text, rules)
                .into_iter()
                .map(|s| s.sql)
                .collect()
        } else {
            quarry_sql::find_statement_at_offset_with(&text, ed.cursor(), rules)
                .into_iter()
                .map(|s| s.sql)
                .collect()
        };
        if statements.is_empty() {
            return;
        }
        if matches!(mode, RunMode::Explain | RunMode::ExplainAnalyze) {
            let sql = statements.swap_remove(0);
            if mode == RunMode::ExplainAnalyze && quarry_core::session::is_mutation(&sql) {
                self.toast("EXPLAIN ANALYZE runs the statement, which would change data. Use Explain instead.", true, cx);
                return;
            }
            let Some(explain) = self
                .state
                .dialect()
                .explain(&sql, mode == RunMode::ExplainAnalyze)
            else {
                return self.toast("This database can't show a query plan", true, cx);
            };
            // Some databases need more than one statement (Oracle: EXPLAIN PLAN, then read it back)
            statements = quarry_sql::split_statements_with(&explain, rules)
                .into_iter()
                .map(|s| s.sql)
                .collect();
        }
        let mut params: Vec<String> = Vec::new();
        for p in statements.iter().flat_map(|s| quarry_sql::find_params(s)) {
            if !params.contains(&p) {
                params.push(p);
            }
        }
        if params.is_empty() {
            self.execute_statements(statements, window, cx);
        } else {
            self.open_params(statements, params, window, cx);
        }
    }

    /// Runs statements in order on the active connection; stops at the first failure (UC3)
    pub fn execute_statements(
        &mut self,
        statements: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_prod = self
            .state
            .active_connection
            .as_ref()
            .is_some_and(|c| c.env == Environment::Prod);
        if is_prod
            && self.settings.confirm_on_prod
            && statements
                .iter()
                .any(|s| quarry_core::session::is_mutation(s))
        {
            self.pending_prod_run = Some(statements);
            cx.notify();
            return;
        }
        self.execute_statements_now(statements, window, cx);
    }

    pub(crate) fn execute_statements_now(
        &mut self,
        statements: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.state.session.clone() else {
            return self.toast("Connect to a database first", true, cx);
        };
        let dialect = session.dialect();
        let Some(Tab::Query {
            id,
            grid,
            is_running,
            error_message,
            schema,
            row_limit,
            ..
        }) = self.state.tabs.get_mut(self.state.active_tab_index)
        else {
            return;
        };
        *is_running = true;
        *error_message = None;
        let (tab_id, grid) = (id.clone(), grid.clone());
        let statements: Vec<String> = match *row_limit {
            Some(n) => statements
                .iter()
                .map(|s| quarry_sql::with_limit(s, n, dialect))
                .collect(),
            None => statements,
        };
        // The session is shared by every tab on this connection, so each run sets the tab's own schema
        let search_path = dialect.set_schema(schema.as_deref());
        cx.notify();

        let stmts = statements.clone();
        let fut = async move {
            if let Some(sp) = search_path {
                if let Err(e) = session.execute_quiet(&sp).await {
                    return (Vec::new(), Some((0, e.to_string())));
                }
            }
            let mut done = Vec::new();
            for (ix, sql) in stmts.iter().enumerate() {
                match session.execute(sql).await {
                    Ok(res) => done.push(res),
                    Err(e) => return (done, Some((ix, e.to_string()))),
                }
            }
            (done, None)
        };
        run_db(window, cx, fut, move |this, (done, failure), window, cx| {
            for res in &done {
                this.state.messages.push(format!(
                    "Ran: \"{}\" -> {} rows in {:.2}ms",
                    res.statement,
                    res.rows_affected,
                    res.execution_time.as_secs_f64() * 1000.0
                ));
                this.state.record_history(
                    res.statement.clone(),
                    res.execution_time,
                    res.rows_affected,
                    None,
                );
            }
            let failed_ix = failure.as_ref().map(|(ix, _)| *ix);
            let failure_msg = failure.map(|(ix, err)| {
                this.state.record_history(
                    statements[ix].clone(),
                    Default::default(),
                    0,
                    Some(err.clone()),
                );
                let msg = if statements.len() > 1 {
                    format!("Statement {} failed: {}", ix + 1, err)
                } else {
                    err
                };
                this.state.messages.push(format!("Error: {}", msg));
                msg
            });

            let row_sets: Vec<_> = done.into_iter().filter(|r| !r.columns.is_empty()).collect();
            let explained = statements
                .iter()
                .any(|s| s.trim_start().to_ascii_uppercase().starts_with("EXPLAIN"));
            let new_plan = explained
                .then(|| row_sets.iter().find_map(|r| dialect.plan(r)))
                .flatten()
                .map(std::rc::Rc::new);
            grid.update(cx, |t, cx| {
                t.delegate_mut().set_results(row_sets);
                t.refresh(cx);
                cx.notify();
            });
            if let Some(Tab::Query {
                is_running,
                error_message,
                editor,
                plan,
                ..
            }) = this.state.tabs.iter_mut().find(|t| t.id() == tab_id)
            {
                *is_running = false;
                *plan = new_plan;
                if let (Some(msg), Some(ix)) = (&failure_msg, failed_ix) {
                    let at = quarry_core::session::error_position(msg);
                    completion::mark_run_error(editor, &statements[ix], at, msg, cx);
                }
                *error_message = failure_msg;
            }
            // Schema changed: keep the sidebar and command bar in sync (S6)
            let is_ddl = |s: &String| {
                let verb = s
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase();
                matches!(verb.as_str(), "CREATE" | "DROP" | "ALTER")
            };
            if statements.iter().any(is_ddl) {
                this.refresh_catalog(window, cx);
            }
        });
    }

    /// Auto-commit ⇄ manual commit for the active connection (E9)
    pub(crate) fn toggle_tx_mode(
        &mut self,
        _: &ToggleTxMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.current_connection(cx).0 else {
            return;
        };
        if session.in_transaction() {
            self.state
                .messages
                .push("Commit or roll back the open transaction before switching modes.".into());
        } else {
            let manual = !session.tx().manual();
            session.tx().set_manual(manual);
            self.state.messages.push(format!(
                "{} mode.",
                if manual {
                    "Manual commit"
                } else {
                    "Auto-commit"
                }
            ));
        }
        cx.notify();
    }

    pub(crate) fn commit_tx(&mut self, _: &CommitTx, window: &mut Window, cx: &mut Context<Self>) {
        self.end_tx(true, window, cx);
    }

    pub(crate) fn rollback_tx(
        &mut self,
        _: &RollbackTx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_tx(false, window, cx);
    }

    pub(crate) fn end_tx(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.current_connection(cx).0.filter(|s| s.in_transaction()) else {
            return;
        };
        let fut = async move {
            if commit {
                session.commit().await
            } else {
                session.rollback().await
            }
        };
        run_db(window, cx, fut, move |this, res, window, cx| {
            match res {
                Ok(()) => this.toast(
                    if commit {
                        "Transaction committed"
                    } else {
                        "Transaction rolled back"
                    },
                    false,
                    cx,
                ),
                Err(e) => this.toast(e.to_string(), true, cx),
            }
            // What the grid shows may have just been undone
            if let Some(tab) = this.state.active_tab() {
                tab.grid().clone().update(cx, |t, cx| {
                    t.delegate_mut().fetch(Fetch::Reload, window, cx)
                });
            }
        });
    }

    /// Executes the pending changes against the database and refreshes the table
    pub fn commit_pending_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_prod = self
            .current_connection(cx)
            .1
            .is_some_and(|c| c.env == Environment::Prod);
        if is_prod && self.settings.confirm_on_prod && !self.state.review_modal_open {
            // Stricter on PROD: show the exact SQL first
            return self.review_changes(&ReviewChanges, window, cx);
        }
        self.state.review_modal_open = false;
        let Some(Tab::Table { grid, .. }) = self.state.active_tab() else {
            return;
        };
        let grid = grid.clone();
        // The tab's own connection, which may differ from the active one
        let Some((session, table, changes)) = grid.update(cx, |t, cx| {
            let rs = t.delegate_mut();
            rs.finish_edit(true, cx); // an open editor counts as part of the submit
            let src = rs.source.as_ref()?;
            (!rs.pending.is_empty()).then(|| {
                (
                    src.session.clone(),
                    src.qualified_name(),
                    rs.pending.clone(),
                )
            })
        }) else {
            return;
        };

        run_db(
            window,
            cx,
            async move {
                session
                    .apply_edits(&table, &changes)
                    .await
                    .map(|n| (n, table))
            },
            move |this, res, window, cx| {
                let toast = grid.update(cx, |t, cx| {
                    let rs = t.delegate_mut();
                    let toast = match res {
                        Ok((n, table)) => {
                            rs.pending.clear();
                            rs.notice = None;
                            // Re-read what's loaded without jumping to the top
                            rs.fetch(Fetch::Reload, window, cx);
                            (
                                format!(
                                    "Applied {} {} to {}",
                                    n,
                                    if n == 1 { "change" } else { "changes" },
                                    table.replace('"', "")
                                ),
                                false,
                            )
                        }
                        // Pending changes stay intact on failure (E8)
                        Err(e) => {
                            rs.notice = Some(e.to_string());
                            (format!("Submit failed: {}", e), true)
                        }
                    };
                    cx.notify();
                    toast
                });
                this.toast(toast.0, toast.1, cx);
            },
        );
    }
}
