// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::*;
use gpui::component::input::{Input, InputState};
use gpui::*;

pub struct CommandBar;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommandItem {
    pub id: String,
    pub title: String,
    pub category: &'static str,
    pub tag: &'static str,
    pub hint: Option<&'static str>,
}

impl CommandBar {
    pub fn get_candidates(state: &AppState, input: &str) -> Vec<CommandItem> {
        let input = input.trim();
        let mut items = Vec::new();

        // 1. Direct query execution if user typed SQL
        let upper = input.to_uppercase();
        if upper.starts_with("SELECT") || upper.starts_with("INSERT") || upper.starts_with("UPDATE") || upper.starts_with("DELETE") {
            items.push(CommandItem {
                id: format!("run:{}", input),
                title: format!("Run query: {}", input),
                category: "QUERY",
                tag: "SQL",
                hint: Some("Enter to run"),
            });
        }

        // 2. Filter tables from catalog
        for tbl in &state.catalog.tables {
            if input.is_empty() || tbl.table_name.to_lowercase().contains(&input.to_lowercase()) {
                items.push(CommandItem {
                    id: format!("table:{}", tbl.table_name),
                    title: format!("Open table: {}", tbl.table_name),
                    category: "TABLE",
                    tag: "TBL",
                    hint: Some("Enter to open"),
                });
            }
        }

        // 3. Actions
        let actions = [
            ("action:new_query", "New Query Tab", "ACTION", "ACT", "⌘N"),
            ("action:review", "Review Pending Changes", "ACTION", "ACT", "⌘S"),
            ("action:add_conn", "Add Database Connection", "ACTION", "ACT", "+"),
            ("action:toggle_sidebar", "Toggle Sidebar", "VIEW", "UI", "⌘B"),
            ("action:toggle_bottom", "Toggle Bottom Panel", "VIEW", "UI", "⌘J"),
            ("action:theme", "Toggle Light/Dark Theme", "VIEW", "UI", ""),
            ("action:demo", "Open Demo Database", "ACTION", "DB", ""),
            ("action:toggle_tx", "Toggle Manual Commit Mode", "ACTION", "TX", ""),
            ("action:commit_tx", "Commit Transaction", "ACTION", "TX", ""),
            ("action:rollback_tx", "Roll Back Transaction", "ACTION", "TX", ""),
        ];

        for (id, title, cat, tag, hint) in actions {
            if input.is_empty() || title.to_lowercase().contains(&input.to_lowercase()) {
                items.push(CommandItem {
                    id: id.to_string(),
                    title: title.to_string(),
                    category: cat,
                    tag,
                    hint: Some(hint),
                });
            }
        }

        items
    }

    pub fn render<T: 'static>(
        state: &AppState,
        theme: &Theme,
        cx: &Context<T>,
        input: &Entity<InputState>,
        on_select: impl Fn(&mut T, String, &mut Window, &mut Context<T>) + Copy + 'static,
        on_close: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        if !state.command_bar_open {
            return div().into_any_element();
        }

        let items = Self::get_candidates(state, &input.read(cx).value());
        let selected_idx = state.command_bar_selected_index.min(items.len().saturating_sub(1));

        div()
            .id("command-bar-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000080))
            .flex()
            .justify_center()
            .pt(px(72.0))
            .on_click(cx.listener(move |this, _, window, cx| {
                on_close(this, window, cx);
            }))
            .child(
                v_flex()
                    .id("command-bar-card")
                    .key_context("CommandBar")
                    .w(px(580.0))
                    .max_h(px(420.0))
                    .rounded(px(8.0))
                    .bg(theme.bg_sidebar)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .on_click(|_, _, cx| cx.stop_propagation()) // keep clicks off the backdrop
                    .child(
                        // Input Bar
                        h_flex()
                            .h(px(42.0))
                            .px_3()
                            .bg(theme.bg_panel)
                            .border_b_1()
                            .border_color(theme.border)
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2p5()
                                    .items_center()
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py(px(0.5))
                                            .rounded(px(3.0))
                                            .bg(theme.bg_surface)
                                            .text_xs()
                                            .text_color(theme.accent)
                                            .font_weight(FontWeight::BOLD)
                                            .child("⌘K"),
                                    )
                                    .child(div().w(px(460.0)).child(Input::new(input).appearance(false))),
                            )
                            .child(
                                div()
                                    .id("cmd-close-btn")
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
                        // Candidate items list
                        v_flex()
                            .id("command-bar-scroll")
                            .overflow_y_scroll()
                            .p_1p5()
                            .gap_0p5()
                            .children(items.iter().enumerate().map(|(idx, item)| {
                                let is_selected = idx == selected_idx;
                                let bg = if is_selected { theme.bg_selected } else { theme.bg_sidebar };
                                let item_id = item.id.clone();

                                h_flex()
                                    .id(("cmd-item", idx))
                                    .px_3()
                                    .py_2()
                                    .rounded(px(4.0))
                                    .bg(bg)
                                    .justify_between()
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        on_select(this, item_id.clone(), window, cx);
                                    }))
                                    .child(
                                        h_flex()
                                            .gap_2p5()
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
                                                    .child(item.tag),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(if is_selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                                    .text_color(if is_selected { theme.text_primary } else { theme.text_secondary })
                                                    .child(item.title.clone()),
                                            ),
                                    )
                                    .when_some(item.hint, |this, hint| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .child(hint),
                                        )
                                    })
                            })),
                    ),
            )
            .into_any_element()
    }
}
