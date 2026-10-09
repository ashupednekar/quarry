// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Pane sizes that survive a restart: each resizable group saves its panel sizes when dragged.

use gpui::component::resizable::ResizableState;
use gpui::*;
use std::collections::HashMap;
use std::sync::Mutex;

static SIZES: Mutex<Option<HashMap<String, Vec<f32>>>> = Mutex::new(None);

fn path() -> Option<std::path::PathBuf> {
    Some(crate::connections::config_dir()?.join("layout.json"))
}

fn with<R>(f: impl FnOnce(&mut HashMap<String, Vec<f32>>) -> R) -> R {
    let mut guard = SIZES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    });
    f(map)
}

/// The saved size of panel `ix` in group `group`, or `default`
pub fn size(group: &str, ix: usize, default: f32) -> Pixels {
    px(with(|m| m.get(group).and_then(|s| s.get(ix)).copied().filter(|v| *v > 0.0)).unwrap_or(default))
}

/// Pass to `ResizablePanelGroup::on_resize`: remembers the group's sizes
pub fn remember(group: &'static str) -> impl Fn(&Entity<ResizableState>, &mut Window, &mut App) + 'static {
    move |state, _, cx| {
        let sizes: Vec<f32> = state.read(cx).sizes().iter().map(|p| f32::from(*p)).collect();
        // A hidden panel reports 0; keep its last real size
        let json = with(|m| {
            let entry = m.entry(group.to_string()).or_default();
            entry.resize(sizes.len().max(entry.len()), 0.0);
            for (old, new) in entry.iter_mut().zip(&sizes) {
                if *new > 0.0 {
                    *old = *new;
                }
            }
            serde_json::to_string_pretty(m).ok()
        });
        if let (Some(p), Some(json)) = (path(), json) {
            let _ = std::fs::write(p, json);
        }
    }
}
