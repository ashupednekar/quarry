// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use quarry_core::{SchemaCatalog, TableSchema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    Keyword,
    Table,
    Column,
    Schema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCandidate {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

pub struct SchemaCompleter;

const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "INNER JOIN",
    "ON",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "CREATE TABLE",
    "DROP TABLE",
    "ALTER TABLE",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS NULL",
    "IS NOT NULL",
    "BETWEEN",
    "LIKE",
    "AS",
    "DISTINCT",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "EXPLAIN ANALYZE",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
];

/// Words that end a `FROM a x, b y` list, so they are never read as an alias
const RESERVED: &[&str] = &[
    "where",
    "on",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "full",
    "cross",
    "natural",
    "group",
    "order",
    "having",
    "limit",
    "offset",
    "union",
    "except",
    "intersect",
    "set",
    "values",
    "using",
    "as",
    "select",
    "from",
    "returning",
    "window",
    "lateral",
    "and",
    "or",
    "into",
    "default",
];

/// A table named in a statement's FROM / JOIN / UPDATE / INTO, with its alias
#[derive(Debug, Clone)]
pub struct TableRef<'a> {
    /// As written, quotes removed (`public.users`)
    pub name: String,
    /// Byte range of the name in the statement
    pub range: std::ops::Range<usize>,
    pub alias: Option<String>,
    /// None when the catalog has no such table
    pub table: Option<&'a TableSchema>,
}

impl TableRef<'_> {
    /// What columns are qualified with: the alias, else the bare table name
    pub fn qualifier(&self) -> String {
        self.alias.clone().unwrap_or_else(|| {
            self.name
                .rsplit('.')
                .next()
                .unwrap_or(&self.name)
                .to_string()
        })
    }
}

/// Identifier-ish tokens (`a.b`, `"x"`), commas and parens, with byte ranges; skips strings and comments
fn tokens(sql: &str) -> Vec<(&str, std::ops::Range<usize>)> {
    let b = sql.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        let c = b[i];
        if c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != b'\'' {
                i += 1;
            }
            i += 1;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c.is_ascii_alphanumeric() || c == b'_' || c == b'"' || c >= 0x80 {
            let start = i;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric()
                    || matches!(b[i], b'_' | b'.' | b'"')
                    || b[i] >= 0x80)
            {
                i += 1;
            }
            out.push((&sql[start..i], start..i));
        } else {
            if matches!(c, b',' | b'(' | b')' | b';') {
                out.push((&sql[i..i + 1], i..i + 1));
            }
            i += 1;
        }
    }
    out
}

/// Where `name` appears as an identifier (a table, column or alias), as byte ranges. Matches each
/// part of a dotted name on its own, so `public.users` and `u.users_id` are checked part by part.
/// Unquoted names match case-insensitively; strings and comments are skipped.
pub fn find_usages(sql: &str, name: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for (tok, range) in tokens(sql) {
        let mut at = range.start;
        for part in tok.split('.') {
            let hit = match part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                Some(quoted) => quoted == name,
                None => part.eq_ignore_ascii_case(name),
            };
            if hit {
                out.push(at..at + part.len());
            }
            at += part.len() + 1;
        }
    }
    out
}

/// `sql` with every usage of `old` renamed to `new` (quoted usages stay quoted)
pub fn rename_identifier(sql: &str, old: &str, new: &str) -> String {
    let mut out = sql.to_string();
    for r in find_usages(sql, old).into_iter().rev() {
        let quoted = sql[r.clone()].starts_with('"');
        out.replace_range(
            r,
            &if quoted {
                format!("\"{new}\"")
            } else {
                new.to_string()
            },
        );
    }
    out
}

fn is_kw(tok: &str, kw: &str) -> bool {
    tok.eq_ignore_ascii_case(kw)
}

fn lookup<'a>(name: &str, catalog: &'a SchemaCatalog) -> Option<&'a TableSchema> {
    match name.split_once('.') {
        Some((schema, table)) => catalog.find_table(Some(schema), table),
        None => catalog.find_table(None, name),
    }
}

/// Every table a statement reads or writes, so aliases and scope can be resolved
pub fn table_refs<'a>(stmt: &str, catalog: &'a SchemaCatalog) -> Vec<TableRef<'a>> {
    let toks = tokens(stmt);
    let mut refs = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i].0;
        let opens_list =
            is_kw(t, "from") || is_kw(t, "join") || is_kw(t, "update") || is_kw(t, "into");
        i += 1;
        if !opens_list {
            continue;
        }
        // `FROM a x, b AS y` — keep reading refs while commas follow
        while let Some((name, range)) = toks.get(i).cloned() {
            if name == "(" || RESERVED.iter().any(|r| is_kw(name, r)) {
                break;
            }
            i += 1;
            let mut alias = None;
            if toks.get(i).is_some_and(|(a, _)| is_kw(a, "as")) {
                i += 1;
            }
            if let Some((a, _)) = toks.get(i) {
                if !matches!(*a, "," | "(" | ")" | ";") && !RESERVED.iter().any(|r| is_kw(a, r)) {
                    alias = Some(a.replace('"', ""));
                    i += 1;
                }
            }
            let name = name.replace('"', "");
            refs.push(TableRef {
                table: lookup(&name, catalog),
                name,
                range,
                alias,
            });
            if toks.get(i).is_some_and(|(c, _)| *c == ",") {
                i += 1;
            } else {
                break;
            }
        }
    }
    refs
}

