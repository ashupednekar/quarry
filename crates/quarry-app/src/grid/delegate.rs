// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! How the table component draws and sorts a result set

use super::*;

impl TableDelegate for ResultSet {
    fn columns_count(&self, _: &App) -> usize {
        self.order
            .as_ref()
            .map_or_else(|| self.result().map_or(0, |r| r.columns.len()), Vec::len)
    }

    fn rows_count(&self, _: &App) -> usize {
        self.total_row_count()
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
            .map(|v| {
                let cow = v.to_display_cow();
                cow.lines().next().map_or(0, |l| l.chars().count())
            })
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

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .gap_2()
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Inbox).size_8().opacity(0.6))
            .child(div().text_sm().child(match &self.source {
                Some(s) if !s.filter.trim().is_empty() => "No rows match this filter",
                Some(_) => "This table is empty",
                None => "The query returned no rows",
            }))
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

            // Aligned like the value it replaces, so editing never moves the text
            let numeric = matches!(ed.input, CellInput::Number(_));
            let text = ed.input.value(cx);
            // Widest line as the grid draws it: decides when text pops out
            let line_w = {
                let mut style = window.text_style();
                style.font_family = mono.clone();
                let size = style.font_size.to_pixels(window.rem_size());
                text.lines()
                    .map(|l| {
                        let run = style.to_run(l.len());
                        window
                            .text_system()
                            .shape_line(l.to_string().into(), size, &[run], None)
                            .width
                    })
                    .fold(px(0.), Pixels::max)
            };
            let text_w = line_w + px(32.0);
            let cell_w = ed.cell_w.get();
            // Numbers stay in the cell: a one-line input scrolls instead
            let popped = !numeric && (text.contains('\n') || (cell_w > px(0.) && text_w > cell_w));
            let textarea = match &ed.input {
                CellInput::Text(e) => Textarea::new(e)
                    .appearance(false)
                    .bordered(false)
                    .font_family(mono.clone())
                    .text_color(c.text)
                    .px_0()
                    .py_0()
                    .w_full()
                    .into_any_element(),
                CellInput::Number(e) => Input::new(e)
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .font_family(mono.clone())
                    .text_color(c.text)
                    .text_right()
                    .px_0()
                    .py_0()
                    .w_full()
                    .into_any_element(),
            };
            let measure = {
                let cell = ed.cell_w.clone();
                canvas(
                    move |bounds, window, _| {
                        if cell.replace(bounds.size.width) != bounds.size.width {
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            };
            let border = if has_error { c.danger } else { c.accent };

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
                                .text_sm()
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
                                .text_sm()
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
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.muted)
                        .bg(c.token_bg)
                        .hover(|s| s.bg(c.accent).text_color(c.on_fill))
                        .tooltip(move |window, cx| {
                            Tooltip::new(crate::ui::format_keys("Set NULL (⌥⌘N)")).build(window, cx)
                        })
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
                .tooltip(move |window, cx| Tooltip::new("Open in Value Editor").build(window, cx))
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

            // Fits the cell exactly: same padding and font as the value, just an accent outline (DataGrip)
            let inline = div()
                .absolute()
                .top(px(-4.0))
                .bottom(px(-4.0))
                .left(px(-8.0))
                .right(px(-8.0))
                .bg(c.editor_bg)
                .border_1()
                .border_color(border)
                .flex()
                .items_center()
                .px(px(7.0))
                .font_family(mono)
                .child(measure);
            // Dates: one slim line under the cell reads the value back in words (so a swapped
            // month/day or a typo shows before saving) and holds the shortcuts. No calendar: in a
            // grid you nudge or paste dates, and ↑/↓ on the part under the caret does the nudging.
            // NULL sits here when there's a strip, else on the popout.
            let (strip_null, null_button) = if ed.date.is_some() {
                (null_button, None)
            } else {
                (None, null_button)
            };
            let strip = ed.date.as_ref().map(|pick| {
                let (readout, valid) = super::editing::date_readout(&text);
                h_flex()
                    .id("cell-date-strip")
                    .items_center()
                    // The readout gets the first line to itself; shortcuts wrap under it
                    .flex_wrap()
                    .gap_x_2()
                    .gap_y_1()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .w_full()
                            .text_color(if valid { c.text } else { c.danger })
                            .child(readout),
                    )
                    .child(
                        div()
                            .id("cell-date-now")
                            .flex_shrink_0()
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(3.0))
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .text_color(c.accent)
                            .bg(c.token_bg)
                            .hover(|s| s.bg(c.accent).text_color(c.on_fill))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|table, _, window, cx| {
                                    table.delegate_mut().pick_now(window, cx);
                                    cx.notify();
                                }),
                            )
                            .child(if pick.timestamp { "Now" } else { "Today" }),
                    )
                    .children(strip_null)
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(c.muted)
                            .child("↑↓ adjusts the part at the caret"),
                    )
            });
            let (calendar, docked_calendar) = match strip {
                Some(strip) if !popped => (
                    Some(
                        div().absolute().top_full().left(px(-1.0)).child(deferred(
                            anchored().snap_to_window_with_margin(px(8.0)).child(
                                strip
                                    .mt_1()
                                    .min_w(px(300.0))
                                    .bg(c.editor_bg)
                                    .border_1()
                                    .border_color(c.grid_line)
                                    .rounded(px(4.0))
                                    .shadow(vec![BoxShadow {
                                        color: hsla(0., 0., 0., 0.3),
                                        offset: point(px(0.), px(2.)),
                                        blur_radius: px(6.),
                                        spread_radius: px(0.),
                                        inset: false,
                                    }]),
                            ),
                        )),
                    ),
                    None,
                ),
                Some(strip) => (None, Some(strip.border_t_1().border_color(c.grid_line))),
                None => (None, None),
            };
            // ↑/↓ step the date part under the caret; when the text isn't a date they do what they did
            let textarea = div()
                .size_full()
                .when(ed.date.is_some(), |d| {
                    // The textarea binds ↑/↓ to MoveUp/MoveDown, so catch those before it does
                    d.capture_action(cx.listener(|table, _: &MoveUp, window, cx| {
                        // Handled: stop here, or the textarea's own MoveUp runs too and moves the caret
                        if table.delegate_mut().step_date(1, window, cx) {
                            cx.stop_propagation();
                        } else {
                            cx.propagate();
                        }
                    }))
                    .capture_action(cx.listener(
                        |table, _: &MoveDown, window, cx| {
                            if table.delegate_mut().step_date(-1, window, cx) {
                                cx.stop_propagation();
                            } else {
                                cx.propagate();
                            }
                        },
                    ))
                })
                .child(textarea);
            if !popped {
                return inline
                    .children(calendar)
                    .id("grid-cell-editor")
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            // A right-aligned one-line input paints ~10px past its box; pull the digits
                            // back to where the grid draws them (measured on screen)
                            .when(numeric, |d| d.pr(px(10.0)))
                            .child(textarea),
                    )
                    .children(error_badge)
                    .into_any_element();
            }
            // Too long or multi-line: a box floats over the cell from its top-left corner and grows
            let w = text_w.max(cell_w).min(px(480.0));
            let popout = v_flex()
                .id("grid-cell-editor")
                .w(w)
                .bg(c.editor_bg)
                .border_1()
                .border_color(border)
                .rounded(px(3.0))
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.4),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .child(div().px(px(7.0)).py(px(3.0)).child(textarea))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_1()
                        .px_1()
                        .pb_1()
                        .children(error_badge)
                        .children(bool_actions)
                        .children(null_button)
                        .child(expand_button),
                )
                .children(docked_calendar);
            return inline
                .id("grid-cell-editor-anchor")
                .child(
                    div()
                        .absolute()
                        .top(px(-1.0))
                        .left(px(-1.0))
                        .child(deferred(
                            anchored().snap_to_window_with_margin(px(8.0)).child(popout),
                        )),
                )
                .into_any_element();
        }
        let state = self.row_state(row_ix);
        let (value, edited) = self.cell_value_ref(row_ix, col_ix);
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
        let content = match value {
            None => token("DEFAULT".into(), c.muted).into_any_element(),
            Some(DbValue::Null) => token(self.null_label.clone(), c.muted).into_any_element(),
            Some(DbValue::Expr(_)) => token("DEFAULT".into(), c.muted).into_any_element(),
            Some(DbValue::Boolean(true)) => token("true".into(), c.success).into_any_element(),
            Some(DbValue::Boolean(false)) => token("false".into(), c.danger).into_any_element(),
            Some(v) => {
                let cow = v.to_display_cow();
                let is_multiline = cow.contains('\n') || cow.contains('\r');
                let text_color = if state == RowState::Deleted {
                    c.muted
                } else if self.is_error_row(row_ix) {
                    c.danger
                } else {
                    c.text
                };

                if is_multiline {
                    let first_line = cow
                        .lines()
                        .map(str::trim_end)
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or_else(|| cow.lines().next().unwrap_or(""));
                    let text: SharedString = SharedString::from(first_line.to_string());
                    h_flex()
                        .items_center()
                        .gap(px(4.0))
                        .size_full()
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(text_color)
                                .when(state == RowState::Deleted, |d| d.line_through())
                                .when(is_fk && state != RowState::Deleted, |d| d.underline())
                                .truncate()
                                .child(text),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(c.muted)
                                .opacity(0.55)
                                .child(Icon::new(IconName::CornerDownLeft).xsmall()),
                        )
                        .into_any_element()
                } else {
                    let text: SharedString = SharedString::from(cow.as_ref());
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(text_color)
                        .when(state == RowState::Deleted, |d| d.line_through())
                        .when(is_fk && state != RowState::Deleted, |d| d.underline())
                        .truncate()
                        .child(text)
                        .into_any_element()
                }
            }
        };
        // Column divider on the cell's true right edge: the table pads cells 8px sideways and 4px up and down (Size::Medium)
        let divider = div()
            .absolute()
            .top(px(-4.0))
            .bottom(px(-4.0))
            .right(px(-8.0))
            .w_px()
            .bg(c.grid_line);

        let in_range = self
            .selected_range
            .as_ref()
            .map_or(false, |r| r.contains(row_ix, display_col));
        let is_multi_range = self
            .selected_range
            .as_ref()
            .map_or(false, |r| !r.is_single_cell());

        // Range selection fill (for non-primary cells in a multi-cell selection)
        let range_box = (in_range && is_multi_range && !is_selected).then(|| {
            div()
                .id("cell-range-box")
                .absolute()
                .top(px(-4.0))
                .bottom(px(-4.0))
                .left(px(-8.0))
                .right(px(-8.0))
                .bg(Theme::tint(c.accent, 0.16))
        });

        // JetBrains DataGrip-style selection box (only when table is focused or active range)
        let is_focused = cx.focus_handle().contains_focused(window, cx);
        let selection_box = (is_selected && (is_focused || in_range)).then(|| {
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
                    Theme::tint(c.accent, 0.22)
                })
                .border_2()
                .border_color(c.accent)
        });

        // Pending uncommitted edit background (when not selected)
        let pending_box = (edited && !is_selected && !in_range).then(|| {
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
            .children(range_box)
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |table, e: &MouseDownEvent, window, cx| {
                    table.delegate_mut().handle_cell_mouse_down(
                        row_ix,
                        display_col,
                        e.modifiers.shift,
                    );
                    if table.delegate().inspector_open() {
                        table
                            .delegate_mut()
                            .inspect(row_ix, display_col, window, cx);
                    }
                    table.refresh(cx);
                }),
            )
            .on_mouse_move(cx.listener(move |table, _, _, cx| {
                if table.delegate().is_dragging {
                    table
                        .delegate_mut()
                        .handle_cell_mouse_drag(row_ix, display_col);
                    table.refresh(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |table, _, _, cx| {
                    if table.delegate().is_dragging {
                        table.delegate_mut().end_drag();
                        table.refresh(cx);
                    }
                }),
            )
            .into_any_element()
    }
}
