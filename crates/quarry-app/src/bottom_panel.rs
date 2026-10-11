// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Bottom panel: Output, Query History (L2), Activity Log (L1, L6) and Server monitoring (L3-L5).

use crate::grid::{DataGrid, ResultSet};
use crate::runtime::run_db;
use crate::state::BottomPanelTab;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::table::TableState;
use gpui::component::{Disableable, Sizable};
use gpui::*;
use quarry_core::monitor::{ServerView, Snapshot};
use quarry_core::{DbError, DbValue, QueryResult};
use std::time::Duration;

const NO_MONITOR_HELP: &str =
    "This database has no server to monitor. The Activity Log shows everything Quarry ran.";

pub struct PanelState {
    history_grid: Entity<TableState<ResultSet>>,
    /// Grid row → index into `AppState::history`
    history_rows: Vec<usize>,
    history_search: Entity<InputState>,
    history_errors_only: bool,
    history_all_connections: bool,
    history_key: Option<(usize, String, bool, bool, String)>,

    log_grid: Entity<TableState<ResultSet>>,
    log_filter: Entity<InputState>,
    log_key: Option<(u64, String)>,
    log_csv: String,

    server_grid: Entity<TableState<ResultSet>>,
    server_filter: Entity<InputState>,
    server_view: ServerView,
    /// Poll interval in seconds; `None` = paused (L4)
    server_interval: Option<u64>,
    server_raw: Option<QueryResult>,
    server_message: Option<String>,
    server_loading: bool,
    /// Logs: files in the log directory (newest first), the one shown (`None` = newest), and how many 64 KB pages back
    log_files: Vec<String>,
    log_file: Option<String>,
    log_pages: u32,
    poll: Option<Task<()>>,
    /// (pid, terminate) awaiting confirmation (L5)
    pub confirm_kill: Option<(i64, bool)>,
    _subs: Vec<Subscription>,
}

impl PanelState {
    pub fn new(theme: &Theme, window: &mut Window, cx: &mut Context<QuarryWindow>) -> Self {
        let mut grid = |cx: &mut Context<QuarryWindow>| {
            cx.new(|cx| {
                TableState::new(ResultSet::new(theme, None), window, cx)
                    .cell_selectable(true)
                    .col_resizable(true)
                    .col_selectable(false)
            })
        };
        let (history_grid, log_grid, server_grid) = (grid(cx), grid(cx), grid(cx));
        let mut input = |placeholder: &'static str, cx: &mut Context<QuarryWindow>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let history_search = input("Search history", cx);
        let log_filter = input("Filter log", cx);
        let server_filter = input("Filter rows", cx);

        let subs = vec![
            cx.subscribe_in(
                &history_grid,
                window,
                |_, grid, ev: &gpui::component::table::TableEvent, _, cx| match *ev {
                    gpui::component::table::TableEvent::SelectCell(r, c) => {
                        grid.update(cx, |t, _| t.delegate_mut().selected_cell = Some((r, c)));
                    }
                    gpui::component::table::TableEvent::ClearSelection => {
                        grid.update(cx, |t, _| t.delegate_mut().selected_cell = None);
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &log_grid,
                window,
                |_, grid, ev: &gpui::component::table::TableEvent, _, cx| match *ev {
                    gpui::component::table::TableEvent::SelectCell(r, c) => {
                        grid.update(cx, |t, _| t.delegate_mut().selected_cell = Some((r, c)));
                    }
                    gpui::component::table::TableEvent::ClearSelection => {
                        grid.update(cx, |t, _| t.delegate_mut().selected_cell = None);
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &server_grid,
                window,
                |_, grid, ev: &gpui::component::table::TableEvent, _, cx| match *ev {
                    gpui::component::table::TableEvent::SelectCell(r, c) => {
                        grid.update(cx, |t, _| t.delegate_mut().selected_cell = Some((r, c)));
                    }
                    gpui::component::table::TableEvent::ClearSelection => {
                        grid.update(cx, |t, _| t.delegate_mut().selected_cell = None);
                    }
                    _ => {}
                },
            ),
            // Double-click a history entry to open it in a new query tab
            cx.subscribe_in(
                &history_grid,
                window,
                |this, _, ev: &gpui::component::table::TableEvent, window, cx| {
                    if let gpui::component::table::TableEvent::DoubleClickedCell(row, _) = *ev {
                        this.open_history_row(row, false, window, cx);
                    }
                },
            ),
            cx.subscribe_in(&history_search, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify()
                }
            }),
            cx.subscribe_in(&log_filter, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify()
                }
            }),
            cx.subscribe_in(&server_filter, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.apply_server_filter(cx);
                }
            }),
        ];

        Self {
            history_grid,
            history_rows: Vec::new(),
            history_search,
            history_errors_only: false,
            history_all_connections: true,
            history_key: None,
            log_grid,
            log_filter,
            log_key: None,
            log_csv: String::new(),
            server_grid,
            server_filter,
            server_view: ServerView::Activity,
            server_interval: Some(5),
            server_raw: None,
            server_message: None,
            server_loading: false,
            log_files: Vec::new(),
            log_file: None,
            log_pages: 1,
            poll: None,
            confirm_kill: None,
            _subs: subs,
        }
    }
}

