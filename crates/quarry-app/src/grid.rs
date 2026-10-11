// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::runtime::run_db;
use crate::theme::Theme;
use crate::ui::*;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Editor, EditorState, Input, InputEvent, InputState, Position};
use gpui::component::menu::PopupMenu;
use gpui::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui::component::tooltip::Tooltip;
use gpui::component::{ActiveTheme, Disableable, Icon, Sizable};
use gpui::*;
use quarry_core::edits::{PendingChanges, PendingRowInsert, RowKey};
use quarry_core::schema::{ColumnSchema, ColumnType};
use quarry_core::session::{DatabaseSession, QueryResult};
use quarry_core::value::DbValue;
use std::sync::Arc;

actions!(
    grid,
    [
        EditCell,
        CancelEdit,
        NextCell,
        PrevCell,
        AddRow,
        DuplicateRow,
        DeleteRow,
        SetNull,
        SetDefault,
        ToggleValueEditor,
        ApplyValue,
        SubmitChanges,
        RevertChanges,
        CopyValue,
        CopyRowTsv,
        CopyRowCsv,
        CopyRowJson,
        CopyRowInsert,
        CopyAllTsv,
        CopyAllCsv,
        CopyAllJson,
        CopyAllInsert,
        HideColumn,
        ShowAllColumns,
        FollowForeignKey
    ]
);

/// DataGrip-style grid keys; scoped to the grid so ⌘N/⌘↵ keep their global meaning elsewhere.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", EditCell, Some("DataTable")),
        KeyBinding::new("f2", EditCell, Some("DataTable")),
        KeyBinding::new("escape", CancelEdit, Some("DataTable > Input")),
        KeyBinding::new("tab", NextCell, Some("DataTable > Input")),
        KeyBinding::new("shift-tab", PrevCell, Some("DataTable > Input")),
        KeyBinding::new("secondary-enter", SubmitChanges, Some("DataTable > Input")),
        KeyBinding::new("secondary-alt-n", SetNull, Some("DataTable > Input")),
        KeyBinding::new("shift-enter", ToggleValueEditor, Some("DataTable > Input")),
        KeyBinding::new("secondary-n", AddRow, Some("DataTable")),
        KeyBinding::new("secondary-d", DuplicateRow, Some("DataTable")),
        KeyBinding::new("secondary-backspace", DeleteRow, Some("DataTable")),
        KeyBinding::new("secondary-alt-n", SetNull, Some("DataTable")),
        // Not ⌥⌘D: macOS reserves it for showing/hiding the Dock
        KeyBinding::new("secondary-shift-d", SetDefault, Some("DataTable")),
        KeyBinding::new("shift-enter", ToggleValueEditor, Some("DataTable")),
        // Registered after the app's Input bindings, so ⌘↵ applies in the value editor instead of running a query
        KeyBinding::new("secondary-enter", ApplyValue, Some("ValueEditor > Input")),
        KeyBinding::new("secondary-enter", SubmitChanges, Some("DataTable")),
        KeyBinding::new("secondary-alt-z", RevertChanges, Some("DataTable")),
        KeyBinding::new("secondary-c", CopyValue, Some("DataTable")),
        KeyBinding::new("secondary-shift-c", CopyRowTsv, Some("DataTable")),
        KeyBinding::new("f4", FollowForeignKey, Some("DataTable")),
    ]);
}


/// Where a table tab's rows come from; enables paging, server-side sort and editing.
pub struct TableSource {
    pub session: Arc<dyn DatabaseSession>,
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnSchema>,
    pub primary_keys: Vec<String>,
    /// Why this table can't be edited, if it can't (E1, E10)
    pub read_only_reason: Option<&'static str>,
    /// Columns that reference other tables (G5)
    pub foreign_keys: Vec<quarry_core::schema::ForeignKeyRef>,
    /// Rows per page (settings: rows_per_page)
    pub page_size: usize,
    pub filter: String,
    pub order_by: Option<(String, bool)>, // (column, descending)
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl TableSource {
    pub fn dialect(&self) -> &'static dyn quarry_core::Dialect {
        self.session.dialect()
    }

    pub fn qualified_name(&self) -> String {
        self.dialect().qualified(&self.schema, &self.table)
    }

    /// The tab's query (filter + sort), without paging
    pub fn select_sql(&self) -> String {
        let mut sql = format!("SELECT * FROM {}", self.qualified_name());
        if !self.filter.trim().is_empty() {
            sql += &format!(" WHERE {}", self.filter);
        }
        if let Some((col, desc)) = &self.order_by {
            sql += &format!(" ORDER BY {}{}", self.dialect().quote_ident(col), if *desc { " DESC" } else { "" });
        }
        sql
    }

    fn page_sql(&self, offset: usize, limit: usize) -> String {
        // One extra row tells us whether another page exists
        self.dialect().limit(&self.select_sql(), limit as u64 + 1, offset as u64)
    }
}

struct Inspector {
    row: usize,
    col: usize,
    editor: Entity<EditorState>,
    initial: String,
}

struct CellEditor {
    row: usize,
    col: usize,
    display_col: usize,
    input: Entity<InputState>,
    initial: String,
    error: Option<String>,
    _sub: Subscription,
}

#[derive(Clone, Copy, PartialEq)]
enum RowState {
    Loaded,
    Inserted,
    Deleted,
}

/// One or more result sets plus pending edits; the model behind every grid.
pub struct ResultSet {
    /// One entry per row-returning statement; shown as sub-tabs when there are several
    pub results: Vec<QueryResult>,
    pub active: usize,
    pub pending: PendingChanges,
    pub source: Option<TableSource>,
    /// Last validation or commit problem, shown in the toolbar until the next change
    pub notice: Option<String>,
    editing: Option<CellEditor>,
    /// The currently selected cell (row, col)
    pub selected_cell: Option<(usize, usize)>,
    /// Side panel for long text and JSON ("value editor"); follows the selected cell
    inspector: Option<Inspector>,
    /// Display order of columns (actual indices); hidden columns are left out. `None` = natural order
    order: Option<Vec<usize>>,
    colors: CellColors,
    /// From settings: how NULL reads and the widest an auto-sized column gets
    null_label: SharedString,
    max_col_width: f32,
    /// The column being sorted (by name) and how; the header icon is rebuilt from this
    sort: Option<(String, ColumnSort)>,
    /// Server logs: rows whose value in this column is ERROR/FATAL/PANIC read in red
    pub level_column: Option<usize>,
    /// Right-click menu on a column header: where, the menu, and its dismiss subscription
    header_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Query results in their original order, so "default" can put them back
    unsorted: Option<Vec<Vec<DbValue>>>,
}

#[derive(Clone, Copy)]
struct CellColors {
    text: Rgba,
    muted: Rgba,
    token_bg: Rgba,
    success: Rgba,
    danger: Rgba,
    pending_bg: Rgba,
    inserted_bg: Rgba,
    deleted_bg: Rgba,
    accent: Rgba,
    editor_bg: Rgba,
    /// Text on a success/danger fill
    on_fill: Rgba,
    grid_line: Hsla,
}

impl From<&Theme> for CellColors {
    fn from(theme: &Theme) -> Self {
        Self {
            text: theme.text_primary,
            muted: theme.text_muted,
            token_bg: theme.bg_surface,
            success: theme.success,
            danger: theme.danger,
            pending_bg: theme.pending_cell_bg,
            inserted_bg: Theme::tint(theme.success, 0.15).into(),
            deleted_bg: Theme::tint(theme.danger, 0.15).into(),
            on_fill: theme.on_accent(),
            grid_line: theme.grid_line(),
            accent: theme.accent,
            editor_bg: if theme.is_dark { theme.bg_surface } else { theme.bg_app },
        }
    }
}

