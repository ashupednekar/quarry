// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

mod add_conn_modal;
mod bottom_panel;
mod command_bar;
mod completion;
mod connections;
mod editor;
mod er_view;
mod export_dialog;
mod files;
mod grid;
mod history;
mod import_wizard;
mod layout;
mod object_menu;
mod params_dialog;
mod peek;
mod pins;
mod plan_view;
mod review_modal;
mod runtime;
mod settings;
mod settings_modal;
mod sidebar;
mod ssh;
mod state;
mod status_bar;
mod tab_switcher;
mod table_detail;
mod theme;
mod ui;
mod usages;

use add_conn_modal::AddConnModal;
use command_bar::CommandBar;
use editor::QueryEditor;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{EditorState, Input, InputEvent, InputState};
use gpui::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui::component::table::{TableEvent, TableState};
use gpui::component::{ActiveTheme, Icon, Sizable};
use gpui::*;
use grid::{DataGrid, Fetch, ResultSet, TableSource};
use quarry_core::DatabaseSession;
use review_modal::ReviewModal;
use runtime::{run_db, run_db_bg, DB_RUNTIME};
use sidebar::Sidebar;
use state::{open_session, AppState, ConnectionProfile, Environment, Tab};
use status_bar::StatusBar;
use std::sync::Arc;
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
        CloseWindow,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        Dismiss,
        ToggleMaximize,
        OpenSettings,
        ToggleTxMode,
        CommitTx,
        RollbackTx,
        ToggleTheme,
        ExportResults,
        Explain,
        ExplainAnalyze,
        FormatSql,
        ReopenClosedTab,
        TabSwitcherNext,
        TabSwitcherPrev,
        ToggleSplit,
        PinCurrentTab,
        CommandPalette,
        SwitchSpace1,
        SwitchSpace2,
        SwitchSpace3,
        SwitchSpace4,
        SwitchSpace5,
        SwitchSpace6,
        SwitchSpace7,
        SwitchSpace8,
        SwitchSpace9,
        OpenInSplit,
        SaveFile,
        SaveFileAs,
        OpenFile,
        MenuUp,
        MenuDown,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
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
        // Standard macOS window/app shortcuts (also shown in the menu bar)
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-h", Hide, None),
        KeyBinding::new("alt-secondary-h", HideOthers, None),
        KeyBinding::new("secondary-m", Minimize, None),
        KeyBinding::new("shift-secondary-w", CloseWindow, None),
        KeyBinding::new("shift-escape", ToggleMaximize, None),
        KeyBinding::new("secondary-shift-e", ExportResults, None),
        KeyBinding::new("secondary-alt-e", Explain, None),
        KeyBinding::new("shift-alt-f", FormatSql, None),
        KeyBinding::new(
            "alt-enter",
            gpui::base::input::ToggleCodeActions,
            Some("Input"),
        ),
        KeyBinding::new("shift-alt-f", FormatSql, Some("Input")),
        KeyBinding::new("secondary-shift-s", SaveFileAs, None),
        KeyBinding::new("secondary-o", OpenFile, None),
        KeyBinding::new("secondary-shift-t", ReopenClosedTab, None),
        KeyBinding::new("ctrl-tab", TabSwitcherNext, None),
        KeyBinding::new("ctrl-shift-tab", TabSwitcherPrev, None),
        KeyBinding::new("secondary-shift-p", CommandPalette, None),
        // Spaces (U5): one keystroke to switch connection
        KeyBinding::new("ctrl-1", SwitchSpace1, None),
        KeyBinding::new("ctrl-2", SwitchSpace2, None),
        KeyBinding::new("ctrl-3", SwitchSpace3, None),
        KeyBinding::new("ctrl-4", SwitchSpace4, None),
        KeyBinding::new("ctrl-5", SwitchSpace5, None),
        KeyBinding::new("ctrl-6", SwitchSpace6, None),
        KeyBinding::new("ctrl-7", SwitchSpace7, None),
        KeyBinding::new("ctrl-8", SwitchSpace8, None),
        KeyBinding::new("ctrl-9", SwitchSpace9, None),
        KeyBinding::new("secondary-\\", ToggleSplit, None),
        // Font size / zoom shortcuts (⌘+/⌘- on macOS, Ctrl+/Ctrl- on others)
        KeyBinding::new("secondary-+", IncreaseFontSize, None),
        KeyBinding::new("secondary-=", IncreaseFontSize, None),
        KeyBinding::new("secondary-shift-=", IncreaseFontSize, None),
        KeyBinding::new("secondary-shift-+", IncreaseFontSize, None),
        KeyBinding::new("secondary-+", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-=", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-shift-=", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-shift-+", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary--", DecreaseFontSize, None),
        KeyBinding::new("secondary--", DecreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-0", ResetFontSize, None),
        KeyBinding::new("secondary-0", ResetFontSize, Some("Input")),
        // ⌘↵ on a command-bar result opens it in the split (U3); registered after the Input binding so it wins
        KeyBinding::new("secondary-enter", OpenInSplit, Some("CommandBar > Input")),
        KeyBinding::new("secondary-alt-shift-e", ExplainAnalyze, None),
        KeyBinding::new("up", MenuUp, Some("CommandBar > Input")),
        KeyBinding::new("down", MenuDown, Some("CommandBar > Input")),
    ]);
}

#[derive(Clone, Copy, PartialEq)]
pub enum RunMode {
    Current,
    All,
    Explain,
    ExplainAnalyze,
}

struct QuarryWindow {
    state: AppState,
    theme: Theme,
    focus_handle: FocusHandle,
    command_input: Entity<InputState>,
    conn_name_input: Entity<InputState>,
    conn_path_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    onboard_input: Entity<InputState>,
    /// SSH tunnel fields in the connection form: host[:port], user, key file
    ssh_inputs: [Entity<InputState>; 3],
    /// One input per field of the picked driver (`Driver::fields`); kept in sync with the URL field
    conn_field_inputs: Vec<Entity<InputState>>,
    conn_field_subs: Vec<Subscription>,
    /// Set while one side of the URL/fields sync writes the other, so it doesn't echo back
    conn_syncing: bool,
    /// Statement timeout (seconds) in the connection form
    timeout_input: Entity<InputState>,
    /// `Some(dark)` once the user picks a theme; otherwise follow the OS
    /// The picked theme; None follows the OS between Quarry Dark and Light
    theme_choice: Option<Theme>,
    toast_task: Option<Task<()>>,
    session_save_task: Option<Task<()>>,
    /// Recently closed tabs, newest last, for ⇧⌘T
    closed_tabs: Vec<(Tab, Vec<(String, Subscription)>)>,
    /// Tab shown in the right pane of the split view, by id
    split_id: Option<String>,
    peek: Option<peek::Peek>,
    settings: settings::Settings,
    /// Settings last pushed into editors and grids (they need a window, so this happens at render)
    applied_settings: Option<settings::Settings>,
    settings_open: bool,
    /// Theme names for the picker, read when Settings opens
    settings_themes: Vec<String>,
    /// Data-changing statements waiting for a "yes" on a PROD connection
    pending_prod_run: Option<Vec<String>>,
    peek_task: Option<Task<()>>,
    /// File the last toast can reveal in the file manager
    toast_reveal: Option<std::path::PathBuf>,
    export: export_dialog::ExportState,
    import: import_wizard::ImportState,
    params: Option<params_dialog::ParamsDialog>,
    /// Open grid context menu: (position, menu, dismiss subscription)
    grid_menu: Option<(
        Point<Pixels>,
        Entity<gpui::component::menu::PopupMenu>,
        Subscription,
    )>,
    /// The query tab whose connection was last synced with the active one (see `sync_tab_connection`)
    synced_tab: Option<String>,
    /// Tab ids, most recently active first (⌃Tab order)
    tab_mru: Vec<String>,
    /// A query tab's connection being reopened; the tab keeps it instead of following the active one meanwhile
    connecting_for_tab: Option<String>,
    /// Table tabs from last session, waiting for their connection (connection id, table)
    pending_table_tabs: Vec<(String, files::SavedTable)>,
    /// Results fill the query tab, editor hidden
    results_expanded: bool,
    /// ⌃Tab switcher open, with this index into `tab_mru` highlighted
    tab_switcher: Option<usize>,
    /// Last value entered for each parameter name
    param_memory: std::collections::HashMap<String, String>,
    /// Per-tab subscriptions (grid events), dropped with the tab
    tab_subscriptions: std::collections::HashMap<String, Subscription>,
    panel: bottom_panel::PanelState,
    confirm_close_open: bool,
    force_close: bool,
}

