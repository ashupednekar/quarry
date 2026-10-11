// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! MySQL and MariaDB through mysql_async. A MySQL "database" is a schema here.

mod monitor;

use crate::conn_url::{self, UrlFields};
use crate::driver::{Driver, Field};
use crate::edits::PendingChanges;
use crate::schema::{ColumnSchema, ColumnType, ForeignKeyRef, IndexInfo, ObjectKind, SchemaCatalog, SchemaObject, TableInfo, TableSchema, EAGER_SCHEMAS};
use crate::session::{is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState, TxVerb, MAX_ROWS};
use crate::value::DbValue;
use async_trait::async_trait;
use mysql_async::consts::ColumnType as T;
use mysql_async::prelude::Queryable;
use mysql_async::{Column, Conn, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SCHEMES: [&str; 2] = ["mysql", "mariadb"];
/// Schemas every server has; hidden from the tree
const SYSTEM: &str = "('mysql', 'information_schema', 'performance_schema', 'sys')";

pub struct MysqlDriver;
pub static DRIVER: MysqlDriver = MysqlDriver;

const fn field(label: &'static str, placeholder: &'static str, secret: bool, row: u8, width: Option<f32>) -> Field {
    Field { label, placeholder, secret, row, width }
}

#[async_trait]
impl Driver for MysqlDriver {
    fn id(&self) -> &'static str {
        "mysql"
    }

    fn name(&self) -> &'static str {
        "MySQL"
    }

    fn sample(&self) -> &'static str {
        "mysql://root@localhost:3306/mysql"
    }

    fn recognizes(&self, conn: &str) -> bool {
        SCHEMES.iter().any(|s| conn.starts_with(&format!("{s}://")))
    }

    fn fields(&self) -> &'static [Field] {
        const FIELDS: [Field; 5] = [
            field("Host", "localhost", false, 0, None),
            field("Port", "3306", false, 0, Some(120.0)),
            field("User", "root", false, 1, None),
            field("Password", "", true, 1, None),
            field("Database", "optional", false, 2, None),
        ];
        &FIELDS
    }

    fn split(&self, conn: &str) -> Vec<String> {
        conn_url::parse(conn, &SCHEMES).map(|(f, _)| f.to_vec()).unwrap_or_else(|| vec![String::new(); 5])
    }

    fn join(&self, values: &[String], previous: &str) -> String {
        let query = conn_url::parse(previous, &SCHEMES).map(|(_, q)| q).unwrap_or_default();
        let fields: UrlFields = std::array::from_fn(|i| values.get(i).cloned().unwrap_or_default());
        conn_url::build("mysql", &fields, &query)
    }

    fn take_secret(&self, conn: &str) -> (String, Option<String>) {
        conn_url::split_password(conn).unwrap_or_else(|| (conn.to_string(), None))
    }

    fn put_secret(&self, conn: &str, secret: &str) -> String {
        conn_url::insert_password(conn, secret)
    }

    fn display_address(&self, conn: &str) -> String {
        let [host, port, _, _, db] = <[String; 5]>::try_from(self.split(conn)).unwrap_or_default();
        let host = if host.is_empty() { "localhost".to_string() } else { host };
        let mut out = if port.is_empty() { host } else { format!("{host}:{port}") };
        if !db.is_empty() {
            out += &format!("/{db}");
        }
        out
    }

    fn tunnel_target(&self, conn: &str) -> Option<(String, u16)> {
        conn_url::target(conn, 3306)
    }

    fn through_tunnel(&self, conn: &str, local_port: u16) -> String {
        conn_url::retarget(conn, local_port)
    }

    async fn connect(&self, conn: &str, read_only: bool) -> Result<Arc<dyn DatabaseSession>, DbError> {
        Ok(Arc::new(MysqlSession::connect(conn, read_only).await?))
    }
}

pub struct MysqlSession {
    conn: Mutex<Conn>,
    /// For mysqldump, which takes the same address
    url: String,
    read_only: bool,
    tx: TxState,
}

fn err(e: mysql_async::Error) -> DbError {
    DbError::SqlError(e.to_string())
}

impl MysqlSession {
    pub async fn connect(url: &str, read_only: bool) -> Result<Self, DbError> {
        let opts = mysql_async::Opts::from_url(url).map_err(|e| DbError::ConnectionFailed(e.to_string()))?;
        // Matched rather than changed rows: an edit that writes the value already there still
        // counts as one row, so the conflict check (exactly one row) stays right
        let opts = mysql_async::OptsBuilder::from_opts(opts).client_found_rows(true);
        let mut conn = Conn::new(opts).await.map_err(|e| DbError::ConnectionFailed(e.to_string()))?;
        if read_only {
            conn.query_drop("SET SESSION TRANSACTION READ ONLY").await.map_err(err)?;
        }
        Ok(Self { conn: Mutex::new(conn), url: url.to_string(), read_only, tx: TxState::default() })
    }

