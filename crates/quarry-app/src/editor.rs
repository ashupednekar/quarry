// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::Environment;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Editor, EditorState};
use gpui::component::{Icon, Sizable};
use gpui::*;

pub struct QueryEditor;

const RUN_COL_W: f32 = 18.0;

impl QueryEditor {
    /// `connection`: the active connection's name and environment, shown at the left of the toolbar
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        editor: &Entity<EditorState>,
        is_running: bool,
        soft_wrap: bool,
        connection: Option<(&str, Environment)>,
        // `Some` when the dialect can switch schemas: the tab's schema label ("default" when unset)
        schema: Option<String>,
        row_limit: Option<u32>,
        // How the connection's dialect splits statements, for the ▶ buttons
        rules: quarry_sql::SplitRules,
        vim_mode: Option<crate::state::VimMode>,
        theme: &Theme,
        cx: &Context<QuarryWindow>,
    ) -> impl IntoElement {
        let picker = |id: &'static str, icon: IconName, text: String, on: bool| {
            h_flex()
                .id(id)
                .h(px(24.0))
                .px_2()
                .gap_1()
                .items_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .text_xs()
                .text_color(if on {
                    theme.text_secondary
                } else {
                    theme.text_muted
                })
                .hover(|s| s.bg(theme.bg_hover))
                .child(Icon::new(icon).xsmall().text_color(theme.text_muted))
                .child(text)
                .child(
                    Icon::new(IconName::ChevronDown)
                        .xsmall()
                        .text_color(theme.text_muted),
                )
        };
        let schema_picker = schema.map(|s| {
            picker("btn-schema", IconName::Layers, s, true)
                .on_click(cx.listener(move |this, _, window, cx| this.open_schema_menu(window, cx)))
        });
        let limit_picker = picker(
            "btn-limit",
            IconName::ListFilter,
            row_limit.map_or("No limit".to_string(), |n| format!("Limit {n}")),
            row_limit.is_some(),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open_limit_menu(window, cx)));
        let is_prod = connection.is_some_and(|(_, env)| env == Environment::Prod);
        let conn_label = match connection {
            Some((name, env)) => h_flex()
                .gap_1p5()
                .items_center()
                .child(div().size(px(6.0)).rounded_full().bg(env_color(env, theme)))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(name.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(env_color(env, theme))
                        .child(env.label()),
                ),
            None => h_flex().child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("Choose a connection"),
            ),
        };
        let conn_label = conn_label
            .id("btn-connection")
            .gap_1p5()
            .items_center()
            .h(px(24.0))
            .px_2()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .on_click(cx.listener(move |this, _, window, cx| this.open_connection_menu(window, cx)))
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(theme.text_muted),
            );
        v_flex()
            .size_full()
            .bg(theme.bg_panel)
            .child(
                h_flex()
                    .h(px(BAR_H))
                    .px_3()
                    // A faint red wash on prod, so it's obvious before you run anything
                    .bg(if is_prod {
                        Theme::tint(theme.danger, 0.08)
                    } else {
                        theme.bg_sidebar.into()
                    })
                    .border_b_1()
                    .border_color(theme.border)
                    .gap_1()
                    .items_center()
                    .child(conn_label)
                    .children(schema_picker)
                    .child(limit_picker)
                    .children(vim_mode.map(|mode| {
                        div()
                            .id("vim-mode-badge")
                            .px_2()
                            .h(px(22.0))
                            .rounded(px(3.0))
                            .flex()
                            .items_center()
                            .bg(theme.bg_hover)
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(match mode {
                                crate::state::VimMode::Normal => theme.accent,
                                crate::state::VimMode::Insert => theme.success,
                                crate::state::VimMode::Visual => theme.warning,
                            })
                            .child(mode.label())
                    }))
                    .child(div().flex_1())
                    .child(toolbar_button(
                        "btn-save",
                        IconName::Save,
                        "Save",
                        "Save (⌘S)",
                        Box::new(crate::SaveFile),
                    ))
                    .child(toolbar_button(
                        "btn-format",
                        IconName::Wand,
                        "Format",
                        "Format (⇧⌥F)",
                        Box::new(crate::FormatSql),
                    ))
                    .child(toolbar_button(
                        "btn-explain",
                        IconName::ListTree,
                        "Explain",
                        "Explain (⌥⌘E)",
                        Box::new(crate::Explain),
                    ))
                    .child(
                        zbutton("btn-run-query")
                            .h(px(24.0))
                            .px_2p5()
                            .ml_1()
                            .icon(IconName::Play)
                            .label(if is_running { "Running…" } else { "Run" })
                            .tooltip(crate::ui::format_keys(
                                "Run the selection, or the statement at the cursor (⌘↵)",
                            ))
                            .primary()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.refresh_active_tab(window, cx)
                            })),
                    ),
            )
            // The left padding is the ▶ column, so the button never sits on the line numbers
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(px(RUN_COL_W))
                    .bg(theme.bg_app)
                    .when(vim_mode.is_some(), |d| {
                        d.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if this.handle_vim_key(event, window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                    })
                    .child(Editor::new(editor).h_full().bordered(false)),
            )
            .children(run_gutter_buttons(
                editor, is_running, soft_wrap, rules, theme, cx,
            ))
    }
}

