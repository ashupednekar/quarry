// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

pub use gpui::prelude::*;
use gpui::*;

#[inline]
pub fn h_flex() -> Div {
    div().flex().flex_row()
}

#[inline]
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

/// One focus handle for whichever modal is open (there's only ever one)
struct ModalFocus(FocusHandle);
impl Global for ModalFocus {}

pub fn init_modal_focus(cx: &mut App) {
    let focus = cx.focus_handle();
    cx.set_global(ModalFocus(focus));
}

/// Dimmed layer behind every modal: blocks the UI underneath, clicking it dismisses.
/// It also traps focus: Tab / ⇧Tab cycle through the modal's controls instead of wandering into
/// the window behind, and focus moves into the modal when it opens.
pub fn modal_backdrop(
    id: &'static str,
    on_dismiss: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let focus = cx.global::<ModalFocus>().0.clone();
    let (trap, claim) = (focus.clone(), focus.clone());
    div()
        .id(id)
        .absolute()
        .inset_0()
        .occlude()
        .track_focus(&focus)
        .capture_key_down(move |e: &KeyDownEvent, window, cx| {
            let m = &e.keystroke.modifiers;
            if e.keystroke.key != "tab" || m.platform || m.control || m.alt {
                return;
            }
            cx.stop_propagation();
            // Step through the window's tab stops until one is inside the modal again
            let start = window.focused(cx);
            for _ in 0..256 {
                if m.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                let now = window.focused(cx);
                if (trap.contains_focused(window, cx) && now.as_ref() != Some(&trap))
                    || now == start
                {
                    break;
                }
            }
        })
        // On open, pull focus in unless the modal already focused one of its own fields
        .child(
            canvas(
                move |_, window, cx| {
                    // Only while focus is outside: once inside, nothing is scheduled
                    if claim.contains_focused(window, cx) {
                        return;
                    }
                    let claim = claim.clone();
                    window.on_next_frame(move |window, cx| {
                        if !claim.contains_focused(window, cx) {
                            claim.focus(window, cx);
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0(),
        )
        // GPUI can't blur what's behind an element; a deep translucent dim plus the card's soft
        // shadow gives the frosted, lifted look instead
        .bg(rgba(0x0a0b0d9c))
        .flex()
        .justify_center()
        .items_center()
        .p_6()
        .on_click(on_dismiss)
}

/// The card a modal sits in; swallows clicks so they don't reach the backdrop
pub fn modal_card(id: &'static str, width: f32, t: &crate::theme::Theme) -> Stateful<Div> {
    v_flex()
        .id(id)
        .w(px(width))
        .max_w_full()
        .bg(t.bg_sidebar)
        .border_1()
        .border_color(t.border)
        .rounded(px(10.0))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.45),
            offset: point(px(0.), px(24.)),
            blur_radius: px(64.),
            spread_radius: px(0.),
            inset: false,
        }])
        .on_click(|_, _, cx| cx.stop_propagation())
}

/// Backdrop handler for modals that close through the global Escape action
pub fn dismiss_on_click(_: &ClickEvent, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(crate::Dismiss), cx);
}

/// Spacing scale: every pane bar, row and modal edge uses one of these
pub const ROW_H: f32 = 28.0;
pub const CONTROL_H: f32 = 32.0;
/// Toolbars, pane headers, panel tab strips
pub const BAR_H: f32 = 36.0;
/// Modal header and footer
pub const MODAL_BAR_H: f32 = 48.0;

/// Modal title row: title (+ optional subtitle) left, close right
pub fn modal_header(
    t: &crate::theme::Theme,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    h_flex()
        .h(px(MODAL_BAR_H + 8.0))
        .px_6()
        .gap_3()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .child(title.into()),
                )
                .when_some(subtitle, |d, s| {
                    d.child(div().text_xs().text_color(t.text_muted).truncate().child(s))
                }),
        )
        .child(
            div()
                .id("modal-close")
                .p_1()
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(t.bg_hover))
                .on_click(on_close)
                .child(
                    gpui::component::Icon::new(gpui::assets::IconName::X).text_color(t.text_muted),
                ),
        )
}

