// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

mod dump;
mod logs;
mod monitor;
mod objects;
mod url;

pub use url::split_password;

use crate::driver::{Driver, Field};
use crate::edits::PendingChanges;
use crate::schema::{
    ColumnSchema, ColumnType, ForeignKeyRef, SchemaCatalog, TableInfo, TableSchema, EAGER_SCHEMAS,
};
use crate::session::{
    is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState, TxVerb,
};
use crate::value::DbValue;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_postgres::types::Type;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

pub struct PostgresDriver;
pub static DRIVER: PostgresDriver = PostgresDriver;

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
impl Driver for PostgresDriver {
    fn id(&self) -> &'static str {
        "postgres"
    }

    fn name(&self) -> &'static str {
        "PostgreSQL"
    }

    fn sample(&self) -> &'static str {
        "postgres://postgres:postgres@localhost:5432/postgres"
    }

    fn recognizes(&self, conn: &str) -> bool {
        conn.starts_with("postgres://")
            || conn.starts_with("postgresql://")
            || conn.contains("host=")
    }

    fn fields(&self) -> &'static [Field] {
        const FIELDS: [Field; 5] = [
            field("Host", "localhost", false, 0, None),
            field("Port", "5432", false, 0, Some(120.0)),
            field("User", "postgres", false, 1, None),
            field("Password", "", true, 1, None),
            field("Database", "postgres", false, 2, None),
        ];
        &FIELDS
    }

    fn split(&self, conn: &str) -> Vec<String> {
        crate::conn_url::parse(conn, &url::SCHEMES)
            .map_or_else(|| url::parse_kv(conn), |(f, _)| f)
            .to_vec()
    }

    fn join(&self, values: &[String], previous: &str) -> String {
        let query = crate::conn_url::parse(previous, &url::SCHEMES)
            .map(|(_, q)| q)
            .unwrap_or_default();
        let fields: crate::conn_url::UrlFields =
            std::array::from_fn(|i| values.get(i).cloned().unwrap_or_default());
        crate::conn_url::build("postgres", &fields, &query)
    }

    fn take_secret(&self, conn: &str) -> (String, Option<String>) {
        url::split_password(conn)
    }

    fn put_secret(&self, conn: &str, secret: &str) -> String {
        url::insert_password(conn, secret)
    }

    /// `host:port/db`, without user or password
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
        Some(url::target(conn))
    }

    fn through_tunnel(&self, conn: &str, local_port: u16) -> String {
        url::retarget(conn, local_port)
    }

    async fn connect(
        &self,
        conn: &str,
        read_only: bool,
    ) -> Result<Arc<dyn DatabaseSession>, DbError> {
        Ok(Arc::new(PostgresSession::connect(conn, read_only).await?))
    }
}

pub struct PostgresSession {
    client: Arc<Client>,
    /// What we connected with (through the SSH tunnel, if any), for client tools like pg_dump
    url: String,
    read_only: bool,
    database_name: String,
    tx: TxState,
    type_cache: std::sync::Mutex<std::collections::HashMap<String, Vec<Type>>>,
}

