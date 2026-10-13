// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::runtime::run_db;
use crate::theme::Theme;
use crate::ui::*;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, MoveDown, MoveUp, Position, Textarea,
    TextareaState,
};
use gpui::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui::component::tooltip::Tooltip;
use gpui::component::{ActiveTheme, Disableable, Icon, Sizable};
use gpui::*;
use quarry_core::edits::{PendingChanges, PendingRowInsert, RowKey};
use quarry_core::schema::{ColumnSchema, ColumnType};
use quarry_core::session::{DatabaseSession, QueryResult};
use quarry_core::value::DbValue;
use std::sync::Arc;

mod actions;
mod delegate;
mod editing;

pub use actions::*;
pub use editing::{Fetch, PageTo};

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
        UndoChange,
        RedoChange,
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
        // Inside the cell editor ⌘Z is the text's own undo; on the grid it steps through pending changes
        KeyBinding::new("secondary-z", UndoChange, Some("DataTable")),
        KeyBinding::new("secondary-shift-z", RedoChange, Some("DataTable")),
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
    /// First row of the page on screen (0-based); pages replace each other, like DataGrip
    pub offset: usize,
    /// Rows matching the filter, once counted (or known from reaching the last page)
    pub total: Option<u64>,
    pub counting: bool,
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
            sql += &format!(
                " ORDER BY {}{}",
                self.dialect().quote_ident(col),
                if *desc { " DESC" } else { "" }
            );
        }
        sql
    }

    /// `SELECT COUNT(*)` over the tab's filter
    fn count_sql(&self) -> String {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.qualified_name());
        if !self.filter.trim().is_empty() {
            sql += &format!(" WHERE {}", self.filter);
        }
        sql
    }

    fn page_sql(&self, offset: usize, limit: usize) -> String {
        // One extra row tells us whether another page exists
        self.dialect()
            .limit(&self.select_sql(), limit as u64 + 1, offset as u64)
    }
}

struct Inspector {
    row: usize,
    col: usize,
    editor: Entity<EditorState>,
    initial: String,
}

/// Text edits in a textarea that grows and pops out; a number in a one-line input, which
/// (unlike the textarea) right-aligns, so the digits stay where the grid drew them
enum CellInput {
    Text(Entity<TextareaState>),
    Number(Entity<InputState>),
}

impl CellInput {
    fn value(&self, cx: &App) -> String {
        match self {
            CellInput::Text(e) => e.read(cx).value().to_string(),
            CellInput::Number(e) => e.read(cx).value().to_string(),
        }
    }
}

struct CellEditor {
    row: usize,
    col: usize,
    display_col: usize,
    input: CellInput,
    /// Cell width from the last paint; the editor pops out over the grid when the text won't fit
    cell_w: std::rc::Rc<std::cell::Cell<Pixels>>,
    initial: String,
    error: Option<String>,
    /// Date and timestamp cells: ↑/↓ step the part under the caret, a strip reads the value back
    date: Option<DatePick>,
    _sub: Subscription,
}

