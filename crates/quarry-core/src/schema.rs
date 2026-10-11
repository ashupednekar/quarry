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
        } else if upper.contains("FLOAT")
            || upper.contains("DOUBLE")
            || upper.contains("REAL")
            || upper.contains("NUMERIC")
            || upper.contains("DECIMAL")
        {
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

    /// `CREATE TABLE` rebuilt from the catalog, for databases that don't store their DDL.
    /// Covers columns, defaults, the primary key and foreign keys; not checks, uniques or comments.
    pub fn create_sql(&self, d: &dyn crate::Dialect) -> String {
        let quote_ident = |s: &str| d.quote_ident(s);
        let qualified_name = |s: &str, t: &str| d.qualified(s, t);
        let list = |cols: &[String]| {
            cols.iter()
                .map(|c| quote_ident(c))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines: Vec<String> = self
            .columns
            .iter()
            .map(|c| {
                let mut l = format!(
                    "  {} {}",
                    quote_ident(&c.name),
                    c.raw_type_name.to_lowercase()
                );
                if !c.is_nullable {
                    l.push_str(" NOT NULL");
                }
                if let Some(d) = &c.default_value {
                    l.push_str(&format!(" DEFAULT {d}"));
                }
                l
            })
            .collect();
        if !self.primary_keys.is_empty() {
            lines.push(format!("  PRIMARY KEY ({})", list(&self.primary_keys)));
        }
        for fk in &self.foreign_keys {
            let schema = if fk.to_schema.is_empty() {
                &self.schema_name
            } else {
                &fk.to_schema
            };
            lines.push(format!(
                "  FOREIGN KEY ({}) REFERENCES {} ({})",
                quote_ident(&fk.from_column),
                qualified_name(schema, &fk.to_table),
                quote_ident(&fk.to_column)
            ));
        }
        format!(
            "CREATE TABLE {} (\n{}\n);",
            qualified_name(&self.schema_name, &self.table_name),
            lines.join(",\n")
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    /// Column list (or expressions) as one display string
    pub columns: String,
}

/// Rows of (name, unique, columns) as indexes; `unique` may come back as a bool or 0/1
pub fn index_rows(r: &crate::session::QueryResult) -> Vec<IndexInfo> {
    let truthy = |v: &crate::DbValue| matches!(v.to_display_string().as_str(), "1" | "t" | "true");
    r.rows
        .iter()
        .map(|r| IndexInfo {
            name: r[0].to_display_string(),
            unique: truthy(&r[1]),
            columns: r[2].to_display_string(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Function,
    Procedure,
    Sequence,
    Trigger,
    /// Enums and other user-defined types
    Type,
}

impl ObjectKind {
    pub const ALL: [ObjectKind; 5] = [
        ObjectKind::Function,
        ObjectKind::Procedure,
        ObjectKind::Sequence,
        ObjectKind::Trigger,
        ObjectKind::Type,
    ];

    pub fn plural(self) -> &'static str {
        match self {
            ObjectKind::Function => "Functions",
            ObjectKind::Procedure => "Procedures",
            ObjectKind::Sequence => "Sequences",
            ObjectKind::Trigger => "Triggers",
            ObjectKind::Type => "Types",
        }
    }
}

/// A schema object other than a table or view, listed lazily when its schema is expanded
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaObject {
    pub kind: ObjectKind,
    pub name: String,
    /// Shown beside the name: arguments, the table a trigger is on, enum labels
    pub detail: String,
    /// Driver-specific handle for `object_ddl` (a Postgres oid); empty when the name is enough
    pub id: String,
}

/// What the Structure and DDL views show beyond the catalog
#[derive(Debug, Clone, Default)]
pub struct TableInfo {
    pub ddl: String,
    pub indexes: Vec<IndexInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaCatalog {
    pub schemas: Vec<String>,
    pub tables: Vec<TableSchema>,
    /// Where unqualified names resolve (Postgres: current_schema(), SQLite: main)
    #[serde(default)]
    pub default_schema: Option<String>,
    /// Schemas whose tables aren't loaded yet (big databases load them when opened). Empty: all loaded.
    #[serde(default)]
    pub unloaded: Vec<String>,
}

/// Above this many schemas, connecting loads only the default one; the rest load when opened
pub const EAGER_SCHEMAS: usize = 8;

impl SchemaCatalog {
    pub fn is_loaded(&self, schema: &str) -> bool {
        !self.unloaded.iter().any(|s| s.eq_ignore_ascii_case(schema))
    }

    /// A catalog holding only the default schema's tables: every other schema is marked unloaded
    pub fn lazy(mut self) -> Self {
        self.unloaded = self
            .schemas
            .iter()
            .filter(|s| self.default_schema.as_ref() != Some(*s))
            .cloned()
            .collect();
        self
    }

    /// Puts a schema's freshly loaded tables in place of whatever was there
    pub fn add_schema(&mut self, schema: &str, tables: Vec<TableSchema>) {
        self.unloaded.retain(|s| !s.eq_ignore_ascii_case(schema));
        self.tables
            .retain(|t| !t.schema_name.eq_ignore_ascii_case(schema));
        self.tables.extend(tables);
    }

    pub fn find_table(&self, schema: Option<&str>, table_name: &str) -> Option<&TableSchema> {
        self.tables.iter().find(|t| {
            if let Some(s) = schema {
                t.schema_name.eq_ignore_ascii_case(s)
                    && t.table_name.eq_ignore_ascii_case(table_name)
            } else {
                t.table_name.eq_ignore_ascii_case(table_name)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_create_table() {
        let col = |n: &str, nullable, default: Option<&str>| ColumnSchema {
            name: n.into(),
            data_type: ColumnType::Integer,
            raw_type_name: "INTEGER".into(),
            is_nullable: nullable,
            is_primary_key: n == "id",
            default_value: default.map(Into::into),
            ordinal_position: 1,
        };
        let t = TableSchema {
            schema_name: "public".into(),
            table_name: "orders".into(),
            is_view: false,
            columns: vec![col("id", false, None), col("customer_id", true, Some("0"))],
            primary_keys: vec!["id".into()],
            foreign_keys: vec![ForeignKeyRef {
                from_column: "customer_id".into(),
                to_schema: String::new(),
                to_table: "customers".into(),
                to_column: "id".into(),
            }],
        };
        assert_eq!(
            t.create_sql(&crate::dialect::Postgres),
            "CREATE TABLE \"public\".\"orders\" (\n  \"id\" integer NOT NULL,\n  \"customer_id\" integer DEFAULT 0,\n  PRIMARY KEY (\"id\"),\n  FOREIGN KEY (\"customer_id\") REFERENCES \"public\".\"customers\" (\"id\")\n);"
        );
    }
}
