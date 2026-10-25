// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Right-click menu for database objects: every object offers its actions in one place (OOUX)

use crate::pins::Pin;
use crate::QuarryWindow;
use gpui::component::menu::{PopupMenu, PopupMenuItem};
use gpui::*;

#[derive(Clone)]
pub enum DbObject {
    Connection {
        id: String,
        name: String,
    },
    Schema(String),
    Table {
        schema: String,
        table: String,
    },
    /// A saved .sql file in the sidebar's Queries section
    Query(std::path::PathBuf),
}

pub const REVEAL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Show in Folder"
};

/// One Lucide icon per action, so the same action looks the same in every menu
fn icon_for(label: &str) -> gpui::assets::IconName {
    use gpui::assets::IconName as I;
    match label {
        "Open" => I::Table,
        "Structure" => I::TableProperties,
        "DDL" => I::FileCode,
        "New Query" | "New Query on Table" => I::SquareTerminal,
        "Refresh Schema" => I::RefreshCw,
        "Connect" => I::Plug,
        "Disconnect" => I::Unplug,
        "Edit…" => I::Pencil,
        "Remove" | "Drop…" | "Move to Trash" => I::Trash,
        "Open Query" => I::SquareTerminal,
        "Rename…" => I::Pencil,
        "Duplicate" => I::Copy,
        REVEAL => I::FolderOpen,
        "Truncate…" => I::Eraser,
        "Export…" => I::Download,
        "Pin" => I::Pin,
        "Unpin" => I::PinOff,
        _ => I::Copy, // Copy Name
    }
}

type Act = fn(&mut QuarryWindow, &DbObject, &mut Window, &mut Context<QuarryWindow>);

