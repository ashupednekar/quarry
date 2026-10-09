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
    "SELECT", "FROM", "WHERE", "JOIN", "LEFT JOIN", "RIGHT JOIN", "INNER JOIN",
    "ON", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET", "INSERT INTO",
    "VALUES", "UPDATE", "SET", "DELETE FROM", "CREATE TABLE", "DROP TABLE",
    "ALTER TABLE", "AND", "OR", "NOT", "IN", "IS NULL", "IS NOT NULL",
    "BETWEEN", "LIKE", "AS", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
    "CASE", "WHEN", "THEN", "ELSE", "END", "EXPLAIN ANALYZE", "BEGIN", "COMMIT", "ROLLBACK",
];

/// Words that end a `FROM a x, b y` list, so they are never read as an alias
const RESERVED: &[&str] = &[
    "where", "on", "join", "left", "right", "inner", "outer", "full", "cross", "natural", "group", "order",
    "having", "limit", "offset", "union", "except", "intersect", "set", "values", "using", "as", "select",
    "from", "returning", "window", "lateral", "and", "or", "into", "default",
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
        self.alias.clone().unwrap_or_else(|| self.name.rsplit('.').next().unwrap_or(&self.name).to_string())
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
            while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'.' | b'"') || b[i] >= 0x80) {
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
        let opens_list = is_kw(t, "from") || is_kw(t, "join") || is_kw(t, "update") || is_kw(t, "into");
        i += 1;
        if !opens_list {
            continue;
        }
        // `FROM a x, b AS y` — keep reading refs while commas follow
        loop {
            let Some((name, range)) = toks.get(i).cloned() else { break };
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
            refs.push(TableRef { table: lookup(&name, catalog), name, range, alias });
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
pub fn subquery_columns(stmt: &str, alias: &str, catalog: &SchemaCatalog) -> Option<Vec<(String, String)>> {
    let toks = tokens(stmt);
    // `) x` or `) AS x`, then back to the matching `(`
    let close = (0..toks.len()).find(|&i| {
        toks[i].0 == ")" && {
            let mut j = i + 1;
            if toks.get(j).is_some_and(|(t, _)| is_kw(t, "as")) {
                j += 1;
            }
            toks.get(j).is_some_and(|(t, _)| t.replace('"', "").eq_ignore_ascii_case(alias))
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
        let words: Vec<&str> = item.iter().map(|(t, _)| *t).filter(|t| !matches!(*t, "(" | ")")).collect();
        let star_of = match words.last() {
            None => Some(None),                                         // `*` (not a token)
            Some(w) if w.ends_with('.') => Some(Some(w.trim_end_matches('.'))), // `t.*`
            _ => None,
        };
        if let Some(q) = star_of {
            for r in refs.iter().filter(|r| q.is_none_or(|q| r.qualifier().eq_ignore_ascii_case(q))) {
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
            out.push((n.rsplit('.').next().unwrap_or(n).replace('"', ""), "subquery".into()));
        }
    }
    Some(out)
}

/// The table an identifier qualifier means in this statement: an alias, a table name, or `schema.table`
pub fn resolve_qualifier<'a>(qualifier: &str, refs: &[TableRef<'a>], catalog: &'a SchemaCatalog) -> Option<&'a TableSchema> {
    refs.iter()
        .find(|r| r.qualifier().eq_ignore_ascii_case(qualifier))
        .and_then(|r| r.table)
        .or_else(|| lookup(qualifier, catalog))
}

impl SchemaCompleter {
    /// Best matches first: prefix, then substring, then letters in order (`cid` → `customer_id`).
    /// Ties keep the scope order (columns in scope, keywords, tables…).
    pub fn complete(sql: &str, cursor_offset: usize, catalog: &SchemaCatalog) -> Vec<CompletionCandidate> {
        let prefix = Self::extract_prefix(sql, cursor_offset.min(sql.len())).to_lowercase();
        let mut out = Self::candidates(sql, cursor_offset, catalog);
        out.sort_by_key(|c| match_score(&prefix, &c.label).unwrap_or(u8::MAX));
        out.truncate(50);
        out
    }

    fn candidates(sql: &str, cursor_offset: usize, catalog: &SchemaCatalog) -> Vec<CompletionCandidate> {
        let cursor_offset = cursor_offset.min(sql.len());
        // Scope is the statement at the cursor, not the whole script
        let (stmt, at) = match crate::find_statement_at_offset(sql, cursor_offset) {
            Some(s) if cursor_offset >= s.start && cursor_offset <= s.end => (&sql[s.start..s.end], cursor_offset - s.start),
            _ => (sql, cursor_offset),
        };
        let prefix = Self::extract_prefix(stmt, at);
        let p = prefix.to_lowercase();
        let starts = |s: &str| match_score(&p, s).is_some();
        // Keywords only by prefix: fuzzy keyword hits are noise
        let kw_starts = |s: &str| s.to_lowercase().starts_with(&p);
        let refs = table_refs(stmt, catalog);
        let mut out = Vec::new();
        let column = |c: &quarry_core::ColumnSchema, detail: String| CompletionCandidate { label: c.name.clone(), kind: CompletionKind::Column, detail: Some(detail) };

        // 1. `x.` — columns of the aliased/named table, or tables of a schema
        if let Some(q) = Self::extract_qualifier(stmt, at) {
            if let Some(t) = resolve_qualifier(&q, &refs, catalog) {
                return t.columns.iter().filter(|c| starts(&c.name)).map(|c| column(c, c.raw_type_name.to_lowercase())).collect();
            }
            if let Some(cols) = subquery_columns(stmt, &q, catalog) {
                return cols
                    .into_iter()
                    .filter(|(n, _)| starts(n))
                    .map(|(label, detail)| CompletionCandidate { label, kind: CompletionKind::Column, detail: Some(detail) })
                    .collect();
            }
            if catalog.schemas.iter().any(|s| s.eq_ignore_ascii_case(&q)) {
                return catalog
                    .tables
                    .iter()
                    .filter(|t| t.schema_name.eq_ignore_ascii_case(&q) && starts(&t.table_name))
                    .map(|t| CompletionCandidate { label: t.table_name.clone(), kind: CompletionKind::Table, detail: Some(format!("{} columns", t.columns.len())) })
                    .collect();
            }
        }

        // Which clause is the cursor in? Tables after FROM/JOIN/UPDATE/INTO (or a comma in that list)
        let before: Vec<_> = tokens(&stmt[..at - prefix.len()]).into_iter().map(|(t, _)| t).collect();
        let last = before.last().copied().unwrap_or("");
        let in_from_list = last == "," && before.iter().rev().find(|t| KEYWORDS_STARTING_CLAUSE.iter().any(|k| is_kw(t, k))).is_some_and(|t| is_kw(t, "from"));
        let wants_table = ["from", "join", "update", "into", "table"].iter().any(|k| is_kw(last, k)) || in_from_list;

        if wants_table {
            // 2a. After JOIN: whole `t ON t.a = x.b` clauses from foreign keys, both directions
            if is_kw(last, "join") {
                for r in refs.iter().filter_map(|r| r.table.map(|t| (r.qualifier(), t))) {
                    let (q, from) = r;
                    for fk in &from.foreign_keys {
                        if starts(&fk.to_table) {
                            out.push(join_candidate(&fk.to_table, &format!("{}.{} = {q}.{}", fk.to_table, fk.to_column, fk.from_column)));
                        }
                    }
                    for t in catalog.tables.iter().filter(|t| starts(&t.table_name)) {
                        for fk in t.foreign_keys.iter().filter(|f| f.to_table.eq_ignore_ascii_case(&from.table_name)) {
                            out.push(join_candidate(&t.table_name, &format!("{}.{} = {q}.{}", t.table_name, fk.from_column, fk.to_column)));
                        }
                    }
                }
            }
            out.extend(catalog.tables.iter().filter(|t| starts(&t.table_name)).map(|t| CompletionCandidate {
                label: t.table_name.clone(),
                kind: CompletionKind::Table,
                detail: Some(format!("{}{} · {} columns", t.schema_name, if t.is_view { " view" } else { "" }, t.columns.len())),
            }));
            out.extend(catalog.schemas.iter().filter(|s| starts(s)).map(|s| CompletionCandidate { label: s.clone(), kind: CompletionKind::Schema, detail: Some("schema".into()) }));
            return out;
        }

        // 2b. Elsewhere: columns of the tables in scope first, then keywords, tables, and other columns
        for r in &refs {
            if let Some(t) = r.table {
                for c in t.columns.iter().filter(|c| starts(&c.name)) {
                    if !out.iter().any(|o: &CompletionCandidate| o.label == c.name) {
                        out.push(column(c, format!("{}.{} {}", r.qualifier(), c.name, c.raw_type_name.to_lowercase())));
                    }
                }
            }
        }
        out.extend(KEYWORDS.iter().filter(|k| kw_starts(k)).map(|k| CompletionCandidate { label: k.to_string(), kind: CompletionKind::Keyword, detail: Some("keyword".into()) }));
        out.extend(catalog.tables.iter().filter(|t| starts(&t.table_name)).map(|t| CompletionCandidate {
            label: t.table_name.clone(),
            kind: CompletionKind::Table,
            detail: Some(format!("{} columns", t.columns.len())),
        }));
        // Columns from anywhere only while nothing is in scope yet (`SELECT em|` before FROM is typed)
        if refs.iter().all(|r| r.table.is_none()) {
            for t in &catalog.tables {
                for c in t.columns.iter().filter(|c| starts(&c.name)) {
                    if !out.iter().any(|o| o.label == c.name && o.kind == CompletionKind::Column) {
                        out.push(column(c, format!("{}.{} {}", t.table_name, c.name, c.raw_type_name.to_lowercase())));
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

        if before_prefix.ends_with('.') {
            let trimmed = &before_prefix[..before_prefix.len() - 1];
            let start = trimmed
                .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|idx| idx + 1)
                .unwrap_or(0);
            let qualifier = &trimmed[start..];
            if !qualifier.is_empty() {
                return Some(qualifier.to_string());
            }
        }
        None
    }
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
        let ctes: Vec<String> = toks.windows(2).filter(|w| is_kw(w[1].0, "as") && toks.iter().any(|t| is_kw(t.0, "with"))).map(|w| w[0].0.to_lowercase()).collect();
        let refs = table_refs(text, catalog);
        for r in refs.iter().filter(|r| r.table.is_none() && !ctes.contains(&r.name.to_lowercase())) {
            out.push((stmt.start + r.range.start..stmt.start + r.range.end, format!("Unknown table `{}`", r.name)));
        }
        for (tok, range) in &toks {
            let Some((q, col)) = tok.replace('"', "").rsplit_once('.').map(|(a, b)| (a.to_string(), b.to_string())) else { continue };
            // Only alias/table qualifiers this statement declares, so `schema.table` names are left alone
            let Some(t) = refs.iter().find(|r| r.qualifier().eq_ignore_ascii_case(&q)).and_then(|r| r.table) else { continue };
            if !col.is_empty() && col != "*" && !t.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&col)) {
                out.push((stmt.start + range.start..stmt.start + range.end, format!("`{}` has no column `{col}`", t.table_name)));
            }
        }
    }
    out
}

/// 0 = prefix, 1 = substring, 2 = letters in order; None = no match. Case-insensitive.
fn match_score(prefix: &str, label: &str) -> Option<u8> {
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

const KEYWORDS_STARTING_CLAUSE: &[&str] = &["select", "from", "where", "join", "on", "group", "order", "having", "set", "values", "limit"];

fn join_candidate(table: &str, on: &str) -> CompletionCandidate {
    CompletionCandidate { label: format!("{table} ON {on}"), kind: CompletionKind::Table, detail: Some("join on foreign key".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::schema::{ColumnSchema, ColumnType, TableSchema};

    #[test]
    fn test_completer_table_and_columns() {
        let catalog = SchemaCatalog {
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
        assert!(candidates.iter().any(|c| c.label == "customers" && c.kind == CompletionKind::Table));

        // Complete qualified column "customers.em"
        let col_candidates = SchemaCompleter::complete("SELECT customers.em", 19, &catalog);
        assert!(col_candidates.iter().any(|c| c.label == "email" && c.kind == CompletionKind::Column));
    }

    fn shop() -> SchemaCatalog {
        use quarry_core::schema::ForeignKeyRef;
        let col = |n: &str| ColumnSchema { name: n.into(), data_type: ColumnType::Integer, raw_type_name: "INT".into(), is_nullable: false, is_primary_key: n == "id", default_value: None, ordinal_position: 1 };
        let t = |name: &str, cols: &[&str], fks: Vec<ForeignKeyRef>| TableSchema {
            schema_name: "public".into(),
            table_name: name.into(),
            is_view: false,
            columns: cols.iter().map(|c| col(c)).collect(),
            primary_keys: vec!["id".into()],
            foreign_keys: fks,
        };
        let fk = ForeignKeyRef { from_column: "customer_id".into(), to_schema: "public".into(), to_table: "customers".into(), to_column: "id".into() };
        SchemaCatalog {
            schemas: vec!["public".into()],
            tables: vec![t("customers", &["id", "email"], vec![]), t("orders", &["id", "customer_id", "total"], vec![fk])],
        }
    }

    fn labels(sql: &str) -> Vec<String> {
        // `|` marks the cursor
        let at = sql.find('|').unwrap();
        SchemaCompleter::complete(&sql.replace('|', ""), at, &shop()).into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn completes_subquery_alias_columns() {
        assert_eq!(labels("select x.| from (select id, total as amount, count(*) from orders) x"), ["id", "amount", "count"]);
        assert_eq!(labels("select x.| from (select * from customers) as x"), ["id", "email"]);
        assert_eq!(labels("select x.e| from (select c.*, o.total from customers c join orders o on o.customer_id = c.id) x"), ["email"]);
    }

    #[test]
    fn reads_aliases_and_from_lists() {
        let cat = shop();
        let refs = table_refs("select * from public.customers c join orders as o on o.customer_id = c.id, x", &cat);
        let got: Vec<_> = refs.iter().map(|r| (r.name.as_str(), r.alias.as_deref(), r.table.is_some())).collect();
        assert_eq!(got, [("public.customers", Some("c"), true), ("orders", Some("o"), true)]);
        let refs = table_refs("select 1 from customers, nope n where 1", &cat);
        assert_eq!(refs.iter().map(|r| (r.name.as_str(), r.table.is_some())).collect::<Vec<_>>(), [("customers", true), ("nope", false)]);
    }

    #[test]
    fn completes_by_alias_scope_and_clause() {
        // Alias resolves to its table
        assert_eq!(labels("select o.| from orders o"), ["id", "customer_id", "total"]);
        // Columns of tables in scope come first, and others are left out
        let l = labels("select t| from orders o");
        assert_eq!(l[0], "total");
        assert!(!l.contains(&"email".to_string()));
        // After FROM only tables and schemas
        assert_eq!(labels("select * from c|"), ["customers"]);
        assert_eq!(labels("select * from customers, o|"), ["orders"]);
        // schema. lists its tables
        assert_eq!(labels("select * from public.o|"), ["orders"]);
        // Only this statement's tables are in scope
        assert_eq!(labels("select * from orders;\nselect e| from customers")[0], "email");
    }

    #[test]
    fn lints_unknown_tables_and_columns() {
        let sql = "select o.nope, o.total from orders o join ghosts g on 1;\nwith x as (select 1) select * from x, public.customers";
        let got: Vec<_> = lint(sql, &shop()).into_iter().map(|(r, m)| (&sql[r], m)).collect();
        assert_eq!(got, [("ghosts", "Unknown table `ghosts`".to_string()), ("o.nope", "`orders` has no column `nope`".to_string())]);
        assert!(lint("select * from anything", &SchemaCatalog::default()).is_empty());
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
        assert_eq!(labels("select * from customers c join |")[0], "orders ON orders.customer_id = c.id");
        assert_eq!(labels("select * from orders o join cu|")[0], "customers ON customers.id = o.customer_id");
    }
}
