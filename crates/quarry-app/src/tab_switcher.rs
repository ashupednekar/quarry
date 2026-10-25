// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! ⌃Tab switcher: hold ⌃, press Tab to walk tabs in most-recently-used order, let go of ⌃ to switch.

use crate::state::Tab;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::{Icon, Sizable};
use gpui::*;

/// Moves `active` to the front of the MRU list and drops ids of closed tabs
pub fn touch(mru: &mut Vec<String>, active: Option<&str>, open: &[&str]) {
    mru.retain(|id| open.contains(&id.as_str()) && Some(id.as_str()) != active);
    if let Some(a) = active {
        mru.insert(0, a.to_string());
    }
    // Tabs never activated yet go last, in strip order
    for id in open {
        if !mru.iter().any(|m| m == id) {
            mru.push(id.to_string());
        }
    }
}

impl QuarryWindow {
    pub fn track_tab_mru(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Not while the switcher is open: the list would reorder under the highlight
        if self.tab_switcher.is_some() {
            return;
        }
        let open: Vec<&str> = self.state.tabs.iter().map(|t| t.id()).collect();
        let active = self.state.active_tab().map(|t| t.id());
        let changed = active.is_some() && self.tab_mru.first().map(|s| s.as_str()) != active;
        touch(&mut self.tab_mru, active, &open);
        // A tab switch (⌃Tab, click, ⌘W…) hides the old tab's editor; if it kept focus, keys would go nowhere
        if changed {
            match self.state.active_tab() {
                Some(Tab::Query { editor, .. }) => {
                    editor.clone().update(cx, |e, cx| e.focus(window, cx))
                }
                _ => self.refocus_root(window, cx),
            }
        }
    }

    /// ⌃Tab / ⌃⇧Tab: open the switcher on the previous tab, or move the highlight
    pub fn step_tab_switcher(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.tab_mru.len();
        if n < 2 {
            return;
        }
        let next = match self.tab_switcher {
            None if forward => 1,
            None => n - 1,
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
        };
        self.tab_switcher = Some(next);
        cx.notify();
    }

    /// Letting go of ⌃ switches to the highlighted tab
    pub fn commit_tab_switcher(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.tab_switcher.take() else {
            return;
        };
        if let Some(ix) = self
            .tab_mru
            .get(i)
            .and_then(|id| self.state.tabs.iter().position(|t| t.id() == id))
        {
            self.state.active_tab_index = ix;
        }
        cx.notify();
    }

    pub fn render_tab_switcher(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sel = self.tab_switcher?;
        let t = &self.theme;
        let rows = self.tab_mru.iter().enumerate().filter_map(|(i, id)| {
            let tab = self.state.tabs.iter().find(|tab| tab.id() == id)?;
            let (icon, detail) = match tab {
                Tab::Query {
                    connection_id,
                    file,
                    ..
                } => (
                    IconName::SquareTerminal,
                    file.as_ref().map(|f| f.display().to_string()).or_else(|| {
                        let id = connection_id.as_ref()?;
                        self.state
                            .saved_connections
                            .iter()
                            .find(|c| &c.id == id)
                            .map(|c| c.name.clone())
                    }),
                ),
                Tab::Table { schema_name, .. } => (IconName::Table, Some(schema_name.clone())),
            };
            let on = i == sel;
            Some(
                h_flex()
                    .id(("switcher-row", i))
                    .h(px(crate::ui::ROW_H))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .rounded(px(crate::ui::RADIUS_SM))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.bg_selected))
                    .hover(|s| s.bg(t.bg_hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tab_switcher = Some(i);
                        this.commit_tab_switcher(window, cx);
                    }))
                    .child(Icon::new(icon).xsmall().text_color(if on {
                        t.accent
                    } else {
                        t.text_muted
                    }))
                    .child(
                        div()
                            .text_sm()
                            .text_color(if on { t.text_primary } else { t.text_secondary })
                            .child(tab.title().to_string()),
                    )
                    .children(detail.map(|d| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(t.text_muted)
                            .truncate()
                            .child(d)
                    })),
            )
        });
        Some(
            crate::ui::fade_in(
                "tab-switcher-fade",
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(120.0))
                    .child(
                        v_flex()
                            .id("tab-switcher")
                            .w(px(420.0))
                            .max_h(px(420.0))
                            .overflow_y_scroll()
                            .p_1()
                            .gap_px()
                            .bg(t.bg_panel)
                            .border_1()
                            .border_color(t.border)
                            .rounded(px(crate::ui::RADIUS_LG))
                            .shadow_lg()
                            .children(rows),
                    ),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::touch;

    #[test]
    fn most_recent_first_and_closed_dropped() {
        let mut mru = vec![];
        touch(&mut mru, Some("a"), &["a", "b", "c"]);
        assert_eq!(mru, ["a", "b", "c"]);
        touch(&mut mru, Some("c"), &["a", "b", "c"]);
        assert_eq!(mru, ["c", "a", "b"]);
        touch(&mut mru, Some("b"), &["b", "c"]);
        assert_eq!(mru, ["b", "c"]);
    }
}
