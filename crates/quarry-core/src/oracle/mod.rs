// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Oracle through rust-oracle (ODPI-C). Needs Oracle Instant Client at connect time, found via
//! ORACLE_CLIENT_LIB_DIR, the usual install folders or Quarry's own download (see `client`). The client blocks, so every call runs on the blocking pool. Schemas are users.

use crate::conn_url::{self, UrlFields};
use crate::dialect::is_plsql_block;
use crate::driver::{Driver, Field};
use crate::edits::PendingChanges;
use crate::monitor::{Monitor, ServerView, Snapshot};
use crate::schema::{
    ColumnSchema, ColumnType, ForeignKeyRef, IndexInfo, ObjectKind, SchemaCatalog, SchemaObject,
    TableInfo, TableSchema, EAGER_SCHEMAS,
};
use crate::session::{
    is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState, TxVerb, MAX_ROWS,
};
use crate::value::DbValue;
use crate::RowBuffer;
use async_trait::async_trait;
use oracle::sql_type::OracleType;
use oracle::Connection;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SCHEMES: [&str; 1] = ["oracle"];
pub mod client;

const NO_CLIENT: &str = "Oracle Instant Client isn't installed";

pub struct OracleDriver;
pub static DRIVER: OracleDriver = OracleDriver;

const fn field(
    label: &'static str,
    placeholder: &'static str,
    secret: bool,
    row: u8,
    width: Option<f32>,
) -> Field {
    Field {
        label,
        placeholder,
        secret,
        row,
        width,
    }
}

#[async_trait]
impl Driver for OracleDriver {
    fn id(&self) -> &'static str {
        "oracle"
    }

    fn name(&self) -> &'static str {
        "Oracle"
    }

    fn sample(&self) -> &'static str {
        "oracle://system@localhost:1521/FREEPDB1"
    }

    fn recognizes(&self, conn: &str) -> bool {
        conn.starts_with("oracle://")
    }

    fn fields(&self) -> &'static [Field] {
        const FIELDS: [Field; 5] = [
            field("Host", "localhost", false, 0, None),
            field("Port", "1521", false, 0, Some(120.0)),
            field("User", "system", false, 1, None),
            field("Password", "", true, 1, None),
            field("Service", "FREEPDB1", false, 2, None),
        ];
        &FIELDS
    }

    fn split(&self, conn: &str) -> Vec<String> {
        conn_url::parse(conn, &SCHEMES)
            .map(|(f, _)| f.to_vec())
            .unwrap_or_else(|| vec![String::new(); 5])
    }

    fn join(&self, values: &[String], previous: &str) -> String {
        let query = conn_url::parse(previous, &SCHEMES)
            .map(|(_, q)| q)
            .unwrap_or_default();
        let fields: UrlFields = std::array::from_fn(|i| values.get(i).cloned().unwrap_or_default());
        conn_url::build("oracle", &fields, &query)
    }

    fn take_secret(&self, conn: &str) -> (String, Option<String>) {
        conn_url::split_password(conn).unwrap_or_else(|| (conn.to_string(), None))
    }

    fn put_secret(&self, conn: &str, secret: &str) -> String {
        conn_url::insert_password(conn, secret)
    }

    fn display_address(&self, conn: &str) -> String {
        let [host, port, _, _, service] =
            <[String; 5]>::try_from(self.split(conn)).unwrap_or_default();
        let host = if host.is_empty() {
            "localhost".to_string()
        } else {
            host
        };
        let mut out = if port.is_empty() {
            host
        } else {
            format!("{host}:{port}")
        };
        if !service.is_empty() {
            out += &format!("/{service}");
        }
        out
    }

    fn tunnel_target(&self, conn: &str) -> Option<(String, u16)> {
        conn_url::target(conn, 1521)
    }

    fn through_tunnel(&self, conn: &str, local_port: u16) -> String {
        conn_url::retarget(conn, local_port)
    }

    async fn connect(
        &self,
        conn: &str,
        read_only: bool,
    ) -> Result<Arc<dyn DatabaseSession>, DbError> {
        Ok(Arc::new(OracleSession::connect(conn, read_only).await?))
    }
}

/// Whether a connect error means Instant Client is missing (so the app can offer the download)
pub fn client_missing(error: &str) -> bool {
    error.contains(NO_CLIENT)
}