/// Output columns of a subquery in FROM, by its alias: `FROM (SELECT a, b AS c FROM t) x` → `x` has `a`, `c`.
/// `*` and `t.*` expand to the inner tables' columns. Each is (name, detail).
pub fn subquery_columns(
    stmt: &str,
    alias: &str,
    catalog: &SchemaCatalog,
) -> Option<Vec<(String, String)>> {
    let toks = tokens(stmt);
    // `) x` or `) AS x`, then back to the matching `(`
    let close = (0..toks.len()).find(|&i| {
        toks[i].0 == ")" && {
            let mut j = i + 1;
            if toks.get(j).is_some_and(|(t, _)| is_kw(t, "as")) {
                j += 1;
            }
            toks.get(j)
                .is_some_and(|(t, _)| t.replace('"', "").eq_ignore_ascii_case(alias))
        }
    })?;
    let mut depth = 0;
    let open = (0..close).rev().find(|&i| {
        match toks[i].0 {
            ")" => depth += 1,
            "(" if depth == 0 => return true,
            "(" => depth -= 1,
            _ => {}
        }
        false
    })?;
    let inner = &stmt[toks[open].1.end..toks[close].1.start];
    let it = tokens(inner);
    // The select list: between SELECT and FROM at depth 0, split on depth-0 commas
    let (mut depth, mut items, mut cur, mut in_list) = (0, Vec::new(), Vec::new(), false);
    for (t, range) in &it {
        match *t {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 && is_kw(t, "select") {
            in_list = true;
            continue;
        }
        if depth == 0 && is_kw(t, "from") {
            break;
        }
        if !in_list {
            continue;
        }
        if depth == 0 && *t == "," {
            items.push(std::mem::take(&mut cur));
        } else {
            cur.push((*t, range.clone()));
        }
    }
    items.push(cur);
    if !in_list {
        return None;
    }
    let refs = table_refs(inner, catalog);
    let mut out = Vec::new();
    for item in items {
        let words: Vec<&str> = item
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| !matches!(*t, "(" | ")"))
            .collect();
        let star_of = match words.last() {
            None => Some(None), // `*` (not a token)
            Some(w) if w.ends_with('.') => Some(Some(w.trim_end_matches('.'))), // `t.*`
            _ => None,
        };
        if let Some(q) = star_of {
            for r in refs
                .iter()
                .filter(|r| q.is_none_or(|q| r.qualifier().eq_ignore_ascii_case(q)))
            {
                for c in r.table.into_iter().flat_map(|t| &t.columns) {
                    out.push((c.name.clone(), c.raw_type_name.to_lowercase()));
                }
            }
            continue;
        }
        let name = match words.iter().position(|w| is_kw(w, "as")) {
            Some(i) => words.get(i + 1).copied(),
            // A call without an alias is named after the function, as Postgres does
            None if item.iter().any(|(t, _)| *t == "(") => words.first().copied(),
            None => words.last().copied(),
        };
        if let Some(n) = name {
            out.push((
                n.rsplit('.').next().unwrap_or(n).replace('"', ""),
                "subquery".into(),
            ));
        }
    }
    Some(out)
}

/// The table an identifier qualifier means in this statement: an alias, a table name, or `schema.table`
pub fn resolve_qualifier<'a>(
    qualifier: &str,
    refs: &[TableRef<'a>],
    catalog: &'a SchemaCatalog,
) -> Option<&'a TableSchema> {
    refs.iter()
        .find(|r| {
            r.qualifier().eq_ignore_ascii_case(qualifier)
                || r.name.eq_ignore_ascii_case(qualifier)
                || qualifier
                    .rsplit('.')
                    .next()
                    .is_some_and(|bare| r.qualifier().eq_ignore_ascii_case(bare))
        })
        .and_then(|r| r.table)
        .or_else(|| lookup(qualifier, catalog))
}

fn detect_group_by_needed(stmt: &str) -> Option<Vec<String>> {
    let it = tokens(stmt);
    let mut depth = 0;
    let mut in_select = false;
    let mut select_items = Vec::new();
    let mut cur_item = Vec::new();
    let mut has_agg = false;

    for (t, _) in &it {
        match *t {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 && is_kw(t, "select") {
            in_select = true;
            continue;
        }
        if depth == 0 && is_kw(t, "from") {
            break;
        }
        if !in_select {
            continue;
        }
        if depth == 0 && *t == "," {
            if !cur_item.is_empty() {
                select_items.push(std::mem::take(&mut cur_item));
            }
        } else {
            cur_item.push(*t);
        }
    }
    if !cur_item.is_empty() {
        select_items.push(cur_item);
    }
    if !in_select || select_items.is_empty() {
        return None;
    }

    let is_agg_fn = |w: &str| {
        is_kw(w, "count")
            || is_kw(w, "sum")
            || is_kw(w, "avg")
            || is_kw(w, "min")
            || is_kw(w, "max")
            || is_kw(w, "string_agg")
            || is_kw(w, "array_agg")
            || is_kw(w, "count_if")
            || is_kw(w, "group_concat")
            || is_kw(w, "every")
            || is_kw(w, "bool_and")
            || is_kw(w, "bool_or")
    };

    let mut non_aggs = Vec::new();
    for item in select_items {
        if item.iter().any(|w| is_agg_fn(w)) {
            has_agg = true;
        } else {
            let words: Vec<&str> = item
                .into_iter()
                .filter(|w| !matches!(*w, "(" | ")"))
                .collect();
            if words.is_empty() || words == ["*"] {
                continue;
            }
            let expr = match words.iter().position(|w| is_kw(w, "as")) {
                Some(idx) => words[..idx].join(" "),
                None => words.join(" "),
            };
            if !expr.is_empty() && !non_aggs.contains(&expr) {
                non_aggs.push(expr);
            }
        }
    }

    if has_agg && !non_aggs.is_empty() {
        Some(non_aggs)
    } else {
        None
    }
}

fn find_column_in_refs<'a>(
    raw_col: &str,
    refs: &[TableRef<'a>],
    catalog: &'a SchemaCatalog,
) -> Option<&'a quarry_core::ColumnSchema> {
    let clean = raw_col.replace('"', "");
    if let Some((q, col)) = clean.split_once('.') {
        if let Some(table) = resolve_qualifier(q, refs, catalog) {
            return table
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(col));
        }
    } else {
        for r in refs {
            if let Some(table) = r.table {
                if let Some(c) = table
                    .columns
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(&clean))
                {
                    return Some(c);
                }
            }
        }
    }
    None
}

fn find_fk_between<'a>(
    t1: &'a TableSchema,
    t2: &'a TableSchema,
) -> Option<(&'a str, &'a str, bool)> {
    // Returns (t1_col, t2_col, t1_points_to_t2)
    for fk in &t1.foreign_keys {
        if fk.to_table.eq_ignore_ascii_case(&t2.table_name) {
            return Some((&fk.from_column, &fk.to_column, true));
        }
    }
    for fk in &t2.foreign_keys {
        if fk.to_table.eq_ignore_ascii_case(&t1.table_name) {
            return Some((&fk.to_column, &fk.from_column, false));
        }
    }
    None
}

