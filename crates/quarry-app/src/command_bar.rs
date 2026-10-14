// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! The command bar (U1-U3, U6): one box for tables, connections, recent queries, SQL and commands.

use crate::history;
use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
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
    Usage,
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
            Kind::Usage => "Usages",
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
            Kind::Usage => IconName::FileCode,
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
    ("action:compare_schema", "Compare Schema With…", ""),
    ("action:open_file", "Open File…", "⌘O"),
    ("action:maximize", "Maximize Editor (toggle)", "⇧⎋"),
    ("action:save_file", "Save Query", "⌘S"),
    ("action:reopen_tab", "Reopen Closed Tab", "⇧⌘T"),
    ("action:split", "Toggle Split View", "⌘\\"),
    (
        "action:toggle_split_direction",
        "Toggle Split Orientation (Vertical / Horizontal)",
        "⌥⌘\\",
    ),
    ("action:focus_other_pane", "Focus Other Split Pane", "⌥⌘→"),
    ("action:swap_split_panes", "Swap Split Panes", ""),
    (
        "action:move_pane_left",
        "Move Active Split Pane to Left",
        "⌥⌘[",
    ),
    (
        "action:move_pane_right",
        "Move Active Split Pane to Right",
        "⌥⌘]",
    ),
    (
        "action:move_pane_up",
        "Move Active Split Pane to Top",
        "⌥⇧⌘[",
    ),
    (
        "action:move_pane_down",
        "Move Active Split Pane to Bottom",
        "⌥⇧⌘]",
    ),
    ("action:close_all", "Close All Tabs", ""),
    ("action:pin", "Pin / Unpin Current Tab", ""),
    ("action:format", "Format SQL", "⇧⌥F"),
    ("action:explain", "Explain Query Plan", "⌥⌘E"),
    ("action:explain_analyze", "Explain Analyze", "⇧⌥⌘E"),
    ("action:review", "Review Pending Changes", "⌘S"),
    ("action:export", "Export Results…", "⇧⌘E"),
    ("action:import", "Import Data (CSV, JSON)…", ""),
    ("action:dump", "Dump Database to SQL…", ""),
    ("action:add_conn", "New Connection…", "⇧⌘N"),
    ("action:demo", "Open Demo Database", ""),
    ("action:toggle_tx", "Toggle Manual Commit Mode", ""),
    ("action:commit_tx", "Commit Transaction", ""),
    ("action:rollback_tx", "Roll Back Transaction", ""),
    ("action:toggle_sidebar", "Toggle Sidebar", "⌘B"),
    ("action:move_sidebar_left", "Move Sidebar to Left", ""),
    ("action:move_sidebar_right", "Move Sidebar to Right", ""),
    ("action:toggle_bottom", "Toggle Bottom Panel", "⌘J"),
    (
        "action:move_bottom_bottom",
        "Move Bottom Panel to Bottom",
        "",
    ),
    ("action:move_bottom_right", "Move Bottom Panel to Right", ""),
    ("action:reset_layout", "Reset Layout", ""),
    ("action:theme", "Toggle Light/Dark Theme", ""),
    ("action:increase_font", "Zoom In (Increase Font Size)", "⌘+"),
    (
        "action:decrease_font",
        "Zoom Out (Decrease Font Size)",
        "⌘-",
    ),
    ("action:reset_font", "Reset Zoom (Actual Size)", "⌘0"),
    ("action:settings", "Settings…", "⌘,"),
    ("action:keymap", "Open Keymap", ""),
    ("action:report_issue", "Report Issue on GitHub…", ""),
    ("action:view_last_crash", "View Last Crash Report…", ""),
];

/// Commands shown when the box is empty
const EVERYDAY: &[&str] = &["action:new_query", "action:open_file", "action:add_conn"];

const SQL_VERBS: &[&str] = &[
    "select", "with", "insert", "update", "delete", "explain", "create", "alter", "drop", "show",
    "pragma", "values", "table",
];

/// Higher is better; `None` is no match. Characters must appear in order (fuzzy), rewarded for
/// Match quality: None = no match. Higher = better match. Rewards consecutive runs, word starts
/// and prefixes. For queries of 4+ characters one typo is forgiven.
#[allow(dead_code)]
pub fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    FuzzyQuery::new(query).score(target)
}

