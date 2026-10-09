// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

mod dump;
mod logs;
mod monitor;

pub use dump::split_password;

use crate::edits::PendingChanges;
use crate::schema::{ColumnSchema, ColumnType, ForeignKeyRef, SchemaCatalog, TableInfo, TableSchema};
use crate::session::{is_mutation, is_read, tx_verb, DatabaseSession, DbError, QueryResult, TxState, TxVerb};
use crate::value::DbValue;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_postgres::types::Type;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

pub struct PostgresSession {
    client: Arc<Client>,
    /// What we connected with (through the SSH tunnel, if any), for client tools like pg_dump
    url: String,
    read_only: bool,
    database_name: String,
    tx: TxState,
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
            let _ = client.batch_execute("SET default_transaction_read_only = TRUE;").await;
        }

        Ok(Self {
            client: Arc::new(client),
            url: connection_str.to_string(),
            read_only,
            database_name,
            tx: TxState::default(),
        })
    }

    pub fn database_name(&self) -> &str {
        &self.database_name
    }
}

/// Keeps Postgres' error position (1-based character in the statement) so the editor can point at it
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

/// Reads back the position `statement_err` appended
pub fn error_position(message: &str) -> Option<usize> {
    let (_, tail) = message.rsplit_once("(at character ")?;
    tail.strip_suffix(')')?.parse().ok()
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
        let schemas_rows = self.client.query(
            "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') ORDER BY schema_name",
            &[],
        ).await.map_err(|e| DbError::SqlError(e.to_string()))?;

        let schemas: Vec<String> = schemas_rows.iter().map(|r| r.get(0)).collect();

        let tables_query = r#"
            SELECT 
                table_schema, 
                table_name, 
                table_type = 'VIEW' as is_view
            FROM information_schema.tables 
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
            ORDER BY table_schema, table_name;
        "#;

        let table_rows = self.client.query(tables_query, &[]).await.map_err(|e| DbError::SqlError(e.to_string()))?;

        let columns_query = r#"
            SELECT 
                table_schema,
                table_name,
                column_name,
                data_type,
                is_nullable = 'YES' as nullable,
                column_default,
                ordinal_position
            FROM information_schema.columns
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
            ORDER BY table_schema, table_name, ordinal_position;
        "#;

        let col_rows = self.client.query(columns_query, &[]).await.map_err(|e| DbError::SqlError(e.to_string()))?;

        // Primary keys query
        let pks_query = r#"
            SELECT 
                kcu.table_schema,
                kcu.table_name,
                kcu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu 
              ON tc.constraint_name = kcu.constraint_name 
             AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY';
        "#;

        let pk_rows = self.client.query(pks_query, &[]).await.map_err(|e| DbError::SqlError(e.to_string()))?;

        // Foreign keys, one row per referencing column (G5)
        let fks_query = r#"
            SELECT kcu.table_schema, kcu.table_name, kcu.column_name,
                   ccu.table_schema, ccu.table_name, ccu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage ccu
              ON ccu.constraint_name = tc.constraint_name AND ccu.constraint_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY';
        "#;
        let fk_rows = self.client.query(fks_query, &[]).await.map_err(|e| DbError::SqlError(e.to_string()))?;

        let mut tables = Vec::new();

        for trow in table_rows {
            let schema_name: String = trow.get(0);
            let table_name: String = trow.get(1);
            let is_view: bool = trow.get(2);

            let table_pks: Vec<String> = pk_rows
                .iter()
                .filter(|r| r.get::<_, String>(0) == schema_name && r.get::<_, String>(1) == table_name)
                .map(|r| r.get::<_, String>(2))
                .collect();

            let columns: Vec<ColumnSchema> = col_rows
                .iter()
                .filter(|r| r.get::<_, String>(0) == schema_name && r.get::<_, String>(1) == table_name)
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
                .filter(|r| r.get::<_, String>(0) == schema_name && r.get::<_, String>(1) == table_name)
                .map(|r| ForeignKeyRef { from_column: r.get(2), to_schema: r.get(3), to_table: r.get(4), to_column: r.get(5) })
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

        let default_schema = self.client.query_one("SELECT current_schema()", &[]).await.ok().and_then(|r| r.get(0));
        Ok(SchemaCatalog { schemas, tables, default_schema })
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult, DbError> {
        if self.read_only && is_mutation(sql) {
            return Err(DbError::ReadOnlyViolation);
        }
        // Manual-commit mode: the first statement opens a transaction the user commits explicitly (E9)
        let verb = tx_verb(sql);
        if self.tx.manual() && !self.tx.open() && verb.is_none() && !is_read(sql) {
            self.client.batch_execute("BEGIN").await.map_err(|e| DbError::SqlError(e.to_string()))?;
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

    async fn apply_edits(&self, table_name: &str, changes: &PendingChanges) -> Result<usize, DbError> {
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
            let _ = self.batch(if keep_open { "ROLLBACK TO SAVEPOINT quarry_edits" } else { "ROLLBACK" }).await;
            return Err(failure);
        }
        self.batch(if keep_open { "RELEASE SAVEPOINT quarry_edits" } else { "COMMIT" }).await?;
        Ok(statements.len())
    }

    fn monitor(&self) -> Option<&dyn crate::monitor::Monitor> {
        Some(self)
    }

    fn dumper(&self) -> Option<&dyn crate::dump::Dumper> {
        Some(self)
    }

    /// Postgres doesn't keep DDL: CREATE TABLE comes from the catalog, the rest from pg_catalog
    async fn table_info(&self, table: &TableSchema) -> Result<TableInfo, DbError> {
        let rel = format!("{}::regclass", sql_string(&self.dialect().qualified(&table.schema_name, &table.table_name)));
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
        let mut parts = vec![format!("-- Rebuilt from the catalog\n{}", table.create_sql(self.dialect()))];
        parts.extend(extra.rows.iter().map(|r| format!("{};", r[0].to_display_string().trim_end_matches(';'))));
        Ok(TableInfo { ddl: parts.join("\n\n"), indexes: crate::schema::index_rows(&indexes) })
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
        self.client.batch_execute(sql).await.map_err(|e| DbError::SqlError(e.to_string()))
    }

    async fn run_statement(&self, sql: &str) -> Result<QueryResult, DbError> {
        let start = Instant::now();

        // Types come from a side-effect-free prepare; values come back over the text protocol,
        // which every Postgres type supports, so nothing can fail to decode or lose precision.
        let types: Option<Vec<Type>> = self
            .client
            .prepare(sql)
            .await
            .ok()
            .map(|stmt| stmt.columns().iter().map(|c| c.type_().clone()).collect());

        let messages = self.client.simple_query(sql).await.map_err(statement_err)?;

        let mut columns = Vec::new();
        let mut rows = Vec::new();
        let mut affected = 0;
        for msg in messages {
            match msg {
                SimpleQueryMessage::RowDescription(cols) => {
                    columns = cols.iter().map(|c| c.name().to_string()).collect();
                }
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

/// Converts a text-protocol value into a typed DbValue. Unknown types stay as their exact text.
fn decode_text(text: Option<&str>, ty: Option<&Type>) -> DbValue {
    let Some(t) = text else { return DbValue::Null };
    let text = || DbValue::Text(t.to_string());
    let Some(ty) = ty else { return text() };
    match *ty {
        Type::BOOL => DbValue::Boolean(t == "t"),
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => t.parse().map(DbValue::Int).unwrap_or_else(|_| text()),
        Type::FLOAT4 | Type::FLOAT8 => t.parse().map(DbValue::Float).unwrap_or_else(|_| text()),
        Type::NUMERIC => DbValue::Decimal(t.to_string()),
        Type::JSON | Type::JSONB => serde_json::from_str(t).map(DbValue::Json).unwrap_or_else(|_| text()),
        Type::BYTEA => t
            .strip_prefix("\\x")
            .and_then(|hex| (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect())
            .map(DbValue::Blob)
            .unwrap_or_else(text),
        _ => text(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_protocol_values() {
        assert_eq!(decode_text(None, Some(&Type::INT4)), DbValue::Null);
        assert_eq!(decode_text(Some("t"), Some(&Type::BOOL)), DbValue::Boolean(true));
        assert_eq!(decode_text(Some("9223372036854775807"), Some(&Type::INT8)), DbValue::Int(i64::MAX));
        assert_eq!(
            decode_text(Some("12345678901234567890.123456789"), Some(&Type::NUMERIC)),
            DbValue::Decimal("12345678901234567890.123456789".into())
        );
        assert_eq!(decode_text(Some("\\x00ff"), Some(&Type::BYTEA)), DbValue::Blob(vec![0, 255]));
        assert_eq!(decode_text(Some("{1,2}"), Some(&Type::INT4_ARRAY)), DbValue::Text("{1,2}".into()));
        assert_eq!(decode_text(Some(""), Some(&Type::TEXT)), DbValue::Text(String::new()));
    }

    #[test]
    fn reads_back_error_position() {
        assert_eq!(super::error_position("db error: ERROR: syntax error at or near \"form\" (at character 10)"), Some(10));
        assert_eq!(super::error_position("db error: ERROR: relation \"x\" does not exist"), None);
    }
}