/// Points ODPI-C at Instant Client: ORACLE_CLIENT_LIB_DIR, then a system install, then Quarry's
/// own download. Only success is remembered, so connecting again after a download works.
fn init_client() -> Result<(), DbError> {
    static READY: OnceLock<()> = OnceLock::new();
    if READY.get().is_some() {
        return Ok(());
    }
    let res = (|| -> Result<(), String> {
        let mut candidates: Vec<std::path::PathBuf> = std::env::var_os("ORACLE_CLIENT_LIB_DIR")
            .map(Into::into)
            .into_iter()
            .collect();
        let globbed = |dir: std::path::PathBuf, prefix: &str| -> Vec<std::path::PathBuf> {
            let mut found: Vec<_> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
                })
                .collect();
            found.sort();
            found.reverse(); // newest version first
            found
        };
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            candidates.extend(globbed(home.join("Downloads"), "instantclient_"));
        }
        candidates.extend(globbed("/opt/oracle".into(), "instantclient"));
        candidates.extend(client::installed());
        let mut params = oracle::InitParams::new();
        if let Some(dir) = candidates.into_iter().find(|d| client::has_client_lib(d)) {
            params
                .oracle_client_lib_dir(dir)
                .map_err(|e| e.to_string())?;
        }
        // Nothing found: ODPI-C still tries its own search (LD_LIBRARY_PATH, ~/lib, …)
        params.init().map(|_| ()).map_err(|e| e.to_string())
    })();
    match res {
        Ok(()) => {
            let _ = READY.set(());
            Ok(())
        }
        // Linux Instant Client needs libaio, a system package we can't install
        Err(e) if e.contains("libaio") => Err(DbError::ConnectionFailed(
            "Oracle's client needs libaio. Install it with `sudo apt install libaio1t64` (Ubuntu 24.04+; `libaio1` on older) or `sudo dnf install libaio`, then connect again.".into(),
        )),
        // DPI-1047: no client library where ODPI-C looked
        Err(e) if e.contains("DPI-1047") => Err(DbError::ConnectionFailed(NO_CLIENT.into())),
        Err(e) => Err(DbError::ConnectionFailed(e)),
    }
}

pub struct OracleSession {
    conn: Arc<Mutex<Connection>>,
    read_only: bool,
    tx: TxState,
}

fn err(e: oracle::Error) -> DbError {
    let msg = e.to_string();
    // DPI-1047: the client library couldn't be loaded
    if msg.contains("DPI-1047") {
        return DbError::ConnectionFailed(NO_CLIENT.into());
    }
    DbError::SqlError(msg)
}

/// SQL statements reach Oracle without their `;` (it rejects them); PL/SQL blocks keep it
fn prepare(sql: &str) -> String {
    let sql = sql.trim();
    if is_plsql_block(sql) {
        sql.to_string()
    } else {
        sql.trim_end_matches(';').trim_end().to_string()
    }
}

/// One statement: rows (capped at MAX_ROWS) for a query, the row count otherwise
fn run_statement(conn: &Connection, sql: &str) -> Result<QueryResult, DbError> {
    let start = Instant::now();
    let text = prepare(sql);
    let mut stmt = conn.statement(&text).build().map_err(err)?;
    if !stmt.is_query() {
        stmt.execute(&[]).map_err(err)?;
        return Ok(QueryResult {
            columns: Vec::new(),
            rows: RowBuffer::default(),
            rows_affected: stmt.row_count().map_err(err)?,
            execution_time: start.elapsed(),
            statement: sql.to_string(),
        });
    }
    let rs = stmt.query(&[]).map_err(err)?;
    let info: Vec<(String, OracleType)> = rs
        .column_info()
        .iter()
        .map(|c| (c.name().to_string(), c.oracle_type().clone()))
        .collect();
    let mut row_buffer = RowBuffer::new(info.len());
    let mut count = 0u64;
    for row in rs {
        let row = row.map_err(err)?;
        count += 1;
        if row_buffer.len() < MAX_ROWS {
            for (i, (v, (_, t))) in row.sql_values().iter().zip(&info).enumerate() {
                row_buffer.column_mut(i).push(decode(v, t));
            }
            row_buffer.increment_row_count();
        }
    }
    Ok(QueryResult {
        columns: info.into_iter().map(|(n, _)| n).collect(),
        rows: row_buffer,
        rows_affected: count,
        execution_time: start.elapsed(),
        statement: sql.to_string(),
    })
}

