// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::edits::PendingChanges;
use crate::schema::{ColumnSchema, ColumnType, ForeignKeyRef, SchemaCatalog, TableSchema};
use crate::session::{is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState};
use crate::value::DbValue;
use async_trait::async_trait;
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct SqliteSession {
    conn: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
    read_only: bool,
    tx: TxState,
}

fn sql_err(e: rusqlite::Error) -> DbError {
    DbError::SqlError(e.to_string())
}

impl SqliteSession {
    pub fn open_file(path: impl Into<PathBuf>, read_only: bool) -> Result<Self, DbError> {
        let path = path.into();
        let flags = if read_only {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
        };

        let conn = Connection::open_with_flags(&path, flags).map_err(|e| DbError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(path),
            read_only,
            tx: TxState::default(),
        })
    }

    pub fn file_path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory().map_err(|e| DbError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: None,
            read_only: false,
            tx: TxState::default(),
        })
    }
}

#[async_trait]
impl DatabaseSession for SqliteSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        let start = Instant::now();
        let conn = self.conn.lock().await;
        conn.execute("SELECT 1", []).map_err(sql_err)?;
        Ok(start.elapsed())
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare("SELECT type, name FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(sql_err)?;

        let table_rows = stmt
            .query_map([], |row| {
                let kind: String = row.get(0)?;
                let name: String = row.get(1)?;
                Ok((kind == "view", name))
            })
            .map_err(sql_err)?;

        let mut tables = Vec::new();

        for table in table_rows {
            let (is_view, table_name) = table.map_err(sql_err)?;

            // Fetch columns using PRAGMA table_info
            let pragma_sql = format!("PRAGMA table_info(\"{}\")", table_name.replace('"', "\"\""));
            let mut pragma_stmt = conn.prepare(&pragma_sql).map_err(sql_err)?;

            let mut columns = Vec::new();
            let mut pks = Vec::new();

            let col_rows = pragma_stmt
                .query_map([], |row| {
                    let cid: i64 = row.get(0)?;
                    let name: String = row.get(1)?;
                    let raw_type: String = row.get(2)?;
                    let notnull: i32 = row.get(3)?;
                    let dflt_value: Option<String> = row.get(4)?;
                    let pk_pos: i32 = row.get(5)?;

                    Ok((cid as usize, name, raw_type, notnull, dflt_value, pk_pos))
                })
                .map_err(sql_err)?;

            for col in col_rows {
                let (cid, col_name, raw_type, notnull, dflt, pk) = col.map_err(sql_err)?;
                let is_pk = pk > 0;
                if is_pk {
                    pks.push(col_name.clone());
                }

                columns.push(ColumnSchema {
                    name: col_name,
                    data_type: ColumnType::from_sql_str(&raw_type),
                    raw_type_name: raw_type,
                    is_nullable: notnull == 0,
                    is_primary_key: is_pk,
                    default_value: dflt,
                    ordinal_position: cid,
                });
            }

            // Fetch foreign keys
            let mut fks = Vec::new();
            let fk_pragma = format!("PRAGMA foreign_key_list(\"{}\")", table_name.replace('"', "\"\""));
            if let Ok(mut fk_stmt) = conn.prepare(&fk_pragma) {
                if let Ok(fk_rows) = fk_stmt.query_map([], |row| {
                    Ok(ForeignKeyRef {
                        to_schema: "main".to_string(),
                        to_table: row.get(2)?,
                        from_column: row.get(3)?,
                        to_column: row.get(4)?,
                    })
                }) {
                    for fk in fk_rows.flatten() {
                        fks.push(fk);
                    }
                }
            }

            tables.push(TableSchema {
                schema_name: "main".to_string(),
                table_name,
                is_view,
                columns,
                primary_keys: pks,
                foreign_keys: fks,
            });
        }

        Ok(SchemaCatalog {
            schemas: vec!["main".to_string()],
            tables,
        })
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        if self.read_only && is_mutation(sql) {
            return Err(DbError::ReadOnlyViolation);
        }

        let start = Instant::now();
        let conn = self.conn.lock().await;
        // Manual-commit mode: the first statement opens a transaction the user commits explicitly (E9)
        if self.tx.manual() && conn.is_autocommit() && tx_verb(sql).is_none() && !is_read(sql) {
            conn.execute_batch("BEGIN").map_err(sql_err)?;
        }
        let result = run_statement(&conn, sql, start);
        self.tx.set_open(!conn.is_autocommit());
        result
    }

    async fn apply_edits(&self, table_name: &str, changes: &PendingChanges) -> Result<usize, DbError> {
        if self.read_only {
            return Err(DbError::ReadOnlyViolation);
        }

        let statements = changes.generate_sql(table_name, self.dialect());
        if statements.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn.lock().await;
        if self.tx.manual() && conn.is_autocommit() {
            conn.execute_batch("BEGIN").map_err(sql_err)?;
        }
        // A savepoint works inside or outside a transaction. Dropping it on an early return rolls
        // this batch back (E8: nothing partial is applied) without ending an outer transaction.
        let result = (|| {
            let sp = conn.savepoint().map_err(sql_err)?;
            for sql in &statements {
                if sp.execute(sql, []).map_err(sql_err)? != 1 {
                    return Err(DbError::ConcurrentModification);
                }
            }
            sp.commit().map_err(sql_err)
        })();
        self.tx.set_open(!conn.is_autocommit());
        result.map(|_| statements.len())
    }

    async fn cancel_backend(&self, _pid: i64) -> Result<(), DbError> {
        Ok(())
    }

    async fn terminate_backend(&self, _pid: i64) -> Result<(), DbError> {
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn tx(&self) -> &TxState {
        &self.tx
    }

    fn dialect(&self) -> &'static dyn crate::Dialect {
        &crate::dialect::Sqlite
    }
}

fn run_statement(conn: &Connection, sql: &str, start: Instant) -> Result<QueryResult, DbError> {
    let mut stmt = conn.prepare(sql).map_err(sql_err)?;
    let col_count = stmt.column_count();

    if col_count == 0 {
        // Non-returning statement (DDL/DML)
        let affected = stmt.execute([]).map_err(sql_err)?;
        return Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: affected as u64,
            execution_time: start.elapsed(),
            statement: sql.to_string(),
        });
    }

    let columns: Vec<String> = stmt.column_names().into_iter().map(|s| s.to_string()).collect();

    let mut rows_data = Vec::new();
    let mut rows = stmt.query([]).map_err(sql_err)?;

    while let Some(row) = rows.next().map_err(sql_err)? {
        let mut row_vals = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let val = match row.get_ref(i).map_err(sql_err)? {
                ValueRef::Null => DbValue::Null,
                ValueRef::Integer(i) => DbValue::Int(i),
                ValueRef::Real(r) => DbValue::Float(r),
                ValueRef::Text(t) => {
                    let s = String::from_utf8_lossy(t).to_string();
                    // Quick JSON heuristic
                    let looks_json = (s.starts_with('{') && s.ends_with('}')) || (s.starts_with('[') && s.ends_with(']'));
                    match looks_json.then(|| serde_json::from_str(&s).ok()).flatten() {
                        Some(j) => DbValue::Json(j),
                        None => DbValue::Text(s),
                    }
                }
                ValueRef::Blob(b) => DbValue::Blob(b.to_vec()),
            };
            row_vals.push(val);
        }
        rows_data.push(row_vals);
    }

    let row_count = rows_data.len() as u64;

    Ok(QueryResult {
        columns,
        rows: rows_data,
        rows_affected: row_count,
        execution_time: start.elapsed(),
        statement: sql.to_string(),
    })
}