impl ResultSet {
    pub fn set_theme(&mut self, theme: &Theme) {
        self.colors = CellColors::from(theme);
    }

    pub fn set_prefs(&mut self, null_label: &str, max_col_width: f32) {
        self.null_label = null_label.to_string().into();
        self.max_col_width = max_col_width;
    }

    pub fn new(theme: &Theme, source: Option<TableSource>) -> Self {
        Self {
            results: Vec::new(),
            active: 0,
            pending: PendingChanges::new(),
            source,
            notice: None,
            editing: None,
            selected_cell: None,
            inspector: None,
            order: None,
            null_label: "NULL".into(),
            max_col_width: 420.0,
            sort: None,
            level_column: None,
            header_menu: None,
            unsorted: None,
            colors: CellColors::from(theme),
        }
    }

    pub fn result(&self) -> Option<&QueryResult> {
        self.results.get(self.active)
    }

    pub fn set_results(&mut self, results: Vec<QueryResult>) {
        // Keep the user's column order/hiding across reloads of the same shape
        let same_columns = self.result().map(|r| &r.columns) == results.first().map(|r| &r.columns);
        if !same_columns {
            self.order = None;
        }
        self.results = results;
        self.active = 0;
        self.editing = None;
        self.selected_cell = None;
        // New query results arrive unsorted; table tabs keep sorting on the server
        if self.source.is_none() {
            self.sort = None;
        }
        self.unsorted = None;
        self.inspector = None;
    }

    /// Switches between result sub-tabs
    pub fn set_active(&mut self, ix: usize) {
        self.active = ix;
        self.order = None;
        self.editing = None;
        self.selected_cell = None;
        self.inspector = None;
    }

    /// Actual column index for a displayed column
    pub fn actual(&self, display: usize) -> usize {
        self.order.as_ref().and_then(|o| o.get(display).copied()).unwrap_or(display)
    }

    /// Displayed columns, as actual indices, in display order
    fn visible(&self) -> Vec<usize> {
        self.order.clone().unwrap_or_else(|| (0..self.result().map_or(0, |r| r.columns.len())).collect())
    }

    pub fn hide_column(&mut self, display: usize) {
        let mut order = self.visible();
        if order.len() > 1 && display < order.len() {
            order.remove(display);
            self.order = Some(order);
        }
    }

    pub fn show_all_columns(&mut self) {
        self.order = None;
    }

    pub fn hidden_count(&self) -> usize {
        self.result().map_or(0, |r| r.columns.len()) - self.visible().len()
    }

    /// The table a foreign-key cell points at, and a WHERE condition for its row
    pub fn foreign_key_target(&self, row: usize, display: usize) -> Option<(String, String, String)> {
        let col = self.actual(display);
        let name = self.column_name(col)?;
        let src = self.source.as_ref()?;
        let fk = src.foreign_keys.iter().find(|f| f.from_column == name)?;
        let value = self.cell_value(row, col).0.filter(|v| !v.is_null())?;
        let filter = format!("{} = {}", src.dialect().quote_ident(&fk.to_column), src.dialect().literal(&value));
        Some((fk.to_schema.clone(), fk.to_table.clone(), filter))
    }

    /// Copies one row (or all rows) of visible columns in the given format
    pub fn copy_text(&self, row: Option<usize>, format: quarry_core::export::ExportFormat, delimiter: char) -> Option<String> {
        let r = self.result()?;
        let rows: Vec<Vec<DbValue>> = match row {
            Some(row) => vec![(0..r.columns.len()).map(|c| self.cell_value(row, c).0.unwrap_or(DbValue::Null)).collect()],
            None => r.rows.clone(),
        };
        let table = self.source.as_ref().map(|s| s.qualified_name()).unwrap_or_else(|| "query_result".into());
        let opts = quarry_core::export::ExportOptions {
            format,
            delimiter,
            header: row.is_none(),
            null_text: String::new(),
            columns: Some(self.visible()),
            table,
        };
        let mut buf = Vec::new();
        let mut ex = quarry_core::export::Exporter::begin(&mut buf, &r.columns, opts).ok()?;
        ex.rows(&rows).ok()?;
        ex.finish().ok()?;
        String::from_utf8(buf).ok().map(|s| s.trim_end().to_string())
    }