pub fn env_color(env: Environment, theme: &Theme) -> Rgba {
    match env {
        Environment::Local => theme.success,
        Environment::Staging => theme.warning,
        Environment::Prod => theme.danger,
    }
}

/// ▶ in the gutter on the first line of every statement in view; clicking one moves the cursor there and runs it.
/// Rows are placed from the editor's line height and scroll offset, which only holds while every line above is
/// one row high; with soft wrap on, a statement is skipped when a line above it might wrap.
/// ponytail: "might wrap" guesses a monospace char at 0.45 × line height; exact needs per-line layout from gpui-kit
fn run_gutter_buttons(
    editor: &Entity<EditorState>,
    is_running: bool,
    soft_wrap: bool,
    rules: quarry_sql::SplitRules,
    theme: &Theme,
    cx: &Context<QuarryWindow>,
) -> Vec<AnyElement> {
    let e = editor.read(cx);
    let Some(line_height) = e.line_height() else {
        return vec![];
    };
    let scroll_y = e.scroll_offset().y;
    let sql = e.value();
    let area = e.input_bounds();
    let starts: Vec<usize> = if rules.go_batches || rules.slash_blocks {
        quarry_sql::split_statements_with(&sql, rules)
            .into_iter()
            .map(|s| s.start)
            .collect()
    } else {
        quarry_sql::tree_sitter_split_statements(&sql)
            .into_iter()
            .map(|s| s.start)
            .collect()
    };
    let lines: Vec<usize> = sql.split('\n').map(|l| l.chars().count()).collect();
    let wrap_cols = (area.size.width / (line_height * 0.45)).floor() as usize;
    let fits = |line: usize| {
        !soft_wrap
            || lines[..line.min(lines.len())]
                .iter()
                .all(|n| *n < wrap_cols)
    };
    let x = area.origin.x - px(RUN_COL_W) + px(3.0);
    starts
        .into_iter()
        .enumerate()
        .filter_map(|(ix, start)| {
            let line = sql[..start.min(sql.len())].matches('\n').count();
            if !fits(line) {
                return None;
            }
            let y = area.origin.y + scroll_y + line_height * line as f32;
            if y < area.origin.y || y + line_height > area.origin.y + area.size.height {
                return None; // scrolled out of view
            }
            let editor = editor.clone();
            let button = div()
                .id(("gutter-run", ix))
                .w(px(14.0))
                .h(line_height)
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_muted)
                .when(!is_running, |d| d.hover(|s| s.text_color(theme.success)))
                .cursor_pointer()
                .tooltip(|window, cx| {
                    gpui::component::tooltip::Tooltip::new(crate::ui::format_keys(
                        "Run statement (⌘↵)",
                    ))
                    .build(window, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    editor.update(cx, |e, cx| e.set_selected_range(start..start, cx));
                    this.refresh_active_tab(window, cx)
                }))
                .child(Icon::new(IconName::Play).xsmall());
            Some(deferred(anchored().position(point(x, y)).child(button)).into_any_element())
        })
        .collect()
}

fn toolbar_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    tip: &'static str,
    action: Box<dyn Action>,
) -> impl IntoElement {
    zbutton(id)
        .h(px(24.0))
        .px_2()
        .icon(icon)
        .label(label)
        .ghost()
        .xsmall()
        .tooltip(crate::ui::format_keys(tip))
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}
