// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::{AppState, Environment};
use crate::theme::Theme;
use crate::ui::*;
use gpui::*;

pub struct StatusBar;

impl StatusBar {
    pub fn render<T: 'static>(
        state: &AppState,
        theme: &Theme,
        cx: &Context<T>,
        on_toggle_bottom: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_sidebar: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_tx: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_commit: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_rollback: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        let (manual, in_tx) = state.session.as_ref().map_or((false, false), |s| (s.tx().manual(), s.in_transaction()));
        let link = |id: &'static str, label: &'static str, color: Rgba| {
            div()
                .id(id)
                .px_1p5()
                .rounded(px(3.0))
                .text_xs()
                .text_color(color)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(label)
        };
        let (row_count_str, timing_str) = state
            .active_tab()
            .and_then(|t| {
                let rs = t.grid().read(cx).delegate();
                let more = if rs.source.as_ref().is_some_and(|s| s.has_more) { "+" } else { "" };
                rs.result().map(|res| (res, more))
            })
            .map(|(res, more)| {
                (
                    format!("{}{} rows", res.rows.len(), more),
                    format!("{:.2} ms", res.execution_time.as_secs_f64() * 1000.0),
                )
            })
            .unwrap_or_else(|| ("0 rows".to_string(), "0.00 ms".to_string()));

        let conn_name = state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "offline".to_string());

        let env_tag = state
            .active_connection
            .as_ref()
            .map(|c| c.env)
            .unwrap_or(Environment::Local);

        let env_color = match env_tag {
            Environment::Local => theme.success,
            Environment::Staging => theme.warning,
            Environment::Prod => theme.danger,
        };

        h_flex()
            .h(px(24.0))
            .w_full()
            .px_3()
            .bg(theme.bg_panel)
            .border_t_1()
            .border_color(theme.border)
            .justify_between()
            .items_center()
            .child(
                // Left items
                h_flex()
                    .gap_3()
                    .items_center()
                    // Transaction mode (E9): click to switch; an open transaction shows Commit / Rollback
                    .child(
                        link("tx-mode", if manual { "Manual commit" } else { "Auto-commit" }, theme.text_muted)
                            .on_click(cx.listener(move |this, _, window, cx| on_toggle_tx(this, window, cx))),
                    )
                    .when(in_tx, |this| {
                        this.child(div().text_xs().font_weight(FontWeight::BOLD).text_color(theme.warning).child("● Transaction open"))
                            .child(
                                link("tx-commit", "Commit", theme.success)
                                    .on_click(cx.listener(move |this, _, window, cx| on_commit(this, window, cx))),
                            )
                            .child(
                                link("tx-rollback", "Rollback", theme.danger)
                                    .on_click(cx.listener(move |this, _, window, cx| on_rollback(this, window, cx))),
                            )
                    })
                    .child(
                        // Active connection badge
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .w(px(5.0))
                                    .h(px(5.0))
                                    .rounded(px(2.5))
                                    .bg(env_color),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(conn_name),
                            ),
                    )
                    .child(
                        // Environment Pill
                        div()
                            .px_1p5()
                            .py(px(0.5))
                            .rounded(px(3.0))
                            .bg(theme.bg_surface)
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(env_color)
                            .child(env_tag.label()),
                    ),
            )
            .child(
                // Center items: Row count & execution time
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(format!("{} · {}", row_count_str, timing_str)),
                    ),
            )
            .child(
                // Right items: Panel & Sidebar toggles
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .id("statusbar-toggle-sidebar")
                            .px_1p5()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(if state.sidebar_visible { theme.bg_surface } else { theme.bg_panel })
                            .text_xs()
                            .text_color(if state.sidebar_visible { theme.text_primary } else { theme.text_muted })
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.text_primary))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                on_toggle_sidebar(this, window, cx);
                            }))
                            .child("Sidebar (⌘B)"),
                    )
                    .child(
                        div()
                            .id("statusbar-toggle-bottom")
                            .px_1p5()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(if state.bottom_panel_open { theme.bg_surface } else { theme.bg_panel })
                            .text_xs()
                            .text_color(if state.bottom_panel_open { theme.text_primary } else { theme.text_muted })
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.text_primary))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                on_toggle_bottom(this, window, cx);
                            }))
                            .child("Panel (⌘J)"),
                    ),
            )
    }
}
