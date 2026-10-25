// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

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
        let env_switch = segmented(t).children(envs.into_iter().enumerate().map(
            |(i, (env, label, color))| {
                let on = state.add_conn_env == env;
                chip(t, ("env", i), label, on)
                    .justify_center()
                    .when(on, |c| c.bg(Rgba { a: 0.16, ..color }))
                    .child(div().ml_1p5().size(px(6.0)).rounded_full().bg(color))
                    .flex_row_reverse()
                    .gap_1p5()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.add_conn_env = env;
                        cx.notify()
                    }))
            },
        ));
        // Settings-style groups: where it is, then how it's labelled
        let general = v_flex()
            .gap_4()
            // One field is the connection string itself (a file path); typing a URL there switches driver
            .when(fields.len() == 1, |d| {
                d.child(settings_group(
                    t,
                    [form_row(
                        t,
                        fields[0].label,
                        Some(fields[0].placeholder.into()),
                        None,
                        file_input(
                            path_input,
                            "btn-browse-db-file",
                            cx.listener(|this, _, window, cx| {
                                this.browse_sqlite_file(window, cx);
                            }),
                        ),
                    )],
                ))
            })
            // Several fields: pasting a URL first fills them, and edits to them rewrite it
            .when(fields.len() > 1, |d| {
                d.child(settings_group(
                    t,
                    [form_row(
                        t,
                        "URL",
                        Some("Paste one to fill the fields below".into()),
                        None,
                        compact_input(path_input),
                    )],
                ))
                .child(settings_group(
                    t,
                    field_rows(t, fields, field_inputs, editing),
                ))
            })
            .child(settings_group(
                t,
                [
                    form_row(
                        t,
                        "Instance name",
                        Some("Shown in the sidebar. Blank uses the address.".into()),
                        None,
                        compact_input(name_input),
                    ),
                    form_row(
                        t,
                        "Environment",
                        Some("Production asks before changing data".into()),
                        None,
                        env_switch,
                    ),
                ],
            ));

        // SSH: a database behind a bastion
        let ssh = if !tunnels {
            v_flex().child(div().text_sm().text_color(t.text_muted).child(format!(
                "{} connections are opened directly, so there's nothing to tunnel.",
                driver.name()
            )))
        } else {
            let mut rows = vec![setting_row(
                t,
                "Connect through an SSH tunnel",
                "Uses the system ssh; passphrase keys must be in ssh-agent",
                gpui::component::switch::Switch::new("ssh-switch")
                    .checked(state.add_conn_ssh)
                    .small()
                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                        this.state.add_conn_ssh = !this.state.add_conn_ssh;
                        cx.notify()
                    })),
            )];
            if state.add_conn_ssh {
                rows.push(form_row(
                    t,
                    "Host",
                    None,
                    None,
                    compact_input(&ssh_inputs[0]),
                ));
                rows.push(form_row(
                    t,
                    "User",
                    None,
                    None,
                    compact_input(&ssh_inputs[1]),
                ));
                rows.push(form_row(
                    t,
                    "Private key",
                    Some("Blank uses ssh-agent and ~/.ssh/config".into()),
                    None,
                    file_input(
                        &ssh_inputs[2],
                        "btn-browse-ssh-key",
                        cx.listener(|this, _, window, cx| {
                            this.browse_ssh_key_file(window, cx);
                        }),
                    ),
                ));
            }
            v_flex().child(settings_group(t, rows))
        };

        // Advanced: limits and guard rails
        let advanced = v_flex().child(settings_group(
            t,
            [
                form_row(
                    t,
                    "Statement timeout",
                    Some("Seconds. Blank uses the default from Settings".into()),
                    Some(120.0),
                    compact_input(timeout_input),
                ),
                setting_row(
                    t,
                    "Read-only",
                    "Refuse INSERT, UPDATE, DELETE and DDL",
                    gpui::component::switch::Switch::new("ro-switch")
                        .checked(state.add_conn_read_only)
                        .small()
                        .on_click(cx.listener(move |this, _: &bool, _, cx| {
                            this.state.add_conn_read_only = !this.state.add_conn_read_only;
                            cx.notify()
                        })),
                ),
            ],
        ));

        // Same page head as Settings: the title large, its option groups as a segmented control
        let tabs = h_flex()
            .justify_between()
            .items_center()
            .flex_shrink_0()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(driver.name()),
            )
            .child(
                segmented(t).children(["General", "SSH", "Advanced"].into_iter().enumerate().map(
                    |(i, label)| {
                        chip(t, ("conn-tab", i), label, tab as usize == i).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.state.add_conn_tab = i as u8;
                                cx.notify()
                            },
                        ))
                    },
                )),
            );

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
                let is_error = *is_error;
                // Drivers nest "failed" prefixes; the code and reason are what matter
                let msg: SharedString = msg.clone().into();
                let copy = msg.clone();
                h_flex()
                    .px_6()
                    .py_2p5()
                    .gap_2()
                    .items_start()
                    .border_t_1()
                    .border_color(t.border)
                    .bg(Rgba {
                        a: 0.08,
                        ..if is_error { t.danger } else { t.success }
                    })
                    .child(
                        Icon::new(if is_error {
                            IconName::CircleAlert
                        } else {
                            IconName::CircleCheck
                        })
                        .xsmall()
                        .mt_0p5()
                        .text_color(if is_error {
                            t.danger
                        } else {
                            t.success
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(if is_error { t.danger } else { t.text_secondary })
                            .child(msg),
                    )
                    .when(is_error, |d| {
                        d.child(
                            zbutton("btn-copy-conn-error")
                                .icon(IconName::Copy)
                                .ghost()
                                .xsmall()
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy.to_string(),
                                    ))
                                }),
                        )
                    })
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
                    .child(div().flex_1().min_w_0().text_sm().text_color(t.text_secondary).child(format!("Oracle connections need Oracle's client library (Instant Client {}).", pkg.version)))
                    .child(zbutton("btn-oracle-licence").pad_sm().label("Licence").ghost().small().on_click(|_, _, cx| cx.open_url(quarry_core::oracle::client::LICENCE_URL)))
                    .child(
                        zbutton("btn-get-oracle-client").pad_sm().label(format!("Download ({} MB)", pkg.size_mb)).small().on_click(cx.listener(move |this, _, window, cx| this.download_oracle_client(window, cx))),
                    )
            });
        let footer = modal_footer(t)
            .justify_between()
            .child(
                h_flex().gap_3().min_w_0().flex_1().child(
                    zbutton("btn-test-conn")
                        .pad_sm()
                        .label("Test connection")
                        .outline()
                        .border_1()
                        .border_color(t.border)
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.test_add_conn_modal(window, cx)
                        })),
                ),
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

        let left_pane =
            v_flex()
                .id("add-conn-db-list")
                .w(px(180.0))
                .flex_none()
                .border_r_1()
                .border_color(t.border)
                .bg(t.bg_panel)
                .p_2()
                .gap_0p5()
                .overflow_y_scroll()
                .children(quarry_core::drivers().iter().enumerate().map(|(i, d)| {
                    let id = d.id();
                    let is_selected = id == driver.id();
                    h_flex()
                        .id(("engine", i))
                        .h(px(crate::ui::CONTROL_H))
                        .px_2p5()
                        .gap_2()
                        .items_center()
                        .rounded(px(crate::ui::RADIUS_MD))
                        .cursor_pointer()
                        .when(is_selected, |s| s.bg(t.bg_selected))
                        .when(!is_selected, |s| s.hover(|h| h.bg(t.bg_hover)))
                        .child(crate::db_icons::database_icon_sized(
                            id,
                            px(16.0),
                            if is_selected {
                                t.accent
                            } else {
                                t.text_secondary
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(if is_selected {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                })
                                .child(d.name()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_conn_driver(id, window, cx)
                        }))
                }));

        let right_pane = modal_body("add-conn-options-scroll")
            .flex_1()
            .min_w_0()
            .child(v_flex().gap_4().child(tabs).child(match tab {
                1 => ssh,
                2 => advanced,
                _ => general,
            }));

        crate::ui::fade_in(
            "add-conn-backdrop-fade",
            modal_backdrop(
                "add-conn-backdrop",
                cx.listener(move |this, _, _, cx| {
                    this.state.add_conn_modal_open = false;
                    cx.notify()
                }),
                cx,
            )
            .child(
                // Same frame as Settings: 820 × 560, nav left, page right
                modal_card("add-conn-modal-card", 820.0, t)
                    .h(px(560.0))
                    .max_h_full()
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
                        editing.then(|| "Changes reconnect this instance".into()),
                        cx.listener(move |this, _, _, cx| {
                            this.state.add_conn_modal_open = false;
                            cx.notify()
                        }),
                    ))
                    .child(
                        h_flex()
                            .id("add-conn-two-pane")
                            .flex_1()
                            .min_h_0()
                            .items_stretch()
                            .border_t_1()
                            .border_color(t.border)
                            .child(left_pane)
                            .child(right_pane),
                    )
                    .children(feedback)
                    .children(client_offer)
                    .child(footer),
            ),
        )
        .into_any_element()
    }
}

