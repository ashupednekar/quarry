// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

mod add_conn_modal;
mod bottom_panel;
mod command_bar;
mod connections;
mod editor;
mod grid;
mod history;
mod review_modal;
mod runtime;
mod sidebar;
mod state;
mod status_bar;
mod theme;
mod ui;

use add_conn_modal::AddConnModal;
use command_bar::CommandBar;
use editor::QueryEditor;
use gpui::component::input::{EditorState, Input, InputEvent, InputState};
use gpui::assets::IconName;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui::component::{Icon, Sizable};
use gpui::*;
use gpui::component::table::{TableEvent, TableState};
use grid::{DataGrid, Fetch, ResultSet, TableSource};
use review_modal::ReviewModal;
use runtime::{run_db, DB_RUNTIME};
use sidebar::Sidebar;
use quarry_core::DatabaseSession;
use std::sync::Arc;
use state::{open_session, AppState, ConnectionProfile, ConnectionTargetKind, Environment, Tab};
use status_bar::StatusBar;
use theme::Theme;
use ui::*;

actions!(
    quarry,
    [
        ToggleCommandBar,
        RunQuery,
        RunAll,
        ReviewChanges,
        ToggleSidebar,
        ToggleBottomPanel,
        NewQuery,
        CloseTab,
        Dismiss,
        ToggleTxMode,
        CommitTx,
        RollbackTx,
        ToggleTheme,
        MenuUp,
        MenuDown,
    ]
);

/// Default keymap. Must run after `gpui::init` so these win over the input's own bindings.
fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-k", ToggleCommandBar, None),
        KeyBinding::new("secondary-enter", RunQuery, None),
        // Inputs map secondary-enter to "newline"; run the query instead.
        KeyBinding::new("secondary-enter", RunQuery, Some("Input")),
        KeyBinding::new("secondary-shift-enter", RunAll, None),
        KeyBinding::new("secondary-shift-enter", RunAll, Some("Input")),
        KeyBinding::new("secondary-s", ReviewChanges, None),
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-j", ToggleBottomPanel, None),
        KeyBinding::new("secondary-n", NewQuery, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("escape", Dismiss, None),
        KeyBinding::new("up", MenuUp, Some("CommandBar > Input")),
        KeyBinding::new("down", MenuDown, Some("CommandBar > Input")),
    ]);
}

const NEW_QUERY_SQL: &str = "SELECT * FROM users LIMIT 50;";

struct QuarryWindow {
    state: AppState,
    theme: Theme,
    focus_handle: FocusHandle,
    command_input: Entity<InputState>,
    conn_name_input: Entity<InputState>,
    conn_path_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    onboard_input: Entity<InputState>,
    /// `Some(dark)` once the user picks a theme; otherwise follow the OS
    theme_override: Option<bool>,
    toast_task: Option<Task<()>>,
    /// Per-tab subscriptions (grid events), dropped with the tab
    tab_subscriptions: std::collections::HashMap<String, Subscription>,
    panel: bottom_panel::PanelState,
}

