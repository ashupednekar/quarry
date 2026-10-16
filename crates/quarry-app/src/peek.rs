// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Peek (U7): hover a table in the sidebar for its columns and first rows, without opening a tab.

use crate::pins::Pin;
use crate::runtime::run_db;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::ButtonVariants;
use gpui::component::ActiveTheme;
use gpui::component::Sizable;
use gpui::*;
use quarry_core::{QueryResult, TableSchema};

const DELAY_MS: u64 = 400;
const ROWS: usize = 5;
const COLS: usize = 6;

pub struct Peek {
    table: TableSchema,
    at: Point<Pixels>,
    rows: Option<Result<QueryResult, String>>,
    /// The mouse moved onto the card: keep it open for its buttons
    card_hovered: bool,
}

impl QuarryWindow {
    pub fn peek_hover(
        &mut self,
        schema: String,
        table: String,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            // A moment's grace to move onto the card; it stays while hovered
            self.peek_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                this.update(cx, |this, cx| {
                    if !this.peek.as_ref().is_some_and(|p| p.card_hovered) {
                        this.peek = None;
                        cx.notify();
                    }
                })
                .ok();
            }));
            return;
        }
        let Some(tbl) = self
            .state
            .catalog
            .find_table(Some(&schema), &table)
            .cloned()
        else {
            return;
        };
        let at = window.mouse_position();
        // Only after resting on the row for a moment, so moving the mouse through the list stays quiet
        self.peek_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(DELAY_MS))
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(session) = this.state.session.clone() else {
                    return;
                };
                let sql =
                    this.state
                        .select_rows_sql(&tbl.schema_name, &tbl.table_name, ROWS as u64);
                this.peek = Some(Peek {
                    table: tbl,
                    at,
                    rows: None,
                    card_hovered: false,
                });
                cx.notify();
                run_db(
                    window,
                    cx,
                    async move { session.execute_quiet(&sql).await.map_err(|e| e.to_string()) },
                    |this, res, _, _| {
                        if let Some(p) = this.peek.as_mut() {
                            p.rows = Some(res);
                        }
                    },
                );
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
            div()
                .w(px(110.0))
                .text_sm()
                .font_family(mono.clone())
                .text_color(if muted { t.text_muted } else { t.text_primary })
                .truncate()
                .child(text)
        };
        let rows: AnyElement = match &p.rows {
            None => div()
                .text_sm()
                .text_color(t.text_muted)
                .child("Loading…")
                .into_any_element(),
            Some(Err(e)) => div()
                .text_sm()
                .text_color(t.danger)
                .child(e.clone())
                .into_any_element(),
            Some(Ok(r)) if r.rows.is_empty() => div()
                .text_sm()
                .text_color(t.text_muted)
                .child("No rows")
                .into_any_element(),
            Some(Ok(r)) => v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .children(r.columns.iter().take(COLS).map(|c| cell(c.clone(), true))),
                )
                .children(r.rows.iter().map(|row| {
                    h_flex().gap_2().children(row.iter().take(COLS).map(|v| {
                        cell(
                            if v.is_null() {
                                "NULL".into()
                            } else {
                                v.to_display_string()
                            },
                            v.is_null(),
                        )
                    }))
                }))
                .into_any_element(),
        };
        let (schema, table) = (p.table.schema_name.clone(), p.table.table_name.clone());
        let action = |id: &'static str,
                      label: &'static str,
                      run: fn(
            &mut QuarryWindow,
            String,
            String,
            &mut Window,
            &mut Context<QuarryWindow>,
        )| {
            let (schema, table) = (schema.clone(), table.clone());
            zbutton(id)
                .pad_xs()
                .label(label)
                .ghost()
                .small()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.peek = None;
                    this.peek_task = None;
                    run(this, schema.clone(), table.clone(), window, cx);
                    cx.notify();
                }))
        };
        let card = v_flex()
            .id("peek-card")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                match (this.peek.as_mut(), *hovered) {
                    (Some(p), true) => p.card_hovered = true,
                    (Some(_), false) => this.peek = None,
                    _ => {}
                }
                cx.notify();
            }))
            .w(px(520.0))
            .p_3()
            .gap_2()
            .rounded(px(10.0))
            .bg(t.bg_sidebar)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_primary)
                            .child(p.table.table_name.clone()),
                    )
                    .child(div().text_sm().text_color(t.text_muted).child(format!(
                        "{} · {} columns{}",
                        p.table.schema_name,
                        p.table.columns.len(),
                        if p.table.is_view { " · view" } else { "" }
                    ))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .children(p.table.columns.iter().map(|c| {
                        let key = if c.is_primary_key {
                            " PK"
                        } else if p.table.foreign_keys.iter().any(|f| f.from_column == c.name) {
                            " FK"
                        } else {
                            ""
                        };
                        h_flex()
                            .gap_1()
                            .text_sm()
                            .child(div().text_color(t.text_secondary).child(c.name.clone()))
                            .child(div().text_color(t.text_muted).child(format!(
                                "{}{}",
                                c.raw_type_name.to_lowercase(),
                                key
                            )))
                    })),
            )
            .child(div().h_px().bg(t.border))
            .child(rows)
            .child(
                h_flex()
                    .gap_1()
                    .child(action("peek-open", "Open", |this, s, t, w, cx| {
                        this.open_pin(
                            Pin::Table {
                                schema: s,
                                table: t,
                            },
                            w,
                            cx,
                        )
                    }))
                    .child(action(
                        "peek-structure",
                        "Structure",
                        |this, s, t, w, cx| {
                            this.open_pin(
                                Pin::Table {
                                    schema: s,
                                    table: t,
                                },
                                w,
                                cx,
                            );
                            this.set_table_view(crate::table_detail::TableView::Structure, w, cx);
                        },
                    ))
                    .child(action("peek-query", "New Query", |this, s, t, w, cx| {
                        this.open_query_tab(
                            &format!("{};", this.state.select_rows_sql(&s, &t, 100)),
                            w,
                            cx,
                        )
                    }))
                    .child(action("peek-copy", "Copy Name", |this, s, t, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            this.state.dialect().qualified(&s, &t),
                        ))
                    }))
                    .child(div().flex_1())
                    .when(cols.len() < p.table.columns.len(), |d| {
                        d.child(div().text_sm().text_color(t.text_muted).child(format!(
                            "{} of {} columns shown",
                            cols.len(),
                            p.table.columns.len()
                        )))
                    }),
            );
        Some(
            deferred(
                anchored()
                    .position(point(p.at.x + px(200.0), p.at.y - px(12.0)))
                    .snap_to_window()
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
