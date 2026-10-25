// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Cell editing, row inserts and deletes, the inspector and paging

use super::*;

impl ResultSet {
    /// Opens the in-cell editor. `seed` replaces the value (type-to-edit); otherwise the value is kept.
    pub fn start_edit(
        &mut self,
        row: usize,
        display_col: usize,
        seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        // Enter inside the editor also bubbles up as EditCell; the open editor must win so it can commit.
        // Another cell, though, means moving on: save this one and open that.
        if let Some(ed) = self.editing.as_ref() {
            if (ed.row, ed.display_col) == (row, display_col) {
                return;
            }
            self.finish_edit(true, cx);
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
        let is_null = self
            .cell_value(row, col)
            .0
            .as_ref()
            .is_some_and(DbValue::is_null);
        let initial = self
            .cell_value(row, col)
            .0
            .map(|v| edit_text(&v))
            .unwrap_or_default();
        let text = seed.clone().unwrap_or_else(|| initial.clone());
        let numeric = matches!(
            self.cell_value(row, col).0,
            Some(DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_))
        );
        // Same setup for either kind of input
        macro_rules! prime {
            ($input:expr) => {{
                let input = $input;
                if seed.is_none() {
                    input.update(cx, |i, cx| i.select_all(window, cx));
                } else {
                    let end = text.chars().count() as u32;
                    input.update(cx, |i, cx| {
                        i.set_cursor_position(Position::new(0, end), window, cx)
                    });
                }
                input.update(cx, |i, cx| i.focus(window, cx));
                cx.subscribe_in(&input, window, |table, _, ev: &InputEvent, window, cx| {
                    on_cell_input(table, ev, window, cx)
                })
            }};
        }
        let (input, sub) = if numeric {
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx).default_value(text.clone());
                if is_null {
                    state = state.placeholder("<null>");
                }
                state
            });
            let sub = prime!(input.clone());
            (CellInput::Number(input), sub)
        } else {
            let input = cx.new(|cx| {
                // Enter commits, ⇧↵ adds a line; grows to 8 rows once it pops out
                let mut state = TextareaState::new(window, cx)
                    .auto_grow(1, 8)
                    .submit_on_enter(true)
                    .default_value(text.clone());
                if is_null {
                    state = state.placeholder("<null>");
                }
                state
            });
            let sub = prime!(input.clone());
            (CellInput::Text(input), sub)
        };

        let date = match (&input, self.column_schema(col).map(|c| c.data_type.clone())) {
            (CellInput::Text(_), Some(kind @ (ColumnType::Date | ColumnType::Timestamp))) => {
                Some(DatePick {
                    timestamp: kind == ColumnType::Timestamp,
                })
            }
            _ => None,
        };

        let error = if seed.is_some() {
            parse_input(&text, self.column_schema(col)).err()
        } else {
            None
        };

        self.editing = Some(CellEditor {
            row,
            col,
            display_col,
            input,
            cell_w: Default::default(),
            date,
            initial,
            error,
            _sub: sub,
        });
        self.selected_cell = Some((row, display_col));
        self.notice = None;
        cx.notify();
    }

    /// "Today" / "Now": the current date, or date and time for a timestamp
    pub(super) fn pick_now(&mut self, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let Some(ed) = self.editing.as_ref() else {
            return;
        };
        let (CellInput::Text(input), Some(pick)) = (&ed.input, ed.date.as_ref()) else {
            return;
        };
        let now = chrono::Local::now();
        let text = if pick.timestamp {
            now.format("%Y-%m-%d %H:%M:%S").to_string()
        } else {
            now.format("%Y-%m-%d").to_string()
        };
        input.update(cx, |i, cx| {
            i.set_value(text, window, cx);
            i.focus(window, cx);
        });
        self.validate_current_edit(cx);
    }

    /// ↑/↓ in a date cell: steps the year, month, day, hour, minute or second under the caret and
    /// selects it, so repeated presses keep going. `false` when the text isn't a date (the key
    /// then does what it normally does).
    pub(super) fn step_date(
        &mut self,
        delta: i64,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        let Some(CellInput::Text(input)) = self
            .editing
            .as_ref()
            .filter(|e| e.date.is_some())
            .map(|e| &e.input)
        else {
            return false;
        };
        let (text, caret) = {
            let i = input.read(cx);
            (i.value().to_string(), i.cursor())
        };
        let Some((next, range)) = step_segment(&text, caret, delta) else {
            return false;
        };
        input.update(cx, |i, cx| {
            i.set_value(next, window, cx);
            i.set_selected_range(range, cx);
        });
        self.validate_current_edit(cx);
        true
    }

    pub(super) fn validate_current_edit(&mut self, cx: &mut Context<TableState<Self>>) {
        let (col, text, initial) = match self.editing.as_ref() {
            Some(ed) => (ed.col, ed.input.value(cx), ed.initial.clone()),
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
        let Some(ed) = self.editing.take() else {
            return;
        };
        let text = ed.input.value(cx);
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

    pub(super) fn editable(&mut self) -> bool {
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
        self.checkpoint();
        self.pending.row_inserts.push(PendingRowInsert {
            temp_id: uuid::Uuid::new_v4().to_string(),
            values: Default::default(),
        });
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
        self.checkpoint();
        self.pending.row_inserts.push(PendingRowInsert {
            temp_id: uuid::Uuid::new_v4().to_string(),
            values,
        });
        Some(self.loaded_len() + self.pending.row_inserts.len() - 1)
    }

    /// Marks a loaded row for deletion (toggle), or drops a pending insert
    pub fn delete_row(&mut self, row: usize) {
        if !self.editable() {
            return;
        }
        self.checkpoint();
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
        let Some(name) = self.column_name(col).map(str::to_string) else {
            return;
        };
        self.checkpoint();
        let loaded = self.loaded_len();
        if row >= loaded {
            if let Some(insert) = self.pending.row_inserts.get_mut(row - loaded) {
                insert.values.remove(&name);
            }
            return;
        }
        let expr = if self
            .source
            .as_ref()
            .is_some_and(|s| s.dialect().supports_default_keyword())
        {
            Some("DEFAULT".to_string())
        } else {
            self.column_schema(col)
                .map(|c| c.default_value.clone().unwrap_or_else(|| "NULL".into()))
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

    /// Drops every pending change; ⌘Z brings them back
    pub fn revert(&mut self) {
        self.checkpoint();
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
    pub fn inspect(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let col = self.actual(col);
        let value = self.cell_value(row, col).0;
        let is_json = matches!(value, Some(DbValue::Json(_)))
            || self
                .column_schema(col)
                .is_some_and(|c| c.data_type == ColumnType::Json);
        let text = match &value {
            Some(DbValue::Json(j)) => serde_json::to_string_pretty(j).unwrap_or_default(),
            Some(v) => edit_text(v),
            None => String::new(),
        };
        let read_only =
            self.read_only_reason().is_some() || self.row_state(row) == RowState::Deleted;
        let editor = cx.new(|cx| {
            let e = EditorState::new(window, cx)
                .soft_wrap(true)
                .default_value(text.clone());
            if is_json {
                e.language("json")
            } else {
                e
            }
        });
        editor.update(cx, |e, cx| e.set_readonly(read_only, cx));
        let parsed_json = serde_json::from_str::<serde_json::Value>(&text).ok();
        let is_valid_json = parsed_json.is_some();
        let search_input = if is_valid_json {
            Some(cx.new(|cx| InputState::new(window, cx).placeholder("Filter JSON…")))
        } else {
            None
        };
        let mut expanded_paths = std::collections::HashSet::new();
        expanded_paths.insert("$".to_string());
        if let Some(serde_json::Value::Object(map)) = &parsed_json {
            for k in map.keys() {
                expanded_paths.insert(format!("$.{k}"));
            }
        } else if let Some(serde_json::Value::Array(arr)) = &parsed_json {
            for i in 0..arr.len().min(10) {
                expanded_paths.insert(format!("$[{i}]"));
            }
        }

        self.inspector = Some(Inspector {
            row,
            col,
            editor,
            initial: text,
            mode: if is_valid_json {
                InspectorMode::Tree
            } else {
                InspectorMode::Text
            },
            parsed_json,
            expanded_paths,
            search_input,
        });
    }

    pub fn set_inspector_mode(&mut self, mode: InspectorMode, _window: &mut Window, cx: &mut App) {
        if let Some(ins) = self.inspector.as_mut() {
            if mode == InspectorMode::Tree {
                let text = ins.editor.read(cx).value().to_string();
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    ins.parsed_json = Some(val);
                    ins.mode = InspectorMode::Tree;
                    self.notice = None;
                } else {
                    self.notice = Some("Cannot switch to Tree view: JSON syntax error".to_string());
                }
            } else {
                ins.mode = InspectorMode::Text;
            }
        }
    }

    pub fn format_inspector_json(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(ins) = self.inspector.as_mut() {
            let text = ins.editor.read(cx).value().to_string();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                    ins.editor
                        .update(cx, |e, cx| e.set_value(pretty, window, cx));
                    ins.parsed_json = Some(val);
                }
            }
        }
    }

    pub fn minify_inspector_json(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(ins) = self.inspector.as_mut() {
            let text = ins.editor.read(cx).value().to_string();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Ok(compact) = serde_json::to_string(&val) {
                    ins.editor
                        .update(cx, |e, cx| e.set_value(compact, window, cx));
                    ins.parsed_json = Some(val);
                }
            }
        }
    }

    pub fn toggle_json_tree_path(&mut self, path: String) {
        if let Some(ins) = self.inspector.as_mut() {
            if ins.expanded_paths.contains(&path) {
                ins.expanded_paths.remove(&path);
            } else {
                ins.expanded_paths.insert(path);
            }
        }
    }

    pub fn expand_all_json_tree(&mut self) {
        if let Some(ins) = self.inspector.as_mut() {
            if let Some(val) = &ins.parsed_json {
                fn collect_paths(
                    v: &serde_json::Value,
                    path: String,
                    set: &mut std::collections::HashSet<String>,
                ) {
                    set.insert(path.clone());
                    match v {
                        serde_json::Value::Object(map) => {
                            for (k, child) in map {
                                collect_paths(child, format!("{path}.{k}"), set);
                            }
                        }
                        serde_json::Value::Array(arr) => {
                            for (i, child) in arr.iter().enumerate() {
                                collect_paths(child, format!("{path}[{i}]"), set);
                            }
                        }
                        _ => {}
                    }
                }
                collect_paths(val, "$".to_string(), &mut ins.expanded_paths);
            }
        }
    }

    pub fn collapse_all_json_tree(&mut self) {
        if let Some(ins) = self.inspector.as_mut() {
            ins.expanded_paths.clear();
            ins.expanded_paths.insert("$".to_string());
        }
    }

    /// Writes the value editor's text back into the cell (validated like any edit)
    pub fn apply_inspector(&mut self, cx: &mut Context<TableState<Self>>) {
        let Some(ins) = self.inspector.as_ref() else {
            return;
        };
        let text = ins.editor.read(cx).value().to_string();
        let (row, col, unchanged) = (ins.row, ins.col, text == ins.initial);
        if unchanged || !self.editable() {
            return;
        }
        let is_json = matches!(self.cell_value(row, col).0, Some(DbValue::Json(_)));
        let parsed = if is_json {
            serde_json::from_str(&text)
                .map(DbValue::Json)
                .map_err(|e| format!("Invalid JSON: {e}"))
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

    /// Loads the first page (`Reset`, e.g. after a new filter), the page at a row offset, or the
    /// page on screen again (`Reload`).
    pub fn fetch(&mut self, mode: Fetch, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.loading {
            return;
        }
        let limit = src.page_size.max(1);
        let offset = match mode {
            Fetch::Reset => {
                src.total = None; // the filter or sort may have changed
                0
            }
            Fetch::Page(offset) => offset,
            Fetch::Reload => src.offset,
        };
        src.loading = true;
        src.error = None;
        let (session, sql) = (src.session.clone(), src.page_sql(offset, limit));

        run_db(
            window,
            cx,
            async move { session.execute(&sql).await },
            move |table, res, _, cx| {
                let rs = table.delegate_mut();
                let Some(src) = rs.source.as_mut() else {
                    return;
                };
                src.loading = false;
                match res {
                    Ok(mut page) => {
                        src.has_more = page.rows.len() > limit;
                        page.rows.truncate(limit);
                        src.offset = offset;
                        if !src.has_more {
                            // The last page tells the total for free
                            src.total = Some((offset + page.rows.len()) as u64);
                        }
                        rs.set_results(vec![page]);
                    }
                    Err(e) => src.error = Some(e.to_string()),
                }
                table.refresh(cx);
                if mode != Fetch::Reload {
                    table.scroll_to_row(0, cx);
                }
            },
        );
    }

    /// Moves to another page. Refused while edits are pending: they belong to the rows on screen.
    pub fn go_to_page(
        &mut self,
        to: PageTo,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(src) = self.source.as_ref() else {
            return;
        };
        if !self.pending.is_empty() {
            self.notice = Some("Submit or revert the pending changes before changing page".into());
            cx.notify();
            return;
        }
        let page = src.page_size.max(1);
        let offset = match to {
            PageTo::First => 0,
            PageTo::Prev => src.offset.saturating_sub(page),
            PageTo::Next => src.offset + page,
            PageTo::Last => match src.total {
                Some(total) => last_page_offset(total, page),
                // Count first, then come back here
                None => return self.count_rows(true, window, cx),
            },
        };
        self.fetch(Fetch::Page(offset), window, cx);
    }

    pub fn set_page_size(
        &mut self,
        size: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if let Some(src) = self.source.as_mut() {
            src.page_size = size;
        }
        self.go_to_page(PageTo::First, window, cx);
    }

    /// Counts the rows matching the filter; `then_last` jumps to the last page afterwards
    pub fn count_rows(
        &mut self,
        then_last: bool,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.counting {
            return;
        }
        src.counting = true;
        let (session, sql) = (src.session.clone(), src.count_sql());
        cx.notify();
        run_db(
            window,
            cx,
            async move { session.execute(&sql).await },
            move |table, res, window, cx| {
                let rs = table.delegate_mut();
                let Some(src) = rs.source.as_mut() else {
                    return;
                };
                src.counting = false;
                match res {
                    Ok(r) => {
                        src.total = r
                            .rows
                            .first()
                            .and_then(|row| row.first())
                            .and_then(|v| v.to_display_string().parse().ok());
                        if then_last {
                            rs.go_to_page(PageTo::Last, window, cx);
                        }
                    }
                    Err(e) => src.error = Some(e.to_string()),
                }
                cx.notify();
            },
        );
    }
}

/// Offset of the last full-or-partial page
pub(super) fn last_page_offset(total: u64, page: usize) -> usize {
    let total = total as usize;
    if total == 0 {
        0
    } else {
        (total - 1) / page * page
    }
}

/// Enter commits and moves down, ⌘↵ commits in place, ⇧↵ (text only) adds a line; leaving commits
fn on_cell_input(
    table: &mut TableState<ResultSet>,
    ev: &InputEvent,
    window: &mut Window,
    cx: &mut Context<TableState<ResultSet>>,
) {
    match ev {
        InputEvent::PressEnter { shift, secondary } => {
            if *secondary {
                table.delegate_mut().finish_edit(true, cx);
                window.focus(&table.focus_handle(cx), cx);
            } else if *shift {
                // The newline is already in; keep editing
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
    }
}

/// The `YYYY-MM-DD` a date or timestamp text starts with
pub(super) fn date_part(text: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
}

/// The `HH:MM:SS` of a `YYYY-MM-DD HH:MM:SS…` text
fn time_part(text: &str) -> Option<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(text.get(11..19)?, "%H:%M:%S").ok()
}

/// Steps the part of a `YYYY-MM-DD[ HH:MM:SS…]` text under byte offset `caret` by `delta`, with
/// calendar arithmetic (Jan 31 + 1 month = Feb 28; 23:59 + 1 minute is tomorrow). Anything after
/// the seconds (fraction, zone) is kept. Returns the new text and the stepped part's range.
pub(super) fn step_segment(
    text: &str,
    caret: usize,
    delta: i64,
) -> Option<(String, std::ops::Range<usize>)> {
    use chrono::{Duration, Months};
    let date = date_part(text)?;
    let time = time_part(text);
    let (end, last) = if time.is_some() { (19, 5) } else { (10, 2) };
    let part = match caret {
        0..=4 => 0,
        5..=7 => 1,
        8..=10 => 2,
        11..=13 => 3,
        14..=16 => 4,
        _ => 5,
    }
    .min(last);
    let at = date.and_time(time.unwrap_or_default());
    let months = |n: i64| {
        let m = Months::new(n.unsigned_abs() as u32);
        if n >= 0 {
            at.checked_add_months(m)
        } else {
            at.checked_sub_months(m)
        }
    };
    let next = match part {
        0 => months(12 * delta)?,
        1 => months(delta)?,
        2 => at + Duration::days(delta),
        3 => at + Duration::hours(delta),
        4 => at + Duration::minutes(delta),
        _ => at + Duration::seconds(delta),
    };
    let format = if time.is_some() {
        "%Y-%m-%d %H:%M:%S"
    } else {
        "%Y-%m-%d"
    };
    let ranges = [0..4, 5..7, 8..10, 11..13, 14..16, 17..19];
    Some((
        format!("{}{}", next.format(format), &text[end..]),
        ranges[part].clone(),
    ))
}

/// The value read back in words ("Fri, 18 Sep 2026 · 13:54:48+00"), and whether it's a date at all
pub(super) fn date_readout(text: &str) -> (String, bool) {
    let text = text.trim();
    if text.is_empty() {
        return ("Empty".into(), true);
    }
    let invalid = || ("Not a date · use YYYY-MM-DD HH:MM:SS".to_string(), false);
    let Some(date) = date_part(text) else {
        return invalid();
    };
    let day = date.format("%a, %-d %b %Y").to_string();
    match (text.len(), time_part(text)) {
        (10, _) => (day, true),
        (_, Some(time)) => (
            format!("{day} · {}{}", time.format("%H:%M:%S"), &text[19..]),
            true,
        ),
        _ => invalid(),
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Fetch {
    Reset,
    Page(usize),
    Reload,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PageTo {
    First,
    Prev,
    Next,
    Last,
}

/// Text shown in the in-cell editor for a value. NULL edits start empty.
pub(super) fn edit_text(v: &DbValue) -> String {
    match v {
        DbValue::Null => String::new(),
        v => v.to_display_string(),
    }
}

/// Parses editor text into a value of the column's type (E7)
pub(super) fn parse_input(text: &str, col: Option<&ColumnSchema>) -> Result<DbValue, String> {
    let Some(col) = col else {
        return Ok(DbValue::Text(text.to_string()));
    };
    let bad = |what: &str| format!("\"{}\" is not a valid {} for {}", text, what, col.name);
    let t = text.trim();
    if col.is_nullable
        && (t.is_empty() || t.eq_ignore_ascii_case("null") || t.eq_ignore_ascii_case("<null>"))
        && col.data_type != ColumnType::Text
    {
        return Ok(DbValue::Null);
    }
    if col.is_nullable && (t.eq_ignore_ascii_case("<null>") || t == "\\N") {
        return Ok(DbValue::Null);
    }
    match &col.data_type {
        ColumnType::Integer | ColumnType::BigInt => {
            t.parse().map(DbValue::Int).map_err(|_| bad("integer"))
        }
        ColumnType::Float
            if col.raw_type_name.to_lowercase().contains("numeric")
                || col.raw_type_name.to_lowercase().contains("decimal") =>
        {
            t.parse::<f64>()
                .map(|_| DbValue::Decimal(t.to_string()))
                .map_err(|_| bad("number"))
        }
        ColumnType::Float => t.parse().map(DbValue::Float).map_err(|_| bad("number")),
        ColumnType::Boolean => match t.to_lowercase().as_str() {
            "true" | "t" | "1" | "yes" => Ok(DbValue::Boolean(true)),
            "false" | "f" | "0" | "no" => Ok(DbValue::Boolean(false)),
            _ => Err(bad("boolean")),
        },
        ColumnType::Json => serde_json::from_str(t)
            .map(DbValue::Json)
            .map_err(|e| format!("Invalid JSON for {}: {}", col.name, e)),
        _ => Ok(DbValue::Text(text.to_string())),
    }
}