struct DatePick {
    /// Timestamps carry a time after the date; "Now" fills both
    timestamp: bool,
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
    /// Earlier and undone states of `pending`, for ⌘Z / ⇧⌘Z until the changes are submitted
    undo: Vec<PendingChanges>,
    redo: Vec<PendingChanges>,
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
            editor_bg: if theme.is_dark {
                theme.bg_surface
            } else {
                theme.bg_app
            },
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
            undo: Vec::new(),
            redo: Vec::new(),
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
        self.order
            .as_ref()
            .and_then(|o| o.get(display).copied())
            .unwrap_or(display)
    }

    /// Displayed columns, as actual indices, in display order
    fn visible(&self) -> Vec<usize> {
        self.order
            .clone()
            .unwrap_or_else(|| (0..self.result().map_or(0, |r| r.columns.len())).collect())
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
    pub fn foreign_key_target(
        &self,
        row: usize,
        display: usize,
    ) -> Option<(String, String, String)> {
        let col = self.actual(display);
        let name = self.column_name(col)?;
        let src = self.source.as_ref()?;
        let fk = src.foreign_keys.iter().find(|f| f.from_column == name)?;
        let value = self.cell_value(row, col).0.filter(|v| !v.is_null())?;
        let filter = format!(
            "{} = {}",
            src.dialect().quote_ident(&fk.to_column),
            src.dialect().literal(&value)
        );
        Some((fk.to_schema.clone(), fk.to_table.clone(), filter))
    }

    /// Copies one row (or all rows) of visible columns in the given format
    pub fn copy_text(
        &self,
        row: Option<usize>,
        format: quarry_core::export::ExportFormat,
        delimiter: char,
    ) -> Option<String> {
        let r = self.result()?;
        let rows: Vec<Vec<DbValue>> = match row {
            Some(row) => vec![(0..r.columns.len())
                .map(|c| self.cell_value(row, c).0.unwrap_or(DbValue::Null))
                .collect()],
            None => r.rows.clone(),
        };
        let table = self
            .source
            .as_ref()
            .map(|s| s.qualified_name())
            .unwrap_or_else(|| "query_result".into());
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
        String::from_utf8(buf)
            .ok()
            .map(|s| s.trim_end().to_string())
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
        self.source
            .as_ref()?
            .columns
            .iter()
            .find(|c| c.name == name)
    }

    /// Primary-key values of a loaded row, as loaded
    fn row_key(&self, row: usize) -> Option<RowKey> {
        let (src, r) = (self.source.as_ref()?, self.result()?);
        let values = r.rows.get(row)?;
        src.primary_keys
            .iter()
            .map(|pk| {
                Some((
                    pk.clone(),
                    values.get(r.columns.iter().position(|c| c == pk)?)?.clone(),
                ))
            })
            .collect()
    }

    /// Sorts by display column `col_ix`; `Default` restores the original order
    fn apply_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let col_ix = self.actual(col_ix);
        let Some(name) = self.result().and_then(|r| r.columns.get(col_ix)).cloned() else {
            return;
        };
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
        let Some(r) = self.results.get_mut(self.active) else {
            return;
        };
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

    fn open_header_menu(
        &mut self,
        col: usize,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let name = self
            .column_name(self.actual(col))
            .unwrap_or_default()
            .to_string();
        let grid = cx.entity().downgrade();
        let item = |label: &'static str,
                    f: fn(
            &mut ResultSet,
            usize,
            &mut Window,
            &mut Context<TableState<ResultSet>>,
        )| {
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
            menu.item(item("Sort Ascending", |rs, c, w, cx| {
                rs.apply_sort(c, ColumnSort::Ascending, w, cx)
            }))
            .item(item("Sort Descending", |rs, c, w, cx| {
                rs.apply_sort(c, ColumnSort::Descending, w, cx)
            }))
            .item(item("Clear Sort", |rs, c, w, cx| {
                rs.apply_sort(c, ColumnSort::Default, w, cx)
            }))
            .separator()
            .when(can_hide, |m| {
                m.item(item("Hide Column", |rs, c, _, _| rs.hide_column(c)))
            })
            .when(has_hidden, |m| {
                m.item(item("Show All Columns", |rs, _, _, _| {
                    rs.show_all_columns()
                }))
            })
            .item(
                gpui::component::menu::PopupMenuItem::new("Copy Column Name").on_click(
                    move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(name.clone())),
                ),
            )
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
            matches!(self.cell_value_ref(row, col).0, Some(DbValue::Text(ref l)) if matches!(l.as_str(), "ERROR" | "FATAL" | "PANIC"))
        })
    }

    fn row_state(&self, row: usize) -> RowState {
        if row >= self.loaded_len() {
            RowState::Inserted
        } else if self
            .row_key(row)
            .is_some_and(|k| self.pending.is_deleted(&k))
        {
            RowState::Deleted
        } else {
            RowState::Loaded
        }
    }

    /// Value of a pending-insert cell as a reference; `None` = left to the column DEFAULT
    fn inserted_value_ref(&self, row: usize, col: usize) -> Option<&DbValue> {
        let name = self.column_name(col)?;
        let insert = self
            .pending
            .row_inserts
            .get(row.checked_sub(self.loaded_len())?)?;
        insert.values.get(name)
    }

    /// The value a cell shows as a reference (pending edits win over loaded values) and whether it was edited
    fn cell_value_ref(&self, row: usize, col: usize) -> (Option<&DbValue>, bool) {
        if row >= self.loaded_len() {
            return (self.inserted_value_ref(row, col), false);
        }
        let Some(original) = self.result().and_then(|r| r.rows.get(row)?.get(col)) else {
            return (None, false);
        };
        // Fast path: if there are no pending cell edits, avoid allocating primary keys and searching edits
        if self.pending.cell_edits.is_empty() {
            return (Some(original), false);
        }
        let edited = self
            .row_key(row)
            .zip(self.column_name(col))
            .and_then(|(k, name)| self.pending.cell_value(&k, name));
        match edited {
            Some(v) => (Some(v), true),
            None => (Some(original), false),
        }
    }

    /// The value a cell shows (pending edits win over loaded values) and whether it was edited
    fn cell_value(&self, row: usize, col: usize) -> (Option<DbValue>, bool) {
        let (val, edited) = self.cell_value_ref(row, col);
        (val.cloned(), edited)
    }

    /// Call before changing `pending`; repeated calls with nothing changed in between are one step
    fn checkpoint(&mut self) {
        if self.undo.last() != Some(&self.pending) {
            self.undo.push(self.pending.clone());
        }
        self.redo.clear();
    }

    /// One step back (`redo == false`) or forward through pending changes
    pub fn step_history(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        // A checkpoint taken by a change that turned out to be a no-op isn't a step
        while from.last() == Some(&self.pending) {
            from.pop();
        }
        if let Some(prev) = from.pop() {
            to.push(std::mem::replace(&mut self.pending, prev));
            self.editing = None;
            self.notice = None;
        }
    }

    /// After a submit: what was pending is now in the database, nothing to step back to
    pub fn forget_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn set_value(&mut self, row: usize, col: usize, value: DbValue) {
        self.checkpoint();
        let Some(name) = self.column_name(col).map(str::to_string) else {
            return;
        };
        let loaded = self.loaded_len();
        if row >= loaded {
            if let Some(insert) = self.pending.row_inserts.get_mut(row - loaded) {
                insert.values.insert(name, value);
            }
        } else if let (Some(key), Some(original)) = (
            self.row_key(row),
            self.result()
                .and_then(|r| r.rows.get(row)?.get(col).cloned()),
        ) {
            self.pending.set_cell(key, &name, original, value);
        }
    }
}