impl QuarryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::new();

        let command_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search tables, actions, or type SQL…")
        });
        // Empty by default: a blank name is filled from the address (host:port/db), never a stale "Local SQLite"
        let conn_name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Instance name, e.g. docker-postgres (optional)")
        });
        let conn_path_input = cx.new(|cx| InputState::new(window, cx).default_value(":memory:"));
        let onboard_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste a connection URL or a SQLite file path")
        });
        onboard_input.update(cx, |i, cx| i.focus(window, cx));
        let timeout_input = cx.new(|cx| InputState::new(window, cx).placeholder("No timeout"));
        let ssh_inputs = [
            cx.new(|cx| {
                InputState::new(window, cx).placeholder("SSH host, e.g. bastion.example.com:22")
            }),
            cx.new(|cx| InputState::new(window, cx).placeholder("SSH user")),
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Private key file (optional; default: ssh-agent)")
            }),
        ];

        let subscriptions = vec![
            cx.subscribe_in(
                &command_input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.state.command_bar_selected_index = 0;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.confirm_command_bar(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &conn_path_input,
                window,
                |this, input, ev: &InputEvent, _window, cx| match ev {
                    // Pasting a URL picks the database type (C1)
                    InputEvent::Change => {
                        let url = input.read(cx).value().to_string();
                        if let Some(d) = quarry_core::detect(&url) {
                            this.set_conn_driver(d.id(), _window, cx);
                        }
                        this.fill_conn_fields(_window, cx);
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &onboard_input,
                window,
                |this, input, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        let text = input.read(cx).value().trim().to_string();
                        if !text.is_empty() {
                            this.connect_address(&text, window, cx);
                        }
                    }
                },
            ),
        ];
        let theme = Theme::for_mode(os_is_dark(window));
        let export = export_dialog::ExportState::new(window, cx);
        let import = import_wizard::ImportState::new(&theme, window, cx);
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
            ssh_inputs,
            conn_field_inputs: Vec::new(),
            conn_field_subs: Vec::new(),
            conn_syncing: false,
            timeout_input,
            theme_choice: None,
            toast_task: None,
            session_save_task: None,
            closed_tabs: Vec::new(),
            split_id: None,
            peek: None,
            settings: settings::Settings::default(),
            applied_settings: None,
            settings_open: false,
            settings_themes: Vec::new(),
            pending_prod_run: None,
            peek_task: None,
            toast_reveal: None,
            export,
            import,
            params: None,
            grid_menu: None,
            synced_tab: None,
            tab_mru: Vec::new(),
            connecting_for_tab: None,
            pending_table_tabs: Vec::new(),
            results_expanded: false,
            tab_switcher: None,
            param_memory: Default::default(),
            confirm_close_open: false,
            force_close: false,
        };
        this.reload_settings(cx);
        let keymap_errors = settings::apply_keymap(cx);
        if let Some(e) = keymap_errors.first() {
            this.toast(e.clone(), true, cx);
        }
        this.apply_theme(cx);
        this.restore_session(window, cx);
        this._subscriptions.push(cx.on_app_quit(|this, cx| {
            this.save_session(cx);
            async {}
        }));
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
    /// Reads settings.json and applies theme, fonts and defaults (P3, P4)
    fn reload_settings(&mut self, cx: &mut Context<Self>) {
        let (s, mut error) = settings::load();
        self.theme_choice = match s.theme.trim().to_lowercase().as_str() {
            "" | "system" => None,
            name => match Theme::named(name, true) {
                Ok(t) => Some(t),
                Err(e) => {
                    error = Some(e);
                    None
                }
            },
        };
        // Re-paint even when only a user theme's colours changed (same name)
        self.theme.name.clear();
        {
            let kit = gpui::component::theme::Theme::global_mut(cx);
            kit.font_size = px(s.font_size);
            kit.mono_font_size = px(s.mono_size());
        }
        theme::apply_fonts(cx, &s.mono_font_family);
        self.settings = s;
        if let Some(e) = error {
            self.toast(e, true, cx);
        }
        cx.refresh_windows();
    }

    /// Pushes editor and grid settings into every open view
    fn apply_settings_to_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.settings.clone();
        for tab in &self.state.tabs {
            if let Tab::Query { editor, .. } = tab {
                editor.update(cx, |e, cx| {
                    e.set_tab_size(
                        gpui::base::input::TabSize {
                            tab_size: s.tab_size.max(1),
                            hard_tabs: false,
                        },
                        cx,
                    );
                    e.set_soft_wrap(s.soft_wrap, window, cx);
                    e.set_line_number(s.line_numbers, window, cx);
                    e.lsp_mut().completion_provider = s.autocomplete.then(|| {
                        std::rc::Rc::new(completion::SchemaCompletions(
                            self.state.completion_catalog.clone(),
                        ))
                            as std::rc::Rc<dyn gpui::component::input::CompletionProvider>
                    });
                });
            }
        }
        let grids: Vec<_> = self
            .state
            .tabs
            .iter()
            .map(|t| t.grid().clone())
            .chain(self.panel.grids())
            .collect();
        for grid in grids {
            grid.update(cx, |t, cx| {
                t.delegate_mut()
                    .set_prefs(&s.null_display, s.max_column_width);
                cx.notify();
            });
        }
        self.applied_settings = Some(s);
    }

    fn open_config_file(&mut self, keymap: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (path, starter) = if keymap {
            (
                settings::keymap_path(),
                settings::STARTER_KEYMAP.to_string(),
            )
        } else {
            (settings::settings_path(), settings::starter_settings())
        };
        let Some(path) = path else { return };
        settings::ensure_file(&path, &starter);
        let text = std::fs::read_to_string(&path).unwrap_or(starter);
        self.open_query_tab_with(&text, Some(path), window, cx);
        cx.notify();
    }

    fn sync_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let want = self
            .theme_choice
            .clone()
            .unwrap_or_else(|| Theme::for_mode(os_is_dark(window)));
        if want.name != self.theme.name {
            self.theme = want;
            self.apply_theme(cx);
        }
    }

    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        use gpui::component::theme::{Theme as KitTheme, ThemeMode};
        KitTheme::change(
            if self.theme.is_dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );
        theme::apply_fonts(cx, &self.settings.mono_font_family);
        self.theme.apply_to_kit(cx);
        let grids: Vec<_> = self
            .state
            .tabs
            .iter()
            .map(|t| t.grid().clone())
            .chain(self.panel.grids())
            .collect();
        for grid in grids {
            grid.update(cx, |t, cx| {
                t.delegate_mut().set_theme(&self.theme);
                cx.notify();
            });
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_choice = Some(Theme::for_mode(!self.theme.is_dark));
        self.sync_theme(window, cx);
        cx.notify();
    }

    fn increase_font_size(&mut self, _: &IncreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings.font_size < settings::Settings::MAX_FONT_SIZE {
            self.change_setting(|s| s.increase_font_size(), cx);
        }
    }

    fn decrease_font_size(&mut self, _: &DecreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings.font_size > settings::Settings::MIN_FONT_SIZE {
            self.change_setting(|s| s.decrease_font_size(), cx);
        }
    }

    fn reset_font_size(&mut self, _: &ResetFontSize, _: &mut Window, cx: &mut Context<Self>) {
        if (self.settings.font_size - settings::Settings::default().font_size).abs() > f32::EPSILON
            || self.settings.mono_font_size > 0.0
        {
            self.change_setting(|s| s.reset_font_size(), cx);
        }
    }

    /// One tab's content: editor over results for queries, filter bar over the grid for tables
    fn render_tab_view(&mut self, tab: &Tab, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let grid = div().size_full().child(DataGrid::render(
            tab.grid(),
            &self.theme,
            cx,
            |this, window, cx| this.refresh_active_tab(window, cx),
            |this, window, cx| this.review_changes(&ReviewChanges, window, cx),
            |this, window, cx| this.commit_pending_changes(window, cx),
            |this, _, cx| this.open_export(cx),
            |this, target, window, cx| this.follow_foreign_key(target, window, cx),
        ));
        match tab {
            // Config files (settings, keymap) are plain editors: nothing to run
            Tab::Query {
                editor,
                file: Some(f),
                ..
            } if f.extension().is_some_and(|e| e == "json") => div()
                .size_full()
                .child(
                    gpui::component::input::Editor::new(editor)
                        .h_full()
                        .bordered(false),
                )
                .into_any_element(),
            // Editor over results, with a draggable divider. Until the first run the editor gets the whole tab.
            Tab::Query {
                editor,
                is_running,
                error_message,
                grid: results,
                schema,
                row_limit,
                plan,
                ..
            } => {
                let conn = self
                    .state
                    .active_connection
                    .as_ref()
                    .map(|c| (c.name.as_str(), c.env));
                let switches_schema = self.state.dialect().switches_schema();
                let schema_label = switches_schema
                    .then(|| schema.clone().unwrap_or_else(|| "default schema".into()));
                let editor = QueryEditor::render(
                    editor,
                    *is_running,
                    self.settings.soft_wrap,
                    conn,
                    schema_label,
                    *row_limit,
                    self.state.dialect().split_rules(),
                    &self.theme,
                    cx,
                    |this, window, cx| this.refresh_active_tab(window, cx),
                    |this, window, cx| this.open_connection_menu(window, cx),
                    |this, window, cx| this.open_schema_menu(window, cx),
                    |this, window, cx| this.open_limit_menu(window, cx),
                );
                let summary = {
                    let rs = results.read(cx).delegate();
                    rs.results.get(rs.active).map(|r| {
                        let ms = r.execution_time.as_millis();
                        match (r.columns.is_empty(), r.rows.len()) {
                            (true, _) => format!("{} rows affected · {ms} ms", r.rows_affected),
                            (false, n) if r.truncated() => format!(
                                "first {n} of {} rows · add a LIMIT, or Export for all · {ms} ms",
                                r.rows_affected
                            ),
                            (false, 1) => format!("1 row · {ms} ms"),
                            (false, n) => format!("{n} rows · {ms} ms"),
                        }
                    })
                };
                if summary.is_none() && error_message.is_none() && !*is_running {
                    return div().size_full().child(editor).into_any_element();
                }
                let header = h_flex()
                    .h(px(28.0))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .bg(t.bg_sidebar)
                    .border_y_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_secondary)
                            .child("Results"),
                    )
                    .map(|h| match (error_message, *is_running) {
                        (_, true) => {
                            h.child(div().text_xs().text_color(t.text_muted).child("Running…"))
                        }
                        (Some(err), _) => h.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(t.danger)
                                .truncate()
                                .child(err.clone()),
                        ),
                        (None, _) => {
                            let text = plan
                                .as_ref()
                                .map(|p| plan_view::summary(p))
                                .or_else(|| summary.clone())
                                .unwrap_or_default();
                            h.child(div().text_xs().text_color(t.text_muted).child(text))
                        }
                    })
                    .child(div().flex_1())
                    .child(
                        zbutton("results-expand")
                            .h(px(22.0))
                            .px_2()
                            .icon(if self.results_expanded {
                                IconName::Minimize2
                            } else {
                                IconName::Maximize2
                            })
                            .ghost()
                            .xsmall()
                            .tooltip(if self.results_expanded {
                                "Show the editor"
                            } else {
                                "Hide the editor"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.results_expanded = !this.results_expanded;
                                cx.notify();
                            })),
                    )
                    .child(
                        zbutton("results-export")
                            .h(px(22.0))
                            .px_2()
                            .icon(IconName::Download)
                            .label("Export")
                            .ghost()
                            .xsmall()
                            .tooltip("Export (⇧⌘E)")
                            .on_click(cx.listener(|this, _, _, cx| this.open_export(cx))),
                    );
                let body = match plan
                    .as_ref()
                    .filter(|_| error_message.is_none() && !*is_running)
                {
                    Some(p) => plan_view::render(p, &t),
                    None => grid.into_any_element(),
                };
                let results = v_flex()
                    .size_full()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(body));
                if self.results_expanded {
                    return results.into_any_element();
                }
                v_resizable("query-split")
                    .on_resize(layout::remember("query-split"))
                    .child(
                        resizable_panel()
                            .size(layout::size("query-split", 0, 260.0))
                            .size_range(px(80.0)..px(1200.0))
                            .child(editor),
                    )
                    .child(resizable_panel().child(results))
                    .into_any_element()
            }
            Tab::Table { view, .. } if *view != table_detail::TableView::Data => v_flex()
                .size_full()
                .child(
                    h_flex()
                        .h(px(BAR_H))
                        .w_full()
                        .px_3()
                        .border_b_1()
                        .border_color(t.border)
                        .items_center()
                        .child(self.render_table_view_switch(*view, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_table_detail(tab, cx)),
                )
                .into_any_element(),
            Tab::Table { filter, view, .. } => {
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .h(px(BAR_H))
                            .w_full()
                            .px_3()
                            .gap_2()
                            .border_b_1()
                            .border_color(t.border)
                            .items_center()
                            .child(self.render_table_view_switch(*view, cx))
                            .child(div().w_px().h(px(16.0)).bg(t.border))
                            // Single-line inputs propagate Enter: apply the filter
                            .on_action(cx.listener(
                                |this, _: &gpui::component::input::Enter, window, cx| {
                                    this.refresh_active_tab(window, cx);
                                },
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text_muted)
                                    .child("WHERE"),
                            )
                            .child(div().flex_1().child(Input::new(filter).appearance(false)))
                            .child(
                                zbutton("filter-clear-btn")
                                    .pad_xs()
                                    .label("Clear")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_filter(window, cx)
                                    })),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(grid))
                    .into_any_element()
            }
        }
    }

    /// Nothing open: the everyday starting points, Zed-style, plus recent tables
    fn render_launcher(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let row = |id: ElementId, icon: IconName, label: SharedString, hint: &'static str| {
            h_flex()
                .id(id)
                .h(px(32.0))
                .px_3()
                .gap_3()
                .items_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(t.bg_hover))
                .child(Icon::new(icon).small().text_color(t.text_muted))
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(t.text_primary)
                        .truncate()
                        .child(label),
                )
                .child(div().text_xs().text_color(t.text_muted).child(hint))
        };
        let start = v_flex()
            .gap_0p5()
            .child(
                row(
                    "l-new".into(),
                    IconName::SquareTerminal,
                    "New query".into(),
                    "⌘N",
                )
                .on_click(cx.listener(|this, _, window, cx| this.new_query(&NewQuery, window, cx))),
            )
            .child(
                row(
                    "l-search".into(),
                    IconName::Search,
                    "Search tables & commands".into(),
                    "⌘K",
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_command_bar(&ToggleCommandBar, window, cx)
                })),
            )
            .child(
                row(
                    "l-open".into(),
                    IconName::FolderOpen,
                    "Open file".into(),
                    "⌘O",
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_sql_file(window, cx))),
            )
            .child(
                row(
                    "l-settings".into(),
                    IconName::Settings2,
                    "Settings".into(),
                    "⌘,",
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
            );
        let recent: Vec<(String, String)> =
            self.state.recent_tables.iter().take(5).cloned().collect();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(380.0))
                    .gap_5()
                    .child(
                        v_flex()
                            .px_3()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary)
                                    .child(
                                        self.state.active_connection.as_ref().map_or_else(
                                            || "Quarry".to_string(),
                                            |c| c.name.clone(),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child("Pick a table from the sidebar, or start here"),
                            ),
                    )
                    .child(start)
                    .when(!recent.is_empty(), |d| {
                        d.child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .px_3()
                                        .pb_1()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text_muted)
                                        .child("Recent"),
                                )
                                .children(recent.into_iter().enumerate().map(
                                    |(i, (schema, table))| {
                                        row(
                                            ("l-recent", i).into(),
                                            IconName::Table,
                                            table.clone().into(),
                                            "",
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.open_pin(
                                                    pins::Pin::Table {
                                                        schema: schema.clone(),
                                                        table: table.clone(),
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            }),
                                        )
                                    },
                                )),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "Run on PROD?" for statements that change data
    fn render_prod_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let statements = self.pending_prod_run.as_ref()?;
        let t = &self.theme;
        let name = self
            .state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let count = statements
            .iter()
            .filter(|s| quarry_core::session::is_mutation(s))
            .count();
        Some(
            modal_backdrop("prod-backdrop", dismiss_on_click)
                .child(
                    modal_card("prod-card", 460.0, t)
                        .p_4()
                        .gap_3()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text_primary).child(format!("Change data on {name} (PROD)?")))
                        .child(div().text_xs().text_color(t.text_secondary).child(format!(
                            "{count} of these statements change data or schema. Turn this check off with \"confirm_on_prod\" in settings."
                        )))
                        .children(statements.iter().take(5).map(|s| {
                            div().text_xs().font_family(cx.theme().mono_font_family.clone()).text_color(t.text_muted).truncate().child(s.clone())
                        }))
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(zbutton("prod-no").pad_sm().label("Cancel").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                                    this.pending_prod_run = None;
                                    cx.notify();
                                })))
                                .child(zbutton("prod-yes").pad_sm().label("Run on PROD").danger().small().on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(statements) = this.pending_prod_run.take() {
                                        this.execute_statements_now(statements, window, cx);
                                    }
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Returns descriptions of any pending/unsaved changes across grids, files, and transactions
    pub fn unsaved_changes(&self, cx: &App) -> Vec<String> {
        let mut changes = Vec::new();

        // 1. Data grid pending changes (cell edits, row inserts, row deletes)
        for tab in &self.state.tabs {
            if let Tab::Table {
                table_name, grid, ..
            } = tab
            {
                let pending = grid.read(cx).delegate().pending.total_count();
                if pending > 0 {
                    changes.push(format!(
                        "{pending} uncommitted {} in '{table_name}'",
                        if pending == 1 { "edit" } else { "edits" }
                    ));
                }
            }
        }
        for grid in self.panel.grids() {
            let pending = grid.read(cx).delegate().pending.total_count();
            if pending > 0 {
                changes.push(format!(
                    "{pending} uncommitted {} in query results",
                    if pending == 1 { "edit" } else { "edits" }
                ));
            }
        }

        // 2. Unsaved query tabs (file-backed or scratch with edits)
        for tab in &self.state.tabs {
            if let Tab::Query {
                title,
                editor,
                file,
                saved_text,
                ..
            } = tab
            {
                let text = editor.read(cx).value();
                if text.as_ref() != saved_text.as_str() {
                    if let Some(f) = file {
                        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or(title);
                        changes.push(format!("Unsaved file '{name}'"));
                    } else if !text.trim().is_empty() {
                        changes.push(format!("Unsaved query in '{title}'"));
                    }
                }
            }
        }

        // 3. Active uncommitted transaction
        if self
            .current_connection(cx)
            .0
            .is_some_and(|s| s.in_transaction())
        {
            changes.push("Open database transaction (will be rolled back)".into());
        }

        // 4. Running queries
        for tab in &self.state.tabs {
            if let Tab::Query {
                title, is_running, ..
            } = tab
            {
                if *is_running {
                    changes.push(format!("Running query in '{title}'"));
                }
            }
        }

        changes
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        !self.unsaved_changes(cx).is_empty()
    }

    fn handle_close_window(
        &mut self,
        _: &CloseWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.force_close || !self.has_unsaved_changes(cx) {
            window.remove_window();
        } else {
            self.confirm_close_open = true;
            cx.notify();
        }
    }

    fn handle_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        if self.force_close || !self.has_unsaved_changes(cx) {
            cx.quit();
        } else {
            self.confirm_close_open = true;
            cx.notify();
        }
    }

    /// "Discard unsaved changes?" modal when closing window
    fn render_close_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirm_close_open {
            return None;
        }
        let changes = self.unsaved_changes(cx);
        let t = &self.theme;
        let count = changes.len();
        Some(
            modal_backdrop("close-confirm-backdrop", dismiss_on_click)
                .child(
                    modal_card("close-confirm-card", 460.0, t)
                        .p_4()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .child("Discard unsaved changes?"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_secondary)
                                .child(if count == 1 {
                                    "You have unsaved changes that will be lost if you close the window:"
                                } else {
                                    "You have multiple unsaved changes that will be lost if you close the window:"
                                }),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .children(changes.into_iter().take(6).map(|item| {
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().size(px(4.0)).rounded_full().bg(t.text_muted))
                                        .child(div().text_xs().text_color(t.text_primary).truncate().child(item))
                                })),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    zbutton("close-confirm-cancel")
                                        .pad_sm()
                                        .label("Cancel")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_close_open = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    zbutton("close-confirm-discard")
                                        .pad_sm()
                                        .label("Close Window")
                                        .danger()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, _cx| {
                                            this.confirm_close_open = false;
                                            this.force_close = true;
                                            window.remove_window();
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
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
                            .gap_2()
                            .child(
                                zbutton("onboard-demo").pad_sm()
                                    .icon(IconName::Database)
                                    .label("Open the demo database")
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| this.open_demo(window, cx))),
                            )
                            .child(
                                zbutton("onboard-form").pad_sm()
                                    .icon(IconName::Settings2)
                                    .label("More options")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| this.open_add_conn(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Short-lived confirmation in the corner; also kept in Output
    fn toast(&mut self, msg: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let msg = msg.into();
        self.state.messages.push(if error {
            format!("Error: {msg}")
        } else {
            msg.clone()
        });
        self.state.toast_message = Some((msg, error));
        self.toast_reveal = None;
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            this.update(cx, |this, cx| {
                this.state.toast_message = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The connection the active tab uses: a table tab's own, otherwise the active one
    fn current_connection(
        &self,
        cx: &App,
    ) -> (Option<Arc<dyn DatabaseSession>>, Option<ConnectionProfile>) {
        match self.state.active_tab() {
            Some(Tab::Table {
                grid, connection, ..
            }) => (
                grid.read(cx)
                    .delegate()
                    .source
                    .as_ref()
                    .map(|s| s.session.clone()),
                connection.clone(),
            ),
            _ => (
                self.state.session.clone(),
                self.state.active_connection.clone(),
            ),
        }
    }

    fn open_query_tab(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_query_tab_with(sql, None, window, cx);
    }

    pub fn open_query_tab_with(
        &mut self,
        sql: &str,
        file: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = self
            .state
            .tabs
            .iter()
            .filter(|t| matches!(t, Tab::Query { .. }))
            .count()
            + 1;
        let language = if file
            .as_ref()
            .is_some_and(|f| f.extension().is_some_and(|e| e == "json"))
        {
            "json"
        } else {
            "sql"
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(self.settings.line_numbers)
                .soft_wrap(self.settings.soft_wrap)
                .tab_size(gpui::base::input::TabSize { tab_size: self.settings.tab_size.max(1), hard_tabs: false })
                .placeholder("Write SQL here. ⌘↵ runs the selection or the statement at the cursor; ⌥↵ shows actions.")
                .default_value(sql.to_string())
        });
        let completions = std::rc::Rc::new(completion::SchemaCompletions(
            self.state.completion_catalog.clone(),
        ));
        let me = cx.entity().downgrade();
        editor.update(cx, |e, cx| {
            e.lsp_mut().completion_provider = self.settings.autocomplete.then_some(completions);
            e.lsp_mut().hover_provider = Some(std::rc::Rc::new(completion::SchemaHover(
                self.state.completion_catalog.clone(),
            )));
            e.lsp_mut()
                .code_action_providers
                .push(std::rc::Rc::new(completion::SqlActions(
                    self.state.completion_catalog.clone(),
                    self.state.completion_dialect.clone(),
                )));
            e.lsp_mut().definition_provider = Some(std::rc::Rc::new(completion::TableDefinitions(
                self.state.completion_catalog.clone(),
            )));
            // ⌘-click lands here: open the table instead of following a URL. Deferred, since the editor is mid-update.
            let me = me.clone();
            e.lsp_mut().show_document = Some(std::rc::Rc::new(move |params, window, cx| {
                let Some((schema, table)) = completion::table_link(&params.uri) else {
                    return false;
                };
                let me = me.clone();
                window.defer(cx, move |window, cx| {
                    me.update(cx, |this, cx| {
                        this.open_pin(pins::Pin::Table { schema, table }, window, cx)
                    })
                    .ok();
                });
                true
            }));
            e.focus(window, cx);
        });
        let id = format!("q-{}", uuid::Uuid::new_v4());
        // Every edit schedules a session save, so unsaved queries survive a restart (Q9)
        completion::lint_editor(&editor, &self.state.completion_catalog, cx);
        let sub = cx.subscribe(&editor, |this, editor, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                completion::lint_editor(&editor, &this.state.completion_catalog, cx);
                let mentioned =
                    quarry_sql::mentioned_unloaded(&editor.read(cx).value(), &this.state.catalog);
                this.load_schemas(mentioned, cx);
                this.schedule_session_save(cx);
                cx.notify();
            }
        });
        self.tab_subscriptions.insert(format!("{id}:editor"), sub);
        self.tab_subscriptions.insert(
            format!("{id}:statement"),
            self.track_current_statement(&editor, cx),
        );
        let grid = self.new_grid(&id, None, window, cx);
        let title = file
            .as_ref()
            .and_then(|f| f.file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("query {}", n));
        self.state.tabs.push(Tab::Query {
            id,
            title,
            editor,
            grid,
            file,
            saved_text: sql.to_string(),
            connection_id: self.state.active_connection.as_ref().map(|c| c.id.clone()),
            schema: None,
            row_limit: None,
            is_running: false,
            error_message: None,
            plan: None,
        });
        self.state.active_tab_index = self.state.tabs.len() - 1;
        self.schedule_session_save(cx);
    }

    pub fn open_table_tab(
        &mut self,
        table_name: &str,
        schema_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.note_recent_table(schema_name, table_name);
        if let Some(idx) = self.state.find_tab(table_name) {
            self.state.active_tab_index = idx;
            return;
        }
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("WHERE condition, e.g. id > 1"));
        let Some(session) = self.state.session.clone() else {
            return;
        };
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
        let (columns, primary_keys, foreign_keys) = schema
            .map(|t| {
                (
                    t.columns.clone(),
                    t.primary_keys.clone(),
                    t.foreign_keys.clone(),
                )
            })
            .unwrap_or_default();
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
                foreign_keys,
                page_size: self.settings.rows_per_page,
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
            view: Default::default(),
            detail: None,
        });
        self.state.active_tab_index = self.state.tabs.len() - 1;
        self.schedule_session_save(cx);
    }

    /// Opens the referenced table filtered to the row a foreign-key cell points at (G5)
    fn follow_foreign_key(
        &mut self,
        (schema, table, filter): (String, String, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_table_tab(&table, &schema, window, cx);
        if let Some(Tab::Table { filter: input, .. }) = self.state.active_tab() {
            input
                .clone()
                .update(cx, |i, cx| i.set_value(filter, window, cx));
        }
        self.refresh_active_tab(window, cx);
        cx.notify();
    }

    /// Tints the statement ⌘↵ would run, and keeps the gutter ▶ next to it
    fn track_current_statement(
        &self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        use gpui::component::input::{
            RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
        };
        let tint = Hsla::from(self.theme.accent).opacity(0.07);
        let decorations: std::rc::Rc<std::cell::RefCell<Option<RangeDecorationCollection>>> =
            Default::default();
        let mut last = (0..0, None);
        cx.observe(editor, move |this, editor, cx| {
            let e = editor.read(cx);
            let sql = e.value();
            let span = if this.settings.highlight_current_statement {
                quarry_sql::find_statement_at_offset_with(
                    &sql,
                    e.cursor().min(sql.len()),
                    this.state.dialect().split_rules(),
                )
                .map_or(0..0, |s| s.start..s.end)
            } else {
                0..0
            };
            let now = (span.clone(), e.cursor_layout().map(|(b, _)| b.origin.y));
            if now == last {
                return;
            }
            let span_changed = now.0 != last.0;
            last = now;
            if span_changed {
                let deco = if span.is_empty() {
                    vec![]
                } else {
                    vec![RangeDecoration::new(span)
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(tint)]
                };
                // Deferred: this can fire while the editor is mid-update, and updating it then aborts
                let (editor, decorations) = (editor.clone(), decorations.clone());
                cx.defer(move |cx| {
                    let mut d = decorations.borrow_mut();
                    match d.as_ref() {
                        // `set` updates the editor itself, so it must not run inside `editor.update`
                        Some(c) => c.set(deco, cx),
                        None => {
                            *d = Some(editor.update(cx, |e, cx| {
                                e.create_range_decorations_collection(deco, cx)
                            }))
                        }
                    }
                });
            }
            cx.notify(); // move the gutter ▶
        })
    }

    fn open_grid_menu(
        &mut self,
        grid: Entity<TableState<ResultSet>>,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = grid.update(cx, |t, cx| {
            t.set_selected_cell(row, col, cx);
            t.focus_handle(cx)
        });
        let menu = gpui::component::menu::PopupMenu::build(window, cx, |menu, _, cx| {
            grid::grid_menu(menu.action_context(focus), grid.read(cx).delegate())
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        self.grid_menu = Some((window.mouse_position(), menu, sub));
        cx.notify();
    }

    fn new_grid(
        &mut self,
        tab_id: &str,
        source: Option<TableSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ResultSet>> {
        let mut rs = ResultSet::new(&self.theme, source);
        rs.set_prefs(&self.settings.null_display, self.settings.max_column_width);
        let grid = cx.new(|cx| {
            TableState::new(rs, window, cx)
                .cell_selectable(true)
                .col_resizable(true)
                .col_selectable(false)
        });
        // Double-click edits the cell (read-only grids explain why they can't)
        let sub = cx.subscribe_in(&grid, window, |this, grid, ev: &TableEvent, window, cx| {
            match *ev {
                TableEvent::DoubleClickedCell(row, col) => grid.update(cx, |t, cx| {
                    t.delegate_mut().start_edit(row, col, None, window, cx)
                }),
                // Cell-selection mode handles right-clicks itself, so open the menu here
                TableEvent::RightClickedCell(row, col) => {
                    this.open_grid_menu(grid.clone(), row, col, window, cx)
                }
                // The value editor follows the selection
                TableEvent::SelectCell(row, col) => grid.update(cx, |t, cx| {
                    t.delegate_mut().selected_cell = Some((row, col));
                    if t.delegate().inspector_open() {
                        t.delegate_mut().inspect(row, col, window, cx);
                    }
                }),
                TableEvent::ClearSelection => grid.update(cx, |t, _| {
                    t.delegate_mut().selected_cell = None;
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

    fn execute_statements_now(
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

    /// Other connected databases to compare the active one with
    fn open_compare_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let choices: Vec<(String, bool, String)> = self
            .state
            .saved_connections
            .iter()
            .filter(|c| {
                Some(&c.id) != active.as_ref() && self.state.open_sessions.contains_key(&c.id)
            })
            .map(|c| (c.name.clone(), false, c.id.clone()))
            .collect();
        if active.is_none() || choices.is_empty() {
            return self.toast(
                "Connect to both databases first: the one to compare and the one to change",
                true,
                cx,
            );
        }
        self.open_choice_menu(
            choices,
            |this, id, cx| {
                this.state.pending_compare = Some(id);
                cx.notify();
            },
            window,
            cx,
        );
    }

    /// The script that makes `target_id` match the active connection's schema, in a tab on the target
    /// Loads schemas a big database's catalog skipped, once each; the sidebar, completion and
    /// squiggles pick them up when they arrive
    fn load_schemas(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let (Some(session), Some(id)) = (
            self.state.session.clone(),
            self.state.active_connection.as_ref().map(|c| c.id.clone()),
        ) else {
            return;
        };
        for schema in names {
            let key = format!("{id}/{schema}");
            if self.state.catalog.is_loaded(&schema)
                || !self.state.loading_schemas.insert(key.clone())
            {
                continue;
            }
            let (session, id) = (session.clone(), id.clone());
            run_db_bg(
                cx,
                async move {
                    session
                        .introspect_schema(&schema)
                        .await
                        .map(|t| (schema, t))
                },
                move |this, res, cx| {
                    this.state.loading_schemas.remove(&key);
                    match res {
                        Ok((schema, tables)) => {
                            if let Some(open) = this.state.open_sessions.get_mut(&id) {
                                open.1.add_schema(&schema, tables.clone());
                            }
                            // Switched away meanwhile: it's kept for when this connection comes back
                            if this
                                .state
                                .active_connection
                                .as_ref()
                                .is_some_and(|c| c.id == id)
                            {
                                let mut catalog = this.state.catalog.clone();
                                catalog.add_schema(&schema, tables);
                                this.state.set_catalog(catalog);
                            }
                        }
                        Err(e) => this.toast(format!("Couldn't load the schema: {e}"), true, cx),
                    }
                },
            );
        }
    }

    fn compare_schema(&mut self, target_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target_session, target_catalog)) =
            self.state.open_sessions.get(target_id).cloned()
        else {
            return;
        };
        // A diff needs every table: big databases' partial catalogs are read in full first
        if !self.state.catalog.unloaded.is_empty() || !target_catalog.unloaded.is_empty() {
            let Some(source_session) = self.state.session.clone() else {
                return;
            };
            let target_id = target_id.to_string();
            self.toast("Reading both schemas in full…".to_string(), false, cx);
            return run_db(
                window,
                cx,
                async move {
                    (
                        source_session.introspect().await,
                        target_session.introspect().await,
                    )
                },
                move |this, res, window, cx| match res {
                    (Ok(source), Ok(target)) => {
                        this.finish_compare(&target_id, &source, &target, window, cx)
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        this.toast(format!("Couldn't read the schemas: {e}"), true, cx)
                    }
                },
            );
        }
        let source_catalog = self.state.catalog.clone();
        self.finish_compare(target_id, &source_catalog, &target_catalog, window, cx);
    }

    fn finish_compare(
        &mut self,
        target_id: &str,
        source_catalog: &quarry_core::SchemaCatalog,
        target_catalog: &quarry_core::SchemaCatalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_session) = self.state.open_sessions.get(target_id).map(|o| o.0.clone())
        else {
            return;
        };
        let source = self
            .state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let target = self
            .state
            .saved_connections
            .iter()
            .find(|c| c.id == target_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let Some(script) =
            quarry_core::diff::diff(source_catalog, target_catalog, target_session.dialect())
        else {
            return self.toast(
                format!("{source} and {target} have the same tables and columns"),
                false,
                cx,
            );
        };
        let header = format!("-- Makes {target} match {source} (tables and columns; views, indexes and constraints aren't compared).\n-- Review before running: this tab runs on {target}.\n\n");
        self.open_query_tab(&(header + &script), window, cx);
        // The tab runs on the target; switching to it switches the connection
        if let Some(Tab::Query { connection_id, .. }) = self.state.tabs.last_mut() {
            *connection_id = Some(target_id.to_string());
        }
    }

    /// Fetches the extra objects of every expanded schema that doesn't have them yet
    fn load_schema_objects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else {
            return;
        };
        let missing: Vec<String> = self
            .state
            .catalog
            .schemas
            .iter()
            .filter(|s| {
                self.state.expanded_schemas.contains(*s)
                    && !self.state.schema_objects.contains_key(*s)
            })
            .cloned()
            .collect();
        for schema in missing {
            self.state.schema_objects.insert(schema.clone(), None);
            let (session, gen) = (session.clone(), self.state.catalog_gen);
            let name = schema.clone();
            run_db(
                window,
                cx,
                async move { session.objects(&name).await },
                move |this, res, _, cx| {
                    // A reconnect or refresh since: these belong to the old catalog
                    if this.state.catalog_gen != gen {
                        return;
                    }
                    let objects = res.unwrap_or_else(|e| {
                        this.state
                            .messages
                            .push(format!("Couldn't list objects in {schema}: {e}"));
                        Vec::new()
                    });
                    this.state.schema_objects.insert(schema, Some(objects));
                    cx.notify();
                },
            );
        }
    }

    /// Opens a function, trigger, sequence or type's definition in a new query tab
    fn open_object_definition(
        &mut self,
        schema: String,
        obj: quarry_core::schema::SchemaObject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.state.session.clone() else {
            return;
        };
        run_db(
            window,
            cx,
            async move { session.object_ddl(&schema, &obj).await },
            |this, res, window, cx| match res {
                Ok(ddl) => this.open_query_tab(&ddl, window, cx),
                Err(e) => this.toast(format!("Couldn't load the definition: {e}"), true, cx),
            },
        );
    }

    /// Re-reads the schema tree of the active connection
    fn refresh_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else {
            return;
        };
        let Some(id) = self.state.active_connection.as_ref().map(|c| c.id.clone()) else {
            return;
        };
        run_db(
            window,
            cx,
            async move { session.load_catalog().await },
            move |this, res, _, cx| match res {
                Ok(catalog) => {
                    // Kept for when this connection is switched back to
                    if let Some(open) = this.state.open_sessions.get_mut(&id) {
                        open.1 = catalog.clone();
                    }
                    // Switched away mid-refresh: don't put this schema under another connection
                    if this
                        .state
                        .active_connection
                        .as_ref()
                        .is_some_and(|c| c.id == id)
                    {
                        this.state.set_catalog(catalog);
                        // A big database's catalog comes back partial: reload the schemas that were open
                        let open: Vec<String> =
                            this.state.expanded_schemas.iter().cloned().collect();
                        this.load_schemas(open, cx);
                        this.toast("Schema refreshed".to_string(), false, cx);
                    }
                }
                Err(e) => this.toast(format!("Schema refresh failed: {e}"), true, cx),
            },
        );
    }

    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Tab::Table { filter, .. }) = self.state.active_tab() {
            filter
                .clone()
                .update(cx, |f, cx| f.set_value("", window, cx));
        }
        self.refresh_active_tab(window, cx);
    }

    pub fn refocus_root(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    // ── Actions ─────────────────────────────────────────────

    fn toggle_command_bar(
        &mut self,
        _: &ToggleCommandBar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.command_bar_open = !self.state.command_bar_open;
        self.state.usages = None; // ⌘K is the normal palette, not the last Find Usages
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

    fn review_changes(&mut self, _: &ReviewChanges, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state.active_tab(), Some(Tab::Query { .. })) {
            return self.save_query(false, window, cx);
        }
        let review_data = match self.state.active_tab() {
            Some(Tab::Table {
                table_name, grid, ..
            }) => {
                let rs = grid.read(cx).delegate();
                rs.source
                    .as_ref()
                    .filter(|_| !rs.pending.is_empty())
                    .map(|src| {
                        (
                            table_name.clone(),
                            rs.pending
                                .generate_sql(&src.qualified_name(), src.dialect()),
                        )
                    })
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

    fn commit_tx(&mut self, _: &CommitTx, window: &mut Window, cx: &mut Context<Self>) {
        self.end_tx(true, window, cx);
    }

    fn rollback_tx(&mut self, _: &RollbackTx, window: &mut Window, cx: &mut Context<Self>) {
        self.end_tx(false, window, cx);
    }

    fn end_tx(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.state.sidebar_visible = !self.state.sidebar_visible;
        cx.notify();
    }

    fn toggle_bottom_panel(
        &mut self,
        _: &ToggleBottomPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.bottom_panel_open = !self.state.bottom_panel_open;
        cx.notify();
    }

    fn new_query(&mut self, _: &NewQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.open_query_tab("", window, cx);
        cx.notify();
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        // ⌘W with nothing left to close closes the window, as everywhere else on macOS
        if self.state.tabs.is_empty() {
            return self.handle_close_window(&CloseWindow, window, cx);
        }
        self.close_tab_at(self.state.active_tab_index, cx);
        // The closed tab may have held focus; keep shortcuts (like ⇧⌘T) working
        self.refocus_root(window, cx);
    }

    fn close_tab_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.state.close_tab(idx) else {
            return;
        };
        // Kept (with its grid, editor and pending edits) so ⇧⌘T can bring it back (U9)
        let id = tab.id().to_string();
        let subs = [id.clone(), format!("{id}:editor")]
            .into_iter()
            .filter_map(|k| self.tab_subscriptions.remove(&k).map(|s| (k, s)))
            .collect();
        self.closed_tabs.push((tab, subs));
        if self.closed_tabs.len() > 20 {
            self.closed_tabs.remove(0);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// The split pane's tab, if it's set and isn't the active one
    fn split_tab(&self) -> Option<Tab> {
        let id = self.split_id.as_deref()?;
        let active = self.state.active_tab()?.id();
        (id != active)
            .then(|| self.state.tabs.iter().find(|t| t.id() == id).cloned())
            .flatten()
    }

    /// ⌘\: put the current tab in the right pane next to the previous one, or close the split
    fn toggle_split(&mut self, cx: &mut Context<Self>) {
        if self.split_tab().is_some() {
            self.split_id = None;
        } else if self.state.tabs.len() > 1 {
            let ix = self.state.active_tab_index;
            self.split_id = Some(self.state.tabs[ix].id().to_string());
            self.state.active_tab_index = if ix == 0 { 1 } else { ix - 1 };
        } else {
            self.toast("Open another tab to split the view", false, cx);
        }
        cx.notify();
    }

    /// Clicking into the right pane makes it the active tab; the old one moves right
    fn swap_split(&mut self, cx: &mut Context<Self>) {
        let (Some(split), Some(active)) = (
            self.split_id.clone(),
            self.state.active_tab().map(|t| t.id().to_string()),
        ) else {
            return;
        };
        if let Some(ix) = self.state.tabs.iter().position(|t| t.id() == split) {
            self.state.active_tab_index = ix;
            self.split_id = Some(active);
            cx.notify();
        }
    }

    fn confirm_command_bar_in_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.state.active_tab().map(|t| t.id().to_string());
        self.confirm_command_bar(window, cx);
        let opened = self.state.active_tab().map(|t| t.id().to_string());
        if let (Some(prev), Some(new)) = (previous, opened) {
            if prev != new {
                self.split_id = Some(new);
                if let Some(ix) = self.state.tabs.iter().position(|t| t.id() == prev) {
                    self.state.active_tab_index = ix;
                }
            }
        }
        cx.notify();
    }

    /// Pins the current table or query tab to its connection (or unpins it)
    fn pin_current_tab(&mut self, cx: &mut Context<Self>) {
        let pin = match self.state.active_tab() {
            Some(Tab::Table {
                schema_name,
                table_name,
                ..
            }) => pins::Pin::Table {
                schema: schema_name.clone(),
                table: table_name.clone(),
            },
            Some(Tab::Query { title, editor, .. }) => pins::Pin::Query {
                title: title.clone(),
                sql: editor.read(cx).value().to_string(),
            },
            None => return,
        };
        self.toggle_pin(pin, cx);
    }

    fn toggle_pin(&mut self, pin: pins::Pin, cx: &mut Context<Self>) {
        let Some(conn) = self.current_connection(cx).1.map(|c| c.id) else {
            return self.toast("Connect to a database to pin things to it", true, cx);
        };
        let label = pin.label().to_string();
        let pinned = pins::toggle(&mut self.state.pins, &conn, pin);
        pins::save(&self.state.pins);
        self.toast(
            format!("{} {}", if pinned { "Pinned" } else { "Unpinned" }, label),
            false,
            cx,
        );
    }

    fn open_pin(&mut self, pin: pins::Pin, window: &mut Window, cx: &mut Context<Self>) {
        match pin {
            pins::Pin::Table { schema, table } => {
                self.open_table_tab(&table, &schema, window, cx);
                self.refresh_active_tab(window, cx);
            }
            pins::Pin::Query { sql, title } => {
                self.open_query_tab(&sql, window, cx);
                if let Some(Tab::Query { title: t, .. }) = self.state.tabs.last_mut() {
                    *t = title;
                }
            }
        }
        cx.notify();
    }

    /// ⌃1–⌃9 (U5): jump to the Nth saved connection; the sidebar swaps to its tree and pins
    fn switch_space(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.state.saved_connections.get(n).map(|c| c.id.clone()) {
            self.connect_saved(id, window, cx);
        }
    }

    /// ⌘⇧P (P1): the command bar in command mode
    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.command_bar_open = true;
        self.state.usages = None;
        self.state.command_bar_selected_index = 0;
        self.command_input.update(cx, |input, cx| {
            input.set_value(">", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn reopen_closed_tab(&mut self, _: &ReopenClosedTab, _: &mut Window, cx: &mut Context<Self>) {
        let Some((tab, subs)) = self.closed_tabs.pop() else {
            return;
        };
        self.tab_subscriptions.extend(subs);
        self.state.tabs.push(tab);
        self.state.active_tab_index = self.state.tabs.len() - 1;
        self.save_session(cx);
        cx.notify();
    }

    /// Escape: close the topmost overlay
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_switcher.take().is_some() {
            cx.notify();
            return;
        }
        if self.state.renaming_query.is_some() {
            return self.finish_rename(None, window, cx);
        }
        if std::mem::take(&mut self.confirm_close_open) {
            cx.notify();
            return;
        }
        if std::mem::take(&mut self.settings_open) {
            cx.notify();
            return;
        }
        if self.pending_prod_run.take().is_some() || self.panel.confirm_kill.take().is_some() {
            cx.notify();
            return;
        }
        if self.params.take().is_some() {
            self.refocus_root(window, cx);
            cx.notify();
            return;
        }
        if self.export.open {
            self.export.open = false;
            cx.notify();
            return;
        }
        if self.import.open {
            // A running import is cancelled from its own button, not Esc
            self.import.open = self.import_busy();
            cx.notify();
            return;
        }
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
        self.state.command_bar_selected_index =
            self.state.command_bar_selected_index.saturating_sub(1);
        cx.notify();
    }

    fn menu_down(&mut self, _: &MenuDown, _: &mut Window, cx: &mut Context<Self>) {
        let count =
            CommandBar::get_candidates(&self.state, &self.command_input.read(cx).value()).len();
        if self.state.command_bar_selected_index + 1 < count {
            self.state.command_bar_selected_index += 1;
        }
        cx.notify();
    }

    fn confirm_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let candidates =
            CommandBar::get_candidates(&self.state, &self.command_input.read(cx).value());
        let idx = self
            .state
            .command_bar_selected_index
            .min(candidates.len().saturating_sub(1));
        if let Some(item) = candidates.get(idx) {
            self.handle_command_action(item.id.clone(), window, cx);
        }
    }

    /// Handles actions triggered from the command bar
    pub fn handle_command_action(
        &mut self,
        action_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.command_bar_open = false;
        self.refocus_root(window, cx);
        if let Some(ix) = action_id
            .strip_prefix("usage:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            return self.open_usage(ix, window, cx);
        }
        self.state.usages = None;

        if let Some((schema, table)) = action_id
            .strip_prefix("table:")
            .and_then(|t| t.split_once('\u{1f}'))
        {
            self.open_table_tab(table, schema, window, cx);
            self.refresh_active_tab(window, cx);
        } else if let Some((view, rest)) = action_id
            .strip_prefix("table-")
            .and_then(|r| r.split_once(':'))
        {
            if let Some((schema, table)) = rest.split_once('\u{1f}') {
                self.open_pin(
                    pins::Pin::Table {
                        schema: schema.into(),
                        table: table.into(),
                    },
                    window,
                    cx,
                );
                let view = if view == "ddl" {
                    table_detail::TableView::Ddl
                } else {
                    table_detail::TableView::Structure
                };
                self.set_table_view(view, window, cx);
            }
        } else if let Some(ix) = action_id
            .strip_prefix("pin:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let pin = self
                .state
                .active_connection
                .as_ref()
                .and_then(|c| self.state.pins.get(&c.id))
                .and_then(|p| p.get(ix))
                .cloned();
            if let Some(pin) = pin {
                self.open_pin(pin, window, cx);
            }
        } else if let Some(id) = action_id.strip_prefix("connection:") {
            self.connect_saved(id.to_string(), window, cx);
        } else if let Some(ix) = action_id
            .strip_prefix("history:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            if let Some(sql) = self.state.history.get(ix).map(|h| h.sql.clone()) {
                self.open_query_tab(&sql, window, cx);
            }
        } else if let Some(query) = action_id.strip_prefix("run:") {
            self.open_query_tab(query, window, cx);
            self.refresh_active_tab(window, cx);
        } else if action_id == "action:maximize" {
            self.state.toggle_maximize();
        } else if action_id == "action:new_query" {
            self.new_query(&NewQuery, window, cx);
        } else if action_id == "action:review" {
            self.review_changes(&ReviewChanges, window, cx);
        } else if action_id == "action:compare_schema" {
            self.open_compare_menu(window, cx);
        } else if action_id == "action:add_conn" {
            self.open_add_conn(window, cx);
        } else if action_id == "action:toggle_sidebar" {
            self.toggle_sidebar(&ToggleSidebar, window, cx);
        } else if action_id == "action:explain" {
            self.start_run(RunMode::Explain, window, cx);
        } else if action_id == "action:explain_analyze" {
            self.start_run(RunMode::ExplainAnalyze, window, cx);
        } else if action_id == "action:settings" {
            self.open_settings(cx);
        } else if action_id == "action:keymap" {
            self.open_config_file(true, window, cx);
        } else if action_id == "action:pin" {
            self.pin_current_tab(cx);
        } else if action_id == "action:split" {
            self.toggle_split(cx);
        } else if action_id == "action:reopen_tab" {
            self.reopen_closed_tab(&ReopenClosedTab, window, cx);
        } else if action_id == "action:format" {
            self.format_query(window, cx);
        } else if action_id == "action:open_file" {
            self.open_sql_file(window, cx);
        } else if action_id == "action:save_file" {
            self.save_query(false, window, cx);
        } else if action_id == "action:export" {
            self.open_export(cx);
        } else if action_id == "action:import" {
            self.open_import(window, cx);
        } else if action_id == "action:dump" {
            self.dump_database(window, cx);
        } else if action_id == "action:theme" {
            self.toggle_theme(&ToggleTheme, window, cx);
        } else if action_id == "action:increase_font" {
            self.increase_font_size(&IncreaseFontSize, window, cx);
        } else if action_id == "action:decrease_font" {
            self.decrease_font_size(&DecreaseFontSize, window, cx);
        } else if action_id == "action:reset_font" {
            self.reset_font_size(&ResetFontSize, window, cx);
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

    /// The Add Connection modal, filled in from a saved connection; saving keeps its id (and keychain password)
    pub fn open_edit_conn(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self
            .state
            .saved_connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
        else {
            return;
        };
        self.state.editing_conn = Some(p.id.clone());
        self.state.add_conn_tab = 0;
        let driver = p.driver.clone();
        self.state.add_conn_env = p.env;
        self.state.add_conn_read_only = p.read_only;
        self.state.add_conn_ssh = p.ssh.is_some();
        self.state.add_conn_feedback = None;
        let path = p.driver().map_or_else(
            || p.path_or_url.clone(),
            |d| d.take_secret(&p.path_or_url).0,
        );
        if let Some(d) = quarry_core::driver(&driver) {
            self.set_conn_driver(d.id(), window, cx);
        }
        self.set_conn_inputs(Some(&p.name), &path, window, cx);
        let ssh = p.ssh.as_ref();
        let values = [
            ssh.map(|s| {
                if s.port == 22 {
                    s.host.clone()
                } else {
                    format!("{}:{}", s.host, s.port)
                }
            }),
            ssh.map(|s| s.user.clone()),
            ssh.and_then(|s| s.key_path.clone()),
        ];
        for (input, v) in self.ssh_inputs.clone().iter().zip(values) {
            input.update(cx, |i, cx| i.set_value(v.unwrap_or_default(), window, cx));
        }
        let timeout = p
            .statement_timeout_secs
            .map(|t| t.to_string())
            .unwrap_or_default();
        self.timeout_input
            .update(cx, |i, cx| i.set_value(timeout, window, cx));
        self.state.add_conn_modal_open = true;
        self.conn_name_input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn open_add_conn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.editing_conn = None;
        self.state.add_conn_feedback = None;
        self.state.add_conn_tab = 0;
        self.state.add_conn_read_only = false;
        self.state.add_conn_ssh = false;
        // A fresh form: nothing carried over from the last connection added
        self.set_conn_driver(self.state.add_conn_driver, window, cx);
        self.set_conn_inputs(Some(""), "", window, cx);
        self.state.add_conn_modal_open = true;
        self.conn_name_input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn set_conn_inputs(
        &mut self,
        name: Option<&str>,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(name) = name {
            self.conn_name_input
                .update(cx, |i, cx| i.set_value(name.to_string(), window, cx));
        }
        self.conn_path_input
            .update(cx, |i, cx| i.set_value(path.to_string(), window, cx));
        self.fill_conn_fields(window, cx);
    }

    fn conn_driver(&self) -> &'static dyn quarry_core::Driver {
        quarry_core::driver(self.state.add_conn_driver).unwrap_or(quarry_core::drivers()[0])
    }

    /// Picks the form's driver and builds one input per field it asks for
    fn set_conn_driver(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.add_conn_driver == id && !self.conn_field_inputs.is_empty() {
            return;
        }
        self.state.add_conn_driver = id;
        let fields = self.conn_driver().fields();
        self.conn_field_inputs = fields
            .iter()
            .map(|f| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(f.placeholder)
                        .masked(f.secret)
                })
            })
            .collect();
        // Field edits write the URL, unless the URL is the one writing the fields
        self.conn_field_subs = self
            .conn_field_inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) && !this.conn_syncing {
                        let values: Vec<String> = this
                            .conn_field_inputs
                            .iter()
                            .map(|i| i.read(cx).value().to_string())
                            .collect();
                        let url = this
                            .conn_driver()
                            .join(&values, &this.conn_path_input.read(cx).value());
                        this.conn_syncing = true;
                        this.conn_path_input
                            .update(cx, |i, cx| i.set_value(url, window, cx));
                        this.conn_syncing = false;
                    }
                })
            })
            .collect();
    }

    /// URL → the driver's fields, unless the fields are the ones writing the URL
    fn fill_conn_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.conn_syncing {
            return;
        }
        let url = self.conn_path_input.read(cx).value().to_string();
        let values = self.conn_driver().split(&url);
        self.conn_syncing = true;
        for (i, v) in self.conn_field_inputs.clone().iter().zip(values) {
            i.update(cx, |i, cx| i.set_value(v, window, cx));
        }
        self.conn_syncing = false;
    }

    /// Connects to a pasted or dropped address, if a driver recognizes it
    pub fn connect_address(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match ConnectionProfile::from_address(text) {
            Some(profile) => self.connect_profile(profile, window, cx),
            None => self.toast("No database driver recognizes that address", true, cx),
        }
    }

    fn profile_from_modal(&self, cx: &App) -> ConnectionProfile {
        let name = self.conn_name_input.read(cx).value().trim().to_string();
        let path_or_url = self.conn_path_input.read(cx).value().trim().to_string();
        ConnectionProfile {
            id: self
                .state
                .editing_conn
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: if name.is_empty() {
                self.conn_driver().display_address(&path_or_url)
            } else {
                name
            },
            driver: self.state.add_conn_driver.to_string(),
            path_or_url,
            env: self.state.add_conn_env,
            read_only: self.state.add_conn_read_only,
            ssh: self.ssh_from_modal(cx),
            statement_timeout_secs: self.timeout_input.read(cx).value().trim().parse().ok(),
        }
    }

    fn ssh_from_modal(&self, cx: &App) -> Option<ssh::SshConfig> {
        if !self.state.add_conn_ssh
            || self
                .conn_driver()
                .tunnel_target(self.conn_driver().sample())
                .is_none()
        {
            return None;
        }
        let [host, user, key] = &self.ssh_inputs;
        let host = host.read(cx).value().trim().to_string();
        let (host, port) = match host.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap_or(22)),
            _ => (host, 22),
        };
        let user = user.read(cx).value().trim().to_string();
        let key = key.read(cx).value().trim().to_string();
        (!host.is_empty()).then(|| ssh::SshConfig {
            host,
            port,
            user: if user.is_empty() {
                std::env::var("USER").unwrap_or_default()
            } else {
                user
            },
            key_path: (!key.is_empty()).then_some(key),
        })
    }

    /// Connects to a new database using values in the Add Connection modal
    pub fn connect_add_conn_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.profile_from_modal(cx);
        // Editing: replace the saved profile in place and reconnect with the new settings
        if let Some(id) = self.state.editing_conn.take() {
            self.state.disconnect(&id);
            if let Some(slot) = self.state.saved_connections.iter_mut().find(|c| c.id == id) {
                *slot = profile.clone();
            }
            if let Err(e) = connections::save(&self.state.saved_connections) {
                self.toast(format!("Could not save connection: {e}"), true, cx);
            }
        }
        self.state.add_conn_feedback = Some(("Connecting…".to_string(), false));
        self.connect_profile(profile, window, cx);
    }

    /// Query tabs keep their own connection. Landing on a query tab switches to its connection;
    /// switching connections (sidebar, picker) while on one moves the tab to the new connection.
    fn sync_tab_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let Some(Tab::Query {
            id, connection_id, ..
        }) = self.state.active_tab()
        else {
            self.synced_tab = None;
            return;
        };
        let (id, wanted) = (id.clone(), connection_id.clone());
        if self.synced_tab.as_ref() != Some(&id) {
            self.synced_tab = Some(id);
            if let Some(w) = wanted.filter(|w| current.as_ref() != Some(w)) {
                if self.state.activate_open_session(&w) {
                    return;
                }
                // Not open (e.g. after a restart): reconnect, and leave the tab alone until that lands
                if let Some(profile) = self
                    .state
                    .saved_connections
                    .iter()
                    .find(|c| c.id == w)
                    .cloned()
                {
                    self.connecting_for_tab = Some(w);
                    self.connect_profile(profile, window, cx);
                    return;
                }
            }
        }
        if let Some(pending) = &self.connecting_for_tab {
            if current.as_ref() != Some(pending) {
                return;
            }
            self.connecting_for_tab = None;
        }
        if let Some(Tab::Query { connection_id, .. }) =
            self.state.tabs.get_mut(self.state.active_tab_index)
        {
            *connection_id = current;
        }
    }

    /// The editor's connection picker: every saved connection, grouped as in the sidebar
    fn open_connection_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut conns = self.state.saved_connections.clone();
        conns.sort_by_key(|c| c.env as u8);
        let current = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let me = cx.entity().downgrade();
        let menu = gpui::component::menu::PopupMenu::build(window, cx, |mut menu, _, _| {
            for c in conns {
                let me = me.clone();
                let label = format!("{}  ·  {}", c.name, c.env.label());
                menu = menu.item(
                    gpui::component::menu::PopupMenuItem::new(label)
                        .checked(current.as_ref() == Some(&c.id))
                        .on_click(move |_, window, cx| {
                            me.update(cx, |this, cx| this.use_connection(c.id.clone(), window, cx))
                                .ok();
                        }),
                );
            }
            menu
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        // Just under the toolbar row, so the menu doesn't cover the picker
        self.grid_menu = Some((
            window.mouse_position() + point(px(0.0), px(14.0)),
            menu,
            sub,
        ));
        cx.notify();
    }

    /// Shows a popup menu of (label, checked, value) under the mouse; picking one calls `apply` with its value
    fn open_choice_menu<V: Clone + 'static>(
        &mut self,
        choices: Vec<(String, bool, V)>,
        apply: fn(&mut Self, V, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity().downgrade();
        let menu = gpui::component::menu::PopupMenu::build(window, cx, |mut menu, _, _| {
            for (label, checked, value) in choices {
                let me = me.clone();
                menu = menu.item(
                    gpui::component::menu::PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            me.update(cx, |this, cx| {
                                apply(this, value.clone(), cx);
                                this.schedule_session_save(cx);
                                cx.notify();
                            })
                            .ok();
                        }),
                );
            }
            menu
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        self.grid_menu = Some((
            window.mouse_position() + point(px(0.0), px(14.0)),
            menu,
            sub,
        ));
        cx.notify();
    }

    fn open_schema_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query {
            schema: current, ..
        }) = self.state.active_tab()
        else {
            return;
        };
        let current = current.clone();
        let mut choices = vec![("Default schema".to_string(), current.is_none(), None)];
        choices.extend(
            self.state
                .catalog
                .schemas
                .iter()
                .map(|s| (s.clone(), current.as_ref() == Some(s), Some(s.clone()))),
        );
        self.open_choice_menu(
            choices,
            |this, v, _| {
                if let Some(Tab::Query { schema, .. }) =
                    this.state.tabs.get_mut(this.state.active_tab_index)
                {
                    *schema = v;
                }
            },
            window,
            cx,
        );
    }

    fn open_limit_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query {
            row_limit: current, ..
        }) = self.state.active_tab()
        else {
            return;
        };
        let current = *current;
        let choices = [None, Some(100), Some(500), Some(1000), Some(5000)]
            .into_iter()
            .map(|n| {
                (
                    n.map_or("No limit".to_string(), |n| format!("{n} rows")),
                    current == n,
                    n,
                )
            })
            .collect();
        self.open_choice_menu(
            choices,
            |this, v, _| {
                if let Some(Tab::Query { row_limit, .. }) =
                    this.state.tabs.get_mut(this.state.active_tab_index)
                {
                    *row_limit = v;
                }
            },
            window,
            cx,
        );
    }

    /// Makes a saved connection the active one, connecting if it isn't open yet
    fn use_connection(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .state
            .active_connection
            .as_ref()
            .is_some_and(|c| c.id == id)
        {
            return;
        }
        if !self.state.activate_open_session(&id) {
            if let Some(profile) = self
                .state
                .saved_connections
                .iter()
                .find(|c| c.id == id)
                .cloned()
            {
                self.connect_profile(profile, window, cx);
            }
        }
        cx.notify();
    }

    /// Sidebar click: switch to a saved connection, reusing its session if it's already open
    fn connect_saved(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Clicking the open connection folds or unfolds its tree
        if self
            .state
            .active_connection
            .as_ref()
            .is_some_and(|c| c.id == id)
        {
            if !self.state.collapsed_connections.remove(&id) {
                self.state.collapsed_connections.insert(id);
            }
            cx.notify();
            return;
        }
        self.state.collapsed_connections.remove(&id);
        if self.state.activate_open_session(&id) {
            cx.notify();
            return;
        }
        if let Some(profile) = self
            .state
            .saved_connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
        {
            self.connect_profile(profile, window, cx);
        }
    }

    fn connect_profile(
        &mut self,
        mut profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if profile.statement_timeout_secs.is_none()
            && self.settings.default_statement_timeout_secs > 0
        {
            profile.statement_timeout_secs = Some(self.settings.default_statement_timeout_secs);
        }
        let name = profile.name.clone();
        self.state.messages.push(format!("Connecting to {}…", name));
        cx.notify();
        let log = self.state.activity_log.clone();
        run_db(
            window,
            cx,
            async move {
                open_session(&profile, log)
                    .await
                    .map(|(s, c, t)| (profile, s, c, t))
            },
            move |this, res, window, cx| match res {
                Ok((profile, session, catalog, tunnel)) => {
                    this.toast(format!("Connected to {}", profile.name), false, cx);
                    session
                        .tx()
                        .set_manual(this.settings.default_transaction_mode == "manual");
                    this.state
                        .apply_connection(profile, session, catalog, tunnel);
                    this.state.add_conn_modal_open = false;
                    this.state.add_conn_feedback = None;
                    this.refocus_root(window, cx);
                }
                Err(e) => {
                    this.toast(format!("Could not connect to {}: {}", name, e), true, cx);
                    this.connecting_for_tab = None;
                    this.state.add_conn_feedback = Some((e, true));
                }
            },
        );
    }

    /// Fetches Oracle's client library (only when asked from the connect form), then connects again
    fn download_oracle_client(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.add_conn_feedback = Some(("Downloading Oracle client…".to_string(), false));
        cx.notify();
        run_db(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(quarry_core::oracle::client::install)
                    .await
                    .map_err(|e| e.to_string())?
            },
            |this, res, window, cx| match res {
                Ok(_) => this.connect_add_conn_modal(window, cx),
                Err(e) => {
                    this.state.add_conn_feedback = Some((e, true));
                    cx.notify();
                }
            },
        );
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
                let (session, _, _tunnel) = open_session(&profile, Default::default()).await?;
                session.ping().await.map_err(|e| e.to_string())
            },
            |this, res, _window, _cx| {
                this.state.add_conn_feedback = Some(match res {
                    Ok(dur) => (
                        format!(
                            "Connection successful ({:.2}ms)",
                            dur.as_secs_f64() * 1000.0
                        ),
                        false,
                    ),
                    Err(e) => (format!("Failed: {}", e), true),
                });
            },
        );
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

