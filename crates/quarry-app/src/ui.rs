// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

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
        .rounded(px(RADIUS_LG))
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
/// Dense controls inside bars: status bar, editor toolbar, inline buttons
pub const SMALL_H: f32 = 24.0;
pub const ROW_H: f32 = 28.0;
pub const CONTROL_H: f32 = 32.0;
/// Toolbars, pane headers, panel tab strips
pub const BAR_H: f32 = 36.0;
/// Corner radii: controls, popovers/menus, modal cards
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS_LG: f32 = 10.0;
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
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .child(title.into()),
                )
                .when_some(subtitle, |d, s| {
                    let tip = s.clone();
                    d.child(
                        div()
                            .id("modal-subtitle")
                            .text_xs()
                            .text_color(t.text_muted)
                            .truncate()
                            .tooltip(move |window, cx| {
                                gpui::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            })
                            .child(s),
                    )
                }),
        )
        .child(
            div()
                .id("modal-close")
                .p_1()
                .rounded(px(RADIUS_SM))
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
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> Div {
    let description: SharedString = description.into();
    h_flex()
        .py_3()
        .gap_6()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_primary)
                        .child(label.into()),
                )
                .when(!description.is_empty(), |d| {
                    d.child(div().text_xs().text_color(t.text_muted).child(description))
                }),
        )
        .child(div().flex_shrink_0().child(control))
}

/// Settings-style group: rows on one rounded surface, hairlines between them
pub fn settings_group(t: &crate::theme::Theme, rows: impl IntoIterator<Item = Div>) -> Div {
    v_flex()
        .px_4()
        .rounded(px(RADIUS_MD))
        .bg(t.bg_app)
        .border_1()
        .border_color(t.border)
        .children(
            rows.into_iter()
                .enumerate()
                .map(|(i, row)| row.when(i > 0, |d| d.border_t_1().border_color(t.border))),
        )
}

/// Form row in a settings group: label (and hint) left, a fixed-width control right.
/// `width` narrows the control (a port, a timeout) while keeping it right-aligned.
pub fn form_row(
    t: &crate::theme::Theme,
    label: impl Into<SharedString>,
    hint: Option<SharedString>,
    width: Option<f32>,
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
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_primary)
                        .child(label.into()),
                )
                .when_some(hint, |d, h| {
                    d.child(div().text_xs().text_color(t.text_muted).child(h))
                }),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .w(px(FORM_CONTROL_W))
                .justify_end()
                .child(div().w(px(width.unwrap_or(FORM_CONTROL_W))).child(control)),
        )
}

/// Width of a text control in a form row
pub const FORM_CONTROL_W: f32 = 300.0;

/// One option in a segmented choice; the caller adds `.on_click`
pub fn chip(
    t: &crate::theme::Theme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(ROW_H))
        .px_3()
        .flex()
        .items_center()
        .rounded(px(RADIUS_SM))
        .text_sm()
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

