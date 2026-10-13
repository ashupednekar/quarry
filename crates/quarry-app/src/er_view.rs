// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! A table and the tables around it, N foreign-key hops out: what it references to the left, what
//! references it to the right. Every column is shown and each key is drawn column to column.
//! Read from the catalog's foreign keys; click a table to go there, ⨝ to write the join.

use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::tooltip::Tooltip;
use gpui::component::{Icon, Sizable};
use gpui::*;
use quarry_core::schema::{ForeignKeyRef, SchemaCatalog, TableSchema};
use std::collections::{HashMap, VecDeque};

const CARD_W: f32 = 250.0;
const HEADER_H: f32 = 34.0;
const ROW_H: f32 = 22.0;
const GAP_X: f32 = 110.0;
const GAP_Y: f32 = 28.0;
const PAD: f32 = 24.0;
/// Card border (1px) + the column list's top padding (4px), above the first row
const ROWS_TOP: f32 = 5.0;
/// Room above the cards for each column's caption
const CAPTION_H: f32 = 30.0;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 2.5;

/// A drawn key: (from, leaves to the right?, to, enters from the right?, highlighted?)
type Wire = (Point<f32>, bool, Point<f32>, bool, bool);

/// One foreign key: (card, column) → (card, column), indices into `Diagram::cards`
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: (usize, usize),
    pub to: (usize, usize),
}

/// Retained layout cache for the ER diagram canvas to avoid per-frame graph traversals
#[derive(Clone, Debug, Default)]
pub struct DiagramLayoutCache {
    pub key: (String, String, usize, usize),
    pub card_keys: Vec<(String, String, i32)>,
    pub edges: Vec<Edge>,
    pub pos: Vec<Point<f32>>,
    pub min_level: i32,
    pub width: f32,
    pub height: f32,
}

pub struct Diagram<'a> {
    /// Each table with its level: 0 the centre, negative what it references, positive what references it
    pub cards: Vec<(&'a TableSchema, i32)>,
    pub edges: Vec<Edge>,
}

fn to_schema<'a>(owner: &'a TableSchema, fk: &'a ForeignKeyRef) -> &'a str {
    if fk.to_schema.is_empty() {
        &owner.schema_name
    } else {
        &fk.to_schema
    }
}

/// Tables up to `depth` hops from `center` (`None` = everything connected), placed breadth-first
pub fn diagram<'a>(
    catalog: &'a SchemaCatalog,
    center: &'a TableSchema,
    depth: Option<usize>,
) -> Diagram<'a> {
    let key = |t: &TableSchema| (t.schema_name.clone(), t.table_name.clone());
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    let mut cards = vec![(center, 0)];
    index.insert(key(center), 0);
    let mut queue = VecDeque::from([(0usize, 0usize)]);
    while let Some((ix, hops)) = queue.pop_front() {
        if depth.is_some_and(|d| hops >= d) {
            continue;
        }
        let (tbl, level) = cards[ix];
        let outgoing = tbl.foreign_keys.iter().filter_map(|fk| {
            catalog
                .find_table(Some(to_schema(tbl, fk)), &fk.to_table)
                .map(|t| (t, level - 1))
        });
        let incoming = catalog
            .tables
            .iter()
            .filter(|o| {
                o.foreign_keys
                    .iter()
                    .any(|fk| fk.to_table == tbl.table_name && to_schema(o, fk) == tbl.schema_name)
            })
            .map(|o| (o, level + 1));
        for (t, lvl) in outgoing.chain(incoming).collect::<Vec<_>>() {
            if let std::collections::hash_map::Entry::Vacant(e) = index.entry(key(t)) {
                e.insert(cards.len());
                queue.push_back((cards.len(), hops + 1));
                cards.push((t, lvl));
            }
        }
    }
    let col_ix = |t: &TableSchema, name: &str| t.columns.iter().position(|c| c.name == name);
    let mut edges = Vec::new();
    for (from, (tbl, _)) in cards.iter().enumerate() {
        for fk in &tbl.foreign_keys {
            let Some(&to) = index.get(&(to_schema(tbl, fk).to_string(), fk.to_table.clone()))
            else {
                continue;
            };
            if let (Some(fc), Some(tc)) = (
                col_ix(tbl, &fk.from_column),
                col_ix(cards[to].0, &fk.to_column),
            ) {
                edges.push(Edge {
                    from: (from, fc),
                    to: (to, tc),
                });
            }
        }
    }
    Diagram { cards, edges }
}

