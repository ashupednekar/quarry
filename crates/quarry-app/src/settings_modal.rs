// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Settings (⌘,): the everyday options as one quiet list. Every change saves to settings.json,
//! so hand edits and the modal never disagree.

use crate::settings::{self, Settings};
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::switch::Switch;
use gpui::component::Sizable;
use gpui::*;

impl QuarryWindow {
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_themes = crate::theme::available();
        self.settings_open = true;
        cx.notify();
    }

    /// Change one setting: save, then reload exactly as a hand edit would
    pub(crate) fn change_setting(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut s = self.settings.clone();
        f(&mut s);
        if let Err(e) = settings::save(&s) {
            return self.toast(format!("Couldn't save settings: {e}"), true, cx);
        }
        self.config_watcher.sync_mtimes();
        self.reload_settings(cx);
    }

    pub fn render_settings_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
        let t = self.theme.clone();
        let s = self.settings.clone();

        // A switch bound to one bool field
        let toggle =
            |id: &'static str, on: bool, set: fn(&mut Settings, bool), cx: &mut Context<Self>| {
                Switch::new(id).checked(on).small().on_click(cx.listener(
                    move |this, v: &bool, _, cx| {
                        let v = *v;
                        this.change_setting(|s| set(s, v), cx)
                    },
                ))
            };
        // Chips over a few fixed values of one field
        fn choices<V: PartialEq + Clone + 'static>(
            t: &crate::theme::Theme,
            id: &'static str,
            options: Vec<(V, String)>,
            current: &V,
            set: fn(&mut Settings, V),
            cx: &mut Context<QuarryWindow>,
        ) -> Div {
            chip_group().children(options.into_iter().enumerate().map(|(i, (v, label))| {
                let on = v == *current;
                chip(t, (id, i), label, on).on_click(cx.listener(move |this, _, _, cx| {
                    let v = v.clone();
                    this.change_setting(|s| set(s, v), cx)
                }))
            }))
        }

        // Stored as written (user theme files are looked up by name); matched without case
        let themes: Vec<(String, String)> = self
            .settings_themes
            .iter()
            .map(|n| (n.clone(), n.clone()))
            .collect();
        let current_theme = self
            .settings_themes
            .iter()
            .find(|n| n.eq_ignore_ascii_case(s.theme.trim()))
            .cloned()
            .unwrap_or_else(|| s.theme.clone());
        let appearance = v_flex()
            .child(section_title(&t, "Appearance"))
            // Themes can be many (user files), so they get the full width under the label
            .child(
                v_flex()
                    .py_2()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().text_sm().text_color(t.text_primary).child("Theme"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child("Built-ins, or drop a .json into the themes folder"),
                            ),
                    )
                    .child(choices(
                        &t,
                        "theme",
                        themes,
                        &current_theme,
                        |s, v| s.theme = v,
                        cx,
                    )),
            )
            .child(setting_row(
                &t,
                "Interface size",
                "Labels, menus and panes",
                choices(
                    &t,
                    "ui-size",
                    [12.0f32, 13.0, 14.0, 15.0, 16.0, 18.0]
                        .map(|v| (v, format!("{v}")))
                        .to_vec(),
                    &s.font_size,
                    |s, v| s.font_size = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Editor & data size",
                "SQL and grid cells, in JetBrainsMono Nerd Font",
                choices(
                    &t,
                    "mono-size",
                    [
                        (0.0f32, "Auto".to_string()),
                        (12.0, "12".into()),
                        (13.0, "13".into()),
                        (14.0, "14".into()),
                        (15.0, "15".into()),
                        (16.0, "16".into()),
                    ]
                    .to_vec(),
                    &s.mono_font_size,
                    |s, v| s.mono_font_size = v,
                    cx,
                ),
            ));

        let editor = v_flex()
            .child(section_title(&t, "Editor"))
            .child(setting_row(
                &t,
                "Tab size",
                "Spaces per indent",
                choices(
                    &t,
                    "tab",
                    vec![(2usize, "2".into()), (4, "4".into())],
                    &s.tab_size,
                    |s, v| s.tab_size = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Soft wrap",
                "Wrap long lines instead of scrolling sideways",
                toggle("soft-wrap", s.soft_wrap, |s, v| s.soft_wrap = v, cx),
            ))
            .child(setting_row(
                &t,
                "Line numbers",
                "In the gutter",
                toggle(
                    "line-numbers",
                    s.line_numbers,
                    |s, v| s.line_numbers = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Highlight current statement",
                crate::ui::format_keys("Tint what ⌘↵ will run"),
                toggle(
                    "highlight",
                    s.highlight_current_statement,
                    |s, v| s.highlight_current_statement = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Autocomplete",
                "Tables, columns and joins as you type",
                toggle(
                    "autocomplete",
                    s.autocomplete,
                    |s, v| s.autocomplete = v,
                    cx,
                ),
            ));

        let grid = v_flex()
            .child(section_title(&t, "Data grid"))
            .child(setting_row(
                &t,
                "Rows per page",
                "Fetched at a time in table tabs",
                choices(
                    &t,
                    "rows",
                    [100usize, 500, 1000, 5000]
                        .map(|v| (v, v.to_string()))
                        .to_vec(),
                    &s.rows_per_page,
                    |s, v| s.rows_per_page = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Show NULL as",
                "How empty values read in cells",
                choices(
                    &t,
                    "null",
                    ["NULL", "null", "∅", "—"]
                        .map(|v| (v.to_string(), v.to_string()))
                        .to_vec(),
                    &s.null_display,
                    |s, v| s.null_display = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Max column width",
                "Longer values are cut off; the value editor shows all of it",
                choices(
                    &t,
                    "colw",
                    [240.0f32, 320.0, 420.0, 640.0]
                        .map(|v| (v, format!("{v}")))
                        .to_vec(),
                    &s.max_column_width,
                    |s, v| s.max_column_width = v,
                    cx,
                ),
            ));

        let safety = v_flex()
            .child(section_title(&t, "Safety & connections"))
            .child(setting_row(
                &t,
                "Confirm on PROD",
                "Ask before statements that change data on PROD connections",
                toggle("prod", s.confirm_on_prod, |s, v| s.confirm_on_prod = v, cx),
            ))
            .child(setting_row(
                &t,
                "Transactions",
                "For new connections: commit each statement, or commit yourself",
                choices(
                    &t,
                    "tx",
                    vec![
                        ("auto".to_string(), "Auto-commit".into()),
                        ("manual".to_string(), "Manual".into()),
                    ],
                    &s.default_transaction_mode,
                    |s, v| s.default_transaction_mode = v,
                    cx,
                ),
            ))
            .child(setting_row(
                &t,
                "Statement timeout",
                "Default for connections that don't set one",
                choices(
                    &t,
                    "timeout",
                    vec![
                        (0u64, "Off".into()),
                        (30, "30s".into()),
                        (60, "1m".into()),
                        (300, "5m".into()),
                    ],
                    &s.default_statement_timeout_secs,
                    |s, v| s.default_statement_timeout_secs = v,
                    cx,
                ),
            ));

        let footer = modal_footer(&t)
            .justify_between()
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        div()
                            .id("open-settings-json")
                            .text_xs()
                            .text_color(t.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text_primary))
                            .child("Edit settings.json")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings_open = false;
                                this.open_config_file(false, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("open-themes")
                            .text_xs()
                            .text_color(t.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text_primary))
                            .child("Open themes folder")
                            .on_click(cx.listener(|_, _, _, cx| {
                                if let Some(dir) = crate::theme::themes_dir() {
                                    if !dir.exists() {
                                        let _ = std::fs::create_dir_all(&dir);
                                    }
                                    cx.reveal_path(&dir);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(t.text_muted)
                    .child("Saved as you go"),
            );

        Some(
            modal_backdrop("settings-backdrop", dismiss_on_click, cx)
                .child(
                    modal_card("settings-card", 640.0, &t)
                        .max_h(px(640.0))
                        .overflow_hidden()
                        .child(modal_header(
                            &t,
                            "Settings",
                            Some(crate::ui::format_keys("Quarry · ⌘,").into()),
                            dismiss_on_click,
                        ))
                        .child(
                            modal_body("settings-body")
                                .flex_1()
                                .gap_6()
                                .child(appearance)
                                .child(editor)
                                .child(grid)
                                .child(safety),
                        )
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