    pub fn read_only_reason(&self) -> Option<&'static str> {
        match &self.source {
            None => Some("Query results are read-only"),
            Some(src) => src.read_only_reason,
        }
    }

    fn loaded_len(&self) -> usize {
        self.result().map_or(0, |r| r.rows.len())
    }

    fn column_name(&self, col: usize) -> Option<&str> {
        self.result()?.columns.get(col).map(String::as_str)
    }

    fn column_schema(&self, col: usize) -> Option<&ColumnSchema> {
        let name = self.column_name(col)?;
        self.source.as_ref()?.columns.iter().find(|c| c.name == name)
    }

    /// Primary-key values of a loaded row, as loaded
    fn row_key(&self, row: usize) -> Option<RowKey> {
        let (src, r) = (self.source.as_ref()?, self.result()?);
        let values = r.rows.get(row)?;
        src.primary_keys
            .iter()
            .map(|pk| Some((pk.clone(), values.get(r.columns.iter().position(|c| c == pk)?)?.clone())))
            .collect()
    }

    /// Sorts by display column `col_ix`; `Default` restores the original order
    fn apply_sort(&mut self, col_ix: usize, sort: ColumnSort, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let col_ix = self.actual(col_ix);
        let Some(name) = self.result().and_then(|r| r.columns.get(col_ix)).cloned() else { return };
        self.sort = (sort != ColumnSort::Default).then(|| (name.clone(), sort));
        // Rebuild the headers from `self.sort` once this update is done, so the icon shows what's applied
        cx.defer_in(window, |table, _, cx| table.refresh(cx));

        // Tables sort on the server (the full table, not just the loaded page), like DataGrip
        if let Some(src) = self.source.as_mut() {
            src.order_by = match sort {
                ColumnSort::Ascending => Some((name, false)),
                ColumnSort::Descending => Some((name, true)),
                ColumnSort::Default => None,
            };
            self.fetch(Fetch::Reset, window, cx);
            return;
        }
        let Some(r) = self.results.get_mut(self.active) else { return };
        let original = self.unsorted.get_or_insert_with(|| r.rows.clone());
        match sort {
            ColumnSort::Ascending => r.rows.sort_by(|a, b| a[col_ix].cmp_val(&b[col_ix])),
            ColumnSort::Descending => r.rows.sort_by(|a, b| b[col_ix].cmp_val(&a[col_ix])),
            ColumnSort::Default => r.rows = std::mem::take(original),
        }
        if sort == ColumnSort::Default {
            self.unsorted = None;
        }
    }

    fn open_header_menu(&mut self, col: usize, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let name = self.column_name(self.actual(col)).unwrap_or_default().to_string();
        let grid = cx.entity().downgrade();
        let item = |label: &'static str, f: fn(&mut ResultSet, usize, &mut Window, &mut Context<TableState<ResultSet>>)| {
            let grid = grid.clone();
            gpui::component::menu::PopupMenuItem::new(label).on_click(move |_, window, cx| {
                grid.update(cx, |t, cx| {
                    f(t.delegate_mut(), col, window, cx);
                    t.refresh(cx);
                    cx.notify();
                })
                .ok();
            })
        };
        let can_hide = self.visible().len() > 1;
        let has_hidden = self.order.is_some();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(item("Sort Ascending", |rs, c, w, cx| rs.apply_sort(c, ColumnSort::Ascending, w, cx)))
                .item(item("Sort Descending", |rs, c, w, cx| rs.apply_sort(c, ColumnSort::Descending, w, cx)))
                .item(item("Clear Sort", |rs, c, w, cx| rs.apply_sort(c, ColumnSort::Default, w, cx)))
                .separator()
                .when(can_hide, |m| m.item(item("Hide Column", |rs, c, _, _| rs.hide_column(c))))
                .when(has_hidden, |m| m.item(item("Show All Columns", |rs, _, _, _| rs.show_all_columns())))
                .item(gpui::component::menu::PopupMenuItem::new("Copy Column Name").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
                }))
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |t, _, _: &DismissEvent, cx| {
            t.delegate_mut().header_menu = None;
            cx.notify();
        });
        self.header_menu = Some((pos, menu, sub));
        cx.notify();
    }

    fn is_error_row(&self, row: usize) -> bool {
        self.level_column.is_some_and(|col| {
            matches!(self.cell_value(row, col).0, Some(DbValue::Text(ref l)) if matches!(l.as_str(), "ERROR" | "FATAL" | "PANIC"))
        })
    }

    fn row_state(&self, row: usize) -> RowState {
        if row >= self.loaded_len() {
            RowState::Inserted
        } else if self.row_key(row).is_some_and(|k| self.pending.is_deleted(&k)) {
            RowState::Deleted
        } else {
            RowState::Loaded
        }
    }

    /// Value of a pending-insert cell; `None` = left to the column DEFAULT
    fn inserted_value(&self, row: usize, col: usize) -> Option<DbValue> {
        let name = self.column_name(col)?;
        if let Some(insert) = self.pending.row_inserts.get(row.checked_sub(self.loaded_len())?) {
            return insert.values.get(name).cloned();
        }
        None
    }

    /// The value a cell shows (pending edits win over loaded values) and whether it was edited
    fn cell_value(&self, row: usize, col: usize) -> (Option<DbValue>, bool) {
        if row >= self.loaded_len() {
            return (self.inserted_value(row, col), false);
        }
        let Some(original) = self.result().and_then(|r| r.rows.get(row)?.get(col).cloned()) else {
            return (None, false);
        };
        let edited = self
            .row_key(row)
            .zip(self.column_name(col))
            .and_then(|(k, name)| self.pending.cell_value(&k, name).cloned());
        match edited {
            Some(v) => (Some(v), true),
            None => (Some(original), false),
        }
    }

    fn set_value(&mut self, row: usize, col: usize, value: DbValue) {
        let Some(name) = self.column_name(col).map(str::to_string) else { return };
        let loaded = self.loaded_len();
        if row >= loaded {
            if let Some(insert) = self.pending.row_inserts.get_mut(row - loaded) {
                insert.values.insert(name, value);
            }
        } else if let (Some(key), Some(original)) =
            (self.row_key(row), self.result().and_then(|r| r.rows.get(row)?.get(col).cloned()))
        {
            self.pending.set_cell(key, &name, original, value);
        }
    }

    // ── Editing ─────────────────────────────────────────────

    /// Opens the in-cell editor. `seed` replaces the value (type-to-edit); otherwise the value is kept.
    pub fn start_edit(&mut self, row: usize, display_col: usize, seed: Option<String>, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        // Enter inside the editor also bubbles up as EditCell; the open editor must win so it can commit
        if self.editing.is_some() {
            return;
        }
        let col = self.actual(display_col);
        if let Some(reason) = self.read_only_reason() {
            self.notice = Some(reason.to_string());
            cx.notify();
            return;
        }
        if self.row_state(row) == RowState::Deleted || self.column_name(col).is_none() {
            return;
        }
        let is_null = self.cell_value(row, col).0.as_ref().is_some_and(DbValue::is_null);
        let initial = self.cell_value(row, col).0.map(|v| edit_text(&v)).unwrap_or_default();
        let text = seed.clone().unwrap_or_else(|| initial.clone());
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).default_value(text.clone());
            if is_null {
                state = state.placeholder("<null>");
            }
            state
        });
        if seed.is_none() {
            input.update(cx, |i, cx| i.select_all(window, cx));
        } else {
            let end = text.chars().count() as u32;
            input.update(cx, |i, cx| i.set_cursor_position(Position::new(0, end), window, cx));
        }
        input.update(cx, |i, cx| i.focus(window, cx));

        let error = if seed.is_some() {
            parse_input(&text, self.column_schema(col)).err()
        } else {
            None
        };

        let sub = cx.subscribe_in(&input, window, |table, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { shift, secondary } => {
                if *secondary {
                    table.delegate_mut().finish_edit(true, cx);
                    window.focus(&table.focus_handle(cx), cx);
                } else if *shift {
                    let (row, display_col) = match table.delegate().editing.as_ref() {
                        Some(ed) => (ed.row, ed.display_col),
                        None => (0, 0),
                    };
                    table.delegate_mut().finish_edit(true, cx);
                    table.delegate_mut().inspect(row, display_col, window, cx);
                } else {
                    let (row, display_col) = match table.delegate().editing.as_ref() {
                        Some(ed) => (ed.row, ed.display_col),
                        None => (0, 0),
                    };
                    table.delegate_mut().finish_edit(true, cx);
                    let total_rows = table.delegate().rows_count(cx);
                    if row + 1 < total_rows {
                        table.set_selected_cell(row + 1, display_col, cx);
                        table.delegate_mut().selected_cell = Some((row + 1, display_col));
                        table.scroll_to_row(row + 1, cx);
                    }
                    window.focus(&table.focus_handle(cx), cx);
                }
            }
            InputEvent::Blur => {
                table.delegate_mut().finish_edit(true, cx);
            }
            InputEvent::Change => {
                table.delegate_mut().validate_current_edit(cx);
            }
            _ => {}
        });
        self.editing = Some(CellEditor { row, col, display_col, input, initial, error, _sub: sub });
        self.selected_cell = Some((row, display_col));
        self.notice = None;
        cx.notify();
    }

    fn validate_current_edit(&mut self, cx: &mut Context<TableState<Self>>) {
        let (col, text, initial) = match self.editing.as_ref() {
            Some(ed) => (ed.col, ed.input.read(cx).value().to_string(), ed.initial.clone()),
            None => return,
        };
        let error = if text != initial {
            parse_input(&text, self.column_schema(col)).err()
        } else {
            None
        };
        if let Some(ed) = self.editing.as_mut() {
            ed.error = error;
        }
        cx.notify();
    }

    /// Commits (or discards) the in-cell editor. Invalid input is rejected with a notice (E7).
    pub fn finish_edit(&mut self, commit: bool, cx: &mut Context<TableState<Self>>) {
        let Some(ed) = self.editing.take() else { return };
        let text = ed.input.read(cx).value().to_string();
        if commit && text != ed.initial {
            match parse_input(&text, self.column_schema(ed.col)) {
                Ok(value) => {
                    self.set_value(ed.row, ed.col, value);
                    self.notice = None;
                }
                Err(e) => self.notice = Some(e),
            }
        }
        cx.notify();
    }

    fn editable(&mut self) -> bool {
        match self.read_only_reason() {
            Some(reason) => {
                self.notice = Some(reason.to_string());
                false
            }
            None => true,
        }
    }

    /// Adds an empty row (columns left out use their DEFAULT); returns its row index
    pub fn add_row(&mut self) -> Option<usize> {
        if !self.editable() {
            return None;
        }
        self.pending.row_inserts.push(PendingRowInsert { temp_id: uuid::Uuid::new_v4().to_string(), values: Default::default() });
        Some(self.loaded_len() + self.pending.row_inserts.len() - 1)
    }

    /// Copies a row into a new pending insert, leaving primary keys to their defaults
    pub fn duplicate_row(&mut self, row: usize) -> Option<usize> {
        if !self.editable() {
            return None;
        }
        let pks = self.source.as_ref()?.primary_keys.clone();
        let columns = self.result()?.columns.clone();
        let values = columns
            .iter()
            .enumerate()
            .filter(|(_, name)| !pks.contains(name))
            .filter_map(|(col, name)| Some((name.clone(), self.cell_value(row, col).0?)))
            .collect();
        self.pending.row_inserts.push(PendingRowInsert { temp_id: uuid::Uuid::new_v4().to_string(), values });
        Some(self.loaded_len() + self.pending.row_inserts.len() - 1)
    }

    /// Marks a loaded row for deletion (toggle), or drops a pending insert
    pub fn delete_row(&mut self, row: usize) {
        if !self.editable() {
            return;
        }
        let loaded = self.loaded_len();
        if row >= loaded {
            if row - loaded < self.pending.row_inserts.len() {
                self.pending.row_inserts.remove(row - loaded);
            }
        } else if let Some(key) = self.row_key(row) {
            self.pending.toggle_delete(key);
        }
    }

    /// Resets a cell to its column default: dropped from a pending insert, or `DEFAULT` in the UPDATE
    pub fn set_default(&mut self, row: usize, col: usize) {
        if !self.editable() || self.row_state(row) == RowState::Deleted {
            return;
        }
        let col = self.actual(col);
        let Some(name) = self.column_name(col).map(str::to_string) else { return };
        let loaded = self.loaded_len();
        if row >= loaded {
            if let Some(insert) = self.pending.row_inserts.get_mut(row - loaded) {
                insert.values.remove(&name);
            }
            return;
        }
        let expr = if self.source.as_ref().is_some_and(|s| s.dialect().supports_default_keyword()) {
            Some("DEFAULT".to_string())
        } else {
            self.column_schema(col).map(|c| c.default_value.clone().unwrap_or_else(|| "NULL".into()))
        };
        if let Some(expr) = expr {
            self.set_value(row, col, DbValue::Expr(expr));
        }
    }

    pub fn set_null(&mut self, row: usize, col: usize) {
        if self.editable() && self.row_state(row) != RowState::Deleted {
            self.set_value(row, self.actual(col), DbValue::Null);
        }
    }

    pub fn revert(&mut self) {
        self.pending.clear();
        self.editing = None;
        self.notice = None;
    }

    // ── Value editor ────────────────────────────────────────

    pub fn inspector_open(&self) -> bool {
        self.inspector.is_some()
    }

    pub fn close_inspector(&mut self) {
        self.inspector = None;
    }

    /// Shows a cell in the side value editor; JSON is pretty-printed
    pub fn inspect(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let col = self.actual(col);
        let value = self.cell_value(row, col).0;
        let is_json = matches!(value, Some(DbValue::Json(_)))
            || self.column_schema(col).is_some_and(|c| c.data_type == ColumnType::Json);
        let text = match &value {
            Some(DbValue::Json(j)) => serde_json::to_string_pretty(j).unwrap_or_default(),
            Some(v) => edit_text(v),
            None => String::new(),
        };
        let read_only = self.read_only_reason().is_some() || self.row_state(row) == RowState::Deleted;
        let editor = cx.new(|cx| {
            let e = EditorState::new(window, cx).soft_wrap(true).default_value(text.clone());
            if is_json { e.language("json") } else { e }
        });
        editor.update(cx, |e, cx| e.set_readonly(read_only, cx));
        self.inspector = Some(Inspector { row, col, editor, initial: text });
    }

    /// Writes the value editor's text back into the cell (validated like any edit)
    pub fn apply_inspector(&mut self, cx: &mut Context<TableState<Self>>) {
        let Some(ins) = self.inspector.as_ref() else { return };
        let text = ins.editor.read(cx).value().to_string();
        let (row, col, unchanged) = (ins.row, ins.col, text == ins.initial);
        if unchanged || !self.editable() {
            return;
        }
        let is_json = matches!(self.cell_value(row, col).0, Some(DbValue::Json(_)));
        let parsed = if is_json {
            serde_json::from_str(&text).map(DbValue::Json).map_err(|e| format!("Invalid JSON: {e}"))
        } else {
            parse_input(&text, self.column_schema(col))
        };
        match parsed {
            Ok(v) => {
                self.set_value(row, col, v);
                self.notice = None;
                if let Some(ins) = self.inspector.as_mut() {
                    ins.initial = text;
                }
            }
            Err(e) => self.notice = Some(e),
        }
        cx.notify();
    }

    // ── Loading ─────────────────────────────────────────────

    /// Loads the first page (`reset`), the next page, or re-reads what's loaded in place (`reload`).
    pub fn fetch(&mut self, mode: Fetch, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let loaded = self.loaded_len();
        let page = self.source.as_ref().map_or(500, |s| s.page_size.max(1));
        let (offset, limit) = match mode {
            Fetch::Reset => (0, page),
            Fetch::More => (loaded, page),
            Fetch::Reload => (0, loaded.max(page)),
        };
        let Some(src) = self.source.as_mut() else { return };
        if src.loading {
            return;
        }
        src.loading = true;
        src.error = None;
        let (session, sql) = (src.session.clone(), src.page_sql(offset, limit));

        run_db(window, cx, async move { session.execute(&sql).await }, move |table, res, _, cx| {
            let rs = table.delegate_mut();
            let Some(src) = rs.source.as_mut() else { return };
            src.loading = false;
            match res {
                Ok(mut page) => {
                    src.has_more = page.rows.len() > limit;
                    page.rows.truncate(limit);
                    match rs.results.first_mut() {
                        Some(existing) if mode == Fetch::More => existing.rows.append(&mut page.rows),
                        _ => rs.set_results(vec![page]),
                    }
                }
                Err(e) => src.error = Some(e.to_string()),
            }
            table.refresh(cx);
            if mode == Fetch::Reset {
                // A new query starts at the top; otherwise the old offset would trigger load_more at once
                table.scroll_to_row(0, cx);
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Fetch {
    Reset,
    More,
    Reload,
}

/// Text shown in the in-cell editor for a value. NULL edits start empty.
fn edit_text(v: &DbValue) -> String {
    match v {
        DbValue::Null => String::new(),
        v => v.to_display_string(),
    }
}

/// Parses editor text into a value of the column's type (E7)
fn parse_input(text: &str, col: Option<&ColumnSchema>) -> Result<DbValue, String> {
    let Some(col) = col else { return Ok(DbValue::Text(text.to_string())) };
    let bad = |what: &str| format!("\"{}\" is not a valid {} for {}", text, what, col.name);
    let t = text.trim();
    if col.is_nullable && (t.is_empty() || t.eq_ignore_ascii_case("null") || t.eq_ignore_ascii_case("<null>")) && col.data_type != ColumnType::Text {
        return Ok(DbValue::Null);
    }
    if col.is_nullable && (t.eq_ignore_ascii_case("<null>") || t == "\\N") {
        return Ok(DbValue::Null);
    }
    match &col.data_type {
        ColumnType::Integer | ColumnType::BigInt => t.parse().map(DbValue::Int).map_err(|_| bad("integer")),
        ColumnType::Float if col.raw_type_name.to_lowercase().contains("numeric")
            || col.raw_type_name.to_lowercase().contains("decimal") =>
        {
            t.parse::<f64>().map(|_| DbValue::Decimal(t.to_string())).map_err(|_| bad("number"))
        }
        ColumnType::Float => t.parse().map(DbValue::Float).map_err(|_| bad("number")),
        ColumnType::Boolean => match t.to_lowercase().as_str() {
            "true" | "t" | "1" | "yes" => Ok(DbValue::Boolean(true)),
            "false" | "f" | "0" | "no" => Ok(DbValue::Boolean(false)),
            _ => Err(bad("boolean")),
        },
        ColumnType::Json => serde_json::from_str(t).map(DbValue::Json).map_err(|e| format!("Invalid JSON for {}: {}", col.name, e)),
        _ => Ok(DbValue::Text(text.to_string())),
    }
}

impl TableDelegate for ResultSet {
    fn columns_count(&self, _: &App) -> usize {
        self.order.as_ref().map_or_else(|| self.result().map_or(0, |r| r.columns.len()), Vec::len)
    }

    fn rows_count(&self, _: &App) -> usize {
        self.loaded_len() + if self.source.is_some() { self.pending.row_inserts.len() } else { 0 }
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let r = self.result().expect("column() is only called when columns exist");
        let col_ix = self.actual(col_ix);
        let numeric = r
            .rows
            .first()
            .and_then(|row| row.get(col_ix))
            .is_some_and(|v| matches!(v, DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_)));
        let name = &r.columns[col_ix];
        // Fit the header and a sample of values; long text columns get more room
        let chars = r
            .rows
            .iter()
            .take(50)
            .filter_map(|row| row.get(col_ix))
            .map(|v| v.to_display_string().chars().count())
            .chain([name.chars().count() + 4])
            .max()
            .unwrap_or(8);
        let max = if matches!(name.as_str(), "statement" | "query") { self.max_col_width.max(720.0) } else { self.max_col_width };
        // ~8.4px per monospace character at the grid's text size, plus cell padding
        let width = (chars as f32 * 8.4 + 32.0).clamp(64.0, max);
        let col = Column::new(col_ix.to_string(), name.clone()).width(px(width));
        let col = match &self.sort {
            Some((n, ColumnSort::Ascending)) if n == name => col.ascending(),
            Some((n, ColumnSort::Descending)) if n == name => col.descending(),
            _ => col.sortable(),
        };
        if numeric { col.text_right() } else { col }
    }

    fn render_th(&mut self, col_ix: usize, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let grid = cx.entity().downgrade();
        div()
            .id(("th", col_ix))
            .size_full()
            .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let pos = ev.position;
                grid.update(cx, |t, cx| t.delegate_mut().open_header_menu(col_ix, pos, window, cx)).ok();
            })
            .child(self.column(col_ix, cx).name.clone())
    }

    fn perform_sort(&mut self, col_ix: usize, _: ColumnSort, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let Some(name) = self.result().and_then(|r| r.columns.get(self.actual(col_ix))).cloned() else { return };
        // Our own cycle, default → ascending → descending → default (the table's own goes the other way)
        let sort = match &self.sort {
            Some((n, ColumnSort::Ascending)) if *n == name => ColumnSort::Descending,
            Some((n, ColumnSort::Descending)) if *n == name => ColumnSort::Default,
            _ => ColumnSort::Ascending,
        };
        self.apply_sort(col_ix, sort, window, cx);
    }

    fn loading(&self, _: &App) -> bool {
        self.source.as_ref().is_some_and(|s| s.loading) && self.results.is_empty()
    }

    fn has_more(&self, _: &App) -> bool {
        self.source.as_ref().is_some_and(|s| s.has_more && !s.loading)
    }

    fn load_more(&mut self, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.fetch(Fetch::More, window, cx);
    }

    fn move_column(&mut self, col_ix: usize, to_ix: usize, _: &mut Window, _: &mut Context<TableState<Self>>) {
        let mut order = self.visible();
        if col_ix < order.len() && to_ix < order.len() {
            let c = order.remove(col_ix);
            order.insert(to_ix, c);
            self.order = Some(order);
        }
    }

    fn context_menu(&mut self, _row_ix: usize, menu: PopupMenu, _: &mut Window, _: &mut Context<TableState<Self>>) -> PopupMenu {
        grid_menu(menu, self)
    }

    fn render_td(&mut self, row_ix: usize, col_ix: usize, window: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let c = self.colors;
        let display_col = col_ix;
        let col_ix = self.actual(col_ix);
        let is_selected = self.selected_cell == Some((row_ix, display_col));
        // Foreign-key cells look like links (F4 follows them)
        let is_fk = self
            .column_name(col_ix)
            .is_some_and(|n| self.source.as_ref().is_some_and(|s| s.foreign_keys.iter().any(|f| f.from_column == n)));
        let mono = cx.theme().mono_font_family.clone();
        if let Some(ed) = self.editing.as_ref().filter(|e| e.row == row_ix && e.col == col_ix) {
            let error = ed.error.clone();
            let has_error = error.is_some();
            let is_bool = self.column_schema(col_ix).is_some_and(|c| c.data_type == ColumnType::Boolean);
            let is_nullable = self.column_schema(col_ix).is_none_or(|c| c.is_nullable);

            let input_widget = div()
                .flex_1()
                .h_full()
                .min_w_0()
                .flex()
                .items_center()
                .child(
                    Input::new(&ed.input)
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .font_family(mono.clone())
                        .text_color(c.text)
                        .px_0()
                        .py_0()
                        .h_full()
                        .w_full(),
                );

            let error_badge = error.map(|err| {
                div()
                    .id("grid-cell-error-icon")
                    .flex_shrink_0()
                    .px_1()
                    .text_color(c.danger)
                    .tooltip(move |window, cx| Tooltip::new(err.clone()).build(window, cx))
                    .child(Icon::new(IconName::TriangleAlert).xsmall())
            });

            let bool_actions = if is_bool {
                Some(
                    h_flex()
                        .flex_shrink_0()
                        .gap_1()
                        .px_1()
                        .child(
                            div()
                                .id("cell-quick-true")
                                .cursor_pointer()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(3.0))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .bg(c.token_bg)
                                .text_color(c.success)
                                .hover(|s| s.bg(c.success).text_color(c.on_fill))
                                .tooltip(move |window, cx| Tooltip::new("Set true").build(window, cx))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |table, _, _, cx| {
                                    table.delegate_mut().editing = None;
                                    table.delegate_mut().set_value(row_ix, col_ix, DbValue::Boolean(true));
                                    table.refresh(cx);
                                    cx.notify();
                                }))
                                .child("true"),
                        )
                        .child(
                            div()
                                .id("cell-quick-false")
                                .cursor_pointer()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(3.0))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .bg(c.token_bg)
                                .text_color(c.danger)
                                .hover(|s| s.bg(c.danger).text_color(c.on_fill))
                                .tooltip(move |window, cx| Tooltip::new("Set false").build(window, cx))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |table, _, _, cx| {
                                    table.delegate_mut().editing = None;
                                    table.delegate_mut().set_value(row_ix, col_ix, DbValue::Boolean(false));
                                    table.refresh(cx);
                                    cx.notify();
                                }))
                                .child("false"),
                        ),
                )
            } else {
                None
            };

            let null_button = if is_nullable && !is_bool {
                Some(
                    div()
                        .id("cell-quick-null")
                        .flex_shrink_0()
                        .cursor_pointer()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(3.0))
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.muted)
                        .bg(c.token_bg)
                        .hover(|s| s.bg(c.accent).text_color(c.on_fill))
                        .tooltip(move |window, cx| Tooltip::new("Set NULL (⌥⌘N)").build(window, cx))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |table, _, _, cx| {
                            table.delegate_mut().editing = None;
                            table.delegate_mut().set_null(row_ix, display_col);
                            table.refresh(cx);
                            cx.notify();
                        }))
                        .child("NULL"),
                )
            } else {
                None
            };

            let expand_button = div()
                .id("cell-quick-expand")
                .flex_shrink_0()
                .cursor_pointer()
                .p_1()
                .rounded(px(3.0))
                .text_color(c.muted)
                .hover(|s| s.bg(c.token_bg).text_color(c.accent))
                .tooltip(move |window, cx| Tooltip::new("Open in Value Editor (⇧↵)").build(window, cx))
                .on_mouse_down(MouseButton::Left, cx.listener(move |table, _, window, cx| {
                    table.delegate_mut().finish_edit(true, cx);
                    table.delegate_mut().inspect(row_ix, display_col, window, cx);
                    table.refresh(cx);
                    cx.notify();
                }))
                .child(Icon::new(IconName::PanelRight).xsmall());

            return div()
                .id("grid-cell-editor")
                .absolute()
                .top(px(-4.0))
                .bottom(px(-4.0))
                .left(px(-8.0))
                .right(px(-8.0))
                .bg(c.editor_bg)
                .border_2()
                .border_color(if has_error { c.danger } else { c.accent })
                .rounded(px(2.0))
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.4),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .flex()
                .items_center()
                .justify_between()
                .overflow_hidden()
                .font_family(mono)
                .pl_2()
                .child(input_widget)
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .pr_1p5()
                        .children(error_badge)
                        .children(bool_actions)
                        .children(null_button)
                        .child(expand_button),
                )
                .into_any_element();
        }
        let state = self.row_state(row_ix);
        let (value, edited) = self.cell_value(row_ix, col_ix);
        let numeric = matches!(value, Some(DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_)));
        let token = |label: SharedString, fg: Rgba| {
            div().px_1p5().rounded(px(3.0)).bg(c.token_bg).text_color(fg).child(label)
        };
        let content = match &value {
            None => token("DEFAULT".into(), c.muted).into_any_element(),
            Some(DbValue::Null) => token(self.null_label.clone(), c.muted).into_any_element(),
            Some(DbValue::Expr(_)) => token("DEFAULT".into(), c.muted).into_any_element(),
            Some(DbValue::Boolean(true)) => token("true".into(), c.success).into_any_element(),
            Some(DbValue::Boolean(false)) => token("false".into(), c.danger).into_any_element(),
            Some(v) => div()
                .text_color(if state == RowState::Deleted {
                    c.muted
                } else if self.is_error_row(row_ix) {
                    c.danger
                } else {
                    c.text
                })
                .when(state == RowState::Deleted, |d| d.line_through())
                .when(is_fk && state != RowState::Deleted, |d| d.underline())
                .truncate()
                .child(v.to_display_string())
                .into_any_element(),
        };
        // Column divider on the cell's true right edge: the table pads cells 8px sideways and 4px up and down (Size::Medium)
        let divider = div().absolute().top(px(-4.0)).bottom(px(-4.0)).right(px(-8.0)).w_px().bg(c.grid_line);

        // JetBrains DataGrip-style selection box (only when table is focused)
        let is_focused = cx.focus_handle().contains_focused(window, cx);
        let selection_box = (is_selected && is_focused).then(|| {
            div()
                .id("cell-selection-box")
                .absolute()
                .top(px(-4.0))
                .bottom(px(-4.0))
                .left(px(-8.0))
                .right(px(-8.0))
                .bg(if edited { Hsla::from(c.pending_bg) } else { Theme::tint(c.accent, 0.14) })
                .border_2()
                .border_color(c.accent)
        });

        // Pending uncommitted edit background (when not selected)
        let pending_box = (edited && !is_selected).then(|| {
            div()
                .id("cell-pending-bg")
                .absolute()
                .top(px(-4.0))
                .bottom(px(-4.0))
                .left(px(-8.0))
                .right(px(-8.0))
                .bg(c.pending_bg)
        });

        // JetBrains DataGrip-style modified cell indicator (top-right corner pip)
        let modified_corner = edited.then(|| {
            div()
                .id("cell-modified-corner")
                .absolute()
                .top(px(-4.0))
                .right(px(-8.0))
                .w(px(7.0))
                .h(px(7.0))
                .bg(c.accent)
                .rounded_bl(px(4.0))
        });

        div()
            .relative()
            .child(divider)
            .children(pending_box)
            .children(selection_box)
            .children(modified_corner)
            .size_full()
            .flex()
            .items_center()
            .font_family(mono)
            .when(numeric, |d| d.justify_end())
            .when(state == RowState::Inserted, |d| d.bg(c.inserted_bg))
            .when(state == RowState::Deleted, |d| d.bg(c.deleted_bg))
            .child(content)
            .into_any_element()
    }
}

