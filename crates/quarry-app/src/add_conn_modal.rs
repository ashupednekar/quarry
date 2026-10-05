// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::{AppState, ConnectionTargetKind, Environment};
use crate::theme::Theme;
use crate::ui::*;
use gpui::component::input::{Input, InputState};
use gpui::*;

pub struct AddConnModal;

impl AddConnModal {
    pub fn render<T: 'static>(
        state: &AppState,
        theme: &Theme,
        cx: &Context<T>,
        name_input: &Entity<InputState>,
        path_input: &Entity<InputState>,
        ssh_inputs: [&Entity<InputState>; 3],
        timeout_input: &Entity<InputState>,
        on_toggle_ssh: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_kind: impl Fn(&mut T, ConnectionTargetKind, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_env: impl Fn(&mut T, Environment, &mut Window, &mut Context<T>) + Copy + 'static,
        on_connect: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_test: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_close: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        if !state.add_conn_modal_open {
            return div().into_any_element();
        }

        let is_sqlite = state.add_conn_kind != ConnectionTargetKind::Postgres;

        div()
            .id("add-conn-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000085))
            .flex()
            .justify_center()
            .items_center()
            .on_click(cx.listener(move |this, _, window, cx| {
                on_close(this, window, cx);
            }))
            .child(
                v_flex()
                    .id("add-conn-modal-card")
                    .w(px(520.0))
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
                                            .child("DB"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_primary)
                                            .child("New Database Connection"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("btn-close-add-conn")
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(3.0))
                                    .bg(theme.bg_surface)
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme.text_primary))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        on_close(this, window, cx);
                                    }))
                                    .child("Esc"),
                            ),
                    )
                    .child(
                        // Form Body
                        v_flex()
                            .p_4()
                            .gap_3()
                            .bg(theme.bg_app)
                            .child(
                                // Database Type Selector
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(theme.text_muted).child("DATABASE TYPE"))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .id("conn-type-sqlite")
                                                    .px_3()
                                                    .py_1p5()
                                                    .rounded(px(4.0))
                                                    .bg(if is_sqlite { theme.accent } else { theme.bg_surface })
                                                    .text_color(if is_sqlite { rgb(0xffffff) } else { theme.text_secondary })
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .cursor_pointer()
                                                    .hover(|s| s.opacity(0.85))
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        on_toggle_kind(this, ConnectionTargetKind::SqliteMemory, window, cx);
                                                    }))
                                                    .child("SQLite (Local / File)"),
                                            )
                                            .child(
                                                div()
                                                    .id("conn-type-postgres")
                                                    .px_3()
                                                    .py_1p5()
                                                    .rounded(px(4.0))
                                                    .bg(if !is_sqlite { theme.accent } else { theme.bg_surface })
                                                    .text_color(if !is_sqlite { rgb(0xffffff) } else { theme.text_secondary })
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .cursor_pointer()
                                                    .hover(|s| s.opacity(0.85))
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        on_toggle_kind(this, ConnectionTargetKind::Postgres, window, cx);
                                                    }))
                                                    .child("PostgreSQL (Remote / URL)"),
                                            ),
                                    ),
                            )
                            .child(
                                // Connection Name
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(theme.text_muted).child("CONNECTION NAME"))
                                    .child(Input::new(name_input)),
                            )
                            .child(
                                // Path / URL Input
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(theme.text_muted).child(if is_sqlite { "DATABASE FILE PATH OR :memory:" } else { "POSTGRESQL CONNECTION STRING" }))
                                    .child(Input::new(path_input)),
                            )
                            // Statement timeout (Q10)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_xs().text_color(theme.text_muted).child("STATEMENT TIMEOUT (SECONDS)"))
                                    .child(div().w(px(120.0)).child(Input::new(timeout_input))),
                            )
                            // SSH tunnel (C4): Postgres behind a bastion
                            .when(!is_sqlite, |this| {
                                this.child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            h_flex()
                                                .id("ssh-toggle")
                                                .gap_1p5()
                                                .items_center()
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, window, cx| on_toggle_ssh(this, window, cx)))
                                                .child(
                                                    div()
                                                        .size(px(12.0))
                                                        .rounded(px(3.0))
                                                        .border_1()
                                                        .border_color(theme.border)
                                                        .when(state.add_conn_ssh, |d| d.bg(theme.accent)),
                                                )
                                                .child(div().text_xs().text_color(theme.text_muted).child("CONNECT THROUGH SSH TUNNEL")),
                                        )
                                        .when(state.add_conn_ssh, |this| {
                                            this.child(h_flex().gap_2().child(div().flex_1().child(Input::new(ssh_inputs[0]))).child(div().w(px(140.0)).child(Input::new(ssh_inputs[1]))))
                                                .child(Input::new(ssh_inputs[2]))
                                                .child(div().text_xs().text_color(theme.text_muted).child(
                                                    "Uses the system ssh. Keys with a passphrase need to be in ssh-agent (ssh-add).",
                                                ))
                                        }),
                                )
                            })
                            .child(
                                // Environment Tag
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(theme.text_muted).child("ENVIRONMENT"))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .id("env-local-btn")
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded(px(3.0))
                                                    .bg(if state.add_conn_env == Environment::Local { theme.success } else { theme.bg_surface })
                                                    .text_color(if state.add_conn_env == Environment::Local { rgb(0xffffff) } else { theme.text_secondary })
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        on_toggle_env(this, Environment::Local, window, cx);
                                                    }))
                                                    .child("LOCAL"),
                                            )
                                            .child(
                                                div()
                                                    .id("env-staging-btn")
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded(px(3.0))
                                                    .bg(if state.add_conn_env == Environment::Staging { theme.warning } else { theme.bg_surface })
                                                    .text_color(if state.add_conn_env == Environment::Staging { rgb(0xffffff) } else { theme.text_secondary })
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        on_toggle_env(this, Environment::Staging, window, cx);
                                                    }))
                                                    .child("STAGING"),
                                            )
                                            .child(
                                                div()
                                                    .id("env-prod-btn")
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded(px(3.0))
                                                    .bg(if state.add_conn_env == Environment::Prod { theme.danger } else { theme.bg_surface })
                                                    .text_color(if state.add_conn_env == Environment::Prod { rgb(0xffffff) } else { theme.text_secondary })
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        on_toggle_env(this, Environment::Prod, window, cx);
                                                    }))
                                                    .child("PROD (GUARDED)"),
                                            ),
                                    ),
                            )
                            .when_some(state.add_conn_feedback.as_ref(), |this, (msg, is_error)| {
                                let fg = if *is_error { theme.danger } else { theme.success };
                                let bg = if *is_error { rgba(0xef444420) } else { rgba(0x10b98120) };
                                this.child(
                                    h_flex()
                                        .px_3()
                                        .py_1p5()
                                        .rounded(px(4.0))
                                        .bg(bg)
                                        .border_1()
                                        .border_color(fg)
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(fg)
                                                .child(msg.clone()),
                                        ),
                                )
                            }),
                    )
                    .child(
                        // Footer Actions
                        h_flex()
                            .h(px(48.0))
                            .px_4()
                            .bg(theme.bg_panel)
                            .border_t_1()
                            .border_color(theme.border)
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .id("btn-test-conn")
                                    .px_3()
                                    .py_1p5()
                                    .rounded(px(4.0))
                                    .bg(theme.bg_surface)
                                    .border_1()
                                    .border_color(theme.border)
                                    .text_xs()
                                    .text_color(theme.text_primary)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        on_test(this, window, cx);
                                    }))
                                    .child("Test Connection"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("btn-cancel-add-conn")
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
                                                on_close(this, window, cx);
                                            }))
                                            .child("Cancel"),
                                    )
                                    .child(
                                        div()
                                            .id("btn-save-add-conn")
                                            .px_3p5()
                                            .py_1p5()
                                            .rounded(px(4.0))
                                            .bg(theme.accent)
                                            .text_xs()
                                            .text_color(rgb(0xffffff))
                                            .font_weight(FontWeight::BOLD)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.accent_hover))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                on_connect(this, window, cx);
                                            }))
                                            .child("Connect & Open"),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
