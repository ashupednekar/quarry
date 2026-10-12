// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::ButtonVariants;
use gpui::component::{ActiveTheme, Sizable};
use gpui::*;
use quarry_core::edits::{PendingChanges, RowKey};

pub struct ReviewModal;

/// One table row: a column with its value before and after (`None` = didn't exist), or a note
enum Line {
    Change(String, Option<String>, String),
    Note(String),
}

/// A row's changes: kind ("UPDATE"…), its key ("id = 8"), and the lines
struct RowDiff {
    kind: &'static str,
    key: String,
    lines: Vec<Line>,
}

fn key_label(key: &RowKey) -> String {
    key.iter()
        .map(|(c, v)| format!("{c} = {}", v.to_display_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Pending changes grouped by row, in the order they were made
fn row_diffs(p: &PendingChanges) -> Vec<RowDiff> {
    let mut rows: Vec<RowDiff> = Vec::new();
    for e in &p.cell_edits {
        let key = key_label(&e.pks);
        let ix = match rows.iter().position(|r| r.kind == "UPDATE" && r.key == key) {
            Some(ix) => ix,
            None => {
                rows.push(RowDiff {
                    kind: "UPDATE",
                    key,
                    lines: Vec::new(),
                });
                rows.len() - 1
            }
        };
        let lines = &mut rows[ix].lines;
        lines.push(Line::Change(
            e.column_name.clone(),
            Some(e.old_value.to_display_string()),
            e.new_value.to_display_string(),
        ));
    }
    for (i, ins) in p.row_inserts.iter().enumerate() {
        let mut cols: Vec<_> = ins.values.iter().collect();
        cols.sort_by(|a, b| a.0.cmp(b.0));
        let mut lines: Vec<Line> = cols
            .into_iter()
            .map(|(c, v)| Line::Change(c.clone(), None, v.to_display_string()))
            .collect();
        if lines.is_empty() {
            lines.push(Line::Note("every column takes its default".into()));
        }
        rows.push(RowDiff {
            kind: "INSERT",
            key: format!("new row {}", i + 1),
            lines,
        });
    }
    for d in &p.row_deletes {
        rows.push(RowDiff {
            kind: "DELETE",
            key: key_label(&d.pks),
            lines: vec![Line::Note("the whole row is removed".into())],
        });
    }
    rows
}

/// "2 rows updated · 1 inserted · 1 deleted"
fn summary(rows: &[RowDiff]) -> String {
    let n = |k: &str| rows.iter().filter(|r| r.kind == k).count();
    let mut parts = Vec::new();
    for (k, verb) in [
        ("UPDATE", "updated"),
        ("INSERT", "inserted"),
        ("DELETE", "deleted"),
    ] {
        let c = n(k);
        if c > 0 {
            parts.push(if parts.is_empty() {
                format!("{c} row{} {verb}", if c == 1 { "" } else { "s" })
            } else {
                format!("{c} {verb}")
            });
        }
    }
    parts.join(" · ")
}

impl ReviewModal {
    pub fn render(state: &AppState, theme: &Theme, cx: &Context<QuarryWindow>) -> impl IntoElement {
        if !state.review_modal_open {
            return div().into_any_element();
        }
        let t = theme;
        let sql_statements = &state.review_modal_sql;
        let table_name = &state.review_modal_table;
        let rows = row_diffs(&state.review_modal_changes);
        let show_sql = state.review_modal_show_sql;
        let mono = cx.theme().mono_font_family.clone();

        let tab = |id: &'static str, label: String, on: bool, sql: bool| {
            div()
                .id(id)
                .px_2p5()
                .py_1()
                .rounded(px(5.0))
                .text_xs()
                .cursor_pointer()
                .when(on, |d| d.bg(t.bg_selected).text_color(t.text_primary))
                .when(!on, |d| {
                    d.text_color(t.text_muted).hover(|s| s.bg(t.bg_hover))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.review_modal_show_sql = sql;
                    cx.notify();
                }))
                .child(label)
        };
        let tabs = h_flex()
            .items_center()
            .gap_1()
            .px_6()
            .pt_2()
            .child(tab(
                "review-tab-changes",
                "Changes".into(),
                !show_sql,
                false,
            ))
            .child(tab(
                "review-tab-sql",
                format!("SQL ({})", sql_statements.len()),
                show_sql,
                true,
            ));

        // Column | Before | After: the eye reads down one column at a time. Before is struck
        // through and muted, after is full contrast; the headers say which is which, not just color.
        const COL_W: f32 = 150.0;
        let cell = || div().flex_1().min_w_0().px_2().py_1();
        let table_head = || {
            h_flex()
                .px_1()
                .border_b_1()
                .border_color(t.border)
                .text_color(t.text_muted)
                .child(cell().flex_none().w(px(COL_W)).child("Column"))
                .child(cell().child("Before"))
                .child(cell().child("After"))
        };
        let table_row = |col: String, before: Option<String>, after: String| {
            h_flex()
                .items_start()
                .px_1()
                .border_b_1()
                .border_color(t.border)
                .child(
                    cell()
                        .flex_none()
                        .w(px(COL_W))
                        .truncate()
                        .text_color(t.text_secondary)
                        .child(col),
                )
                .child(match before {
                    Some(v) => cell().text_color(t.text_muted).line_through().child(v),
                    None => cell().text_color(t.text_muted).child("—"),
                })
                .child(
                    cell()
                        .bg(crate::Theme::tint(t.success, 0.10))
                        .text_color(t.text_primary)
                        .child(after),
                )
        };
        let badge = |kind: &'static str| {
            let color = match kind {
                "INSERT" => t.success,
                "DELETE" => t.danger,
                _ => t.warning,
            };
            div()
                .px_1p5()
                .rounded(px(3.0))
                .bg(crate::Theme::tint(color, 0.15))
                .text_color(color)
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(kind)
        };
        let changes = rows.iter().map(|r| {
            v_flex()
                .rounded(px(6.0))
                .border_1()
                .border_color(t.border)
                .overflow_hidden()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1p5()
                        .bg(t.bg_panel)
                        .border_b_1()
                        .border_color(t.border)
                        .child(badge(r.kind))
                        .child(
                            div()
                                .text_xs()
                                .font_family(mono.clone())
                                .text_color(t.text_secondary)
                                .truncate()
                                .child(r.key.clone()),
                        ),
                )
                .child(
                    v_flex()
                        .text_xs()
                        .font_family(mono.clone())
                        .when(r.lines.iter().any(|l| matches!(l, Line::Change(..))), |d| {
                            d.child(table_head())
                        })
                        .children(r.lines.iter().map(|l| {
                            match l {
                                Line::Change(c, before, after) => {
                                    table_row(c.clone(), before.clone(), after.clone())
                                }
                                Line::Note(n) => h_flex()
                                    .px_3()
                                    .py_1p5()
                                    .text_color(t.text_muted)
                                    .child(n.clone()),
                            }
                        })),
                )
        });
        let sql = sql_statements.iter().enumerate().map(|(idx, sql)| {
            h_flex()
                .p_3()
                .gap_3()
                .items_start()
                .rounded(px(6.0))
                .bg(t.bg_panel)
                .border_1()
                .border_color(t.border)
                .child(
                    div()
                        .w(px(20.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(format!("{}", idx + 1)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .font_family(mono.clone())
                        .text_color(t.text_primary)
                        .child(sql.clone()),
                )
        });

        let close = cx.listener(move |this, _, _, cx| this.close_review_modal(cx));
        modal_backdrop(
            "review-modal-backdrop",
            cx.listener(move |this, _, _, cx| this.close_review_modal(cx)),
            cx,
        )
        .child(
            modal_card("review-modal-card", 760.0, theme)
                .max_h(px(620.0))
                .overflow_hidden()
                .child(modal_header(
                    theme,
                    "Review changes",
                    Some(
                        format!(
                            "{} on {table_name} · applied in one transaction",
                            summary(&rows)
                        )
                        .into(),
                    ),
                    close,
                ))
                .child(tabs)
                .child(
                    modal_body("review-sql-scroll")
                        .flex_1()
                        .gap_2()
                        .when(!show_sql, |d| d.children(changes))
                        .when(show_sql, |d| d.children(sql)),
                )
                .child(
                    modal_footer(theme)
                        .child(
                            zbutton("review-cancel-btn")
                                .pad_sm()
                                .label("Cancel")
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.close_review_modal(cx)),
                                ),
                        )
                        .child(
                            zbutton("review-commit-btn")
                                .pad_sm()
                                .label("Submit ⌘↵")
                                .primary()
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.commit_pending_changes(window, cx)
                                })),
                        ),
                ),
        )
        .into_any_element()
    }
}

impl QuarryWindow {
    pub(crate) fn close_review_modal(&mut self, cx: &mut Context<Self>) {
        self.state.review_modal_open = false;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{row_diffs, summary};
    use quarry_core::edits::{PendingCellEdit, PendingChanges, PendingRowDelete};
    use quarry_core::DbValue;

    #[test]
    fn groups_edits_by_row() {
        let key = |id| vec![("id".to_string(), DbValue::Int(id))];
        let edit = |id, col: &str| PendingCellEdit {
            pks: key(id),
            column_name: col.into(),
            old_value: DbValue::Text("a".into()),
            new_value: DbValue::Text("b".into()),
        };
        let p = PendingChanges {
            cell_edits: vec![edit(1, "name"), edit(2, "name"), edit(1, "email")],
            row_inserts: vec![],
            row_deletes: vec![PendingRowDelete { pks: key(3) }],
        };
        let rows = row_diffs(&p);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].key.as_str(), rows[0].lines.len()), ("id = 1", 2));
        assert_eq!(summary(&rows), "2 rows updated · 1 deleted");
    }
}
