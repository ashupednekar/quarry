// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! SQL Server through tiberius (TDS 7.3, so SQL Server 2008 and newer). Schemas are the ones in
//! the connected database (dbo, …); switching databases is a new connection.

use crate::conn_url::{self, UrlFields};
use crate::driver::{Driver, Field};
use crate::edits::PendingChanges;
use crate::monitor::{Monitor, ServerView, Snapshot};
use crate::schema::{
    ColumnSchema, ColumnType, ForeignKeyRef, IndexInfo, ObjectKind, SchemaCatalog, SchemaObject,
    TableInfo, TableSchema,
};
use crate::session::{
    is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState, TxVerb, MAX_ROWS,
};
use crate::value::DbValue;
use crate::RowBuffer;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiberius::{AuthMethod, Client, ColumnData, Config, EncryptionLevel, FromSql, QueryItem};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

const SCHEMES: [&str; 2] = ["sqlserver", "mssql"];
/// Column name of the count appended to every batch; see `run_batch`
const ROWCOUNT: &str = "quarry_rowcount";

pub struct MssqlDriver;
pub static DRIVER: MssqlDriver = MssqlDriver;

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
impl Driver for MssqlDriver {
    fn id(&self) -> &'static str {
        "mssql"
    }

    fn name(&self) -> &'static str {
        "SQL Server"
    }

    fn sample(&self) -> &'static str {
        "sqlserver://sa@localhost:1433/master"
    }

    fn recognizes(&self, conn: &str) -> bool {
        SCHEMES.iter().any(|s| conn.starts_with(&format!("{s}://")))
    }

    fn fields(&self) -> &'static [Field] {
        const FIELDS: [Field; 5] = [
            field("Host", "localhost", false, 0, None),
            field("Port", "1433", false, 0, Some(120.0)),
            field("User", "sa", false, 1, None),
            field("Password", "", true, 1, None),
            field("Database", "master", false, 2, None),
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
        conn_url::build("sqlserver", &fields, &query)
    }

    fn take_secret(&self, conn: &str) -> (String, Option<String>) {
        conn_url::split_password(conn).unwrap_or_else(|| (conn.to_string(), None))
    }

    fn put_secret(&self, conn: &str, secret: &str) -> String {
        conn_url::insert_password(conn, secret)
    }

    fn display_address(&self, conn: &str) -> String {
        let [host, port, _, _, db] = <[String; 5]>::try_from(self.split(conn)).unwrap_or_default();
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
        if !db.is_empty() {
            out += &format!("/{db}");
        }
        out
    }

    fn tunnel_target(&self, conn: &str) -> Option<(String, u16)> {
        conn_url::target(conn, 1433)
    }

    fn through_tunnel(&self, conn: &str, local_port: u16) -> String {
        conn_url::retarget(conn, local_port)
    }

    async fn connect(
        &self,
        conn: &str,
        read_only: bool,
    ) -> Result<Arc<dyn DatabaseSession>, DbError> {
        Ok(Arc::new(MssqlSession::connect(conn, read_only).await?))
    }
}

pub struct MssqlSession {
    client: Mutex<Client<Compat<TcpStream>>>,
    read_only: bool,
    tx: TxState,
}

fn err(e: tiberius::error::Error) -> DbError {
    DbError::SqlError(e.to_string())
}

/// Statements that must be alone in their batch, so nothing can be appended after them
fn must_stand_alone(sql: &str) -> bool {
    let words: Vec<String> = sql
        .split_whitespace()
        .take(4)
        .map(|w| w.to_ascii_uppercase())
        .collect();
    let verb = words
        .iter()
        .position(|w| w == "CREATE" || w == "ALTER")
        .map(|i| words.get(i + 1).map(String::as_str));
    matches!(
        verb,
        Some(Some(
            "PROCEDURE" | "PROC" | "FUNCTION" | "TRIGGER" | "VIEW" | "SCHEMA"
        ))
    ) || words
        .first()
        .is_some_and(|w| w == "CREATE" && words.get(1).is_some_and(|x| x == "OR"))
}

