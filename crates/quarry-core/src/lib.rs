// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

pub mod conformance;
pub mod conn_url;
pub mod dialect;
pub mod diff;
pub mod driver;
pub mod dump;
pub mod edits;
pub mod export;
pub mod import;
pub mod lexer;
pub mod logged;
pub mod monitor;
#[cfg(feature = "mssql")]
pub mod mssql;
#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "oracle")]
pub mod oracle;
pub mod plan;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod row_buffer;
pub mod schema;
pub mod session;
#[cfg(feature = "sqlite")]
pub mod sqlite;
pub mod value;

pub use dialect::Dialect;
pub use driver::{detect, driver, drivers, Driver, Field};
pub use edits::{PendingCellEdit, PendingChanges, PendingRowDelete, PendingRowInsert, RowKey};
pub use lexer::{parse_dollar_tag, SqlLexer, Token, TokenKind};
pub use logged::{ActivityLog, LogEntry, LoggedSession};
#[cfg(feature = "postgres")]
pub use postgres::PostgresSession;
pub use row_buffer::{RowBuffer, RowIter, RowRef};
pub use schema::{ColumnSchema, ColumnType, SchemaCatalog, TableSchema};
pub use session::{DatabaseSession, DbError, QueryResult, TxState};
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteSession;
pub use value::DbValue;

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_in_memory_flow() {
        let session = SqliteSession::open_in_memory().unwrap();

        // 1. Create table
        let ddl = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL, score REAL, active BOOLEAN);";
        let res = session.execute(ddl).await.unwrap();
        assert_eq!(res.rows_affected, 0);

        // 2. Insert data
        let insert = "INSERT INTO users (id, email, score, active) VALUES (1, 'alice@example.com', 99.5, 1);";
        session.execute(insert).await.unwrap();

        // 3. Introspect schema
        let catalog = session.introspect().await.unwrap();
        let table = catalog
            .find_table(None, "users")
            .expect("users table should exist");
        assert_eq!(table.primary_keys, vec!["id"]);
        assert_eq!(table.columns.len(), 4);

        // 4. Query
        let query_res = session
            .execute("SELECT id, email, score, active FROM users")
            .await
            .unwrap();
        assert_eq!(query_res.rows.len(), 1);
        assert_eq!(query_res.rows[(0, 0)], DbValue::Int(1));
        assert_eq!(
            query_res.rows[(0, 1)],
            DbValue::Text("alice@example.com".to_string())
        );

        // 5. Test pending edit and commit
        let mut changes = PendingChanges::new();
        let key = vec![("id".to_string(), DbValue::Int(1))];
        let old = DbValue::Text("alice@example.com".to_string());
        changes.set_cell(
            key.clone(),
            "email",
            old.clone(),
            DbValue::Text("alice.updated@example.com".to_string()),
        );

        let table = edits::qualified_name("main", "users");
        assert_eq!(session.apply_edits(&table, &changes).await.unwrap(), 1);

        // Same edit again: the row no longer holds the old value, so it's a conflict and nothing changes
        let mut stale = PendingChanges::new();
        stale.set_cell(
            key,
            "email",
            old,
            DbValue::Text("someone.else@example.com".to_string()),
        );
        assert!(matches!(
            session.apply_edits(&table, &stale).await,
            Err(DbError::ConcurrentModification)
        ));

        // 6. Table info: stored DDL plus indexes
        session
            .execute("CREATE UNIQUE INDEX users_email ON users (email)")
            .await
            .unwrap();
        let table = session
            .introspect()
            .await
            .unwrap()
            .find_table(None, "users")
            .cloned()
            .unwrap();
        let info = session.table_info(&table).await.unwrap();
        assert!(
            info.ddl.starts_with("CREATE TABLE users")
                && info.ddl.contains("CREATE UNIQUE INDEX users_email")
        );
        assert_eq!(
            info.indexes,
            vec![schema::IndexInfo {
                name: "users_email".into(),
                unique: true,
                columns: "email".into()
            }]
        );

        // 7. Triggers are listed and their SQL comes back
        session
            .execute("CREATE TRIGGER users_touch AFTER UPDATE ON users BEGIN SELECT 1; END")
            .await
            .unwrap();
        let objects = session.objects("main").await.unwrap();
        assert_eq!(
            (objects[0].name.as_str(), objects[0].detail.as_str()),
            ("users_touch", "on users")
        );
        assert!(session
            .object_ddl("main", &objects[0])
            .await
            .unwrap()
            .starts_with("CREATE TRIGGER users_touch"));

        // 8. Big results keep MAX_ROWS rows and still report the full count
        let big = format!("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {}) SELECT i FROM n", session::MAX_ROWS + 5);
        let r = session.execute(&big).await.unwrap();
        assert_eq!(
            (r.rows.len(), r.rows_affected, r.truncated()),
            (session::MAX_ROWS, session::MAX_ROWS as u64 + 5, true)
        );

        // 9. Verify updated row
        let verify = session
            .execute("SELECT email FROM users WHERE id = 1")
            .await
            .unwrap();
        assert_eq!(
            verify.rows[(0, 0)],
            DbValue::Text("alice.updated@example.com".to_string())
        );
    }
}
