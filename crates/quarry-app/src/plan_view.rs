// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! EXPLAIN as a tree: one row per step, a bar for its share of the time (or cost), and
//! estimated vs actual rows flagged when they're far apart

use crate::theme::Theme;
use gpui::*;
use quarry_core::plan::{Plan, PlanNode};

/// Share of the whole that this step alone accounts for: time with ANALYZE, otherwise cost
fn share(n: &PlanNode, root: &PlanNode) -> f32 {
    match (n.self_ms(), root.ms) {
        (Some(own), Some(total)) if total > 0.0 => (own / total) as f32,
        _ => match (n.cost, root.cost) {
            (Some(c), Some(total)) if total > 0.0 => {
                let children: f64 = n.children.iter().filter_map(|c| c.cost).sum();
                ((c - children).max(0.0) / total) as f32
            }
            _ => 0.0,
        },
    }
}

fn fmt_rows(r: f64) -> String {
    match r {
        r if r >= 1e6 => format!("{:.1}M", r / 1e6),
        r if r >= 1e4 => format!("{:.0}k", r / 1e3),
        r => format!("{r:.0}"),
    }
}

/// One-line summary for the results header
pub fn summary(plan: &Plan) -> String {
    match (plan.execution_ms, plan.planning_ms) {
        (Some(e), Some(p)) => format!("Plan · ran in {e:.1} ms · planned in {p:.1} ms"),
        _ => match plan.root.cost {
            Some(c) => format!("Plan · estimated cost {c:.0}"),
            None => "Plan".into(),
        },
    }
}

pub fn render(plan: &Plan, t: &Theme) -> AnyElement {
    let root = &plan.root;
    let rows = root
        .flatten()
        .into_iter()
        .enumerate()
        .map(|(i, (depth, n))| {
            let share = share(n, root);
            let hot = share >= 0.3;
            let rows = match (n.est_rows, n.rows) {
                (Some(e), Some(a)) => Some(format!("{} → {}", fmt_rows(e), fmt_rows(a))),
                (Some(e), None) => Some(fmt_rows(e)),
                _ => None,
            };
            let metric = n
                .self_ms()
                .map(|ms| format!("{ms:.2} ms"))
                .or_else(|| n.cost.map(|c| format!("cost {c:.0}")));
            div()
                .id(("plan-row", i))
                .flex()
                .items_center()
                .gap_3()
                .h(px(crate::ui::ROW_H))
                .px_3()
                .border_b_1()
                .border_color(t.border)
                .hover(|s| s.bg(t.bg_hover))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_1()
                        .min_w_0()
                        .pl(px(depth as f32 * 18.0))
                        .child(div().text_color(t.text_muted).child(if depth == 0 {
                            ""
                        } else {
                            "└"
                        }))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .child(n.op.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(t.text_muted)
                                .child(n.detail.clone()),
                        ),
                )
                .children(rows.map(|r| {
                    div()
                        .w(px(120.0))
                        .text_sm()
                        .text_right()
                        .text_color(if n.misestimated() {
                            t.warning
                        } else {
                            t.text_secondary
                        })
                        .child(r)
                }))
                .child(
                    div()
                        .w(px(80.0))
                        .text_sm()
                        .text_right()
                        .text_color(t.text_secondary)
                        .children(metric),
                )
                .child(
                    div()
                        .w(px(90.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(t.border)
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(px(90.0 * share.clamp(0.0, 1.0)))
                                .bg(if hot { t.danger } else { t.accent }),
                        ),
                )
        });
    let legend = div()
        .flex()
        .gap_4()
        .px_3()
        .py_1p5()
        .text_sm()
        .text_color(t.text_muted)
        .border_b_1()
        .border_color(t.border)
        .child("Rows: estimated → actual (amber when 10× off)")
        .child(if root.ms.is_some() {
            "Bar: share of the run's time spent in that step"
        } else {
            "Bar: share of the estimated cost. Explain Analyze for real timings"
        });
    div()
        .id("plan-view")
        .size_full()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .child(legend)
        .children(rows)
        .into_any_element()
}