impl PostgresSession {
    pub async fn connect(connection_str: &str, read_only: bool) -> Result<Self, DbError> {
        let (client, connection) = tokio_postgres::connect(connection_str, NoTls)
            .await
            .map_err(|e| DbError::ConnectionFailed(e.to_string()))?;

        // Spawn connection poller on background tokio task
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("PostgreSQL connection error: {}", e);
            }
        });

        // Extract database name if available
        let database_name = client
            .query_one("SELECT current_database()", &[])
            .await
            .map(|row| row.get::<_, String>(0))
            .unwrap_or_else(|_| "postgres".to_string());

        if read_only {
            let _ = client
                .batch_execute("SET default_transaction_read_only = TRUE;")
                .await;
        }

        Ok(Self {
            client: Arc::new(client),
            url: connection_str.to_string(),
            read_only,
            database_name,
            tx: TxState::default(),
            type_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    pub fn database_name(&self) -> &str {
        &self.database_name
    }
}

/// Keeps Postgres' error position (1-based character in the statement) so the editor can point at it; see `session::error_position`
fn statement_err(e: tokio_postgres::Error) -> DbError {
    use tokio_postgres::error::ErrorPosition;
    match e.as_db_error().and_then(|d| d.position()) {
        Some(ErrorPosition::Original(at)) => DbError::SqlError(format!("{e} (at character {at})")),
        _ => DbError::SqlError(e.to_string()),
    }
}

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[async_trait]
impl DatabaseSession for PostgresSession {
    async fn ping(&self) -> Result<Duration, DbError> {
        let start = Instant::now();
        self.client
            .execute("SELECT 1", &[])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;
        Ok(start.elapsed())
    }

    async fn introspect(&self) -> Result<SchemaCatalog, DbError> {
        self.catalog_where(None).await
    }

    /// Many schemas: only the one unqualified names resolve to, the rest when opened
    async fn load_catalog(&self) -> Result<SchemaCatalog, DbError> {
        let c = self.catalog_where(Some("current_schema()")).await?;
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
        // Manual-commit mode: the first statement opens a transaction the user commits explicitly (E9)
        let verb = tx_verb(sql);
        if self.tx.manual() && !self.tx.open() && verb.is_none() && !is_read(sql) {
            self.client
                .batch_execute("BEGIN")
                .await
                .map_err(|e| DbError::SqlError(e.to_string()))?;
            self.tx.set_open(true);
        }
        let result = self.run_statement(sql).await;
        match verb {
            Some(TxVerb::Begin) if result.is_ok() => self.tx.set_open(true),
            // COMMIT/ROLLBACK end the transaction even when they report an error
            Some(TxVerb::End) => self.tx.set_open(false),
            _ => {}
        }
        result
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

        // Inside a transaction (open, or about to be in manual mode) a savepoint keeps a failed
        // submit from undoing the user's other work; otherwise the batch is its own transaction.
        let keep_open = self.tx.open() || self.tx.manual();
        if !self.tx.open() {
            self.batch("BEGIN").await?;
            self.tx.set_open(keep_open);
        }
        if keep_open {
            self.batch("SAVEPOINT quarry_edits").await?;
        }
        for sql in &statements {
            let failure = match self.client.execute(sql.as_str(), &[]).await {
                Ok(1) => continue,
                Ok(_) => DbError::ConcurrentModification,
                Err(e) => DbError::SqlError(e.to_string()),
            };
            let _ = self
                .batch(if keep_open {
                    "ROLLBACK TO SAVEPOINT quarry_edits"
                } else {
                    "ROLLBACK"
                })
                .await;
            return Err(failure);
        }
        self.batch(if keep_open {
            "RELEASE SAVEPOINT quarry_edits"
        } else {
            "COMMIT"
        })
        .await?;
        Ok(statements.len())
    }

    fn monitor(&self) -> Option<&dyn crate::monitor::Monitor> {
        Some(self)
    }

    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        Some(self)
    }

    async fn objects(&self, schema: &str) -> Result<Vec<crate::schema::SchemaObject>, DbError> {
        self.list_objects(schema).await
    }

    async fn object_ddl(
        &self,
        _schema: &str,
        obj: &crate::schema::SchemaObject,
    ) -> Result<String, DbError> {
        self.definition(obj).await
    }

    /// Postgres doesn't keep DDL: CREATE TABLE comes from the catalog, the rest from pg_catalog
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        let rel = format!(
            "{}::regclass",
            sql_string(
                &self
                    .dialect()
                    .qualified(&table.schema_name, &table.table_name)
            )
        );
        let extra = self.run_statement(&format!(
            "SELECT format('ALTER TABLE %s ADD CONSTRAINT %I %s', conrelid::regclass, conname, pg_get_constraintdef(oid)) \
             FROM pg_constraint WHERE conrelid = {rel} AND contype IN ('c', 'u', 'x') \
             UNION ALL SELECT pg_get_indexdef(indexrelid) FROM pg_index \
             WHERE indrelid = {rel} AND indexrelid NOT IN (SELECT conindid FROM pg_constraint WHERE conrelid = {rel}) \
             UNION ALL SELECT format('COMMENT ON TABLE %s IS %L', {rel}, obj_description({rel}, 'pg_class')) \
             WHERE obj_description({rel}, 'pg_class') IS NOT NULL \
             UNION ALL SELECT format('COMMENT ON COLUMN %s.%I IS %L', {rel}, attname, col_description(attrelid, attnum)) \
             FROM pg_attribute WHERE attrelid = {rel} AND attnum > 0 AND NOT attisdropped AND col_description(attrelid, attnum) IS NOT NULL"
        )).await?;
        let indexes = self.run_statement(&format!(
            "SELECT i.relname, ix.indisunique, array_to_string(ARRAY(SELECT pg_get_indexdef(ix.indexrelid, k + 1, true) \
             FROM generate_subscripts(ix.indkey, 1) AS k ORDER BY k), ', ') \
             FROM pg_index ix JOIN pg_class i ON i.oid = ix.indexrelid WHERE ix.indrelid = {rel} ORDER BY i.relname"
        )).await?;
        let mut parts = vec![format!(
            "-- Rebuilt from the catalog\n{}",
            table.create_sql(self.dialect())
        )];
        parts.extend(
            extra
                .rows
                .iter()
                .map(|r| format!("{};", r[0].to_display_string().trim_end_matches(';'))),
        );
        // Extras are best-effort: a missing privilege shouldn't hide the DDL
        let comments = self.run_statement(&format!(
            "SELECT attname, col_description(attrelid, attnum) FROM pg_attribute \
             WHERE attrelid = {rel} AND attnum > 0 AND NOT attisdropped AND col_description(attrelid, attnum) IS NOT NULL \
             UNION ALL SELECT '', obj_description({rel}, 'pg_class') WHERE obj_description({rel}, 'pg_class') IS NOT NULL"
        )).await;
        let triggers = self.run_statement(&format!(
            "SELECT tgname, pg_get_triggerdef(oid) FROM pg_trigger WHERE tgrelid = {rel} AND NOT tgisinternal ORDER BY 1"
        )).await;
        let stats = self.run_statement(&format!(
            "SELECT CASE WHEN c.reltuples < 0 THEN 'not analyzed' ELSE c.reltuples::bigint::text END AS \"Rows (estimate)\", \
             pg_size_pretty(pg_total_relation_size(c.oid)) AS \"Total size\", \
             pg_size_pretty(pg_relation_size(c.oid)) AS \"Table size\", \
             pg_size_pretty(pg_indexes_size(c.oid)) AS \"Index size\", \
             coalesce(greatest(s.last_vacuum, s.last_autovacuum)::text, 'never') AS \"Last vacuum\", \
             coalesce(greatest(s.last_analyze, s.last_autoanalyze)::text, 'never') AS \"Last analyze\" \
             FROM pg_class c LEFT JOIN pg_stat_all_tables s ON s.relid = c.oid WHERE c.oid = {rel}"
        )).await;
        Ok(TableInfo {
            ddl: parts.join("\n\n"),
            indexes: crate::schema::index_rows(&indexes),
            comments: comments
                .map(|r| crate::schema::pair_rows(&r))
                .unwrap_or_default(),
            triggers: triggers
                .map(|r| crate::schema::pair_rows(&r))
                .unwrap_or_default(),
            stats: stats
                .map(|r| crate::schema::labelled_row(&r))
                .unwrap_or_default(),
        })
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn tx(&self) -> &TxState {
        &self.tx
    }

    fn dialect(&self) -> &'static dyn crate::Dialect {
        &crate::dialect::Postgres
    }
}