/// `SELECT … JOIN …` over every key between two tables, in `dialect`
fn join_sql(a: &TableSchema, b: &TableSchema, dialect: &dyn quarry_core::Dialect) -> String {
    let alias = |t: &TableSchema, other: &str| {
        let first: String = t
            .table_name
            .chars()
            .take(1)
            .collect::<String>()
            .to_lowercase();
        if t.table_name == other {
            format!("{first}2")
        } else {
            first
        }
    };
    let (aa, mut ba) = (alias(a, ""), alias(b, &a.table_name));
    if ba == aa {
        ba = format!("{ba}2");
    }
    let q = |c: &str| dialect.quote_ident(c);
    let mut on: Vec<String> = Vec::new();
    for fk in &a.foreign_keys {
        if fk.to_table == b.table_name && to_schema(a, fk) == b.schema_name {
            on.push(format!(
                "{ba}.{} = {aa}.{}",
                q(&fk.to_column),
                q(&fk.from_column)
            ));
        }
    }
    for fk in &b.foreign_keys {
        if fk.to_table == a.table_name && to_schema(b, fk) == a.schema_name {
            on.push(format!(
                "{ba}.{} = {aa}.{}",
                q(&fk.from_column),
                q(&fk.to_column)
            ));
        }
    }
    format!(
        "SELECT *\nFROM {} {aa}\nJOIN {} {ba} ON {}",
        dialect.qualified(&a.schema_name, &a.table_name),
        dialect.qualified(&b.schema_name, &b.table_name),
        if on.is_empty() {
            "TRUE".into()
        } else {
            on.join("\n    AND ")
        }
    )
}

