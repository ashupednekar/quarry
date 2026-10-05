// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Schema-aware autocomplete for the SQL editor (Q2): keywords, tables, and table.column.

use gpui::component::input::{CompletionProvider, Rope, RopeExt};
use gpui::{App, Task, Window};
use lsp_types::{CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit, TextEdit};
use quarry_core::SchemaCatalog;
use quarry_sql::{CompletionKind, SchemaCompleter};
use std::cell::RefCell;
use std::rc::Rc;

/// Shared with every editor; replaced whenever the active connection's schema changes
pub type SharedCatalog = Rc<RefCell<SchemaCatalog>>;

pub struct SchemaCompletions(pub SharedCatalog);

impl CompletionProvider for SchemaCompletions {
    fn completions(&self, text: &Rope, offset: usize, _: CompletionContext, _: &mut Window, _: &mut App) -> Task<anyhow::Result<CompletionResponse>> {
        let sql = text.to_string();
        let offset = offset.min(sql.len());
        // Replace the identifier being typed
        let start = sql[..offset].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        let range = lsp_types::Range { start: text.offset_to_position(start), end: text.offset_to_position(offset) };
        let items = SchemaCompleter::complete(&sql, offset, &self.0.borrow())
            .into_iter()
            .map(|c| CompletionItem {
                kind: Some(match c.kind {
                    CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                    CompletionKind::Table => CompletionItemKind::STRUCT,
                    CompletionKind::Column => CompletionItemKind::FIELD,
                    CompletionKind::Schema => CompletionItemKind::MODULE,
                }),
                detail: c.detail,
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: c.label.clone() })),
                label: c.label,
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    /// Typing a word character or `.` (for table.column) opens suggestions
    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }
}
