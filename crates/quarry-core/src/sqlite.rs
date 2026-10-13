// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use crate::driver::{Driver, Field};
use crate::edits::PendingChanges;
use crate::schema::{
    ColumnSchema, ColumnType, ForeignKeyRef, SchemaCatalog, TableInfo, TableSchema,
};
use crate::session::{
    is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState,
};
use crate::value::DbValue;
use async_trait::async_trait;
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct SqliteDriver;
pub static DRIVER: SqliteDriver = SqliteDriver;

#[async_trait]
impl Driver for SqliteDriver {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    fn name(&self) -> &'static str {
        "SQLite"
    }

    fn sample(&self) -> &'static str {
        ":memory:"
    }

    /// A file path or `:memory:`; anything with a scheme belongs to another driver
    fn recognizes(&self, conn: &str) -> bool {
        !conn.is_empty() && !conn.contains("://") && !conn.contains('=')
    }

    fn fields(&self) -> &'static [Field] {
        &[Field {
            label: "File",
            placeholder: "~/data/app.db, or :memory:",
            secret: false,
            row: 0,
            width: None,
        }]
    }

    fn split(&self, conn: &str) -> Vec<String> {
        vec![conn.to_string()]
    }

    fn join(&self, values: &[String], _previous: &str) -> String {
        values.first().cloned().unwrap_or_default()
    }

    fn is_ephemeral(&self, conn: &str) -> bool {
        conn.trim() == ":memory:"
    }

    fn display_address(&self, conn: &str) -> String {
        if conn == ":memory:" {
            return conn.to_string();
        }
        std::path::Path::new(conn)
            .file_name()
            .map_or_else(|| conn.to_string(), |f| f.to_string_lossy().into_owned())
    }

    async fn connect(
        &self,
        conn: &str,
        read_only: bool,
    ) -> Result<Arc<dyn DatabaseSession>, DbError> {
        Ok(Arc::new(match conn.trim() {
            ":memory:" => SqliteSession::open_in_memory()?,
            path => SqliteSession::open_file(expand_home(path), read_only)?,
        }))
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

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

        let conn = Connection::open_with_flags(&path, flags)
            .map_err(|e| DbError::ConnectionFailed(e.to_string()))?;

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
        let conn =
            Connection::open_in_memory().map_err(|e| DbError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: None,
            read_only: false,
            tx: TxState::default(),
        })
    }

    /// Runs `f` with the connection on Tokio's blocking thread pool so heavy disk I/O
    /// or large row decodings never stall async executor threads.
    async fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    ) -> Result<T, DbError> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| DbError::Internal("connection poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }
}

fn introspect_sqlite(conn: &mut Connection) -> Result<SchemaCatalog, DbError> {
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
        let fk_pragma = format!(
            "PRAGMA foreign_key_list(\"{}\")",
            table_name.replace('"', "\"\"")
        );
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
        default_schema: Some("main".to_string()),
        ..Default::default()
    })
}

