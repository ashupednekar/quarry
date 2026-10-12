// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::dialect::{Ansi, Dialect};
use crate::value::DbValue;
use std::collections::HashMap;

/// A row's identity: its primary-key columns and their values as loaded.
pub type RowKey = Vec<(String, DbValue)>;

#[derive(Debug, Clone, PartialEq)]
pub struct PendingCellEdit {
    pub pks: RowKey,
    pub column_name: String,
    pub old_value: DbValue,
    pub new_value: DbValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingRowInsert {
    pub temp_id: String,
    pub values: HashMap<String, DbValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingRowDelete {
    pub pks: RowKey,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingChanges {
    pub cell_edits: Vec<PendingCellEdit>,
    pub row_inserts: Vec<PendingRowInsert>,
    pub row_deletes: Vec<PendingRowDelete>,
}

impl PendingChanges {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.cell_edits.is_empty() && self.row_inserts.is_empty() && self.row_deletes.is_empty()
    }

    pub fn total_count(&self) -> usize {
        self.cell_edits.len() + self.row_inserts.len() + self.row_deletes.len()
    }

    pub fn clear(&mut self) {
        self.cell_edits.clear();
        self.row_inserts.clear();
        self.row_deletes.clear();
    }

    /// Pending value for a loaded cell, if it was edited
    pub fn cell_value(&self, pks: &RowKey, column: &str) -> Option<&DbValue> {
        self.cell_edits
            .iter()
            .find(|e| &e.pks == pks && e.column_name == column)
            .map(|e| &e.new_value)
    }

    /// Records an edit, replacing an earlier one on the same cell; editing back to the original drops it.
    pub fn set_cell(&mut self, pks: RowKey, column: &str, old: DbValue, new: DbValue) {
        self.cell_edits
            .retain(|e| !(e.pks == pks && e.column_name == column));
        if old != new {
            self.cell_edits.push(PendingCellEdit {
                pks,
                column_name: column.to_string(),
                old_value: old,
                new_value: new,
            });
        }
    }

    pub fn is_deleted(&self, pks: &RowKey) -> bool {
        self.row_deletes.iter().any(|d| &d.pks == pks)
    }

    pub fn toggle_delete(&mut self, pks: RowKey) {
        if self.is_deleted(&pks) {
            self.row_deletes.retain(|d| d.pks != pks);
        } else {
            self.cell_edits.retain(|e| e.pks != pks);
            self.row_deletes.push(PendingRowDelete { pks });
        }
    }

    /// Exact SQL for review (E5). Each statement must affect exactly one row when applied.
    pub fn generate_sql(&self, table: &str, d: &dyn Dialect) -> Vec<String> {
        let mut statements = Vec::new();

        // One UPDATE per row; the WHERE also checks the edited columns still hold their loaded
        // values, so a row changed by someone else matches nothing instead of being overwritten.
        let mut rows: Vec<(&RowKey, Vec<&PendingCellEdit>)> = Vec::new();
        for edit in &self.cell_edits {
            match rows.iter_mut().find(|(pks, _)| *pks == &edit.pks) {
                Some((_, edits)) => edits.push(edit),
                None => rows.push((&edit.pks, vec![edit])),
            }
        }
        for (pks, edits) in rows {
            let set: Vec<String> = edits
                .iter()
                .map(|e| {
                    format!(
                        "{} = {}",
                        d.quote_ident(&e.column_name),
                        d.literal(&e.new_value)
                    )
                })
                .collect();
            let mut conds = key_conditions(pks, d);
            conds.extend(
                edits
                    .iter()
                    .filter_map(|e| unchanged_condition(&e.column_name, &e.old_value, d)),
            );
            statements.push(format!(
                "UPDATE {} SET {} WHERE {};",
                table,
                set.join(", "),
                conds.join(" AND ")
            ));
        }

        for insert in &self.row_inserts {
            let mut cols: Vec<&String> = insert.values.keys().collect();
            cols.sort();
            statements.push(if cols.is_empty() {
                format!("INSERT INTO {} DEFAULT VALUES;", table)
            } else {
                format!(
                    "INSERT INTO {} ({}) VALUES ({});",
                    table,
                    cols.iter()
                        .map(|c| d.quote_ident(c))
                        .collect::<Vec<_>>()
                        .join(", "),
                    cols.iter()
                        .map(|c| d.literal(&insert.values[*c]))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        }

        for delete in &self.row_deletes {
            statements.push(format!(
                "DELETE FROM {} WHERE {};",
                table,
                key_conditions(&delete.pks, d).join(" AND ")
            ));
        }

        statements
    }
}

fn key_conditions(pks: &RowKey, d: &dyn Dialect) -> Vec<String> {
    pks.iter()
        .map(|(c, v)| format!("{} = {}", d.quote_ident(c), d.literal(v)))
        .collect()
}

/// Optimistic-concurrency predicate. Floats and JSON don't round-trip exactly through SQL text, so they're skipped.
fn unchanged_condition(column: &str, old: &DbValue, d: &dyn Dialect) -> Option<String> {
    match old {
        DbValue::Null => Some(format!("{} IS NULL", d.quote_ident(column))),
        DbValue::Float(_) | DbValue::Json(_) | DbValue::Blob(_) => None,
        v => Some(format!("{} = {}", d.quote_ident(column), d.literal(v))),
    }
}

/// ANSI quoting, for SQL built with no session at hand; prefer `session.dialect()`
pub fn quote_ident(ident: &str) -> String {
    Ansi.quote_ident(ident)
}

/// `"schema"."table"` in ANSI quoting; prefer `session.dialect().qualified(..)`
pub fn qualified_name(schema: &str, table: &str) -> String {
    Ansi.qualified(schema, table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: i64) -> RowKey {
        vec![
            ("tenant_id".into(), DbValue::Int(101)),
            ("user_id".into(), DbValue::Int(id)),
        ]
    }

    #[test]
    fn composite_pk_update_checks_old_value() {
        let mut changes = PendingChanges::new();
        changes.set_cell(
            key(42),
            "status",
            DbValue::Text("inactive".into()),
            DbValue::Text("active".into()),
        );
        let sql = changes.generate_sql("\"tenant_users\"", &Ansi);
        assert_eq!(
            sql,
            vec![
                r#"UPDATE "tenant_users" SET "status" = 'active' WHERE "tenant_id" = 101 AND "user_id" = 42 AND "status" = 'inactive';"#
            ]
        );
    }

    #[test]
    fn re_editing_a_cell_replaces_and_reverting_drops_it() {
        let mut c = PendingChanges::new();
        c.set_cell(key(1), "a", DbValue::Int(1), DbValue::Int(2));
        c.set_cell(key(1), "a", DbValue::Int(1), DbValue::Int(3));
        assert_eq!(c.cell_value(&key(1), "a"), Some(&DbValue::Int(3)));
        c.set_cell(key(1), "a", DbValue::Int(1), DbValue::Int(1));
        assert!(c.is_empty());
    }

    #[test]
    fn deleting_a_row_drops_its_edits_and_null_old_value_uses_is_null() {
        let mut c = PendingChanges::new();
        c.set_cell(key(1), "a", DbValue::Null, DbValue::Int(2));
        assert!(c.generate_sql("t", &Ansi)[0].contains(r#""a" IS NULL"#));
        c.toggle_delete(key(1));
        assert_eq!(c.cell_edits.len(), 0);
        assert_eq!(
            c.generate_sql("t", &Ansi),
            vec![r#"DELETE FROM t WHERE "tenant_id" = 101 AND "user_id" = 1;"#]
        );
        c.toggle_delete(key(1));
        assert!(c.is_empty());
    }

    #[test]
    fn total_count_tracks_all_mutation_types() {
        let mut c = PendingChanges::new();
        assert!(c.is_empty());
        assert_eq!(c.total_count(), 0);

        c.set_cell(key(1), "a", DbValue::Int(1), DbValue::Int(2));
        assert_eq!(c.total_count(), 1);

        c.row_inserts.push(PendingRowInsert {
            temp_id: uuid::Uuid::new_v4().to_string(),
            values: std::collections::HashMap::new(),
        });
        assert_eq!(c.total_count(), 2);

        c.row_deletes.push(PendingRowDelete { pks: key(2) });
        assert_eq!(c.total_count(), 3);
        assert!(!c.is_empty());

        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.total_count(), 0);
    }
}