/// DataGrip's pager: |< <  1–500 of ?  > >|. The range opens a page-size menu; "?" counts the rows.
fn pager(
    grid: &Entity<TableState<ResultSet>>,
    rs: &ResultSet,
    theme: &Theme,
) -> Option<impl IntoElement> {
    let src = rs.source.as_ref()?;
    let rows = rs.loaded_len();
    let (offset, page, has_more, total) = (src.offset, src.page_size, src.has_more, src.total);
    let busy = src.loading || src.counting;
    let nav = |id: &'static str, icon: IconName, tip: &'static str, enabled: bool, to: PageTo| {
        let grid = grid.clone();
        zbutton(id)
            .icon(icon)
            .ghost()
            .xsmall()
            .tooltip(tip)
            .disabled(!enabled || busy)
            .on_click(move |_, window, cx| {
                grid.update(cx, |t, cx| t.delegate_mut().go_to_page(to, window, cx))
            })
    };
    let range = if rows == 0 {
        "0 rows".to_string()
    } else {
        format!("{}–{}", group(offset + 1), group(offset + rows))
    };
    let sizes = {
        let grid = grid.clone();
        zbutton("grid-page-size")
            .pad_xs()
            .label(range)
            .ghost()
            .xsmall()
            .tooltip("Rows per page")
            .dropdown_menu(move |mut menu, _, _| {
                for n in [100, 250, 500, 1000, 5000] {
                    let grid = grid.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{} rows per page", group(n)))
                            .checked(n == page)
                            .on_click(move |_, window, cx| {
                                grid.update(cx, |t, cx| {
                                    t.delegate_mut().set_page_size(n, window, cx)
                                })
                            }),
                    );
                }
                menu
            })
    };
    let of = {
        let grid = grid.clone();
        let counted = total.is_some();
        div()
            .id("grid-row-count")
            .px_1()
            .rounded(px(3.0))
            .text_xs()
            .text_color(if counted {
                theme.text_secondary
            } else {
                theme.accent
            })
            .when(!counted, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .on_click(move |_, window, cx| {
                        grid.update(cx, |t, cx| t.delegate_mut().count_rows(false, window, cx))
                    })
            })
            .tooltip(move |window, cx| {
                Tooltip::new(if counted {
                    "Rows matching the filter"
                } else {
                    "Count rows"
                })
                .build(window, cx)
            })
            .child(match total {
                Some(n) => format!("of {}", group(n as usize)),
                None if src.counting => "of …".to_string(),
                None => "of ?".to_string(),
            })
    };
    Some(
        h_flex()
            .items_center()
            .gap_0p5()
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child(nav(
                "grid-first",
                IconName::ChevronsLeft,
                "First page",
                offset > 0,
                PageTo::First,
            ))
            .child(nav(
                "grid-prev",
                IconName::ChevronLeft,
                "Previous page",
                offset > 0,
                PageTo::Prev,
            ))
            .child(sizes)
            .child(of)
            .child(nav(
                "grid-next",
                IconName::ChevronRight,
                "Next page",
                has_more,
                PageTo::Next,
            ))
            .child(nav(
                "grid-last",
                IconName::ChevronsRight,
                "Last page",
                has_more,
                PageTo::Last,
            )),
    )
}