/// Pre-processed query for allocation-free scoring across many candidate targets.
pub struct FuzzyQuery {
    chars: Vec<char>,
}

impl FuzzyQuery {
    pub fn new(query: &str) -> Self {
        let chars: Vec<char> = query
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        Self { chars }
    }

    pub fn score(&self, target: &str) -> Option<i32> {
        let q = &self.chars;
        if q.is_empty() {
            return Some(0);
        }

        // Fast stack buffer for targets up to 128 chars (table names, column names, commands)
        const STACK_LIMIT: usize = 128;
        let mut stack_buf = ['\0'; STACK_LIMIT];
        let heap_buf;
        let t: &[char] = {
            let mut count = 0;
            let mut fits = true;
            for c in target.chars().flat_map(|c| c.to_lowercase()) {
                if count < STACK_LIMIT {
                    stack_buf[count] = c;
                    count += 1;
                } else {
                    fits = false;
                    break;
                }
            }
            if fits {
                &stack_buf[..count]
            } else {
                heap_buf = target.to_lowercase().chars().collect::<Vec<char>>();
                &heap_buf
            }
        };

        let (mut from, mut score, mut prev) = (0usize, 0i32, None::<usize>);
        let mut matched = true;
        for &qc in q {
            let Some(gap) = t
                .get(from..)
                .and_then(|rest| rest.iter().position(|&c| c == qc))
            else {
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
            return Some(score + if t.starts_with(q) { 50 } else { 0 });
        }
        // Typo tolerance: any window of similar length within one edit
        if q.len() >= 4 {
            for len in [q.len(), q.len() + 1, q.len() - 1] {
                if len <= t.len() && t.windows(len).any(|w| edit_distance(q, w) <= 1) {
                    return Some(1);
                }
            }
        }
        None
    }
}

/// Optimal string alignment distance (insert, delete, substitute, transpose)
/// Uses a flat stack buffer for typical queries to eliminate heap allocations.
fn edit_distance(a: &[char], b: &[char]) -> usize {
    const STACK_MAX: usize = 65;
    if a.len() < STACK_MAX && b.len() < STACK_MAX {
        let cols = b.len() + 1;
        let mut d = [0usize; STACK_MAX * STACK_MAX];
        for i in 0..=a.len() {
            d[i * cols] = i;
        }
        for (j, cell) in d[..cols].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                let mut cell = (d[(i - 1) * cols + j] + 1)
                    .min(d[i * cols + j - 1] + 1)
                    .min(d[(i - 1) * cols + j - 1] + cost);
                if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    cell = cell.min(d[(i - 2) * cols + j - 2] + 1);
                }
                d[i * cols + j] = cell;
            }
        }
        d[a.len() * cols + b.len()]
    } else {
        let cols = b.len() + 1;
        let mut d = vec![0usize; (a.len() + 1) * cols];
        for i in 0..=a.len() {
            d[i * cols] = i;
        }
        for (j, cell) in d[..cols].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                let mut cell = (d[(i - 1) * cols + j] + 1)
                    .min(d[i * cols + j - 1] + 1)
                    .min(d[(i - 1) * cols + j - 1] + cost);
                if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    cell = cell.min(d[(i - 2) * cols + j - 2] + 1);
                }
                d[i * cols + j] = cell;
            }
        }
        d[a.len() * cols + b.len()]
    }
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
        // Find Usages: only its results, narrowed by what's typed
        if let Some(u) = &state.usages {
            let fq_raw = FuzzyQuery::new(raw);
            return u
                .items
                .iter()
                .enumerate()
                .filter(|(_, x)| {
                    raw.is_empty()
                        || fq_raw.score(&x.label).is_some()
                        || fq_raw.score(&x.line_text).is_some()
                })
                .take(50)
                .map(|(i, x)| CommandItem {
                    id: format!("usage:{i}"),
                    title: x.line_text.clone(),
                    detail: format!("{} · {}", x.label, u.name),
                    kind: Kind::Usage,
                    hint: "↵",
                })
                .collect();
        }
        let fq = FuzzyQuery::new(query);
        let conn = state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let mut scored: Vec<(i32, CommandItem)> = Vec::new();
        let mut push = |score: Option<i32>, bias: i32, item: CommandItem| {
            if let Some(s) = score {
                scored.push((s + bias, item));
            }
        };

        // Typed SQL: running it is the top result
        let first_word = raw.split_whitespace().next().unwrap_or("").to_lowercase();
        if !only_commands
            && !only_tables
            && SQL_VERBS.contains(&first_word.as_str())
            && raw.contains(' ')
        {
            push(
                Some(10_000),
                0,
                CommandItem {
                    id: format!("run:{raw}"),
                    title: raw.to_string(),
                    detail: "Run this query".into(),
                    kind: Kind::Sql,
                    hint: "↵",
                },
            );
        }

        // Empty box: recents first (U6)
        if query.is_empty() && !only_commands {
            for (i, (schema, table)) in state.recent_tables.iter().enumerate() {
                push(
                    Some(900 - i as i32),
                    0,
                    CommandItem {
                        id: format!("table:{schema}\u{1f}{table}"),
                        title: table.clone(),
                        detail: format!("Recent · {schema}"),
                        kind: Kind::Table,
                        hint: "↵",
                    },
                );
            }
        }

        if !only_commands {
            for t in &state.catalog.tables {
                let is_recent = state
                    .recent_tables
                    .iter()
                    .any(|(s, n)| *s == t.schema_name && *n == t.table_name);
                if query.is_empty() && is_recent {
                    continue; // already listed under Recent
                }
                let table_score = fq.score(&t.table_name);
                push(
                    table_score,
                    if query.is_empty() { -1000 } else { 30 },
                    CommandItem {
                        id: format!("table:{}\u{1f}{}", t.schema_name, t.table_name),
                        title: t.table_name.clone(),
                        detail: format!("{} · {}", t.schema_name, conn),
                        kind: if t.is_view { Kind::View } else { Kind::Table },
                        hint: "↵",
                    },
                );
                // The table's other views, once its name is being typed
                if !query.is_empty() {
                    for (view, label) in [("structure", "Structure"), ("ddl", "DDL")] {
                        push(
                            table_score,
                            20,
                            CommandItem {
                                id: format!("table-{view}:{}\u{1f}{}", t.schema_name, t.table_name),
                                title: format!("{} · {label}", t.table_name),
                                detail: format!("{} · {}", t.schema_name, conn),
                                kind: if t.is_view { Kind::View } else { Kind::Table },
                                hint: "↵",
                            },
                        );
                    }
                }
            }
        }

        if !only_commands && !only_tables {
            for c in &state.saved_connections {
                let active = state
                    .active_connection
                    .as_ref()
                    .is_some_and(|a| a.id == c.id);
                push(
                    fq.score(&c.name),
                    if query.is_empty() { -2000 } else { 10 },
                    CommandItem {
                        id: format!("connection:{}", c.id),
                        title: c.name.clone(),
                        detail: if active {
                            "Connected".into()
                        } else {
                            format!("Switch connection · {}", c.env.label())
                        },
                        kind: Kind::Connection,
                        hint: "↵",
                    },
                );
            }
            // Pinned queries on this connection
            let pinned = state
                .active_connection
                .as_ref()
                .and_then(|c| state.pins.get(&c.id))
                .cloned()
                .unwrap_or_default();
            for (i, pin) in pinned.iter().enumerate() {
                if let crate::pins::Pin::Query { title, .. } = pin {
                    push(
                        if query.is_empty() {
                            Some(800 - i as i32)
                        } else {
                            fq.score(title)
                        },
                        15,
                        CommandItem {
                            id: format!("pin:{i}"),
                            title: title.clone(),
                            detail: "Pinned query".into(),
                            kind: Kind::Recent,
                            hint: "↵",
                        },
                    );
                }
            }
            // Recent queries, newest first. Empty query: the last few. Typed: the whole history is searched.
            let cap = if query.is_empty() {
                20
            } else {
                history::MAX_ENTRIES
            };
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for (i, h) in state
                .history
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, h)| h.error.is_none())
            {
                if seen.len() >= cap || !seen.insert(h.sql.as_str()) {
                    continue;
                }
                let one_line: String = h.sql.split_whitespace().collect::<Vec<_>>().join(" ");
                let score = if query.is_empty() {
                    (seen.len() <= 5).then_some(500 - seen.len() as i32)
                } else {
                    fq.score(&one_line)
                };
                push(
                    score,
                    -5,
                    CommandItem {
                        id: format!("history:{i}"),
                        title: one_line,
                        detail: format!("Recent query · {}", h.connection),
                        kind: Kind::Recent,
                        hint: "↵",
                    },
                );
            }
        }

        if !only_tables {
            for (id, title, hint) in COMMANDS {
                let score = if query.is_empty() && !only_commands {
                    EVERYDAY.contains(id).then_some(100)
                } else {
                    fq.score(title)
                };
                push(
                    score,
                    0,
                    CommandItem {
                        id: id.to_string(),
                        title: title.to_string(),
                        detail: String::new(),
                        kind: Kind::Command,
                        hint,
                    },
                );
            }
        }

        scored.sort_by_key(|(score, ..)| std::cmp::Reverse(*score));
        // A short list reads at a glance; typing narrows it rather than scrolling
        let limit = if query.is_empty() { 8 } else { 14 };
        let mut items: Vec<CommandItem> = scored
            .into_iter()
            .take(limit)
            .map(|(_, item)| item)
            .collect();
        // Group by section, sections in order of their best match (stable, so ranking holds inside each)
        let first_seen = |g: &str, items: &[CommandItem]| {
            items
                .iter()
                .position(|i| i.kind.group() == g)
                .unwrap_or(usize::MAX)
        };
        let order: Vec<(String, usize)> = items
            .iter()
            .map(|i| {
                (
                    i.kind.group().to_string(),
                    first_seen(i.kind.group(), &items),
                )
            })
            .collect();
        let mut keyed: Vec<(usize, CommandItem)> = order
            .into_iter()
            .map(|(_, k)| k)
            .zip(items.drain(..))
            .collect();
        keyed.sort_by_key(|(k, _)| *k);
        keyed.into_iter().map(|(_, i)| i).collect()
    }

    pub fn render(
        state: &AppState,
        theme: &Theme,
        cx: &Context<QuarryWindow>,
        input: &Entity<InputState>,
    ) -> impl IntoElement {
        if !state.command_bar_open {
            return div().into_any_element();
        }

        let typed = input.read(cx).value();
        let items = Self::get_candidates(state, &typed);
        let selected_idx = state
            .command_bar_selected_index
            .min(items.len().saturating_sub(1));

        let kbd = |k: &'static str| {
            div()
                .px_1p5()
                .h(px(18.0))
                .flex()
                .items_center()
                .rounded(px(4.0))
                .bg(theme.bg_surface)
                .text_xs()
                .text_color(theme.text_muted)
                .child(crate::ui::format_keys(k))
        };
        let mut list = v_flex()
            .id("command-bar-scroll")
            .overflow_y_scroll()
            .px_2()
            .py_2()
            .gap_px();
        if items.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_6()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("Nothing matches. Try > for commands or / for tables."),
            );
        }
        let mut last_group = "";
        for (idx, item) in items.iter().enumerate() {
            let group = item.kind.group();
            if group != last_group {
                last_group = group;
                list = list.child(
                    div()
                        .px_3()
                        .pt(px(if idx == 0 { 4.0 } else { 12.0 }))
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(group),
                );
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.handle_command_action(item_id.clone(), window, cx)
                    }))
                    .child(
                        Icon::new(item.kind.icon())
                            .small()
                            .text_color(if is_selected {
                                theme.accent
                            } else {
                                theme.text_muted
                            }),
                    )
                    .child(
                        div()
                            .max_w(px(380.0))
                            .text_sm()
                            .text_color(theme.text_primary)
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(item.detail.clone()),
                    )
                    .when(!item.hint.is_empty() && item.hint != "↵", |d| {
                        d.child(kbd(item.hint))
                    }),
            );
        }

        modal_backdrop(
            "command-bar-backdrop",
            cx.listener(move |this, _, window, cx| this.dismiss(&crate::Dismiss, window, cx)),
            cx,
        )
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
                        .child(
                            div()
                                .flex_1()
                                .text_base()
                                .child(Input::new(input).appearance(false)),
                        ),
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
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(kbd("↑↓"))
                                .child("navigate"),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(kbd("↵"))
                                .child("open"),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(kbd("esc"))
                                .child("close"),
                        )
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
