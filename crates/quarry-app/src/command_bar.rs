// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! The command bar (U1-U3, U6): one box for tables, connections, recent queries, SQL and commands.

use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::*;
use gpui::assets::IconName;
use gpui::component::input::{Input, InputState};
use gpui::component::{Icon, Sizable};
use gpui::*;

pub struct CommandBar;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Sql,
    Table,
    View,
    Connection,
    Recent,
    Command,
}

impl Kind {
    /// Section the palette files this under
    fn group(self) -> &'static str {
        match self {
            Kind::Sql => "Run",
            Kind::Table | Kind::View => "Tables",
            Kind::Recent => "Queries",
            Kind::Connection => "Connections",
            Kind::Command => "Commands",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Kind::Sql => IconName::Play,
            Kind::Table => IconName::Table,
            Kind::View => IconName::Eye,
            Kind::Connection => IconName::Database,
            Kind::Recent => IconName::ScrollText,
            Kind::Command => IconName::SquareTerminal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandItem {
    pub id: String,
    pub title: String,
    /// Muted context: schema, connection, or "Recent query"
    pub detail: String,
    pub kind: Kind,
    pub hint: &'static str,
}

/// Commands: (id, title, shortcut)
const COMMANDS: &[(&str, &str, &str)] = &[
    ("action:new_query", "New Query", "⌘N"),
    ("action:open_file", "Open File…", "⌘O"),
    ("action:maximize", "Maximize Editor (toggle)", "⇧⎋"),
    ("action:save_file", "Save Query", "⌘S"),
    ("action:reopen_tab", "Reopen Closed Tab", "⇧⌘T"),
    ("action:split", "Toggle Split View", "⌘\\"),
    ("action:pin", "Pin / Unpin Current Tab", ""),
    ("action:format", "Format SQL", "⇧⌥F"),
    ("action:explain", "Explain Query Plan", "⌥⌘E"),
    ("action:explain_analyze", "Explain Analyze", "⇧⌥⌘E"),
    ("action:review", "Review Pending Changes", "⌘S"),
    ("action:export", "Export Results…", "⇧⌘E"),
    ("action:import", "Import Data (CSV, JSON)…", ""),
    ("action:dump", "Dump Database to SQL…", ""),
    ("action:add_conn", "Add Connection…", ""),
    ("action:demo", "Open Demo Database", ""),
    ("action:toggle_tx", "Toggle Manual Commit Mode", ""),
    ("action:commit_tx", "Commit Transaction", ""),
    ("action:rollback_tx", "Roll Back Transaction", ""),
    ("action:toggle_sidebar", "Toggle Sidebar", "⌘B"),
    ("action:toggle_bottom", "Toggle Bottom Panel", "⌘J"),
    ("action:theme", "Toggle Light/Dark Theme", ""),
    ("action:increase_font", "Zoom In (Increase Font Size)", "⌘+"),
    ("action:decrease_font", "Zoom Out (Decrease Font Size)", "⌘-"),
    ("action:reset_font", "Reset Zoom (Actual Size)", "⌘0"),
    ("action:settings", "Settings…", "⌘,"),
    ("action:keymap", "Open Keymap", ""),
];

/// Commands shown when the box is empty
const EVERYDAY: &[&str] = &["action:new_query", "action:open_file", "action:add_conn"];

const SQL_VERBS: &[&str] = &["select", "with", "insert", "update", "delete", "explain", "create", "alter", "drop", "show", "pragma", "values", "table"];

/// Higher is better; `None` is no match. Characters must appear in order (fuzzy), rewarded for
/// runs, word starts and prefixes. For queries of 4+ characters one typo is forgiven.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = target.to_lowercase().chars().collect();
    let (mut from, mut score, mut prev) = (0usize, 0i32, None::<usize>);
    let mut matched = true;
    for &qc in &q {
        let Some(gap) = t.get(from..).and_then(|rest| rest.iter().position(|&c| c == qc)) else {
            matched = false;
            break;
        };
        let at = from + gap;
        score += 10 - gap.min(9) as i32;
        if prev.is_some_and(|p| p + 1 == at) {
            score += 15;
        }
        if at == 0 || !t[at - 1].is_alphanumeric() {
            score += 10;
        }
        prev = Some(at);
        from = at + 1;
    }
    if matched {
        return Some(score + if t.starts_with(&q) { 50 } else { 0 });
    }
    // Typo tolerance: any window of similar length within one edit
    if q.len() >= 4 {
        for len in [q.len(), q.len() + 1, q.len() - 1] {
            if len <= t.len() && t.windows(len).any(|w| edit_distance(&q, w) <= 1) {
                return Some(1);
            }
        }
    }
    None
}

/// Optimal string alignment distance (insert, delete, substitute, transpose)
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

impl CommandBar {
    pub fn get_candidates(state: &AppState, input: &str) -> Vec<CommandItem> {
        let raw = input.trim();
        // ">" = commands only, "/" = tables only (U2)
        let (query, only_commands, only_tables) = match raw.chars().next() {
            Some('>') => (raw[1..].trim(), true, false),
            Some('/') => (raw[1..].trim(), false, true),
            _ => (raw, false, false),
        };
        let conn = state.active_connection.as_ref().map(|c| c.name.clone()).unwrap_or_default();
        let mut scored: Vec<(i32, CommandItem)> = Vec::new();
        let mut push = |score: Option<i32>, bias: i32, item: CommandItem| {
            if let Some(s) = score {
                scored.push((s + bias, item));
            }
        };

        // Typed SQL: running it is the top result
        let first_word = raw.split_whitespace().next().unwrap_or("").to_lowercase();
        if !only_commands && !only_tables && SQL_VERBS.contains(&first_word.as_str()) && raw.contains(' ') {
            push(Some(10_000), 0, CommandItem {
                id: format!("run:{raw}"),
                title: raw.to_string(),
                detail: "Run this query".into(),
                kind: Kind::Sql,
                hint: "↵",
            });
        }

        // Empty box: recents first (U6)
        if query.is_empty() && !only_commands {
            for (i, (schema, table)) in state.recent_tables.iter().enumerate() {
                push(Some(900 - i as i32), 0, CommandItem {
                    id: format!("table:{schema}\u{1f}{table}"),
                    title: table.clone(),
                    detail: format!("Recent · {schema}"),
                    kind: Kind::Table,
                    hint: "↵",
                });
            }
        }

        if !only_commands {
            for t in &state.catalog.tables {
                let is_recent = state.recent_tables.iter().any(|(s, n)| *s == t.schema_name && *n == t.table_name);
                if query.is_empty() && is_recent {
                    continue; // already listed under Recent
                }
                push(fuzzy_score(query, &t.table_name), if query.is_empty() { -1000 } else { 30 }, CommandItem {
                    id: format!("table:{}\u{1f}{}", t.schema_name, t.table_name),
                    title: t.table_name.clone(),
                    detail: format!("{} · {}", t.schema_name, conn),
                    kind: if t.is_view { Kind::View } else { Kind::Table },
                    hint: "↵",
                });
                // The table's other views, once its name is being typed
                if !query.is_empty() {
                    for (view, label) in [("structure", "Structure"), ("ddl", "DDL")] {
                        push(fuzzy_score(query, &t.table_name), 20, CommandItem {
                            id: format!("table-{view}:{}\u{1f}{}", t.schema_name, t.table_name),
                            title: format!("{} · {label}", t.table_name),
                            detail: format!("{} · {}", t.schema_name, conn),
                            kind: if t.is_view { Kind::View } else { Kind::Table },
                            hint: "↵",
                        });
                    }
                }
            }
        }

        if !only_commands && !only_tables {
            for c in &state.saved_connections {
                let active = state.active_connection.as_ref().is_some_and(|a| a.id == c.id);
                push(fuzzy_score(query, &c.name), if query.is_empty() { -2000 } else { 10 }, CommandItem {
                    id: format!("connection:{}", c.id),
                    title: c.name.clone(),
                    detail: if active { "Connected".into() } else { format!("Switch connection · {}", c.env.label()) },
                    kind: Kind::Connection,
                    hint: "↵",
                });
            }
            // Pinned queries on this connection
            let pinned = state.active_connection.as_ref().and_then(|c| state.pins.get(&c.id)).cloned().unwrap_or_default();
            for (i, pin) in pinned.iter().enumerate() {
                if let crate::pins::Pin::Query { title, .. } = pin {
                    push(if query.is_empty() { Some(800 - i as i32) } else { fuzzy_score(query, title) }, 15, CommandItem {
                        id: format!("pin:{i}"),
                        title: title.clone(),
                        detail: "Pinned query".into(),
                        kind: Kind::Recent,
                        hint: "↵",
                    });
                }
            }
            // Recent queries, newest first; only once there's something typed or as the last recents
            let mut seen: Vec<&str> = Vec::new();
            for (i, h) in state.history.iter().enumerate().rev().filter(|(_, h)| h.error.is_none()) {
                if seen.contains(&h.sql.as_str()) || seen.len() >= 20 {
                    continue;
                }
                seen.push(&h.sql);
                let one_line: String = h.sql.split_whitespace().collect::<Vec<_>>().join(" ");
                let score = if query.is_empty() { (seen.len() <= 5).then_some(500 - seen.len() as i32) } else { fuzzy_score(query, &one_line) };
                push(score, -5, CommandItem {
                    id: format!("history:{i}"),
                    title: one_line,
                    detail: format!("Recent query · {}", h.connection),
                    kind: Kind::Recent,
                    hint: "↵",
                });
            }
        }

        if !only_tables {
            for (id, title, hint) in COMMANDS {
                let score = if query.is_empty() && !only_commands {
                    EVERYDAY.contains(id).then_some(100)
                } else {
                    fuzzy_score(query, title)
                };
                push(score, 0, CommandItem { id: id.to_string(), title: title.to_string(), detail: String::new(), kind: Kind::Command, hint });
            }
        }

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        // A short list reads at a glance; typing narrows it rather than scrolling
        let limit = if query.is_empty() { 8 } else { 14 };
        let mut items: Vec<CommandItem> = scored.into_iter().take(limit).map(|(_, item)| item).collect();
        // Group by section, sections in order of their best match (stable, so ranking holds inside each)
        let first_seen = |g: &str, items: &[CommandItem]| items.iter().position(|i| i.kind.group() == g).unwrap_or(usize::MAX);
        let order: Vec<(String, usize)> = items.iter().map(|i| (i.kind.group().to_string(), first_seen(i.kind.group(), &items))).collect();
        let mut keyed: Vec<(usize, CommandItem)> = order.into_iter().map(|(_, k)| k).zip(items.drain(..)).collect();
        keyed.sort_by_key(|(k, _)| *k);
        keyed.into_iter().map(|(_, i)| i).collect()
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

        let typed = input.read(cx).value();
        let items = Self::get_candidates(state, &typed);
        let selected_idx = state.command_bar_selected_index.min(items.len().saturating_sub(1));

        let kbd = |k: &'static str| {
            div().px_1p5().h(px(18.0)).flex().items_center().rounded(px(4.0)).bg(theme.bg_surface).text_xs().text_color(theme.text_muted).child(k)
        };
        let mut list = v_flex().id("command-bar-scroll").overflow_y_scroll().px_2().py_2().gap_px();
        if items.is_empty() {
            list = list.child(div().px_3().py_6().text_sm().text_color(theme.text_muted).child("Nothing matches. Try > for commands or / for tables."));
        }
        let mut last_group = "";
        for (idx, item) in items.iter().enumerate() {
            let group = item.kind.group();
            if group != last_group {
                last_group = group;
                list = list.child(div().px_3().pt(px(if idx == 0 { 4.0 } else { 12.0 })).pb_1().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme.text_muted).child(group));
            }
            let is_selected = idx == selected_idx;
            let item_id = item.id.clone();
            list = list.child(
                h_flex()
                    .id(("cmd-item", idx))
                    .h(px(36.0))
                    .px_3()
                    .gap_3()
                    .rounded(px(6.0))
                    .items_center()
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(theme.bg_selected))
                    .when(!is_selected, |d| d.hover(|s| s.bg(theme.bg_hover)))
                    .on_click(cx.listener(move |this, _, window, cx| on_select(this, item_id.clone(), window, cx)))
                    .child(Icon::new(item.kind.icon()).small().text_color(if is_selected { theme.accent } else { theme.text_muted }))
                    .child(div().max_w(px(380.0)).text_sm().text_color(theme.text_primary).truncate().child(item.title.clone()))
                    .child(div().flex_1().min_w_0().text_xs().text_color(theme.text_muted).truncate().child(item.detail.clone()))
                    .when(!item.hint.is_empty() && item.hint != "↵", |d| d.child(kbd(item.hint))),
            );
        }

        modal_backdrop("command-bar-backdrop", cx.listener(move |this, _, window, cx| on_close(this, window, cx)))
            .child(
                modal_card("command-bar-card", 640.0, theme)
                    .key_context("CommandBar")
                    .max_h(px(520.0))
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .h(px(52.0))
                            .px_4()
                            .gap_3()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(Icon::new(IconName::Search).text_color(theme.text_muted))
                            .child(div().flex_1().text_base().child(Input::new(input).appearance(false))),
                    )
                    .child(list.flex_1().min_h_0())
                    .child(
                        h_flex()
                            .h(px(36.0))
                            .px_4()
                            .gap_4()
                            .items_center()
                            .border_t_1()
                            .border_color(theme.border)
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(h_flex().gap_1p5().items_center().child(kbd("↑↓")).child("navigate"))
                            .child(h_flex().gap_1p5().items_center().child(kbd("↵")).child("open"))
                            .child(h_flex().gap_1p5().items_center().child(kbd("esc")).child("close"))
                            .child(div().flex_1())
                            .child("> commands · / tables · or type SQL"),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_prefers_prefixes_and_forgives_one_typo() {
        assert!(fuzzy_score("usr", "users").is_some());
        assert!(fuzzy_score("ord", "orders") > fuzzy_score("ord", "customer_records"));
        assert!(fuzzy_score("cust", "customers") > fuzzy_score("cust", "accounts_customers"));
        assert!(fuzzy_score("cusotmers", "customers").is_some()); // transposed letters
        assert!(fuzzy_score("prodcts", "products").is_some()); // missing letter
        assert!(fuzzy_score("xyz", "users").is_none());
    }
}
