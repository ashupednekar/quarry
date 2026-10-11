// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Query history (L2), persisted as JSON lines in ~/.config/quarry/history.jsonl.

use crate::state::QueryHistoryItem;
use std::io::Write;

/// Retention (L7): entries older than this, or beyond the newest MAX_ENTRIES, are dropped on load
pub const MAX_AGE_DAYS: i64 = 30;
pub const MAX_ENTRIES: usize = 10_000;

fn path() -> Option<std::path::PathBuf> {
    Some(crate::connections::config_dir()?.join("history.jsonl"))
}

/// Loads history, applying retention and rewriting the file if anything was pruned
pub fn load() -> Vec<QueryHistoryItem> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let total = text.lines().count();
    let cutoff = chrono::Utc::now() - chrono::Duration::days(MAX_AGE_DAYS);
    let mut items: Vec<QueryHistoryItem> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|i: &QueryHistoryItem| i.timestamp >= cutoff)
        .collect();
    if items.len() > MAX_ENTRIES {
        items.drain(..items.len() - MAX_ENTRIES);
    }
    if items.len() != total {
        let lines: Vec<String> = items
            .iter()
            .filter_map(|i| serde_json::to_string(i).ok())
            .collect();
        let _ = std::fs::write(&path, lines.join("\n") + "\n");
    }
    items
}

pub fn append(item: &QueryHistoryItem) {
    let (Some(path), Ok(line)) = (path(), serde_json::to_string(item)) else {
        return;
    };
    let _ = std::fs::create_dir_all(path.parent().expect("has parent"));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}
