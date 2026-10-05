// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! Query parameters (Q7): `:name` and `$1` placeholders, filled in before running.

use std::collections::HashMap;

/// Byte ranges and names (`:id`, `$1`) of placeholders, skipping strings, comments, dollar quotes and `::casts`
fn scan(sql: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' => {
                let q = b[i];
                i += 1;
                while i < b.len() && !(b[i] == q && b.get(i + 1) != Some(&q)) {
                    i += if b[i] == q { 2 } else { 1 };
                }
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            b':' if b.get(i + 1) == Some(&b':') => i += 2, // ::cast
            b':' if i > 0 && b[i - 1] == b':' => i += 1,
            b':' if b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') => {
                let start = i;
                i += 1;
                while i < b.len() && word(b[i]) {
                    i += 1;
                }
                out.push((start..i, sql[start..i].to_string()));
            }
            b'$' if b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                let start = i;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                out.push((start..i, sql[start..i].to_string()));
            }
            b'$' => {
                // Dollar-quoted body: $tag$ ... $tag$
                let tag_end = sql[i + 1..].find('$').map(|e| i + 1 + e);
                match tag_end.filter(|&e| b[i + 1..e].iter().all(|c| word(*c))) {
                    Some(e) => {
                        let tag = &sql[i..=e];
                        i = sql[e + 1..].find(tag).map_or(b.len(), |p| e + 1 + p + tag.len());
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    out
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
    } else if v.parse::<f64>().is_ok() || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false") {
        v.to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
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
        let values = HashMap::from([(":id".to_string(), "42".to_string()), (":name".to_string(), "O'Brien".to_string()), ("$1".to_string(), "null".to_string())]);
        assert_eq!(
            bind_params("select * from t where id = :id and name = :name and x is $1 and y = ':id'", &values),
            "select * from t where id = 42 and name = 'O''Brien' and x is NULL and y = ':id'"
        );
    }
}
