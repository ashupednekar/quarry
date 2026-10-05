// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::runtime::run_db;
use crate::theme::Theme;
use crate::ui::*;
use gpui::assets::IconName;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Editor, EditorState, Input, InputEvent, InputState, Position};
use gpui::component::menu::PopupMenu;
use gpui::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui::component::{Disableable, Sizable};
use gpui::*;
use quarry_core::edits::{quote_ident, PendingChanges, PendingRowInsert, RowKey};
use quarry_core::schema::{ColumnSchema, ColumnType};
use quarry_core::session::{DatabaseSession, QueryResult};
use quarry_core::value::DbValue;
use std::sync::Arc;

actions!(
    grid,
    [EditCell, CancelEdit, AddRow, DuplicateRow, DeleteRow, SetNull, SetDefault, ToggleValueEditor, ApplyValue, SubmitChanges, RevertChanges]
);

/// DataGrip-style grid keys; scoped to the grid so ⌘N/⌘↵ keep their global meaning elsewhere.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", EditCell, Some("DataTable")),
        KeyBinding::new("f2", EditCell, Some("DataTable")),
        KeyBinding::new("escape", CancelEdit, Some("DataTable > Input")),
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
    ]);
}

pub const PAGE_SIZE: usize = 500;

/// Where a table tab's rows come from; enables paging, server-side sort and editing.
pub struct TableSource {
    pub session: Arc<dyn DatabaseSession>,
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnSchema>,
    pub primary_keys: Vec<String>,
    /// Why this table can't be edited, if it can't (E1, E10)
    pub read_only_reason: Option<&'static str>,
    /// Postgres accepts `SET col = DEFAULT`; SQLite needs the column's default expression
    pub default_keyword: bool,
    pub filter: String,
    pub order_by: Option<(String, bool)>, // (column, descending)
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl TableSource {
    pub fn qualified_name(&self) -> String {
        quarry_core::edits::qualified_name(&self.schema, &self.table)
    }

    fn page_sql(&self, offset: usize, limit: usize) -> String {
        let mut sql = format!("SELECT * FROM {}", self.qualified_name());
        if !self.filter.trim().is_empty() {
            sql += &format!(" WHERE {}", self.filter);
        }
        if let Some((col, desc)) = &self.order_by {
            sql += &format!(" ORDER BY {}{}", quote_ident(col), if *desc { " DESC" } else { "" });
        }
        // One extra row tells us whether another page exists
        sql + &format!(" LIMIT {} OFFSET {}", limit + 1, offset)
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
    input: Entity<InputState>,
    initial: String,
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
    /// Side panel for long text and JSON ("value editor"); follows the selected cell
    inspector: Option<Inspector>,
    colors: CellColors,
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
}

impl ResultSet {
    pub fn new(theme: &Theme, source: Option<TableSource>) -> Self {
        Self {
            results: Vec::new(),
            active: 0,
            pending: PendingChanges::new(),
            source,
            notice: None,
            editing: None,
            inspector: None,
            colors: CellColors {
                text: theme.text_primary,
                muted: theme.text_muted,
                token_bg: theme.bg_surface,
                success: theme.success,
                danger: theme.danger,
                pending_bg: theme.pending_cell_bg,
                inserted_bg: rgba(0x10b98120),
                deleted_bg: rgba(0xef444420),
            },
        }
    }

    pub fn result(&self) -> Option<&QueryResult> {
        self.results.get(self.active)
    }

    pub fn set_results(&mut self, results: Vec<QueryResult>) {
        self.results = results;
        self.active = 0;
        self.editing = None;
        self.inspector = None;
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
    pub fn start_edit(&mut self, row: usize, col: usize, seed: Option<String>, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        // Enter inside the editor also bubbles up as EditCell; the open editor must win so it can commit
        if self.editing.is_some() {
            return;
        }
        if let Some(reason) = self.read_only_reason() {
            self.notice = Some(reason.to_string());
            cx.notify();
            return;
        }
        if self.row_state(row) == RowState::Deleted || self.column_name(col).is_none() {
            return;
        }
        let initial = self.cell_value(row, col).0.map(|v| edit_text(&v)).unwrap_or_default();
        let text = seed.unwrap_or_else(|| initial.clone());
        let end = text.chars().count() as u32;
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        input.update(cx, |i, cx| i.set_cursor_position(Position::new(0, end), window, cx));
        let sub = cx.subscribe_in(&input, window, |table, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                table.delegate_mut().finish_edit(true, cx);
                window.focus(&table.focus_handle(cx), cx);
            }
            _ => {}
        });
        self.editing = Some(CellEditor { row, col, input, initial, _sub: sub });
        self.notice = None;
        cx.notify();
    }

    /// Commits (or discards) the in-cell editor. Invalid input is rejected with a notice (E7).
    pub fn finish_edit(&mut self, commit: bool, cx: &mut Context<TableState<Self>>) {
        let Some(ed) = self.editing.take() else { return };
        let text = ed.input.read(cx).value().to_string();
        if commit && text != ed.initial {
            match parse_input(&text, self.column_schema(ed.col)) {
                Ok(value) => self.set_value(ed.row, ed.col, value),
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
        let Some(name) = self.column_name(col).map(str::to_string) else { return };
        let loaded = self.loaded_len();
        if row >= loaded {
            if let Some(insert) = self.pending.row_inserts.get_mut(row - loaded) {
                insert.values.remove(&name);
            }
            return;
        }
        let expr = if self.source.as_ref().is_some_and(|s| s.default_keyword) {
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
            self.set_value(row, col, DbValue::Null);
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
        let (offset, limit) = match mode {
            Fetch::Reset => (0, PAGE_SIZE),
            Fetch::More => (loaded, PAGE_SIZE),
            Fetch::Reload => (0, loaded.max(PAGE_SIZE)),
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
        self.result().map_or(0, |r| r.columns.len())
    }

    fn rows_count(&self, _: &App) -> usize {
        self.loaded_len() + if self.source.is_some() { self.pending.row_inserts.len() } else { 0 }
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let r = self.result().expect("column() is only called when columns exist");
        let numeric = r
            .rows
            .first()
            .and_then(|row| row.get(col_ix))
            .is_some_and(|v| matches!(v, DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_)));
        let name = &r.columns[col_ix];
        let width = if matches!(name.as_str(), "statement" | "query") { 640.0 } else { 160.0 };
        let col = Column::new(col_ix.to_string(), name.clone()).width(px(width)).sortable();
        if numeric { col.text_right() } else { col }
    }

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        // Tables sort on the server (the full table, not just the loaded page), like DataGrip
        if let Some(name) = self.source.as_ref().and(self.column_name(col_ix)).map(str::to_string) {
            let src = self.source.as_mut().expect("checked above");
            src.order_by = match sort {
                ColumnSort::Ascending => Some((name, false)),
                ColumnSort::Descending => Some((name, true)),
                ColumnSort::Default => None,
            };
            self.fetch(Fetch::Reset, window, cx);
            return;
        }
        let Some(r) = self.results.get_mut(self.active) else { return };
        match sort {
            ColumnSort::Ascending => r.rows.sort_by(|a, b| a[col_ix].cmp_val(&b[col_ix])),
            ColumnSort::Descending => r.rows.sort_by(|a, b| b[col_ix].cmp_val(&a[col_ix])),
            ColumnSort::Default => {}
        }
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

    fn context_menu(&mut self, _row_ix: usize, menu: PopupMenu, _: &mut Window, _: &mut Context<TableState<Self>>) -> PopupMenu {
        if self.read_only_reason().is_some() {
            return menu;
        }
        menu.menu("Edit Cell", Box::new(EditCell))
            .menu("Set NULL", Box::new(SetNull))
            .menu("Set DEFAULT", Box::new(SetDefault))
            .menu("Open in Value Editor", Box::new(ToggleValueEditor))
            .separator()
            .menu("Add Row", Box::new(AddRow))
            .menu("Duplicate Row", Box::new(DuplicateRow))
            .menu("Delete Row", Box::new(DeleteRow))
    }

    fn render_td(&mut self, row_ix: usize, col_ix: usize, _: &mut Window, _: &mut Context<TableState<Self>>) -> impl IntoElement {
        let c = self.colors;
        if let Some(ed) = self.editing.as_ref().filter(|e| e.row == row_ix && e.col == col_ix) {
            return div().size_full().child(Input::new(&ed.input).xsmall()).into_any_element();
        }
        let state = self.row_state(row_ix);
        let (value, edited) = self.cell_value(row_ix, col_ix);
        let numeric = matches!(value, Some(DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_)));
        let token = |label: &'static str, fg: Rgba| {
            div().px_1p5().rounded(px(3.0)).bg(c.token_bg).text_color(fg).child(label)
        };
        let content = match &value {
            None => token("DEFAULT", c.muted).into_any_element(),
            Some(DbValue::Null) => token("NULL", c.muted).into_any_element(),
            Some(DbValue::Expr(_)) => token("DEFAULT", c.muted).into_any_element(),
            Some(DbValue::Boolean(true)) => token("true", c.success).into_any_element(),
            Some(DbValue::Boolean(false)) => token("false", c.danger).into_any_element(),
            Some(v) => div()
                .text_color(if state == RowState::Deleted { c.muted } else { c.text })
                .when(state == RowState::Deleted, |d| d.line_through())
                .truncate()
                .child(v.to_display_string())
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .when(numeric, |d| d.justify_end())
            .when(state == RowState::Inserted, |d| d.bg(c.inserted_bg))
            .when(state == RowState::Deleted, |d| d.bg(c.deleted_bg))
            .when(edited, |d| d.bg(c.pending_bg))
            .child(content)
            .into_any_element()
    }
}

/// Row/cell operations shared by keyboard actions, the context menu and the toolbar.
/// They target the selected cell, or the right-clicked row.
pub fn apply_row_action(grid: &Entity<TableState<ResultSet>>, action: &dyn Action, window: &mut Window, cx: &mut App) {
    grid.update(cx, |table, cx| {
        let target = table.selected_cell().or_else(|| table.right_clicked_row().map(|r| (r, 0)));
        let any = action.as_any();
        let rs = table.delegate_mut();
        let new_row = match target {
            _ if any.is::<AddRow>() => rs.add_row(),
            Some((row, col)) if any.is::<EditCell>() => {
                rs.start_edit(row, col, None, window, cx);
                None
            }
            Some((row, col)) if any.is::<SetDefault>() => {
                rs.set_default(row, col);
                None
            }
            Some((row, col)) if any.is::<ToggleValueEditor>() => {
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
                rs.set_null(row, col);
                None
            }
            _ => None,
        };
        table.refresh(cx);
        if let Some(row) = new_row {
            table.scroll_to_row(row, cx);
            table.set_selected_cell(row, 0, cx);
        }
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
    ) -> impl IntoElement {
        let rs = grid.read(cx).delegate();
        let pending_count = rs.pending.total_count();
        let (set_count, active) = (rs.results.len(), rs.active);
        let error = rs.source.as_ref().and_then(|s| s.error.clone());
        let notice = rs.notice.clone();
        let is_table = rs.source.is_some();
        let read_only = rs.read_only_reason();
        let inspector = rs.inspector.as_ref().map(|i| {
            let col = rs.column_name(i.col).unwrap_or_default().to_string();
            (i.editor.clone(), col, i.row, read_only.is_some() || rs.row_state(i.row) == RowState::Deleted)
        });

        let action_button = |id: &'static str, icon: IconName, tip: &'static str, action: Box<dyn Action>| {
            let grid = grid.clone();
            Button::new(id)
                .icon(icon)
                .ghost()
                .xsmall()
                .tooltip(tip)
                .disabled(read_only.is_some())
                .on_click(move |_, window, cx| apply_row_action(&grid, action.as_ref(), window, cx))
        };

        // Toolbar: Zed-quiet icon buttons with DataGrip's operations
        let toolbar = h_flex()
            .h(px(30.0))
            .px_2()
            .gap_0p5()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("grid-refresh")
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
                Button::new("grid-value-editor")
                    .icon(IconName::PanelRight)
                    .ghost()
                    .xsmall()
                    .tooltip("Value editor (⇧↵)")
                    .on_click(move |_, window, cx| apply_row_action(&grid, &ToggleValueEditor, window, cx))
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
                    Button::new("grid-revert")
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
                    Button::new("grid-review")
                        .label("Review")
                        .ghost()
                        .xsmall()
                        .tooltip("Show the SQL (⌘S)")
                        .on_click(cx.listener(move |this, _, window, cx| on_review(this, window, cx))),
                )
                .child(
                    Button::new("grid-submit")
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

        v_flex()
            .size_full()
            .bg(theme.bg_app)
            .when(is_table, |this| this.child(toolbar))
            .when(!is_table, |this| {
                this.when_some(notice.clone(), |this, n| {
                    this.child(div().px_3().py_1().text_xs().text_color(theme.text_muted).border_b_1().border_color(theme.border).child(n))
                })
            })
            // Result sub-tabs, DataGrip style: one per row-returning statement
            .when(set_count > 1, |this| {
                this.child(
                    h_flex()
                        .h(px(28.0))
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
                                        t.delegate_mut().active = ix;
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
            .on_action(cx.listener(move |this, _: &SubmitChanges, window, cx| on_submit(this, window, cx)))
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
                    .child(div().flex_1().h_full().min_w_0().child(DataTable::new(grid).bordered(false)))
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
                                        .h(px(30.0))
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
                                            Button::new("value-editor-close")
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
                                            Button::new("value-editor-apply")
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