impl QuarryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::new();

        let command_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search tables, actions, or type SQL…"));
        let conn_name_input = cx.new(|cx| InputState::new(window, cx).default_value("Local SQLite"));
        let conn_path_input = cx.new(|cx| InputState::new(window, cx).default_value(":memory:"));
        let onboard_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste a connection URL or a SQLite file path")
        });
        onboard_input.update(cx, |i, cx| i.focus(window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&command_input, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.state.command_bar_selected_index = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.confirm_command_bar(window, cx),
                _ => {}
            }),
            cx.subscribe_in(&conn_path_input, window, |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.connect_add_conn_modal(window, cx),
                // Pasting a URL picks the database type (C1)
                InputEvent::Change => {
                    this.state.add_conn_kind = guess_profile(&input.read(cx).value()).kind;
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe_in(&onboard_input, window, |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().trim().to_string();
                    if !text.is_empty() {
                        this.connect_profile(guess_profile(&text), window, cx);
                    }
                }
            }),
        ];

        let theme = Theme::for_mode(os_is_dark(window));
        let panel = bottom_panel::PanelState::new(&theme, window, cx);
        let mut this = Self {
            state,
            theme,
            panel,
            focus_handle: cx.focus_handle(),
            command_input,
            conn_name_input,
            conn_path_input,
            _subscriptions: subscriptions,
            tab_subscriptions: Default::default(),
            onboard_input,
            theme_override: None,
            toast_task: None,
        };
        this.apply_theme(cx);
        this
    }

    /// Loads the in-memory demo database (users, products, orders) and opens it
    fn open_demo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // In-memory SQLite seeding is sub-millisecond; fine to block here.
        DB_RUNTIME.block_on(self.state.init_demo_playground());
        self.open_table_tab("users", "main", window, cx);
        self.refresh_active_tab(window, cx);
        cx.notify();
    }

    /// Theme follows the OS appearance unless the user picked one
    fn sync_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let dark = self.theme_override.unwrap_or_else(|| os_is_dark(window));
        if dark != self.theme.is_dark {
            self.theme = Theme::for_mode(dark);
            self.apply_theme(cx);
        }
    }

    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        use gpui::component::theme::{Theme as KitTheme, ThemeMode};
        KitTheme::change(if self.theme.is_dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
        let grids: Vec<_> = self.state.tabs.iter().map(|t| t.grid().clone()).chain(self.panel.grids()).collect();
        for grid in grids {
            grid.update(cx, |t, cx| {
                t.delegate_mut().set_theme(&self.theme);
                cx.notify();
            });
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_override = Some(!self.theme.is_dark);
        self.sync_theme(window, cx);
        cx.notify();
    }

    /// First run (U11): one input, nothing else
    fn render_onboarding(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(520.0))
                    .gap_3()
                    .child(Icon::new(IconName::Database).text_color(t.text_muted))
                    .child(div().text_lg().text_color(t.text_primary).child("Connect a database"))
                    .child(Input::new(&self.onboard_input))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("postgres://user@host:5432/db  ·  ~/data/app.sqlite  ·  :memory:  ·  or drop a SQLite file here"),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("onboard-demo")
                                    .label("Open the demo database")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| this.open_demo(window, cx))),
                            )
                            .child(
                                Button::new("onboard-form")
                                    .label("More options")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| this.open_add_conn(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Short-lived confirmation in the corner; also kept in Output
    fn toast(&mut self, msg: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let msg = msg.into();
        self.state.messages.push(if error { format!("Error: {msg}") } else { msg.clone() });
        self.state.toast_message = Some((msg, error));
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
            this.update(cx, |this, cx| {
                this.state.toast_message = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The connection the active tab uses: a table tab's own, otherwise the active one
    fn current_connection(&self, cx: &App) -> (Option<Arc<dyn DatabaseSession>>, Option<ConnectionProfile>) {
        match self.state.active_tab() {
            Some(Tab::Table { grid, connection, .. }) => {
                (grid.read(cx).delegate().source.as_ref().map(|s| s.session.clone()), connection.clone())
            }
            _ => (self.state.session.clone(), self.state.active_connection.clone()),
        }
    }

    fn open_query_tab(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.state.tabs.iter().filter(|t| matches!(t, Tab::Query { .. })).count() + 1;
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
                .default_value(sql.to_string())
        });
        editor.update(cx, |e, cx| e.focus(window, cx));
        let id = format!("q-{}", uuid::Uuid::new_v4());
        let grid = self.new_grid(&id, None, window, cx);
        self.state.tabs.push(Tab::Query {
            id,
            title: format!("query {}", n),
            editor,
            grid,
            is_running: false,
            error_message: None,
        });
        self.state.active_tab_index = self.state.tabs.len() - 1;
    }

    fn open_table_tab(&mut self, table_name: &str, schema_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(idx) = self.state.find_tab(table_name) {
            self.state.active_tab_index = idx;
            return;
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("WHERE condition, e.g. id > 1"));
        let Some(session) = self.state.session.clone() else { return };
        let schema = self
            .state
            .catalog
            .tables
            .iter()
            .find(|t| t.schema_name == schema_name && t.table_name == table_name);
        let read_only_reason = match schema {
            _ if session.is_read_only() => Some("Read-only connection"),
            Some(t) if t.is_view => Some("Views are read-only"),
            Some(t) if t.primary_keys.is_empty() => Some("Read-only: table has no primary key"),
            None => Some("Read-only: table not found in schema"),
            _ => None,
        };
        let (columns, primary_keys) = schema.map(|t| (t.columns.clone(), t.primary_keys.clone())).unwrap_or_default();
        let id = format!("t-{}.{}", schema_name, table_name);
        let grid = self.new_grid(
            &id,
            Some(TableSource {
                session,
                schema: schema_name.to_string(),
                table: table_name.to_string(),
                columns,
                primary_keys,
                read_only_reason,
                default_keyword: self.state.active_connection.as_ref().is_some_and(|c| c.kind == ConnectionTargetKind::Postgres),
                filter: String::new(),
                order_by: None,
                has_more: false,
                loading: false,
                error: None,
            }),
            window,
            cx,
        );
        self.state.tabs.push(Tab::Table {
            id,
            table_name: table_name.to_string(),
            schema_name: schema_name.to_string(),
            filter,
            grid,
            connection: self.state.active_connection.clone(),
        });
        self.state.active_tab_index = self.state.tabs.len() - 1;
    }

    fn new_grid(&mut self, tab_id: &str, source: Option<TableSource>, window: &mut Window, cx: &mut Context<Self>) -> Entity<TableState<ResultSet>> {
        let rs = ResultSet::new(&self.theme, source);
        let grid = cx.new(|cx| TableState::new(rs, window, cx).cell_selectable(true).col_resizable(true).col_selectable(false));
        // Double-click edits the cell (read-only grids explain why they can't)
        let sub = cx.subscribe_in(&grid, window, |_, grid, ev: &TableEvent, window, cx| {
            match *ev {
                TableEvent::DoubleClickedCell(row, col) => {
                    grid.update(cx, |t, cx| t.delegate_mut().start_edit(row, col, None, window, cx))
                }
                // The value editor follows the selection
                TableEvent::SelectCell(row, col) => grid.update(cx, |t, cx| {
                    if t.delegate().inspector_open() {
                        t.delegate_mut().inspect(row, col, window, cx);
                    }
                }),
                _ => {}
            }
        });
        self.tab_subscriptions.insert(tab_id.to_string(), sub);
        grid
    }

    /// Re-runs the active tab: table tabs reload their first page, query tabs run the statement at the cursor
    pub fn refresh_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_active_tab(false, window, cx);
    }

    fn run_active_tab(&mut self, run_all: bool, window: &mut Window, cx: &mut Context<Self>) {
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
    fn run_query_tab(&mut self, run_all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else { return };
        let Some(Tab::Query { id, editor, grid, is_running, error_message, .. }) =
            self.state.tabs.get_mut(self.state.active_tab_index)
        else {
            return;
        };
        if *is_running {
            return;
        }
        let ed = editor.read(cx);
        let text = ed.value();
        let sel = ed.selected_range();
        let statements: Vec<String> = if !sel.is_empty() {
            quarry_sql::split_statements(&text[sel]).into_iter().map(|s| s.sql).collect()
        } else if run_all {
            quarry_sql::split_statements(&text).into_iter().map(|s| s.sql).collect()
        } else {
            quarry_sql::find_statement_at_offset(&text, ed.cursor()).into_iter().map(|s| s.sql).collect()
        };
        if statements.is_empty() {
            return;
        }
        *is_running = true;
        *error_message = None;
        let (tab_id, grid) = (id.clone(), grid.clone());
        cx.notify();

        let stmts = statements.clone();
        let fut = async move {
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
                this.state.record_history(res.statement.clone(), res.execution_time, res.rows_affected, None);
            }
            let failure_msg = failure.map(|(ix, err)| {
                this.state.record_history(statements[ix].clone(), Default::default(), 0, Some(err.clone()));
                let msg = if statements.len() > 1 { format!("Statement {} failed: {}", ix + 1, err) } else { err };
                this.state.messages.push(format!("Error: {}", msg));
                msg
            });

            let row_sets: Vec<_> = done.into_iter().filter(|r| !r.columns.is_empty()).collect();
            grid.update(cx, |t, cx| {
                t.delegate_mut().set_results(row_sets);
                t.refresh(cx);
                cx.notify();
            });
            if let Some(Tab::Query { is_running, error_message, .. }) = this.state.tabs.iter_mut().find(|t| t.id() == tab_id) {
                *is_running = false;
                *error_message = failure_msg;
            }
            // Schema changed: keep the sidebar and command bar in sync (S6)
            let is_ddl = |s: &String| {
                let verb = s.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                matches!(verb.as_str(), "CREATE" | "DROP" | "ALTER")
            };
            if statements.iter().any(is_ddl) {
                this.refresh_catalog(window, cx);
            }
        });
    }

    /// Re-reads the schema tree of the active connection
    fn refresh_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else { return };
        run_db(window, cx, async move { session.introspect().await }, |this, res, _, _| match res {
            Ok(catalog) => this.state.catalog = catalog,
            Err(e) => this.state.messages.push(format!("Schema refresh failed: {}", e)),
        });
    }

    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Tab::Table { filter, .. }) = self.state.active_tab() {
            filter.clone().update(cx, |f, cx| f.set_value("", window, cx));
        }
        self.refresh_active_tab(window, cx);
    }

    fn refocus_root(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    // ── Actions ─────────────────────────────────────────────

    fn toggle_command_bar(&mut self, _: &ToggleCommandBar, window: &mut Window, cx: &mut Context<Self>) {
        self.state.command_bar_open = !self.state.command_bar_open;
        if self.state.command_bar_open {
            self.state.command_bar_selected_index = 0;
            self.command_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.focus(window, cx);
            });
        } else {
            self.refocus_root(window, cx);
        }
        cx.notify();
    }

    fn run_query(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.review_modal_open {
            return self.commit_pending_changes(window, cx);
        }
        self.refresh_active_tab(window, cx);
    }

    fn run_all(&mut self, _: &RunAll, window: &mut Window, cx: &mut Context<Self>) {
        self.run_active_tab(true, window, cx);
    }

    fn review_changes(&mut self, _: &ReviewChanges, _: &mut Window, cx: &mut Context<Self>) {
        let review_data = match self.state.active_tab() {
            Some(Tab::Table { table_name, grid, .. }) => {
                let rs = grid.read(cx).delegate();
                let name = rs.source.as_ref().map(|s| s.qualified_name()).unwrap_or_default();
                (!rs.pending.is_empty()).then(|| (table_name.clone(), rs.pending.generate_sql(&name)))
            }
            _ => None,
        };
        if let Some((table, sqls)) = review_data {
            self.state.review_modal_sql = sqls;
            self.state.review_modal_table = table;
            self.state.review_modal_open = true;
            cx.notify();
        }
    }

    /// Auto-commit ⇄ manual commit for the active connection (E9)
    fn toggle_tx_mode(&mut self, _: &ToggleTxMode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.current_connection(cx).0 else { return };
        if session.in_transaction() {
            self.state.messages.push("Commit or roll back the open transaction before switching modes.".into());
        } else {
            let manual = !session.tx().manual();
            session.tx().set_manual(manual);
            self.state.messages.push(format!("{} mode.", if manual { "Manual commit" } else { "Auto-commit" }));
        }
        cx.notify();
    }

    fn commit_tx(&mut self, _: &CommitTx, window: &mut Window, cx: &mut Context<Self>) {
        self.end_tx(true, window, cx);
    }

    fn rollback_tx(&mut self, _: &RollbackTx, window: &mut Window, cx: &mut Context<Self>) {
        self.end_tx(false, window, cx);
    }

    fn end_tx(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.current_connection(cx).0.filter(|s| s.in_transaction()) else { return };
        let fut = async move { if commit { session.commit().await } else { session.rollback().await } };
        run_db(window, cx, fut, move |this, res, window, cx| {
            match res {
                Ok(()) => this.toast(if commit { "Transaction committed" } else { "Transaction rolled back" }, false, cx),
                Err(e) => this.toast(e.to_string(), true, cx),
            }
            // What the grid shows may have just been undone
            if let Some(tab) = this.state.active_tab() {
                tab.grid().clone().update(cx, |t, cx| t.delegate_mut().fetch(Fetch::Reload, window, cx));
            }
        });
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.state.sidebar_visible = !self.state.sidebar_visible;
        cx.notify();
    }

    fn toggle_bottom_panel(&mut self, _: &ToggleBottomPanel, _: &mut Window, cx: &mut Context<Self>) {
        self.state.bottom_panel_open = !self.state.bottom_panel_open;
        cx.notify();
    }

    fn new_query(&mut self, _: &NewQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.open_query_tab(NEW_QUERY_SQL, window, cx);
        cx.notify();
    }

    fn close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.state.active_tab_index, cx);
    }

    fn close_tab_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.state.tabs.len() > 1 {
            if let Some(tab) = self.state.tabs.get(idx) {
                self.tab_subscriptions.remove(tab.id());
            }
        }
        self.state.close_tab(idx);
        cx.notify();
    }

    /// Escape: close the topmost overlay
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.state;
        let Some(open) = [
            &mut s.command_bar_open,
            &mut s.review_modal_open,
            &mut s.add_conn_modal_open,
        ]
        .into_iter()
        .find(|open| **open) else {
            return;
        };
        *open = false;
        self.refocus_root(window, cx);
        cx.notify();
    }

    fn menu_up(&mut self, _: &MenuUp, _: &mut Window, cx: &mut Context<Self>) {
        self.state.command_bar_selected_index = self.state.command_bar_selected_index.saturating_sub(1);
        cx.notify();
    }

    fn menu_down(&mut self, _: &MenuDown, _: &mut Window, cx: &mut Context<Self>) {
        let count = CommandBar::get_candidates(&self.state, &self.command_input.read(cx).value()).len();
        if self.state.command_bar_selected_index + 1 < count {
            self.state.command_bar_selected_index += 1;
        }
        cx.notify();
    }

    fn confirm_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let candidates = CommandBar::get_candidates(&self.state, &self.command_input.read(cx).value());
        let idx = self.state.command_bar_selected_index.min(candidates.len().saturating_sub(1));
        if let Some(item) = candidates.get(idx) {
            self.handle_command_action(item.id.clone(), window, cx);
        }
    }

    /// Handles actions triggered from the command bar
    pub fn handle_command_action(&mut self, action_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.state.command_bar_open = false;
        self.refocus_root(window, cx);

        if let Some(table_name) = action_id.strip_prefix("table:") {
            let schema = self
                .state
                .find_table_schema(table_name)
                .map(|t| t.schema_name.clone())
                .unwrap_or_default();
            self.open_table_tab(table_name, &schema, window, cx);
            self.refresh_active_tab(window, cx);
        } else if let Some(query) = action_id.strip_prefix("run:") {
            self.open_query_tab(query, window, cx);
            self.refresh_active_tab(window, cx);
        } else if action_id == "action:new_query" {
            self.new_query(&NewQuery, window, cx);
        } else if action_id == "action:review" {
            self.review_changes(&ReviewChanges, window, cx);
        } else if action_id == "action:add_conn" {
            self.open_add_conn(window, cx);
        } else if action_id == "action:toggle_sidebar" {
            self.toggle_sidebar(&ToggleSidebar, window, cx);
        } else if action_id == "action:theme" {
            self.toggle_theme(&ToggleTheme, window, cx);
        } else if action_id == "action:demo" {
            self.open_demo(window, cx);
        } else if action_id == "action:toggle_tx" {
            self.toggle_tx_mode(&ToggleTxMode, window, cx);
        } else if action_id == "action:commit_tx" {
            self.commit_tx(&CommitTx, window, cx);
        } else if action_id == "action:rollback_tx" {
            self.rollback_tx(&RollbackTx, window, cx);
        } else if action_id == "action:toggle_bottom" {
            self.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
        }

        cx.notify();
    }

    fn open_add_conn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.add_conn_modal_open = true;
        self.conn_name_input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn set_conn_inputs(&mut self, name: Option<&str>, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = name {
            self.conn_name_input.update(cx, |i, cx| i.set_value(name.to_string(), window, cx));
        }
        self.conn_path_input.update(cx, |i, cx| i.set_value(path.to_string(), window, cx));
    }

    fn profile_from_modal(&self, cx: &App) -> ConnectionProfile {
        let name = self.conn_name_input.read(cx).value().trim().to_string();
        ConnectionProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: if name.is_empty() { "New Connection".to_string() } else { name },
            kind: self.state.add_conn_kind.clone(),
            path_or_url: self.conn_path_input.read(cx).value().trim().to_string(),
            env: self.state.add_conn_env,
            read_only: self.state.add_conn_read_only,
        }
    }

    /// Connects to a new database using values in the Add Connection modal
    pub fn connect_add_conn_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.profile_from_modal(cx);
        self.state.add_conn_feedback = Some(("Connecting…".to_string(), false));
        self.connect_profile(profile, window, cx);
    }

    /// Sidebar click: switch to a saved connection, reusing its session if it's already open
    fn connect_saved(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.active_connection.as_ref().is_some_and(|c| c.id == id) {
            return;
        }
        if self.state.activate_open_session(&id) {
            cx.notify();
            return;
        }
        if let Some(profile) = self.state.saved_connections.iter().find(|c| c.id == id).cloned() {
            self.connect_profile(profile, window, cx);
        }
    }

    fn connect_profile(&mut self, profile: ConnectionProfile, window: &mut Window, cx: &mut Context<Self>) {
        let name = profile.name.clone();
        self.state.messages.push(format!("Connecting to {}…", name));
        cx.notify();
        let log = self.state.activity_log.clone();
        run_db(window, cx, async move { open_session(&profile, log).await.map(|(s, c)| (profile, s, c)) }, move |this, res, window, cx| {
            match res {
                Ok((profile, session, catalog)) => {
                    this.toast(format!("Connected to {}", profile.name), false, cx);
                    this.state.apply_connection(profile, session, catalog);
                    this.state.add_conn_modal_open = false;
                    this.state.add_conn_feedback = None;
                    this.refocus_root(window, cx);
                }
                Err(e) => {
                    this.toast(format!("Could not connect to {}: {}", name, e), true, cx);
                    this.state.add_conn_feedback = Some((e, true));
                }
            }
        });
    }

    /// Tests connection parameters
    pub fn test_add_conn_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.profile_from_modal(cx);
        self.state.add_conn_feedback = Some(("Testing…".to_string(), false));
        cx.notify();

        run_db(
            window,
            cx,
            async move {
                let (session, _) = open_session(&profile, Default::default()).await?;
                session.ping().await.map_err(|e| e.to_string())
            },
            |this, res, _window, _cx| {
                this.state.add_conn_feedback = Some(match res {
                    Ok(dur) => (format!("Connection successful ({:.2}ms)", dur.as_secs_f64() * 1000.0), false),
                    Err(e) => (format!("Failed: {}", e), true),
                });
            },
        );
    }

    /// Executes the pending changes against the database and refreshes the table
    pub fn commit_pending_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.review_modal_open = false;
        let Some(Tab::Table { grid, .. }) = self.state.active_tab() else { return };
        let grid = grid.clone();
        // The tab's own connection, which may differ from the active one
        let Some((session, table, changes)) = grid.update(cx, |t, cx| {
            let rs = t.delegate_mut();
            rs.finish_edit(true, cx); // an open editor counts as part of the submit
            let src = rs.source.as_ref()?;
            (!rs.pending.is_empty()).then(|| (src.session.clone(), src.qualified_name(), rs.pending.clone()))
        }) else {
            return;
        };

        run_db(window, cx, async move { session.apply_edits(&table, &changes).await.map(|n| (n, table)) }, move |this, res, window, cx| {
            let toast = grid.update(cx, |t, cx| {
                let rs = t.delegate_mut();
                let toast = match res {
                    Ok((n, table)) => {
                        rs.pending.clear();
                        rs.notice = None;
                        // Re-read what's loaded without jumping to the top
                        rs.fetch(Fetch::Reload, window, cx);
                        (format!("Applied {} {} to {}", n, if n == 1 { "change" } else { "changes" }, table.replace('"', "")), false)
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
        });
    }
}

impl Render for QuarryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_theme(window, cx);
        let bottom_panel = self.render_bottom_panel(window, cx);
        let kill_confirm = self.render_kill_confirm(cx);
        let active_tab = self.state.active_tab().cloned();
        let (current_session, current_conn) = self.current_connection(cx);
        let t = self.theme.clone();

        // macOS: clear the traffic lights; elsewhere a normal gutter
        #[cfg(target_os = "macos")]
        let titlebar_left_pad = px(78.0);
        #[cfg(not(target_os = "macos"))]
        let titlebar_left_pad = px(12.0);

        let env_color = |env: Environment| match env {
            Environment::Local => t.success,
            Environment::Staging => t.warning,
            Environment::Prod => t.danger,
        };

        // Title bar: where you are, and the one box that does everything (⌘K)
        let titlebar = h_flex()
            .h(px(38.0))
            .w_full()
            .pl(titlebar_left_pad)
            .pr_3()
            .bg(t.bg_sidebar)
            .border_b_1()
            .border_color(t.border)
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .gap_1p5()
                    .items_center()
                    .when_some(current_conn.clone(), |this, c| {
                        this.child(div().size(px(6.0)).rounded_full().bg(env_color(c.env)))
                            .child(div().text_xs().text_color(t.text_secondary).child(c.name.clone()))
                            .when(c.env != Environment::Local, |this| {
                                this.child(div().text_xs().font_weight(FontWeight::BOLD).text_color(env_color(c.env)).child(c.env.label()))
                            })
                    }),
            )
            .child(
                h_flex()
                    .id("omnibox-trigger-btn")
                    .w(px(360.0))
                    .h(px(26.0))
                    .px_2p5()
                    .gap_2()
                    .rounded(px(6.0))
                    .bg(t.bg_panel)
                    .border_1()
                    .border_color(t.border)
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_command_bar(&ToggleCommandBar, window, cx)))
                    .child(Icon::new(IconName::Search).xsmall().text_color(t.text_muted))
                    .child(div().flex_1().text_xs().text_color(t.text_muted).child("Search tables, run SQL, or type > for commands"))
                    .child(div().text_xs().text_color(t.text_muted).child("⌘K")),
            )
            .child(div().flex_1());

        // Tabs: icon + title + close; hidden when nothing is open
        let tab_strip = (!self.state.tabs.is_empty()).then(|| {
            h_flex()
                .h(px(32.0))
                .w_full()
                .px_1()
                .gap_px()
                .bg(t.bg_panel)
                .border_b_1()
                .border_color(t.border)
                .items_end()
                .children(self.state.tabs.iter().enumerate().map(|(idx, tab)| {
                    let is_active = idx == self.state.active_tab_index;
                    let icon = match tab {
                        Tab::Query { .. } => IconName::SquareTerminal,
                        Tab::Table { .. } => IconName::Table,
                    };
                    h_flex()
                        .id(("tab-select", idx))
                        .h(px(28.0))
                        .pl_2p5()
                        .pr_1()
                        .gap_1p5()
                        .items_center()
                        .rounded_t(px(5.0))
                        .cursor_pointer()
                        .when(is_active, |d| d.bg(t.bg_app))
                        .when(!is_active, |d| d.hover(|s| s.bg(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.active_tab_index = idx;
                            cx.notify();
                        }))
                        .child(Icon::new(icon).xsmall().text_color(if is_active { t.accent } else { t.text_muted }))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if is_active { t.text_primary } else { t.text_secondary })
                                .child(tab.title().to_string()),
                        )
                        .child(
                            Button::new(("tab-close", idx))
                                .icon(IconName::X)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_tab_at(idx, cx);
                                    cx.notify();
                                })),
                        )
                }))
                .child(
                    Button::new("tab-add-btn")
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .tooltip("New query (⌘N)")
                        .on_click(cx.listener(|this, _, window, cx| this.new_query(&NewQuery, window, cx))),
                )
        });

        let center: AnyElement = match active_tab.as_ref() {
            None if self.state.session.is_none() => self.render_onboarding(cx),
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_sm().text_color(t.text_secondary).child("Pick a table from the sidebar"))
                .child(div().text_xs().text_color(t.text_muted).child("or press ⌘K to search, ⌘N for a new query"))
                .into_any_element(),
            Some(tab) => v_flex()
                .size_full()
                .child(match tab {
                    Tab::Query { editor, is_running, error_message, .. } => QueryEditor::render(
                        editor,
                        *is_running,
                        error_message.as_deref(),
                        &self.theme,
                        cx,
                        |this, window, cx| this.refresh_active_tab(window, cx),
                    )
                    .into_any_element(),
                    Tab::Table { filter, .. } => h_flex()
                        .h(px(34.0))
                        .w_full()
                        .px_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(t.border)
                        .items_center()
                        // Single-line inputs propagate Enter: apply the filter
                        .on_action(cx.listener(|this, _: &gpui::component::input::Enter, window, cx| {
                            this.refresh_active_tab(window, cx);
                        }))
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(t.text_muted).child("WHERE"))
                        .child(div().flex_1().child(Input::new(filter).appearance(false)))
                        .child(
                            Button::new("filter-clear-btn")
                                .label("Clear")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| this.clear_filter(window, cx))),
                        )
                        .into_any_element(),
                })
                .child(div().flex_1().min_h_0().child(DataGrid::render(
                    tab.grid(),
                    &self.theme,
                    cx,
                    |this, window, cx| this.refresh_active_tab(window, cx),
                    |this, window, cx| this.review_changes(&ReviewChanges, window, cx),
                    |this, window, cx| this.commit_pending_changes(window, cx),
                )))
                .into_any_element(),
        };

        let sidebar = Sidebar::render(
            &self.state,
            &self.theme,
            cx,
            |this, table_name, schema_name, window, cx| {
                this.open_table_tab(&table_name, &schema_name, window, cx);
                this.refresh_active_tab(window, cx);
                cx.notify();
            },
            |this, window, cx| this.open_add_conn(window, cx),
            |this, schema_name, _window, cx| {
                this.state.toggle_schema(&schema_name);
                cx.notify();
            },
            |this, id, window, cx| this.connect_saved(id, window, cx),
        );

        div()
            .id("quarry-root")
            .track_focus(&self.focus_handle)
            .key_context("Quarry")
            .on_action(cx.listener(Self::toggle_command_bar))
            .on_action(cx.listener(Self::run_query))
            .on_action(cx.listener(Self::run_all))
            .on_action(cx.listener(Self::review_changes))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_bottom_panel))
            .on_action(cx.listener(Self::new_query))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::toggle_tx_mode))
            .on_action(cx.listener(Self::commit_tx))
            .on_action(cx.listener(Self::rollback_tx))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::menu_up))
            .on_action(cx.listener(Self::menu_down))
            // Drop a SQLite file anywhere to open it (C3, U11)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.connect_profile(guess_profile(&path.to_string_lossy()), window, cx);
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg_app)
            .text_color(t.text_primary)
            .child(titlebar)
            .child(div().flex_1().min_h_0().child(
                h_resizable("main-split")
                    .child(
                        resizable_panel()
                            .size(px(240.0))
                            .size_range(px(160.0)..px(480.0))
                            .visible(self.state.sidebar_visible)
                            .child(sidebar),
                    )
                    .child(
                        resizable_panel().child(
                            v_resizable("content-split")
                                .child(resizable_panel().child(v_flex().size_full().children(tab_strip).child(center)))
                                .child(
                                    resizable_panel()
                                        .size(px(280.0))
                                        .size_range(px(120.0)..px(900.0))
                                        .visible(self.state.bottom_panel_open)
                                        .child(bottom_panel),
                                ),
                        ),
                    ),
            ))
            .child(StatusBar::render(
                &self.state,
                current_session.as_ref(),
                current_conn.as_ref(),
                &self.theme,
                cx,
                |this, _window, cx| {
                    this.state.bottom_panel_open = !this.state.bottom_panel_open;
                    cx.notify();
                },
                |this, _window, cx| {
                    this.state.sidebar_visible = !this.state.sidebar_visible;
                    cx.notify();
                },
                |this, window, cx| this.toggle_tx_mode(&ToggleTxMode, window, cx),
                |this, window, cx| this.commit_tx(&CommitTx, window, cx),
                |this, window, cx| this.rollback_tx(&RollbackTx, window, cx),
            ))
            // Modals & Overlays (only rendered when active)
            .children(kill_confirm)
            .when_some(self.state.toast_message.clone(), |this, (msg, error)| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(36.0))
                        .right(px(16.0))
                        .max_w(px(480.0))
                        .px_3()
                        .py_2()
                        .rounded(px(6.0))
                        .bg(t.bg_panel)
                        .border_1()
                        .border_color(if error { t.danger } else { t.border })
                        .shadow_lg()
                        .text_xs()
                        .text_color(if error { t.danger } else { t.text_primary })
                        .child(msg),
                )
            })
            .when(self.state.command_bar_open, |this| {
                this.child(CommandBar::render(
                    &self.state,
                    &self.theme,
                    cx,
                    &self.command_input,
                    |this, action_id, window, cx| {
                        this.handle_command_action(action_id, window, cx);
                    },
                    |this, window, cx| {
                        this.dismiss(&Dismiss, window, cx);
                    },
                ))
            })
            .when(self.state.review_modal_open, |this| {
                this.child(ReviewModal::render(
                    &self.state,
                    &self.theme,
                    cx,
                    |this, window, cx| {
                        this.commit_pending_changes(window, cx);
                    },
                    |this, _window, cx| {
                        this.state.review_modal_open = false;
                        cx.notify();
                    },
                ))
            })
            .when(self.state.add_conn_modal_open, |this| {
                this.child(AddConnModal::render(
                    &self.state,
                    &self.theme,
                    cx,
                    &self.conn_name_input,
                    &self.conn_path_input,
                    |this, kind, window, cx| {
                        let path = if kind == ConnectionTargetKind::Postgres {
                            "postgres://postgres:postgres@localhost:5432/postgres"
                        } else {
                            ":memory:"
                        };
                        this.state.add_conn_kind = kind;
                        this.set_conn_inputs(None, path, window, cx);
                        cx.notify();
                    },
                    |this, env, _window, cx| {
                        this.state.add_conn_env = env;
                        cx.notify();
                    },
                    |this, window, cx| {
                        this.connect_add_conn_modal(window, cx);
                    },
                    |this, window, cx| {
                        this.test_add_conn_modal(window, cx);
                    },
                    |this, _window, cx| {
                        this.state.add_conn_modal_open = false;
                        cx.notify();
                    },
                ))
            })
    }
}

