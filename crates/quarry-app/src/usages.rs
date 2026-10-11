// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Find Usages: where a table is named in open query tabs and saved .sql files, listed in the palette

use crate::state::Tab;
use crate::QuarryWindow;
use gpui::*;
use std::ops::Range;
use std::path::PathBuf;

#[derive(Clone)]
pub enum Place {
    Tab(String),
    File(PathBuf),
}

#[derive(Clone)]
pub struct Usage {
    pub place: Place,
    /// Where it is, for the palette: "orders.sql:12" or "query 2:3"
    pub label: String,
    pub line_text: String,
    pub range: Range<usize>,
}

/// The palette shows these instead of its usual items while set
pub struct Usages {
    pub name: String,
    pub items: Vec<Usage>,
}

fn collect(place: Place, title: &str, text: &str, name: &str, out: &mut Vec<Usage>) {
    for range in quarry_sql::find_usages(text, name) {
        let line = text[..range.start].matches('\n').count() + 1;
        let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let end = text[range.end..]
            .find('\n')
            .map_or(text.len(), |i| range.end + i);
        out.push(Usage {
            place: place.clone(),
            label: format!("{title}:{line}"),
            line_text: text[start..end].trim().to_string(),
            range,
        });
    }
}

impl QuarryWindow {
    pub fn find_usages(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut items = Vec::new();
        let mut open_files = Vec::new();
        for tab in &self.state.tabs {
            if let Tab::Query {
                id,
                title,
                editor,
                file,
                ..
            } = tab
            {
                collect(
                    Place::Tab(id.clone()),
                    title,
                    &editor.read(cx).value(),
                    name,
                    &mut items,
                );
                open_files.extend(file.clone());
            }
        }
        // Saved files not already open (an open tab may hold unsaved edits, so it wins)
        for path in crate::files::saved_queries()
            .into_iter()
            .chain(crate::files::recent_files())
        {
            if open_files.contains(&path) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                let title = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                collect(Place::File(path.clone()), &title, &text, name, &mut items);
                open_files.push(path);
            }
        }
        if items.is_empty() {
            return self.toast(format!("No open or saved query mentions {name}"), false, cx);
        }
        self.state.usages = Some(Usages {
            name: name.to_string(),
            items,
        });
        self.state.command_bar_open = true;
        self.state.command_bar_selected_index = 0;
        self.command_input.update(cx, |i, cx| {
            i.set_value("", window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    /// `ALTER TABLE … RENAME TO …` to review and run, with the saved queries that will need updating
    pub fn rename_table_sql(&self, schema: &str, table: &str) -> String {
        let d = self.state.dialect();
        let mut files: Vec<String> = crate::files::saved_queries()
            .into_iter()
            .filter(|p| {
                std::fs::read_to_string(p)
                    .is_ok_and(|t| !quarry_sql::find_usages(&t, table).is_empty())
            })
            .map(|p| format!("--   {}", p.display()))
            .collect();
        let note = if files.is_empty() {
            "-- No saved query mentions it.".to_string()
        } else {
            files.insert(0, "-- Saved queries that name it (right-click the table → Find Usages to go through them):".into());
            files.join("\n")
        };
        format!("-- Rename {table}: edit the new name, then run\nALTER TABLE {} RENAME TO {};\n\n{note}\n", d.qualified(schema, table), d.quote_ident(&format!("{table}_new")))
    }

    /// Opens the tab or file a usage is in, with the name selected
    pub fn open_usage(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(usage) = self
            .state
            .usages
            .take()
            .and_then(|u| u.items.into_iter().nth(ix))
        else {
            return;
        };
        let tab_ix = match &usage.place {
            Place::Tab(id) => self.state.tabs.iter().position(|t| t.id() == id),
            Place::File(path) => {
                self.open_query_file(path.clone(), window, cx);
                self.state
                    .tabs
                    .iter()
                    .position(|t| matches!(t, Tab::Query { file: Some(f), .. } if f == path))
            }
        };
        let Some(tab_ix) = tab_ix else { return };
        self.state.active_tab_index = tab_ix;
        if let Some(Tab::Query { editor, .. }) = self.state.tabs.get(tab_ix) {
            let editor = editor.clone();
            editor.update(cx, |e, cx| {
                e.set_selected_range(usage.range, cx);
                e.focus(window, cx);
            });
        }
        cx.notify();
    }
}
