// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Database schema introspection catalog, table definitions, and type mapping.
//!
//! Provides [`SchemaCatalog`] representing the full metadata tree of a connected database,
//! [`TableSchema`], [`ColumnSchema`], and normalized [`ColumnType`] mappings across database engines.

use serde::{Deserialize, Serialize};

/// Normalized column data type category across all database dialects.
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

/// Metadata description of an individual table or view column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSchema {
    /// Column name as reported by the database engine.
    pub name: String,
    /// Normalized cross-engine type category.
    pub data_type: ColumnType,
    /// Raw database engine data type (e.g. `varchar(255)`, `int4`, `number(10,2)`).
    pub raw_type_name: String,
    /// True if the column allows NULL values.
    pub is_nullable: bool,
    /// True if this column is part of the table's primary key.
    pub is_primary_key: bool,
    /// Default column expression if configured.
    pub default_value: Option<String>,
    /// One-based column order in the table schema definition.
    pub ordinal_position: usize,
}

/// Introspected foreign key reference between two table columns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyRef {
    /// Originating column in this table.
    pub from_column: String,
    /// Target schema name (empty if within the same schema).
    #[serde(default)]
    pub to_schema: String,
    /// Referenced target table name.
    pub to_table: String,
    /// Referenced target column name.
    pub to_column: String,
}

/// Metadata description of a single database table or view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    /// Containing schema name.
    pub schema_name: String,
    /// Table or view name.
    pub table_name: String,
    /// True if this entry represents a database view rather than a physical table.
    pub is_view: bool,
    /// Ordered list of columns belonging to the table.
    pub columns: Vec<ColumnSchema>,
    /// List of column names participating in the primary key.
    pub primary_keys: Vec<String>,
    /// Foreign key relationships declared on this table.
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
    /// Column name → comment; the table's own comment under ""
    pub comments: Vec<(String, String)>,
    /// (name, definition or timing)
    pub triggers: Vec<(String, String)>,
    /// (label, value): row estimate, sizes, engine… whatever the database knows cheaply
    pub stats: Vec<(String, String)>,
}

/// Two-column rows as (key, value) pairs
pub fn pair_rows(r: &crate::session::QueryResult) -> Vec<(String, String)> {
    r.rows
        .iter()
        .map(|r| (r[0].to_display_string(), r[1].to_display_string()))
        .collect()
}

/// A one-row result as (column name, value) pairs, skipping NULLs
pub fn labelled_row(r: &crate::session::QueryResult) -> Vec<(String, String)> {
    let Some(row) = r.rows.first() else {
        return Vec::new();
    };
    r.columns
        .iter()
        .zip(row)
        .filter(|(_, v)| !v.is_null())
        .map(|(c, v)| (c.clone(), v.to_display_string()))
        .collect()
}

/// In-memory metadata catalog describing all discovered database schemas, tables, and views.
///
/// Supports partial, lazy loading via `unloaded` for high-density enterprise environments
/// where loading all schemas eagerly would introduce unacceptable startup latency.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaCatalog {
    /// List of all schema names present in the database.
    pub schemas: Vec<String>,
    /// Collection of table and view definitions loaded so far.
    pub tables: Vec<TableSchema>,
    /// Default schema context where unqualified table names resolve (e.g. `public`, `main`).
    #[serde(default)]
    pub default_schema: Option<String>,
    /// Schemas whose table definitions have not yet been introspected.
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