/// Modal action row: secondary things left, the primary action last on the right
pub fn modal_footer(t: &crate::theme::Theme) -> Div {
    h_flex()
        .h(px(MODAL_BAR_H + 8.0))
        .px_6()
        .gap_2()
        .items_center()
        .justify_end()
        .flex_shrink_0()
        .border_t_1()
        .border_color(t.border)
}

/// The scrolling middle of a modal, on the 16/24 rhythm
pub fn modal_body(id: &'static str) -> Stateful<Div> {
    v_flex()
        .id(id)
        .px_6()
        .pt_3()
        .pb_6()
        .gap_6()
        .overflow_y_scroll()
        .min_h_0()
}

/// A label above its control, with an optional hint under it
pub fn field(
    t: &crate::theme::Theme,
    label: impl Into<SharedString>,
    hint: Option<&'static str>,
    control: impl IntoElement,
) -> Div {
    v_flex()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_secondary)
                .child(label.into()),
        )
        .child(control)
        .when_some(hint, |d, h| {
            d.child(div().text_xs().text_color(t.text_muted).child(h))
        })
}

/// Small caps-free section heading inside a modal
pub fn section_title(t: &crate::theme::Theme, title: &'static str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_muted)
        .child(title)
}

/// Settings row: what it is (and why) on the left, the control on the right
pub fn setting_row(
    t: &crate::theme::Theme,
    label: &'static str,
    description: &'static str,
    control: impl IntoElement,
) -> Div {
    h_flex()
        .py_3()
        .gap_6()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(div().text_sm().text_color(t.text_primary).child(label))
                .child(div().text_xs().text_color(t.text_muted).child(description)),
        )
        .child(div().flex_shrink_0().child(control))
}

/// One option in a segmented choice; the caller adds `.on_click`
pub fn chip(
    t: &crate::theme::Theme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .px_3()
        .flex()
        .items_center()
        .rounded(px(5.0))
        .text_xs()
        .cursor_pointer()
        .when(on, |d| d.bg(t.bg_selected).text_color(t.text_primary))
        .when(!on, |d| {
            d.text_color(t.text_secondary).hover(|s| s.bg(t.bg_hover))
        })
        .child(label.into())
}

/// Chips on one quiet track
pub fn chip_group() -> Div {
    h_flex().gap_0p5().flex_wrap()
}

/// Underline tabs (connection modal: General / SSH / Advanced); the caller adds `.on_click`
pub fn underline_tab(
    t: &crate::theme::Theme,
    id: impl Into<ElementId>,
    label: &'static str,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(36.0))
        .flex()
        .items_center()
        .text_sm()
        .cursor_pointer()
        .border_b_2()
        .border_color(if on {
            t.accent
        } else {
            gpui::transparent_black().into()
        })
        .text_color(if on { t.text_primary } else { t.text_muted })
        .when(!on, |d| d.hover(|s| s.text_color(t.text_secondary)))
        .child(label)
}

/// Zed-style button: no border, the theme's soft corners. Labelled ones add `.pad_sm()` / `.pad_xs()`.
pub fn zbutton(id: impl Into<ElementId>) -> gpui::component::button::Button {
    gpui::component::button::Button::new(id)
        .border_0()
        .cursor_pointer()
}

/// Roomier padding than gpui-kit's defaults (24px / 8px), for buttons with a label
pub trait ButtonPadding: Styled + Sized {
    /// 28px tall, 12px sides: modal and form actions
    fn pad_sm(self) -> Self {
        self.h(px(32.0)).px_4()
    }
    /// 24px tall, 10px sides: toolbars and dense bars
    fn pad_xs(self) -> Self {
        self.h(px(28.0)).px_3()
    }
}

impl ButtonPadding for gpui::component::button::Button {}

/// Form text input at a comfortable height; Input ignores `.h()`, so height comes from the large preset (44px)
pub fn form_input(state: &Entity<gpui::component::input::InputState>) -> Div {
    use gpui::component::Sizable;
    div().child(
        gpui::component::input::Input::new(state)
            .large()
            .text_sm()
            .px_3p5(),
    )
}
