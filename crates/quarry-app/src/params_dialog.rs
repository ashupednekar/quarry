// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Parameter prompt (Q7): fills `:name` / `$1` placeholders before a query runs.

use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::{ActiveTheme, Sizable};
use gpui::*;
use std::collections::HashMap;

pub struct ParamsDialog {
    statements: Vec<String>,
    inputs: Vec<(String, Entity<InputState>)>,
    _subs: Vec<Subscription>,
}

impl QuarryWindow {
    pub fn open_params(&mut self, statements: Vec<String>, names: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let inputs: Vec<(String, Entity<InputState>)> = names
            .into_iter()
            .map(|name| {
                let remembered = self.param_memory.get(&name).cloned().unwrap_or_default();
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("value, or NULL").default_value(remembered));
                (name, input)
            })
            .collect();
        // Enter in any field runs the query
        let subs = inputs
            .iter()
            .map(|(_, input)| {
                cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.run_with_params(window, cx);
                    }
                })
            })
            .collect();
        if let Some((_, first)) = inputs.first() {
            first.update(cx, |i, cx| i.focus(window, cx));
        }
        self.params = Some(ParamsDialog { statements, inputs, _subs: subs });
        cx.notify();
    }

    fn run_with_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.params.take() else { return };
        let values: HashMap<String, String> = dialog.inputs.iter().map(|(n, i)| (n.clone(), i.read(cx).value().to_string())).collect();
        self.param_memory.extend(values.clone());
        let bound = dialog.statements.iter().map(|s| quarry_sql::bind_params(s, &values)).collect();
        self.refocus_root(window, cx);
        self.execute_statements(bound, window, cx);
    }

    pub fn render_params_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.params.as_ref()?;
        let t = &self.theme;
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(rgba(0x00000060))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(120.0))
                .child(
                    v_flex()
                        .id("params-card")
                        .w(px(420.0))
                        .p_4()
                        .gap_3()
                        .rounded(px(8.0))
                        .bg(t.bg_sidebar)
                        .border_1()
                        .border_color(t.border)
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text_primary).child("Query parameters"))
                        .children(dialog.inputs.iter().map(|(name, input)| {
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().w(px(110.0)).text_xs().font_family(cx.theme().mono_font_family.clone()).text_color(t.text_secondary).child(name.clone()))
                                .child(div().flex_1().child(Input::new(input).xsmall()))
                        }))
                        .child(div().text_xs().text_color(t.text_muted).child("Numbers and true/false go in as-is; anything else is quoted."))
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(Button::new("params-cancel").label("Cancel").ghost().small().on_click(cx.listener(|this, _, window, cx| {
                                    this.params = None;
                                    this.refocus_root(window, cx);
                                    cx.notify();
                                })))
                                .child(Button::new("params-run").label("Run").primary().small().on_click(cx.listener(|this, _, window, cx| {
                                    this.run_with_params(window, cx)
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }
}