impl SchemaCompleter {
    /// Best matches first: prefix, then substring, then letters in order (`cid` → `customer_id`).
    /// Ties keep the scope order (columns in scope, keywords, tables…).
    pub fn complete(
        sql: &str,
        cursor_offset: usize,
        catalog: &SchemaCatalog,
    ) -> Vec<CompletionCandidate> {
        Self::complete_with_usage(sql, cursor_offset, catalog, None)
    }

    /// Complete with frequency usage ranking as a tiebreaker for equal match scores.
    pub fn complete_with_usage(
        sql: &str,
        cursor_offset: usize,
        catalog: &SchemaCatalog,
        usage: Option<&std::collections::HashMap<String, u32>>,
    ) -> Vec<CompletionCandidate> {
        let prefix = Self::extract_prefix(sql, cursor_offset.min(sql.len())).to_lowercase();
        let mut out = Self::candidates(sql, cursor_offset, catalog);
        out.sort_by(|a, b| {
            let score_a = match_score(&prefix, &a.label).unwrap_or(u8::MAX);
            let score_b = match_score(&prefix, &b.label).unwrap_or(u8::MAX);
            if score_a != score_b {
                return score_a.cmp(&score_b);
            }
            if let Some(usage) = usage {
                let count_a = usage.get(&a.label.to_lowercase()).copied().unwrap_or(0);
                let count_b = usage.get(&b.label.to_lowercase()).copied().unwrap_or(0);
                if count_a != count_b {
                    return count_b.cmp(&count_a); // higher usage comes first
                }
            }
            std::cmp::Ordering::Equal
        });
        out.truncate(50);
        out
    }

    /// Returns a ghost text continuation at the end of an open statement, predicting
    /// next clauses (`GROUP BY`, `ORDER BY`, `WHERE`, `JOIN ... ON`), safe guards
    /// (`WHERE ` on UPDATE/DELETE), value defaults, or `LIMIT 100`.
    pub fn continuation(
        sql: &str,
        cursor_offset: usize,
        catalog: &SchemaCatalog,
    ) -> Option<String> {
        let cursor_offset = cursor_offset.min(sql.len());
        let (stmt, at) = match crate::find_statement_at_offset(sql, cursor_offset) {
            Some(s) if cursor_offset >= s.start && cursor_offset <= s.end => {
                (&sql[s.start..s.end], cursor_offset - s.start)
            }
            _ => (sql, cursor_offset),
        };
        let before = &stmt[..at];
        let trimmed_before = before.trim_end();
        let after = stmt[at..].trim();
        if !after.is_empty() && after != ";" {
            return None;
        }

        let toks = tokens(trimmed_before);
        if toks.is_empty() {
            return None;
        }

        let pad = if before.ends_with(' ') { "" } else { " " };
        let has_kw = |kw: &str| toks.iter().any(|(t, _)| is_kw(t, kw));
        let last_tok = toks.last().map(|(t, _)| *t).unwrap_or("");
        let refs = table_refs(stmt, catalog);

        // 1. Operator value completions: `= `, `IS `, `IS NOT `, `IN `, `>= `
        if let Some(stripped) = trimmed_before.strip_suffix('=') {
            let before_eq = stripped.trim_end();
            let col_name = before_eq.split_whitespace().last().unwrap_or("");
            if let Some(c) = find_column_in_refs(col_name, &refs, catalog) {
                if c.data_type == quarry_core::ColumnType::Boolean
                    || c.name.starts_with("is_")
                    || c.name.starts_with("has_")
                    || c.name.starts_with("can_")
                {
                    return Some(format!("{pad}true"));
                } else if c.name.eq_ignore_ascii_case("status") || c.name.ends_with("_status") {
                    return Some(format!("{pad}'active'"));
                } else if c.name.eq_ignore_ascii_case("type") || c.name.ends_with("_type") {
                    return Some(format!("{pad}'default'"));
                }
            }
        }

        if is_kw(last_tok, "is") && before.ends_with(' ') {
            return Some(format!("{pad}NULL"));
        }

        if is_kw(last_tok, "in") && before.ends_with(' ') {
            if let Some(prev) = toks.iter().rev().nth(1) {
                if let Some(c) = find_column_in_refs(prev.0, &refs, catalog) {
                    if matches!(
                        c.data_type,
                        quarry_core::ColumnType::Integer
                            | quarry_core::ColumnType::BigInt
                            | quarry_core::ColumnType::Float
                    ) {
                        return Some(format!("{pad}(1, 2, 3)"));
                    } else {
                        return Some(format!("{pad}('...')"));
                    }
                }
            }
        }

        if (trimmed_before.ends_with(">=")
            || trimmed_before.ends_with('>')
            || trimmed_before.ends_with("<="))
            && before.ends_with(' ')
        {
            let op_len = if trimmed_before.ends_with(">=") || trimmed_before.ends_with("<=") {
                2
            } else {
                1
            };
            let before_op = trimmed_before[..trimmed_before.len() - op_len].trim_end();
            let col_name = before_op.split_whitespace().last().unwrap_or("");
            if let Some(c) = find_column_in_refs(col_name, &refs, catalog) {
                if matches!(
                    c.data_type,
                    quarry_core::ColumnType::Timestamp | quarry_core::ColumnType::Date
                ) || c.name.ends_with("_at")
                    || c.name.contains("date")
                    || c.name.contains("time")
                {
                    return Some(format!("{pad}CURRENT_DATE - INTERVAL '7 days'"));
                }
            }
        }

        // 2. WHERE context defaults (when cursor is after WHERE + space)
        if is_kw(last_tok, "where") && before.ends_with(' ') {
            if let Some(r) = refs.first() {
                if let Some(table) = r.table {
                    let q = r
                        .alias
                        .as_ref()
                        .map(|a| format!("{a}."))
                        .unwrap_or_default();
                    if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("deleted_at"))
                    {
                        return Some(format!("{pad}{q}deleted_at IS NULL"));
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("is_deleted"))
                    {
                        return Some(format!("{pad}{q}is_deleted = false"));
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("status"))
                    {
                        return Some(format!("{pad}{q}status = 'active'"));
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("is_active"))
                    {
                        return Some(format!("{pad}{q}is_active = true"));
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("active"))
                    {
                        return Some(format!("{pad}{q}active = true"));
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("id"))
                    {
                        return Some(format!("{pad}{q}id = "));
                    }
                }
            }
        }

