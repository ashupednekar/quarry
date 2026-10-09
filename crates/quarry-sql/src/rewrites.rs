// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Code actions (⌥↵) on one statement: each returns the statement rewritten

use crate::completer::table_refs;
use quarry_core::session::{is_mutation, is_read};
use quarry_core::SchemaCatalog;

/// (title, rewritten statement) for every action that applies to `stmt`
pub fn rewrites(stmt: &str, catalog: &SchemaCatalog) -> Vec<(&'static str, String)> {
    let body = stmt.trim().trim_end_matches(';').trim_end();
    let semi = if stmt.trim_end().ends_with(';') { ";" } else { "" };
    let lower = body.to_lowercase();
    let is_select = lower.starts_with("select") || lower.starts_with("with");
    let mut out = Vec::new();
    if let Some(expanded) = expand_star(body, catalog) {
        out.push(("Expand * into column names", format!("{expanded}{semi}")));
    }
    if is_select && !lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == "limit") {
        out.push(("Add LIMIT 100", format!("{body}\nLIMIT 100{semi}")));
    }
    if is_read(body) && !lower.starts_with("explain") {
        out.push(("Prefix with EXPLAIN", format!("EXPLAIN {body}{semi}")));
    }
    if is_mutation(body) {
        out.push(("Wrap in a transaction", format!("BEGIN;\n{body};\nCOMMIT;")));
    }
    out
}

/// A SELECT without its own LIMIT gets `LIMIT n` (the editor's row limit); anything else is returned as is
pub fn with_limit(stmt: &str, n: u32) -> String {
    let body = stmt.trim().trim_end_matches(';').trim_end();
    let lower = body.to_lowercase();
    let is_select = lower.starts_with("select") || lower.starts_with("with");
    let has_limit = lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == "limit" || w == "fetch");
    if !is_select || has_limit || is_mutation(body) {
        return stmt.to_string();
    }
    format!("{body}\nLIMIT {n}")
}

/// `SELECT *` (or `SELECT a.*`) → the columns of the tables in FROM/JOIN
fn expand_star(body: &str, catalog: &SchemaCatalog) -> Option<String> {
    let refs: Vec<_> = table_refs(body, catalog).into_iter().filter(|r| r.table.is_some()).collect();
    if refs.is_empty() {
        return None;
    }
    let qualify = refs.len() > 1;
    let cols = |only: Option<&str>| -> Vec<String> {
        refs.iter()
            .filter(|r| only.is_none_or(|q| r.qualifier().eq_ignore_ascii_case(q)))
            .flat_map(|r| {
                let q = r.qualifier();
                r.table.unwrap().columns.iter().map(move |c| if qualify || only.is_some() { format!("{q}.{}", c.name) } else { c.name.clone() })
            })
            .collect()
    };
    let b = body.as_bytes();
    for (i, _) in body.match_indices('*') {
        // Not `count(*)` or `a * b`: the star must stand alone in the select list
        let before = body[..i].trim_end();
        let after = body[i + 1..].trim_start().to_lowercase();
        let (start, only) = match before.strip_suffix('.') {
            Some(q) => {
                let qs = q.rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '"')).map_or(0, |p| p + 1);
                (qs, Some(q[qs..].trim_matches('"')))
            }
            None => (i, None),
        };
        let lead = body[..start].trim_end().to_lowercase();
        let in_list = lead.ends_with("select") || lead.ends_with("distinct") || lead.ends_with(',');
        if !in_list || !(after.starts_with("from") || after.starts_with(',')) || (i > 0 && b[i - 1] == b'(') {
            continue;
        }
        let list = cols(only);
        if list.is_empty() {
            continue;
        }
        return Some(format!("{}{}{}", &body[..start], list.join(", "), &body[i + 1..]));
    }
    None
}

#[cfg(test)]
mod limit_tests {
    #[test]
    fn adds_limit_only_to_unlimited_selects() {
        assert_eq!(super::with_limit("select * from t;", 100), "select * from t\nLIMIT 100");
        assert_eq!(super::with_limit("select * from t limit 5", 100), "select * from t limit 5");
        assert_eq!(super::with_limit("delete from t", 100), "delete from t");
        assert_eq!(super::with_limit("with x as (select 1) select * from x", 10), "with x as (select 1) select * from x\nLIMIT 10");
    }
}

#[cfg(test)]
mod tests {
    use super::rewrites;
    use quarry_core::{ColumnSchema, ColumnType, SchemaCatalog, TableSchema};

    fn cat() -> SchemaCatalog {
        let col = |n: &str| ColumnSchema { name: n.into(), data_type: ColumnType::Integer, raw_type_name: "int".into(), is_nullable: true, is_primary_key: false, default_value: None, ordinal_position: 1 };
        let t = |n: &str, c: &[&str]| TableSchema { schema_name: "public".into(), table_name: n.into(), is_view: false, columns: c.iter().map(|c| col(c)).collect(), primary_keys: vec![], foreign_keys: vec![] };
        SchemaCatalog { schemas: vec!["public".into()], tables: vec![t("users", &["id", "email"]), t("orders", &["id", "user_id"])] }
    }

    fn get(stmt: &str, title: &str) -> Option<String> {
        rewrites(stmt, &cat()).into_iter().find(|(t, _)| *t == title).map(|(_, s)| s)
    }

    #[test]
    fn expands_star() {
        let x = "Expand * into column names";
        assert_eq!(get("select * from users;", x).unwrap(), "select id, email from users;");
        assert_eq!(get("select * from users u join orders o on o.user_id = u.id", x).unwrap(), "select u.id, u.email, o.id, o.user_id from users u join orders o on o.user_id = u.id");
        assert_eq!(get("select o.*, u.email from orders o join users u on 1", x).unwrap(), "select o.id, o.user_id, u.email from orders o join users u on 1");
        assert_eq!(get("select count(*) from users", x), None);
        assert_eq!(get("select * from nowhere", x), None);
    }

    #[test]
    fn offers_the_right_actions() {
        assert_eq!(get("select 1 from users;", "Add LIMIT 100").unwrap(), "select 1 from users\nLIMIT 100;");
        assert_eq!(get("select 1 from users limit 5", "Add LIMIT 100"), None);
        assert_eq!(get("select 1", "Prefix with EXPLAIN").unwrap(), "EXPLAIN select 1");
        assert_eq!(get("delete from users;", "Wrap in a transaction").unwrap(), "BEGIN;\ndelete from users;\nCOMMIT;");
        assert_eq!(get("select 1", "Wrap in a transaction"), None);
    }
}
