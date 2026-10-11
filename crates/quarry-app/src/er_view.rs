// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! A table and its direct neighbours: what it references on the left, what references it on
//! the right. Read from the catalog's foreign keys; click a neighbour to go there.

use crate::ui::*;
use crate::QuarryWindow;
use gpui::*;
use quarry_core::schema::{SchemaCatalog, TableSchema};

/// (neighbour, the link shown on its card: "orders.customer_id → id")
type Link<'a> = (&'a TableSchema, String);

/// Tables `table` points at, and tables that point at it
pub fn neighbours<'a>(catalog: &'a SchemaCatalog, table: &'a TableSchema) -> (Vec<Link<'a>>, Vec<Link<'a>>) {
    let schema_of = |s: &'a str| if s.is_empty() { table.schema_name.as_str() } else { s };
    let mut out: Vec<Link> = Vec::new();
    for fk in &table.foreign_keys {
        if let Some(t) = catalog.find_table(Some(schema_of(&fk.to_schema)), &fk.to_table) {
            out.push((t, format!("{} → {}", fk.from_column, fk.to_column)));
        }
    }
    let mut inc: Vec<Link> = Vec::new();
    for o in &catalog.tables {
        for fk in &o.foreign_keys {
            let to_schema = if fk.to_schema.is_empty() { o.schema_name.as_str() } else { fk.to_schema.as_str() };
            if fk.to_table == table.table_name && to_schema == table.schema_name {
                inc.push((o, format!("{} → {}", fk.from_column, fk.to_column)));
            }
        }
    }
    (out, inc)
}

impl QuarryWindow {
    pub fn render_diagram(&self, schema: &str, table: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let Some(center) = self.state.catalog.find_table(Some(schema), table) else {
            return div().p_4().text_xs().text_color(t.text_muted).child("Not in the schema; refresh it from the connection's menu").into_any_element();
        };
        let (outgoing, incoming) = neighbours(&self.state.catalog, center);
        let fk_cols: Vec<&str> = center.foreign_keys.iter().map(|f| f.from_column.as_str()).collect();

        // A table card. `columns`: (name, badge) rows; `link`: how it joins the centre table
        let card = |id: ElementId, tbl: &TableSchema, link: Option<String>, columns: Vec<(String, &'static str)>, focus: bool, cx: &mut Context<Self>| {
            let (s, n) = (tbl.schema_name.clone(), tbl.table_name.clone());
            v_flex()
                .id(id)
                .w(px(240.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(if focus { t.accent } else { t.border })
                .bg(t.bg_sidebar)
                .overflow_hidden()
                .when(!focus, |d| {
                    d.cursor_pointer().hover(|s| s.border_color(t.accent)).on_click(cx.listener(move |this, _, window, cx| {
                        this.open_pin(crate::pins::Pin::Table { schema: s.clone(), table: n.clone() }, window, cx)
                    }))
                })
                .child(
                    v_flex()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text_primary).child(tbl.table_name.clone()))
                        .children(link.map(|l| div().text_xs().text_color(t.accent).child(l))),
                )
                .children(columns.into_iter().map(|(name, badge)| {
                    h_flex()
                        .px_3()
                        .py_0p5()
                        .gap_2()
                        .text_xs()
                        .child(div().flex_1().min_w_0().truncate().text_color(t.text_secondary).child(name))
                        .child(div().text_color(t.text_muted).child(badge))
                }))
        };
        // Neighbours show only their keys: enough to read the join, not a second structure view
        let keys = |tbl: &TableSchema| -> Vec<(String, &'static str)> {
            tbl.columns
                .iter()
                .filter(|c| c.is_primary_key || tbl.foreign_keys.iter().any(|f| f.from_column == c.name))
                .map(|c| (c.name.clone(), if c.is_primary_key { "PK" } else { "FK" }))
                .collect()
        };
        let side = |title: &'static str, links: Vec<Link>, prefix: &'static str, cx: &mut Context<Self>| {
            v_flex()
                .gap_3()
                .items_center()
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(t.text_muted).child(title))
                .when(links.is_empty(), |d| d.child(div().text_xs().text_color(t.text_muted).child("None")))
                .children(links.into_iter().enumerate().map(|(i, (tbl, link))| card((prefix, i).into(), tbl, Some(link), keys(tbl), false, cx)))
        };
        let center_cols = center
            .columns
            .iter()
            .map(|c| {
                let badge = match (c.is_primary_key, fk_cols.contains(&c.name.as_str())) {
                    (true, true) => "PK FK",
                    (true, false) => "PK",
                    (false, true) => "FK",
                    _ => "",
                };
                (c.name.clone(), badge)
            })
            .collect();
        let arrow = || div().text_color(t.text_muted).child("⟶");
        div()
            .id("er-view")
            .size_full()
            .overflow_scroll()
            .p_6()
            .child(
                h_flex()
                    .gap_6()
                    .items_start()
                    .justify_center()
                    .child(side("REFERENCES", outgoing, "er-out", cx))
                    .child(div().pt_8().child(arrow()))
                    .child(v_flex().items_center().gap_3().child(div().text_xs().text_color(t.text_muted).child(" ")).child(card("er-center".into(), center, None, center_cols, true, cx)))
                    .child(div().pt_8().child(arrow()))
                    .child(side("REFERENCED BY", incoming, "er-in", cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::neighbours;
    use quarry_core::schema::{ColumnSchema, ColumnType, ForeignKeyRef, SchemaCatalog, TableSchema};

    #[test]
    fn finds_both_directions() {
        let col = |n: &str| ColumnSchema { name: n.into(), data_type: ColumnType::Integer, raw_type_name: "int".into(), is_nullable: false, is_primary_key: n == "id", default_value: None, ordinal_position: 0 };
        let fk = |from: &str, to: &str| ForeignKeyRef { from_column: from.into(), to_schema: "public".into(), to_table: to.into(), to_column: "id".into() };
        let t = |n: &str, fks| TableSchema { schema_name: "public".into(), table_name: n.into(), is_view: false, columns: vec![col("id")], primary_keys: vec!["id".into()], foreign_keys: fks };
        let cat = SchemaCatalog {
            schemas: vec!["public".into()],
            tables: vec![t("customers", vec![]), t("orders", vec![fk("customer_id", "customers")]), t("items", vec![fk("order_id", "orders")])],
            default_schema: None,
        };
        let (out, inc) = neighbours(&cat, &cat.tables[1]);
        assert_eq!(out.iter().map(|(t, l)| (t.table_name.as_str(), l.as_str())).collect::<Vec<_>>(), vec![("customers", "customer_id → id")]);
        assert_eq!(inc.iter().map(|(t, l)| (t.table_name.as_str(), l.as_str())).collect::<Vec<_>>(), vec![("items", "order_id → id")]);
    }
}