#[async_trait]
impl DatabaseSession for SqliteSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        let start = Instant::now();
        self.with(|conn| {
            conn.execute("SELECT 1", []).map_err(sql_err)?;
            Ok(())
        })
        .await?;
        Ok(start.elapsed())
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        self.with(introspect_sqlite).await
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        if self.read_only && is_mutation(sql) {
            return Err(DbError::ReadOnlyViolation);
        }

        let start = Instant::now();
        let sql = sql.to_string();
        let manual = self.tx.manual();
        let opens = manual && tx_verb(&sql).is_none() && !is_read(&sql);
        let (result, is_open) = self
            .with(move |conn| {
                // Manual-commit mode: the first statement opens a transaction the user commits explicitly (E9)
                if opens && conn.is_autocommit() {
                    conn.execute_batch("BEGIN").map_err(sql_err)?;
                }
                let res = run_statement(conn, &sql, start);
                let open = !conn.is_autocommit();
                res.map(|r| (r, open))
            })
            .await?;
        self.tx.set_open(is_open);
        Ok(result)
    }

    async fn apply_edits(
        &self,
        table_name: &str,
        changes: &PendingChanges,
    ) -> Result<usize, DbError> {
        if self.read_only {
            return Err(DbError::ReadOnlyViolation);
        }

        let statements = changes.generate_sql(table_name, self.dialect());
        if statements.is_empty() {
            return Ok(0);
        }

        let manual = self.tx.manual();
        let count = statements.len();
        // A savepoint works inside or outside a transaction. Dropping it on an early return rolls
        // this batch back (E8: nothing partial is applied) without ending an outer transaction.
        let is_open = self
            .with(move |conn| {
                if manual && conn.is_autocommit() {
                    conn.execute_batch("BEGIN").map_err(sql_err)?;
                }
                let sp = conn.savepoint().map_err(sql_err)?;
                for sql in &statements {
                    if sp.execute(sql, []).map_err(sql_err)? != 1 {
                        return Err(DbError::ConcurrentModification);
                    }
                }
                sp.commit().map_err(sql_err)?;
                Ok(!conn.is_autocommit())
            })
            .await?;
        self.tx.set_open(is_open);
        Ok(count)
    }

    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        Some(self)
    }

    /// SQLite's only schema objects besides tables and views are triggers (and indexes, shown per table)
    async fn objects(&self, _schema: &str) -> Result<Vec<crate::schema::SchemaObject>, DbError> {
        self.with(|conn| {
            let r = run_statement(
                conn,
                "SELECT name, tbl_name FROM sqlite_master WHERE type = 'trigger' ORDER BY name",
                Instant::now(),
            )?;
            Ok(r.rows
                .iter()
                .map(|row| crate::schema::SchemaObject {
                    kind: crate::schema::ObjectKind::Trigger,
                    name: row[0].to_display_string(),
                    detail: format!("on {}", row[1].to_display_string()),
                    id: String::new(),
                })
                .collect())
        })
        .await
    }

    async fn object_ddl(
        &self,
        _schema: &str,
        obj: &crate::schema::SchemaObject,
    ) -> Result<String, DbError> {
        let name = obj.name.replace('\'', "''");
        let obj_name = obj.name.clone();
        self.with(move |conn| {
            let r = run_statement(
                conn,
                &format!("SELECT sql FROM sqlite_master WHERE name = '{name}'"),
                Instant::now(),
            )?;
            r.rows
                .first()
                .map(|row| format!("{};", row[0].to_display_string()))
                .ok_or_else(|| DbError::SqlError(format!("{obj_name} no longer exists")))
        })
        .await
    }

    /// SQLite keeps each object's DDL
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        let table = table.clone();
        self.with(move |conn| {
            let name = format!("'{}'", table.table_name.replace('\'', "''"));
            let ddl = run_statement(conn, &format!("SELECT sql FROM sqlite_master WHERE tbl_name = {name} AND sql IS NOT NULL ORDER BY type = 'index', name"), Instant::now())?;
            let indexes = run_statement(
                conn,
                &format!("SELECT il.name, il.\"unique\", (SELECT group_concat(ii.name, ', ') FROM pragma_index_info(il.name) ii) FROM pragma_index_list({name}) il ORDER BY il.name"),
                Instant::now(),
            )?;
            let ddl = ddl
                .rows
                .iter()
                .map(|r| format!("{};", r[0].to_display_string().trim_end_matches(';')))
                .collect::<Vec<_>>()
                .join("\n\n");
            let triggers = run_statement(conn, &format!("SELECT name, sql FROM sqlite_master WHERE type = 'trigger' AND tbl_name = {name} ORDER BY name"), Instant::now());
            // Exact count: SQLite keeps no estimate, and local files are small enough
            let quoted = format!("\"{}\"", table.table_name.replace('"', "\"\""));
            let stats = run_statement(
                conn,
                &format!("SELECT count(*) AS \"Rows\" FROM {quoted}"),
                Instant::now(),
            );
            Ok(TableInfo {
                ddl,
                indexes: crate::schema::index_rows(&indexes),
                triggers: triggers
                    .map(|r| crate::schema::pair_rows(&r))
                    .unwrap_or_default(),
                stats: stats
                    .map(|r| crate::schema::labelled_row(&r))
                    .unwrap_or_default(),
                ..Default::default()
            })
        })
        .await
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

    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let mut rows_data = Vec::new();
    let mut rows = stmt.query([]).map_err(sql_err)?;
    let mut row_count = 0u64;

    while let Some(row) = rows.next().map_err(sql_err)? {
        // Past the cap rows are counted, not decoded
        row_count += 1;
        if rows_data.len() >= crate::session::MAX_ROWS {
            continue;
        }
        let row_vals = (0..col_count)
            .map(|i| row.get_ref(i).map(decode).map_err(sql_err))
            .collect::<Result<Vec<_>, _>>()?;
        rows_data.push(row_vals);
    }

    Ok(QueryResult {
        columns,
        rows: rows_data,
        rows_affected: row_count,
        execution_time: start.elapsed(),
        statement: sql.to_string(),
    })
}

fn decode(v: ValueRef) -> DbValue {
    match v {
        ValueRef::Null => DbValue::Null,
        ValueRef::Integer(i) => DbValue::Int(i),
        ValueRef::Real(r) => DbValue::Float(r),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).to_string();
            // Quick JSON heuristic
            let looks_json = (s.starts_with('{') && s.ends_with('}'))
                || (s.starts_with('[') && s.ends_with(']'));
            match looks_json.then(|| serde_json::from_str(&s).ok()).flatten() {
                Some(j) => DbValue::Json(j),
                None => DbValue::Text(s),
            }
        }
        ValueRef::Blob(b) => DbValue::Blob(b.to_vec()),
    }
}

impl SqliteSession {
    /// Hands each row of `sql` to `f` as it's read, keeping none of them (dumps of any size)
    pub async fn for_each_row(
        &self,
        sql: &str,
        mut f: impl FnMut(&[String], Vec<DbValue>) -> Result<(), String>,
    ) -> Result<usize, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "connection poisoned".to_string())?;
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let cols: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut n = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let vals = (0..cols.len())
                .map(|i| row.get_ref(i).map(decode))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            f(&cols, vals)?;
            n += 1;
        }
        Ok(n)
    }
}