    async fn run(&self, sql: &str) -> Result<QueryResult, DbError> {
        let mut conn = self.conn.lock().await;
        run_statement(&mut conn, sql).await
    }

    /// First column of every row, as text (catalog queries)
    async fn column(&self, sql: &str) -> Result<Vec<String>, DbError> {
        Ok(self.run(sql).await?.rows.iter().map(|r| r[0].to_display_string()).collect())
    }
}

/// Text protocol, first result set only; rows past MAX_ROWS are counted but not kept
async fn run_statement(conn: &mut Conn, sql: &str) -> Result<QueryResult, DbError> {
    let start = Instant::now();
    let mut result = conn.query_iter(sql).await.map_err(err)?;
    let columns: Arc<[Column]> = result.columns().unwrap_or_else(|| Arc::from(Vec::new()));
    let mut rows = Vec::new();
    let mut count = 0u64;
    result
        .for_each(|row| {
            count += 1;
            if rows.len() < MAX_ROWS {
                rows.push(row.unwrap().into_iter().zip(columns.iter()).map(|(v, c)| decode(v, c)).collect());
            }
        })
        .await
        .map_err(err)?;
    let affected = if columns.is_empty() { result.affected_rows() } else { count };
    result.drop_result().await.map_err(err)?;
    Ok(QueryResult {
        columns: columns.iter().map(|c| c.name_str().into_owned()).collect(),
        rows,
        rows_affected: affected,
        execution_time: start.elapsed(),
        statement: sql.to_string(),
    })
}

/// Charset 63 marks binary data; anything else in a string/blob column is text
const BINARY: u16 = 63;

/// The text protocol sends values as strings; the column type says what they are
fn decode(v: Value, col: &Column) -> DbValue {
    let bytes = match v {
        Value::NULL => return DbValue::Null,
        Value::Bytes(b) => b,
        Value::Int(i) => return DbValue::Int(i),
        Value::UInt(u) => return i64::try_from(u).map(DbValue::Int).unwrap_or_else(|_| DbValue::Decimal(u.to_string())),
        Value::Float(f) => return DbValue::Float(f.into()),
        Value::Double(f) => return DbValue::Float(f),
        other => return DbValue::Text(other.as_sql(true).trim_matches('\'').to_string()),
    };
    let text = || String::from_utf8_lossy(&bytes).into_owned();
    match col.column_type() {
        T::MYSQL_TYPE_TINY | T::MYSQL_TYPE_SHORT | T::MYSQL_TYPE_LONG | T::MYSQL_TYPE_INT24 | T::MYSQL_TYPE_LONGLONG | T::MYSQL_TYPE_YEAR => {
            // BIGINT UNSIGNED above i64::MAX keeps its exact digits
            text().parse().map(DbValue::Int).unwrap_or_else(|_| DbValue::Decimal(text()))
        }
        T::MYSQL_TYPE_FLOAT | T::MYSQL_TYPE_DOUBLE => text().parse().map(DbValue::Float).unwrap_or_else(|_| DbValue::Text(text())),
        T::MYSQL_TYPE_DECIMAL | T::MYSQL_TYPE_NEWDECIMAL => DbValue::Decimal(text()),
        T::MYSQL_TYPE_JSON => serde_json::from_slice(&bytes).map(DbValue::Json).unwrap_or_else(|_| DbValue::Text(text())),
        T::MYSQL_TYPE_BIT | T::MYSQL_TYPE_GEOMETRY => DbValue::Blob(bytes),
        T::MYSQL_TYPE_TINY_BLOB | T::MYSQL_TYPE_MEDIUM_BLOB | T::MYSQL_TYPE_LONG_BLOB | T::MYSQL_TYPE_BLOB | T::MYSQL_TYPE_VAR_STRING | T::MYSQL_TYPE_STRING | T::MYSQL_TYPE_VARCHAR
            if col.character_set() == BINARY =>
        {
            DbValue::Blob(bytes)
        }
        // Dates and times stay as the server wrote them, so nothing is shifted or rounded
        _ => DbValue::Text(text()),
    }
}

fn sql_string(s: &str) -> String {
    crate::dialect::Dialect::literal(&crate::dialect::Mysql, &DbValue::Text(s.to_string()))
}

