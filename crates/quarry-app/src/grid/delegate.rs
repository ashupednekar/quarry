// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! How the table component draws and sorts a result set

use super::*;

impl TableDelegate for ResultSet {
    fn columns_count(&self, _: &App) -> usize {
        self.order
            .as_ref()
            .map_or_else(|| self.result().map_or(0, |r| r.columns.len()), Vec::len)
    }

    fn rows_count(&self, _: &App) -> usize {
        self.loaded_len()
            + if self.source.is_some() {
                self.pending.row_inserts.len()
            } else {
                0
            }
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let r = self
            .result()
            .expect("column() is only called when columns exist");
        let col_ix = self.actual(col_ix);
        let numeric = r
            .rows
            .first()
            .and_then(|row| row.get(col_ix))
            .is_some_and(|v| {
                matches!(v, DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_))
            });
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
        let max = if matches!(name.as_str(), "statement" | "query") {
            self.max_col_width.max(720.0)
        } else {
            self.max_col_width
        };
        // ~8.4px per monospace character at the grid's text size, plus cell padding
        let width = (chars as f32 * 8.4 + 32.0).clamp(64.0, max);
        let col = Column::new(col_ix.to_string(), name.clone()).width(px(width));
        let col = match &self.sort {
            Some((n, ColumnSort::Ascending)) if n == name => col.ascending(),
            Some((n, ColumnSort::Descending)) if n == name => col.descending(),
            _ => col.sortable(),
        };
        if numeric {
            col.text_right()
        } else {
            col
        }
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let grid = cx.entity().downgrade();
        div()
            .id(("th", col_ix))
            .size_full()
            .on_mouse_down(
                MouseButton::Right,
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let pos = ev.position;
                    grid.update(cx, |t, cx| {
                        t.delegate_mut().open_header_menu(col_ix, pos, window, cx)
                    })
                    .ok();
                },
            )
            .child(self.column(col_ix, cx).name.clone())
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        _: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(name) = self
            .result()
            .and_then(|r| r.columns.get(self.actual(col_ix)))
            .cloned()
        else {
            return;
        };
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
        self.source
            .as_ref()
            .is_some_and(|s| s.has_more && !s.loading)
    }

    fn load_more(&mut self, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.fetch(Fetch::More, window, cx);
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let mut order = self.visible();
        if col_ix < order.len() && to_ix < order.len() {
            let c = order.remove(col_ix);
            order.insert(to_ix, c);
            self.order = Some(order);
        }
    }

    fn context_menu(
        &mut self,
        _row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        grid_menu(menu, self)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let c = self.colors;
        let display_col = col_ix;
        let col_ix = self.actual(col_ix);
        let is_selected = self.selected_cell == Some((row_ix, display_col));
        // Foreign-key cells look like links (F4 follows them)
        let is_fk = self.column_name(col_ix).is_some_and(|n| {
            self.source
                .as_ref()
                .is_some_and(|s| s.foreign_keys.iter().any(|f| f.from_column == n))
        });
        let mono = cx.theme().mono_font_family.clone();
        if let Some(ed) = self
            .editing
            .as_ref()
            .filter(|e| e.row == row_ix && e.col == col_ix)
        {
            let error = ed.error.clone();
            let has_error = error.is_some();
            let is_bool = self
                .column_schema(col_ix)
                .is_some_and(|c| c.data_type == ColumnType::Boolean);
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
                                .tooltip(move |window, cx| {
                                    Tooltip::new("Set true").build(window, cx)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |table, _, _, cx| {
                                        table.delegate_mut().editing = None;
                                        table.delegate_mut().set_value(
                                            row_ix,
                                            col_ix,
                                            DbValue::Boolean(true),
                                        );
                                        table.refresh(cx);
                                        cx.notify();
                                    }),
                                )
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
                                .tooltip(move |window, cx| {
                                    Tooltip::new("Set false").build(window, cx)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |table, _, _, cx| {
                                        table.delegate_mut().editing = None;
                                        table.delegate_mut().set_value(
                                            row_ix,
                                            col_ix,
                                            DbValue::Boolean(false),
                                        );
                                        table.refresh(cx);
                                        cx.notify();
                                    }),
                                )
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |table, _, _, cx| {
                                table.delegate_mut().editing = None;
                                table.delegate_mut().set_null(row_ix, display_col);
                                table.refresh(cx);
                                cx.notify();
                            }),
                        )
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
                .tooltip(move |window, cx| {
                    Tooltip::new("Open in Value Editor (⇧↵)").build(window, cx)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |table, _, window, cx| {
                        table.delegate_mut().finish_edit(true, cx);
                        table
                            .delegate_mut()
                            .inspect(row_ix, display_col, window, cx);
                        table.refresh(cx);
                        cx.notify();
                    }),
                )
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
        let numeric = matches!(
            value,
            Some(DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_))
        );
        let token = |label: SharedString, fg: Rgba| {
            div()
                .px_1p5()
                .rounded(px(3.0))
                .bg(c.token_bg)
                .text_color(fg)
                .child(label)
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
        let divider = div()
            .absolute()
            .top(px(-4.0))
            .bottom(px(-4.0))
            .right(px(-8.0))
            .w_px()
            .bg(c.grid_line);

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
                .bg(if edited {
                    Hsla::from(c.pending_bg)
                } else {
                    Theme::tint(c.accent, 0.14)
                })
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
