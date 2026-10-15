// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::crash_reporter::CrashReport;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::{Icon, Sizable};
use gpui::*;

pub struct CrashModal;

impl CrashModal {
    pub fn render(
        w: &QuarryWindow,
        report: &CrashReport,
        cx: &Context<QuarryWindow>,
    ) -> impl IntoElement {
        let theme = &w.theme;
        let loc = report.location.as_deref().unwrap_or("unknown location");
        let msg = &report.panic_message;
        let report_for_copy = report.clone();
        let report_for_gh = report.clone();

        crate::ui::fade_in("crash-modal-backdrop-fade", modal_backdrop(
            "crash-modal-backdrop",
            cx.listener(|this, _, _, cx| this.dismiss_crash_report(cx)),
            cx,
        )
        .child(
            modal_card("crash-modal-card", 620.0, theme)
                .max_h(px(520.0))
                .overflow_hidden()
                .child(modal_header(
                    theme,
                    "Quarry Closed Unexpectedly",
                    Some("A crash report was recorded from your previous session.".into()),
                    cx.listener(|this, _, _, cx| this.dismiss_crash_report(cx)),
                ))
                .child(
                    modal_body("crash-modal-body")
                        .flex_1()
                        .gap_3()
                        .child(
                            v_flex()
                                .gap_1p5()
                                .p_3()
                                .rounded_md()
                                .bg(theme.bg_surface)
                                .border_1()
                                .border_color(theme.border)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(16.0))
                                                .text_color(theme.danger),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme.danger)
                                                .child(msg.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(crate::theme::MONO_FONT)
                                        .text_color(theme.text_secondary)
                                        .child(format!("Location: {}", loc)),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_secondary)
                                        .child("Stack Backtrace Excerpt:"),
                                )
                                .child(
                                    div()
                                        .id("crash-backtrace-excerpt")
                                        .p_2p5()
                                        .rounded_md()
                                        .bg(theme.bg_app)
                                        .border_1()
                                        .border_color(theme.border)
                                        .max_h(px(160.0))
                                        .overflow_y_scroll()
                                        .font_family(crate::theme::MONO_FONT)
                                        .text_sm()
                                        .text_color(theme.text_muted)
                                        .child(
                                            report
                                                .backtrace
                                                .lines()
                                                .take(30)
                                                .collect::<Vec<_>>()
                                                .join("\n"),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_muted)
                                .child("Would you like to file an issue on GitHub? You can also copy the complete report to your clipboard."),
                        ),
                )
                .child(
                    modal_footer(theme)
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    zbutton("btn-crash-copy")
                                        .icon(IconName::Copy)
                                        .label("Copy Report")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let md = crate::crash_reporter::format_markdown_report(
                                                &report_for_copy,
                                            );
                                            cx.write_to_clipboard(ClipboardItem::new_string(md));
                                            this.toast("Crash report copied to clipboard", false, cx);
                                        })),
                                )
                                .child(
                                    zbutton("btn-crash-github")
                                        .icon(IconName::Github)
                                        .label("Report on GitHub")
                                        .primary()
                                        .small()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let url = crate::crash_reporter::format_github_issue_url(
                                                &report_for_gh,
                                            );
                                            cx.open_url(&url);
                                            this.dismiss_crash_report(cx);
                                        })),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    zbutton("btn-crash-clear")
                                        .label("Clear Session")
                                        .ghost()
                                        .small()
                                        .tooltip("Clear previous session tabs (Safe Mode)")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.clear_saved_session(cx);
                                            this.dismiss_crash_report(cx);
                                        })),
                                )
                                .child(
                                    zbutton("btn-crash-dismiss")
                                        .label("Dismiss")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.dismiss_crash_report(cx);
                                        })),
                                ),
                        ),
                ),
        ))
    }
}
