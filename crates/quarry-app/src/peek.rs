// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Peek (U7): hover a table in the sidebar for its columns and first rows, without opening a tab.

use crate::runtime::run_db;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::ActiveTheme;
use gpui::*;
use quarry_core::{QueryResult, TableSchema};

const DELAY_MS: u64 = 400;
const ROWS: usize = 5;
const COLS: usize = 6;

pub struct Peek {
    table: TableSchema,
    at: Point<Pixels>,
    rows: Option<Result<QueryResult, String>>,
}

impl QuarryWindow {
    pub fn peek_hover(&mut self, schema: String, table: String, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !hovered {
            self.peek = None;
            self.peek_task = None;
            cx.notify();
            return;
        }
        let Some(tbl) = self.state.catalog.find_table(Some(&schema), &table).cloned() else { return };
        let at = window.mouse_position();
        // Only after resting on the row for a moment, so moving the mouse through the list stays quiet
        self.peek_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(DELAY_MS)).await;
            this.update_in(cx, |this, window, cx| {
                let Some(session) = this.state.session.clone() else { return };
                let sql = format!("SELECT * FROM {} LIMIT {ROWS}", quarry_core::edits::qualified_name(&tbl.schema_name, &tbl.table_name));
                this.peek = Some(Peek { table: tbl, at, rows: None });
                cx.notify();
                run_db(window, cx, async move { session.execute_quiet(&sql).await.map_err(|e| e.to_string()) }, |this, res, _, _| {
                    if let Some(p) = this.peek.as_mut() {
                        p.rows = Some(res);
                    }
                });
            })
            .ok();
        }));
    }

    /// Shown to the right of the hovered row, clear of the table list so moving down the list keeps working
    pub fn render_peek(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.peek.as_ref()?;
        let t = &self.theme;
        let mono = cx.theme().mono_font_family.clone();
        let cols: Vec<_> = p.table.columns.iter().take(COLS).collect();
        let cell = |text: String, muted: bool| {
            div().w(px(110.0)).text_xs().font_family(mono.clone()).text_color(if muted { t.text_muted } else { t.text_primary }).truncate().child(text)
        };
        let rows: AnyElement = match &p.rows {
            None => div().text_xs().text_color(t.text_muted).child("Loading…").into_any_element(),
            Some(Err(e)) => div().text_xs().text_color(t.danger).child(e.clone()).into_any_element(),
            Some(Ok(r)) if r.rows.is_empty() => div().text_xs().text_color(t.text_muted).child("No rows").into_any_element(),
            Some(Ok(r)) => v_flex()
                .gap_1()
                .child(h_flex().gap_2().children(r.columns.iter().take(COLS).map(|c| cell(c.clone(), true))))
                .children(r.rows.iter().map(|row| {
                    h_flex().gap_2().children(row.iter().take(COLS).map(|v| cell(if v.is_null() { "NULL".into() } else { v.to_display_string() }, v.is_null())))
                }))
                .into_any_element(),
        };
        let card = v_flex()
            .w(px(520.0))
            .p_3()
            .gap_2()
            .rounded(px(8.0))
            .bg(t.bg_sidebar)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text_primary).child(p.table.table_name.clone()))
                    .child(div().text_xs().text_color(t.text_muted).child(format!(
                        "{} · {} columns{}",
                        p.table.schema_name,
                        p.table.columns.len(),
                        if p.table.is_view { " · view" } else { "" }
                    ))),
            )
            .child(
                h_flex().flex_wrap().gap_x_3().gap_y_1().children(p.table.columns.iter().map(|c| {
                    let key = if c.is_primary_key {
                        " PK"
                    } else if p.table.foreign_keys.iter().any(|f| f.from_column == c.name) {
                        " FK"
                    } else {
                        ""
                    };
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .child(div().text_color(t.text_secondary).child(c.name.clone()))
                        .child(div().text_color(t.text_muted).child(format!("{}{}", c.raw_type_name.to_lowercase(), key)))
                })),
            )
            .child(div().h_px().bg(t.border))
            .child(rows)
            .child(div().text_xs().text_color(t.text_muted).child(if cols.len() < p.table.columns.len() {
                "Click to open the full table · more columns there"
            } else {
                "Click to open the full table"
            }));
        Some(deferred(anchored().position(point(p.at.x + px(200.0), p.at.y - px(12.0))).snap_to_window().child(card)).with_priority(2).into_any_element())
    }
}
