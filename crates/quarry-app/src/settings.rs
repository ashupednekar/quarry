// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Settings and keymap as human-editable JSON (P2-P4) in ~/.config/quarry.

use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // Appearance
    /// "system", a built-in ("Quarry Dark", "Quarry Light", "Graphite", "Paper") or a themes/<name>.json
    pub theme: String,
    /// Interface text size in points
    pub font_size: f32,
    /// SQL editor and data cells; 0 follows the interface size
    pub mono_font_size: f32,
    /// Font for SQL and data cells; empty keeps the bundled JetBrainsMono Nerd Font
    pub mono_font_family: String,

    // Editor
    pub tab_size: usize,
    pub soft_wrap: bool,
    pub line_numbers: bool,
    /// Tint the statement ⌘↵ would run
    pub highlight_current_statement: bool,
    /// Schema-aware completions in the SQL editor
    pub autocomplete: bool,

    // Data grid
    /// Rows fetched per page in table tabs
    pub rows_per_page: usize,
    /// How NULL cells read
    pub null_display: String,
    /// Widest a column grows to fit its values, in pixels
    pub max_column_width: f32,

    // Safety & connections
    /// Ask before running statements that change data on PROD connections, and always review submits there
    pub confirm_on_prod: bool,
    /// "auto" or "manual" commit for new connections
    pub default_transaction_mode: String,
    /// For connections that don't set their own; 0 = none
    pub default_statement_timeout_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            font_size: 16.0,
            mono_font_size: 0.0,
            mono_font_family: String::new(),
            tab_size: 2,
            soft_wrap: false,
            line_numbers: true,
            highlight_current_statement: true,
            autocomplete: true,
            rows_per_page: 500,
            null_display: "NULL".into(),
            max_column_width: 420.0,
            confirm_on_prod: true,
            default_transaction_mode: "auto".into(),
            default_statement_timeout_secs: 0,
        }
    }
}

impl Settings {
    pub const MIN_FONT_SIZE: f32 = 8.0;
    pub const MAX_FONT_SIZE: f32 = 32.0;

    pub fn mono_size(&self) -> f32 {
        if self.mono_font_size > 0.0 {
            self.mono_font_size
        } else {
            self.font_size - 2.0
        }
    }

    /// Increase overall font size by 1 point, up to MAX_FONT_SIZE
    pub fn increase_font_size(&mut self) {
        self.font_size = (self.font_size + 1.0).min(Self::MAX_FONT_SIZE);
        if self.mono_font_size > 0.0 {
            self.mono_font_size = (self.mono_font_size + 1.0).min(Self::MAX_FONT_SIZE);
        }
    }

    /// Decrease overall font size by 1 point, down to MIN_FONT_SIZE
    pub fn decrease_font_size(&mut self) {
        self.font_size = (self.font_size - 1.0).max(Self::MIN_FONT_SIZE);
        if self.mono_font_size > 0.0 {
            self.mono_font_size = (self.mono_font_size - 1.0).max(Self::MIN_FONT_SIZE);
        }
    }

    /// Reset font size to default
    pub fn reset_font_size(&mut self) {
        self.font_size = Self::default().font_size;
        if self.mono_font_size > 0.0 {
            self.mono_font_size = Self::default().mono_font_size;
        }
    }
}

/// Writes settings.json (the modal saves through this; hand edits are picked up the same way)
pub fn save(s: &Settings) -> Result<(), String> {
    let path = settings_path().ok_or("no home directory")?;
    std::fs::create_dir_all(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, json + "\n").map_err(|e| e.to_string())
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
    let Some(text) = keymap_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
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
        let context = match e
            .context
            .as_deref()
            .map(KeyBindingContextPredicate::parse)
            .transpose()
        {
            Ok(c) => c.map(Into::into),
            Err(err) => {
                errors.push(format!("keymap.json: bad context for {}: {err}", e.key));
                continue;
            }
        };
        match KeyBinding::load(
            &e.key,
            action,
            context,
            false,
            None,
            &gpui::DummyKeyboardMapper,
        ) {
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
    fn font_size_adjustment() {
        let mut s = Settings::default();
        assert_eq!(s.font_size, 16.0);
        assert_eq!(s.mono_size(), 14.0);

        s.increase_font_size();
        assert_eq!(s.font_size, 17.0);
        assert_eq!(s.mono_size(), 15.0);

        s.decrease_font_size();
        assert_eq!(s.font_size, 16.0);

        s.font_size = Settings::MAX_FONT_SIZE;
        s.increase_font_size();
        assert_eq!(s.font_size, Settings::MAX_FONT_SIZE);

        s.font_size = Settings::MIN_FONT_SIZE;
        s.decrease_font_size();
        assert_eq!(s.font_size, Settings::MIN_FONT_SIZE);

        s.reset_font_size();
        assert_eq!(s.font_size, 16.0);

        // With explicit mono font size
        s.mono_font_size = 12.0;
        s.increase_font_size();
        assert_eq!(s.font_size, 17.0);
        assert_eq!(s.mono_font_size, 13.0);
        s.reset_font_size();
        assert_eq!(s.font_size, 16.0);
        assert_eq!(s.mono_font_size, 0.0);
    }

    #[test]
    fn test_zoom_key_bindings() {
        use crate::{DecreaseFontSize, IncreaseFontSize, ResetFontSize};
        use gpui::KeyBinding;

        let bindings = [
            KeyBinding::new("secondary-=", IncreaseFontSize, None),
            KeyBinding::new("secondary-+", IncreaseFontSize, None),
            KeyBinding::new("secondary-shift-=", IncreaseFontSize, None),
            KeyBinding::new("secondary-shift-+", IncreaseFontSize, None),
            KeyBinding::new("secondary--", DecreaseFontSize, None),
            KeyBinding::new("secondary-0", ResetFontSize, None),
        ];
        assert_eq!(bindings.len(), 6);
    }

    #[test]
    fn partial_settings_fill_in_defaults() {
        let s: Settings =
            serde_json::from_str(r#"{ "theme": "dark", "rows_per_page": 1000 }"#).unwrap();
        assert_eq!(
            (s.theme.as_str(), s.rows_per_page, s.confirm_on_prod),
            ("dark", 1000, true)
        );
    }
}
