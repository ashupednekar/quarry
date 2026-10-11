// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! A table tab is the table's home: its data, its structure, and its DDL (OOUX)

use crate::runtime::run_db;
use crate::state::Tab;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::ButtonVariants;
use gpui::component::input::EditorState;
use gpui::component::{ActiveTheme, Sizable};
use gpui::*;

#[derive(Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum TableView {
    #[default]
    Data,
    Structure,
    Ddl,
    /// The table and its foreign-key neighbours (catalog only, nothing to load)
    Diagram,
}

/// Loaded the first time Structure or DDL is opened
#[derive(Clone)]
pub struct TableDetail {
    /// (name, unique, columns)
    pub indexes: Vec<(String, bool, String)>,
    pub ddl: Entity<EditorState>,
}

impl QuarryWindow {
    pub fn set_table_view(&mut self, view: TableView, window: &mut Window, cx: &mut Context<Self>) {
        self.schedule_session_save(cx);
        let ix = self.state.active_tab_index;
        let Some(Tab::Table {
            view: v,
            detail,
            schema_name,
            table_name,
            grid,
            ..
        }) = self.state.tabs.get_mut(ix)
        else {
            return;
        };
        *v = view;
        cx.notify();
        if matches!(view, TableView::Data | TableView::Diagram) || detail.is_some() {
            return;
        }
        let Some(session) = grid
            .read(cx)
            .delegate()
            .source
            .as_ref()
            .map(|s| s.session.clone())
        else {
            return;
        };
        let (schema, table) = (schema_name.clone(), table_name.clone());
        let Some(tbl) = self
            .state
            .catalog
            .find_table(Some(&schema), &table)
            .cloned()
        else {
            return;
        };
        let fut = async move { session.table_info(&tbl).await.map_err(|e| e.to_string()) };
        run_db(window, cx, fut, move |this, res, window, cx| {
            let detail = res.map(|info| {
                let ddl = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language("sql")
                        .line_number(true)
                        .default_value(info.ddl)
                });
                let indexes = info
                    .indexes
                    .into_iter()
                    .map(|i| (i.name, i.unique, i.columns))
                    .collect();
                TableDetail { indexes, ddl }
            });
            // The tab may have moved or closed while loading
            if let Some(Tab::Table { detail: d, .. }) = this.state.tabs.iter_mut().find(|t| matches!(t, Tab::Table { schema_name: s, table_name: n, .. } if *s == schema && *n == table)) {
                *d = Some(detail);
            }
        });
    }

    /// Data · Structure · DDL
    pub fn render_table_view_switch(
        &self,
        current: TableView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        use gpui::assets::IconName;
        let views = [
            (TableView::Data, "Data", IconName::Table),
            (TableView::Structure, "Structure", IconName::TableProperties),
            (TableView::Ddl, "DDL", IconName::FileCode),
            (TableView::Diagram, "Diagram", IconName::Network),
        ];
        h_flex()
            .gap_0p5()
            .children(views.map(|(view, label, icon)| {
                let on = view == current;
                h_flex()
                    .id(label)
                    .gap_1()
                    .items_center()
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.0))
                    .text_xs()
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.bg_selected).text_color(t.text_primary))
                    .when(!on, |d| {
                        d.text_color(t.text_muted).hover(|s| s.bg(t.bg_hover))
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.set_table_view(view, window, cx)
                        }),
                    )
                    .child(gpui::component::Icon::new(icon).xsmall())
                    .child(label)
            }))
    }

    pub fn render_table_detail(&self, tab: &Tab, cx: &mut Context<Self>) -> AnyElement {
        let Tab::Table {
            view,
            detail,
            schema_name,
            table_name,
            ..
        } = tab
        else {
            return div().into_any_element();
        };
        let t = &self.theme;
        if *view == TableView::Diagram {
            return self.render_diagram(schema_name, table_name, cx);
        }
        let detail = match detail {
            None => {
                return div()
                    .p_4()
                    .text_xs()
                    .text_color(t.text_muted)
                    .child("Loading…")
                    .into_any_element()
            }
            Some(Err(e)) => {
                return div()
                    .p_4()
                    .text_xs()
                    .text_color(t.danger)
                    .child(e.clone())
                    .into_any_element()
            }
            Some(Ok(d)) => d,
        };
        if *view == TableView::Ddl {
            return v_flex()
                .size_full()
                .child(
                    h_flex().px_3().py_1().justify_end().child(
                        zbutton("ddl-to-query")
                            .pad_xs()
                            .label("Open in Query Tab")
                            .ghost()
                            .xsmall()
                            .on_click({
                                let ddl = detail.ddl.clone();
                                cx.listener(move |this, _, window, cx| {
                                    let sql = ddl.read(cx).value().to_string();
                                    this.open_query_tab(&sql, window, cx);
                                })
                            }),
                    ),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        gpui::component::input::Editor::new(&detail.ddl)
                            .h_full()
                            .bordered(false),
                    ),
                )
                .into_any_element();
        }

        let Some(table) = self.state.catalog.find_table(Some(schema_name), table_name) else {
            return div()
                .p_4()
                .text_xs()
                .text_color(t.text_muted)
                .child("Not in the schema; refresh it from the connection's menu")
                .into_any_element();
        };
        let mono = cx.theme().mono_font_family.clone();
        let heading = |s: &'static str| {
            div()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_muted)
                .child(s)
        };
        let cell = |w: f32, text: String, color: Rgba| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .truncate()
                .text_color(color)
                .child(text)
        };
        let row = || {
            h_flex()
                .py_1()
                .gap_3()
                .border_b_1()
                .border_color(t.border)
                .text_xs()
                .font_family(mono.clone())
        };

        let columns = v_flex()
            .child(
                row()
                    .child(cell(200.0, "name".into(), t.text_muted))
                    .child(cell(160.0, "type".into(), t.text_muted))
                    .child(cell(70.0, "null".into(), t.text_muted))
                    .child(cell(160.0, "default".into(), t.text_muted))
                    .child(cell(240.0, "key".into(), t.text_muted)),
            )
            .children(table.columns.iter().enumerate().map(|(ix, c)| {
                let fk = table.foreign_keys.iter().find(|f| f.from_column == c.name);
                let target = fk.map(|f| {
                    (
                        if f.to_schema.is_empty() {
                            table.schema_name.clone()
                        } else {
                            f.to_schema.clone()
                        },
                        f.to_table.clone(),
                    )
                });
                let key = match (c.is_primary_key, fk) {
                    (true, Some(f)) => format!("PK · FK → {}.{}", f.to_table, f.to_column),
                    (true, None) => "PK".into(),
                    (false, Some(f)) => format!("FK → {}.{}", f.to_table, f.to_column),
                    _ => String::new(),
                };
                row()
                    .child(cell(200.0, c.name.clone(), t.text_primary))
                    .child(cell(
                        160.0,
                        c.raw_type_name.to_lowercase(),
                        t.text_secondary,
                    ))
                    .child(cell(
                        70.0,
                        if c.is_nullable { "yes" } else { "no" }.into(),
                        t.text_muted,
                    ))
                    .child(cell(
                        160.0,
                        c.default_value.clone().unwrap_or_default(),
                        t.text_muted,
                    ))
                    .child(cell(240.0, key, t.accent).id(("fk", ix)).when_some(
                        target,
                        |d, (schema, table)| {
                            d.cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_pin(
                                        crate::pins::Pin::Table {
                                            schema: schema.clone(),
                                            table: table.clone(),
                                        },
                                        window,
                                        cx,
                                    )
                                }))
                        },
                    ))
            }));
        let referenced_by: Vec<_> = self
            .state
            .catalog
            .tables
            .iter()
            .flat_map(|o| {
                o.foreign_keys
                    .iter()
                    .filter(|f| f.to_table == table.table_name)
                    .map(move |f| {
                        (
                            (o.schema_name.clone(), o.table_name.clone()),
                            format!("{}.{} → {}", o.table_name, f.from_column, f.to_column),
                        )
                    })
            })
            .collect();
        let referenced_by =
            v_flex()
                .text_xs()
                .font_family(mono.clone())
                .gap_1()
                .when(referenced_by.is_empty(), |d| {
                    d.child(
                        div()
                            .text_color(t.text_muted)
                            .child("No other table points here"),
                    )
                })
                .children(referenced_by.into_iter().enumerate().map(
                    |(i, ((schema, name), label))| {
                        div()
                            .id(("ref-by", i))
                            .text_color(t.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_pin(
                                    crate::pins::Pin::Table {
                                        schema: schema.clone(),
                                        table: name.clone(),
                                    },
                                    window,
                                    cx,
                                )
                            }))
                            .child(label)
                    },
                ));
        div()
            .id("table-structure")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .child(heading("COLUMNS"))
            .child(columns)
            .child(heading("INDEXES"))
            .child(
                v_flex()
                    .when(detail.indexes.is_empty(), |d| {
                        d.child(div().text_xs().text_color(t.text_muted).child("None"))
                    })
                    .children(detail.indexes.iter().map(|(name, unique, cols)| {
                        row()
                            .child(cell(200.0, name.clone(), t.text_primary))
                            .child(cell(300.0, cols.clone(), t.text_secondary))
                            .child(cell(
                                70.0,
                                if *unique {
                                    "unique".into()
                                } else {
                                    String::new()
                                },
                                t.accent,
                            ))
                    })),
            )
            .child(heading("REFERENCED BY"))
            .child(referenced_by)
            .into_any_element()
    }
}