impl PostgresSession {
    async fn batch(&self, sql: &str) -> Result<(), DbError> {
        self.client
            .batch_execute(sql)
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))
    }

    async fn run_statement(&self, sql: &str) -> Result<QueryResult, DbError> {
        let start = Instant::now();

        // Determine whether this statement could return rows. Mutations without RETURNING
        // never return row descriptions, so we bypass prepare() entirely to avoid an extra RTT.
        let is_mutation_stmt = is_mutation(sql);
        let might_return_rows = !is_mutation_stmt || has_returning(sql);

        // Types come from a side-effect-free prepare (cached when possible); values come back
        // over the text protocol, which every Postgres type supports without failing or losing precision.
        let types: Option<Vec<Type>> = if might_return_rows {
            let cached = self
                .type_cache
                .lock()
                .ok()
                .and_then(|c| c.get(sql).cloned());
            if cached.is_some() {
                cached
            } else {
                let fetched: Option<Vec<Type>> = self
                    .client
                    .prepare(sql)
                    .await
                    .ok()
                    .map(|stmt| stmt.columns().iter().map(|c| c.type_().clone()).collect());
                if let Some(ref t) = fetched {
                    if let Ok(mut cache) = self.type_cache.lock() {
                        if cache.len() >= 256 {
                            cache.clear();
                        }
                        cache.insert(sql.to_string(), t.clone());
                    }
                }
                fetched
            }
        } else {
            None
        };

        // Streamed so rows past MAX_ROWS are counted, not kept. The stream is read to the end
        // either way, so the connection stays in step for the next statement.
        let messages = self
            .client
            .simple_query_raw(sql)
            .await
            .map_err(statement_err)?;
        futures_util::pin_mut!(messages);

        let mut columns = Vec::new();
        let mut rows = Vec::new();
        let mut affected = 0;
        while let Some(msg) = futures_util::TryStreamExt::try_next(&mut messages)
            .await
            .map_err(statement_err)?
        {
            match msg {
                SimpleQueryMessage::RowDescription(cols) => {
                    columns = cols.iter().map(|c| c.name().to_string()).collect();
                }
                SimpleQueryMessage::Row(_) if rows.len() >= crate::session::MAX_ROWS => {}
                SimpleQueryMessage::Row(row) => {
                    rows.push(
                        (0..row.len())
                            .map(|i| decode_text(row.get(i), types.as_ref().and_then(|t| t.get(i))))
                            .collect(),
                    );
                }
                SimpleQueryMessage::CommandComplete(n) => affected = n,
                _ => {}
            }
        }

        Ok(QueryResult {
            columns,
            rows,
            rows_affected: affected,
            execution_time: start.elapsed(),
            statement: sql.to_string(),
        })
    }
}

