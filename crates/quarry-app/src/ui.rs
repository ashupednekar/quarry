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
