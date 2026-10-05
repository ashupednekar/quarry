// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! SQL files and session restore (Q8, Q9): format, open/save .sql, and reopen unsaved query tabs.

use crate::state::Tab;
use crate::QuarryWindow;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct SavedTab {
    title: String,
    sql: String,
    file: Option<PathBuf>,
}

fn session_path() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("session.json"))
}

/// Pretty-prints SQL; keeps the user's keyword case
pub fn format_sql(sql: &str) -> String {
    let opts = sqlformat::FormatOptions { lines_between_queries: 2, ..Default::default() };
    sqlformat::format(sql, &sqlformat::QueryParams::None, &opts)
}

impl QuarryWindow {
    /// Formats the selection, or the whole query
    pub fn format_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query { editor, .. }) = self.state.active_tab() else { return };
        let editor = editor.clone();
        let (text, sel) = {
            let e = editor.read(cx);
            (e.value().to_string(), e.selected_range())
        };
        let formatted = if sel.is_empty() {
            format_sql(&text)
        } else {
            format!("{}{}{}", &text[..sel.start], format_sql(&text[sel.clone()]), &text[sel.end..])
        };
        editor.update(cx, |e, cx| e.set_value(formatted, window, cx));
    }

    /// ⌘S on a query tab: write to its file, or ask where (Save As) the first time
    pub fn save_query(&mut self, save_as: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.state.active_tab_index;
        let Some(Tab::Query { editor, file, title, .. }) = self.state.tabs.get(ix) else { return };
        let text = editor.read(cx).value().to_string();
        let (existing, name) = (file.clone(), format!("{}.sql", title.trim_end_matches(".sql").replace(' ', "_")));
        match existing.filter(|_| !save_as) {
            Some(path) => self.write_query(ix, path, text, cx),
            None => {
                let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                let rx = cx.prompt_for_new_path(&dir, Some(&name));
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(path))) = rx.await else { return };
                    this.update(cx, |this, cx| this.write_query(ix, path, text, cx)).ok();
                })
                .detach();
            }
        }
    }

    fn write_query(&mut self, ix: usize, path: PathBuf, text: String, cx: &mut Context<Self>) {
        if let Err(e) = std::fs::write(&path, &text) {
            return self.toast(format!("Couldn't save: {e}"), true, cx);
        }
        if let Some(Tab::Query { file, title, saved_text, .. }) = self.state.tabs.get_mut(ix) {
            *title = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            *file = Some(path.clone());
            *saved_text = text;
        }
        self.save_session(cx);
        // Saving a config file applies it right away
        if crate::settings::settings_path().is_some_and(|p| p == path) {
            self.reload_settings(cx);
            self.toast("Settings applied", false, cx);
        } else if crate::settings::keymap_path().is_some_and(|p| p == path) {
            match crate::settings::apply_keymap(cx).first() {
                Some(e) => self.toast(e.clone(), true, cx),
                None => self.toast("Keymap applied", false, cx),
            }
        }
        cx.notify();
    }

    /// ⌘O: open a .sql file in a new query tab
    pub fn open_sql_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            this.update_in(cx, |this, window, cx| {
                for path in paths {
                    match std::fs::read_to_string(&path) {
                        Ok(sql) => this.open_query_tab_with(&sql, Some(path), window, cx),
                        Err(e) => this.toast(format!("Couldn't open {}: {}", path.display(), e), true, cx),
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes open query tabs to disk shortly after typing stops, so nothing is lost
    pub fn schedule_session_save(&mut self, cx: &mut Context<Self>) {
        self.session_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            this.update(cx, |this, cx| this.save_session(cx)).ok();
        }));
    }

    pub fn save_session(&mut self, cx: &mut Context<Self>) {
        let tabs: Vec<SavedTab> = self
            .state
            .tabs
            .iter()
            .filter_map(|t| match t {
                Tab::Query { title, editor, file, .. } => {
                    Some(SavedTab { title: title.clone(), sql: editor.read(cx).value().to_string(), file: file.clone() })
                }
                _ => None,
            })
            .collect();
        let Some(path) = session_path() else { return };
        let _ = std::fs::create_dir_all(path.parent().expect("has parent"));
        if let Ok(json) = serde_json::to_string_pretty(&tabs) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Reopens last session's query tabs (unsaved text included)
    pub fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = session_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Vec<SavedTab>>(&s).ok())
        else {
            return;
        };
        for tab in saved {
            self.open_query_tab_with(&tab.sql, tab.file.clone(), window, cx);
            if let Some(Tab::Query { title, saved_text, file, .. }) = self.state.tabs.last_mut() {
                *title = tab.title;
                // A file-backed tab is "dirty" if its text differs from what's on disk
                if let Some(disk) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
                    *saved_text = disk;
                }
            }
        }
        if !self.state.tabs.is_empty() {
            self.state.active_tab_index = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_and_keeps_case() {
        assert_eq!(super::format_sql("select a,b from t where x=1"), "select\n  a,\n  b\nfrom\n  t\nwhere\n  x = 1");
    }
}