impl Render for QuarryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_theme(window, cx);
        if self.applied_settings.as_ref() != Some(&self.settings) {
            self.apply_settings_to_views(window, cx);
        }
        // The schema changed (connect, switch, refresh): squiggles must reflect the new one
        self.load_schema_objects(window, cx);
        if let Some(id) = self.state.pending_compare.take() {
            self.compare_schema(&id, window, cx);
        }
        if self.state.linted_gen != self.state.catalog_gen {
            self.state.linted_gen = self.state.catalog_gen;
            let editors: Vec<_> = self
                .state
                .tabs
                .iter()
                .filter_map(|t| match t {
                    Tab::Query { editor, .. } => Some(editor.clone()),
                    _ => None,
                })
                .collect();
            let catalog = self.state.completion_catalog.clone();
            cx.defer(move |cx| {
                editors
                    .iter()
                    .for_each(|e| completion::lint_editor(e, &catalog, cx))
            });
        }
        self.sync_tab_connection(window, cx);
        self.reopen_pending_table_tabs(window, cx);
        self.track_tab_mru(window, cx);
        let bottom_panel = self.render_bottom_panel(window, cx);
        let kill_confirm = self.render_kill_confirm(cx);
        let tab_switcher = self.render_tab_switcher(cx);
        let settings_modal = self.render_settings_modal(cx);
        let export_dialog = self.render_export_dialog(cx);
        let import_wizard = self.render_import_wizard(cx);
        let params_dialog = self.render_params_dialog(cx);
        let peek = self.render_peek(cx);
        let prod_confirm = self.render_prod_confirm(cx);
        let close_confirm = self.render_close_confirm(cx);
        // While ⌃Tab is held, the main area previews the highlighted tab; letting go makes it active
        let previewed = self
            .tab_switcher
            .and_then(|i| self.tab_mru.get(i))
            .and_then(|id| self.state.tabs.iter().find(|t| t.id() == id));
        let active_tab = previewed.or(self.state.active_tab()).cloned();
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
            .child(div().flex_1())
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
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_command_bar(&ToggleCommandBar, window, cx)
                    }))
                    .child(
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(t.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Search tables, run SQL, or type > for commands"),
                    )
                    .child(div().text_xs().text_color(t.text_muted).child("⌘K")),
            )
            .child(div().flex_1());

        // Tabs: icon + title + close; hidden when nothing is open
        let tab_strip = (!self.state.tabs.is_empty()).then(|| {
            h_flex()
                .h(px(BAR_H))
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
                    // Table tabs keep their connection: mark staging/prod so a risky tab is obvious at a glance
                    let env_color = match tab {
                        Tab::Table { connection: Some(c), .. } => match c.env {
                            Environment::Prod => Some(t.danger),
                            Environment::Staging => Some(t.warning),
                            Environment::Local => None,
                        },
                        _ => None,
                    };
                    let table_obj = match tab {
                        Tab::Table { schema_name, table_name, .. } => {
                            Some(object_menu::DbObject::Table { schema: schema_name.clone(), table: table_name.clone() })
                        }
                        _ => None,
                    };
                    h_flex()
                        .id(("tab-select", idx))
                        .h(px(CONTROL_H))
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
                        .when_some(env_color, |d, c| d.border_b_2().border_color(c))
                        .when_some(table_obj, |d, obj| {
                            d.on_mouse_down(MouseButton::Right, cx.listener(move |this, _, window, cx| {
                                this.state.active_tab_index = idx;
                                this.open_object_menu(obj.clone(), window, cx);
                            }))
                        })
                        .child(Icon::new(icon).xsmall().text_color(if is_active { t.accent } else { t.text_muted }))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if is_active { t.text_primary } else { t.text_secondary })
                                .child(tab.title().to_string()),
                        )
                        .when(
                            // Unsaved: a new tab with text in it, or a file that differs from disk
                            matches!(tab, Tab::Query { editor, saved_text, .. } if editor.read(cx).value().as_ref() != saved_text.as_str()),
                            |d| d.child(div().size(px(6.0)).rounded_full().bg(t.text_secondary)),
                        )
                        .child(
                            zbutton(("tab-close", idx))
                                .icon(IconName::X)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    // The tab under this button selects itself on click; without this it would
                                    // re-select the closed slot (a neighbour, or nothing when it was the last)
                                    cx.stop_propagation();
                                    this.close_tab_at(idx, cx);
                                    this.refocus_root(window, cx);
                                })),
                        )
                }))
                // Tabs sit on the strip's bottom edge; centre the + on a tab's height so it lines up with them
                .child(
                    h_flex().h(px(CONTROL_H)).px_1().items_center().child(
                        zbutton("tab-add-btn")
                            .icon(IconName::Plus)
                            .ghost()
                            .xsmall()
                            .tooltip("New query (⌘N)")
                            .on_click(cx.listener(|this, _, window, cx| this.new_query(&NewQuery, window, cx))),
                    ),
                )
        });

        let center: AnyElement = match active_tab.as_ref() {
            None if self.state.session.is_none() => self.render_onboarding(cx),
            None => self.render_launcher(cx),
            Some(tab) => match self.split_tab().filter(|_| self.state.tabs.len() > 1) {
                // Split view (U8): the active tab on the left, the split tab on the right
                Some(split) => {
                    let left = self.render_tab_view(tab, cx);
                    let right = self.render_tab_view(&split, cx);
                    h_resizable("split-view")
                        .child(resizable_panel().child(left))
                        .child(
                            resizable_panel().child(
                                div()
                                    .size_full()
                                    .border_l_1()
                                    .border_color(t.border)
                                    // Working in the right pane makes it the active one
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.swap_split(cx)),
                                    )
                                    .child(right),
                            ),
                        )
                        .into_any_element()
                }
                None => self.render_tab_view(tab, cx),
            },
        };

        let sidebar = Sidebar::render(
            &self.state,
            &self.theme,
            cx,
            |this, table_name, schema_name, window, cx| {
                this.peek = None;
                this.peek_task = None;
                this.open_table_tab(&table_name, &schema_name, window, cx);
                this.refresh_active_tab(window, cx);
                cx.notify();
            },
            |this, window, cx| this.open_add_conn(window, cx),
            |this, schema_name, _window, cx| {
                this.state.toggle_schema(&schema_name);
                if this.state.expanded_schemas.contains(&schema_name) {
                    this.load_schemas(vec![schema_name], cx);
                }
                cx.notify();
            },
            |this, id, window, cx| this.connect_saved(id, window, cx),
            |this, schema, table, hovered, window, cx| {
                this.peek_hover(schema, table, hovered, window, cx)
            },
            |this, pin, window, cx| this.open_pin(pin, window, cx),
            |this, pin, _, cx| this.toggle_pin(pin, cx),
            |this, obj, window, cx| {
                this.peek = None;
                this.open_object_menu(obj, window, cx)
            },
            |this, path, window, cx| this.open_query_file(path, window, cx),
            |this, key, _, cx| {
                if !this.state.expanded_object_groups.remove(&key) {
                    this.state.expanded_object_groups.insert(key);
                }
                cx.notify();
            },
            |this, schema, obj, window, cx| this.open_object_definition(schema, obj, window, cx),
        );

        div()
            .id("quarry-root")
            .font_family(theme::UI_FONT)
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
            .on_action(
                cx.listener(|this, _: &TabSwitcherNext, _, cx| this.step_tab_switcher(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TabSwitcherPrev, _, cx| this.step_tab_switcher(false, cx)),
            )
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, window, cx| {
                if !ev.modifiers.control {
                    this.commit_tab_switcher(window, cx);
                }
            }))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(cx.listener(Self::handle_close_window))
            .on_action(cx.listener(Self::handle_quit))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .on_action(cx.listener(|this, _: &ToggleMaximize, _, cx| {
                this.state.toggle_maximize();
                cx.notify();
            }))
            .on_action(cx.listener(Self::toggle_tx_mode))
            .on_action(cx.listener(Self::commit_tx))
            .on_action(cx.listener(Self::rollback_tx))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::increase_font_size))
            .on_action(cx.listener(Self::decrease_font_size))
            .on_action(cx.listener(Self::reset_font_size))
            .on_action(cx.listener(Self::reopen_closed_tab))
            .on_action(cx.listener(|this, _: &PinCurrentTab, _, cx| this.pin_current_tab(cx)))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                this.open_command_palette(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SwitchSpace1, window, cx| this.switch_space(0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace2, window, cx| this.switch_space(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace3, window, cx| this.switch_space(2, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace4, window, cx| this.switch_space(3, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace5, window, cx| this.switch_space(4, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace6, window, cx| this.switch_space(5, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace7, window, cx| this.switch_space(6, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace8, window, cx| this.switch_space(7, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace9, window, cx| this.switch_space(8, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleSplit, _, cx| this.toggle_split(cx)))
            .on_action(cx.listener(|this, _: &OpenInSplit, window, cx| {
                this.confirm_command_bar_in_split(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExportResults, _, cx| this.open_export(cx)))
            .on_action(cx.listener(|this, _: &Explain, window, cx| {
                this.start_run(RunMode::Explain, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatSql, window, cx| this.format_query(window, cx)))
            .on_action(
                cx.listener(|this, _: &SaveFileAs, window, cx| this.save_query(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| this.open_sql_file(window, cx)))
            .on_action(cx.listener(|this, _: &ExplainAnalyze, window, cx| {
                this.start_run(RunMode::ExplainAnalyze, window, cx)
            }))
            .on_action(cx.listener(Self::menu_up))
            .on_action(cx.listener(Self::menu_down))
            // Drop a SQLite file anywhere to open it (C3, U11)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.connect_address(&path.to_string_lossy(), window, cx);
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg_app)
            .text_color(t.text_primary)
            .when_some(
                current_conn
                    .as_ref()
                    .filter(|c| c.env != Environment::Local),
                |this, c| this.child(div().h(px(2.0)).w_full().bg(env_color(c.env))),
            )
            .child(titlebar)
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("main-split")
                        .on_resize(layout::remember("main-split"))
                        .child(
                            resizable_panel()
                                .size(layout::size("main-split", 0, 240.0))
                                .size_range(px(160.0)..px(480.0))
                                .visible(self.state.sidebar_visible)
                                .child(sidebar),
                        )
                        .child(
                            resizable_panel().child(
                                v_resizable("content-split")
                                    .on_resize(layout::remember("content-split"))
                                    .child(resizable_panel().child(
                                        v_flex().size_full().children(tab_strip).child(center),
                                    ))
                                    .child(
                                        resizable_panel()
                                            .size(layout::size("content-split", 1, 280.0))
                                            .size_range(px(120.0)..px(900.0))
                                            .visible(self.state.bottom_panel_open)
                                            .child(bottom_panel),
                                    ),
                            ),
                        ),
                ),
            )
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
            .children(tab_switcher)
            .children(export_dialog)
            .children(import_wizard)
            .children(params_dialog)
            .children(peek)
            .children(prod_confirm)
            .children(settings_modal)
            .children(close_confirm)
            .when_some(
                self.grid_menu.as_ref().map(|(p, m, _)| (*p, m.clone())),
                |this, (pos, menu)| {
                    this.child(
                        deferred(anchored().position(pos).snap_to_window().child(menu))
                            .with_priority(1),
                    )
                },
            )
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
                        .flex()
                        .gap_3()
                        .child(msg)
                        .when_some(self.toast_reveal.clone(), |this, path| {
                            this.child(
                                div()
                                    .id("toast-reveal")
                                    .text_color(t.accent)
                                    .cursor_pointer()
                                    .on_click(move |_, _, _| export_dialog::reveal(&path))
                                    .child(if cfg!(target_os = "macos") {
                                        "Reveal in Finder"
                                    } else {
                                        "Show in folder"
                                    }),
                            )
                        }),
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
                    [
                        &self.ssh_inputs[0],
                        &self.ssh_inputs[1],
                        &self.ssh_inputs[2],
                    ],
                    &self.conn_field_inputs,
                    &self.timeout_input,
                    |this, _window, cx| {
                        this.state.add_conn_ssh = !this.state.add_conn_ssh;
                        cx.notify();
                    },
                    |this, id, window, cx| {
                        // Only replace an empty field or another driver's sample, never something typed
                        let current = this.conn_path_input.read(cx).value().trim().to_string();
                        this.set_conn_driver(id, window, cx);
                        if current.is_empty()
                            || quarry_core::drivers().iter().any(|d| d.sample() == current)
                        {
                            this.set_conn_inputs(None, this.conn_driver().sample(), window, cx);
                        } else {
                            this.fill_conn_fields(window, cx);
                        }
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
                    |this, tab, _window, cx| {
                        this.state.add_conn_tab = tab;
                        cx.notify();
                    },
                    |this, _window, cx| {
                        this.state.add_conn_read_only = !this.state.add_conn_read_only;
                        cx.notify();
                    },
                    |this, window, cx| this.download_oracle_client(window, cx),
                ))
            })
    }
}

/// The macOS menu bar; its items also make the standard shortcuts (⌘Q, ⌘H, ⌘M…) work
fn set_app_menus(cx: &mut App) {
    use gpui::component::input;
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    // Closing the last window quits, like other single-window macOS apps
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    cx.set_menus([
        Menu {
            name: "Quarry".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Hide Quarry", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Quarry", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Zoom In", IncreaseFontSize),
                MenuItem::action("Zoom Out", DecreaseFontSize),
                MenuItem::separator(),
                MenuItem::action("Actual Size", ResetFontSize),
            ],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Close Window", CloseWindow),
            ],
            disabled: false,
        },
    ]);
}

fn main() {
    gpui::application()
        .with_assets(gpui::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui::init(cx);
            // Quarry's own palette is dark-only for now; keep gpui-kit widgets in sync
            gpui::component::theme::Theme::change(
                gpui::component::theme::ThemeMode::Dark,
                None,
                cx,
            );
            theme::load_fonts(cx);
            bind_keys(cx);
            grid::bind_keys(cx);
            set_app_menus(cx);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: None, // Custom unified titlebar with cross-platform OS traffic light offset
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.0), px(12.0))),
                }),
                ..Default::default()
            };

            gpui::open_window(options, cx, |window, cx| {
                let entity = cx.new(|cx| {
                    // Starts with the onboarding input focused, so a pasted URL just works
                    QuarryWindow::new(window, cx)
                });
                let weak = entity.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    weak.update(cx, |this, cx| {
                        if this.force_close {
                            true
                        } else if this.has_unsaved_changes(cx) {
                            this.confirm_close_open = true;
                            cx.notify();
                            false
                        } else {
                            true
                        }
                    })
                    .unwrap_or(true)
                });
                entity
            })
            .unwrap();
        });
}

fn os_is_dark(window: &Window) -> bool {
    matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}