impl MssqlSession {
    pub async fn connect(url: &str, read_only: bool) -> Result<Self, DbError> {
        let fail = |e: String| DbError::ConnectionFailed(e);
        let (f, _) = conn_url::parse(url, &SCHEMES)
            .ok_or_else(|| fail("Not a sqlserver:// address".into()))?;
        let [host, port, user, pw, db] = f;
        let mut config = Config::new();
        config.host(if host.is_empty() { "localhost" } else { &host });
        config.port(port.parse().unwrap_or(1433));
        config.authentication(AuthMethod::sql_server(&user, &pw));
        if !db.is_empty() {
            config.database(&db);
        }
        // ponytail: no TLS yet (tiberius built without a TLS backend); servers that force encryption refuse
        config.encryption(EncryptionLevel::NotSupported);
        let tcp = TcpStream::connect(config.get_addr())
            .await
            .map_err(|e| fail(e.to_string()))?;
        tcp.set_nodelay(true).map_err(|e| fail(e.to_string()))?;
        let client = Client::connect(config, tcp.compat_write())
            .await
            .map_err(|e| fail(e.to_string()))?;
        Ok(Self {
            client: Mutex::new(client),
            read_only,
            tx: TxState::default(),
        })
    }

    async fn run(&self, sql: &str) -> Result<QueryResult, DbError> {
        run_batch(&mut *self.client.lock().await, sql).await
    }

    /// One row per result, as text cells (catalog queries)
    async fn rows(&self, sql: &str) -> Result<Vec<Vec<String>>, DbError> {
        Ok(self
            .run(sql)
            .await?
            .rows
            .iter()
            .map(|r| r.iter().map(|v| v.to_display_string()).collect())
            .collect())
    }
}

/// Runs one batch. tiberius doesn't surface row counts from a plain batch, so `SELECT @@ROWCOUNT`
/// rides along after it (except where the statement must be alone). The first result set is the
/// one shown; rows past MAX_ROWS are counted, not kept.
async fn run_batch(
    client: &mut Client<Compat<TcpStream>>,
    sql: &str,
) -> Result<QueryResult, DbError> {
    let start = Instant::now();
    let counted = !must_stand_alone(sql);
    let batch = if counted {
        format!("{sql}\nSELECT @@ROWCOUNT AS {ROWCOUNT}")
    } else {
        sql.to_string()
    };
    let mut stream = client.simple_query(batch).await.map_err(err)?;
    let (mut columns, mut row_buffer, mut seen, mut affected) =
        (Vec::new(), RowBuffer::default(), 0u64, 0u64);
    let mut first_set: Option<usize> = None;
    while let Some(item) = stream.try_next().await.map_err(err)? {
        match item {
            QueryItem::Metadata(meta) => {
                let names: Vec<String> = meta
                    .columns()
                    .iter()
                    .map(|c| c.name().to_string())
                    .collect();
                if names != [ROWCOUNT] && first_set.is_none() {
                    first_set = Some(meta.result_index());
                    row_buffer = RowBuffer::new(names.len());
                    columns = names;
                }
            }
            QueryItem::Row(row) if Some(row.result_index()) == first_set => {
                seen += 1;
                if row_buffer.len() < MAX_ROWS {
                    for (i, (_, v)) in row.cells().enumerate() {
                        row_buffer.column_mut(i).push(decode(v));
                    }
                    row_buffer.increment_row_count();
                }
            }
            QueryItem::Row(row) => {
                if let Some(ColumnData::I32(Some(n))) = row.cells().next().map(|(_, v)| v) {
                    affected = *n as u64;
                }
            }
        }
    }
    Ok(QueryResult {
        rows_affected: if columns.is_empty() { affected } else { seen },
        columns,
        rows: row_buffer,
        execution_time: start.elapsed(),
        statement: sql.to_string(),
    })
}

