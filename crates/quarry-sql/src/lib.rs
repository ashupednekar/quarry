// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

pub mod completer;
pub mod params;
pub mod rewrites;
pub mod splitter;

pub use params::{bind_params, find_params};
pub use rewrites::{rewrites, with_limit};
pub use completer::{lint, resolve_qualifier, table_refs, CompletionCandidate, CompletionKind, SchemaCompleter, TableRef};
pub use splitter::{find_statement_at_offset, split_statements, StatementSpan};
