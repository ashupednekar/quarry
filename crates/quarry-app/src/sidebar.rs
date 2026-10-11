// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::object_menu::DbObject;
use crate::state::{AppState, Environment};
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::{Icon, Sizable};
use gpui::*;

/// DataGrip-style database explorer: connections → schemas → tables/views.
pub struct Sidebar;

impl Sidebar {
    pub fn render(state: &AppState, theme: &Theme, cx: &Context<QuarryWindow>) -> impl IntoElement {
        if !state.sidebar_visible {
            return div().into_any_element();
        }

        let active_id = state.active_connection.as_ref().map(|c| c.id.clone());
        let active_table = state.active_tab().and_then(|t| match t {
            crate::state::Tab::Table {
                table_name,
                schema_name,
                ..
            } => Some((schema_name.as_str(), table_name.as_str())),
            _ => None,
        });

        let icon = |name: IconName, color: Rgba| Icon::new(name).xsmall().text_color(color);
        let row = |id: ElementId, depth: f32| {
            h_flex()
                .id(id)
                .h(px(ROW_H))
                .pl(px(8.0 + depth * 16.0))
                .pr_2()
                .gap_1p5()
                .items_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
        };
        let menu_on = |obj: DbObject| {
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_sidebar_menu(obj.clone(), window, cx)
            })
        };
        let label = |text: String, color: Rgba| {
            div()
                .flex_1()
                .text_xs()
                .text_color(color)
                .truncate()
                .child(text)
        };
        let muted = |text: String| div().text_xs().text_color(theme.text_muted).child(text);

        let mut tree = v_flex().py_2().px_2().gap_px();
        if state.saved_connections.is_empty() {
            tree = tree.child(
                v_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child("No connections yet."),
                    )
                    .child(
                        zbutton("sidebar-empty-add")
                            .pad_sm()
                            .label("Add Connection")
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_add_conn(window, cx)
                            })),
                    ),
            );
        }

        // Grouped by environment, so prod connections are never mixed in with local ones
        let mut conns: Vec<_> = state.saved_connections.iter().enumerate().collect();
        conns.sort_by_key(|(_, c)| c.env as u8);
        let mut last_env = None;
        for (conn_ix, conn) in conns {
            if last_env != Some(conn.env) {
                last_env = Some(conn.env);
                let color = match conn.env {
                    Environment::Local => theme.success,
                    Environment::Staging => theme.warning,
                    Environment::Prod => theme.danger,
                };
                tree = tree.child(
                    h_flex()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .gap_1p5()
                        .items_center()
                        .child(div().size(px(6.0)).rounded_full().bg(color))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_muted)
                                .child(conn.env.label()),
                        ),
                );
            }
            let is_active = active_id.as_deref() == Some(conn.id.as_str());
            let expanded = is_active && !state.collapsed_connections.contains(&conn.id);
            let kind = conn.driver.as_str();
            let conn_id = conn.id.clone();
            tree = tree.child(
                row(("conn", conn_ix).into(), 0.0)
                    .on_mouse_down(
                        MouseButton::Right,
                        menu_on(DbObject::Connection {
                            id: conn.id.clone(),
                            name: conn.name.clone(),
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_saved(conn_id.clone(), window, cx)
                    }))
                    .child(icon(
                        if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        theme.text_muted,
                    ))
                    .child(icon(
                        IconName::Database,
                        if is_active {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(label(
                        conn.name.clone(),
                        if is_active {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    ))
                    .child(muted(kind.to_string())),
            );
            if !expanded {
                continue;
            }

            // Pinned items first (U6): this connection's favourite tables and saved queries
            let pinned = state.pins.get(&conn.id).cloned().unwrap_or_default();
            for (pin_ix, pin) in pinned.into_iter().enumerate() {
                let (open_pin, drop_pin) = (pin.clone(), pin.clone());
                let is_table = matches!(pin, crate::pins::Pin::Table { .. });
                tree = tree.child(
                    row(("pin", pin_ix).into(), 1.0)
                        .group("pin-row")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_pin(open_pin.clone(), window, cx)
                        }))
                        .child(icon(IconName::Pin, theme.accent))
                        .child(icon(
                            if is_table {
                                IconName::Table
                            } else {
                                IconName::SquareTerminal
                            },
                            theme.text_muted,
                        ))
                        .child(label(pin.label().to_string(), theme.text_primary))
                        .child(
                            div()
                                .id(("unpin", pin_ix))
                                .invisible()
                                .group_hover("pin-row", |s| s.visible())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_pin(drop_pin.clone(), cx)
                                }))
                                .child(icon(IconName::PinOff, theme.text_muted)),
                        ),
                );
            }

            for (schema_ix, schema) in state.catalog.schemas.iter().enumerate() {
                let tables: Vec<_> = state
                    .catalog
                    .tables
                    .iter()
                    .filter(|t| &t.schema_name == schema)
                    .collect();
                let expanded = state.expanded_schemas.contains(schema);
                let schema_name = schema.clone();
                tree = tree.child(
                    row(("schema", schema_ix).into(), 1.0)
                        .on_mouse_down(
                            MouseButton::Right,
                            menu_on(DbObject::Schema(schema.clone())),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_sidebar_schema(schema_name.clone(), cx)
                        }))
                        .child(icon(
                            if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            theme.text_muted,
                        ))
                        .child(icon(IconName::Layers, theme.text_muted))
                        .child(label(schema.clone(), theme.text_secondary))
                        // Not loaded yet (big databases): no count rather than a wrong 0
                        .child(muted(if state.catalog.is_loaded(schema) {
                            tables.len().to_string()
                        } else {
                            "…".into()
                        })),
                );
                if !expanded {
                    continue;
                }
                if !state.catalog.is_loaded(schema) {
                    tree = tree.child(
                        row(("schema-loading", schema_ix).into(), 2.0)
                            .child(muted("Loading…".to_string())),
                    );
                    continue;
                }
                for (tbl_ix, tbl) in tables.into_iter().enumerate() {
                    let selected =
                        active_table == Some((tbl.schema_name.as_str(), tbl.table_name.as_str()));
                    let (t_name, s_name) = (tbl.table_name.clone(), tbl.schema_name.clone());
                    let (h_name, h_schema) = (t_name.clone(), s_name.clone());
                    tree = tree.child(
                        row(("table", schema_ix * 100_000 + tbl_ix).into(), 2.0)
                            .when(selected, |r| r.bg(theme.bg_selected))
                            .on_mouse_down(
                                MouseButton::Right,
                                menu_on(DbObject::Table {
                                    schema: tbl.schema_name.clone(),
                                    table: tbl.table_name.clone(),
                                }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_sidebar_table(&t_name, &s_name, window, cx)
                            }))
                            // Peek (U7): hovering previews the table without opening it
                            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                this.peek_hover(
                                    h_schema.clone(),
                                    h_name.clone(),
                                    *hovered,
                                    window,
                                    cx,
                                )
                            }))
                            .child(icon(
                                if tbl.is_view {
                                    IconName::Eye
                                } else {
                                    IconName::Table
                                },
                                if selected {
                                    theme.accent
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .child(label(
                                tbl.table_name.clone(),
                                if selected {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                },
                            ))
                            .child(muted(tbl.columns.len().to_string())),
                    );
                }

                // Functions, sequences, triggers, types: one collapsed group per kind, loaded on expand
                let objects = state.schema_objects.get(schema);
                if matches!(objects, Some(None)) {
                    tree = tree.child(
                        row(("objects-loading", schema_ix).into(), 2.0)
                            .child(muted("Loading…".into())),
                    );
                }
                let objects = objects.and_then(|o| o.as_deref()).unwrap_or_default();
                for (kind_ix, kind) in quarry_core::schema::ObjectKind::ALL.into_iter().enumerate()
                {
                    let of_kind: Vec<_> = objects.iter().filter(|o| o.kind == kind).collect();
                    if of_kind.is_empty() {
                        continue;
                    }
                    let key = format!("{schema}/{}", kind.plural());
                    let open = state.expanded_object_groups.contains(&key);
                    let group_id = schema_ix * 10 + kind_ix;
                    tree = tree.child(
                        row(("object-group", group_id).into(), 2.0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_sidebar_group(key.clone(), cx)
                            }))
                            .child(icon(
                                if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme.text_muted,
                            ))
                            .child(icon(object_icon(kind), theme.text_muted))
                            .child(label(kind.plural().to_string(), theme.text_secondary))
                            .child(muted(of_kind.len().to_string())),
                    );
                    if !open {
                        continue;
                    }
                    for (obj_ix, obj) in of_kind.into_iter().enumerate() {
                        let (s_name, o) = (schema.clone(), obj.clone());
                        tree = tree.child(
                            row(("object", group_id * 100_000 + obj_ix).into(), 3.0)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_object_definition(
                                        s_name.clone(),
                                        o.clone(),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(icon(object_icon(kind), theme.text_muted))
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(140.0))
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .truncate()
                                        .child(obj.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .truncate()
                                        .child(obj.detail.clone()),
                                ),
                        );
                    }
                }
            }
        }

        let active_file = state.active_tab().and_then(|t| match t {
            crate::state::Tab::Query { file, .. } => file.clone(),
            _ => None,
        });
        let queries = crate::files::saved_queries();
        let recent = crate::files::recent_files();
        let mut query_list = v_flex().py_1().px_2().gap_px();
        if queries.is_empty() && recent.is_empty() {
            query_list = query_list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("Save a query (⌘S) to see it here."),
            );
        }
        for (ix, path) in queries.into_iter().enumerate() {
            if let Some((_, input)) = state.renaming_query.as_ref().filter(|(p, _)| *p == path) {
                query_list = query_list.child(
                    h_flex()
                        .h(px(ROW_H))
                        .pl_2()
                        .pr_1()
                        .gap_1p5()
                        .items_center()
                        .child(icon(IconName::FileCode, theme.accent))
                        .child(
                            div()
                                .flex_1()
                                .child(gpui::component::input::Input::new(input).xsmall()),
                        ),
                );
                continue;
            }
            let selected = active_file.as_ref() == Some(&path);
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            query_list = query_list.child(
                row(("query-file", ix).into(), 0.0)
                    .when(selected, |r| r.bg(theme.bg_selected))
                    .on_mouse_down(MouseButton::Right, menu_on(DbObject::Query(path.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_query_file(path.clone(), window, cx)
                    }))
                    .child(icon(
                        IconName::FileCode,
                        if selected {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(label(
                        name,
                        if selected {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    )),
            );
        }
        if !recent.is_empty() {
            query_list = query_list.child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("Recent"),
            );
        }
        for (ix, path) in recent.into_iter().enumerate() {
            let selected = active_file.as_ref() == Some(&path);
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let folder = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            query_list = query_list.child(
                row(("recent-file", ix).into(), 0.0)
                    .when(selected, |r| r.bg(theme.bg_selected))
                    .on_mouse_down(MouseButton::Right, menu_on(DbObject::Query(path.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_query_file(path.clone(), window, cx)
                    }))
                    .child(icon(
                        IconName::FileCode,
                        if selected {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(label(
                        name,
                        if selected {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    ))
                    .child(muted(folder)),
            );
        }
        let section_title = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(text)
        };

        v_flex()
            .size_full()
            .bg(theme.bg_sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(BAR_H))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .child(section_title("DATABASE"))
                    .child(
                        div()
                            .id("sidebar-add-conn")
                            .p_1()
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_add_conn(window, cx)
                            }))
                            .child(icon(IconName::Plus, theme.text_muted)),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    // Drag the divider to give queries more or less room
                    gpui::component::resizable::v_resizable("sidebar-split")
                        .on_resize(crate::layout::remember("sidebar-split"))
                        .child(
                            gpui::component::resizable::resizable_panel().child(
                                div()
                                    .id("sidebar-tree-scroll")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .child(tree),
                            ),
                        )
                        .child(
                            gpui::component::resizable::resizable_panel()
                                .size(crate::layout::size("sidebar-split", 1, 200.0))
                                .size_range(px(BAR_H)..px(2000.0))
                                .child(
                                    v_flex()
                                        .size_full()
                                        .child(
                                            h_flex()
                                                .h(px(BAR_H))
                                                .flex_none()
                                                .px_3()
                                                .items_center()
                                                .child(section_title("QUERIES")),
                                        )
                                        .child(
                                            div()
                                                .id("sidebar-queries-scroll")
                                                .flex_1()
                                                .min_h_0()
                                                .overflow_y_scroll()
                                                .child(query_list),
                                        ),
                                ),
                        ),
                ),
            )
            .into_any_element()
    }
}

