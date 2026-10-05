// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use quarry_core::SchemaCatalog;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    Keyword,
    Table,
    Column,
    Schema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCandidate {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

pub struct SchemaCompleter;

impl SchemaCompleter {
    pub fn complete(
        sql: &str,
        cursor_offset: usize,
        catalog: &SchemaCatalog,
    ) -> Vec<CompletionCandidate> {
        let prefix = Self::extract_prefix(sql, cursor_offset);
        let prefix_lower = prefix.to_lowercase();

        let mut results = Vec::new();

        // 1. Check if user typed `<identifier>.` -> suggest columns of that table or schema
        if let Some(qualifier) = Self::extract_qualifier(sql, cursor_offset) {
            let qual_lower = qualifier.to_lowercase();
            // Check if qualifier matches any table
            for table in &catalog.tables {
                if table.table_name.to_lowercase() == qual_lower {
                    for col in &table.columns {
                        if col.name.to_lowercase().starts_with(&prefix_lower) {
                            results.push(CompletionCandidate {
                                label: col.name.clone(),
                                kind: CompletionKind::Column,
                                detail: Some(format!("{} (column)", col.raw_type_name)),
                            });
                        }
                    }
                    return results;
                }
            }
        }

        // 2. Suggest Tables
        for table in &catalog.tables {
            if table.table_name.to_lowercase().starts_with(&prefix_lower) {
                results.push(CompletionCandidate {
                    label: table.table_name.clone(),
                    kind: CompletionKind::Table,
                    detail: Some(format!("{} columns", table.columns.len())),
                });
            }
        }

        // 3. Suggest Columns from all tables
        for table in &catalog.tables {
            for col in &table.columns {
                if col.name.to_lowercase().starts_with(&prefix_lower) {
                    // Avoid duplicate column names
                    if !results.iter().any(|r| r.label == col.name && r.kind == CompletionKind::Column) {
                        results.push(CompletionCandidate {
                            label: col.name.clone(),
                            kind: CompletionKind::Column,
                            detail: Some(format!("{}.{} ({})", table.table_name, col.name, col.raw_type_name)),
                        });
                    }
                }
            }
        }

        // 4. Suggest SQL Keywords
        const KEYWORDS: &[&str] = &[
            "SELECT", "FROM", "WHERE", "JOIN", "LEFT JOIN", "RIGHT JOIN", "INNER JOIN",
            "ON", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET", "INSERT INTO",
            "VALUES", "UPDATE", "SET", "DELETE FROM", "CREATE TABLE", "DROP TABLE",
            "ALTER TABLE", "AND", "OR", "NOT", "IN", "IS NULL", "IS NOT NULL",
            "BETWEEN", "LIKE", "AS", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
            "CASE", "WHEN", "THEN", "ELSE", "END", "EXPLAIN ANALYZE", "BEGIN", "COMMIT", "ROLLBACK",
        ];

        for kw in KEYWORDS {
            if kw.to_lowercase().starts_with(&prefix_lower) {
                results.push(CompletionCandidate {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                    detail: Some("SQL Keyword".to_string()),
                });
            }
        }

        // Limit results to top 20
        results.truncate(20);
        results
    }

    fn extract_prefix(sql: &str, cursor_offset: usize) -> &str {
        let safe_offset = cursor_offset.min(sql.len());
        let before_cursor = &sql[..safe_offset];
        let start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        &before_cursor[start..]
    }

    fn extract_qualifier(sql: &str, cursor_offset: usize) -> Option<String> {
        let safe_offset = cursor_offset.min(sql.len());
        let before_cursor = &sql[..safe_offset];
        let prefix = Self::extract_prefix(sql, cursor_offset);
        let before_prefix = &before_cursor[..before_cursor.len() - prefix.len()];

        if before_prefix.ends_with('.') {
            let trimmed = &before_prefix[..before_prefix.len() - 1];
            let start = trimmed
                .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|idx| idx + 1)
                .unwrap_or(0);
            let qualifier = &trimmed[start..];
            if !qualifier.is_empty() {
                return Some(qualifier.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_core::schema::{ColumnSchema, ColumnType, TableSchema};

    #[test]
    fn test_completer_table_and_columns() {
        let catalog = SchemaCatalog {
            schemas: vec!["public".to_string()],
            tables: vec![TableSchema {
                schema_name: "public".to_string(),
                table_name: "customers".to_string(),
                is_view: false,
                columns: vec![
                    ColumnSchema {
                        name: "customer_id".to_string(),
                        data_type: ColumnType::Integer,
                        raw_type_name: "int".to_string(),
                        is_nullable: false,
                        is_primary_key: true,
                        default_value: None,
                        ordinal_position: 1,
                    },
                    ColumnSchema {
                        name: "email".to_string(),
                        data_type: ColumnType::Text,
                        raw_type_name: "text".to_string(),
                        is_nullable: false,
                        is_primary_key: false,
                        default_value: None,
                        ordinal_position: 2,
                    },
                ],
                primary_keys: vec!["customer_id".to_string()],
                foreign_keys: vec![],
            }],
        };

        // Complete table prefix "cust"
        let candidates = SchemaCompleter::complete("SELECT * FROM cust", 18, &catalog);
        assert!(candidates.iter().any(|c| c.label == "customers" && c.kind == CompletionKind::Table));

        // Complete qualified column "customers.em"
        let col_candidates = SchemaCompleter::complete("SELECT customers.em", 19, &catalog);
        assert!(col_candidates.iter().any(|c| c.label == "email" && c.kind == CompletionKind::Column));
    }
}
