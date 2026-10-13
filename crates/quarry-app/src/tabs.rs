// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Opening, closing, splitting and pinning tabs

use super::*;

impl QuarryWindow {
    pub(crate) fn new_query(&mut self, _: &NewQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.open_query_tab("", window, cx);
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        // ⌘W with nothing left to close closes the window, as everywhere else on macOS
        if self.state.tabs.is_empty() {
            return self.handle_close_window(&CloseWindow, window, cx);
        }
        self.close_tab_at(self.state.active_tab_index, cx);
        // The closed tab may have held focus; keep shortcuts (like ⇧⌘T) working
        self.refocus_root(window, cx);
    }

    pub(crate) fn close_tab_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.state.close_tab(idx) else {
            return;
        };
        // Kept (with its grid, editor and pending edits) so ⇧⌘T can bring it back (U9)
        let id = tab.id().to_string();
        let subs = [id.clone(), format!("{id}:editor")]
            .into_iter()
            .filter_map(|k| self.tab_subscriptions.remove(&k).map(|s| (k, s)))
            .collect();
        self.closed_tabs.push((tab, subs));
        if self.closed_tabs.len() > 20 {
            self.closed_tabs.remove(0);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Closes every tab; each stays reopenable with ⇧⌘T
    pub(crate) fn close_all_tabs(&mut self, cx: &mut Context<Self>) {
        self.split_id = None;
        while !self.state.tabs.is_empty() {
            self.close_tab_at(self.state.tabs.len() - 1, cx);
        }
    }

    /// The split pane's tab, if it's set and isn't the active one
    pub(crate) fn split_tab(&self) -> Option<Tab> {
        let id = self.split_id.as_deref()?;
        let active = self.state.active_tab()?.id();
        (id != active)
            .then(|| self.state.tabs.iter().find(|t| t.id() == id).cloned())
            .flatten()
    }

    /// ⌘\: put the current tab in the right pane next to the previous one, or close the split
    pub(crate) fn toggle_split(&mut self, cx: &mut Context<Self>) {
        self.split_active_right = false;
        if self.split_tab().is_some() {
            self.split_id = None;
        } else if self.state.tabs.len() > 1 {
            let ix = self.state.active_tab_index;
            self.split_id = Some(self.state.tabs[ix].id().to_string());
            self.state.active_tab_index = if ix == 0 { 1 } else { ix - 1 };
        } else {
            self.toast("Open another tab to split the view", false, cx);
        }
        cx.notify();
    }

    /// Clicking into the inactive pane makes its tab active; both tabs stay on their side
    pub(crate) fn swap_split(&mut self, cx: &mut Context<Self>) {
        let (Some(split), Some(active)) = (
            self.split_id.clone(),
            self.state.active_tab().map(|t| t.id().to_string()),
        ) else {
            return;
        };
        if let Some(ix) = self.state.tabs.iter().position(|t| t.id() == split) {
            self.state.active_tab_index = ix;
            self.split_id = Some(active);
            self.split_active_right = !self.split_active_right;
            cx.notify();
        }
    }

    pub(crate) fn confirm_command_bar_in_split(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.state.active_tab().map(|t| t.id().to_string());
        self.confirm_command_bar(window, cx);
        let opened = self.state.active_tab().map(|t| t.id().to_string());
        if let (Some(prev), Some(new)) = (previous, opened) {
            if prev != new {
                self.split_id = Some(new);
                self.split_active_right = false;
                if let Some(ix) = self.state.tabs.iter().position(|t| t.id() == prev) {
                    self.state.active_tab_index = ix;
                }
            }
        }
        cx.notify();
    }

    /// Pins the current table or query tab to its connection (or unpins it)
    pub(crate) fn pin_current_tab(&mut self, cx: &mut Context<Self>) {
        let pin = match self.state.active_tab() {
            Some(Tab::Table {
                schema_name,
                table_name,
                ..
            }) => pins::Pin::Table {
                schema: schema_name.clone(),
                table: table_name.clone(),
            },
            Some(Tab::Query { title, editor, .. }) => pins::Pin::Query {
                title: title.clone(),
                sql: editor.read(cx).value().to_string(),
            },
            None => return,
        };
        self.toggle_pin(pin, cx);
    }

    pub(crate) fn toggle_pin(&mut self, pin: pins::Pin, cx: &mut Context<Self>) {
        let Some(conn) = self.current_connection(cx).1.map(|c| c.id) else {
            return self.toast("Connect to a database to pin things to it", true, cx);
        };
        let label = pin.label().to_string();
        let pinned = pins::toggle(&mut self.state.pins, &conn, pin);
        pins::save(&self.state.pins);
        self.toast(
            format!("{} {}", if pinned { "Pinned" } else { "Unpinned" }, label),
            false,
            cx,
        );
    }

    pub(crate) fn open_pin(&mut self, pin: pins::Pin, window: &mut Window, cx: &mut Context<Self>) {
        match pin {
            pins::Pin::Table { schema, table } => {
                self.open_table_tab(&table, &schema, window, cx);
                self.refresh_active_tab(window, cx);
            }
            pins::Pin::Query { sql, title } => {
                self.open_query_tab(&sql, window, cx);
                if let Some(Tab::Query { title: t, .. }) = self.state.tabs.last_mut() {
                    *t = title;
                }
            }
        }
        cx.notify();
    }

    /// ⌃1–⌃9 (U5): jump to the Nth saved connection; the sidebar swaps to its tree and pins
    pub(crate) fn switch_space(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.state.saved_connections.get(n).map(|c| c.id.clone()) {
            self.connect_saved(id, window, cx);
        }
    }

    /// ⌘⇧P (P1): the command bar in command mode
    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.command_bar_open = true;
        self.state.usages = None;
        self.state.command_bar_selected_index = 0;
        self.command_input.update(cx, |input, cx| {
            input.set_value(">", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn reopen_closed_tab(
        &mut self,
        _: &ReopenClosedTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((tab, subs)) = self.closed_tabs.pop() else {
            return;
        };
        self.tab_subscriptions.extend(subs);
        self.state.tabs.push(tab);
        self.state.active_tab_index = self.state.tabs.len() - 1;
        self.save_session(cx);
        cx.notify();
    }
}
