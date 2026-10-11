// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Writes result rows as CSV, JSON or SQL INSERT statements (X1, X2). Streams in chunks.

use crate::edits::quote_ident;
use crate::value::DbValue;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportFormat {
    Csv,
    Json,
    Sql,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub delimiter: char,
    pub header: bool,
    /// How NULL is written in CSV (JSON uses `null`, SQL uses `NULL`)
    pub null_text: String,
    /// Indices of the columns to write, in order; `None` = all
    pub columns: Option<Vec<usize>>,
    /// Table name used in SQL INSERTs (already quoted/qualified, or a plain name)
    pub table: String,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Csv,
            delimiter: ',',
            header: true,
            null_text: String::new(),
            columns: None,
            table: "exported".into(),
        }
    }
}

/// Streaming writer: `begin`, any number of `rows`, then `finish`.
pub struct Exporter<W: Write> {
    out: W,
    opts: ExportOptions,
    names: Vec<String>,
    picked: Vec<usize>,
    written: usize,
}

impl<W: Write> Exporter<W> {
    pub fn begin(mut out: W, columns: &[String], opts: ExportOptions) -> io::Result<Self> {
        let picked = opts
            .columns
            .clone()
            .unwrap_or_else(|| (0..columns.len()).collect());
        let names: Vec<String> = picked
            .iter()
            .filter_map(|&i| columns.get(i).cloned())
            .collect();
        match opts.format {
            ExportFormat::Csv if opts.header => {
                let line: Vec<String> =
                    names.iter().map(|n| csv_field(n, opts.delimiter)).collect();
                writeln!(out, "{}", line.join(&opts.delimiter.to_string()))?;
            }
            ExportFormat::Json => write!(out, "[")?,
            _ => {}
        }
        Ok(Self {
            out,
            opts,
            names,
            picked,
            written: 0,
        })
    }

    pub fn rows(&mut self, rows: &[Vec<DbValue>]) -> io::Result<()> {
        let d = self.opts.delimiter.to_string();
        for row in rows {
            let vals: Vec<&DbValue> = self
                .picked
                .iter()
                .map(|&i| row.get(i).unwrap_or(&DbValue::Null))
                .collect();
            match self.opts.format {
                ExportFormat::Csv => {
                    let line: Vec<String> = vals
                        .iter()
                        .map(|v| match v {
                            DbValue::Null => self.opts.null_text.clone(),
                            v => csv_field(&plain_text(v), self.opts.delimiter),
                        })
                        .collect();
                    writeln!(self.out, "{}", line.join(&d))?;
                }
                ExportFormat::Json => {
                    let fields: Vec<String> = self
                        .names
                        .iter()
                        .zip(&vals)
                        .map(|(n, v)| {
                            format!("{}:{}", serde_json::Value::String(n.clone()), json_value(v))
                        })
                        .collect();
                    let sep = if self.written == 0 { "" } else { "," };
                    write!(self.out, "{sep}\n  {{{}}}", fields.join(","))?;
                }
                ExportFormat::Sql => {
                    let cols: Vec<String> = self.names.iter().map(|n| quote_ident(n)).collect();
                    let values: Vec<String> = vals.iter().map(|v| v.to_sql_literal()).collect();
                    writeln!(
                        self.out,
                        "INSERT INTO {} ({}) VALUES ({});",
                        self.opts.table,
                        cols.join(", "),
                        values.join(", ")
                    )?;
                }
            }
            self.written += 1;
        }
        Ok(())
    }

    pub fn written(&self) -> usize {
        self.written
    }

    pub fn finish(mut self) -> io::Result<usize> {
        if self.opts.format == ExportFormat::Json {
            writeln!(self.out, "{}]", if self.written == 0 { "" } else { "\n" })?;
        }
        self.out.flush()?;
        Ok(self.written)
    }
}

/// Text form used in CSV: JSON as JSON, blobs as hex, timestamps as RFC 3339
fn plain_text(v: &DbValue) -> String {
    match v {
        DbValue::Json(j) => j.to_string(),
        DbValue::Blob(b) => format!(
            "\\x{}",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ),
        DbValue::Timestamp(t) => t.to_rfc3339(),
        v => v.to_display_string(),
    }
}

fn csv_field(s: &str, delimiter: char) -> String {
    if s.contains(delimiter)
        || s.contains(['"', '\n', '\r'])
        || s.starts_with(' ')
        || s.ends_with(' ')
    {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn json_value(v: &DbValue) -> String {
    match v {
        DbValue::Null => "null".into(),
        DbValue::Boolean(b) => b.to_string(),
        DbValue::Int(i) => i.to_string(),
        DbValue::Float(f) if f.is_finite() => f.to_string(),
        DbValue::Float(_) => "null".into(),
        // Exact decimal text is a valid JSON number; no float rounding
        DbValue::Decimal(d) => d.clone(),
        DbValue::Json(j) => j.to_string(),
        v => serde_json::Value::String(plain_text(v)).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(opts: ExportOptions) -> String {
        let cols = vec!["id".to_string(), "note".to_string(), "amount".to_string()];
        let rows = vec![
            vec![
                DbValue::Int(1),
                DbValue::Text("a, \"quoted\"\nline".into()),
                DbValue::Decimal("12345678901234567890.10".into()),
            ],
            vec![DbValue::Int(2), DbValue::Null, DbValue::Text(String::new())],
        ];
        let mut buf = Vec::new();
        let mut e = Exporter::begin(&mut buf, &cols, opts).unwrap();
        e.rows(&rows).unwrap();
        assert_eq!(e.finish().unwrap(), 2);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn csv_quotes_and_keeps_null_distinct_from_empty() {
        let out = export(ExportOptions {
            null_text: "NULL".into(),
            ..Default::default()
        });
        assert_eq!(
            out,
            "id,note,amount\n1,\"a, \"\"quoted\"\"\nline\",12345678901234567890.10\n2,NULL,\n"
        );
    }

    #[test]
    fn json_keeps_types_and_exact_decimals() {
        let out = export(ExportOptions {
            format: ExportFormat::Json,
            ..Default::default()
        });
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[1]["note"], serde_json::Value::Null);
        assert_eq!(parsed[1]["amount"], "");
        assert!(out.contains(r#""amount":12345678901234567890.10"#));
    }

    #[test]
    fn sql_and_column_subset() {
        let out = export(ExportOptions {
            format: ExportFormat::Sql,
            table: "\"t\"".into(),
            columns: Some(vec![1, 0]),
            ..Default::default()
        });
        assert_eq!(
            out.lines().last().unwrap(),
            r#"INSERT INTO "t" ("note", "id") VALUES (NULL, 2);"#
        );
    }
}