impl QuarryWindow {
    pub fn render_diagram(&self, schema: &str, table: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let Some(center) = self.state.catalog.find_table(Some(schema), table) else {
            return div()
                .p_4()
                .text_xs()
                .text_color(t.text_muted)
                .child("Not in the schema; refresh it from the connection's menu")
                .into_any_element();
        };
        let depth = (self.diagram_depth > 0).then_some(self.diagram_depth);
        let depth_val = self.diagram_depth;
        let cat_len = self.state.catalog.tables.len();
        let cache_key = (schema.to_string(), table.to_string(), depth_val, cat_len);

        let mut cache = self.diagram_layout.borrow_mut();
        let is_hit = cache.as_ref().is_some_and(|c| c.key == cache_key);
        if !is_hit {
            let Diagram { cards, edges } = diagram(&self.state.catalog, center, depth);
            let min_level = cards.iter().map(|c| c.1).min().unwrap_or(0);
            let mut next_y: HashMap<i32, f32> = HashMap::new();
            let pos: Vec<Point<f32>> = cards
                .iter()
                .map(|(tbl, level)| {
                    let y = next_y.entry(*level).or_insert(PAD + CAPTION_H);
                    let p = point(PAD + (level - min_level) as f32 * (CARD_W + GAP_X), *y);
                    *y += HEADER_H + tbl.columns.len() as f32 * ROW_H + 8.0 + GAP_Y;
                    p
                })
                .collect();
            let width = pos.iter().map(|p| p.x + CARD_W + PAD).fold(0.0, f32::max);
            let height = next_y.values().copied().fold(0.0, f32::max);
            let card_keys = cards
                .iter()
                .map(|(t, l)| (t.schema_name.clone(), t.table_name.clone(), *l))
                .collect();
            *cache = Some(DiagramLayoutCache {
                key: cache_key,
                card_keys,
                edges,
                pos,
                min_level,
                width,
                height,
            });
        }
        let cached = cache.as_ref().unwrap();
        let card_keys = cached.card_keys.clone();
        let edges = cached.edges.clone();
        let pos = cached.pos.clone();
        let min_level = cached.min_level;
        let width = cached.width;
        let height = cached.height;
        drop(cache);

        let cards: Vec<(&TableSchema, i32)> = card_keys
            .iter()
            .filter_map(|(s, n, l)| self.state.catalog.find_table(Some(s), n).map(|t| (t, *l)))
            .collect();
        // Layout is in unzoomed units; `z` scales fixed sizes, and the rem size scales text and spacing
        let z = self.diagram_zoom;
        let sz = move |v: f32| px(v * z);
        let interactive = z >= 0.5;

        // What the hovered column joins to lights up
        let hover = self.diagram_hover.as_ref();
        let is_hovered = |(card, col): (usize, usize)| {
            hover.is_some_and(|(s, n, c)| {
                let tbl = cards[card].0;
                tbl.schema_name == *s && tbl.table_name == *n && tbl.columns[col].name == *c
            })
        };
        let anchor = |(card, col): (usize, usize), right: bool| {
            let p = pos[card];
            point(
                p.x + if right { CARD_W } else { 0.0 },
                p.y + HEADER_H + ROWS_TOP + col as f32 * ROW_H + ROW_H / 2.0,
            )
        };
        let lines: Vec<Wire> = edges
            .iter()
            .map(|e| {
                let (fx, tx) = (pos[e.from.0].x, pos[e.to.0].x);
                // Leave from the side facing the target; same column → loop out the right
                let (from_right, to_right) = if fx > tx {
                    (false, true)
                } else if fx < tx {
                    (true, false)
                } else {
                    (true, true)
                };
                (
                    anchor(e.from, from_right),
                    from_right,
                    anchor(e.to, to_right),
                    to_right,
                    is_hovered(e.from) || is_hovered(e.to),
                )
            })
            .collect();
        let (line_color, hot_color) = (t.text_muted, t.accent);
        let wires = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let o = bounds.origin;
                let pt = |p: Point<f32>| point(o.x + px(p.x * z), o.y + px(p.y * z));
                // Hot lines last, so they draw over the rest
                for hot in [false, true] {
                    for (a, ar, b, br, h) in lines.iter().filter(|l| l.4 == hot) {
                        let bend = |p: &Point<f32>, right: bool| {
                            point(p.x + if right { 50.0 } else { -50.0 }, p.y)
                        };
                        let mut path =
                            PathBuilder::stroke(px(if *h { 2.0 } else { 1.0 } * z.max(0.75)));
                        path.move_to(pt(*a));
                        path.cubic_bezier_to(pt(*b), pt(bend(a, *ar)), pt(bend(b, *br)));
                        if let Ok(p) = path.build() {
                            let c: Hsla = if *h { hot_color } else { line_color }.into();
                            window.paint_path(p, if *h { c } else { c.opacity(0.6) });
                        }
                    }
                }
            },
        )
        .absolute()
        .size_full();

        let fk_of = |tbl: &TableSchema, col: &str| {
            tbl.foreign_keys
                .iter()
                .find(|f| f.from_column == col)
                .cloned()
        };
        let card = |ix: usize, tbl: &TableSchema, p: Point<f32>, cx: &mut Context<Self>| {
            let focus = ix == 0;
            let (s, n) = (tbl.schema_name.clone(), tbl.table_name.clone());
            // Direct neighbours of the centre get a join button
            let joins_center = !focus
                && edges
                    .iter()
                    .any(|e| (e.from.0 == 0 && e.to.0 == ix) || (e.from.0 == ix && e.to.0 == 0));
            let header = h_flex()
                .items_center()
                .id(("er-head", ix))
                .h(sz(HEADER_H))
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(t.border)
                .bg(if focus {
                    crate::Theme::tint(t.accent, 0.12)
                } else {
                    t.bg_panel.into()
                })
                .child(
                    div()
                        .size(sz(16.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(if focus { t.accent } else { t.text_muted })
                        .child(
                            Icon::new(if tbl.is_view {
                                IconName::Eye
                            } else {
                                IconName::Table
                            })
                            .small(),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .when(tbl.schema_name != center.schema_name, |d| {
                            d.child(format!("{}.", tbl.schema_name))
                        })
                        .child(tbl.table_name.clone()),
                )
                .when(joins_center, |d| {
                    let (a, b) = (center.clone(), tbl.clone());
                    d.child(
                        div()
                            .id(("er-join", ix))
                            .size(sz(22.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .text_color(t.text_secondary)
                            .hover(|s| s.bg(t.bg_hover).text_color(t.accent))
                            .tooltip(move |window, cx| {
                                Tooltip::new("Open a JOIN with the centre table").build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                let sql = join_sql(&a, &b, this.state.dialect());
                                this.open_query_tab(&sql, window, cx);
                            }))
                            .child(Icon::new(IconName::Merge).small()),
                    )
                })
                .when(!focus, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_pin(
                                crate::pins::Pin::Table {
                                    schema: s.clone(),
                                    table: n.clone(),
                                },
                                window,
                                cx,
                            )
                        }))
                });
            let rows = tbl.columns.iter().enumerate().map(|(ci, c)| {
                let fk = fk_of(tbl, &c.name);
                let hot = is_hovered((ix, ci))
                    || edges.iter().any(|e| {
                        (e.from == (ix, ci) && is_hovered(e.to))
                            || (e.to == (ix, ci) && is_hovered(e.from))
                    });
                // How this column joins: "→ customers.id (many to one)", or who points at it
                let mut joins: Vec<String> = edges
                    .iter()
                    .filter_map(|e| {
                        let other = if e.from == (ix, ci) {
                            e.to
                        } else if e.to == (ix, ci) {
                            e.from
                        } else {
                            return None;
                        };
                        let o = cards[other.0].0;
                        let arrow = if e.from == (ix, ci) { "→" } else { "←" };
                        Some(format!(
                            "{arrow} {}.{}",
                            o.table_name, o.columns[other.1].name
                        ))
                    })
                    .collect();
                if let Some(f) = &fk {
                    let kind = if c.is_primary_key {
                        "one to one"
                    } else {
                        "many to one"
                    };
                    if joins.is_empty() {
                        joins.push(format!("→ {}.{}", f.to_table, f.to_column));
                    }
                    joins[0] = format!("{} ({kind})", joins[0]);
                }
                let tip = (!joins.is_empty()).then(|| joins.join("\n"));
                let key = (
                    tbl.schema_name.clone(),
                    tbl.table_name.clone(),
                    c.name.clone(),
                );
                h_flex()
                    .items_center()
                    .id(("er-col", ix * 10_000 + ci))
                    .h(sz(ROW_H))
                    .px_3()
                    .gap_2()
                    .text_xs()
                    .when(hot, |d| d.bg(crate::Theme::tint(t.accent, 0.15)))
                    // Zoomed far out the rows are too small to point at: skip their handlers
                    .when(interactive, |d| {
                        d.on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            let next = on.then(|| key.clone());
                            if this.diagram_hover != next
                                && (*on || this.diagram_hover.as_ref() == Some(&key))
                            {
                                this.diagram_hover = next;
                                cx.notify();
                            }
                        }))
                        .when_some(tip, |d, tip| {
                            d.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        })
                    })
                    .child({
                        let (label, color) = match (c.is_primary_key, fk.is_some()) {
                            (true, _) => (Some("PK"), t.warning),
                            (false, true) => (Some("FK"), t.accent),
                            _ => (None, t.text_muted),
                        };
                        div()
                            .w(sz(24.0))
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .children(label.map(|l| {
                                div()
                                    .px_1()
                                    .rounded(px(3.0))
                                    .bg(crate::Theme::tint(color, 0.15))
                                    .text_color(color)
                                    .text_size(sz(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(l)
                            }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_primary)
                            .when(c.is_primary_key, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(c.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(sz(90.0))
                            .truncate()
                            .text_color(t.text_muted)
                            .child(c.raw_type_name.to_lowercase()),
                    )
            });
            v_flex()
                .absolute()
                .left(sz(p.x))
                .top(sz(p.y))
                .w(sz(CARD_W))
                .rounded(px(6.0))
                .border_1()
                .border_color(if focus { t.accent } else { t.border })
                .bg(t.bg_sidebar)
                .overflow_hidden()
                .shadow_sm()
                .child(header)
                .child(v_flex().py_1().children(rows.collect::<Vec<_>>()))
        };
        // Only cards on screen are built: layout and paint cost scale with what's drawn
        let view = self.diagram_view.get();
        let pan = self.diagram_pan.unwrap_or_else(|| {
            // This table in the middle across, its top near the top
            let centred = point(
                view.size.width / 2.0 - px((pos[0].x + CARD_W / 2.0) * z),
                px(0.),
            );
            self.diagram_auto_pan.set(centred);
            centred
        });
        let visible = |p: Point<f32>, tbl: &TableSchema| {
            if view.size.width <= px(0.) {
                return true; // not measured yet
            }
            let (vw, vh) = (f32::from(view.size.width), f32::from(view.size.height));
            let (ox, oy) = (f32::from(pan.x), f32::from(pan.y));
            let h = HEADER_H + tbl.columns.len() as f32 * ROW_H + 8.0;
            let (x0, y0) = (ox + p.x * z, oy + p.y * z);
            x0 < vw && y0 < vh && x0 + CARD_W * z > 0.0 && y0 + h * z > 0.0
        };
        let card_els: Vec<_> = cards
            .iter()
            .enumerate()
            .filter(|(ix, (tbl, _))| visible(pos[*ix], tbl))
            .map(|(ix, (tbl, _))| card(ix, tbl, pos[ix], cx))
            .collect();

        // Each column says what it holds, e.g. "REFERENCES · 2 HOPS · 3 TABLES"
        let mut levels: Vec<i32> = cards.iter().map(|c| c.1).collect();
        levels.sort();
        levels.dedup();
        let captions: Vec<_> = levels
            .iter()
            .map(|&level| {
                let n = cards.iter().filter(|c| c.1 == level).count();
                let (title, color) = match level {
                    0 => ("THIS TABLE".to_string(), t.accent),
                    l if l < 0 => ("← REFERENCES".to_string(), t.text_secondary),
                    _ => ("REFERENCED BY →".to_string(), t.text_secondary),
                };
                let hops = level.unsigned_abs();
                let detail = match (hops, n) {
                    (0, _) => String::new(),
                    (1, n) => format!("{n} table{}", if n == 1 { "" } else { "s" }),
                    (h, n) => format!("{h} hops · {n} table{}", if n == 1 { "" } else { "s" }),
                };
                v_flex()
                    .absolute()
                    .left(sz(PAD + (level - min_level) as f32 * (CARD_W + GAP_X)))
                    .top(sz(PAD - 6.0))
                    .w(sz(CARD_W))
                    .text_xs()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(title),
                    )
                    .when(!detail.is_empty(), |d| {
                        d.child(div().text_color(t.text_muted).child(detail))
                    })
            })
            .collect();

        let depth_picker = h_flex()
            .items_center()
            .p_0p5()
            .gap_0p5()
            .rounded(px(6.0))
            .border_1()
            .border_color(t.border)
            .bg(t.bg_panel)
            .text_xs()
            .child(
                div()
                    .px_1p5()
                    .text_color(t.text_muted)
                    .child("Hops")
                    .id("er-hops-label")
                    .tooltip(|window, cx| {
                        Tooltip::new("How many foreign keys away from this table to show")
                            .build(window, cx)
                    }),
            )
            .children(
                [(1, "1"), (2, "2"), (3, "3"), (0, "All")].map(|(d, label)| {
                    let on = self.diagram_depth == d;
                    div()
                        .id(("er-depth", d))
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .when(on, |s| s.bg(t.bg_selected).text_color(t.text_primary))
                        .when(!on, |s| {
                            s.text_color(t.text_muted).hover(|s| s.bg(t.bg_hover))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.diagram_depth = d;
                            cx.notify();
                        }))
                        .child(label)
                }),
            );
        let badge = |label: &'static str, color: Rgba| {
            div()
                .px_1()
                .rounded(px(3.0))
                .bg(crate::Theme::tint(color, 0.15))
                .text_color(color)
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        let legend = h_flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .gap_3()
            .text_xs()
            .text_color(t.text_muted)
            .child(div().text_color(t.text_secondary).child(format!(
                "{} tables · {} foreign keys",
                cards.len(),
                edges.len()
            )))
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(badge("PK", t.warning))
                    .child("primary key"),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(badge("FK", t.accent))
                    .child("foreign key"),
            )
            .child(div().truncate().child(
                "Hover a column to trace its joins · drag to move · pinch or ⌘ scroll to zoom",
            ));
        let content = size(width, height);
        let zoom_button = |id: &'static str, label: &'static str, tip: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(px(4.0))
                .cursor_pointer()
                .text_color(t.text_secondary)
                .hover(|s| s.bg(t.bg_hover).text_color(t.text_primary))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(label)
        };
        let zoom_controls = h_flex()
            .items_center()
            .p_0p5()
            .gap_0p5()
            .rounded(px(6.0))
            .border_1()
            .border_color(t.border)
            .bg(t.bg_panel)
            .text_xs()
            .child(
                zoom_button("er-zoom-out", "−", "Zoom out").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.diagram_zoom_at(None, 1.0 / 1.2);
                        cx.notify();
                    },
                )),
            )
            .child(
                zoom_button("er-zoom-reset", "", "Actual size")
                    .min_w(px(44.0))
                    .flex()
                    .justify_center()
                    .child(format!("{:.0}%", z * 100.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.diagram_zoom = 1.0;
                        this.diagram_pan = None;
                        cx.notify();
                    })),
            )
            .child(
                zoom_button("er-zoom-in", "+", "Zoom in").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.diagram_zoom_at(None, 1.2);
                        cx.notify();
                    },
                )),
            )
            .child(
                zoom_button("er-zoom-fit", "Fit", "Fit the whole diagram").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.diagram_fit(content);
                        cx.notify();
                    },
                )),
            );

        let view = self.diagram_view.clone();
        let measure = canvas(
            move |b, window, _| {
                // A resized view shows other cards
                if view.replace(b).size != b.size {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let dragging = self.diagram_drag.is_some();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .items_center()
                    .px_3()
                    .py_1p5()
                    .gap_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(legend)
                    .child(zoom_controls)
                    .child(depth_picker),
            )
            .child(
                div()
                    .id("er-view")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .cursor(if dragging {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::OpenHand
                    })
                    .child(measure)
                    .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| {
                        let d = e.delta.pixel_delta(window.line_height());
                        if e.modifiers.platform || e.modifiers.control {
                            let factor = (1.0 + f32::from(d.y) / 300.0).clamp(0.5, 2.0);
                            this.diagram_zoom_at(Some(e.position), factor);
                        } else {
                            this.diagram_pan = Some(this.diagram_pan() + d);
                        }
                        cx.notify();
                    }))
                    .on_pinch(cx.listener(|this, e: &PinchEvent, _, cx| {
                        this.diagram_zoom_at(Some(e.position), 1.0 + e.delta);
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, _| {
                            this.diagram_drag = Some(e.position)
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                        if let Some(last) = this.diagram_drag {
                            if e.pressed_button == Some(MouseButton::Left) {
                                this.diagram_pan = Some(this.diagram_pan() + (e.position - last));
                                this.diagram_drag = Some(e.position);
                                cx.notify();
                            } else {
                                this.diagram_drag = None;
                                cx.notify();
                            }
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.diagram_drag = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div().absolute().left(pan.x).top(pan.y).child(Scaled {
                            zoom: z,
                            child: div()
                                .relative()
                                .w(sz(width))
                                .h(sz(height))
                                .child(wires)
                                .children(captions)
                                .children(card_els)
                                .into_any_element(),
                        }),
                    ),
            )
            .into_any_element()
    }
}

impl QuarryWindow {
    /// Where the diagram sits now, whether placed by hand or centred
    fn diagram_pan(&self) -> Point<Pixels> {
        self.diagram_pan
            .unwrap_or_else(|| self.diagram_auto_pan.get())
    }

    /// Zooms by `factor`, keeping the point under `at` (window coordinates) still; `None` = the view's centre
    fn diagram_zoom_at(&mut self, at: Option<Point<Pixels>>, factor: f32) {
        let view = self.diagram_view.get();
        let at = at.unwrap_or_else(|| view.center()) - view.origin;
        let z0 = self.diagram_zoom;
        let z1 = (z0 * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.diagram_pan = Some(at - (at - self.diagram_pan()) * (z1 / z0));
        self.diagram_zoom = z1;
    }

    /// Zooms out (never in) until the whole diagram is in view, and centres it
    fn diagram_fit(&mut self, content: Size<f32>) {
        let view = self.diagram_view.get().size;
        let z = (f32::from(view.width) / content.width)
            .min(f32::from(view.height) / content.height)
            .clamp(ZOOM_MIN, 1.0);
        self.diagram_zoom = z;
        self.diagram_pan = Some(point(
            ((view.width - px(content.width * z)) / 2.0).max(px(0.)),
            px(0.),
        ));
    }
}

/// Lays out and paints its child at `zoom` × the window's rem size, so text and spacing zoom with it
struct Scaled {
    zoom: f32,
    child: AnyElement,
}

impl IntoElement for Scaled {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Scaled {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let rem = window.rem_size() * self.zoom;
        let id = window.with_rem_size(Some(rem), |w| self.child.request_layout(w, cx));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let rem = window.rem_size() * self.zoom;
        window.with_rem_size(Some(rem), |w| {
            self.child.prepaint(w, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let rem = window.rem_size() * self.zoom;
        window.with_rem_size(Some(rem), |w| self.child.paint(w, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::{diagram, join_sql, Edge};
    use quarry_core::schema::{
        ColumnSchema, ColumnType, ForeignKeyRef, SchemaCatalog, TableSchema,
    };

    fn catalog() -> SchemaCatalog {
        let col = |n: &str| ColumnSchema {
            name: n.into(),
            data_type: ColumnType::Integer,
            raw_type_name: "int".into(),
            is_nullable: false,
            is_primary_key: n == "id",
            default_value: None,
            ordinal_position: 0,
        };
        let fk = |from: &str, to: &str| ForeignKeyRef {
            from_column: from.into(),
            to_schema: "public".into(),
            to_table: to.into(),
            to_column: "id".into(),
        };
        let t = |n: &str, cols: &[&str], fks| TableSchema {
            schema_name: "public".into(),
            table_name: n.into(),
            is_view: false,
            columns: cols.iter().map(|c| col(c)).collect(),
            primary_keys: vec!["id".into()],
            foreign_keys: fks,
        };
        SchemaCatalog {
            schemas: vec!["public".into()],
            tables: vec![
                t("customers", &["id"], vec![]),
                t(
                    "orders",
                    &["id", "customer_id"],
                    vec![fk("customer_id", "customers")],
                ),
                t("items", &["id", "order_id"], vec![fk("order_id", "orders")]),
            ],
            default_schema: None,
            ..Default::default()
        }
    }

    #[test]
    fn hops_and_levels() {
        let cat = catalog();
        let d = diagram(&cat, &cat.tables[0], Some(1));
        let names = |d: &super::Diagram| {
            d.cards
                .iter()
                .map(|(t, l)| (t.table_name.clone(), *l))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&d),
            vec![("customers".into(), 0), ("orders".into(), 1)]
        );
        assert_eq!(
            d.edges,
            vec![Edge {
                from: (1, 1),
                to: (0, 0)
            }]
        );
        let d = diagram(&cat, &cat.tables[0], None);
        assert_eq!(names(&d).len(), 3);
        assert_eq!(d.edges.len(), 2);
    }

    #[test]
    fn joins_on_the_key() {
        let cat = catalog();
        let sql = join_sql(&cat.tables[0], &cat.tables[1], &quarry_core::dialect::Ansi);
        assert!(
            sql.contains("JOIN \"public\".\"orders\" o ON o.\"customer_id\" = c.\"id\""),
            "{sql}"
        );
    }
}
