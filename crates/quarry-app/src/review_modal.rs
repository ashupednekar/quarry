// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::*;
use gpui::*;

pub struct ReviewModal;

impl ReviewModal {
    pub fn render<T: 'static>(
        state: &AppState,
        theme: &Theme,
        cx: &Context<T>,
        on_commit: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_cancel: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        if !state.review_modal_open {
            return div().into_any_element();
        }

        let sql_statements = &state.review_modal_sql;
        let table_name = &state.review_modal_table;
        let count = sql_statements.len();

        div()
            .id("review-modal-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000085))
            .flex()
            .justify_center()
            .items_center()
            .on_click(cx.listener(move |this, _, window, cx| {
                on_cancel(this, window, cx);
            }))
            .child(
                v_flex()
                    .id("review-modal-card")
                    .w(px(640.0))
                    .max_h(px(520.0))
                    .rounded(px(8.0))
                    .bg(theme.bg_sidebar)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .on_click(|_, _, cx| cx.stop_propagation()) // keep clicks off the backdrop
                    .child(
                        // Header
                        h_flex()
                            .h(px(40.0))
                            .px_4()
                            .bg(theme.bg_panel)
                            .border_b_1()
                            .border_color(theme.border)
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py(px(0.5))
                                            .rounded(px(2.0))
                                            .bg(theme.bg_surface)
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.accent)
                                            .child("DIFF"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_primary)
                                            .child("Review Pending Changes"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(format!(
                                        "{} {} on \"{}\"",
                                        count,
                                        if count == 1 { "statement" } else { "statements" },
                                        table_name
                                    )),
                            ),
                    )
                    .child(
                        // SQL Preview
                        v_flex()
                            .id("review-sql-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .p_3()
                            .gap_2()
                            .bg(theme.bg_app)
                            .children(sql_statements.iter().enumerate().map(|(idx, sql)| {
                                h_flex()
                                    .p_2p5()
                                    .rounded(px(4.0))
                                    .bg(theme.bg_panel)
                                    .border_1()
                                    .border_color(theme.border)
                                    .gap_3()
                                    .items_start()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_muted)
                                            .child(format!("{}.", idx + 1)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(sql.clone()),
                                    )
                            })),
                    )
                    .child(
                        // Actions Footer
                        h_flex()
                            .h(px(44.0))
                            .px_4()
                            .bg(theme.bg_panel)
                            .border_t_1()
                            .border_color(theme.border)
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child("Changes will be executed atomically inside a transaction."),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("review-cancel-btn")
                                            .px_3()
                                            .py_1p5()
                                            .rounded(px(4.0))
                                            .bg(theme.bg_surface)
                                            .border_1()
                                            .border_color(theme.border)
                                            .text_xs()
                                            .text_color(theme.text_secondary)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover).text_color(theme.text_primary))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                on_cancel(this, window, cx);
                                            }))
                                            .child("Cancel (Esc)"),
                                    )
                                    .child(
                                        div()
                                            .id("review-commit-btn")
                                            .px_3()
                                            .py_1p5()
                                            .rounded(px(4.0))
                                            .bg(theme.accent)
                                            .text_xs()
                                            .text_color(rgb(0xffffff))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.accent_hover))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                on_commit(this, window, cx);
                                            }))
                                            .child("Submit (⌘↵)"),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
