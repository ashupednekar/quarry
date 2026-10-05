// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::theme::Theme;
use crate::ui::*;
use gpui::component::input::{Editor, EditorState};
use gpui::*;

pub struct QueryEditor;

impl QueryEditor {
    pub fn render<T: 'static>(
        editor: &Entity<EditorState>,
        is_running: bool,
        error_message: Option<&str>,
        theme: &Theme,
        cx: &Context<T>,
        on_run: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(theme.bg_panel)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(32.0))
                    .px_3()
                    .bg(theme.bg_sidebar)
                    .border_b_1()
                    .border_color(theme.border)
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("btn-run-query")
                            .px_2p5()
                            .py_1()
                            .rounded(px(4.0))
                            .bg(theme.accent)
                            .text_color(rgb(0xffffff))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.accent_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                on_run(this, window, cx);
                            }))
                            .child(if is_running { "Running..." } else { "Run ⌘↵" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("Runs the selection, or the statement at the cursor"),
                    ),
            )
            .child(div().flex_1().min_h_0().child(Editor::new(editor).h_full().bordered(false)))
            .when_some(error_message, |this, err| {
                this.child(
                    h_flex()
                        .px_3()
                        .py_1p5()
                        .bg(rgba(0xef444420))
                        .border_t_1()
                        .border_color(theme.danger)
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.danger)
                                .child("Error:"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(err.to_string()),
                        ),
                )
            })
    }
}