impl QuarryWindow {
    pub fn open_object_menu(&mut self, obj: DbObject, window: &mut Window, cx: &mut Context<Self>) {
        let is_active = |id: &str| {
            self.state
                .active_connection
                .as_ref()
                .is_some_and(|c| c.id == id)
        };
        let conn_id = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let mut items: Vec<(&str, Act)> = Vec::new();
        match &obj {
            DbObject::Connection { id, .. } => {
                if is_active(id) {
                    items.push(("New Query", |this, _, w, cx| this.open_query_tab("", w, cx)));
                    items.push(("Refresh Schema", |this, _, w, cx| {
                        this.refresh_catalog(w, cx)
                    }));
                } else {
                    items.push(("Connect", |this, o, w, cx| {
                        if let DbObject::Connection { id, .. } = o {
                            this.connect_saved(id.clone(), w, cx)
                        }
                    }));
                }
                if self.state.open_sessions.contains_key(id) {
                    items.push(("Disconnect", |this, o, _, cx| {
                        if let DbObject::Connection { id, name } = o {
                            this.state.disconnect(id);
                            this.toast(format!("Disconnected from {name}"), false, cx);
                        }
                    }));
                }
                items.push(("Edit…", |this, o, w, cx| {
                    if let DbObject::Connection { id, .. } = o {
                        this.open_edit_conn(id, w, cx)
                    }
                }));
                items.push(("Remove", |this, o, _, cx| {
                    if let DbObject::Connection { id, name } = o {
                        this.state.remove_connection(id);
                        this.toast(format!("Removed {name}"), false, cx);
                    }
                }));
            }
            DbObject::Schema(_) => {
                items.push(("New Query", |this, _, w, cx| this.open_query_tab("", w, cx)));
                items.push(("Refresh Schema", |this, _, w, cx| {
                    this.refresh_catalog(w, cx)
                }));
            }
            DbObject::Table { schema, table } => {
                let pinned = conn_id
                    .and_then(|c| self.state.pins.get(&c))
                    .is_some_and(|p| {
                        p.contains(&Pin::Table {
                            schema: schema.clone(),
                            table: table.clone(),
                        })
                    });
                items.push(("Open", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_pin(
                            Pin::Table {
                                schema: schema.clone(),
                                table: table.clone(),
                            },
                            w,
                            cx,
                        )
                    }
                }));
                items.push(("Structure", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_pin(
                            Pin::Table {
                                schema: schema.clone(),
                                table: table.clone(),
                            },
                            w,
                            cx,
                        );
                        this.set_table_view(crate::table_detail::TableView::Structure, w, cx);
                    }
                }));
                items.push(("DDL", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_pin(
                            Pin::Table {
                                schema: schema.clone(),
                                table: table.clone(),
                            },
                            w,
                            cx,
                        );
                        this.set_table_view(crate::table_detail::TableView::Ddl, w, cx);
                    }
                }));
                items.push(("New Query on Table", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_query_tab(
                            &format!("{};", this.state.select_rows_sql(schema, table, 100)),
                            w,
                            cx,
                        )
                    }
                }));
                items.push(("Export…", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_pin(
                            Pin::Table {
                                schema: schema.clone(),
                                table: table.clone(),
                            },
                            w,
                            cx,
                        );
                        this.open_export(cx);
                    }
                }));
                items.push(("Find Usages", |this, o, w, cx| {
                    if let DbObject::Table { table, .. } = o {
                        this.find_usages(table, w, cx)
                    }
                }));
                items.push(("Rename…", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        let sql = this.rename_table_sql(schema, table);
                        this.open_query_tab(&sql, w, cx)
                    }
                }));
                // Destructive statements open in a query tab to review and run, so the prod check still applies
                items.push(("Truncate…", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_query_tab(
                            &format!(
                                "-- Deletes every row\nTRUNCATE TABLE {};",
                                this.state.dialect().qualified(schema, table)
                            ),
                            w,
                            cx,
                        )
                    }
                }));
                items.push(("Drop…", |this, o, w, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.open_query_tab(
                            &format!(
                                "-- Deletes the table and its data\nDROP TABLE {};",
                                this.state.dialect().qualified(schema, table)
                            ),
                            w,
                            cx,
                        )
                    }
                }));
                items.push((if pinned { "Unpin" } else { "Pin" }, |this, o, _, cx| {
                    if let DbObject::Table { schema, table } = o {
                        this.toggle_pin(
                            Pin::Table {
                                schema: schema.clone(),
                                table: table.clone(),
                            },
                            cx,
                        )
                    }
                }));
            }
            DbObject::Query(_) => {
                items.push(("Open Query", |this, o, w, cx| {
                    if let DbObject::Query(p) = o {
                        this.open_query_file(p.clone(), w, cx)
                    }
                }));
                items.push(("Rename…", |this, o, w, cx| {
                    if let DbObject::Query(p) = o {
                        this.rename_query_file(p.clone(), w, cx)
                    }
                }));
                items.push(("Duplicate", |this, o, _, cx| {
                    if let DbObject::Query(p) = o {
                        this.duplicate_query_file(p, cx)
                    }
                }));
                items.push((REVEAL, |_, o, _, _| {
                    if let DbObject::Query(p) = o {
                        crate::export_dialog::reveal(p)
                    }
                }));
                items.push(("Move to Trash", |this, o, _, cx| {
                    if let DbObject::Query(p) = o {
                        this.trash_query_file(p, cx)
                    }
                }));
            }
        }
        items.push((
            if matches!(obj, DbObject::Query(_)) {
                "Copy Path"
            } else {
                "Copy Name"
            },
            |this, o, _, cx| {
                let text = match o {
                    DbObject::Connection { name, .. } => name.clone(),
                    DbObject::Schema(s) => s.clone(),
                    DbObject::Table { schema, table } => {
                        this.state.dialect().qualified(schema, table)
                    }
                    DbObject::Query(p) => p.display().to_string(),
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            },
        ));

        let me = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, |mut menu, _, _| {
            for (label, act) in items {
                let (me, obj) = (me.clone(), obj.clone());
                menu = menu.item(PopupMenuItem::new(label).icon(icon_for(label)).on_click(
                    move |_, window, cx| {
                        me.update(cx, |this, cx| {
                            act(this, &obj, window, cx);
                            cx.notify();
                        })
                        .ok();
                    },
                ));
            }
            menu
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        // Shares the grid's slot: one popup menu open at a time
        self.grid_menu = Some((window.mouse_position(), menu, sub));
        cx.notify();
    }
}
