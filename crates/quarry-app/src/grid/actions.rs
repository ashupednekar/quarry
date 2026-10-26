// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! The grid's context menu, row actions and keyboard cell navigation

use super::*;

/// The grid's right-click menu (DataGrip's cell menu)
pub fn grid_menu(menu: PopupMenu, rs: &ResultSet) -> PopupMenu {
    let has_fks = rs
        .source
        .as_ref()
        .is_some_and(|s| !s.foreign_keys.is_empty());
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
        .when(rs.source.is_some(), |m| {
            m.menu("Filter by Value", Box::new(FilterByValue))
                .menu("Filter Excluding Value", Box::new(FilterExcludeValue))
                .menu("Filter Builder…", Box::new(OpenFilterBuilder))
                .separator()
        })
        .menu("Hide Column", Box::new(HideColumn))
        .when(rs.order.is_some(), |m| {
            m.menu("Show All Columns", Box::new(ShowAllColumns))
        })
        .when(has_fks, |m| {
            m.menu("Go to Referenced Row", Box::new(FollowForeignKey))
        })
        .menu("Transposed Row View", Box::new(ToggleTransposedView))
        .menu("Open in Value Editor", Box::new(ToggleValueEditor));
    if rs.read_only_reason().is_some() {
        return menu;
    }
    // A multi-cell selection: the edits apply to every selected cell / row
    let (cells, rows) = rs.selected_range.filter(|r| !r.is_single_cell()).map_or(
        (String::new(), String::new()),
        |r| {
            (
                format!(" ({} cells)", r.cell_count()),
                format!(" ({} rows)", r.row_count()),
            )
        },
    );
    menu.separator()
        .menu("Edit Cell", Box::new(EditCell))
        .menu(format!("Set NULL{cells}"), Box::new(SetNull))
        .menu(format!("Set DEFAULT{cells}"), Box::new(SetDefault))
        .separator()
        .menu("Add Row", Box::new(AddRow))
        .menu("Duplicate Row", Box::new(DuplicateRow))
        .menu(format!("Delete Row{rows}"), Box::new(DeleteRow))
}

/// Set NULL / Set DEFAULT / Delete Row over a multi-cell selection, as one undo step.
/// Returns false when there's no such selection or the action isn't one of these.
fn apply_to_range(rs: &mut ResultSet, any: &dyn std::any::Any) -> bool {
    let Some(r) = rs.selected_range.filter(|r| !r.is_single_cell()) else {
        return false;
    };
    let (null, default, delete) = (
        any.is::<SetNull>(),
        any.is::<SetDefault>(),
        any.is::<DeleteRow>(),
    );
    if !(null || default || delete) {
        return false;
    }
    let total = rs.total_row_count();
    let cols = rs.visible().len();
    if total == 0 || cols == 0 {
        return true;
    }
    let row_range = r.min_row()..=r.max_row().min(total - 1);
    let undo_len = rs.undo.len();
    if delete {
        // Bottom-up: removing a pending insert shifts the rows after it
        for row in row_range.rev() {
            rs.delete_row(row);
        }
    } else {
        for row in row_range {
            for col in r.min_col()..=r.max_col().min(cols - 1) {
                if null {
                    rs.set_null(row, col);
                } else {
                    rs.set_default(row, col);
                }
            }
        }
    }
    rs.undo.truncate(undo_len + 1);
    true
}

/// Row/cell operations shared by keyboard actions, the context menu and the toolbar.
/// They target the selected cell, or the right-clicked row.
pub fn apply_row_action(
    grid: &Entity<TableState<ResultSet>>,
    action: &dyn Action,
    window: &mut Window,
    cx: &mut App,
) {
    grid.update(cx, |table, cx| {
        let target = table
            .selected_cell()
            .or_else(|| table.right_clicked_row().map(|r| (r, 0)));
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
        let whole = any.is::<CopyAllTsv>()
            || any.is::<CopyAllCsv>()
            || any.is::<CopyAllJson>()
            || any.is::<CopyAllInsert>();
        if apply_to_range(rs, any) {
            rs.finish_edit(false, cx);
            table.refresh(cx);
            cx.notify();
            return;
        }
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
            _ if any.is::<CopyValue>() => {
                clipboard = if let Some(range) =
                    rs.selected_range.as_ref().filter(|r| !r.is_single_cell())
                {
                    Some(rs.copy_range_tsv(range))
                } else if let Some((row, col)) = target {
                    rs.cell_value(row, rs.actual(col)).0.map(|v| {
                        if v.is_null() {
                            String::new()
                        } else {
                            v.to_display_string()
                        }
                    })
                } else {
                    None
                };
                None
            }
            _ if any.is::<SelectRangeUp>() => {
                let total_rows = rs.rows_count(cx);
                if total_rows > 0 {
                    let mut range = rs.selected_range.unwrap_or_else(|| {
                        let (r, c) = rs.selected_cell.unwrap_or((0, 0));
                        CellRange::single(r, c)
                    });
                    if range.focus_row > 0 {
                        range.focus_row -= 1;
                        let focus = (range.focus_row, range.focus_col);
                        rs.selected_range = Some(range);
                        rs.selected_cell = Some(focus);
                        table.scroll_to_row(focus.0, cx);
                    }
                }
                None
            }
            _ if any.is::<SelectRangeDown>() => {
                let total_rows = rs.rows_count(cx);
                if total_rows > 0 {
                    let mut range = rs.selected_range.unwrap_or_else(|| {
                        let (r, c) = rs.selected_cell.unwrap_or((0, 0));
                        CellRange::single(r, c)
                    });
                    if range.focus_row + 1 < total_rows {
                        range.focus_row += 1;
                        let focus = (range.focus_row, range.focus_col);
                        rs.selected_range = Some(range);
                        rs.selected_cell = Some(focus);
                        table.scroll_to_row(focus.0, cx);
                    }
                }
                None
            }
            _ if any.is::<SelectRangeLeft>() => {
                let visible_cols = rs.visible().len();
                if visible_cols > 0 {
                    let mut range = rs.selected_range.unwrap_or_else(|| {
                        let (r, c) = rs.selected_cell.unwrap_or((0, 0));
                        CellRange::single(r, c)
                    });
                    if range.focus_col > 0 {
                        range.focus_col -= 1;
                        let focus = (range.focus_row, range.focus_col);
                        rs.selected_range = Some(range);
                        rs.selected_cell = Some(focus);
                    }
                }
                None
            }
            _ if any.is::<SelectRangeRight>() => {
                let visible_cols = rs.visible().len();
                if visible_cols > 0 {
                    let mut range = rs.selected_range.unwrap_or_else(|| {
                        let (r, c) = rs.selected_cell.unwrap_or((0, 0));
                        CellRange::single(r, c)
                    });
                    if range.focus_col + 1 < visible_cols {
                        range.focus_col += 1;
                        let focus = (range.focus_row, range.focus_col);
                        rs.selected_range = Some(range);
                        rs.selected_cell = Some(focus);
                    }
                }
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
            _ if any.is::<ToggleTransposedView>() => {
                rs.toggle_transposed_view(window, cx);
                None
            }
            _ if any.is::<TransposedNextRow>() => {
                rs.transposed_next_row();
                None
            }
            _ if any.is::<TransposedPrevRow>() => {
                rs.transposed_prev_row();
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
        table
            .delegate_mut()
            .start_edit(next_row, next_col, None, window, cx);
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
        table
            .delegate_mut()
            .start_edit(prev_row, prev_col, None, window, cx);
        cx.notify();
    });
}
