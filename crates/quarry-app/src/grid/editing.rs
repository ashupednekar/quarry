// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

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
            input.update(cx, |i, cx| {
                i.set_cursor_position(Position::new(0, end), window, cx)
            });
        }
        input.update(cx, |i, cx| i.focus(window, cx));

        let error = if seed.is_some() {
            parse_input(&text, self.column_schema(col)).err()
        } else {
            None
        };

        let sub = cx.subscribe_in(
            &input,
            window,
            |table, _, ev: &InputEvent, window, cx| match ev {
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
            },
        );
        self.editing = Some(CellEditor {
            row,
            col,
            display_col,
            input,
            initial,
            error,
            _sub: sub,
        });
        self.selected_cell = Some((row, display_col));
        self.notice = None;
        cx.notify();
    }

    pub(super) fn validate_current_edit(&mut self, cx: &mut Context<TableState<Self>>) {
        let (col, text, initial) = match self.editing.as_ref() {
            Some(ed) => (
                ed.col,
                ed.input.read(cx).value().to_string(),
                ed.initial.clone(),
            ),
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
        self.inspector = Some(Inspector {
            row,
            col,
            editor,
            initial: text,
        });
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

    /// Loads the first page (`reset`), the next page, or re-reads what's loaded in place (`reload`).
    pub fn fetch(&mut self, mode: Fetch, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        let loaded = self.loaded_len();
        let page = self.source.as_ref().map_or(500, |s| s.page_size.max(1));
        let (offset, limit) = match mode {
            Fetch::Reset => (0, page),
            Fetch::More => (loaded, page),
            Fetch::Reload => (0, loaded.max(page)),
        };
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.loading {
            return;
        }
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
                        match rs.results.first_mut() {
                            Some(existing) if mode == Fetch::More => {
                                existing.rows.append(&mut page.rows)
                            }
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
            },
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Fetch {
    Reset,
    More,
    Reload,
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
