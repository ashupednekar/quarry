// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use quarry_core::{DatabaseSession, PostgresSession};

/// Session must survive across separate runtime calls (connection driver lifetime).
/// Runs only when QUARRY_PG_URL is set, e.g. postgres://postgres:postgres@localhost:55432/postgres
#[test]
fn postgres_session_survives_between_calls() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    for _ in 0..3 {
        rt.block_on(s.execute("SELECT 1")).unwrap();
    }
}

/// Types the old binary decoder panicked on must come back intact.
#[test]
fn postgres_decodes_every_type() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    let sql = "SELECT 1.10::numeric AS n, '6f1c1d1e-0000-4000-8000-000000000001'::uuid AS u, \
               DATE '2026-10-06' AS d, ARRAY[1,2] AS a, '10.0.0.1'::inet AS ip, interval '1 day' AS iv, \
               '{\"k\":1}'::jsonb AS j, '\\x00ff'::bytea AS b, NULL::text AS nul, '' AS empty, \
               9223372036854775807::bigint AS big, now() AS ts";
    let r = rt.block_on(s.execute(sql)).unwrap();
    println!("{:?}", r.columns);
    println!("{:?}", r.rows[0]);
    use quarry_core::DbValue::*;
    assert_eq!(r.rows[0][0], Decimal("1.10".into()));
    assert_eq!(r.rows[0][3], Text("{1,2}".into()));
    assert_eq!(r.rows[0][7], Blob(vec![0, 255]));
    assert_eq!(r.rows[0][8], Null);
    assert_eq!(r.rows[0][9], Text(String::new()));
    assert_eq!(r.rows[0][10], Int(i64::MAX));

    // Zero-row results still report their columns
    let empty = rt.block_on(s.execute("SELECT 1 AS x WHERE false")).unwrap();
    assert_eq!(empty.columns, vec!["x"]);
}

/// Grid edits against Postgres: uuid PK, numeric value, conflict detection, insert + delete in one transaction.
#[test]
fn postgres_applies_edits_atomically() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    use quarry_core::edits::{qualified_name, PendingChanges, PendingRowInsert};
    use quarry_core::{DbError, DbValue::*};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    rt.block_on(s.execute("DROP TABLE IF EXISTS edit_t")).unwrap();
    rt.block_on(s.execute("CREATE TABLE edit_t (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), amount numeric(10,2), note text)")).unwrap();
    rt.block_on(s.execute("INSERT INTO edit_t (id, amount, note) VALUES ('6f1c1d1e-0000-4000-8000-000000000001', 1.10, NULL)")).unwrap();
    let table = qualified_name("public", "edit_t");
    let key = vec![("id".to_string(), Text("6f1c1d1e-0000-4000-8000-000000000001".into()))];

    let mut c = PendingChanges::new();
    c.set_cell(key.clone(), "amount", Decimal("1.10".into()), Decimal("99.95".into()));
    c.set_cell(key.clone(), "note", Null, Text("it's fine".into()));
    c.row_inserts.push(PendingRowInsert { temp_id: "t".into(), values: [("amount".to_string(), Decimal("5".into()))].into() });
    assert_eq!(rt.block_on(s.apply_edits(&table, &c)).unwrap(), 2);
    let r = rt.block_on(s.execute("SELECT amount, note FROM edit_t ORDER BY amount DESC")).unwrap();
    assert_eq!(r.rows[0], vec![Decimal("99.95".into()), Text("it's fine".into())]);
    assert_eq!(r.rows.len(), 2);

    // Stale edit (old value no longer matches) + an insert: conflict, and the insert must roll back too
    let mut stale2 = PendingChanges::new();
    stale2.set_cell(key.clone(), "amount", Decimal("1.10".into()), Decimal("0".into()));
    stale2.row_inserts.push(PendingRowInsert { temp_id: "u".into(), values: Default::default() });
    assert!(matches!(rt.block_on(s.apply_edits(&table, &stale2)), Err(DbError::ConcurrentModification)));
    let count = rt.block_on(s.execute("SELECT count(*) FROM edit_t")).unwrap();
    assert_eq!(count.rows[0][0], Int(2), "failed submit must not leave the insert behind");
    rt.block_on(s.execute("DROP TABLE edit_t")).unwrap();
}