fn result(columns: &[&str], rows: Vec<Vec<DbValue>>) -> QueryResult {
    QueryResult {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows_affected: rows.len() as u64,
        rows,
        execution_time: Duration::ZERO,
        statement: String::new(),
    }
}

fn one_line(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn local_time(t: chrono::DateTime<chrono::Utc>) -> DbValue {
    DbValue::Text(
        t.with_timezone(&chrono::Local)
            .format("%m-%d %H:%M:%S")
            .to_string(),
    )
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn set_grid(grid: &Entity<TableState<ResultSet>>, res: QueryResult, cx: &mut App) {
    grid.update(cx, |t, cx| {
        t.delegate_mut().set_results(vec![res]);
        t.refresh(cx);
        cx.notify();
    });
}

impl PanelState {
    pub fn grids(&self) -> [Entity<TableState<ResultSet>>; 3] {
        [
            self.history_grid.clone(),
            self.log_grid.clone(),
            self.server_grid.clone(),
        ]
    }
}

impl QuarryWindow {
    /// Rebuilds the history and log grids when their inputs changed
    fn sync_panel_grids(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.panel;
        let active_conn = self
            .state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let search = p.history_search.read(cx).value().to_lowercase();
        let key = (
            self.state.history.len(),
            search.clone(),
            p.history_errors_only,
            p.history_all_connections,
            active_conn.clone(),
        );
        if p.history_key.as_ref() != Some(&key) {
            let (mut rows, mut idx) = (Vec::new(), Vec::new());
            for (i, h) in self.state.history.iter().enumerate().rev() {
                if (p.history_errors_only && h.error.is_none())
                    || (!p.history_all_connections && h.connection != active_conn)
                    || (!search.is_empty() && !h.sql.to_lowercase().contains(&search))
                {
                    continue;
                }
                idx.push(i);
                rows.push(vec![
                    local_time(h.timestamp),
                    DbValue::Text(h.connection.clone()),
                    DbValue::Text(if h.error.is_some() { "error" } else { "ok" }.into()),
                    DbValue::Int(h.rows_affected as i64),
                    DbValue::Float(
                        (h.execution_time.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                    ),
                    DbValue::Text(one_line(&h.sql)),
                ]);
            }
            p.history_rows = idx;
            set_grid(
                &p.history_grid,
                result(
                    &["time", "connection", "status", "rows", "ms", "statement"],
                    rows,
                ),
                cx,
            );
            p.history_key = Some(key);
        }

        let filter = p.log_filter.read(cx).value().to_lowercase();
        let key = (self.state.activity_log.version(), filter.clone());
        if p.log_key.as_ref() != Some(&key) {
            let mut rows = Vec::new();
            let mut csv = String::from("time,connection,ms,rows,error,statement\n");
            for e in self.state.activity_log.snapshot() {
                let text = one_line(&e.sql);
                let err = e.error.clone().unwrap_or_default();
                if !filter.is_empty()
                    && !text.to_lowercase().contains(&filter)
                    && !err.to_lowercase().contains(&filter)
                {
                    continue;
                }
                let ms = (e.duration.as_secs_f64() * 1000.0 * 100.0).round() / 100.0;
                csv += &[
                    e.at.to_rfc3339(),
                    csv_field(&e.connection),
                    ms.to_string(),
                    e.rows.to_string(),
                    csv_field(&err),
                    csv_field(&e.sql),
                ]
                .join(",");
                csv.push('\n');
                rows.push(vec![
                    local_time(e.at),
                    DbValue::Text(e.connection),
                    DbValue::Float(ms),
                    DbValue::Int(e.rows as i64),
                    if e.error.is_some() {
                        DbValue::Text(err)
                    } else {
                        DbValue::Null
                    },
                    DbValue::Text(text),
                ]);
            }
            p.log_csv = csv;
            set_grid(
                &p.log_grid,
                result(
                    &["time", "connection", "ms", "rows", "error", "statement"],
                    rows,
                ),
                cx,
            );
            p.log_key = Some(key);
        }
    }

    fn open_history_row(
        &mut self,
        row: usize,
        run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sql) = self
            .panel
            .history_rows
            .get(row)
            .and_then(|&i| self.state.history.get(i))
            .map(|h| h.sql.clone())
        else {
            return;
        };
        self.open_query_tab(&sql, window, cx);
        if run {
            self.run_query_tab(true, window, cx);
        }
        cx.notify();
    }

    fn selected_history_row(&self, cx: &App) -> Option<usize> {
        self.panel
            .history_grid
            .read(cx)
            .selected_cell()
            .map(|(r, _)| r)
    }

    // ── Server monitoring ───────────────────────────────────

    /// Starts/stops polling so it only runs while the Server tab is visible and not paused
    fn manage_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible =
            self.state.bottom_panel_open && self.state.bottom_panel_tab == BottomPanelTab::Server;
        match (visible, self.panel.server_interval) {
            (true, Some(secs)) if self.panel.poll.is_none() => {
                self.panel.poll = Some(cx.spawn_in(window, async move |this, cx| loop {
                    if this
                        .update_in(cx, |this, window, cx| this.refresh_server(window, cx))
                        .is_err()
                    {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_secs(secs))
                        .await;
                }));
            }
            (true, Some(_)) => {}
            _ => self.panel.poll = None,
        }
    }

    pub fn refresh_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone().filter(|s| s.monitor().is_some()) else {
            self.panel.server_message = Some(
                if self.state.session.is_some() {
                    NO_MONITOR_HELP
                } else {
                    "Connect to a database first."
                }
                .into(),
            );
            self.panel.server_raw = None;
            set_grid(&self.panel.server_grid, result(&[], vec![]), cx);
            return;
        };
        if self.panel.server_loading {
            return;
        }
        self.panel.server_loading = true;
        let view = self.panel.server_view;
        let (log_file, log_pages) = (self.panel.log_file.clone(), self.panel.log_pages);
        // L3a: the driver says what's missing instead of failing silently
        let fut = async move {
            let monitor = session.monitor().ok_or(DbError::Cancelled)?;
            let (snapshot, files) = match view {
                ServerView::Logs => monitor
                    .logs(log_file, log_pages)
                    .await
                    .map(|p| (p.rows, p.files))?,
                _ => (monitor.snapshot(view).await?, Vec::new()),
            };
            Ok(match snapshot {
                Snapshot::Rows(r) => Ok((r, files)),
                Snapshot::Unavailable(help) => Err(help),
            })
        };
        run_db(
            window,
            cx,
            fut,
            move |this, res: Result<Result<(QueryResult, Vec<String>), String>, DbError>, _, cx| {
                let p = &mut this.panel;
                p.server_loading = false;
                if p.server_view != view {
                    return; // switched views while loading
                }
                match res {
                    Ok(Ok((r, files))) => {
                        p.server_raw = Some(r);
                        p.server_message = None;
                        if !files.is_empty() {
                            p.log_files = files;
                        }
                    }
                    Ok(Err(help)) => {
                        p.server_raw = None;
                        p.server_message = Some(help);
                    }
                    Err(e) => {
                        p.server_raw = None;
                        p.server_message = Some(e.to_string());
                    }
                }
                this.apply_server_filter(cx);
            },
        );
    }

    /// Client-side text filter over the latest server snapshot (L4)
    fn apply_server_filter(&mut self, cx: &mut Context<Self>) {
        let p = &self.panel;
        let filter = p.server_filter.read(cx).value().to_lowercase();
        let filtered = p.server_raw.clone().map(|mut r| {
            if !filter.is_empty() {
                r.rows.retain(|row| {
                    row.iter()
                        .any(|v| v.to_display_string().to_lowercase().contains(&filter))
                });
            }
            r
        });
        set_grid(
            &p.server_grid,
            filtered.unwrap_or_else(|| result(&[], vec![])),
            cx,
        );
        let level = (p.server_view == ServerView::Logs).then_some(2);
        p.server_grid
            .update(cx, |t, _| t.delegate_mut().level_column = level);
        cx.notify();
    }

    fn open_log_file_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .panel
            .log_file
            .clone()
            .or_else(|| self.panel.log_files.first().cloned());
        let choices = self
            .panel
            .log_files
            .iter()
            .map(|f| (f.clone(), current.as_ref() == Some(f), f.clone()))
            .collect();
        self.open_choice_menu(
            choices,
            |this, file, _| {
                this.panel.log_file = Some(file);
                this.panel.log_pages = 1;
                this.panel.server_raw = None;
                this.panel.poll = None; // refreshes right away with the new file
            },
            window,
            cx,
        );
    }

    fn set_server_view(&mut self, view: ServerView, window: &mut Window, cx: &mut Context<Self>) {
        self.panel.server_view = view;
        self.panel.server_raw = None;
        self.panel.server_message = None;
        self.panel.server_loading = false;
        self.refresh_server(window, cx);
    }

    fn selected_pid(&self, cx: &App) -> Option<i64> {
        let row = self.panel.server_grid.read(cx).selected_cell()?.0;
        let rs = self.panel.server_grid.read(cx).delegate();
        match rs.result()?.rows.get(row)?.first()? {
            DbValue::Int(pid) => Some(*pid),
            _ => None,
        }
    }

    /// After the confirm dialog: cancel or terminate the backend (L5)
    pub fn kill_backend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((pid, terminate)) = self.panel.confirm_kill.take() else {
            return;
        };
        let Some(session) = self.state.session.clone() else {
            return;
        };
        let fut = async move {
            let monitor = session.monitor().ok_or(DbError::Cancelled)?;
            if terminate {
                monitor.terminate(pid).await
            } else {
                monitor.cancel(pid).await
            }
        };
        run_db(window, cx, fut, move |this, res, window, cx| {
            this.state.messages.push(match res {
                Ok(()) => format!(
                    "{} backend {}.",
                    if terminate {
                        "Terminated"
                    } else {
                        "Cancelled the query on"
                    },
                    pid
                ),
                Err(e) => format!("Error: {}", e),
            });
            this.refresh_server(window, cx);
        });
    }

    fn export_log(&mut self, cx: &mut Context<Self>) {
        let csv = self.panel.log_csv.clone();
        let dir = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join("Downloads"))
            .unwrap_or_default();
        let name = format!(
            "quarry-activity-{}.csv",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let msg = match std::fs::write(&path, csv) {
                Ok(()) => format!("Exported activity log to {}", path.display()),
                Err(e) => format!("Export failed: {}", e),
            };
            this.update(cx, |this, cx| {
                this.state.messages.push(msg);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── Rendering ───────────────────────────────────────────

    pub fn render_bottom_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.manage_poll(window, cx);
        if !self.state.bottom_panel_open {
            return div().into_any_element();
        }
        self.sync_panel_grids(cx);
        let theme = &self.theme;
        let active = self.state.bottom_panel_tab;

        let tab = |label: &'static str, which: BottomPanelTab| {
            div()
                .id(label)
                .px_2()
                .py_0p5()
                .rounded(px(3.0))
                .text_xs()
                .cursor_pointer()
                .when(active == which, |d| {
                    d.bg(theme.bg_selected).text_color(theme.text_primary)
                })
                .when(active != which, |d| {
                    d.text_color(theme.text_muted)
                        .hover(|s| s.bg(theme.bg_hover))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.bottom_panel_tab = which;
                    if which == BottomPanelTab::Server {
                        this.refresh_server(window, cx);
                    }
                    cx.notify();
                }))
                .child(label)
        };
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            zbutton(id)
                .pad_xs()
                .label(label)
                .xsmall()
                .map(|b| if on { b.primary() } else { b.ghost() })
        };
        let grid_view = |grid: &Entity<TableState<ResultSet>>, cx: &Context<Self>| {
            DataGrid::render(
                grid,
                theme,
                cx,
                |_, _, _| {},
                |_, _, _| {},
                |_, _, _| {},
                |_, _, _| {},
                |_, _, _, _| {},
            )
        };

        let body: AnyElement = match active {
            BottomPanelTab::Messages => v_flex()
                .id("output-scroll")
                .flex_1()
                .overflow_y_scroll()
                .p_2()
                .gap_1()
                .when(self.state.messages.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("Nothing yet."),
                    )
                })
                .children(self.state.messages.iter().rev().take(500).map(|m| {
                    div()
                        .text_xs()
                        .text_color(if m.starts_with("Error") {
                            theme.danger
                        } else {
                            theme.text_secondary
                        })
                        .child(m.clone())
                }))
                .into_any_element(),
            BottomPanelTab::History => {
                let p = &self.panel;
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .h(px(BAR_H))
                            .px_3()
                            .gap_1()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .w(px(240.0))
                                    .child(Input::new(&p.history_search).xsmall().cleanable(true)),
                            )
                            .child(
                                toggle("hist-errors", "Errors only", p.history_errors_only)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.panel.history_errors_only =
                                            !this.panel.history_errors_only;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                toggle("hist-conn", "This connection", !p.history_all_connections)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.panel.history_all_connections =
                                            !this.panel.history_all_connections;
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child("Double-click to open"),
                            )
                            .child(
                                zbutton("hist-open")
                                    .pad_xs()
                                    .label("Open")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(row) = this.selected_history_row(cx) {
                                            this.open_history_row(row, false, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                zbutton("hist-run")
                                    .pad_xs()
                                    .icon(IconName::Play)
                                    .label("Run")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(row) = this.selected_history_row(cx) {
                                            this.open_history_row(row, true, window, cx);
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(grid_view(&p.history_grid, cx)),
                    )
                    .into_any_element()
            }
            BottomPanelTab::Logs => {
                let p = &self.panel;
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .h(px(BAR_H))
                            .px_3()
                            .gap_1()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .w(px(240.0))
                                    .child(Input::new(&p.log_filter).xsmall().cleanable(true)),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child("Everything Quarry ran, newest first"),
                            )
                            .child(
                                zbutton("log-copy")
                                    .pad_xs()
                                    .icon(IconName::Copy)
                                    .label("Copy")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            this.panel.log_csv.clone(),
                                        ));
                                        this.state.messages.push(
                                            "Copied the activity log (CSV) to the clipboard."
                                                .into(),
                                        );
                                    })),
                            )
                            .child(
                                zbutton("log-export")
                                    .pad_xs()
                                    .icon(IconName::Download)
                                    .label("Export CSV")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, _, cx| this.export_log(cx))),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(grid_view(&p.log_grid, cx)))
                    .into_any_element()
            }
            BottomPanelTab::Server => {
                let p = &self.panel;
                let pid = self
                    .selected_pid(cx)
                    .filter(|_| p.server_view == ServerView::Activity);
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .h(px(BAR_H))
                            .px_3()
                            .gap_1()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .children(
                                self.state
                                    .session
                                    .as_ref()
                                    .and_then(|s| s.monitor())
                                    .map_or(&[][..], |m| m.views())
                                    .iter()
                                    .copied()
                                    .map(|view| {
                                        toggle(view.label(), view.label(), p.server_view == view)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.set_server_view(view, window, cx)
                                            }))
                                    }),
                            )
                            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
                            .child(
                                div()
                                    .w(px(180.0))
                                    .child(Input::new(&p.server_filter).xsmall().cleanable(true)),
                            )
                            .child(div().flex_1())
                            .children(
                                [
                                    (2u64, "poll-2", "2s"),
                                    (5, "poll-5", "5s"),
                                    (10, "poll-10", "10s"),
                                ]
                                .map(|(secs, id, label)| {
                                    toggle(id, label, p.server_interval == Some(secs)).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.panel.server_interval = Some(secs);
                                            this.panel.poll = None; // restarts with the new interval
                                            cx.notify();
                                        }),
                                    )
                                }),
                            )
                            .child(
                                zbutton("poll-pause")
                                    .icon(if p.server_interval.is_some() {
                                        IconName::Pause
                                    } else {
                                        IconName::Play
                                    })
                                    .ghost()
                                    .xsmall()
                                    .tooltip(if p.server_interval.is_some() {
                                        "Pause refresh"
                                    } else {
                                        "Resume refresh"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.panel.server_interval =
                                            if this.panel.server_interval.is_some() {
                                                None
                                            } else {
                                                Some(5)
                                            };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                zbutton("server-refresh")
                                    .icon(IconName::RefreshCw)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Refresh now")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.refresh_server(window, cx)
                                    })),
                            )
                            .when(p.server_view == ServerView::Logs, |this| {
                                let file = p
                                    .log_file
                                    .clone()
                                    .or_else(|| p.log_files.first().cloned())
                                    .unwrap_or_else(|| "Newest log".into());
                                this.child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
                                    .child(
                                        zbutton("log-file")
                                            .pad_xs()
                                            .icon(IconName::FileCode)
                                            .label(file)
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Pick a log file")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_log_file_menu(window, cx)
                                            })),
                                    )
                                    .child(
                                        zbutton("log-older")
                                            .pad_xs()
                                            .label(format!("Load older ({} KB)", p.log_pages * 64))
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Read another 64 KB further back")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.panel.log_pages += 1;
                                                this.refresh_server(window, cx);
                                            })),
                                    )
                            })
                            .when(p.server_view == ServerView::Activity, |this| {
                                this.child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
                                    .child(
                                        zbutton("kill-cancel")
                                            .pad_xs()
                                            .label("Cancel query")
                                            .ghost()
                                            .xsmall()
                                            .disabled(pid.is_none())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.panel.confirm_kill = pid.map(|p| (p, false));
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        zbutton("kill-terminate")
                                            .pad_xs()
                                            .label("Terminate")
                                            .ghost()
                                            .xsmall()
                                            .disabled(pid.is_none())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.panel.confirm_kill = pid.map(|p| (p, true));
                                                cx.notify();
                                            })),
                                    )
                            }),
                    )
                    .when_some(p.server_message.clone(), |this, msg| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(msg),
                        )
                    })
                    .when(p.server_message.is_none(), |this| {
                        this.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .child(grid_view(&p.server_grid, cx)),
                        )
                    })
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .bg(theme.bg_panel)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(BAR_H))
                    .px_3()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(tab("Output", BottomPanelTab::Messages))
                    .child(tab("History", BottomPanelTab::History))
                    .child(tab("Activity Log", BottomPanelTab::Logs))
                    .child(tab("Server", BottomPanelTab::Server))
                    .child(div().flex_1())
                    .child(
                        zbutton("close-bottom-panel")
                            .icon(IconName::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Close (⌘J)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.bottom_panel_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    /// Confirmation for cancel/terminate (destructive, so a real dialog)
    pub fn render_kill_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (pid, terminate) = self.panel.confirm_kill?;
        let theme = &self.theme;
        let (title, detail, action) = if terminate {
            (
                "Terminate backend?",
                format!("Backend {pid} will be disconnected and its transaction rolled back."),
                "Terminate",
            )
        } else {
            ("Cancel query?", format!("The query running on backend {pid} will be stopped. The session stays connected."), "Cancel query")
        };
        Some(
            modal_backdrop("kill-backdrop", dismiss_on_click)
                .child(
                    modal_card("kill-card", 380.0, theme)
                        .p_4()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child(detail),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    zbutton("kill-no")
                                        .pad_sm()
                                        .label("Keep it")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.panel.confirm_kill = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    zbutton("kill-yes")
                                        .pad_sm()
                                        .label(action)
                                        .danger()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.kill_backend(window, cx)
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