#[async_trait]
impl DatabaseSession for MysqlSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        let start = Instant::now();
        self.conn.lock().await.ping().await.map_err(err)?;
        Ok(start.elapsed())
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        self.catalog_where(None).await
    }

    /// Many databases on the server: only the selected one, the rest when opened
    async fn load_catalog(&self) -> Result<SchemaCatalog, DbError> {
        let c = self.catalog_where(Some("DATABASE()")).await?;
        if c.schemas.len() > EAGER_SCHEMAS { Ok(c.lazy()) } else { self.introspect().await }
    }

    async fn introspect_schema(&self, schema: &str) -> Result<Vec<TableSchema>, DbError> {
        Ok(self.catalog_where(Some(&self.dialect().literal(&DbValue::Text(schema.into())))).await?.tables)
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        if self.read_only && is_mutation(sql) {
            return Err(DbError::ReadOnlyViolation);
        }
        let mut conn = self.conn.lock().await;
        // Manual-commit mode: the first write opens a transaction the user commits explicitly (E9)
        let verb = tx_verb(sql);
        if self.tx.manual() && !self.tx.open() && verb.is_none() && !is_read(sql) {
            conn.query_drop("BEGIN").await.map_err(err)?;
            self.tx.set_open(true);
        }
        let result = run_statement(&mut conn, sql).await;
        match verb {
            Some(TxVerb::Begin) if result.is_ok() => self.tx.set_open(true),
            Some(TxVerb::End) => self.tx.set_open(false),
            _ => {}
        }
        result
    }

    async fn apply_edits(&self, table: &str, changes: &PendingChanges) -> Result<usize, DbError> {
        if self.read_only {
            return Err(DbError::ReadOnlyViolation);
        }
        let statements = changes.generate_sql(table, self.dialect());
        if statements.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().await;
        // Inside a transaction a savepoint keeps a failed submit from undoing the user's other work
        let keep_open = self.tx.open() || self.tx.manual();
        if !self.tx.open() {
            conn.query_drop("BEGIN").await.map_err(err)?;
            self.tx.set_open(keep_open);
        }
        if keep_open {
            conn.query_drop("SAVEPOINT quarry_edits").await.map_err(err)?;
        }
        for sql in &statements {
            let failure = match conn.query_drop(sql.as_str()).await {
                Ok(()) if conn.affected_rows() == 1 => continue,
                Ok(()) => DbError::ConcurrentModification,
                Err(e) => err(e),
            };
            let _ = conn.query_drop(if keep_open { "ROLLBACK TO SAVEPOINT quarry_edits" } else { "ROLLBACK" }).await;
            return Err(failure);
        }
        conn.query_drop(if keep_open { "RELEASE SAVEPOINT quarry_edits" } else { "COMMIT" }).await.map_err(err)?;
        Ok(statements.len())
    }

    fn tx(&self) -> &TxState {
        &self.tx
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn dialect(&self) -> &'static dyn crate::Dialect {
        &crate::dialect::Mysql
    }

    /// MySQL keeps each table's DDL: SHOW CREATE TABLE has the indexes and constraints too
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        let name = self.dialect().qualified(&table.schema_name, &table.table_name);
        let ddl = self.run(&format!("SHOW CREATE TABLE {name}")).await?;
        let ddl = ddl.rows.first().and_then(|r| r.get(1)).map(|v| format!("{};", v.to_display_string())).unwrap_or_default();
        let indexes = self
            .run(&format!(
                "SELECT INDEX_NAME, NON_UNIQUE = 0, GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ', ') \
                 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} GROUP BY INDEX_NAME, NON_UNIQUE ORDER BY 1",
                sql_string(&table.schema_name),
                sql_string(&table.table_name)
            ))
            .await?;
        let indexes = indexes
            .rows
            .iter()
            .map(|r| IndexInfo { name: r[0].to_display_string(), unique: matches!(r[1], DbValue::Int(1)), columns: r[2].to_display_string() })
            .collect();
        Ok(TableInfo { ddl, indexes })
    }

    async fn objects(&self, schema: &str) -> Result<Vec<SchemaObject>, DbError> {
        let q = sql_string(schema);
        let r = self
            .run(&format!(
                "SELECT ROUTINE_TYPE, ROUTINE_NAME, '' FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = {q} \
                 UNION ALL SELECT 'TRIGGER', TRIGGER_NAME, CONCAT('on ', EVENT_OBJECT_TABLE) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = {q} \
                 ORDER BY 1, 2"
            ))
            .await?;
        Ok(r.rows
            .iter()
            .filter_map(|row| {
                let kind = match row[0].to_display_string().as_str() {
                    "FUNCTION" => ObjectKind::Function,
                    "PROCEDURE" => ObjectKind::Procedure,
                    "TRIGGER" => ObjectKind::Trigger,
                    _ => return None,
                };
                Some(SchemaObject { kind, name: row[1].to_display_string(), detail: row[2].to_display_string(), id: String::new() })
            })
            .collect())
    }

    async fn object_ddl(&self, schema: &str, obj: &SchemaObject) -> Result<String, DbError> {
        let what = match obj.kind {
            ObjectKind::Function => "FUNCTION",
            ObjectKind::Procedure => "PROCEDURE",
            ObjectKind::Trigger => "TRIGGER",
            _ => return Err(DbError::Internal(format!("MySQL has no {:?} objects", obj.kind))),
        };
        let r = self.run(&format!("SHOW CREATE {what} {}", self.dialect().qualified(schema, &obj.name))).await?;
        // The statement is the third column for all three
        r.rows.first().and_then(|row| row.get(2)).map(|v| format!("{};", v.to_display_string())).ok_or_else(|| DbError::SqlError(format!("{} no longer exists", obj.name)))
    }

    fn monitor(&self) -> Option<&dyn crate::monitor::Monitor> {
        Some(self)
    }

    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        Some(self)
    }
}

