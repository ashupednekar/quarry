// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Blurs the desktop behind the window (Settings → Window translucency).
//!
//! macOS: gpui's own `Blurred` background strips layers off its NSVisualEffectView to drop the
//! system tint, and on recent macOS that leaves plain transparency. We ask gpui for a merely
//! transparent window and put a stock NSVisualEffectView behind its content instead.
//! Elsewhere gpui's `Blurred` is used as is (real blur on Windows 11 and KDE Wayland).

use gpui::{Window, WindowBackgroundAppearance};

pub fn set(window: &Window, on: bool) {
    #[cfg(target_os = "macos")]
    {
        window.set_background_appearance(if on {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        });
        macos::set(window, on);
    }
    #[cfg(not(target_os = "macos"))]
    window.set_background_appearance(if on {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    });
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSAutoresizingMaskOptions, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        /// The blur view per window, keyed by gpui's view, so it can be taken out again
        static VIEWS: RefCell<HashMap<usize, Retained<NSVisualEffectView>>> = RefCell::default();
    }

    pub fn set(window: &gpui::Window, on: bool) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // gpui's view is a subview of the window's content view; find that content view by
        // matching addresses (no raw-pointer casts) and put the blur beneath gpui's view in it
        let key = h.ns_view.as_ptr() as usize;
        let Some((content, gpui_view)) = NSApplication::sharedApplication(mtm)
            .windows()
            .iter()
            .filter_map(|w| w.contentView())
            .find_map(|c| {
                let v = c
                    .subviews()
                    .iter()
                    .find(|v| Retained::as_ptr(v) as usize == key)?;
                Some((c, v))
            })
        else {
            tracing::warn!("window blur: couldn't find the window's view");
            return;
        };
        VIEWS.with_borrow_mut(|views| {
            if let Some(old) = views.remove(&key) {
                old.removeFromSuperview();
            }
            if !on {
                return;
            }
            let fx =
                NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.bounds());
            fx.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
            fx.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            fx.setState(NSVisualEffectState::Active);
            fx.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            content.addSubview_positioned_relativeTo(
                &fx,
                NSWindowOrderingMode::Below,
                Some(&gpui_view),
            );
            views.insert(key, fx);
        });
    }
}
