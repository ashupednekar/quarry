// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::{AppState, ConnectionProfile, Environment};
use crate::theme::Theme;
use crate::ui::*;
use gpui::*;
use quarry_core::DatabaseSession;
use std::sync::Arc;

pub struct StatusBar;

impl StatusBar {
    pub fn render<T: 'static>(
        state: &AppState,
        session: Option<&Arc<dyn DatabaseSession>>,
        connection: Option<&ConnectionProfile>,
        theme: &Theme,
        cx: &Context<T>,
        on_toggle_bottom: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_sidebar: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_tx: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_commit: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_rollback: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        let (manual, in_tx) =
            session.map_or((false, false), |s| (s.tx().manual(), s.in_transaction()));
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
        // Only shown once there's a result to describe; query tabs show it in their results header
        let stats = state
            .active_tab()
            .filter(|t| !matches!(t, crate::state::Tab::Query { .. }))
            .and_then(|t| {
                let rs = t.grid().read(cx).delegate();
                let more = if rs.source.as_ref().is_some_and(|s| s.has_more) {
                    "+"
                } else {
                    ""
                };
                rs.result().map(|res| (res, more))
            })
            .map(|(res, more)| {
                (
                    format!("{}{} rows", res.rows.len(), more),
                    format!("{:.2} ms", res.execution_time.as_secs_f64() * 1000.0),
                )
            })
            .map(|(rows, time)| format!("{} · {}", rows, time));

        let conn_name = connection
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Not connected".to_string());
        // Where it points, so two connections with similar names can't be confused
        let address = connection
            .map(|c| crate::connections::display_address(c))
            .filter(|a| *a != conn_name);
        let env_tag = connection.map(|c| c.env).unwrap_or(Environment::Local);
        // Flow C: "PROD · READ-ONLY" is impossible to miss
        let env_label = match (env_tag, session.is_some_and(|s| s.is_read_only())) {
            (env, true) => format!("{} · READ-ONLY", env.label()),
            (env, false) => env.label().to_string(),
        };

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
                    .when(session.is_some(), |this| {
                        this.child(
                            link(
                                "tx-mode",
                                if manual {
                                    "Manual commit"
                                } else {
                                    "Auto-commit"
                                },
                                theme.text_muted,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| on_toggle_tx(this, window, cx),
                            )),
                        )
                    })
                    .when(in_tx, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.warning)
                                .child("● Transaction open"),
                        )
                        .child(link("tx-commit", "Commit", theme.success).on_click(
                            cx.listener(move |this, _, window, cx| on_commit(this, window, cx)),
                        ))
                        .child(
                            link("tx-rollback", "Rollback", theme.danger).on_click(cx.listener(
                                move |this, _, window, cx| on_rollback(this, window, cx),
                            )),
                        )
                    })
                    .child(
                        // Active connection badge
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(div().w(px(5.0)).h(px(5.0)).rounded(px(2.5)).bg(env_color))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(conn_name),
                            )
                            .when_some(address, |d, a| {
                                d.child(div().text_xs().text_color(theme.text_muted).child(a))
                            }),
                    )
                    .when(connection.is_some(), |this| {
                        this.child(
                            // Environment Pill
                            div()
                                .px_1p5()
                                .py(px(0.5))
                                .rounded(px(3.0))
                                .bg(theme.bg_surface)
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(env_color)
                                .child(env_label),
                        )
                    }),
            )
            .child(
                // Center items: Row count & execution time
                h_flex().gap_2().items_center().children(
                    stats.map(|s| div().text_xs().text_color(theme.text_secondary).child(s)),
                ),
            )
            .child(
                // Right items: quiet toggles (also on ⌘B / ⌘J and in ⌘K)
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        link(
                            "statusbar-toggle-sidebar",
                            "Sidebar ⌘B",
                            if state.sidebar_visible {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            },
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| on_toggle_sidebar(this, window, cx),
                        )),
                    )
                    .child(
                        link(
                            "statusbar-toggle-bottom",
                            "Panel ⌘J",
                            if state.bottom_panel_open {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            },
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| on_toggle_bottom(this, window, cx),
                        )),
                    ),
            )
    }
}