#[async_trait]
impl crate::dump::Dumper for MysqlSession {
    /// mysqldump; the password goes through MYSQL_PWD so it never shows in the process list
    async fn dump(&self, out: &std::path::Path) -> Result<String, String> {
        let [host, port, user, pw, db] = <[String; 5]>::try_from(DRIVER.split(&self.url)).unwrap_or_default();
        if db.is_empty() {
            return Err("Pick a database in the connection's URL to dump it".into());
        }
        let mut cmd = std::process::Command::new("mysqldump");
        cmd.args(["--single-transaction", "--routines", "--triggers", "-h", &host]);
        if !port.is_empty() {
            cmd.args(["-P", &port]);
        }
        if !user.is_empty() {
            cmd.args(["-u", &user]);
        }
        if !pw.is_empty() {
            cmd.env("MYSQL_PWD", pw);
        }
        cmd.arg("--result-file").arg(out).arg(&db);
        match tokio::task::spawn_blocking(move || cmd.output()).await.map_err(|e| e.to_string())? {
            Ok(o) if o.status.success() => Ok(format!("{db}")),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err("mysqldump isn't installed. Install the MySQL client tools (e.g. `brew install mysql-client`) and try again.".into())
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

impl MysqlSession {
    /// Everything (`only` = None) or one schema: `only` is a SQL expression for its name
    async fn catalog_where(&self, only: Option<&str>) -> Result<SchemaCatalog, DbError> {
        let f = only.map(|e| format!(" AND TABLE_SCHEMA = {e}")).unwrap_or_default();
        let schemas = self.column(&format!("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN {SYSTEM} ORDER BY 1")).await?;
        let tables = self
            .run(&format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE = 'VIEW' FROM information_schema.TABLES WHERE TABLE_SCHEMA NOT IN {SYSTEM}{f} ORDER BY 1, 2"
            ))
            .await?;
        let columns = self
            .run(&format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE = 'YES', COLUMN_DEFAULT, ORDINAL_POSITION, COLUMN_KEY = 'PRI' \
                 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA NOT IN {SYSTEM}{f} ORDER BY 1, 2, ORDINAL_POSITION"
            ))
            .await?;
        let fks = self
            .run(&format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
                 FROM information_schema.KEY_COLUMN_USAGE WHERE REFERENCED_TABLE_NAME IS NOT NULL AND TABLE_SCHEMA NOT IN {SYSTEM}{f}"
            ))
            .await?;
        let default_schema = self.column("SELECT DATABASE()").await?.into_iter().next().filter(|s| s != "NULL");
        let s = |v: &DbValue| v.to_display_string();
        let truthy = |v: &DbValue| matches!(v, DbValue::Int(1));
        let tables = tables
            .rows
            .iter()
            .map(|t| {
                let (schema, name) = (s(&t[0]), s(&t[1]));
                let mine = |r: &&Vec<DbValue>| s(&r[0]) == schema && s(&r[1]) == name;
                let columns: Vec<ColumnSchema> = columns
                    .rows
                    .iter()
                    .filter(mine)
                    .map(|c| ColumnSchema {
                        name: s(&c[2]),
                        data_type: ColumnType::from_sql_str(&s(&c[3])),
                        raw_type_name: s(&c[3]),
                        is_nullable: truthy(&c[4]),
                        is_primary_key: truthy(&c[7]),
                        default_value: (!c[5].is_null()).then(|| s(&c[5])),
                        ordinal_position: s(&c[6]).parse().unwrap_or(0),
                    })
                    .collect();
                TableSchema {
                    primary_keys: columns.iter().filter(|c| c.is_primary_key).map(|c| c.name.clone()).collect(),
                    foreign_keys: fks
                        .rows
                        .iter()
                        .filter(mine)
                        .map(|f| ForeignKeyRef { from_column: s(&f[2]), to_schema: s(&f[3]), to_table: s(&f[4]), to_column: s(&f[5]) })
                        .collect(),
                    schema_name: schema,
                    table_name: name,
                    is_view: truthy(&t[2]),
                    columns,
                }
            })
            .collect();
        Ok(SchemaCatalog { schemas, tables, default_schema, ..Default::default() })
    }
}
