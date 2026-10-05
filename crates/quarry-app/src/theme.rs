// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use gpui::*;

#[allow(dead_code)]
pub struct Theme {
    pub bg_app: Rgba,
    pub bg_sidebar: Rgba,
    pub bg_panel: Rgba,
    pub bg_surface: Rgba,
    pub bg_hover: Rgba,
    pub bg_selected: Rgba,
    pub border: Rgba,
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub null_token: Rgba,
    pub pending_cell_bg: Rgba,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg_app: rgb(0x0f0f11),
            bg_sidebar: rgb(0x141417),
            bg_panel: rgb(0x18181c),
            bg_surface: rgb(0x222228),
            bg_hover: rgb(0x2a2a32),
            bg_selected: rgb(0x2f303a),
            border: rgb(0x2d2d34),
            text_primary: rgb(0xf4f4f6),
            text_secondary: rgb(0xa1a1aa),
            text_muted: rgb(0x71717a),
            accent: rgb(0x3b82f6),
            accent_hover: rgb(0x60a5fa),
            success: rgb(0x10b981),
            warning: rgb(0xf59e0b),
            danger: rgb(0xef4444),
            null_token: rgb(0x71717a),
            pending_cell_bg: rgba(0xf59e0b25),
        }
    }
}
