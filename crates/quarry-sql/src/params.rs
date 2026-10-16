// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Query parameters (Q7): `:name` and `$1` placeholders, filled in before running.

use std::collections::HashMap;

/// Byte ranges and names (`:id`, `$1`) of placeholders, skipping strings, comments, dollar quotes and `::casts`
fn scan(sql: &str) -> Vec<(std::ops::Range<usize>, String)> {
    quarry_core::lexer::SqlLexer::new(sql)
        .filter_map(|tok| match tok.kind {
            quarry_core::lexer::TokenKind::Placeholder(p) => Some((tok.span, p.to_string())),
            _ => None,
        })
        .collect()
}

/// Distinct placeholder names in order of first use
pub fn find_params(sql: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, n) in scan(sql) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names
}

/// A typed value as a SQL literal: numbers and booleans as-is, `NULL`, otherwise a quoted string
pub fn literal(value: &str) -> String {
    let v = value.trim();
    if v.eq_ignore_ascii_case("null") {
        "NULL".into()
    } else if v.parse::<f64>().is_ok_and(|f| f.is_finite())
        && !(v.len() > 1 && v.starts_with('0') && !v.starts_with("0."))
    {
        v.to_string()
    } else if v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false") {
        v.to_ascii_uppercase()
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
    }
}

/// Replaces every placeholder with its value's literal
pub fn bind_params(sql: &str, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut last = 0;
    for (range, name) in scan(sql) {
        out.push_str(&sql[last..range.start]);
        out.push_str(&values.get(&name).map(|v| literal(v)).unwrap_or(name));
        last = range.end;
    }
    out.push_str(&sql[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_params_but_not_casts_strings_or_comments() {
        let sql = "select :id::int, ':not', $1, $$ :no $$ -- :nope\n from t where a = :id and b = :name /* :x */";
        assert_eq!(find_params(sql), vec![":id", "$1", ":name"]);
    }

    #[test]
    fn binds_values_as_literals() {
        let values = HashMap::from([
            (":id".to_string(), "42".to_string()),
            (":name".to_string(), "O'Brien".to_string()),
            ("$1".to_string(), "null".to_string()),
        ]);
        assert_eq!(
            bind_params(
                "select * from t where id = :id and name = :name and x is $1 and y = ':id'",
                &values
            ),
            "select * from t where id = 42 and name = 'O''Brien' and x is NULL and y = ':id'"
        );
    }
}
