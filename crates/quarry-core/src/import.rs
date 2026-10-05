// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>


//! CSV / JSON import (X4-X8): decode, parse, infer types, and load in one transaction.

use crate::edits::quote_ident;
use crate::session::DatabaseSession;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoding {
    Utf8,
    Latin1,
}

/// Decodes file bytes: strips a UTF-8 BOM, and falls back to Latin-1 when the bytes aren't UTF-8.
pub fn decode(bytes: &[u8], forced: Option<Encoding>) -> (String, Encoding) {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match forced {
        Some(Encoding::Latin1) => (latin1(bytes), Encoding::Latin1),
        _ => match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_string(), Encoding::Utf8),
            Err(_) => (latin1(bytes), Encoding::Latin1),
        },
    }
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Parsed rows. `None` is NULL.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// Picks the most frequent of `,` `;` tab in the first line
pub fn guess_delimiter(text: &str) -> char {
    let first = text.lines().next().unwrap_or("");
    [',', ';', '\t'].into_iter().max_by_key(|d| first.matches(*d).count()).unwrap_or(',')
}

/// RFC 4180 CSV: quoted fields, doubled quotes, embedded newlines, CRLF.
pub fn parse_csv(text: &str, delimiter: char, header: bool, empty_is_null: bool) -> ImportData {
    let mut records: Vec<Vec<String>> = Vec::new();
    let (mut record, mut field) = (Vec::new(), String::new());
    let (mut in_quotes, mut any) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                c => field.push(c),
            }
        } else if c == '"' && field.is_empty() {
            in_quotes = true;
        } else if c == delimiter {
            record.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            record.push(std::mem::take(&mut field));
            records.push(std::mem::take(&mut record));
            any = false;
        } else {
            field.push(c);
        }
    }
    if any {
        record.push(field);
        records.push(record);
    }
    records.retain(|r| !(r.len() == 1 && r[0].is_empty()));

    let width = records.iter().map(Vec::len).max().unwrap_or(0);
    let headers = if header && !records.is_empty() {
        let mut h = records.remove(0);
        h.resize_with(width, String::new);
        h.iter().enumerate().map(|(i, n)| if n.trim().is_empty() { format!("column_{}", i + 1) } else { n.trim().to_string() }).collect()
    } else {
        (1..=width).map(|i| format!("column_{i}")).collect()
    };
    let rows = records
        .into_iter()
        .map(|r| {
            let mut r: Vec<Option<String>> = r.into_iter().map(|v| if empty_is_null && v.is_empty() { None } else { Some(v) }).collect();
            r.resize(width, None);
            r
        })
        .collect();
    ImportData { headers, rows }
}

/// JSON array of objects. Keys become columns in order of first appearance; nested values stay JSON text.
pub fn parse_json(text: &str) -> Result<ImportData, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("Not valid JSON: {e}"))?;
    let items = value.as_array().ok_or("Expected a JSON array of objects")?;
    let mut headers: Vec<String> = Vec::new();
    for item in items {
        for key in item.as_object().ok_or("Every array element must be an object")?.keys() {
            if !headers.contains(key) {
                headers.push(key.clone());
            }
        }
    }
    let rows = items
        .iter()
        .map(|item| {
            headers
                .iter()
                .map(|h| match item.get(h) {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s)) => Some(s.clone()),
                    Some(v) => Some(v.to_string()),
                })
                .collect()
        })
        .collect();
    Ok(ImportData { headers, rows })
}

/// Guesses a column type from its values (X6)
pub fn infer_types(data: &ImportData, postgres: bool) -> Vec<String> {
    (0..data.headers.len())
        .map(|i| {
            let values: Vec<&str> = data.rows.iter().filter_map(|r| r.get(i)?.as_deref()).collect();
            let all = |f: fn(&str) -> bool| !values.is_empty() && values.iter().all(|v| f(v));
            let ty = if all(|v| v.parse::<i64>().is_ok() && numeric(v)) {
                if postgres { "BIGINT" } else { "INTEGER" }
            } else if all(|v| v.parse::<f64>().is_ok() && numeric(v)) {
                if postgres { "DOUBLE PRECISION" } else { "REAL" }
            } else if all(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "false")) {
                "BOOLEAN"
            } else if postgres && all(is_date) {
                "DATE"
            } else if postgres && all(is_timestamp) {
                "TIMESTAMP"
            } else {
                "TEXT"
            };
            ty.to_string()
        })
        .collect()
}