fn decode(v: &ColumnData<'static>) -> DbValue {
    let opt = |o: Option<DbValue>| o.unwrap_or(DbValue::Null);
    let time = |s: Option<String>| opt(s.map(DbValue::Text));
    match v {
        ColumnData::U8(x) => opt(x.map(|n| DbValue::Int(n.into()))),
        ColumnData::I16(x) => opt(x.map(|n| DbValue::Int(n.into()))),
        ColumnData::I32(x) => opt(x.map(|n| DbValue::Int(n.into()))),
        ColumnData::I64(x) => opt(x.map(DbValue::Int)),
        ColumnData::F32(x) => opt(x.map(|n| DbValue::Float(n.into()))),
        ColumnData::F64(x) => opt(x.map(DbValue::Float)),
        ColumnData::Bit(x) => opt(x.map(DbValue::Boolean)),
        ColumnData::String(x) => opt(x.as_ref().map(|s| DbValue::Text(s.to_string()))),
        ColumnData::Guid(x) => opt(x.map(|g| DbValue::Text(g.to_string().to_uppercase()))),
        ColumnData::Binary(x) => opt(x.as_ref().map(|b| DbValue::Blob(b.to_vec()))),
        ColumnData::Numeric(x) => opt(x.map(|n| DbValue::Decimal(n.to_string()))),
        ColumnData::Xml(x) => opt(x.as_ref().map(|x| DbValue::Text(x.to_string()))),
        // Dates and times as the server's text, so nothing shifts zone or rounds
        ColumnData::Date(_) => time(
            chrono::NaiveDate::from_sql(v)
                .ok()
                .flatten()
                .map(|d| d.to_string()),
        ),
        ColumnData::Time(_) => time(
            chrono::NaiveTime::from_sql(v)
                .ok()
                .flatten()
                .map(|t| t.to_string()),
        ),
        ColumnData::DateTimeOffset(_) => time(
            chrono::DateTime::<chrono::FixedOffset>::from_sql(v)
                .ok()
                .flatten()
                .map(|t| t.to_string()),
        ),
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => time(
            chrono::NaiveDateTime::from_sql(v)
                .ok()
                .flatten()
                .map(|t| t.format("%Y-%m-%d %H:%M:%S%.f").to_string()),
        ),
    }
}

fn sql_string(s: &str) -> String {
    crate::dialect::Dialect::literal(&crate::dialect::Mssql, &DbValue::Text(s.to_string()))
}