/// Introspection reports foreign keys with the referenced schema (G5)
#[test]
fn postgres_introspects_foreign_keys() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS fk_test CASCADE",
        "CREATE SCHEMA fk_test",
        "CREATE TABLE fk_test.parent (id int PRIMARY KEY)",
        "CREATE TABLE fk_test.child (id int PRIMARY KEY, parent_id int REFERENCES fk_test.parent(id))",
    ] {
        rt.block_on(s.execute(sql)).unwrap();
    }
    let cat = rt.block_on(s.introspect()).unwrap();
    let child = cat.find_table(Some("fk_test"), "child").unwrap();
    let fk = &child.foreign_keys[0];
    assert_eq!((fk.from_column.as_str(), fk.to_schema.as_str(), fk.to_table.as_str(), fk.to_column.as_str()), ("parent_id", "fk_test", "parent", "id"));
    rt.block_on(s.execute("DROP SCHEMA fk_test CASCADE")).unwrap();
}

/// Table info, monitoring and bytea edits go through the driver, not app-side SQL
#[test]
fn postgres_driver_extras() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    use quarry_core::edits::PendingChanges;
    use quarry_core::monitor::{ServerView, Snapshot};
    use quarry_core::DbValue::*;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    for sql in [
        "DROP TABLE IF EXISTS extras_t",
        "CREATE TABLE extras_t (id int PRIMARY KEY, code text UNIQUE, data bytea)",
        "COMMENT ON TABLE extras_t IS 'for tests'",
        "INSERT INTO extras_t VALUES (1, 'a', NULL)",
    ] {
        rt.block_on(s.execute(sql)).unwrap();
    }
    let table = rt.block_on(s.introspect()).unwrap().find_table(Some("public"), "extras_t").cloned().unwrap();
    let info = rt.block_on(s.table_info(&table)).unwrap();
    assert!(info.ddl.contains("CREATE TABLE \"public\".\"extras_t\"") && info.ddl.contains("UNIQUE (code)") && info.ddl.contains("'for tests'"), "{}", info.ddl);
    assert_eq!(info.indexes.iter().map(|i| (i.name.as_str(), i.unique)).collect::<Vec<_>>(), vec![("extras_t_code_key", true), ("extras_t_pkey", true)]);

    let monitor = s.monitor().unwrap();
    assert!(matches!(rt.block_on(monitor.snapshot(ServerView::Database)).unwrap(), Snapshot::Rows(r) if !r.rows.is_empty()));

    // Blob literals use bytea's hex form
    let mut c = PendingChanges::new();
    c.set_cell(vec![("id".into(), Int(1))], "data", Null, Blob(vec![0, 255]));
    rt.block_on(s.apply_edits("\"public\".\"extras_t\"", &c)).unwrap();
    let r = rt.block_on(s.execute("SELECT data FROM extras_t")).unwrap();
    assert_eq!(r.rows[0][0], Blob(vec![0, 255]));

    // pg_dump runs with the session's own URL; skipped where the client tools aren't installed
    let out = std::env::temp_dir().join("quarry_live_dump.sql");
    match rt.block_on(s.dumper().unwrap().dump(&out)) {
        Ok(_) => assert!(std::fs::read_to_string(&out).unwrap().contains("CREATE TABLE public.extras_t")),
        Err(e) => assert!(e.contains("isn't installed") || e.contains("version mismatch"), "{e}"),
    }
    rt.block_on(s.execute("DROP TABLE extras_t")).unwrap();
}