/// Leading zeros (zip codes, ids) mean text: a number would drop them
fn numeric(v: &str) -> bool {
    !(v.len() > 1 && v.starts_with('0') && !v.starts_with("0."))
}

fn is_date(v: &str) -> bool {
    chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok()
}

fn is_timestamp(v: &str) -> bool {
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
        .iter()
        .any(|f| chrono::NaiveDateTime::parse_from_str(v, f).is_ok())
        || chrono::DateTime::parse_from_rfc3339(v).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnConflict {
    /// Plain INSERT; a conflict is an error
    Error,
    /// INSERT … ON CONFLICT DO NOTHING
    SkipDuplicates,
}

#[derive(Debug, Clone)]
pub struct ImportPlan {
    /// Quoted, schema-qualified target table
    pub table: String,
    /// `Some((column, type))` creates the table first (same transaction)
    pub create: Option<Vec<(String, String)>>,
    /// (source column index, target column name)
    pub mapping: Vec<(usize, String)>,
    pub on_conflict: OnConflict,
    /// Stop and roll back at the first failing row (the table is left untouched)
    pub all_or_nothing: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub inserted: usize,
    pub skipped: usize,
    /// (1-based data row, error) — rows that failed
    pub failures: Vec<(usize, String)>,
    pub cancelled: bool,
    /// All-or-nothing import that failed: nothing was written
    pub rolled_back: bool,
}

/// Loads rows inside one transaction (or a savepoint when one is already open).
pub async fn run(
    session: &dyn DatabaseSession,
    data: &ImportData,
    plan: &ImportPlan,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<ImportReport, String> {
    let exec = |sql: String| async move { session.execute_quiet(&sql).await.map_err(|e| e.to_string()) };
    let nested = session.in_transaction();
    exec(if nested { "SAVEPOINT quarry_import".into() } else { "BEGIN".into() }).await?;
    let abort = |nested: bool| exec(if nested { "ROLLBACK TO SAVEPOINT quarry_import".into() } else { "ROLLBACK".into() });

    if let Some(columns) = &plan.create {
        let defs: Vec<String> = columns.iter().map(|(c, t)| format!("{} {}", quote_ident(c), t)).collect();
        if let Err(e) = exec(format!("CREATE TABLE {} ({})", plan.table, defs.join(", "))).await {
            abort(nested).await.ok();
            return Err(e);
        }
    }

    let cols: Vec<String> = plan.mapping.iter().map(|(_, c)| quote_ident(c)).collect();
    let suffix = if plan.on_conflict == OnConflict::SkipDuplicates { " ON CONFLICT DO NOTHING" } else { "" };
    let mut report = ImportReport::default();
    for (i, row) in data.rows.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            abort(nested).await.ok();
            report.cancelled = true;
            return Ok(report);
        }
        let values: Vec<String> = plan
            .mapping
            .iter()
            .map(|(src, _)| match row.get(*src).and_then(|v| v.as_deref()) {
                None => "NULL".into(),
                Some(v) => format!("'{}'", v.replace('\'', "''")),
            })
            .collect();
        let insert = format!("INSERT INTO {} ({}) VALUES ({}){}", plan.table, cols.join(", "), values.join(", "), suffix);
        if !plan.all_or_nothing {
            exec("SAVEPOINT quarry_row".into()).await?;
        }
        match session.execute_quiet(&insert).await.map_err(|e| e.to_string()) {
            // ON CONFLICT DO NOTHING reports 0 affected rows for a duplicate
            Ok(r) if r.rows_affected == 0 => report.skipped += 1,
            Ok(_) => report.inserted += 1,
            Err(e) if plan.all_or_nothing => {
                abort(nested).await.ok();
                report.failures.push((i + 1, e));
                report.rolled_back = true;
                return Ok(report);
            }
            Err(e) => {
                exec("ROLLBACK TO SAVEPOINT quarry_row".into()).await?;
                report.failures.push((i + 1, e));
            }
        }
        if !plan.all_or_nothing {
            exec("RELEASE SAVEPOINT quarry_row".into()).await?;
        }
        progress.store(i + 1, Ordering::Relaxed);
    }
    exec(if nested { "RELEASE SAVEPOINT quarry_import".into() } else { "COMMIT".into() }).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DbValue, SqliteSession};

    #[test]
    fn csv_handles_quotes_newlines_crlf_and_nulls() {
        let text = "id,name,note\r\n1,\"Smith, J\",\"says \"\"hi\"\"\nthere\"\r\n2,,x\r\n";
        let d = parse_csv(text, guess_delimiter(text), true, true);
        assert_eq!(d.headers, vec!["id", "name", "note"]);
        assert_eq!(d.rows[0], vec![Some("1".into()), Some("Smith, J".into()), Some("says \"hi\"\nthere".into())]);
        assert_eq!(d.rows[1], vec![Some("2".into()), None, Some("x".into())]);
    }

    #[test]
    fn decodes_bom_and_latin1() {
        assert_eq!(decode(b"\xEF\xBB\xBFa,b", None), ("a,b".into(), Encoding::Utf8));
        assert_eq!(decode(b"caf\xE9", None), ("café".into(), Encoding::Latin1));
    }

    #[test]
    fn infers_types_and_keeps_leading_zeros_as_text() {
        let d = parse_csv("n,f,b,zip,when\n1,1.5,true,01234,2026-01-02\n22,2,false,99999,2026-03-04\n", ',', true, true);
        assert_eq!(infer_types(&d, true), vec!["BIGINT", "DOUBLE PRECISION", "BOOLEAN", "TEXT", "DATE"]);
    }

    #[test]
    fn json_objects_with_missing_keys() {
        let d = parse_json(r#"[{"a":1,"b":"x"},{"b":null,"c":{"k":true}}]"#).unwrap();
        assert_eq!(d.headers, vec!["a", "b", "c"]);
        assert_eq!(d.rows[1], vec![None, None, Some(r#"{"k":true}"#.into())]);
    }

    async fn count(s: &SqliteSession, t: &str) -> DbValue {
        s.execute(&format!("SELECT count(*) FROM {t}")).await.map(|r| r.rows[0][0].clone()).unwrap_or(DbValue::Null)
    }

    #[tokio::test]
    async fn all_or_nothing_leaves_table_untouched_and_skip_mode_reports_rows() {
        let s = SqliteSession::open_in_memory().unwrap();
        s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)").await.unwrap();
        let d = parse_csv("id,v\n1,a\n2,\n3,c\n", ',', true, true); // row 2 violates NOT NULL
        let plan = ImportPlan {
            table: "\"t\"".into(),
            create: None,
            mapping: vec![(0, "id".into()), (1, "v".into())],
            on_conflict: OnConflict::Error,
            all_or_nothing: true,
        };
        let (p, c) = (AtomicUsize::new(0), AtomicBool::new(false));
        let r = run(&s, &d, &plan, &p, &c).await.unwrap();
        assert!(r.rolled_back && r.failures[0].0 == 2);
        assert_eq!(count(&s, "t").await, DbValue::Int(0));

        let r = run(&s, &d, &ImportPlan { all_or_nothing: false, ..plan.clone() }, &p, &c).await.unwrap();
        assert_eq!((r.inserted, r.failures.len(), r.failures[0].0), (2, 1, 2));
        assert_eq!(count(&s, "t").await, DbValue::Int(2));

        // Re-importing with skip-duplicates doesn't fail on the existing ids
        let r = run(&s, &d, &ImportPlan { all_or_nothing: false, on_conflict: OnConflict::SkipDuplicates, ..plan }, &p, &c).await.unwrap();
        assert_eq!((r.inserted, r.skipped, r.failures.len()), (0, 2, 1));
        assert_eq!(count(&s, "t").await, DbValue::Int(2));
    }

    #[tokio::test]
    async fn creates_new_table_in_the_same_transaction() {
        let s = SqliteSession::open_in_memory().unwrap();
        let d = parse_csv("n,label\n1,a\n2,b\n", ',', true, true);
        let plan = ImportPlan {
            table: "\"fresh\"".into(),
            create: Some(vec![("n".into(), "INTEGER".into()), ("label".into(), "TEXT".into())]),
            mapping: vec![(0, "n".into()), (1, "label".into())],
            on_conflict: OnConflict::Error,
            all_or_nothing: true,
        };
        run(&s, &d, &plan, &AtomicUsize::new(0), &AtomicBool::new(false)).await.unwrap();
        assert_eq!(count(&s, "fresh").await, DbValue::Int(2));
    }
}
