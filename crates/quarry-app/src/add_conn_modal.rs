// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::{AppState, ConnectionTargetKind, Environment};
use crate::theme::Theme;
use crate::ui::*;
use gpui::component::{Icon, Sizable};
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Input, InputContentType, InputState};
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
        pg_inputs: [&Entity<InputState>; 5],
        timeout_input: &Entity<InputState>,
        on_toggle_ssh: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_kind: impl Fn(&mut T, ConnectionTargetKind, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_env: impl Fn(&mut T, Environment, &mut Window, &mut Context<T>) + Copy + 'static,
        on_connect: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_test: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_close: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_tab: impl Fn(&mut T, u8, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_read_only: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        if !state.add_conn_modal_open {
            return div().into_any_element();
        }
        let t = theme;
        let is_sqlite = state.add_conn_kind != ConnectionTargetKind::Postgres;
        let editing = state.editing_conn.is_some();
        let tab = state.add_conn_tab;

        // General: what and where
        let kinds = [(ConnectionTargetKind::SqliteFile, "SQLite"), (ConnectionTargetKind::Postgres, "PostgreSQL")];
        let envs = [(Environment::Local, "Local", t.success), (Environment::Staging, "Staging", t.warning), (Environment::Prod, "Production", t.danger)];
        let general = v_flex()
            .gap_6()
            .child(field(
                t,
                "Database",
                None,
                chip_group().children(kinds.into_iter().enumerate().map(|(i, (kind, label))| {
                    let on = (kind == ConnectionTargetKind::Postgres) != is_sqlite;
                    chip(t, ("kind", i), label, on).on_click(cx.listener(move |this, _, window, cx| on_toggle_kind(this, kind.clone(), window, cx)))
                })),
            ))
            .child(field(t, "Instance name", Some("Shown in the sidebar and status bar. Blank uses the address."), form_input(name_input)))
            .when(is_sqlite, |d| d.child(field(t, "File", Some("A path to a .db / .sqlite file, or :memory:"), form_input(path_input))))
            .when(!is_sqlite, |d| {
                let [host, port, user, password, database] = pg_inputs;
                let pair = |a: Div, b: Div, b_w: Option<f32>| {
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(div().flex_1().min_w_0().child(a))
                        .child(match b_w {
                            Some(w) => div().w(px(w)).child(b),
                            None => div().flex_1().min_w_0().child(b),
                        })
                };
                let password_input = div().child(Input::new(password).large().text_sm().px_3p5().content_type(InputContentType::Password).mask_toggle());
                d.child(pair(field(t, "Host", None, form_input(host)), field(t, "Port", None, form_input(port)), Some(120.0)))
                    .child(pair(
                        field(t, "User", None, form_input(user)),
                        field(t, "Password", Some(if editing { "Blank keeps the saved password" } else { "Stored in the keychain" }), password_input),
                        None,
                    ))
                    .child(field(t, "Database", None, form_input(database)))
                    .child(field(t, "URL", Some("Paste a URL to fill the fields above"), form_input(path_input)))
            })
            .child(field(
                t,
                "Environment",
                Some("Production asks before anything that changes data"),
                chip_group().children(envs.into_iter().enumerate().map(|(i, (env, label, color))| {
                    let on = state.add_conn_env == env;
                    chip(t, ("env", i), label, on)
                        .child(div().ml_1p5().size(px(6.0)).rounded_full().bg(color))
                        .flex_row_reverse()
                        .gap_1p5()
                        .on_click(cx.listener(move |this, _, window, cx| on_toggle_env(this, env, window, cx)))
                })),
            ));

        // SSH: Postgres behind a bastion
        let ssh = if is_sqlite {
            v_flex().child(div().text_sm().text_color(t.text_muted).child("SQLite files are opened directly, so there's nothing to tunnel."))
        } else {
            v_flex()
                .gap_6()
                .child(setting_row(
                    t,
                    "Connect through an SSH tunnel",
                    "Uses the system ssh; keys with a passphrase must be in ssh-agent",
                    gpui::component::switch::Switch::new("ssh-switch")
                        .checked(state.add_conn_ssh)
                        .small()
                        .on_click(cx.listener(move |this, _: &bool, window, cx| on_toggle_ssh(this, window, cx))),
                ))
                .when(state.add_conn_ssh, |d| {
                    d.child(
                        h_flex()
                            .gap_3()
                            .child(div().flex_1().child(field(t, "Host", None, form_input(ssh_inputs[0]))))
                            .child(div().w(px(160.0)).child(field(t, "User", None, form_input(ssh_inputs[1])))),
                    )
                    .child(field(t, "Private key", Some("Blank uses ssh-agent and ~/.ssh/config"), form_input(ssh_inputs[2])))
                })
        };

        // Advanced: limits and guard rails
        let advanced = v_flex()
            .gap_6()
            .child(field(t, "Statement timeout (seconds)", Some("Blank uses the default from Settings"), div().w(px(160.0)).child(form_input(timeout_input))))
            .child(setting_row(
                t,
                "Read-only",
                "Refuse INSERT, UPDATE, DELETE and DDL on this connection",
                gpui::component::switch::Switch::new("ro-switch")
                    .checked(state.add_conn_read_only)
                    .small()
                    .on_click(cx.listener(move |this, _: &bool, window, cx| on_toggle_read_only(this, window, cx))),
            ));

        let tabs = h_flex().px_6().gap_6().border_b_1().border_color(t.border).children(
            ["General", "SSH", "Advanced"].into_iter().enumerate().map(|(i, label)| {
                underline_tab(t, ("conn-tab", i), label, tab as usize == i).on_click(cx.listener(move |this, _, window, cx| on_tab(this, i as u8, window, cx)))
            }),
        );

        // Test result sits next to the button that produced it
        let feedback = state.add_conn_feedback.as_ref().map(|(msg, is_error)| {
            h_flex()
                .gap_1p5()
                .min_w_0()
                .items_center()
                .child(Icon::new(if *is_error { IconName::CircleAlert } else { IconName::CircleCheck }).xsmall().text_color(if *is_error { t.danger } else { t.success }))
                .child(div().text_xs().truncate().text_color(if *is_error { t.danger } else { t.text_secondary }).child(msg.clone()))
        });
        let footer = modal_footer(t)
            .justify_between()
            .child(
                h_flex()
                    .gap_3()
                    .min_w_0()
                    .flex_1()
                    .child(zbutton("btn-test-conn").pad_sm().label("Test connection").ghost().small().on_click(cx.listener(move |this, _, window, cx| on_test(this, window, cx))))
                    .children(feedback),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(zbutton("btn-cancel-add-conn").pad_sm().label("Cancel").ghost().small().on_click(cx.listener(move |this, _, window, cx| on_close(this, window, cx))))
                    .child(
                        zbutton("btn-save-add-conn").pad_sm()
                            .label(if editing { "Save & reconnect" } else { "Connect" })
                            .primary()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| on_connect(this, window, cx))),
                    ),
            );

        modal_backdrop("add-conn-backdrop", cx.listener(move |this, _, window, cx| on_close(this, window, cx)))
            .child(
                modal_card("add-conn-modal-card", 640.0, t)
                    // Enter in any field connects, like the primary button
                    .on_action(cx.listener(move |this, _: &gpui::component::input::Enter, window, cx| on_connect(this, window, cx)))
                    .overflow_hidden()
                    .child(modal_header(
                        t,
                        if editing { "Edit connection" } else { "New connection" },
                        Some(if editing { "Changes reconnect this instance".into() } else { "Paste a URL or a file path; the type is picked for you".into() }),
                        cx.listener(move |this, _, window, cx| on_close(this, window, cx)),
                    ))
                    .child(tabs)
                    .child(
                        modal_body("add-conn-body").h(px(480.0)).pt_6()
                            .child(match tab {
                                1 => ssh,
                                2 => advanced,
                                _ => general,
                            }),
                    )
                    .child(footer),
            )
            .into_any_element()
    }
}