        // 3. ORDER BY auto-synthesis (when cursor is after ORDER BY or ORDER + space)
        let is_order_by =
            (is_kw(last_tok, "by") && toks.len() >= 2 && is_kw(toks[toks.len() - 2].0, "order"))
                || (is_kw(last_tok, "order") && before.ends_with(' '));
        if is_order_by && before.ends_with(' ') {
            if let Some(r) = refs.first() {
                if let Some(table) = r.table {
                    let q = r
                        .alias
                        .as_ref()
                        .map(|a| format!("{a}."))
                        .unwrap_or_default();
                    let col_name = if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("created_at"))
                    {
                        Some("created_at")
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("inserted_at"))
                    {
                        Some("inserted_at")
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("created_date"))
                    {
                        Some("created_date")
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("updated_at"))
                    {
                        Some("updated_at")
                    } else if table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case("id"))
                    {
                        Some("id")
                    } else {
                        table.primary_keys.first().map(|s| s.as_str())
                    };

                    if let Some(c) = col_name {
                        let by_pad = if is_kw(last_tok, "order") { "BY " } else { "" };
                        return Some(format!("{pad}{by_pad}{q}{c} DESC"));
                    }
                }
            }
        }

        // 4. GROUP BY auto-synthesis
        if let Some(non_aggs) = detect_group_by_needed(stmt) {
            let cols_str = non_aggs.join(", ");
            if is_kw(last_tok, "group") && before.ends_with(' ') {
                return Some(format!("{pad}BY {cols_str}"));
            }
            if is_kw(last_tok, "by")
                && toks.len() >= 2
                && is_kw(toks[toks.len() - 2].0, "group")
                && before.ends_with(' ')
            {
                return Some(format!("{pad}{cols_str}"));
            }
            if !has_kw("group")
                && !has_kw("having")
                && !has_kw("order")
                && !has_kw("limit")
                && has_kw("from")
            {
                let from_pos = toks.iter().rposition(|(t, _)| is_kw(t, "from"))?;
                if from_pos + 1 < toks.len() {
                    return Some(format!("{pad}GROUP BY {cols_str}"));
                }
            }
        }

        // 5. JOIN <table> auto-wiring: `SELECT ... FROM users u JOIN orders o ` -> `ON o.user_id = u.id`
        if has_kw("join") && before.ends_with(' ') {
            let join_pos = toks.iter().rposition(|(t, _)| is_kw(t, "join"))?;
            let has_on_after = toks[join_pos..].iter().any(|(t, _)| is_kw(t, "on"));
            if !has_on_after && join_pos + 1 < toks.len() {
                let target_name = toks[join_pos + 1].0.replace('"', "");
                let target_alias = if join_pos + 2 < toks.len()
                    && !matches!(
                        toks[join_pos + 2].0,
                        "," | "(" | ")" | ";" | "where" | "join" | "group" | "order"
                    ) {
                    Some(toks[join_pos + 2].0.replace('"', ""))
                } else {
                    None
                };
                let target_table = lookup(&target_name, catalog);
                let target_q = target_alias.as_deref().unwrap_or(&target_name);

                if let Some(tt) = target_table {
                    for r in &refs {
                        if let Some(other_table) = r.table {
                            if other_table.table_name.eq_ignore_ascii_case(&tt.table_name) {
                                continue;
                            }
                            if let Some((t_col, o_col, _)) = find_fk_between(tt, other_table) {
                                let other_q = r.qualifier();
                                return Some(format!(
                                    "{pad}ON {target_q}.{t_col} = {other_q}.{o_col}"
                                ));
                            }
                        }
                    }
                }
            }
        }

        // 6. Safety continuations: DELETE FROM <table> and UPDATE <table> SET ...
        if has_kw("delete") && has_kw("from") && !has_kw("where") {
            let from_pos = toks.iter().rposition(|(t, _)| is_kw(t, "from"))?;
            if from_pos + 1 < toks.len() {
                return Some(format!("{pad}WHERE "));
            }
        }
        if has_kw("update") && has_kw("set") && !has_kw("where") {
            let set_pos = toks.iter().rposition(|(t, _)| is_kw(t, "set"))?;
            if set_pos + 1 < toks.len() {
                return Some(format!("{pad}WHERE "));
            }
        }

        // 7. INSERT INTO <table> scaffold
        if has_kw("insert")
            && has_kw("into")
            && !has_kw("values")
            && !has_kw("select")
            && !toks.iter().any(|(t, _)| *t == "(")
        {
            let into_pos = toks.iter().rposition(|(t, _)| is_kw(t, "into"))?;
            if into_pos + 1 < toks.len() {
                let table_name = toks[into_pos + 1].0.replace('"', "");
                if let Some(table) = lookup(&table_name, catalog) {
                    let cols: Vec<&str> = table
                        .columns
                        .iter()
                        .filter(|c| !(c.is_primary_key && c.name.eq_ignore_ascii_case("id")))
                        .map(|c| c.name.as_str())
                        .collect();
                    if !cols.is_empty() {
                        let vals = vec!["'...'"; cols.len()].join(", ");
                        return Some(format!("{pad}({}) VALUES ({vals})", cols.join(", ")));
                    }
                }
            }
        }

        // 8. SELECT continuation: after SELECT ... FROM table with no JOIN, WHERE, or LIMIT
        if has_kw("select")
            && has_kw("from")
            && !has_kw("join")
            && !has_kw("where")
            && !has_kw("limit")
            && !has_kw("group")
            && !has_kw("order")
        {
            let from_pos = toks.iter().rposition(|(t, _)| is_kw(t, "from"))?;
            if from_pos + 1 < toks.len() {
                let table_tok = toks[from_pos + 1].0.replace('"', "");
                let table_name = table_tok.rsplit('.').next().unwrap_or(&table_tok);
                if lookup(table_name, catalog).is_some() {
                    return Some(format!("{pad}LIMIT 100"));
                }
            }
        }

        None
    }

    fn candidates(
        sql: &str,
        cursor_offset: usize,
        catalog: &SchemaCatalog,
    ) -> Vec<CompletionCandidate> {
        let cursor_offset = cursor_offset.min(sql.len());
        // Scope is the statement at the cursor, not the whole script
        let (stmt, at) = match crate::find_statement_at_offset(sql, cursor_offset) {
            Some(s) if cursor_offset >= s.start && cursor_offset <= s.end => {
                (&sql[s.start..s.end], cursor_offset - s.start)
            }
            _ => (sql, cursor_offset),
        };
        let prefix = Self::extract_prefix(stmt, at);
        let p = prefix.to_lowercase();
        let starts = |s: &str| match_score(&p, s).is_some();
        // Keywords only by prefix: fuzzy keyword hits are noise
        let kw_starts = |s: &str| s.to_lowercase().starts_with(&p);
        let refs = table_refs(stmt, catalog);
        let mut out = Vec::new();
        let column = |c: &quarry_core::ColumnSchema, detail: String| CompletionCandidate {
            label: c.name.clone(),
            kind: CompletionKind::Column,
            detail: Some(detail),
        };

        // 1. `x.` — columns of the aliased/named table, or tables of a schema
        if let Some(q) = Self::extract_qualifier(stmt, at) {
            if let Some(t) = resolve_qualifier(&q, &refs, catalog) {
                return t
                    .columns
                    .iter()
                    .filter(|c| starts(&c.name))
                    .map(|c| {
                        let type_str = c.raw_type_name.to_lowercase();
                        let pk = if c.is_primary_key { " · PK" } else { "" };
                        column(c, format!("{type_str}{pk}"))
                    })
                    .collect();
            }
            if let Some(cols) = subquery_columns(stmt, &q, catalog) {
                return cols
                    .into_iter()
                    .filter(|(n, _)| starts(n))
                    .map(|(label, detail)| CompletionCandidate {
                        label,
                        kind: CompletionKind::Column,
                        detail: Some(detail),
                    })
                    .collect();
            }
            let is_schema = catalog.schemas.iter().any(|s| s.eq_ignore_ascii_case(&q))
                || catalog.tables.iter().any(|t| t.schema_name.eq_ignore_ascii_case(&q));
            if is_schema {
                return catalog
                    .tables
                    .iter()
                    .filter(|t| t.schema_name.eq_ignore_ascii_case(&q) && starts(&t.table_name))
                    .map(|t| CompletionCandidate {
                        label: t.table_name.clone(),
                        kind: CompletionKind::Table,
                        detail: Some(format!(
                            "{} ({} cols)",
                            if t.is_view { "view" } else { "table" },
                            t.columns.len()
                        )),
                    })
                    .collect();
            }
        }

        // Which clause is the cursor in? Tables after FROM/JOIN/UPDATE/INTO (or a comma in that list)
        let before: Vec<_> = tokens(&stmt[..at - prefix.len()])
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        let last = before.last().copied().unwrap_or("");
        let in_from_list = last == ","
            && before
                .iter()
                .rev()
                .find(|t| KEYWORDS_STARTING_CLAUSE.iter().any(|k| is_kw(t, k)))
                .is_some_and(|t| is_kw(t, "from"));
        let wants_table = ["from", "join", "update", "into", "table"]
            .iter()
            .any(|k| is_kw(last, k))
            || in_from_list;

        if wants_table {
            // 2a. After JOIN: whole `t ON t.a = x.b` clauses from foreign keys, both directions
            if is_kw(last, "join") {
                for r in refs
                    .iter()
                    .filter_map(|r| r.table.map(|t| (r.qualifier(), t)))
                {
                    let (q, from) = r;
                    for fk in &from.foreign_keys {
                        if starts(&fk.to_table) {
                            out.push(join_candidate(
                                &fk.to_table,
                                &format!(
                                    "{}.{} = {q}.{}",
                                    fk.to_table, fk.to_column, fk.from_column
                                ),
                            ));
                        }
                    }
                    for t in catalog.tables.iter().filter(|t| starts(&t.table_name)) {
                        for fk in t
                            .foreign_keys
                            .iter()
                            .filter(|f| f.to_table.eq_ignore_ascii_case(&from.table_name))
                        {
                            out.push(join_candidate(
                                &t.table_name,
                                &format!(
                                    "{}.{} = {q}.{}",
                                    t.table_name, fk.from_column, fk.to_column
                                ),
                            ));
                        }
                    }
                }
            }
            out.extend(
                catalog
                    .tables
                    .iter()
                    .filter(|t| starts(&t.table_name))
                    .map(|t| CompletionCandidate {
                        label: t.table_name.clone(),
                        kind: CompletionKind::Table,
                        detail: Some(format!(
                            "{} · {} ({} cols)",
                            t.schema_name,
                            if t.is_view { "view" } else { "table" },
                            t.columns.len()
                        )),
                    }),
            );
            out.extend(
                catalog
                    .schemas
                    .iter()
                    .filter(|s| starts(s))
                    .map(|s| CompletionCandidate {
                        label: s.clone(),
                        kind: CompletionKind::Schema,
                        detail: Some("schema".into()),
                    }),
            );
            return out;
        }

        // 2b. After ON: join condition from foreign keys between the joining table and in-scope tables
        if is_kw(last, "on") && refs.len() >= 2 {
            let cur_ref = refs.last().unwrap();
            let prev_refs = &refs[..refs.len() - 1];
            if let Some(cur_table) = cur_ref.table {
                let cur_q = cur_ref.qualifier();
                for fk in &cur_table.foreign_keys {
                    for prev in prev_refs {
                        if let Some(prev_table) = prev.table {
                            if fk.to_table.eq_ignore_ascii_case(&prev_table.table_name) {
                                let cond = format!(
                                    "{cur_q}.{} = {}.{}",
                                    fk.from_column,
                                    prev.qualifier(),
                                    fk.to_column
                                );
                                if starts(&cond) {
                                    out.push(CompletionCandidate {
                                        label: cond,
                                        kind: CompletionKind::Column,
                                        detail: Some("FK condition".into()),
                                    });
                                }
                            }
                        }
                    }
                }
                for prev in prev_refs {
                    if let Some(prev_table) = prev.table {
                        for fk in &prev_table.foreign_keys {
                            if fk.to_table.eq_ignore_ascii_case(&cur_table.table_name) {
                                let cond = format!(
                                    "{cur_q}.{} = {}.{}",
                                    fk.to_column,
                                    prev.qualifier(),
                                    fk.from_column
                                );
                                if starts(&cond) {
                                    out.push(CompletionCandidate {
                                        label: cond,
                                        kind: CompletionKind::Column,
                                        detail: Some("FK condition".into()),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2c. Elsewhere: columns of the tables in scope first, then keywords, tables, and other columns
        for r in &refs {
            if let Some(t) = r.table {
                for c in t.columns.iter().filter(|c| starts(&c.name)) {
                    if !out.iter().any(|o: &CompletionCandidate| o.label == c.name) {
                        let pk = if c.is_primary_key { " · PK" } else { "" };
                        out.push(column(
                            c,
                            format!(
                                "{} · {}{pk}",
                                r.qualifier(),
                                c.raw_type_name.to_lowercase()
                            ),
                        ));
                    }
                }
            }
        }
        out.extend(
            KEYWORDS
                .iter()
                .filter(|k| kw_starts(k))
                .map(|k| CompletionCandidate {
                    label: k.to_string(),
                    kind: CompletionKind::Keyword,
                    detail: Some("keyword".into()),
                }),
        );
        out.extend(
            catalog
                .tables
                .iter()
                .filter(|t| starts(&t.table_name))
                .map(|t| CompletionCandidate {
                    label: t.table_name.clone(),
                    kind: CompletionKind::Table,
                    detail: Some(format!(
                        "{} · {} ({} cols)",
                        t.schema_name,
                        if t.is_view { "view" } else { "table" },
                        t.columns.len()
                    )),
                }),
        );
        // Columns from anywhere only while nothing is in scope yet (`SELECT em|` before FROM is typed)
        if refs.iter().all(|r| r.table.is_none()) {
            for t in &catalog.tables {
                for c in t.columns.iter().filter(|c| starts(&c.name)) {
                    if !out
                        .iter()
                        .any(|o| o.label == c.name && o.kind == CompletionKind::Column)
                    {
                        let pk = if c.is_primary_key { " · PK" } else { "" };
                        out.push(column(
                            c,
                            format!(
                                "{} · {}{pk}",
                                t.table_name,
                                c.raw_type_name.to_lowercase()
                            ),
                        ));
                    }
                }
            }
        }
        out
    }

    fn extract_prefix(sql: &str, cursor_offset: usize) -> &str {
        let safe_offset = cursor_offset.min(sql.len());
        let before_cursor = &sql[..safe_offset];
        let start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        &before_cursor[start..]
    }

    fn extract_qualifier(sql: &str, cursor_offset: usize) -> Option<String> {
        let safe_offset = cursor_offset.min(sql.len());
        let before_cursor = &sql[..safe_offset];
        let prefix = Self::extract_prefix(sql, cursor_offset);
        let before_prefix = &before_cursor[..before_cursor.len() - prefix.len()];

        if let Some(trimmed) = before_prefix.strip_suffix('.') {
            let trimmed = trimmed.trim_end();
            if trimmed.is_empty() {
                return None;
            }
            let b = trimmed.as_bytes();
            let mut i = b.len();
            while i > 0 {
                let c = b[i - 1];
                if c.is_ascii_alphanumeric()
                    || matches!(c, b'_' | b'.' | b'"' | b'`' | b'[' | b']')
                    || c >= 0x80
                {
                    i -= 1;
                } else {
                    break;
                }
            }
            let raw_qualifier = &trimmed[i..];
            let clean = raw_qualifier.replace(['"', '`', '[', ']'], "");
            let clean = clean.trim();
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
        None
    }
}

/// Unloaded schemas `sql` mentions as `schema.`, so they can be loaded before completion needs them
pub fn mentioned_unloaded(sql: &str, catalog: &SchemaCatalog) -> Vec<String> {
    if catalog.unloaded.is_empty() {
        return vec![];
    }
    let text = sql.replace(['"', '`', '[', ']'], "").to_lowercase();
    catalog
        .unloaded
        .iter()
        .filter(|s| {
            let needle = format!("{}.", s.to_lowercase());
            text.match_indices(&needle)
                .any(|(i, _)| !text[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
        .cloned()
        .collect()
}

/// Names the catalog doesn't know: tables in FROM/JOIN and `alias.column` references.
/// Returns byte ranges into `sql` with a message. Skips CTE names, which aren't in the catalog.
pub fn lint(sql: &str, catalog: &SchemaCatalog) -> Vec<(std::ops::Range<usize>, String)> {
    if catalog.tables.is_empty() {
        return vec![]; // not connected, or nothing loaded yet
    }
    let mut out = Vec::new();
    for stmt in crate::split_statements(sql) {
        let text = &sql[stmt.start..stmt.end];
        let toks = tokens(text);
        // `WITH x AS (...)` names a table only this statement has
        let ctes: Vec<String> = toks
            .windows(2)
            .filter(|w| is_kw(w[1].0, "as") && toks.iter().any(|t| is_kw(t.0, "with")))
            .map(|w| w[0].0.to_lowercase())
            .collect();
        let refs = table_refs(text, catalog);
        // `schema.table` in a schema that isn't loaded yet isn't unknown, just not known yet
        let pending = |r: &TableRef| {
            r.name
                .rsplit_once('.')
                .is_some_and(|(schema, _)| !catalog.is_loaded(schema))
        };
        for r in refs
            .iter()
            .filter(|r| r.table.is_none() && !ctes.contains(&r.name.to_lowercase()) && !pending(r))
        {
            out.push((
                stmt.start + r.range.start..stmt.start + r.range.end,
                format!("Unknown table `{}`", r.name),
            ));
        }
        for (tok, range) in &toks {
            let Some((q, col)) = tok
                .replace('"', "")
                .rsplit_once('.')
                .map(|(a, b)| (a.to_string(), b.to_string()))
            else {
                continue;
            };
            // Only alias/table qualifiers this statement declares, so `schema.table` names are left alone
            let Some(t) = refs
                .iter()
                .find(|r| r.qualifier().eq_ignore_ascii_case(&q))
                .and_then(|r| r.table)
            else {
                continue;
            };
            if !col.is_empty()
                && col != "*"
                && !t.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&col))
            {
                out.push((
                    stmt.start + range.start..stmt.start + range.end,
                    format!("`{}` has no column `{col}`", t.table_name),
                ));
            }
        }
    }
    out
}

/// 0 = prefix, 1 = substring, 2 = letters in order; None = no match. Case-insensitive.
pub fn match_score(prefix: &str, label: &str) -> Option<u8> {
    let label = label.to_lowercase();
    if label.starts_with(prefix) {
        return Some(0);
    }
    // One letter matches nearly everything somewhere; only prefixes count until there are two
    if prefix.chars().count() < 2 {
        return None;
    }
    if label.contains(prefix) {
        return Some(1);
    }
    let mut chars = label.chars();
    prefix.chars().all(|p| chars.any(|c| c == p)).then_some(2)
}

const KEYWORDS_STARTING_CLAUSE: &[&str] = &[
    "select", "from", "where", "join", "on", "group", "order", "having", "set", "values", "limit",
];

fn join_candidate(table: &str, on: &str) -> CompletionCandidate {
    CompletionCandidate {
        label: format!("{table} ON {on}"),
        kind: CompletionKind::Table,
        detail: Some("FK join".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::schema::{ColumnSchema, ColumnType, TableSchema};

    #[test]
    fn test_completer_table_and_columns() {
        let catalog = SchemaCatalog {
            default_schema: None,
            unloaded: vec![],
            schemas: vec!["public".to_string()],
            tables: vec![TableSchema {
                schema_name: "public".to_string(),
                table_name: "customers".to_string(),
                is_view: false,
                columns: vec![
                    ColumnSchema {
                        name: "customer_id".to_string(),
                        data_type: ColumnType::Integer,
                        raw_type_name: "int".to_string(),
                        is_nullable: false,
                        is_primary_key: true,
                        default_value: None,
                        ordinal_position: 1,
                    },
                    ColumnSchema {
                        name: "email".to_string(),
                        data_type: ColumnType::Text,
                        raw_type_name: "text".to_string(),
                        is_nullable: false,
                        is_primary_key: false,
                        default_value: None,
                        ordinal_position: 2,
                    },
                ],
                primary_keys: vec!["customer_id".to_string()],
                foreign_keys: vec![],
            }],
        };

        // Complete table prefix "cust"
        let candidates = SchemaCompleter::complete("SELECT * FROM cust", 18, &catalog);
        assert!(candidates
            .iter()
            .any(|c| c.label == "customers" && c.kind == CompletionKind::Table));

        // Complete qualified column "customers.em"
        let col_candidates = SchemaCompleter::complete("SELECT customers.em", 19, &catalog);
        assert!(col_candidates
            .iter()
            .any(|c| c.label == "email" && c.kind == CompletionKind::Column));
    }

    fn shop() -> SchemaCatalog {
        use quarry_core::schema::ForeignKeyRef;
        let col = |n: &str| ColumnSchema {
            name: n.into(),
            data_type: ColumnType::Integer,
            raw_type_name: "INT".into(),
            is_nullable: false,
            is_primary_key: n == "id",
            default_value: None,
            ordinal_position: 1,
        };
        let t = |name: &str, cols: &[&str], fks: Vec<ForeignKeyRef>| TableSchema {
            schema_name: "public".into(),
            table_name: name.into(),
            is_view: false,
            columns: cols.iter().map(|c| col(c)).collect(),
            primary_keys: vec!["id".into()],
            foreign_keys: fks,
        };
        let fk = ForeignKeyRef {
            from_column: "customer_id".into(),
            to_schema: "public".into(),
            to_table: "customers".into(),
            to_column: "id".into(),
        };
        SchemaCatalog {
            schemas: vec!["public".into()],
            tables: vec![
                t("customers", &["id", "email"], vec![]),
                t("orders", &["id", "customer_id", "total"], vec![fk]),
            ],
            default_schema: None,
            ..Default::default()
        }
    }

    fn labels(sql: &str) -> Vec<String> {
        // `|` marks the cursor
        let at = sql.find('|').unwrap();
        SchemaCompleter::complete(&sql.replace('|', ""), at, &shop())
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    #[test]
    fn completes_subquery_alias_columns() {
        assert_eq!(
            labels("select x.| from (select id, total as amount, count(*) from orders) x"),
            ["id", "amount", "count"]
        );
        assert_eq!(
            labels("select x.| from (select * from customers) as x"),
            ["id", "email"]
        );
        assert_eq!(labels("select x.e| from (select c.*, o.total from customers c join orders o on o.customer_id = c.id) x"), ["email"]);
    }

    #[test]
    fn reads_aliases_and_from_lists() {
        let cat = shop();
        let refs = table_refs(
            "select * from public.customers c join orders as o on o.customer_id = c.id, x",
            &cat,
        );
        let got: Vec<_> = refs
            .iter()
            .map(|r| (r.name.as_str(), r.alias.as_deref(), r.table.is_some()))
            .collect();
        assert_eq!(
            got,
            [
                ("public.customers", Some("c"), true),
                ("orders", Some("o"), true)
            ]
        );
        let refs = table_refs("select 1 from customers, nope n where 1", &cat);
        assert_eq!(
            refs.iter()
                .map(|r| (r.name.as_str(), r.table.is_some()))
                .collect::<Vec<_>>(),
            [("customers", true), ("nope", false)]
        );
    }

    #[test]
    fn completes_by_alias_scope_and_clause() {
        // Alias resolves to its table
        assert_eq!(
            labels("select o.| from orders o"),
            ["id", "customer_id", "total"]
        );
        // Columns of tables in scope come first, and others are left out
        let l = labels("select t| from orders o");
        assert_eq!(l[0], "total");
        assert!(!l.contains(&"email".to_string()));
        // After FROM only tables and schemas
        assert_eq!(labels("select * from c|"), ["customers"]);
        assert_eq!(labels("select * from customers, o|"), ["orders"]);
        // schema. lists its tables
        assert_eq!(labels("select * from public.|"), ["customers", "orders"]);
        assert_eq!(labels("select * from public.o|"), ["orders"]);
        assert_eq!(labels("select * from \"public\".|"), ["customers", "orders"]);
        assert_eq!(labels("select * from \"public\".o|"), ["orders"]);
        // table. lists its columns
        assert_eq!(labels("select customers.|"), ["id", "email"]);
        assert_eq!(labels("select \"customers\".|"), ["id", "email"]);
        assert_eq!(labels("select * from customers where customers.|"), ["id", "email"]);
        assert_eq!(labels("select * from customers c where c.|"), ["id", "email"]);
        assert_eq!(labels("select public.customers.|"), ["id", "email"]);
        assert_eq!(labels("select \"public\".\"customers\".|"), ["id", "email"]);
        // schema. lists tables even if catalog.schemas is empty
        let mut cat_no_schemas = shop();
        cat_no_schemas.schemas.clear();
        let at = "select * from public.".len();
        let res: Vec<_> = SchemaCompleter::complete("select * from public.", at, &cat_no_schemas)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(res, ["customers", "orders"]);
        // Only this statement's tables are in scope
        assert_eq!(
            labels("select * from orders;\nselect e| from customers")[0],
            "email"
        );
    }

    #[test]
    fn lints_unknown_tables_and_columns() {
        let sql = "select o.nope, o.total from orders o join ghosts g on 1;\nwith x as (select 1) select * from x, public.customers";
        let got: Vec<_> = lint(sql, &shop())
            .into_iter()
            .map(|(r, m)| (&sql[r], m))
            .collect();
        assert_eq!(
            got,
            [
                ("ghosts", "Unknown table `ghosts`".to_string()),
                ("o.nope", "`orders` has no column `nope`".to_string())
            ]
        );
        assert!(lint("select * from anything", &SchemaCatalog::default()).is_empty());
    }

    #[test]
    fn leaves_schemas_that_are_not_loaded_yet_alone() {
        let mut cat = shop();
        cat.schemas.push("sales".into());
        cat.unloaded = vec!["sales".into()];
        // sales.* can't be judged yet; unknown names in loaded places still are
        let sql = "select * from sales.invoices, \"Sales\".x, ghosts";
        let got: Vec<_> = lint(sql, &cat).into_iter().map(|(r, _)| &sql[r]).collect();
        assert_eq!(got, ["ghosts"]);
        assert_eq!(
            mentioned_unloaded("select * from \"Sales\".", &cat),
            ["sales"]
        );
        assert!(
            mentioned_unloaded("select presales.x from t", &cat).is_empty(),
            "only whole schema names"
        );
        cat.add_schema("sales", vec![]);
        assert!(cat.is_loaded("sales") && mentioned_unloaded("sales.x", &cat).is_empty());
    }

    #[test]
    fn fuzzy_matches_rank_after_prefix() {
        // `cid` isn't a prefix of anything, but its letters run through customer_id
        assert_eq!(labels("select cid| from orders"), ["customer_id"]);
        // A prefix hit beats a substring hit: `id` before `customer_id`
        assert_eq!(labels("select id| from orders")[..2], ["id", "customer_id"]);
    }

    #[test]
    fn suggests_joins_from_foreign_keys() {
        assert_eq!(
            labels("select * from customers c join |")[0],
            "orders ON orders.customer_id = c.id"
        );
        assert_eq!(
            labels("select * from orders o join cu|")[0],
            "customers ON customers.id = o.customer_id"
        );
    }

    #[test]
    fn suggests_on_condition_from_foreign_keys() {
        assert_eq!(
            labels("select * from orders o join customers c on |")[0],
            "c.id = o.customer_id"
        );
    }

    #[test]
    fn usage_boost_breaks_ties() {
        let cat = shop();
        let (sql, offset) = ("select  from orders", 7);
        // Default ranking: columns in scope order
        let default_res = SchemaCompleter::complete(sql, offset, &cat);
        assert_eq!(default_res[0].label, "id");

        // With usage boost: if customer_id was heavily used, it takes top spot over id
        let mut usage = std::collections::HashMap::new();
        usage.insert("customer_id".to_string(), 100);
        let boosted_res = SchemaCompleter::complete_with_usage(sql, offset, &cat, Some(&usage));
        assert_eq!(boosted_res[0].label, "customer_id");
    }

    #[test]
    fn statement_continuations() {
        let cat = shop();
        assert_eq!(
            SchemaCompleter::continuation("select * from customers ", 24, &cat),
            Some("LIMIT 100".to_string())
        );
        assert_eq!(
            SchemaCompleter::continuation("select * from customers where id = 1 ", 37, &cat),
            None
        );
        assert_eq!(
            SchemaCompleter::continuation("update customers set name = 'x' ", 32, &cat),
            Some("WHERE ".to_string())
        );
        assert_eq!(
            SchemaCompleter::continuation("delete from customers ", 22, &cat),
            Some("WHERE ".to_string())
        );
        // GROUP BY auto-synthesis on aggregate + non-aggregate select
        assert_eq!(
            SchemaCompleter::continuation("select customer_id, count(*) from orders ", 41, &cat),
            Some("GROUP BY customer_id".to_string())
        );
        assert_eq!(
            SchemaCompleter::continuation(
                "select customer_id, count(*) from orders group ",
                47,
                &cat
            ),
            Some("BY customer_id".to_string())
        );
        // JOIN <table> ON auto-wiring
        assert_eq!(
            SchemaCompleter::continuation("select * from customers c join orders o ", 40, &cat),
            Some("ON o.customer_id = c.id".to_string())
        );
        // ORDER BY
        assert_eq!(
            SchemaCompleter::continuation("select * from customers order by ", 33, &cat),
            Some("id DESC".to_string())
        );
    }
}

#[cfg(test)]
mod usage_tests {
    use super::{find_usages, rename_identifier};

    #[test]
    fn finds_identifiers_not_strings_or_comments() {
        let sql = "select u.id from public.users u -- users\nwhere u.name = 'users' and \"Users\".x = 1 and users_id = 2";
        let hits: Vec<&str> = find_usages(sql, "users")
            .iter()
            .map(|r| &sql[r.clone()])
            .collect();
        assert_eq!(
            hits,
            vec!["users"],
            "quoted \"Users\" is case-sensitive; users_id is another name"
        );
        assert_eq!(
            find_usages(sql, "Users").len(),
            2,
            "unquoted matches any case, quoted matches exactly"
        );
    }

    #[test]
    fn renames_keeping_quotes() {
        assert_eq!(
            rename_identifier(
                "select * from \"orders\" o join orders x",
                "orders",
                "purchases"
            ),
            "select * from \"purchases\" o join purchases x"
        );
    }
}