/// The grid's right-click menu (DataGrip's cell menu)
pub fn grid_menu(menu: PopupMenu, rs: &ResultSet) -> PopupMenu {
    let has_fks = rs.source.as_ref().is_some_and(|s| !s.foreign_keys.is_empty());
    let menu = menu
        .menu("Copy Value", Box::new(CopyValue))
        .menu("Copy Row as TSV", Box::new(CopyRowTsv))
        .menu("Copy Row as CSV", Box::new(CopyRowCsv))
        .menu("Copy Row as JSON", Box::new(CopyRowJson))
        .menu("Copy Row as SQL INSERT", Box::new(CopyRowInsert))
        .menu("Copy All as TSV", Box::new(CopyAllTsv))
        .menu("Copy All as CSV", Box::new(CopyAllCsv))
        .menu("Copy All as JSON", Box::new(CopyAllJson))
        .menu("Copy All as SQL INSERT", Box::new(CopyAllInsert))
        .separator()
        .menu("Hide Column", Box::new(HideColumn))
        .when(rs.order.is_some(), |m| m.menu("Show All Columns", Box::new(ShowAllColumns)))
        .when(has_fks, |m| m.menu("Go to Referenced Row", Box::new(FollowForeignKey)))
        .menu("Open in Value Editor", Box::new(ToggleValueEditor));
    if rs.read_only_reason().is_some() {
        return menu;
    }
    menu.separator()
        .menu("Edit Cell", Box::new(EditCell))
        .menu("Set NULL", Box::new(SetNull))
        .menu("Set DEFAULT", Box::new(SetDefault))
        .separator()
        .menu("Add Row", Box::new(AddRow))
        .menu("Duplicate Row", Box::new(DuplicateRow))
        .menu("Delete Row", Box::new(DeleteRow))
}

