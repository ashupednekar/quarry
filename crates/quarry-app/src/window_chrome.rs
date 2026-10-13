// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! The window frame around the tabs: title bar, tab strip and the centre area

use super::*;

impl QuarryWindow {
    /// Keeps derived state (theme, settings, lint, tab connections) in step before drawing
    pub(crate) fn sync_before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let changes = self
            .config_watcher
            .poll_changes(std::time::Duration::from_millis(500));
        if changes.settings_changed {
            self.reload_settings(cx);
        }
        if changes.keymap_changed {
            let _ = crate::settings::apply_keymap(cx);
        }

        self.sync_theme(window, cx);
        if self.applied_settings.as_ref() != Some(&self.settings) {
            self.apply_settings_to_views(window, cx);
        }
        // The schema changed (connect, switch, refresh): squiggles must reflect the new one
        self.load_schema_objects(window, cx);
        if let Some(id) = self.state.pending_compare.take() {
            self.compare_schema(&id, window, cx);
        }
        if self.state.linted_gen != self.state.catalog_gen {
            self.state.linted_gen = self.state.catalog_gen;
            let editors: Vec<_> = self
                .state
                .tabs
                .iter()
                .filter_map(|t| match t {
                    Tab::Query { editor, .. } => Some(editor.clone()),
                    _ => None,
                })
                .collect();
            let catalog = self.state.completion_catalog.clone();
            cx.defer(move |cx| {
                editors
                    .iter()
                    .for_each(|e| completion::lint_editor(e, &catalog, cx))
            });
        }
        self.sync_tab_connection(window, cx);
        self.reopen_pending_table_tabs(window, cx);
        self.track_tab_mru(window, cx);
    }

    /// Where you are, and the one box that does everything (⌘K)
    pub(crate) fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        // macOS: clear the traffic lights; elsewhere a normal gutter
        #[cfg(target_os = "macos")]
        let titlebar_left_pad = px(78.0);
        #[cfg(not(target_os = "macos"))]
        let titlebar_left_pad = px(12.0);
        h_flex()
            .h(px(38.0))
            .w_full()
            .pl(titlebar_left_pad)
            .pr_3()
            .bg(t.bg_sidebar)
            .border_b_1()
            .border_color(t.border)
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pr_3()
                    .children(self.render_breadcrumb()),
            )
            .child(
                h_flex()
                    .id("omnibox-trigger-btn")
                    .w(px(360.0))
                    .h(px(26.0))
                    .px_2p5()
                    .gap_2()
                    .rounded(px(6.0))
                    .bg(t.bg_panel)
                    .border_1()
                    .border_color(t.border)
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_command_bar(&ToggleCommandBar, window, cx)
                    }))
                    .child(
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(t.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Search tables, run SQL, or type > for commands"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(crate::ui::format_keys("⌘K")),
                    ),
            )
            .child(div().flex_1())
    }

    /// Where the active tab points, DataGrip-style: `quarry-db › postgres › public › tables › ▦ users`.
    /// Hover for the full address.
    fn render_breadcrumb(&self) -> Option<impl IntoElement> {
        let t = &self.theme;
        let (conn, schema, table) = match self.state.active_tab()? {
            Tab::Table {
                connection,
                schema_name,
                table_name,
                ..
            } => (
                connection.as_ref()?,
                Some(schema_name.clone()),
                Some(table_name.clone()),
            ),
            Tab::Query { schema, .. } => {
                (self.state.active_connection.as_ref()?, schema.clone(), None)
            }
        };
        let address = crate::connections::display_address(conn);
        // The database is the address's last part: `localhost:5432/postgres`, or a SQLite file name
        let database = address
            .rsplit(['/', '\\'])
            .next()
            .filter(|d| !d.is_empty() && *d != conn.name)
            .map(str::to_string);
        let mut parts: Vec<String> = vec![conn.name.clone()];
        parts.extend(database);
        parts.extend(schema);
        if table.is_some() {
            parts.push("tables".into());
        }
        let sep = || {
            Icon::new(IconName::ChevronRight)
                .xsmall()
                .text_color(t.text_muted)
                .flex_shrink_0()
        };
        let tip = address.clone();
        Some(
            h_flex()
                .id("breadcrumb")
                .items_center()
                .gap_1()
                .min_w_0()
                .overflow_hidden()
                .text_xs()
                .text_color(t.text_muted)
                .tooltip(move |window, cx| {
                    gpui::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .children(parts.into_iter().enumerate().flat_map(|(i, part)| {
                    let label = div().flex_shrink_0().child(part);
                    if i == 0 {
                        vec![label.into_any_element()]
                    } else {
                        vec![sep().into_any_element(), label.into_any_element()]
                    }
                }))
                .children(table.map(|name| {
                    h_flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .child(sep())
                        .child(
                            Icon::new(IconName::Table)
                                .xsmall()
                                .text_color(t.text_secondary),
                        )
                        .child(div().truncate().text_color(t.text_primary).child(name))
                })),
        )
    }

    /// Icon + title + close per tab; hidden when nothing is open
    pub(crate) fn render_tab_strip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = &self.theme;
        (!self.state.tabs.is_empty()).then(|| {
            h_flex()
                .h(px(BAR_H))
                .w_full()
                .px_1()
                .gap_px()
                .bg(t.bg_panel)
                .border_b_1()
                .border_color(t.border)
                .items_end()
                .children(self.state.tabs.iter().enumerate().map(|(idx, tab)| {
                    let is_active = idx == self.state.active_tab_index;
                    let icon = match tab {
                        Tab::Query { .. } => IconName::SquareTerminal,
                        Tab::Table { .. } => IconName::Table,
                    };
                    // Table tabs keep their connection: mark staging/prod so a risky tab is obvious at a glance
                    let env_color = match tab {
                        Tab::Table { connection: Some(c), .. } => match c.env {
                            Environment::Prod => Some(t.danger),
                            Environment::Staging => Some(t.warning),
                            Environment::Local => None,
                        },
                        _ => None,
                    };
                    let table_obj = match tab {
                        Tab::Table { schema_name, table_name, .. } => {
                            Some(object_menu::DbObject::Table { schema: schema_name.clone(), table: table_name.clone() })
                        }
                        _ => None,
                    };
                    h_flex()
                        .id(("tab-select", idx))
                        .h(px(CONTROL_H))
                        .pl_2p5()
                        .pr_1()
                        .gap_1p5()
                        .items_center()
                        .rounded_t(px(5.0))
                        .cursor_pointer()
                        .when(is_active, |d| d.bg(t.bg_app))
                        .when(!is_active, |d| d.hover(|s| s.bg(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.active_tab_index = idx;
                            cx.notify();
                        }))
                        .when_some(env_color, |d, c| d.border_b_2().border_color(c))
                        .when_some(table_obj, |d, obj| {
                            d.on_mouse_down(MouseButton::Right, cx.listener(move |this, _, window, cx| {
                                this.state.active_tab_index = idx;
                                this.open_object_menu(obj.clone(), window, cx);
                            }))
                        })
                        .child(Icon::new(icon).xsmall().text_color(if is_active { t.accent } else { t.text_muted }))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if is_active { t.text_primary } else { t.text_secondary })
                                .child(tab.title().to_string()),
                        )
                        .when(
                            // Unsaved: a new tab with text in it, or a file that differs from disk
                            matches!(tab, Tab::Query { editor, saved_text, .. } if editor.read(cx).value().as_ref() != saved_text.as_str()),
                            |d| d.child(div().size(px(6.0)).rounded_full().bg(t.text_secondary)),
                        )
                        .child(
                            zbutton(("tab-close", idx))
                                .icon(IconName::X)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    // The tab under this button selects itself on click; without this it would
                                    // re-select the closed slot (a neighbour, or nothing when it was the last)
                                    cx.stop_propagation();
                                    this.close_tab_at(idx, cx);
                                    this.refocus_root(window, cx);
                                })),
                        )
                }))
                // Tabs sit on the strip's bottom edge; centre the + on a tab's height so it lines up with them
                .child(
                    h_flex().h(px(CONTROL_H)).px_1().items_center().child(
                        zbutton("tab-add-btn")
                            .icon(IconName::Plus)
                            .ghost()
                            .xsmall()
                            .tooltip(crate::ui::format_keys("New query (⌘N)"))
                            .on_click(cx.listener(|this, _, window, cx| this.new_query(&NewQuery, window, cx))),
                    ),
                )
        })
    }

    pub(crate) fn render_center(
        &mut self,
        active_tab: Option<&Tab>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme.clone();
        match active_tab {
            None if self.state.session.is_none() => self.render_onboarding(cx),
            None => self.render_launcher(cx),
            Some(tab) => match self.split_tab().filter(|_| self.state.tabs.len() > 1) {
                // Split view (U8): the active tab on the left, the split tab on the right
                Some(split) => {
                    let left = self.render_tab_view(tab, cx);
                    let right = self.render_tab_view(&split, cx);
                    h_resizable("split-view")
                        .child(resizable_panel().child(left))
                        .child(
                            resizable_panel().child(
                                div()
                                    .size_full()
                                    .border_l_1()
                                    .border_color(t.border)
                                    // Working in the right pane makes it the active one
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.swap_split(cx)),
                                    )
                                    .child(right),
                            ),
                        )
                        .into_any_element()
                }
                None => self.render_tab_view(tab, cx),
            },
        }
    }
}