/// Checks whether a mutation statement contains a RETURNING clause
fn has_returning(sql: &str) -> bool {
    crate::lexer::SqlLexer::new(sql).any(|tok| tok.is_keyword("RETURNING"))
}

/// Converts a text-protocol value into a typed DbValue. Unknown types stay as their exact text.
fn decode_text(text: Option<&str>, ty: Option<&Type>) -> DbValue {
    let Some(t) = text else { return DbValue::Null };
    let text = || DbValue::Text(t.to_string());
    let Some(ty) = ty else {
        // Fallback heuristic if type information is unavailable (e.g. non-preparable statements)
        let looks_json =
            (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']'));
        if looks_json {
            if let Ok(j) = serde_json::from_str(t) {
                return DbValue::Json(j);
            }
        }
        return text();
    };
    match *ty {
        Type::BOOL => DbValue::Boolean(t == "t"),
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => {
            t.parse().map(DbValue::Int).unwrap_or_else(|_| text())
        }
        Type::FLOAT4 | Type::FLOAT8 => t.parse().map(DbValue::Float).unwrap_or_else(|_| text()),
        Type::NUMERIC => DbValue::Decimal(t.to_string()),
        Type::JSON | Type::JSONB => serde_json::from_str(t)
            .map(DbValue::Json)
            .unwrap_or_else(|_| text()),
        Type::BYTEA => t
            .strip_prefix("\\x")
            .and_then(|hex| {
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                    .collect()
            })
            .map(DbValue::Blob)
            .unwrap_or_else(text),
        _ => text(),
    }
}