#[async_trait]
impl DatabaseSession for MssqlSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        let start = Instant::now();
        self.run("SELECT 1").await?;
        Ok(start.elapsed())
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        let schemas: Vec<String> = self
            .rows(
                "SELECT s.name FROM sys.schemas s WHERE s.schema_id < 16384 AND s.name NOT IN ('sys', 'INFORMATION_SCHEMA', 'guest') \
                 OR EXISTS (SELECT 1 FROM sys.objects o WHERE o.schema_id = s.schema_id) AND s.name NOT IN ('sys', 'INFORMATION_SCHEMA') ORDER BY 1",
            )
            .await?
            .into_iter()
            .map(|r| r[0].clone())
            .collect();
        let tables = self
            .rows("SELECT TABLE_SCHEMA, TABLE_NAME, CASE TABLE_TYPE WHEN 'VIEW' THEN 1 ELSE 0 END FROM INFORMATION_SCHEMA.TABLES ORDER BY 1, 2")
            .await?;
        let columns = self
            .rows(
                "SELECT c.TABLE_SCHEMA, c.TABLE_NAME, c.COLUMN_NAME, \
                   c.DATA_TYPE + CASE WHEN c.CHARACTER_MAXIMUM_LENGTH = -1 THEN '(max)' \
                                      WHEN c.CHARACTER_MAXIMUM_LENGTH IS NOT NULL THEN '(' + CAST(c.CHARACTER_MAXIMUM_LENGTH AS varchar) + ')' \
                                      WHEN c.DATA_TYPE IN ('decimal', 'numeric') THEN '(' + CAST(c.NUMERIC_PRECISION AS varchar) + ',' + CAST(c.NUMERIC_SCALE AS varchar) + ')' ELSE '' END, \
                   CASE c.IS_NULLABLE WHEN 'YES' THEN 1 ELSE 0 END, c.COLUMN_DEFAULT, c.ORDINAL_POSITION \
                 FROM INFORMATION_SCHEMA.COLUMNS c ORDER BY 1, 2, c.ORDINAL_POSITION",
            )
            .await?;
        let pks = self
            .rows(
                "SELECT k.TABLE_SCHEMA, k.TABLE_NAME, k.COLUMN_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS t \
                 JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE k ON k.CONSTRAINT_NAME = t.CONSTRAINT_NAME AND k.TABLE_SCHEMA = t.TABLE_SCHEMA \
                 WHERE t.CONSTRAINT_TYPE = 'PRIMARY KEY' ORDER BY k.ORDINAL_POSITION",
            )
            .await?;
        let fks = self
            .rows(
                "SELECT SCHEMA_NAME(p.schema_id), p.name, pc.name, SCHEMA_NAME(r.schema_id), r.name, rc.name \
                 FROM sys.foreign_key_columns f \
                 JOIN sys.tables p ON p.object_id = f.parent_object_id JOIN sys.columns pc ON pc.object_id = p.object_id AND pc.column_id = f.parent_column_id \
                 JOIN sys.tables r ON r.object_id = f.referenced_object_id JOIN sys.columns rc ON rc.object_id = r.object_id AND rc.column_id = f.referenced_column_id",
            )
            .await?;
        let default_schema = self
            .rows("SELECT SCHEMA_NAME()")
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
                        default_value: (c[5] != "NULL").then(|| c[5].clone()),
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

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        if self.read_only && is_mutation(sql) {
            return Err(DbError::ReadOnlyViolation);
        }
        let mut client = self.client.lock().await;
        // Manual-commit mode: the first write opens a transaction the user commits explicitly (E9)
        let verb = tx_verb(sql);
        if self.tx.manual() && !self.tx.open() && verb.is_none() && !is_read(sql) {
            run_batch(&mut client, "BEGIN TRANSACTION").await?;
            self.tx.set_open(true);
        }
        let result = run_batch(&mut client, sql).await;
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
        let mut client = self.client.lock().await;
        // SQL Server's savepoints: SAVE TRANSACTION, rolled back to by name, never released
        let keep_open = self.tx.open() || self.tx.manual();
        if !self.tx.open() {
            run_batch(&mut client, "BEGIN TRANSACTION").await?;
            self.tx.set_open(keep_open);
        }
        if keep_open {
            run_batch(&mut client, "SAVE TRANSACTION quarry_edits").await?;
        }
        for sql in &statements {
            let failure = match run_batch(&mut client, sql).await {
                Ok(r) if r.rows_affected == 1 => continue,
                Ok(_) => DbError::ConcurrentModification,
                Err(e) => e,
            };
            let _ = run_batch(
                &mut client,
                if keep_open {
                    "ROLLBACK TRANSACTION quarry_edits"
                } else {
                    "ROLLBACK TRANSACTION"
                },
            )
            .await;
            return Err(failure);
        }
        if !keep_open {
            run_batch(&mut client, "COMMIT TRANSACTION").await?;
        }
        Ok(statements.len())
    }

    fn tx(&self) -> &TxState {
        &self.tx
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn dialect(&self) -> &'static dyn crate::Dialect {
        &crate::dialect::Mssql
    }

    /// CREATE TABLE rebuilt from the catalog (SQL Server keeps no table DDL), plus its indexes
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        let name = sql_string(
            &self
                .dialect()
                .qualified(&table.schema_name, &table.table_name),
        );
        let indexes = self
            .rows(&format!(
                "SELECT i.name, CAST(i.is_unique AS int), STRING_AGG(c.name, ', ') WITHIN GROUP (ORDER BY ic.key_ordinal) \
                 FROM sys.indexes i JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                 JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
                 WHERE i.object_id = OBJECT_ID({name}) AND i.name IS NOT NULL AND ic.is_included_column = 0 \
                 GROUP BY i.name, i.is_unique ORDER BY 1"
            ))
            .await?;
        Ok(TableInfo {
            ddl: table.create_sql(self.dialect()),
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
                "SELECT o.type, o.name, CASE WHEN o.type = 'TR' THEN 'on ' + OBJECT_NAME(o.parent_object_id) ELSE '' END, CAST(o.object_id AS varchar) \
                 FROM sys.objects o WHERE o.schema_id = SCHEMA_ID({}) AND o.type IN ('FN', 'IF', 'TF', 'P', 'SO', 'TR') AND o.is_ms_shipped = 0 ORDER BY 1, 2",
                sql_string(schema)
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let kind = match r[0].trim() {
                    "FN" | "IF" | "TF" => ObjectKind::Function,
                    "P" => ObjectKind::Procedure,
                    "SO" => ObjectKind::Sequence,
                    "TR" => ObjectKind::Trigger,
                    _ => return None,
                };
                Some(SchemaObject {
                    kind,
                    name: r[1].clone(),
                    detail: r[2].clone(),
                    id: r[3].clone(),
                })
            })
            .collect())
    }

    async fn object_ddl(&self, schema: &str, obj: &SchemaObject) -> Result<String, DbError> {
        let sql = match obj.kind {
            ObjectKind::Sequence => format!(
                "SELECT 'CREATE SEQUENCE ' + QUOTENAME({s}) + '.' + QUOTENAME(name) + ' AS ' + TYPE_NAME(user_type_id) \
                 + ' START WITH ' + CAST(start_value AS varchar) + ' INCREMENT BY ' + CAST(increment AS varchar) + ';' \
                 FROM sys.sequences WHERE object_id = {}",
                obj.id,
                s = sql_string(schema)
            ),
            _ => format!("SELECT OBJECT_DEFINITION({})", obj.id),
        };
        let rows = self.rows(&sql).await?;
        rows.into_iter()
            .next()
            .map(|r| r[0].clone())
            .filter(|d| d != "NULL")
            .ok_or_else(|| DbError::SqlError(format!("{} has no visible definition", obj.name)))
    }

    fn monitor(&self) -> Option<&dyn Monitor> {
        Some(self)
    }
}

