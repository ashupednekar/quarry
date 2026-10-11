// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! The editor toolbar's pickers: connection, schema and row limit

use super::*;

impl QuarryWindow {
    /// The editor's connection picker: every saved connection, grouped as in the sidebar
    pub(crate) fn open_connection_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut conns = self.state.saved_connections.clone();
        conns.sort_by_key(|c| c.env as u8);
        let current = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let me = cx.entity().downgrade();
        let menu = gpui::component::menu::PopupMenu::build(window, cx, |mut menu, _, _| {
            for c in conns {
                let me = me.clone();
                let label = format!("{}  ·  {}", c.name, c.env.label());
                menu = menu.item(
                    gpui::component::menu::PopupMenuItem::new(label)
                        .checked(current.as_ref() == Some(&c.id))
                        .on_click(move |_, window, cx| {
                            me.update(cx, |this, cx| this.use_connection(c.id.clone(), window, cx))
                                .ok();
                        }),
                );
            }
            menu
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        // Just under the toolbar row, so the menu doesn't cover the picker
        self.grid_menu = Some((
            window.mouse_position() + point(px(0.0), px(14.0)),
            menu,
            sub,
        ));
        cx.notify();
    }

    /// Shows a popup menu of (label, checked, value) under the mouse; picking one calls `apply` with its value
    pub(crate) fn open_choice_menu<V: Clone + 'static>(
        &mut self,
        choices: Vec<(String, bool, V)>,
        apply: fn(&mut Self, V, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity().downgrade();
        let menu = gpui::component::menu::PopupMenu::build(window, cx, |mut menu, _, _| {
            for (label, checked, value) in choices {
                let me = me.clone();
                menu = menu.item(
                    gpui::component::menu::PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            me.update(cx, |this, cx| {
                                apply(this, value.clone(), cx);
                                this.schedule_session_save(cx);
                                cx.notify();
                            })
                            .ok();
                        }),
                );
            }
            menu
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        self.grid_menu = Some((
            window.mouse_position() + point(px(0.0), px(14.0)),
            menu,
            sub,
        ));
        cx.notify();
    }

    pub(crate) fn open_schema_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query {
            schema: current, ..
        }) = self.state.active_tab()
        else {
            return;
        };
        let current = current.clone();
        let mut choices = vec![("Default schema".to_string(), current.is_none(), None)];
        choices.extend(
            self.state
                .catalog
                .schemas
                .iter()
                .map(|s| (s.clone(), current.as_ref() == Some(s), Some(s.clone()))),
        );
        self.open_choice_menu(
            choices,
            |this, v, _| {
                if let Some(Tab::Query { schema, .. }) =
                    this.state.tabs.get_mut(this.state.active_tab_index)
                {
                    *schema = v;
                }
            },
            window,
            cx,
        );
    }

    pub(crate) fn open_limit_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query {
            row_limit: current, ..
        }) = self.state.active_tab()
        else {
            return;
        };
        let current = *current;
        let choices = [None, Some(100), Some(500), Some(1000), Some(5000)]
            .into_iter()
            .map(|n| {
                (
                    n.map_or("No limit".to_string(), |n| format!("{n} rows")),
                    current == n,
                    n,
                )
            })
            .collect();
        self.open_choice_menu(
            choices,
            |this, v, _| {
                if let Some(Tab::Query { row_limit, .. }) =
                    this.state.tabs.get_mut(this.state.active_tab_index)
                {
                    *row_limit = v;
                }
            },
            window,
            cx,
        );
    }
}