fn decode(v: &oracle::SqlValue, t: &OracleType) -> DbValue {
    if v.is_null().unwrap_or(true) {
        return DbValue::Null;
    }
    let text = || {
        v.get::<String>()
            .map(DbValue::Text)
            .unwrap_or(DbValue::Null)
    };
    match t {
        // Whole numbers that fit an i64 are integers (NUMBER(10), COUNT(*)); anything else keeps its exact digits
        OracleType::Number(_, _) | OracleType::Float(_) => match v.get::<String>() {
            Ok(s) => s
                .parse::<i64>()
                .map(DbValue::Int)
                .unwrap_or(DbValue::Decimal(s)),
            Err(_) => DbValue::Null,
        },
        OracleType::BinaryFloat | OracleType::BinaryDouble => v
            .get::<f64>()
            .map(DbValue::Float)
            .unwrap_or_else(|_| text()),
        OracleType::Raw(_) | OracleType::BLOB | OracleType::LongRaw => v
            .get::<Vec<u8>>()
            .map(DbValue::Blob)
            .unwrap_or(DbValue::Null),
        OracleType::Boolean => v
            .get::<bool>()
            .map(DbValue::Boolean)
            .unwrap_or_else(|_| text()),
        OracleType::Json => v
            .get::<String>()
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .map(DbValue::Json)
            .unwrap_or_else(text),
        // Dates, timestamps, intervals, text: as Oracle formats them, so nothing shifts or rounds
        _ => text(),
    }
}

impl OracleSession {
    pub async fn connect(url: &str, read_only: bool) -> Result<Self, DbError> {
        let (f, _) = conn_url::parse(url, &SCHEMES)
            .ok_or_else(|| DbError::ConnectionFailed("Not an oracle:// address".into()))?;
        let [host, port, user, pw, service] = f;
        let target = format!(
            "//{}:{}/{service}",
            if host.is_empty() { "localhost" } else { &host },
            if port.is_empty() { "1521" } else { &port }
        );
        let conn = tokio::task::spawn_blocking(move || -> Result<Connection, DbError> {
            init_client()?;
            let mut conn = Connection::connect(&user, &pw, &target).map_err(|e| match err(e) {
                DbError::SqlError(m) => DbError::ConnectionFailed(m),
                other => other,
            })?;
            conn.set_autocommit(true);
            if read_only {
                conn.execute("SET TRANSACTION READ ONLY", &[])
                    .map_err(err)?;
            }
            Ok(conn)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))??;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            read_only,
            tx: TxState::default(),
        })
    }

    /// Runs `f` with the connection on the blocking pool
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

    async fn run(&self, sql: &str) -> Result<QueryResult, DbError> {
        let sql = sql.to_string();
        self.with(move |c| run_statement(c, &sql)).await
    }

    async fn rows(&self, sql: &str) -> Result<Vec<Vec<String>>, DbError> {
        Ok(self
            .run(sql)
            .await?
            .rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|v| {
                        if v.is_null() {
                            String::new()
                        } else {
                            v.to_display_string()
                        }
                    })
                    .collect()
            })
            .collect())
    }
}

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Schemas made by Oracle itself (SYS, XDB, …) are hidden
const USER_SCHEMAS: &str = "SELECT username FROM all_users WHERE oracle_maintained = 'N'";

