// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Settings (⌘,): the everyday options as one quiet list. Every change saves to settings.json,
//! so hand edits and the modal never disagree.

use crate::settings::{self, Settings};
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::switch::Switch;
use gpui::component::{Icon, Sizable};
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
        const THEME_HINT: &str = "Built-ins, or drop a .json into the themes folder";
        let few_themes = themes.len() <= crate::theme::BUILT_IN.len() + 1;
        let theme_choices = choices(&t, "theme", themes, &current_theme, |s, v| s.theme = v, cx);
        // Built-ins sit on the right like every other row; many user themes get the full width
        let theme_row = if few_themes {
            setting_row(&t, "Theme", THEME_HINT, theme_choices)
        } else {
            v_flex()
                .py_3()
                .gap_2()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_sm().text_color(t.text_primary).child("Theme"))
                        .child(div().text_sm().text_color(t.text_muted).child(THEME_HINT)),
                )
                .child(theme_choices)
        };

        let current_ui_font = if s.ui_font_family.eq_ignore_ascii_case("Satoshi") {
            String::new()
        } else {
            s.ui_font_family.clone()
        };
        let mut ui_font_options = vec![
            ("".to_string(), "Space Grotesk (Default)".to_string()),
            ("Satoshi".to_string(), "Satoshi".to_string()),
            ("Geist".to_string(), "Geist".to_string()),
        ];
        if !current_ui_font.is_empty()
            && !ui_font_options
                .iter()
                .any(|(v, _)| v.eq_ignore_ascii_case(&current_ui_font))
        {
            ui_font_options.push((current_ui_font.clone(), current_ui_font.clone()));
        }

        let appearance = vec![
            theme_row,
            setting_row(
                &t,
                "Window translucency",
                "Blur the desktop behind the window, with any theme",
                choices(
                    &t,
                    "window-opacity",
                    [
                        (1.0f32, "Off".to_string()),
                        (0.9, "Subtle".into()),
                        (0.8, "Medium".into()),
                        (0.65, "Strong".into()),
                    ]
                    .to_vec(),
                    &s.window_opacity,
                    |s, v| s.window_opacity = v,
                    cx,
                ),
            ),
            setting_row(
                &t,
                "Interface font",
                "Application typography",
                choices(
                    &t,
                    "ui-font",
                    ui_font_options,
                    &current_ui_font,
                    |s, v| s.ui_font_family = v,
                    cx,
                ),
            ),
            setting_row(
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
            ),
            setting_row(
                &t,
                "Editor & data size",
                crate::ui::format_keys("SQL and grid cells. Auto is a step above the interface; ⌘ scroll in the editor adjusts it"),
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
                        (18.0, "18".into()),
                    ]
                    .to_vec(),
                    &s.mono_font_size,
                    |s, v| s.mono_font_size = v,
                    cx,
                ),
            ),
        ];

        let editor = vec![
            setting_row(
                &t,
                "Vim mode",
                "Normal, Insert and Visual modes with motions and operators (dd, cw, yy…)",
                toggle("vim", s.vim_mode, |s, v| s.vim_mode = v, cx),
            ),
            setting_row(
                &t,
                "Block cursor",
                "In Vim's Normal and Visual modes; Insert keeps the bar",
                toggle(
                    "vim-block",
                    s.vim_block_cursor,
                    |s, v| s.vim_block_cursor = v,
                    cx,
                ),
            ),
            setting_row(
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
            ),
            setting_row(
                &t,
                "Soft wrap",
                "Wrap long lines instead of scrolling sideways",
                toggle("soft-wrap", s.soft_wrap, |s, v| s.soft_wrap = v, cx),
            ),
            setting_row(
                &t,
                "Line numbers",
                "In the gutter",
                toggle(
                    "line-numbers",
                    s.line_numbers,
                    |s, v| s.line_numbers = v,
                    cx,
                ),
            ),
            setting_row(
                &t,
                "Highlight current statement",
                crate::ui::format_keys("Tint what ⌘↵ will run"),
                toggle(
                    "highlight",
                    s.highlight_current_statement,
                    |s, v| s.highlight_current_statement = v,
                    cx,
                ),
            ),
            setting_row(
                &t,
                "Autocomplete",
                "Tables, columns and joins as you type",
                toggle(
                    "autocomplete",
                    s.autocomplete,
                    |s, v| s.autocomplete = v,
                    cx,
                ),
            ),
        ];

        let grid = vec![
            setting_row(
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
            ),
            setting_row(
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
            ),
            setting_row(
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
            ),
        ];

        let safety = vec![
            setting_row(
                &t,
                "Confirm on PROD",
                "Ask before statements that change data on PROD connections",
                toggle("prod", s.confirm_on_prod, |s, v| s.confirm_on_prod = v, cx),
            ),
            setting_row(
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
            ),
            setting_row(
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
            ),
        ];

        // System Settings style: sections on the left, the picked one's rows on the right
        let sections = [
            ("Appearance", IconName::Eye, appearance),
            ("Editor", IconName::SquareTerminal, editor),
            ("Data grid", IconName::Table, grid),
            ("Safety", IconName::Lock, safety),
        ];
        let current = self.settings_section.min(sections.len() - 1);
        let nav = v_flex()
            .w(px(180.0))
            .flex_none()
            .p_2()
            .gap_0p5()
            .bg(t.bg_panel)
            .border_r_1()
            .border_color(t.border)
            .children(sections.iter().enumerate().map(|(i, (name, icon, _))| {
                let on = i == current;
                h_flex()
                    .id(("settings-nav", i))
                    .h(px(crate::ui::CONTROL_H))
                    .px_2p5()
                    .gap_2()
                    .items_center()
                    .rounded(px(crate::ui::RADIUS_MD))
                    .cursor_pointer()
                    .text_sm()
                    .when(on, |d| d.bg(t.bg_selected).text_color(t.text_primary))
                    .when(!on, |d| {
                        d.text_color(t.text_secondary).hover(|s| s.bg(t.bg_hover))
                    })
                    .child(Icon::new(*icon).small().text_color(if on {
                        t.accent
                    } else {
                        t.text_muted
                    }))
                    .child(*name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_section = i;
                        cx.notify();
                    }))
            }));
        let (title, _, rows) = sections.into_iter().nth(current).unwrap();
        let page = v_flex()
            .gap_4()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(settings_group(&t, rows));

        let footer = modal_footer(&t)
            .justify_between()
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        div()
                            .id("open-settings-json")
                            .text_sm()
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
                            .text_sm()
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
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("Saved as you go"),
            );

        Some(
            crate::ui::fade_in(
                "settings-backdrop-fade",
                modal_backdrop("settings-backdrop", dismiss_on_click, cx).child(
                    modal_card("settings-card", 820.0, &t)
                        .h(px(560.0))
                        .max_h_full()
                        .overflow_hidden()
                        .child(modal_header(&t, "Settings", None, dismiss_on_click))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_h_0()
                                .items_stretch()
                                .border_t_1()
                                .border_color(t.border)
                                .child(nav)
                                .child(modal_body("settings-body").flex_1().child(page)),
                        )
                        .child(footer),
                ),
            )
            .into_any_element(),
        )
    }
}