fn main() {
    gpui::application().with_assets(gpui::assets::AllAssets).run(|cx: &mut App| {
        gpui::init(cx);
        // Quarry's own palette is dark-only for now; keep gpui-kit widgets in sync
        gpui::component::theme::Theme::change(gpui::component::theme::ThemeMode::Dark, None, cx);
        bind_keys(cx);
        grid::bind_keys(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: None, // Custom unified titlebar with cross-platform OS traffic light offset
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.0), px(12.0))),
            }),
            ..Default::default()
        };

        gpui::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                let mut app_window = QuarryWindow::new(window, cx);
                // Pre-populate data for initial users table tab
                app_window.refresh_active_tab(window, cx);
                window.focus(&app_window.focus_handle, cx);
                app_window
            })
        })
        .unwrap();
    });
}

/// Turns pasted text into a connection: Postgres URLs / key=value strings, `:memory:`, or a SQLite file path
fn guess_profile(text: &str) -> ConnectionProfile {
    let text = text.trim();
    let (kind, name) = if text.starts_with("postgres://") || text.starts_with("postgresql://") || text.contains("host=") {
        let (bare, _) = connections::split_password(text);
        let host_db = bare.split_once('@').map(|(_, rest)| rest).unwrap_or(&bare);
        (ConnectionTargetKind::Postgres, host_db.split('?').next().unwrap_or(host_db).to_string())
    } else if text == ":memory:" {
        (ConnectionTargetKind::SqliteMemory, "In-memory SQLite".to_string())
    } else {
        let name = std::path::Path::new(text).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| text.to_string());
        (ConnectionTargetKind::SqliteFile, name)
    };
    let path = match text.strip_prefix("~/") {
        Some(rest) if kind == ConnectionTargetKind::SqliteFile => {
            std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| text.to_string())
        }
        _ => text.to_string(),
    };
    ConnectionProfile {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        kind,
        path_or_url: path,
        env: Environment::Local,
        read_only: false,
    }
}

fn os_is_dark(window: &Window) -> bool {
    matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark)
}