/// The driver's fields, one form row each
fn field_rows(
    t: &Theme,
    fields: &[quarry_core::Field],
    inputs: &[Entity<InputState>],
    editing: bool,
) -> Vec<Div> {
    fields
        .iter()
        .zip(inputs)
        .map(|(f, input)| {
            let control = if f.secret {
                div().child(
                    Input::new(input)
                        .text_sm()
                        .px_3()
                        .content_type(InputContentType::Password)
                        .mask_toggle(),
                )
            } else {
                compact_input(input)
            };
            let hint = f.secret.then_some(if editing {
                "Blank keeps the saved password"
            } else {
                "Stored in the keychain"
            });
            // Narrow fields (port) keep their width, right-aligned like the rest
            form_row(t, f.label, hint.map(Into::into), f.width, control)
        })
        .collect()
}

/// Default-height input: the modal's dense form doesn't need form_input's 44px
fn compact_input(state: &Entity<InputState>) -> Div {
    div().child(Input::new(state).text_sm().px_3())
}

/// Compact input with a trailing folder button to open the system file picker
fn file_input(
    state: &Entity<InputState>,
    btn_id: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div().child(
        Input::new(state).text_sm().px_3().suffix(
            zbutton(btn_id)
                .icon(IconName::FolderOpen)
                .ghost()
                .xsmall()
                .tooltip("Choose file…")
                .on_click(on_click),
        ),
    )
}
