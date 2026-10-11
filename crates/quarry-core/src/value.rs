// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DbValue {
    Null,
    Boolean(bool),
    Int(i64),
    Float(f64),
    /// Exact numeric (Postgres NUMERIC) kept as its decimal text so no precision is lost
    Decimal(String),
    Text(String),
    Blob(Vec<u8>),
    Timestamp(DateTime<Utc>),
    Json(serde_json::Value),
    /// A raw SQL expression written into edits as-is, e.g. `DEFAULT` or a column's default expression
    Expr(String),
}

impl DbValue {
    pub fn is_null(&self) -> bool {
        matches!(self, DbValue::Null)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            DbValue::Null => "NULL",
            DbValue::Boolean(_) => "BOOLEAN",
            DbValue::Int(_) => "INTEGER",
            DbValue::Float(_) => "FLOAT",
            DbValue::Decimal(_) => "NUMERIC",
            DbValue::Text(_) => "TEXT",
            DbValue::Blob(_) => "BLOB",
            DbValue::Timestamp(_) => "TIMESTAMP",
            DbValue::Json(_) => "JSON",
            DbValue::Expr(_) => "EXPR",
        }
    }

    /// User-friendly display string for cell rendering
    pub fn to_display_string(&self) -> String {
        match self {
            DbValue::Null => "NULL".to_string(),
            DbValue::Boolean(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            DbValue::Int(i) => i.to_string(),
            DbValue::Float(f) => {
                if f.fract() == 0.0 {
                    format!("{:.1}", f)
                } else {
                    f.to_string()
                }
            }
            DbValue::Decimal(s) | DbValue::Text(s) => s.clone(),
            DbValue::Blob(b) => format!("<blob {} B>", b.len()),
            DbValue::Timestamp(ts) => ts.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            DbValue::Json(j) => j.to_string(),
            DbValue::Expr(e) => e.clone(),
        }
    }

    /// SQL literal representation for generating valid UPDATE/INSERT queries
    pub fn to_sql_literal(&self) -> String {
        match self {
            DbValue::Null => "NULL".to_string(),
            DbValue::Boolean(b) => {
                if *b {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }
            }
            DbValue::Int(i) => i.to_string(),
            DbValue::Float(f) => f.to_string(),
            DbValue::Decimal(s) => s.clone(),
            DbValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
            DbValue::Blob(b) => format!("X'{}'", hex_encode(b)),
            DbValue::Timestamp(ts) => format!("'{}'", ts.to_rfc3339()),
            DbValue::Json(j) => format!("'{}'", j.to_string().replace('\'', "''")),
            DbValue::Expr(e) => e.clone(),
        }
    }

    pub fn cmp_val(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (DbValue::Null, DbValue::Null) => std::cmp::Ordering::Equal,
            (DbValue::Null, _) => std::cmp::Ordering::Less,
            (_, DbValue::Null) => std::cmp::Ordering::Greater,
            (DbValue::Int(a), DbValue::Int(b)) => a.cmp(b),
            (DbValue::Float(a), DbValue::Float(b)) => {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            }
            (DbValue::Boolean(a), DbValue::Boolean(b)) => a.cmp(b),
            (DbValue::Text(a), DbValue::Text(b)) => a.cmp(b),
            (DbValue::Decimal(a), DbValue::Decimal(b)) => {
                let (x, y) = (
                    a.parse::<f64>().unwrap_or(f64::NAN),
                    b.parse::<f64>().unwrap_or(f64::NAN),
                );
                x.partial_cmp(&y).unwrap_or_else(|| a.cmp(b))
            }
            _ => self.to_string().cmp(&other.to_string()),
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect()
}

impl std::fmt::Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}
