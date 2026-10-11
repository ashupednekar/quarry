// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Pinned tables and queries per connection (U6), in ~/.config/quarry/pins.json.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Pin {
    Table { schema: String, table: String },
    Query { title: String, sql: String },
}

impl Pin {
    pub fn label(&self) -> &str {
        match self {
            Pin::Table { table, .. } => table,
            Pin::Query { title, .. } => title,
        }
    }
}

/// Connection id → its pins, in pin order
pub type Pins = HashMap<String, Vec<Pin>>;

fn path() -> Option<std::path::PathBuf> {
    Some(crate::connections::config_dir()?.join("pins.json"))
}

pub fn load() -> Pins {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(pins: &Pins) {
    let Some(path) = path() else { return };
    let _ = std::fs::create_dir_all(path.parent().expect("has parent"));
    if let Ok(json) = serde_json::to_string_pretty(pins) {
        let _ = std::fs::write(path, json);
    }
}

/// Adds the pin, or removes it if it's already pinned. Returns whether it's pinned now.
pub fn toggle(pins: &mut Pins, connection: &str, pin: Pin) -> bool {
    let list = pins.entry(connection.to_string()).or_default();
    if let Some(i) = list.iter().position(|p| p == &pin) {
        list.remove(i);
        false
    } else {
        list.push(pin);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_pins_per_connection() {
        let mut pins = Pins::new();
        let t = Pin::Table {
            schema: "public".into(),
            table: "users".into(),
        };
        assert!(toggle(&mut pins, "a", t.clone()));
        assert!(toggle(&mut pins, "b", t.clone()));
        assert!(!toggle(&mut pins, "a", t.clone()));
        assert!(pins["a"].is_empty() && pins["b"].len() == 1);
        let json = serde_json::to_string(&pins["b"]).unwrap();
        assert_eq!(
            json,
            r#"[{"kind":"table","schema":"public","table":"users"}]"#
        );
    }
}
