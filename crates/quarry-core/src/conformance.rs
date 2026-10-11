// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! What every driver must do, as one check. A new driver's tests call `check` against a live
//! session; it panics with the step that failed.

use crate::edits::{PendingChanges, PendingRowInsert};
use crate::schema::ColumnType;
use crate::session::{DatabaseSession, DbError};
use crate::value::DbValue;

/// Creates `table` in `schema`, exercises it, and drops it again
pub async fn check(s: &dyn DatabaseSession, schema: &str) {
    let d = s.dialect();
    let t = d.qualified(schema, "quarry_conformance");
    let ty = |c: ColumnType| d.type_name(&c).unwrap_or("TEXT");
    let _ = s.execute(&format!("DROP TABLE {t}")).await;
    s.execute(&format!(
        "CREATE TABLE {t} ({} {} PRIMARY KEY, {} {}, {} {})",
        d.quote_ident("id"),
        ty(ColumnType::BigInt),
        d.quote_ident("name"),
        ty(ColumnType::Text),
        d.quote_ident("score"),
        ty(ColumnType::Float),
    ))
    .await
    .expect("create table");

    // Introspection finds the table, its columns and its primary key
    let catalog = s.introspect().await.expect("introspect");
    let table = catalog
        .find_table(Some(schema), "quarry_conformance")
        .expect("introspect: table not found");
    assert_eq!(table.primary_keys, vec!["id"], "introspect: primary key");
    assert_eq!(
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "name", "score"],
        "introspect: columns"
    );

    // One schema on its own, and the catalog a connection starts with, both find it too
    let one = s
        .introspect_schema(&table.schema_name)
        .await
        .expect("introspect one schema");
    assert!(
        one.iter()
            .any(|t| t.table_name.eq_ignore_ascii_case("quarry_conformance")),
        "introspect_schema: table not found"
    );
    assert!(
        one.iter().all(|t| t.schema_name == table.schema_name),
        "introspect_schema: other schemas leaked in"
    );
    let start = s.load_catalog().await.expect("load catalog");
    assert!(
        start
            .find_table(Some(schema), "quarry_conformance")
            .is_some()
            || !start.is_loaded(schema),
        "load_catalog: a loaded schema is missing its table"
    );

    // Literals round-trip, including quotes; LIMIT comes from the dialect
    let insert = |id: i64, name: &str| {
        format!(
            "INSERT INTO {t} ({}, {}) VALUES ({id}, {})",
            d.quote_ident("id"),
            d.quote_ident("name"),
            d.literal(&DbValue::Text(name.into()))
        )
    };
    s.execute(&insert(1, "it's")).await.expect("insert");
    s.execute(&insert(2, "b")).await.expect("insert");
    let r = s
        .execute(&d.limit(
            &format!(
                "SELECT {} FROM {t} ORDER BY {}",
                d.quote_ident("name"),
                d.quote_ident("id")
            ),
            1,
            0,
        ))
        .await
        .expect("select with limit");
    assert_eq!(
        r.rows,
        vec![vec![DbValue::Text("it's".into())]],
        "select: quoting or limit"
    );

    // Grid edits apply atomically and refuse to overwrite a row someone else changed
    let key = |id: i64| vec![("id".to_string(), DbValue::Int(id))];
    let mut c = PendingChanges::new();
    c.set_cell(
        key(1),
        "name",
        DbValue::Text("it's".into()),
        DbValue::Text("a".into()),
    );
    c.row_inserts.push(PendingRowInsert {
        temp_id: "n".into(),
        values: [("id".to_string(), DbValue::Int(3))].into(),
    });
    assert_eq!(
        s.apply_edits(&t, &c).await.expect("apply edits"),
        2,
        "apply edits: count"
    );
    let mut stale = PendingChanges::new();
    stale.set_cell(
        key(1),
        "name",
        DbValue::Text("it's".into()),
        DbValue::Text("z".into()),
    );
    stale.toggle_delete(key(2));
    assert!(
        matches!(
            s.apply_edits(&t, &stale).await,
            Err(DbError::ConcurrentModification)
        ),
        "apply edits: stale edit must conflict"
    );
    let r = s
        .execute(&format!("SELECT count(*) FROM {t}"))
        .await
        .expect("count");
    assert_eq!(
        r.rows[0][0],
        DbValue::Int(3),
        "apply edits: a conflict must roll back the whole batch"
    );

    // Manual commit: changes wait for COMMIT and ROLLBACK undoes them
    s.tx().set_manual(true);
    s.execute(&format!("DELETE FROM {t}"))
        .await
        .expect("delete in transaction");
    assert!(
        s.in_transaction(),
        "manual commit: a write opens a transaction"
    );
    s.rollback().await.expect("rollback");
    s.tx().set_manual(false);
    let r = s
        .execute(&format!("SELECT count(*) FROM {t}"))
        .await
        .expect("count");
    assert_eq!(
        r.rows[0][0],
        DbValue::Int(3),
        "manual commit: rollback must undo"
    );

    // Explain, where the dialect has one, reads back as a plan tree. It may be several statements;
    // the plan is in the last one's rows.
    if let Some(explain) = d.explain(
        &format!("SELECT * FROM {t} WHERE {} = 1", d.quote_ident("id")),
        false,
    ) {
        let mut last = None;
        for stmt in explain.split(";\n") {
            last = Some(s.execute(stmt).await.expect("explain"));
        }
        let plan = d
            .plan(&last.expect("explain: no statement"))
            .expect("explain: plan didn't parse");
        assert!(!plan.root.op.is_empty(), "explain: empty plan");
    }

    // The DDL view works for every driver
    let info = s.table_info(table).await.expect("table info");
    assert!(
        info.ddl.to_lowercase().contains("quarry_conformance"),
        "table info: DDL names the table"
    );

    s.execute(&format!("DROP TABLE {t}"))
        .await
        .expect("drop table");
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    #[tokio::test]
    async fn sqlite_conforms() {
        let s = crate::SqliteSession::open_in_memory().unwrap();
        super::check(&s, "main").await;
    }
}