fn object_icon(kind: quarry_core::schema::ObjectKind) -> IconName {
    use quarry_core::schema::ObjectKind::*;
    match kind {
        Function => IconName::SquareFunction,
        Procedure => IconName::Workflow,
        Sequence => IconName::ListOrdered,
        Trigger => IconName::Zap,
        Type => IconName::Tag,
    }
}

/// Sidebar clicks that do more than call one existing method
impl QuarryWindow {
    fn open_sidebar_table(
        &mut self,
        table: &str,
        schema: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.peek = None;
        self.peek_task = None;
        self.open_table_tab(table, schema, window, cx);
        self.refresh_active_tab(window, cx);
        cx.notify();
    }

    fn toggle_sidebar_schema(&mut self, schema: String, cx: &mut Context<Self>) {
        self.state.toggle_schema(&schema);
        if self.state.expanded_schemas.contains(&schema) {
            self.load_schemas(vec![schema], cx);
        }
        cx.notify();
    }

    fn open_sidebar_menu(&mut self, obj: DbObject, window: &mut Window, cx: &mut Context<Self>) {
        self.peek = None;
        self.open_object_menu(obj, window, cx)
    }

    fn toggle_sidebar_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.state.expanded_object_groups.remove(&key) {
            self.state.expanded_object_groups.insert(key);
        }
        cx.notify();
    }
}
