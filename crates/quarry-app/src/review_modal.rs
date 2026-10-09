// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::*;
use gpui::component::button::ButtonVariants;
use gpui::component::{ActiveTheme, Sizable};
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

        let mono = cx.theme().mono_font_family.clone();
        let close = cx.listener(move |this, _, window, cx| on_cancel(this, window, cx));
        modal_backdrop("review-modal-backdrop", cx.listener(move |this, _, window, cx| on_cancel(this, window, cx)))
            .child(
                modal_card("review-modal-card", 640.0, theme)
                    .max_h(px(560.0))
                    .overflow_hidden()
                    .child(modal_header(
                        theme,
                        "Review changes",
                        Some(format!("{count} {} on {table_name} · applied in one transaction", if count == 1 { "statement" } else { "statements" }).into()),
                        close,
                    ))
                    .child(modal_body("review-sql-scroll").flex_1().gap_2().children(sql_statements.iter().enumerate().map(|(idx, sql)| {
                        h_flex()
                            .p_3()
                            .gap_3()
                            .items_start()
                            .rounded(px(6.0))
                            .bg(theme.bg_panel)
                            .border_1()
                            .border_color(theme.border)
                            .child(div().w(px(20.0)).flex_shrink_0().text_xs().text_color(theme.text_muted).child(format!("{}", idx + 1)))
                            .child(div().text_xs().font_family(mono.clone()).text_color(theme.text_primary).child(sql.clone()))
                    })))
                    .child(
                        modal_footer(theme)
                            .child(zbutton("review-cancel-btn").pad_sm().label("Cancel").ghost().small().on_click(cx.listener(move |this, _, window, cx| on_cancel(this, window, cx))))
                            .child(zbutton("review-commit-btn").pad_sm().label("Submit ⌘↵").primary().small().on_click(cx.listener(move |this, _, window, cx| on_commit(this, window, cx)))),
                    ),
            )
            .into_any_element()
    }
}
