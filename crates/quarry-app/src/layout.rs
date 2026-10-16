// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Pane sizes and dock positions that survive a restart.

use gpui::component::resizable::ResizableState;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dock {
    #[serde(default = "default_sidebar")]
    pub sidebar: Side, // Left | Right
    #[serde(default = "default_bottom_panel")]
    pub bottom_panel: Side, // Bottom | Right
}

fn default_sidebar() -> Side {
    Side::Left
}

fn default_bottom_panel() -> Side {
    Side::Bottom
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            sidebar: Side::Left,
            bottom_panel: Side::Bottom,
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDirection {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Sidebar,
    BottomPanel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DraggedPane(pub Pane);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DraggedTab(pub usize);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DraggedSplitPane(pub usize);

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct LayoutStore {
    #[serde(default)]
    pub dock: Dock,
    #[serde(default)]
    pub split_direction: SplitDirection,
    #[serde(default)]
    pub sizes: HashMap<String, Vec<f32>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawLayout {
    New(LayoutStore),
    Old(HashMap<String, Vec<f32>>),
}

static STORE: Mutex<Option<LayoutStore>> = Mutex::new(None);

fn path() -> Option<std::path::PathBuf> {
    Some(crate::connections::config_dir()?.join("layout.json"))
}

fn with<R>(f: impl FnOnce(&mut LayoutStore) -> R) -> R {
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let store = guard.get_or_insert_with(|| {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| match serde_json::from_str::<RawLayout>(&s) {
                Ok(RawLayout::New(n)) => Some(n),
                Ok(RawLayout::Old(sizes)) => Some(LayoutStore {
                    dock: Dock::default(),
                    split_direction: SplitDirection::default(),
                    sizes,
                }),
                Err(_) => None,
            })
            .unwrap_or_default()
    });
    f(store)
}

fn save(store: &LayoutStore) {
    if let (Some(p), Ok(json)) = (path(), serde_json::to_string_pretty(store)) {
        let _ = std::fs::write(p, json);
    }
}

pub fn dock() -> Dock {
    with(|s| s.dock)
}

pub fn set_dock(dock: Dock) {
    with(|s| {
        s.dock = dock;
        save(s);
    });
}

pub fn split_direction() -> SplitDirection {
    with(|s| s.split_direction)
}

pub fn set_split_direction(direction: SplitDirection) {
    with(|s| {
        s.split_direction = direction;
        save(s);
    });
}

pub fn reset() {
    with(|s| {
        s.dock = Dock::default();
        s.split_direction = SplitDirection::Vertical;
        s.sizes.clear();
        save(s);
    });
}

pub fn get_size(group: &str, ix: usize) -> Option<Pixels> {
    with(|m| {
        m.sizes
            .get(group)
            .and_then(|s| s.get(ix))
            .copied()
            .filter(|v| *v > 0.0)
            .or_else(|| {
                if group == "split-view:vertical" {
                    m.sizes
                        .get("split-view")
                        .and_then(|s| s.get(ix))
                        .copied()
                        .filter(|v| *v > 0.0)
                } else {
                    None
                }
            })
            .map(px)
    })
}

/// The saved size of panel `ix` in group `group`, or `default`
pub fn size(group: &str, ix: usize, default: f32) -> Pixels {
    px(with(|m| {
        m.sizes
            .get(group)
            .and_then(|s| s.get(ix))
            .copied()
            .filter(|v| *v > 0.0)
            .or_else(|| {
                if group == "main-split:sidebar-left" && ix == 0 {
                    m.sizes.get("main-split").and_then(|s| s.first()).copied()
                } else if group == "main-split:sidebar-right" && ix == 1 {
                    m.sizes.get("main-split").and_then(|s| s.first()).copied()
                } else if (group == "content-split:bottom" || group == "content-split:right")
                    && ix == 1
                {
                    m.sizes.get("content-split").and_then(|s| s.get(1)).copied()
                } else if group == "split-view:vertical" {
                    m.sizes.get("split-view").and_then(|s| s.get(ix)).copied()
                } else {
                    None
                }
            })
            .filter(|v| *v > 0.0)
    })
    .unwrap_or(default))
}

/// Pass to `ResizablePanelGroup::on_resize`: remembers the group's sizes
pub fn remember(
    group: &'static str,
) -> impl Fn(&Entity<ResizableState>, &mut Window, &mut App) + 'static {
    move |state, _, cx| {
        let sizes: Vec<f32> = state
            .read(cx)
            .sizes()
            .iter()
            .map(|p| f32::from(*p))
            .collect();
        with(|store| {
            let entry = store.sizes.entry(group.to_string()).or_default();
            entry.resize(sizes.len().max(entry.len()), 0.0);
            for (old, new) in entry.iter_mut().zip(&sizes) {
                if *new > 0.0 {
                    *old = *new;
                }
            }
            save(store);
        });
    }
}
