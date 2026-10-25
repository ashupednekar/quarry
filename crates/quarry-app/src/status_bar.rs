// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use crate::state::{AppState, ConnectionProfile, Environment};
use crate::theme::Theme;
use crate::ui::*;
use crate::{CommitTx, QuarryWindow, RollbackTx, ToggleBottomPanel, ToggleSidebar, ToggleTxMode};
use gpui::component::ActiveTheme;
use gpui::*;
use quarry_core::DatabaseSession;
use std::sync::Arc;

pub struct StatusBar;

impl StatusBar {
    pub fn render(
        state: &AppState,
        session: Option<&Arc<dyn DatabaseSession>>,
        connection: Option<&ConnectionProfile>,
        vim_mode: Option<crate::state::VimMode>,
        theme: &Theme,
        cx: &Context<QuarryWindow>,
    ) -> impl IntoElement {
        let (manual, in_tx) =
            session.map_or((false, false), |s| (s.tx().manual(), s.in_transaction()));
        let link = |id: &'static str, label: &str, color: Rgba| {
            div()
                .id(id)
                .px_1p5()
                .rounded(px(crate::ui::RADIUS_SM))
                .text_xs()
                .text_color(color)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(label.to_string())
        };
        // Only shown once there's a result to describe; query tabs show it in their results header
        let stats = state
            .active_tab()
            .filter(|t| !matches!(t, crate::state::Tab::Query { .. }))
            .and_then(|t| {
                let rs = t.grid().read(cx).delegate();
                rs.result()
            })
            // Row count lives in the grid's footer; only the timing is shown here
            .map(|res| format!("{:.2} ms", res.execution_time.as_secs_f64() * 1000.0));

        let conn_name = connection
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Not connected".to_string());
        // Where it points, so two connections with similar names can't be confused
        let address = connection
            .map(crate::connections::display_address)
            .filter(|a| *a != conn_name);
        let env_tag = connection.map(|c| c.env).unwrap_or(Environment::Local);
        // Flow C: "Production · Read-only" is impossible to miss
        let env_label = match (env_tag, session.is_some_and(|s| s.is_read_only())) {
            (env, true) => format!("{} · Read-only", env.name()),
            (env, false) => env.name().to_string(),
        };
        // The name is in the title bar and editor toolbar; here it's the hover detail
        let conn_tip: Option<SharedString> = connection.map(|_| match &address {
            Some(a) => format!("{conn_name} · {a}").into(),
            None => conn_name.clone().into(),
        });

        let env_color = match env_tag {
            Environment::Local => theme.success,
            Environment::Staging => theme.warning,
            Environment::Prod => theme.danger,
        };

        h_flex()
            .h(px(crate::ui::SMALL_H))
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
                    // Vim mode sits where Vim puts it: bottom left
                    .children(vim_mode.map(|mode| {
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(match mode {
                                crate::state::VimMode::Normal => theme.accent,
                                crate::state::VimMode::Insert => theme.success,
                                crate::state::VimMode::Visual => theme.warning,
                            })
                            .child(mode.label())
                    }))
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
                                move |this, _, window, cx| {
                                    this.toggle_tx_mode(&ToggleTxMode, window, cx)
                                },
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
                        .child(
                            link("tx-commit", "Commit", theme.success).on_click(cx.listener(
                                move |this, _, window, cx| this.commit_tx(&CommitTx, window, cx),
                            )),
                        )
                        .child(
                            link("tx-rollback", "Rollback", theme.danger).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.rollback_tx(&RollbackTx, window, cx)
                                },
                            )),
                        )
                    })
                    // Environment: a quiet dot locally, a tinted pill where mistakes cost more
                    .child(match conn_tip {
                        Some(tip) => {
                            let loud = env_tag != Environment::Local;
                            h_flex()
                                .id("statusbar-conn")
                                .gap_1p5()
                                .items_center()
                                .px_1p5()
                                .rounded(px(crate::ui::RADIUS_SM))
                                .when(loud, |d| {
                                    d.bg(Theme::tint(env_color, 0.14))
                                        .font_weight(FontWeight::SEMIBOLD)
                                })
                                .tooltip(move |window, cx| {
                                    gpui::component::tooltip::Tooltip::new(tip.clone())
                                        .build(window, cx)
                                })
                                .child(div().size(px(6.0)).rounded_full().bg(env_color))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if loud {
                                            env_color
                                        } else {
                                            theme.text_secondary
                                        })
                                        .child(env_label),
                                )
                        }
                        None => h_flex().id("statusbar-conn").child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(conn_name),
                        ),
                    }),
            )
            .child(
                // Right items: result stats, then quiet toggles (also on ⌘B / ⌘J and in ⌘K)
                h_flex()
                    .gap_1()
                    .items_center()
                    // Mono digits: the timing changes every run and mustn't jiggle
                    .children(stats.map(|s| {
                        div()
                            .mr_3()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(theme.text_muted)
                            .child(s)
                    }))
                    .child(
                        link(
                            "statusbar-toggle-sidebar",
                            &format_keys("Sidebar ⌘B"),
                            if state.sidebar_visible {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            },
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx)
                            },
                        )),
                    )
                    .child(
                        link(
                            "statusbar-toggle-bottom",
                            &format_keys("Panel ⌘J"),
                            if state.bottom_panel_open {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            },
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.toggle_bottom_panel(&ToggleBottomPanel, window, cx)
                            },
                        )),
                    ),
            )
    }
}
