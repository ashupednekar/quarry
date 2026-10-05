// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Settings and keymap as human-editable JSON (P2-P4) in ~/.config/quarry.

use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "system", "light" or "dark"
    pub theme: String,
    /// UI and editor text size in points
    pub font_size: f32,
    /// Font for SQL and data cells; empty keeps the default
    pub mono_font_family: String,
    /// Rows fetched per page in table tabs
    pub rows_per_page: usize,
    /// "auto" or "manual" commit for new connections
    pub default_transaction_mode: String,
    /// Ask before running statements that change data on PROD connections, and always review submits there
    pub confirm_on_prod: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            font_size: 14.0,
            mono_font_family: String::new(),
            rows_per_page: 500,
            default_transaction_mode: "auto".into(),
            confirm_on_prod: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeymapEntry {
    /// e.g. "secondary-shift-r"
    pub key: String,
    /// e.g. "quarry::RunAll" or "grid::AddRow"
    pub action: String,
    /// e.g. "DataTable"; omit for everywhere
    #[serde(default)]
    pub context: Option<String>,
}

pub fn settings_path() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("settings.json"))
}

pub fn keymap_path() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("keymap.json"))
}

/// Loads settings; a broken file falls back to defaults and reports why
pub fn load() -> (Settings, Option<String>) {
    let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return (Settings::default(), None);
    };
    match serde_json::from_str(&text) {
        Ok(s) => (s, None),
        Err(e) => (Settings::default(), Some(format!("settings.json: {e}"))),
    }
}

/// Writes a starter file the first time it's opened, so there's something to edit
pub fn ensure_file(path: &PathBuf, starter: &str) {
    if !path.exists() {
        let _ = std::fs::create_dir_all(path.parent().expect("has parent"));
        let _ = std::fs::write(path, starter);
    }
}

pub fn starter_settings() -> String {
    serde_json::to_string_pretty(&Settings::default()).unwrap_or_default() + "\n"
}

pub const STARTER_KEYMAP: &str = r#"[
  { "key": "secondary-shift-r", "action": "quarry::RunAll" },
  { "key": "secondary-y", "action": "grid::DuplicateRow", "context": "DataTable" }
]
"#;

/// Adds the user's bindings on top of the defaults (later bindings win). Returns problems found.
pub fn apply_keymap(cx: &mut App) -> Vec<String> {
    let Some(text) = keymap_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    let entries: Vec<KeymapEntry> = match serde_json::from_str(&text) {
        Ok(e) => e,
        Err(e) => return vec![format!("keymap.json: {e}")],
    };
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    for e in entries {
        let action = match cx.build_action(&e.action, None) {
            Ok(a) => a,
            Err(_) => {
                errors.push(format!("keymap.json: unknown action \"{}\"", e.action));
                continue;
            }
        };
        let context = match e.context.as_deref().map(KeyBindingContextPredicate::parse).transpose() {
            Ok(c) => c.map(Into::into),
            Err(err) => {
                errors.push(format!("keymap.json: bad context for {}: {err}", e.key));
                continue;
            }
        };
        match KeyBinding::load(&e.key, action, context, false, None, &gpui::DummyKeyboardMapper) {
            Ok(b) => bindings.push(b),
            Err(err) => errors.push(format!("keymap.json: bad key \"{}\": {err}", e.key)),
        }
    }
    cx.bind_keys(bindings);
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_settings_fill_in_defaults() {
        let s: Settings = serde_json::from_str(r#"{ "theme": "dark", "rows_per_page": 1000 }"#).unwrap();
        assert_eq!((s.theme.as_str(), s.rows_per_page, s.confirm_on_prod), ("dark", 1000, true));
    }
}
