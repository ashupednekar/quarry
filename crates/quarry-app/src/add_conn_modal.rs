// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::Environment;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Input, InputContentType, InputState};
use gpui::component::{Icon, Sizable};
use gpui::*;

pub struct AddConnModal;

impl AddConnModal {
    /// The inputs live on the window so they keep their text between openings
    pub fn render(w: &QuarryWindow, cx: &Context<QuarryWindow>) -> impl IntoElement {
        let (state, theme) = (&w.state, &w.theme);
        let (name_input, path_input, timeout_input) =
            (&w.conn_name_input, &w.conn_path_input, &w.timeout_input);
        let ssh_inputs = &w.ssh_inputs;
        let field_inputs = &w.conn_field_inputs;
        if !state.add_conn_modal_open {
            return div().into_any_element();
        }
        let t = theme;
        let driver =
            quarry_core::driver(state.add_conn_driver).unwrap_or(quarry_core::drivers()[0]);
        let fields = driver.fields();
        let tunnels = driver.tunnel_target(driver.sample()).is_some();
        let editing = state.editing_conn.is_some();
        let tab = state.add_conn_tab;

        // General: what and where
        let envs = [
            (Environment::Local, "Local", t.success),
            (Environment::Staging, "Staging", t.warning),
            (Environment::Prod, "Production", t.danger),
        ];
        let general = v_flex()
            .gap_6()
            .child(field(
                t,
                "Database",
                None,
                chip_group().children(quarry_core::drivers().iter().enumerate().map(|(i, d)| {
                    let id = d.id();
                    chip(t, ("kind", i), d.name(), id == driver.id()).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.pick_conn_driver(id, window, cx)
                        }),
                    )
                })),
            ))
            .child(field(
                t,
                "Instance name",
                Some("Shown in the sidebar and status bar. Blank uses the address."),
                form_input(name_input),
            ))
            // One field is the connection string itself (a file path); typing a URL there switches driver
            .when(fields.len() == 1, |d| {
                d.child(field(
                    t,
                    fields[0].label,
                    Some(fields[0].placeholder),
                    form_input(path_input),
                ))
            })
            // Several fields: the URL they make is shown too, and pasting one fills them
            .when(fields.len() > 1, |d| {
                d.children(field_rows(t, fields, field_inputs, editing))
                    .child(field(
                        t,
                        "URL",
                        Some("Paste a URL to fill the fields above"),
                        form_input(path_input),
                    ))
            })
            .child(field(
                t,
                "Environment",
                Some("Production asks before anything that changes data"),
                chip_group().children(envs.into_iter().enumerate().map(
                    |(i, (env, label, color))| {
                        let on = state.add_conn_env == env;
                        chip(t, ("env", i), label, on)
                            .child(div().ml_1p5().size(px(6.0)).rounded_full().bg(color))
                            .flex_row_reverse()
                            .gap_1p5()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.add_conn_env = env;
                                cx.notify()
                            }))
                    },
                )),
            ));

        // SSH: a database behind a bastion
        let ssh = if !tunnels {
            v_flex().child(div().text_sm().text_color(t.text_muted).child(format!(
                "{} connections are opened directly, so there's nothing to tunnel.",
                driver.name()
            )))
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
                        .on_click(cx.listener(move |this, _: &bool, _, cx| {
                            this.state.add_conn_ssh = !this.state.add_conn_ssh;
                            cx.notify()
                        })),
                ))
                .when(state.add_conn_ssh, |d| {
                    d.child(
                        h_flex()
                            .gap_3()
                            .child(div().flex_1().child(field(
                                t,
                                "Host",
                                None,
                                form_input(&ssh_inputs[0]),
                            )))
                            .child(div().w(px(160.0)).child(field(
                                t,
                                "User",
                                None,
                                form_input(&ssh_inputs[1]),
                            ))),
                    )
                    .child(field(
                        t,
                        "Private key",
                        Some("Blank uses ssh-agent and ~/.ssh/config"),
                        form_input(&ssh_inputs[2]),
                    ))
                })
        };

        // Advanced: limits and guard rails
        let advanced = v_flex()
            .gap_6()
            .child(field(
                t,
                "Statement timeout (seconds)",
                Some("Blank uses the default from Settings"),
                div().w(px(160.0)).child(form_input(timeout_input)),
            ))
            .child(setting_row(
                t,
                "Read-only",
                "Refuse INSERT, UPDATE, DELETE and DDL on this connection",
                gpui::component::switch::Switch::new("ro-switch")
                    .checked(state.add_conn_read_only)
                    .small()
                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                        this.state.add_conn_read_only = !this.state.add_conn_read_only;
                        cx.notify()
                    })),
            ));

        let tabs =
            h_flex()
                .px_6()
                .gap_6()
                .border_b_1()
                .border_color(t.border)
                .children(["General", "SSH", "Advanced"].into_iter().enumerate().map(
                    |(i, label)| {
                        underline_tab(t, ("conn-tab", i), label, tab as usize == i).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.state.add_conn_tab = i as u8;
                                cx.notify()
                            }),
                        )
                    },
                ));

        // Test result sits next to the button that produced it
        let needs_client = state
            .add_conn_feedback
            .as_ref()
            .is_some_and(|(msg, is_error)| *is_error && quarry_core::oracle::client_missing(msg));
        let feedback = state
            .add_conn_feedback
            .as_ref()
            .filter(|_| !needs_client)
            .map(|(msg, is_error)| {
                h_flex()
                    .gap_1p5()
                    .min_w_0()
                    .items_center()
                    .child(
                        Icon::new(if *is_error {
                            IconName::CircleAlert
                        } else {
                            IconName::CircleCheck
                        })
                        .xsmall()
                        .text_color(if *is_error {
                            t.danger
                        } else {
                            t.success
                        }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(if *is_error {
                                t.danger
                            } else {
                                t.text_secondary
                            })
                            .child(msg.clone()),
                    )
            });
        // Oracle without its client library: a strip above the footer offers Oracle's download, never started unasked
        let client_offer = needs_client
            .then(quarry_core::oracle::client::package)
            .flatten()
            .map(|pkg| {
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .border_t_1()
                    .border_color(t.border)
                    .child(Icon::new(IconName::CircleAlert).xsmall().text_color(t.warning))
                    .child(div().flex_1().min_w_0().text_xs().text_color(t.text_secondary).child(format!("Oracle connections need Oracle's client library (Instant Client {}).", pkg.version)))
                    .child(zbutton("btn-oracle-licence").pad_sm().label("Licence").ghost().small().on_click(|_, _, cx| cx.open_url(quarry_core::oracle::client::LICENCE_URL)))
                    .child(
                        zbutton("btn-get-oracle-client").pad_sm().label(format!("Download ({} MB)", pkg.size_mb)).small().on_click(cx.listener(move |this, _, window, cx| this.download_oracle_client(window, cx))),
                    )
            });
        let footer = modal_footer(t)
            .justify_between()
            .child(
                h_flex()
                    .gap_3()
                    .min_w_0()
                    .flex_1()
                    .child(
                        zbutton("btn-test-conn")
                            .pad_sm()
                            .label("Test connection")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.test_add_conn_modal(window, cx)
                            })),
                    )
                    .children(feedback),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        zbutton("btn-cancel-add-conn")
                            .pad_sm()
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.add_conn_modal_open = false;
                                cx.notify()
                            })),
                    )
                    .child(
                        zbutton("btn-save-add-conn")
                            .pad_sm()
                            .label(if editing {
                                "Save & reconnect"
                            } else {
                                "Connect"
                            })
                            .primary()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.connect_add_conn_modal(window, cx)
                            })),
                    ),
            );

        modal_backdrop(
            "add-conn-backdrop",
            cx.listener(move |this, _, _, cx| {
                this.state.add_conn_modal_open = false;
                cx.notify()
            }),
        )
        .child(
            modal_card("add-conn-modal-card", 640.0, t)
                // Enter in any field connects, like the primary button
                .on_action(cx.listener(
                    move |this, _: &gpui::component::input::Enter, window, cx| {
                        this.connect_add_conn_modal(window, cx)
                    },
                ))
                .overflow_hidden()
                .child(modal_header(
                    t,
                    if editing {
                        "Edit connection"
                    } else {
                        "New connection"
                    },
                    Some(if editing {
                        "Changes reconnect this instance".into()
                    } else {
                        "Paste a URL or a file path; the type is picked for you".into()
                    }),
                    cx.listener(move |this, _, _, cx| {
                        this.state.add_conn_modal_open = false;
                        cx.notify()
                    }),
                ))
                .child(tabs)
                .child(
                    modal_body("add-conn-body")
                        .h(px(480.0))
                        .pt_6()
                        .child(match tab {
                            1 => ssh,
                            2 => advanced,
                            _ => general,
                        }),
                )
                .children(client_offer)
                .child(footer),
        )
        .into_any_element()
    }
}

/// The driver's fields, laid out by row; fields sharing a row sit side by side
fn field_rows(
    t: &Theme,
    fields: &[quarry_core::Field],
    inputs: &[Entity<InputState>],
    editing: bool,
) -> Vec<Div> {
    let mut rows: Vec<(u8, Div)> = Vec::new();
    for (f, input) in fields.iter().zip(inputs) {
        let control = if f.secret {
            div().child(
                Input::new(input)
                    .large()
                    .text_sm()
                    .px_3p5()
                    .content_type(InputContentType::Password)
                    .mask_toggle(),
            )
        } else {
            form_input(input)
        };
        let hint = match (f.secret, editing) {
            (true, true) => Some("Blank keeps the saved password"),
            (true, false) => Some("Stored in the keychain"),
            _ => None,
        };
        let cell = match f.width {
            Some(w) => div().w(px(w)),
            None => div().flex_1().min_w_0(),
        }
        .child(field(t, f.label, hint, control));
        match rows.last_mut() {
            Some((row, line)) if *row == f.row => {
                *line = std::mem::replace(line, div()).child(cell)
            }
            _ => rows.push((f.row, h_flex().gap_3().items_start().child(cell))),
        }
    }
    rows.into_iter().map(|(_, line)| line).collect()
}
