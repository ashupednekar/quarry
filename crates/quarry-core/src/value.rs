// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

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

    /// User-friendly display string for cell rendering, borrowing where possible to avoid heap allocations.
    pub fn to_display_cow(&self) -> Cow<'_, str> {
        match self {
            DbValue::Null => Cow::Borrowed("NULL"),
            DbValue::Boolean(true) => Cow::Borrowed("true"),
            DbValue::Boolean(false) => Cow::Borrowed("false"),
            DbValue::Int(i) => Cow::Owned(i.to_string()),
            DbValue::Float(f) => {
                if f.fract() == 0.0 {
                    Cow::Owned(format!("{:.1}", f))
                } else {
                    Cow::Owned(f.to_string())
                }
            }
            DbValue::Decimal(s) | DbValue::Text(s) => Cow::Borrowed(s.as_str()),
            DbValue::Blob(b) => Cow::Owned(format!("<blob {} B>", b.len())),
            DbValue::Timestamp(ts) => Cow::Owned(ts.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
            DbValue::Json(j) => Cow::Owned(j.to_string()),
            DbValue::Expr(e) => Cow::Borrowed(e.as_str()),
        }
    }

    /// User-friendly display string for cell rendering
    pub fn to_display_string(&self) -> String {
        self.to_display_cow().into_owned()
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
        match self {
            DbValue::Null => f.write_str("NULL"),
            DbValue::Boolean(true) => f.write_str("true"),
            DbValue::Boolean(false) => f.write_str("false"),
            DbValue::Int(i) => write!(f, "{i}"),
            DbValue::Float(fl) => {
                if fl.fract() == 0.0 {
                    write!(f, "{:.1}", fl)
                } else {
                    write!(f, "{fl}")
                }
            }
            DbValue::Decimal(s) | DbValue::Text(s) => f.write_str(s),
            DbValue::Blob(b) => write!(f, "<blob {} B>", b.len()),
            DbValue::Timestamp(ts) => write!(f, "{}", ts.format("%Y-%m-%d %H:%M:%S UTC")),
            DbValue::Json(j) => write!(f, "{j}"),
            DbValue::Expr(e) => f.write_str(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_display_cow_borrowing() {
        assert!(matches!(
            DbValue::Null.to_display_cow(),
            Cow::Borrowed("NULL")
        ));
        assert!(matches!(
            DbValue::Boolean(true).to_display_cow(),
            Cow::Borrowed("true")
        ));
        assert!(matches!(
            DbValue::Boolean(false).to_display_cow(),
            Cow::Borrowed("false")
        ));
        let text = DbValue::Text("hello world".to_string());
        assert!(matches!(
            text.to_display_cow(),
            Cow::Borrowed("hello world")
        ));
        let dec = DbValue::Decimal("123.456".to_string());
        assert!(matches!(dec.to_display_cow(), Cow::Borrowed("123.456")));
        let expr = DbValue::Expr("NOW()".to_string());
        assert!(matches!(expr.to_display_cow(), Cow::Borrowed("NOW()")));
        let int = DbValue::Int(42);
        assert_eq!(int.to_display_cow().as_ref(), "42");
        assert_eq!(format!("{text}"), "hello world");
        assert_eq!(format!("{int}"), "42");
        assert_eq!(format!("{}", DbValue::Null), "NULL");
    }
}