impl PostgresSession {
    /// Everything (`only` = None) or one schema: `only` is a SQL expression for its name
    async fn catalog_where(&self, only: Option<&str>) -> Result<SchemaCatalog, DbError> {
        let f = |col: &str| {
            only.map(|e| format!(" AND {col} = {e}"))
                .unwrap_or_default()
        };
        let schemas_rows = self.client.query(
            "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') ORDER BY schema_name",
            &[],
        ).await.map_err(|e| DbError::SqlError(e.to_string()))?;

        let schemas: Vec<String> = schemas_rows.iter().map(|r| r.get(0)).collect();

        let tables_query = format!(
            r#"
            SELECT 
                table_schema, 
                table_name, 
                table_type = 'VIEW' as is_view
            FROM information_schema.tables 
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema'){}
            ORDER BY table_schema, table_name;
        "#,
            f("table_schema")
        );

        let table_rows = self
            .client
            .query(&tables_query, &[])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;

        let columns_query = format!(
            r#"
            SELECT 
                table_schema,
                table_name,
                column_name,
                data_type,
                is_nullable = 'YES' as nullable,
                column_default,
                ordinal_position
            FROM information_schema.columns
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema'){}
            ORDER BY table_schema, table_name, ordinal_position;
        "#,
            f("table_schema")
        );

        let col_rows = self
            .client
            .query(&columns_query, &[])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;

        // Primary keys query
        let pks_query = format!(
            r#"
            SELECT 
                kcu.table_schema,
                kcu.table_name,
                kcu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu 
              ON tc.constraint_name = kcu.constraint_name 
             AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY'{};
        "#,
            f("kcu.table_schema")
        );

        let pk_rows = self
            .client
            .query(&pks_query, &[])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;

        // Foreign keys, one row per referencing column (G5)
        let fks_query = format!(
            r#"
            SELECT kcu.table_schema, kcu.table_name, kcu.column_name,
                   ccu.table_schema, ccu.table_name, ccu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage ccu
              ON ccu.constraint_name = tc.constraint_name AND ccu.constraint_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY'{};
        "#,
            f("kcu.table_schema")
        );
        let fk_rows = self
            .client
            .query(&fks_query, &[])
            .await
            .map_err(|e| DbError::SqlError(e.to_string()))?;

        let mut tables = Vec::new();

        for trow in table_rows {
            let schema_name: String = trow.get(0);
            let table_name: String = trow.get(1);
            let is_view: bool = trow.get(2);

            let table_pks: Vec<String> = pk_rows
                .iter()
                .filter(|r| {
                    r.get::<_, String>(0) == schema_name && r.get::<_, String>(1) == table_name
                })
                .map(|r| r.get::<_, String>(2))
                .collect();

            let columns: Vec<ColumnSchema> = col_rows
                .iter()
                .filter(|r| {
                    r.get::<_, String>(0) == schema_name && r.get::<_, String>(1) == table_name
                })
                .map(|r| {
                    let col_name: String = r.get(2);
                    let raw_type: String = r.get(3);
                    let is_nullable: bool = r.get(4);
                    let default_val: Option<String> = r.get(5);
                    let ord: i32 = r.get(6);
                    let is_pk = table_pks.contains(&col_name);

                    ColumnSchema {
                        name: col_name,
                        data_type: ColumnType::from_sql_str(&raw_type),
                        raw_type_name: raw_type,
                        is_nullable,
                        is_primary_key: is_pk,
                        default_value: default_val,
                        ordinal_position: ord as usize,
                    }
                })
                .collect();

            let foreign_keys = fk_rows
                .iter()
                .filter(|r| {
                    r.get::<_, String>(0) == schema_name && r.get::<_, String>(1) == table_name
                })
                .map(|r| ForeignKeyRef {
                    from_column: r.get(2),
                    to_schema: r.get(3),
                    to_table: r.get(4),
                    to_column: r.get(5),
                })
                .collect();
            tables.push(TableSchema {
                schema_name,
                table_name,
                is_view,
                columns,
                primary_keys: table_pks,
                foreign_keys,
            });
        }

        let default_schema = self
            .client
            .query_one("SELECT current_schema()", &[])
            .await
            .ok()
            .and_then(|r| r.get(0));
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
    use super::*;

    #[test]
    fn decodes_text_protocol_values() {
        assert_eq!(decode_text(None, Some(&Type::INT4)), DbValue::Null);
        assert_eq!(
            decode_text(Some("t"), Some(&Type::BOOL)),
            DbValue::Boolean(true)
        );
        assert_eq!(
            decode_text(Some("9223372036854775807"), Some(&Type::INT8)),
            DbValue::Int(i64::MAX)
        );
        assert_eq!(
            decode_text(Some("12345678901234567890.123456789"), Some(&Type::NUMERIC)),
            DbValue::Decimal("12345678901234567890.123456789".into())
        );
        assert_eq!(
            decode_text(Some("\\x00ff"), Some(&Type::BYTEA)),
            DbValue::Blob(vec![0, 255])
        );
        assert_eq!(
            decode_text(Some("{1,2}"), Some(&Type::INT4_ARRAY)),
            DbValue::Text("{1,2}".into())
        );
        assert_eq!(
            decode_text(Some(""), Some(&Type::TEXT)),
            DbValue::Text(String::new())
        );
    }

    #[test]
    fn reads_back_error_position() {
        use crate::session::error_position;
        assert_eq!(
            error_position("db error: ERROR: syntax error at or near \"form\" (at character 10)"),
            Some(10)
        );
        assert_eq!(
            error_position("db error: ERROR: relation \"x\" does not exist"),
            None
        );
    }

    #[test]
    fn detects_has_returning_safely() {
        assert!(has_returning(
            "INSERT INTO users (name) VALUES ('a') RETURNING id"
        ));
        assert!(has_returning("UPDATE users SET name = 'b' returning *;"));
        assert!(has_returning(
            "DELETE FROM users WHERE id = 1 RETURNING id, name"
        ));
        // Does not trigger for strings or comments
        assert!(!has_returning(
            "INSERT INTO users (name) VALUES ('RETURNING')"
        ));
        assert!(!has_returning(
            "/* RETURNING */ INSERT INTO users (name) VALUES ('a')"
        ));
        assert!(!has_returning(
            "-- RETURNING\nINSERT INTO users (name) VALUES ('a')"
        ));
        assert!(!has_returning("INSERT INTO users (name) VALUES ('a')"));
    }

    #[test]
    fn decodes_without_type_info_with_fallback() {
        assert_eq!(decode_text(None, None), DbValue::Null);
        assert_eq!(
            decode_text(Some("hello"), None),
            DbValue::Text("hello".into())
        );
        assert_eq!(
            decode_text(Some("{\"a\": 1}"), None),
            DbValue::Json(serde_json::json!({"a": 1}))
        );
        assert_eq!(
            decode_text(Some("[1, 2]"), None),
            DbValue::Json(serde_json::json!([1, 2]))
        );
        assert_eq!(
            decode_text(Some("{not valid json}"), None),
            DbValue::Text("{not valid json}".into())
        );
    }
}