#[async_trait]
impl DatabaseSession for OracleSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        let start = Instant::now();
        self.with(|c| c.ping().map_err(err)).await?;
        Ok(start.elapsed())
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        self.catalog_where(None).await
    }

    /// Many schemas: only the session's own, the rest when opened
    async fn load_catalog(&self) -> Result<SchemaCatalog, DbError> {
        let c = self
            .catalog_where(Some("SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA')"))
            .await?;
        if c.schemas.len() > EAGER_SCHEMAS {
            Ok(c.lazy())
        } else {
            self.introspect().await
        }
    }

    async fn introspect_schema(&self, schema: &str) -> Result<Vec<TableSchema>, DbError> {
        Ok(self
            .catalog_where(Some(&self.dialect().literal(&DbValue::Text(schema.into()))))
            .await?
            .tables)
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        if self.read_only && is_mutation(sql) {
            return Err(DbError::ReadOnlyViolation);
        }
        // BEGIN starts a PL/SQL block in Oracle, not a transaction: only COMMIT/ROLLBACK matter here
        let ends = matches!(tx_verb(sql), Some(TxVerb::End)) && !is_plsql_block(sql);
        // Manual-commit mode: autocommit goes off at the first write and back on at COMMIT/ROLLBACK (E9)
        let opens = self.tx.manual() && !self.tx.open() && !ends && !is_read(sql);
        let sql = sql.to_string();
        let result = self
            .with(move |c| {
                if opens {
                    c.set_autocommit(false);
                }
                let r = run_statement(c, &sql);
                if ends {
                    c.set_autocommit(true);
                }
                r
            })
            .await;
        if opens {
            self.tx.set_open(true);
        }
        if ends {
            self.tx.set_open(false);
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
        let keep_open = self.tx.open() || self.tx.manual();
        let n = statements.len();
        let result = self
            .with(move |c| {
                // The batch is one transaction: autocommit off until it commits or rolls back
                c.set_autocommit(false);
                if keep_open {
                    run_statement(c, "SAVEPOINT quarry_edits")?;
                }
                for sql in &statements {
                    let failure = match run_statement(c, sql) {
                        Ok(r) if r.rows_affected == 1 => continue,
                        Ok(_) => DbError::ConcurrentModification,
                        Err(e) => e,
                    };
                    let _ = if keep_open {
                        run_statement(c, "ROLLBACK TO SAVEPOINT quarry_edits").map(|_| ())
                    } else {
                        c.rollback().map_err(err)
                    };
                    if !keep_open {
                        c.set_autocommit(true);
                    }
                    return Err(failure);
                }
                if !keep_open {
                    c.commit().map_err(err)?;
                    c.set_autocommit(true);
                }
                Ok(n)
            })
            .await;
        if keep_open {
            self.tx.set_open(true);
        }
        result
    }

    fn tx(&self) -> &TxState {
        &self.tx
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn dialect(&self) -> &'static dyn crate::Dialect {
        &crate::dialect::Oracle
    }

    /// DBMS_METADATA writes the full DDL: storage, constraints and all
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        let kind = if table.is_view { "VIEW" } else { "TABLE" };
        let (owner, name) = (
            sql_string(&table.schema_name),
            sql_string(&table.table_name),
        );
        let ddl = self
            .rows(&format!(
                "SELECT DBMS_METADATA.GET_DDL('{kind}', {name}, {owner}) FROM dual"
            ))
            .await?;
        let indexes = self
            .rows(&format!(
                "SELECT i.index_name, CASE i.uniqueness WHEN 'UNIQUE' THEN 1 ELSE 0 END, \
                   LISTAGG(c.column_name, ', ') WITHIN GROUP (ORDER BY c.column_position) \
                 FROM all_indexes i JOIN all_ind_columns c ON c.index_owner = i.owner AND c.index_name = i.index_name \
                 WHERE i.table_owner = {owner} AND i.table_name = {name} GROUP BY i.index_name, i.uniqueness ORDER BY 1"
            ))
            .await?;
        Ok(TableInfo {
            ddl: ddl
                .into_iter()
                .next()
                .map(|r| format!("{};", r[0].trim()))
                .unwrap_or_else(|| table.create_sql(self.dialect())),
            indexes: indexes
                .into_iter()
                .map(|r| IndexInfo {
                    name: r[0].clone(),
                    unique: r[1] == "1",
                    columns: r[2].clone(),
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn objects(&self, schema: &str) -> Result<Vec<SchemaObject>, DbError> {
        let rows = self
            .rows(&format!(
                "SELECT o.object_type, o.object_name, CASE WHEN o.object_type = 'TRIGGER' THEN 'on ' || t.table_name END \
                 FROM all_objects o LEFT JOIN all_triggers t ON o.object_type = 'TRIGGER' AND t.owner = o.owner AND t.trigger_name = o.object_name \
                 WHERE o.owner = {} AND o.object_type IN ('FUNCTION', 'PROCEDURE', 'PACKAGE', 'SEQUENCE', 'TRIGGER', 'TYPE') ORDER BY 1, 2",
                sql_string(schema)
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let kind = match r[0].as_str() {
                    "FUNCTION" => ObjectKind::Function,
                    // A package groups procedures and functions; it opens as one definition
                    "PROCEDURE" | "PACKAGE" => ObjectKind::Procedure,
                    "SEQUENCE" => ObjectKind::Sequence,
                    "TRIGGER" => ObjectKind::Trigger,
                    "TYPE" => ObjectKind::Type,
                    _ => return None,
                };
                Some(SchemaObject {
                    kind,
                    name: r[1].clone(),
                    detail: r[2].clone(),
                    id: r[0].clone(),
                })
            })
            .collect())
    }

    async fn object_ddl(&self, schema: &str, obj: &SchemaObject) -> Result<String, DbError> {
        // `id` holds Oracle's object type, which DBMS_METADATA takes as is
        let rows = self
            .rows(&format!(
                "SELECT DBMS_METADATA.GET_DDL({}, {}, {}) FROM dual",
                sql_string(&obj.id),
                sql_string(&obj.name),
                sql_string(schema)
            ))
            .await?;
        rows.into_iter()
            .next()
            .map(|r| format!("{}\n/", r[0].trim()))
            .ok_or_else(|| DbError::SqlError(format!("{} no longer exists", obj.name)))
    }

    fn monitor(&self) -> Option<&dyn Monitor> {
        Some(self)
    }
}

const MONITOR_HELP: &str = "Monitoring reads V$SESSION and V$SQL, which needs SELECT_CATALOG_ROLE (or SELECT ANY DICTIONARY): \
GRANT SELECT_CATALOG_ROLE TO your_user;";

#[async_trait]
impl Monitor for OracleSession {
    fn views(&self) -> &'static [ServerView] {
        &[
            ServerView::Activity,
            ServerView::Statements,
            ServerView::Locks,
        ]
    }

    async fn snapshot(&self, view: ServerView) -> Result<Snapshot, DbError> {
        let sql = match view {
            // SID first: cancel/terminate take it (the serial# is looked up when needed)
            ServerView::Activity => {
                "SELECT s.sid, s.username AS \"user\", s.machine AS client, s.status, s.event AS waiting_on, \
                 s.last_call_et AS seconds, SUBSTR(q.sql_text, 1, 500) AS query \
                 FROM v$session s LEFT JOIN v$sql q ON q.sql_id = s.sql_id AND q.child_number = 0 \
                 WHERE s.type = 'USER' AND s.sid <> SYS_CONTEXT('USERENV', 'SID') ORDER BY s.last_call_et DESC"
            }
            ServerView::Statements => {
                "SELECT executions AS calls, ROUND(elapsed_time / 1000, 1) AS total_ms, \
                 ROUND(elapsed_time / NULLIF(executions, 0) / 1000, 2) AS mean_ms, rows_processed AS \"rows\", SUBSTR(sql_text, 1, 500) AS query \
                 FROM v$sql ORDER BY elapsed_time DESC FETCH FIRST 200 ROWS ONLY"
            }
            ServerView::Locks => {
                "SELECT l.sid, l.type, l.lmode AS mode_held, l.request AS mode_wanted, o.object_name AS relation, l.block AS blocking \
                 FROM v$lock l LEFT JOIN all_objects o ON o.object_id = l.id1 WHERE l.type IN ('TM', 'TX') ORDER BY l.block DESC, l.sid"
            }
            _ => return Ok(Snapshot::Unavailable("Not available for Oracle".into())),
        };
        match self.run(sql).await {
            Err(e) if e.to_string().contains("ORA-00942") => {
                Ok(Snapshot::Unavailable(MONITOR_HELP.into()))
            }
            other => other.map(Snapshot::Rows),
        }
    }

    async fn cancel(&self, sid: i64) -> Result<(), DbError> {
        let serial = self
            .rows(&format!("SELECT serial# FROM v$session WHERE sid = {sid}"))
            .await?;
        let serial = serial
            .first()
            .map(|r| r[0].clone())
            .ok_or_else(|| DbError::SqlError(format!("session {sid} has ended")))?;
        self.run(&format!("ALTER SYSTEM CANCEL SQL '{sid},{serial}'"))
            .await
            .map(|_| ())
    }

    async fn terminate(&self, sid: i64) -> Result<(), DbError> {
        let serial = self
            .rows(&format!("SELECT serial# FROM v$session WHERE sid = {sid}"))
            .await?;
        let serial = serial
            .first()
            .map(|r| r[0].clone())
            .ok_or_else(|| DbError::SqlError(format!("session {sid} has ended")))?;
        self.run(&format!(
            "ALTER SYSTEM KILL SESSION '{sid},{serial}' IMMEDIATE"
        ))
        .await
        .map(|_| ())
    }
}

impl OracleSession {
    /// Everything (`only` = None) or one schema: `only` is a SQL expression for its name
    async fn catalog_where(&self, only: Option<&str>) -> Result<SchemaCatalog, DbError> {
        let f = |col: &str| {
            only.map(|e| format!(" AND {col} = {e}"))
                .unwrap_or_default()
        };
        let (owner, c_owner) = (f("owner"), f("c.owner"));
        let schemas: Vec<String> = self
            .rows(&format!("{USER_SCHEMAS} ORDER BY 1"))
            .await?
            .into_iter()
            .map(|r| r[0].clone())
            .collect();
        let tables = self
            .rows(&format!(
                "SELECT owner, table_name, 0 FROM all_tables WHERE owner IN ({USER_SCHEMAS}){owner} AND nested = 'NO' AND secondary = 'N' \
                 UNION ALL SELECT owner, view_name, 1 FROM all_views WHERE owner IN ({USER_SCHEMAS}){owner} ORDER BY 1, 2"
            ))
            .await?;
        let columns = self
            .rows(&format!(
                "SELECT owner, table_name, column_name, \
                   CASE WHEN data_type IN ('VARCHAR2', 'NVARCHAR2', 'CHAR', 'NCHAR', 'RAW') THEN data_type || '(' || char_length || ')' \
                        WHEN data_type = 'NUMBER' AND data_precision IS NOT NULL THEN 'NUMBER(' || data_precision || ',' || data_scale || ')' \
                        ELSE data_type END, \
                   CASE nullable WHEN 'Y' THEN 1 ELSE 0 END, data_default, column_id \
                 FROM all_tab_columns WHERE owner IN ({USER_SCHEMAS}){owner} ORDER BY 1, 2, column_id"
            ))
            .await?;
        let pks = self
            .rows(&format!(
                "SELECT c.owner, c.table_name, cc.column_name FROM all_constraints c \
                 JOIN all_cons_columns cc ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
                 WHERE c.constraint_type = 'P' AND c.owner IN ({USER_SCHEMAS}){c_owner} ORDER BY cc.position"
            ))
            .await?;
        let fks = self
            .rows(&format!(
                "SELECT c.owner, c.table_name, cc.column_name, r.owner, r.table_name, rc.column_name FROM all_constraints c \
                 JOIN all_cons_columns cc ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
                 JOIN all_constraints r ON r.owner = c.r_owner AND r.constraint_name = c.r_constraint_name \
                 JOIN all_cons_columns rc ON rc.owner = r.owner AND rc.constraint_name = r.constraint_name AND rc.position = cc.position \
                 WHERE c.constraint_type = 'R' AND c.owner IN ({USER_SCHEMAS}){c_owner}"
            ))
            .await?;
        let default_schema = self
            .rows("SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual")
            .await?
            .into_iter()
            .next()
            .map(|r| r[0].clone());
        let tables = tables
            .iter()
            .map(|t| {
                let mine = |r: &&Vec<String>| r[0] == t[0] && r[1] == t[1];
                let primary_keys: Vec<String> =
                    pks.iter().filter(mine).map(|r| r[2].clone()).collect();
                let columns = columns
                    .iter()
                    .filter(mine)
                    .map(|c| ColumnSchema {
                        name: c[2].clone(),
                        data_type: ColumnType::from_sql_str(&c[3]),
                        raw_type_name: c[3].clone(),
                        is_nullable: c[4] == "1",
                        is_primary_key: primary_keys.contains(&c[2]),
                        default_value: Some(c[5].trim().to_string()).filter(|d| !d.is_empty()),
                        ordinal_position: c[6].parse().unwrap_or(0),
                    })
                    .collect();
                TableSchema {
                    schema_name: t[0].clone(),
                    table_name: t[1].clone(),
                    is_view: t[2] == "1",
                    columns,
                    foreign_keys: fks
                        .iter()
                        .filter(mine)
                        .map(|f| ForeignKeyRef {
                            from_column: f[2].clone(),
                            to_schema: f[3].clone(),
                            to_table: f[4].clone(),
                            to_column: f[5].clone(),
                        })
                        .collect(),
                    primary_keys,
                }
            })
            .collect();
        Ok(SchemaCatalog {
            schemas,
            tables,
            default_schema,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_semicolons_from_sql_but_not_from_blocks() {
        assert_eq!(super::prepare("select 1 from dual;"), "select 1 from dual");
        assert_eq!(super::prepare("BEGIN NULL; END;"), "BEGIN NULL; END;");
    }
}