/// Chips on a bordered track, read as one segmented control
pub fn segmented(t: &crate::theme::Theme) -> Div {
    chip_group()
        .flex_nowrap()
        .p_0p5()
        .rounded(px(RADIUS_MD))
        .bg(t.bg_sidebar)
        .border_1()
        .border_color(t.border)
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
        .h(px(BAR_H))
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
    /// 32px tall, 16px sides: modal and form actions
    fn pad_sm(self) -> Self {
        self.h(px(CONTROL_H)).px_4()
    }
    /// 28px tall, 12px sides: toolbars and dense bars
    fn pad_xs(self) -> Self {
        self.h(px(ROW_H)).px_3()
    }
    /// 24px tall, 8px sides: inside 28px bars and narrow side panels
    fn pad_xxs(self) -> Self {
        self.h(px(SMALL_H)).px_2()
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

/// Filter / search box for toolbars: one height (32px) and look across the app
pub fn search_input(
    state: &Entity<gpui::component::input::InputState>,
    icon: gpui::assets::IconName,
) -> gpui::component::input::Input {
    gpui::component::input::Input::new(state)
        .text_sm()
        .cleanable(true)
        .prefix(gpui::component::Sizable::small(gpui::component::Icon::new(
            icon,
        )))
}

/// Translates keyboard shortcut symbols based on the operating system.
/// On macOS: preserves standard symbols (⌘, ⌥, ⇧, ⌃, ↵, ⌫, ⎋).
/// On Windows and Linux: maps them to conventional representations (Ctrl+, Alt+, Shift+, Enter, Backspace, Esc).
pub fn format_keys(s: impl AsRef<str>) -> String {
    format_keys_for_os(s.as_ref(), cfg!(target_os = "macos"))
}

fn format_keys_for_os(s: &str, is_macos: bool) -> String {
    if is_macos {
        return s.to_string();
    }

    let mut result = s.to_string();
    result = result.replace("⇧⌥⌘", "Ctrl+Alt+Shift+");
    result = result.replace("⌥⇧⌘", "Ctrl+Alt+Shift+");
    result = result.replace("⇧⌥", "Alt+Shift+");
    result = result.replace("⌥⌘", "Ctrl+Alt+");
    result = result.replace("⇧⌘", "Ctrl+Shift+");
    result = result.replace("⌘ ", "Ctrl ");
    result = result.replace('⌘', "Ctrl+");
    result = result.replace('⌥', "Alt+");
    result = result.replace('⇧', "Shift+");
    result = result.replace('⌃', "Ctrl+");
    result = result.replace('↵', "Enter");
    result = result.replace('⌫', "Backspace");
    result = result.replace('⎋', "Esc");
    result
}

pub struct DragGhost {
    label: SharedString,
}

impl DragGhost {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::component::ActiveTheme;
        let t = cx.theme();
        div()
            .px_3()
            .py_1p5()
            .rounded(px(RADIUS_MD))
            .bg(t.muted)
            .text_color(t.foreground)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.label.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::format_keys_for_os;

    #[test]
    fn formats_keys_differently_per_os() {
        assert_eq!(format_keys_for_os("⌘N", true), "⌘N");
        assert_eq!(format_keys_for_os("⌘N", false), "Ctrl+N");
        assert_eq!(format_keys_for_os("⇧⌘T", false), "Ctrl+Shift+T");
        assert_eq!(format_keys_for_os("⌥⌘E", false), "Ctrl+Alt+E");
        assert_eq!(format_keys_for_os("⇧⌥⌘E", false), "Ctrl+Alt+Shift+E");
        assert_eq!(
            format_keys_for_os("Run statement (⌘↵)", false),
            "Run statement (Ctrl+Enter)"
        );
        assert_eq!(
            format_keys_for_os("Value editor (⇧↵)", false),
            "Value editor (Shift+Enter)"
        );
        assert_eq!(
            format_keys_for_os("Delete row (⌘⌫)", false),
            "Delete row (Ctrl+Backspace)"
        );
        assert_eq!(
            format_keys_for_os("Maximize (⇧⎋)", false),
            "Maximize (Shift+Esc)"
        );
    }
}

/// macOS System Settings › Accessibility › Display › Reduce motion
pub fn os_reduce_motion() -> bool {
    cfg!(target_os = "macos")
        && std::process::Command::new("defaults")
            .args(["read", "com.apple.universalaccess", "reduceMotion"])
            .output()
            .is_ok_and(|o| o.stdout.starts_with(b"1"))
}

/// Modals ease in (fade plus a small rise) the first frames they're shown; closing is instant
pub fn fade_in(id: &'static str, el: impl IntoElement + 'static) -> impl IntoElement {
    div().absolute().inset_0().child(el).with_animation(
        id,
        Animation::new(std::time::Duration::from_millis(140)).with_easing(ease_out_quint()),
        |d, t| d.opacity(t).top(px(4.0 * (1.0 - t))),
    )
}
