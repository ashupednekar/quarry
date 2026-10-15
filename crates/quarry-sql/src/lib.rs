// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

pub mod completer;
pub mod params;
pub mod rewrites;
pub mod splitter;

pub use completer::{
    extract_ctes, extract_select_columns, find_usages, lint, match_score, mentioned_unloaded,
    rename_identifier, resolve_cte, resolve_qualifier, smart_alias, table_refs,
    CompletionCandidate, CompletionKind, CteSchema, SchemaCompleter, TableRef,
};
pub use params::{bind_params, find_params};
pub use rewrites::{rewrites, with_limit};
pub use splitter::{
    find_statement_at_offset, find_statement_at_offset_with, split_statements,
    split_statements_with, tree_sitter_split_statements, tree_sitter_statement_at_offset,
    SplitRules, StatementSpan,
};