#[async_trait]
impl Monitor for MssqlSession {
    fn views(&self) -> &'static [ServerView] {
        &[
            ServerView::Activity,
            ServerView::Statements,
            ServerView::Locks,
        ]
    }

    async fn snapshot(&self, view: ServerView) -> Result<Snapshot, DbError> {
        let sql = match view {
            // Session id first: terminate takes it
            ServerView::Activity => {
                "SELECT s.session_id, s.login_name AS [user], s.host_name AS client, DB_NAME(r.database_id) AS [database], r.status, \
                 r.wait_type AS waiting_on, r.total_elapsed_time AS running_ms, LEFT(t.text, 500) AS query \
                 FROM sys.dm_exec_requests r JOIN sys.dm_exec_sessions s ON s.session_id = r.session_id \
                 CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) t WHERE s.is_user_process = 1 AND r.session_id <> @@SPID ORDER BY r.total_elapsed_time DESC"
            }
            ServerView::Statements => {
                "SELECT TOP 200 qs.execution_count AS calls, qs.total_elapsed_time / 1000 AS total_ms, qs.total_elapsed_time / qs.execution_count / 1000.0 AS mean_ms, \
                 qs.total_rows AS [rows], LEFT(t.text, 500) AS query FROM sys.dm_exec_query_stats qs CROSS APPLY sys.dm_exec_sql_text(qs.sql_handle) t \
                 ORDER BY qs.total_elapsed_time DESC"
            }
            ServerView::Locks => {
                "SELECT l.request_session_id AS session_id, l.resource_type, l.request_mode AS mode, l.request_status AS status, \
                 OBJECT_NAME(p.object_id) AS relation FROM sys.dm_tran_locks l LEFT JOIN sys.partitions p ON p.hobt_id = l.resource_associated_entity_id \
                 WHERE l.request_session_id <> @@SPID ORDER BY l.request_status, l.request_session_id"
            }
            _ => return Ok(Snapshot::Unavailable("Not available for SQL Server".into())),
        };
        match self.run(sql).await {
            Err(e) if e.to_string().contains("VIEW SERVER STATE") => {
                Ok(Snapshot::Unavailable("Monitoring needs the VIEW SERVER STATE permission: GRANT VIEW SERVER STATE TO your_login;".into()))
            }
            other => other.map(Snapshot::Rows),
        }
    }

    /// SQL Server can't stop one query of another session, only end the session
    async fn cancel(&self, _id: i64) -> Result<(), DbError> {
        Err(DbError::Internal(
            "SQL Server can only end the whole session: use Terminate".into(),
        ))
    }

    async fn terminate(&self, id: i64) -> Result<(), DbError> {
        self.run(&format!("KILL {id}")).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn knows_which_statements_must_be_alone() {
        assert!(super::must_stand_alone("CREATE PROCEDURE p AS SELECT 1"));
        assert!(super::must_stand_alone(
            "create or alter view v as select 1"
        ));
        assert!(!super::must_stand_alone("CREATE TABLE t (id int)"));
        assert!(!super::must_stand_alone("UPDATE t SET a = 1"));
    }
}
