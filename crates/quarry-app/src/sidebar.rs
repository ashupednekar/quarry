// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


use crate::state::{AppState, ConnectionTargetKind};
use crate::theme::Theme;
use crate::ui::*;
use gpui::assets::IconName;
use gpui::component::{Icon, Sizable};
use gpui::*;

/// DataGrip-style database explorer: connections → schemas → tables/views.
pub struct Sidebar;

impl Sidebar {
    pub fn render<T: 'static>(
        state: &AppState,
        theme: &Theme,
        cx: &Context<T>,
        on_table_click: impl Fn(&mut T, String, String, &mut Window, &mut Context<T>) + Copy + 'static,
        on_add_conn: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_toggle_schema: impl Fn(&mut T, String, &mut Window, &mut Context<T>) + Copy + 'static,
        on_conn_click: impl Fn(&mut T, String, &mut Window, &mut Context<T>) + Copy + 'static,
    ) -> impl IntoElement {
        if !state.sidebar_visible {
            return div().into_any_element();
        }

        let active_id = state.active_connection.as_ref().map(|c| c.id.clone());
        let active_table = state.active_tab().and_then(|t| match t {
            crate::state::Tab::Table { table_name, schema_name, .. } => Some((schema_name.as_str(), table_name.as_str())),
            _ => None,
        });

        let icon = |name: IconName, color: Rgba| Icon::new(name).xsmall().text_color(color);
        let row = |id: ElementId, depth: f32| {
            h_flex()
                .id(id)
                .h(px(24.0))
                .pl(px(6.0 + depth * 14.0))
                .pr_2()
                .gap_1p5()
                .items_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
        };
        let label = |text: String, color: Rgba| div().flex_1().text_xs().text_color(color).truncate().child(text);
        let muted = |text: String| div().text_xs().text_color(theme.text_muted).child(text);

        let mut tree = v_flex().py_1p5().px_1p5().gap_px();

        for (conn_ix, conn) in state.saved_connections.iter().enumerate() {
            let is_active = active_id.as_deref() == Some(conn.id.as_str());
            let kind = match conn.kind {
                ConnectionTargetKind::Postgres => "postgres",
                _ => "sqlite",
            };
            let conn_id = conn.id.clone();
            tree = tree.child(
                row(("conn", conn_ix).into(), 0.0)
                    .on_click(cx.listener(move |this, _, window, cx| on_conn_click(this, conn_id.clone(), window, cx)))
                    .child(icon(if is_active { IconName::ChevronDown } else { IconName::ChevronRight }, theme.text_muted))
                    .child(icon(IconName::Database, if is_active { theme.accent } else { theme.text_muted }))
                    .child(label(conn.name.clone(), if is_active { theme.text_primary } else { theme.text_secondary }))
                    .child(muted(kind.to_string())),
            );
            if !is_active {
                continue;
            }

            for (schema_ix, schema) in state.catalog.schemas.iter().enumerate() {
                let tables: Vec<_> = state.catalog.tables.iter().filter(|t| &t.schema_name == schema).collect();
                let expanded = state.expanded_schemas.contains(schema);
                let schema_name = schema.clone();
                tree = tree.child(
                    row(("schema", schema_ix).into(), 1.0)
                        .on_click(cx.listener(move |this, _, window, cx| on_toggle_schema(this, schema_name.clone(), window, cx)))
                        .child(icon(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }, theme.text_muted))
                        .child(icon(IconName::Layers, theme.text_muted))
                        .child(label(schema.clone(), theme.text_secondary))
                        .child(muted(tables.len().to_string())),
                );
                if !expanded {
                    continue;
                }
                for (tbl_ix, tbl) in tables.into_iter().enumerate() {
                    let selected = active_table == Some((tbl.schema_name.as_str(), tbl.table_name.as_str()));
                    let (t_name, s_name) = (tbl.table_name.clone(), tbl.schema_name.clone());
                    tree = tree.child(
                        row(("table", schema_ix * 100_000 + tbl_ix).into(), 2.0)
                            .when(selected, |r| r.bg(theme.bg_selected))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                on_table_click(this, t_name.clone(), s_name.clone(), window, cx)
                            }))
                            .child(icon(
                                if tbl.is_view { IconName::Eye } else { IconName::Table },
                                if selected { theme.accent } else { theme.text_muted },
                            ))
                            .child(label(tbl.table_name.clone(), if selected { theme.text_primary } else { theme.text_secondary }))
                            .child(muted(tbl.columns.len().to_string())),
                    );
                }
            }
        }

        v_flex()
            .w(px(240.0))
            .h_full()
            .bg(theme.bg_sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(32.0))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.text_muted).child("DATABASE"))
                    .child(
                        div()
                            .id("sidebar-add-conn")
                            .p_1()
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .on_click(cx.listener(move |this, _, window, cx| on_add_conn(this, window, cx)))
                            .child(icon(IconName::Plus, theme.text_muted)),
                    ),
            )
            .child(div().id("sidebar-tree-scroll").flex_1().overflow_y_scroll().child(tree))
            .into_any_element()
    }
}
