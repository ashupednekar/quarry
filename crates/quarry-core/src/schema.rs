// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColumnType {
    Integer,
    BigInt,
    Float,
    Text,
    Boolean,
    Timestamp,
    Date,
    Blob,
    Json,
    Uuid,
    Custom(String),
}

impl ColumnType {
    pub fn from_sql_str(type_str: &str) -> Self {
        let upper = type_str.to_uppercase();
        if upper.contains("INT") {
            if upper.contains("BIG") || upper.contains("8") {
                ColumnType::BigInt
            } else {
                ColumnType::Integer
            }
        } else if upper.contains("CHAR") || upper.contains("TEXT") || upper.contains("CLOB") {
            ColumnType::Text
        } else if upper.contains("BOOL") {
            ColumnType::Boolean
        } else if upper.contains("FLOAT") || upper.contains("DOUBLE") || upper.contains("REAL") || upper.contains("NUMERIC") || upper.contains("DECIMAL") {
            ColumnType::Float
        } else if upper.contains("TIME") || upper.contains("DATE") {
            if upper.contains("DATE") && !upper.contains("TIME") {
                ColumnType::Date
            } else {
                ColumnType::Timestamp
            }
        } else if upper.contains("BLOB") || upper.contains("BYTEA") || upper.contains("BINARY") {
            ColumnType::Blob
        } else if upper.contains("JSON") {
            ColumnType::Json
        } else if upper.contains("UUID") {
            ColumnType::Uuid
        } else {
            ColumnType::Custom(type_str.to_string())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: ColumnType,
    pub raw_type_name: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub default_value: Option<String>,
    pub ordinal_position: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyRef {
    pub from_column: String,
    #[serde(default)]
    pub to_schema: String,
    pub to_table: String,
    pub to_column: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub schema_name: String,
    pub table_name: String,
    pub is_view: bool,
    pub columns: Vec<ColumnSchema>,
    pub primary_keys: Vec<String>,
    pub foreign_keys: Vec<ForeignKeyRef>,
}

impl TableSchema {
    pub fn has_primary_key(&self) -> bool {
        !self.primary_keys.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaCatalog {
    pub schemas: Vec<String>,
    pub tables: Vec<TableSchema>,
}

impl SchemaCatalog {
    pub fn find_table(&self, schema: Option<&str>, table_name: &str) -> Option<&TableSchema> {
        self.tables.iter().find(|t| {
            if let Some(s) = schema {
                t.schema_name.eq_ignore_ascii_case(s) && t.table_name.eq_ignore_ascii_case(table_name)
            } else {
                t.table_name.eq_ignore_ascii_case(table_name)
            }
        })
    }
}