/// 12345 → "12,345"
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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
        on_follow_fk: impl Fn(&mut T, (String, String, String), &mut Window, &mut Context<T>)
            + Copy
            + 'static,
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
            (
                i.editor.clone(),
                col,
                i.row,
                read_only.is_some() || rs.row_state(i.row) == RowState::Deleted,
            )
        });

        let action_button = |id: &'static str,
                             icon: IconName,
                             tip: &'static str,
                             action: Box<dyn Action>| {
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
            .children(pager(grid, rs, theme))
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child(action_button(
                "grid-add",
                IconName::Plus,
                "Add row (⌘N)",
                Box::new(AddRow),
            ))
            .child(action_button(
                "grid-dup",
                IconName::Copy,
                "Duplicate row (⌘D)",
                Box::new(DuplicateRow),
            ))
            .child(action_button(
                "grid-del",
                IconName::Minus,
                "Delete row (⌘⌫)",
                Box::new(DeleteRow),
            ))
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child({
                let grid = grid.clone();
                zbutton("grid-value-editor")
                    .icon(IconName::PanelRight)
                    .ghost()
                    .xsmall()
                    .tooltip("Value editor (⇧↵)")
                    .on_click(move |_, window, cx| {
                        apply_row_action(&grid, &ToggleValueEditor, window, cx)
                    })
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
                    zbutton("grid-hidden")
                        .pad_xs()
                        .label(format!("{hidden} hidden"))
                        .ghost()
                        .xsmall()
                        .tooltip("Show all columns")
                        .on_click(move |_, window, cx| {
                            apply_row_action(&grid, &ShowAllColumns, window, cx)
                        }),
                )
            })
            .child(div().flex_1())
            .when_some(notice.clone(), |this, n| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(theme.danger)
                        .truncate()
                        .child(n),
                )
            })
            .when_some(read_only.filter(|_| notice.is_none()), |this, reason| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(reason),
                )
            })
            .when(pending_count > 0, |this| {
                let grid = grid.clone();
                this.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(format!(
                            "{} pending {}",
                            pending_count,
                            if pending_count == 1 {
                                "change"
                            } else {
                                "changes"
                            }
                        )),
                )
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
                    zbutton("grid-review")
                        .pad_xs()
                        .label("Review")
                        .ghost()
                        .xsmall()
                        .tooltip("Show the SQL (⌘S)")
                        .on_click(
                            cx.listener(move |this, _, window, cx| on_review(this, window, cx)),
                        ),
                )
                .child(
                    zbutton("grid-submit")
                        .pad_xs()
                        .icon(IconName::Check)
                        .label("Submit")
                        .primary()
                        .xsmall()
                        .tooltip("Apply in one transaction (⌘↵)")
                        .on_click(
                            cx.listener(move |this, _, window, cx| on_submit(this, window, cx)),
                        ),
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
                this.child(
                    deferred(anchored().position(pos).snap_to_window().child(menu))
                        .with_priority(1),
                )
            })
            .when(!is_table, |this| {
                this.when_some(notice.clone(), |this, n| {
                    this.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .border_b_1()
                            .border_color(theme.border)
                            .child(n),
                    )
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
                                .when(ix == active, |d| {
                                    d.bg(theme.bg_selected).text_color(theme.text_primary)
                                })
                                .when(ix != active, |d| {
                                    d.text_color(theme.text_muted)
                                        .hover(|s| s.bg(theme.bg_hover))
                                })
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
                    let target = g
                        .read(cx)
                        .selected_cell()
                        .and_then(|(row, col)| g.read(cx).delegate().foreign_key_target(row, col));
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
            .on_action({
                let g = grid.clone();
                move |_: &UndoChange, _, cx| {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().step_history(false);
                        t.refresh(cx);
                        cx.notify();
                    })
                }
            })
            .on_action({
                let g = grid.clone();
                move |_: &RedoChange, _, cx| {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().step_history(true);
                        t.refresh(cx);
                        cx.notify();
                    })
                }
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
                    let Some(ch) = ks
                        .key_char
                        .clone()
                        .filter(|c| !c.chars().any(char::is_control))
                    else {
                        return;
                    };
                    grid.update(cx, |t, cx| {
                        let Some((row, col)) = t.selected_cell() else {
                            return;
                        };
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
                                                .child(
                                                    div().text_color(theme.text_primary).child(col),
                                                )
                                                .child(
                                                    div()
                                                        .text_color(theme.text_muted)
                                                        .child(format!("row {}", row + 1)),
                                                ),
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
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .child(Editor::new(&editor).h_full().bordered(false)),
                                )
                                .when(!read_only, |this| {
                                    this.child(
                                        h_flex()
                                            .p_2()
                                            .justify_end()
                                            .border_t_1()
                                            .border_color(theme.border)
                                            .child(
                                                zbutton("value-editor-apply")
                                                    .pad_xs()
                                                    .label("Apply")
                                                    .primary()
                                                    .xsmall()
                                                    .tooltip("Write to the cell (⌘↵)")
                                                    .on_click(move |_, _, cx| {
                                                        g_apply.update(cx, |t, cx| {
                                                            t.delegate_mut().apply_inspector(cx)
                                                        })
                                                    }),
                                            ),
                                    )
                                }),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would shadow the std one
    use super::{PendingRowInsert, ResultSet, Theme};

    #[test]
    fn date_part_reads_the_leading_date() {
        let d = |s| super::editing::date_part(s).map(|d| d.to_string());
        assert_eq!(
            d("2026-09-18 13:54:48.480843+00").as_deref(),
            Some("2026-09-18")
        );
        assert_eq!(d("2026-02-30"), None);
        assert_eq!(d("soon"), None);
    }

    #[test]
    fn step_segment_moves_the_part_under_the_caret() {
        let step =
            |s, caret, d| super::editing::step_segment(s, caret, d).map(|(t, r)| (t, r.start));
        let ts = "2026-01-31 23:59:58.480843+00";
        assert_eq!(
            step(ts, 6, 1),
            Some(("2026-02-28 23:59:58.480843+00".into(), 5))
        );
        assert_eq!(
            step(ts, 15, 1),
            Some(("2026-02-01 00:00:58.480843+00".into(), 14))
        );
        assert_eq!(
            step(ts, 2, -1),
            Some(("2025-01-31 23:59:58.480843+00".into(), 0))
        );
        assert_eq!(
            step(ts, 25, 3),
            Some(("2026-02-01 00:00:01.480843+00".into(), 17))
        );
        assert_eq!(step("2026-03-01", 9, -1), Some(("2026-02-28".into(), 8)));
        assert_eq!(step("2026-03-01", 30, 1), Some(("2026-03-02".into(), 8)));
        assert_eq!(step("tomorrow", 2, 1), None);
    }

    #[test]
    fn date_readout_spells_it_out() {
        let r = super::editing::date_readout;
        assert_eq!(
            r("2026-09-18 13:54:48.480843+00"),
            ("Fri, 18 Sep 2026 · 13:54:48.480843+00".into(), true)
        );
        assert_eq!(r("2026-09-18"), ("Fri, 18 Sep 2026".into(), true));
        assert!(!r("2026-13-01").1);
        assert!(!r("2026-09-18 25:00").1);
    }

    #[test]
    fn paging_helpers() {
        assert_eq!(super::group(1234567), "1,234,567");
        assert_eq!(super::group(999), "999");
        assert_eq!(super::editing::last_page_offset(1001, 500), 1000);
        assert_eq!(super::editing::last_page_offset(1000, 500), 500);
        assert_eq!(super::editing::last_page_offset(0, 500), 0);
    }

    #[test]
    fn undo_redo_steps_through_pending() {
        let mut rs = ResultSet::new(&Theme::dark(), None);
        let insert = |n: &str| PendingRowInsert {
            temp_id: n.into(),
            values: Default::default(),
        };
        rs.checkpoint();
        rs.pending.row_inserts.push(insert("a"));
        rs.checkpoint();
        rs.checkpoint(); // no-op change: not a step
        rs.pending.row_inserts.push(insert("b"));
        rs.step_history(false);
        assert_eq!(rs.pending.row_inserts.len(), 1);
        rs.step_history(false);
        assert!(rs.pending.is_empty());
        rs.step_history(false); // nothing left
        assert!(rs.pending.is_empty());
        rs.step_history(true);
        rs.step_history(true);
        assert_eq!(rs.pending.row_inserts.len(), 2);
        rs.step_history(false);
        rs.checkpoint(); // a new change drops the redo branch
        rs.step_history(true);
        assert_eq!(rs.pending.row_inserts.len(), 1);
    }
}