/// Row/cell operations shared by keyboard actions, the context menu and the toolbar.
/// They target the selected cell, or the right-clicked row.
pub fn apply_row_action(grid: &Entity<TableState<ResultSet>>, action: &dyn Action, window: &mut Window, cx: &mut App) {
    grid.update(cx, |table, cx| {
        let target = table.selected_cell().or_else(|| table.right_clicked_row().map(|r| (r, 0)));
        let any = action.as_any();
        let rs = table.delegate_mut();
        use quarry_core::export::ExportFormat::{Csv, Json, Sql};
        let copy_format = if any.is::<CopyRowTsv>() || any.is::<CopyAllTsv>() {
            Some((Csv, '\t'))
        } else if any.is::<CopyRowCsv>() || any.is::<CopyAllCsv>() {
            Some((Csv, ','))
        } else if any.is::<CopyRowJson>() || any.is::<CopyAllJson>() {
            Some((Json, ','))
        } else if any.is::<CopyRowInsert>() || any.is::<CopyAllInsert>() {
            Some((Sql, ','))
        } else {
            None
        };
        let whole = any.is::<CopyAllTsv>() || any.is::<CopyAllCsv>() || any.is::<CopyAllJson>() || any.is::<CopyAllInsert>();
        let mut clipboard = None;
        let new_row = match target {
            _ if any.is::<AddRow>() => rs.add_row(),
            _ if any.is::<ShowAllColumns>() => {
                rs.show_all_columns();
                None
            }
            _ if whole => {
                clipboard = copy_format.and_then(|(f, d)| rs.copy_text(None, f, d));
                None
            }
            Some((row, _)) if copy_format.is_some() => {
                clipboard = copy_format.and_then(|(f, d)| rs.copy_text(Some(row), f, d));
                None
            }
            Some((row, col)) if any.is::<CopyValue>() => {
                clipboard = rs.cell_value(row, rs.actual(col)).0.map(|v| if v.is_null() { String::new() } else { v.to_display_string() });
                None
            }
            Some((_, col)) if any.is::<HideColumn>() => {
                rs.hide_column(col);
                None
            }
            Some((row, col)) if any.is::<EditCell>() => {
                rs.start_edit(row, col, None, window, cx);
                None
            }
            Some((row, col)) if any.is::<SetDefault>() => {
                rs.finish_edit(false, cx);
                rs.set_default(row, col);
                None
            }
            Some((row, col)) if any.is::<ToggleValueEditor>() => {
                rs.finish_edit(true, cx);
                if rs.inspector_open() {
                    rs.close_inspector();
                } else {
                    rs.inspect(row, col, window, cx);
                }
                None
            }
            Some((row, _)) if any.is::<DuplicateRow>() => rs.duplicate_row(row),
            Some((row, _)) if any.is::<DeleteRow>() => {
                rs.delete_row(row);
                None
            }
            Some((row, col)) if any.is::<SetNull>() => {
                rs.finish_edit(false, cx);
                rs.set_null(row, col);
                None
            }
            _ => None,
        };
        if let Some(text) = clipboard {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        table.refresh(cx);
        if let Some(row) = new_row {
            table.scroll_to_row(row, cx);
            table.set_selected_cell(row, 0, cx);
            table.delegate_mut().selected_cell = Some((row, 0));
        }
        cx.notify();
    });
}

pub fn next_cell(grid: &Entity<TableState<ResultSet>>, window: &mut Window, cx: &mut App) {
    grid.update(cx, |table, cx| {
        let (row, display_col) = match table.delegate().editing.as_ref() {
            Some(ed) => (ed.row, ed.display_col),
            None => table.selected_cell().unwrap_or((0, 0)),
        };
        table.delegate_mut().finish_edit(true, cx);
        let total_rows = table.delegate().rows_count(cx);
        let visible_cols = table.delegate().visible().len();
        if visible_cols == 0 || total_rows == 0 {
            return;
        }
        let (next_row, next_col) = if display_col + 1 < visible_cols {
            (row, display_col + 1)
        } else if row + 1 < total_rows {
            (row + 1, 0)
        } else if let Some(new_row) = table.delegate_mut().add_row() {
            (new_row, 0)
        } else {
            (row, display_col)
        };
        table.set_selected_cell(next_row, next_col, cx);
        table.delegate_mut().selected_cell = Some((next_row, next_col));
        table.scroll_to_row(next_row, cx);
        table.delegate_mut().start_edit(next_row, next_col, None, window, cx);
        cx.notify();
    });
}

pub fn prev_cell(grid: &Entity<TableState<ResultSet>>, window: &mut Window, cx: &mut App) {
    grid.update(cx, |table, cx| {
        let (row, display_col) = match table.delegate().editing.as_ref() {
            Some(ed) => (ed.row, ed.display_col),
            None => table.selected_cell().unwrap_or((0, 0)),
        };
        table.delegate_mut().finish_edit(true, cx);
        let total_rows = table.delegate().rows_count(cx);
        let visible_cols = table.delegate().visible().len();
        if visible_cols == 0 || total_rows == 0 {
            return;
        }
        let (prev_row, prev_col) = if display_col > 0 {
            (row, display_col - 1)
        } else if row > 0 {
            (row - 1, visible_cols.saturating_sub(1))
        } else {
            (row, display_col)
        };
        table.set_selected_cell(prev_row, prev_col, cx);
        table.delegate_mut().selected_cell = Some((prev_row, prev_col));
        table.scroll_to_row(prev_row, cx);
        table.delegate_mut().start_edit(prev_row, prev_col, None, window, cx);
        cx.notify();
    });
}

pub struct DataGrid;

impl DataGrid {
    #[allow(clippy::too_many_arguments)]
    pub fn render<T: 'static>(
        grid: &Entity<TableState<ResultSet>>,
        theme: &Theme,
        cx: &Context<T>,
        on_refresh: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_review: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_submit: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_export: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_follow_fk: impl Fn(&mut T, (String, String, String), &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        let rs = grid.read(cx).delegate();
        let pending_count = rs.pending.total_count();
        let (set_count, active) = (rs.results.len(), rs.active);
        let error = rs.source.as_ref().and_then(|s| s.error.clone());
        let notice = rs.notice.clone();
        let header_menu = rs.header_menu.as_ref().map(|(p, m, _)| (*p, m.clone()));
        let is_table = rs.source.is_some();
        let read_only = rs.read_only_reason();
        let hidden = rs.hidden_count();
        let inspector = rs.inspector.as_ref().map(|i| {
            let col = rs.column_name(i.col).unwrap_or_default().to_string();
            (i.editor.clone(), col, i.row, read_only.is_some() || rs.row_state(i.row) == RowState::Deleted)
        });

        let action_button = |id: &'static str, icon: IconName, tip: &'static str, action: Box<dyn Action>| {
            let grid = grid.clone();
            zbutton(id)
                .icon(icon)
                .ghost()
                .xsmall()
                .tooltip(tip)
                .disabled(read_only.is_some())
                .on_click(move |_, window, cx| apply_row_action(&grid, action.as_ref(), window, cx))
        };

        // Toolbar: Zed-quiet icon buttons with DataGrip's operations
        let toolbar = h_flex()
            .h(px(BAR_H))
            .px_3()
            .gap_0p5()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                zbutton("grid-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Reload (⌘↵ outside the grid)")
                    .on_click(cx.listener(move |this, _, window, cx| on_refresh(this, window, cx))),
            )
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child(action_button("grid-add", IconName::Plus, "Add row (⌘N)", Box::new(AddRow)))
            .child(action_button("grid-dup", IconName::Copy, "Duplicate row (⌘D)", Box::new(DuplicateRow)))
            .child(action_button("grid-del", IconName::Minus, "Delete row (⌘⌫)", Box::new(DeleteRow)))
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child({
                let grid = grid.clone();
                zbutton("grid-value-editor")
                    .icon(IconName::PanelRight)
                    .ghost()
                    .xsmall()
                    .tooltip("Value editor (⇧↵)")
                    .on_click(move |_, window, cx| apply_row_action(&grid, &ToggleValueEditor, window, cx))
            })
            .child(
                zbutton("grid-export")
                    .icon(IconName::Download)
                    .ghost()
                    .xsmall()
                    .tooltip("Export (⇧⌘E)")
                    .on_click(cx.listener(move |this, _, window, cx| on_export(this, window, cx))),
            )
            .when(hidden > 0, |this| {
                let grid = grid.clone();
                this.child(
                    zbutton("grid-hidden").pad_xs()
                        .label(format!("{hidden} hidden"))
                        .ghost()
                        .xsmall()
                        .tooltip("Show all columns")
                        .on_click(move |_, window, cx| apply_row_action(&grid, &ShowAllColumns, window, cx)),
                )
            })
            .child(div().flex_1())
            .when_some(notice.clone(), |this, n| {
                this.child(div().px_2().text_xs().text_color(theme.danger).truncate().child(n))
            })
            .when_some(read_only.filter(|_| notice.is_none()), |this, reason| {
                this.child(div().px_2().text_xs().text_color(theme.text_muted).child(reason))
            })
            .when(pending_count > 0, |this| {
                let grid = grid.clone();
                this.child(div().px_1().text_xs().text_color(theme.warning).child(format!(
                    "{} pending {}",
                    pending_count,
                    if pending_count == 1 { "change" } else { "changes" }
                )))
                .child(
                    zbutton("grid-revert")
                        .icon(IconName::Undo2)
                        .ghost()
                        .xsmall()
                        .tooltip("Revert all (⌥⌘Z)")
                        .on_click(move |_, _, cx| {
                            grid.update(cx, |t, cx| {
                                t.delegate_mut().revert();
                                t.refresh(cx);
                                cx.notify();
                            })
                        }),
                )
                .child(
                    zbutton("grid-review").pad_xs()
                        .label("Review")
                        .ghost()
                        .xsmall()
                        .tooltip("Show the SQL (⌘S)")
                        .on_click(cx.listener(move |this, _, window, cx| on_review(this, window, cx))),
                )
                .child(
                    zbutton("grid-submit").pad_xs()
                        .icon(IconName::Check)
                        .label("Submit")
                        .primary()
                        .xsmall()
                        .tooltip("Apply in one transaction (⌘↵)")
                        .on_click(cx.listener(move |this, _, window, cx| on_submit(this, window, cx))),
                )
            });

        let grid_cancel = grid.clone();
        let grid_revert = grid.clone();
        let grid_out = grid.clone();

        v_flex()
            .id("data-grid-root")
            .size_full()
            .bg(theme.bg_app)
            .on_mouse_down_out(move |_, _, cx| {
                grid_out.update(cx, |t, cx| {
                    t.delegate_mut().finish_edit(true, cx);
                    t.clear_selection(cx);
                    t.delegate_mut().selected_cell = None;
                    t.refresh(cx);
                });
            })
            .when(is_table, |this| this.child(toolbar))
            .when_some(header_menu, |this, (pos, menu)| {
                this.child(deferred(anchored().position(pos).snap_to_window().child(menu)).with_priority(1))
            })
            .when(!is_table, |this| {
                this.when_some(notice.clone(), |this, n| {
                    this.child(div().px_3().py_1().text_xs().text_color(theme.text_muted).border_b_1().border_color(theme.border).child(n))
                })
            })
            // Result sub-tabs, DataGrip style: one per row-returning statement
            .when(set_count > 1, |this| {
                this.child(
                    h_flex()
                        .h(px(CONTROL_H))
                        .px_2()
                        .gap_1()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .children((0..set_count).map(|ix| {
                            let grid = grid.clone();
                            div()
                                .id(("result-set", ix))
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.0))
                                .text_xs()
                                .cursor_pointer()
                                .when(ix == active, |d| d.bg(theme.bg_selected).text_color(theme.text_primary))
                                .when(ix != active, |d| d.text_color(theme.text_muted).hover(|s| s.bg(theme.bg_hover)))
                                .on_click(move |_, _, cx| {
                                    grid.update(cx, |t, cx| {
                                        t.delegate_mut().set_active(ix);
                                        t.refresh(cx);
                                        cx.notify();
                                    });
                                })
                                .child(format!("Result {}", ix + 1))
                        })),
                )
            })
            .when_some(error, |this, err| {
                this.child(
                    div()
                        .px_3()
                        .py_1p5()
                        .text_xs()
                        .text_color(theme.danger)
                        .border_b_1()
                        .border_color(theme.border)
                        .child(err),
                )
            })
            .on_action({
                let g = grid.clone();
                move |a: &EditCell, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &AddRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &DuplicateRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &DeleteRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SetNull, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SetDefault, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &ToggleValueEditor, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyValue, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowTsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowCsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowJson, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowInsert, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllTsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllCsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllJson, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllInsert, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &HideColumn, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &ShowAllColumns, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                cx.listener(move |this, _: &FollowForeignKey, window, cx| {
                    let target = g.read(cx).selected_cell().and_then(|(row, col)| g.read(cx).delegate().foreign_key_target(row, col));
                    if let Some(target) = target {
                        on_follow_fk(this, target, window, cx);
                    }
                })
            })
            .on_action({
                let g = grid.clone();
                move |_: &NextCell, window, cx| next_cell(&g, window, cx)
            })
            .on_action({
                let g = grid.clone();
                move |_: &PrevCell, window, cx| prev_cell(&g, window, cx)
            })
            .on_action(move |_: &CancelEdit, window, cx| {
                grid_cancel.update(cx, |t, cx| {
                    t.delegate_mut().finish_edit(false, cx);
                    window.focus(&t.focus_handle(cx), cx);
                })
            })
            .on_action(move |_: &RevertChanges, _, cx| {
                grid_revert.update(cx, |t, cx| {
                    t.delegate_mut().revert();
                    t.refresh(cx);
                    cx.notify();
                })
            })
            .on_action(cx.listener({
                let g = grid.clone();
                move |this, _: &SubmitChanges, window, cx| {
                    g.update(cx, |t, cx| t.delegate_mut().finish_edit(true, cx));
                    on_submit(this, window, cx);
                }
            }))
            // Type-to-edit: a printable key on a selected cell starts editing with that character (DataGrip)
            .on_key_down({
                let grid = grid.clone();
                move |ev: &KeyDownEvent, window, cx| {
                    let ks = &ev.keystroke;
                    if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.function {
                        return;
                    }
                    let Some(ch) = ks.key_char.clone().filter(|c| !c.chars().any(char::is_control)) else { return };
                    grid.update(cx, |t, cx| {
                        let Some((row, col)) = t.selected_cell() else { return };
                        if t.delegate().editing.is_none() {
                            t.delegate_mut().start_edit(row, col, Some(ch), window, cx);
                            cx.stop_propagation();
                        }
                    });
                }
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("data-table-container")
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .on_click({
                                let g = grid.clone();
                                move |_, _, cx| {
                                    g.update(cx, |t, cx| {
                                        t.clear_selection(cx);
                                        t.delegate_mut().selected_cell = None;
                                        t.refresh(cx);
                                    });
                                }
                            })
                            .child(DataTable::new(grid).bordered(false)),
                    )
                    .when_some(inspector, |this, (editor, col, row, read_only)| {
                        let (g_close, g_apply, g_key) = (grid.clone(), grid.clone(), grid.clone());
                        this.child(
                            v_flex()
                                .key_context("ValueEditor")
                                .on_action(move |_: &ApplyValue, _, cx| {
                                    g_key.update(cx, |t, cx| t.delegate_mut().apply_inspector(cx))
                                })
                                .w(px(360.0))
                                .h_full()
                                .border_l_1()
                                .border_color(theme.border)
                                .bg(theme.bg_panel)
                                .child(
                                    h_flex()
                                        .h(px(BAR_H))
                                        .pl_3()
                                        .pr_1()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(theme.border)
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .text_xs()
                                                .child(div().text_color(theme.text_primary).child(col))
                                                .child(div().text_color(theme.text_muted).child(format!("row {}", row + 1))),
                                        )
                                        .child(
                                            zbutton("value-editor-close")
                                                .icon(IconName::X)
                                                .ghost()
                                                .xsmall()
                                                .on_click(move |_, _, cx| {
                                                    g_close.update(cx, |t, cx| {
                                                        t.delegate_mut().close_inspector();
                                                        cx.notify();
                                                    })
                                                }),
                                        ),
                                )
                                .child(div().flex_1().min_h_0().child(Editor::new(&editor).h_full().bordered(false)))
                                .when(!read_only, |this| {
                                    this.child(
                                        h_flex().p_2().justify_end().border_t_1().border_color(theme.border).child(
                                            zbutton("value-editor-apply").pad_xs()
                                                .label("Apply")
                                                .primary()
                                                .xsmall()
                                                .tooltip("Write to the cell (⌘↵)")
                                                .on_click(move |_, _, cx| {
                                                    g_apply.update(cx, |t, cx| t.delegate_mut().apply_inspector(cx))
                                                }),
                                        ),
                                    )
                                }),
                        )
                    }),
            )
    }
}
