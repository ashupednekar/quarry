// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

#![cfg(feature = "postgres")]

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

/// The shared driver contract (quarry_core::conformance)
#[test]
fn postgres_conforms() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    rt.block_on(quarry_core::conformance::check(&s, "public"));
}

/// EXPLAIN through the dialect reads back as a plan tree
#[test]
fn postgres_explain_reads_as_a_plan() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    let d = s.dialect();
    let sql = "SELECT * FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relname = 'x'";
    let r = rt.block_on(s.execute(&d.explain(sql, true).unwrap())).unwrap();
    let plan = d.plan(&r).expect("plan");
    assert!(plan.execution_ms.is_some() && plan.root.ms.is_some() && !plan.root.children.is_empty(), "{plan:?}");
}

/// Functions, sequences, triggers and types are listed per schema with their definitions
#[test]
fn postgres_lists_schema_objects() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    use quarry_core::schema::ObjectKind::*;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS obj_test CASCADE",
        "CREATE SCHEMA obj_test",
        "CREATE TYPE obj_test.mood AS ENUM ('sad', 'ok')",
        "CREATE DOMAIN obj_test.pos AS int NOT NULL",
        "CREATE SEQUENCE obj_test.counter START 5",
        "CREATE TABLE obj_test.t (id int)",
        "CREATE FUNCTION obj_test.touch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$",
        "CREATE PROCEDURE obj_test.noop(a int) LANGUAGE sql AS $$ SELECT 1 $$",
        "CREATE TRIGGER t_touch BEFORE INSERT ON obj_test.t FOR EACH ROW EXECUTE FUNCTION obj_test.touch()",
    ] {
        rt.block_on(s.execute(sql)).unwrap();
    }
    let objects = rt.block_on(s.objects("obj_test")).unwrap();
    let names: Vec<_> = objects.iter().map(|o| (o.kind, o.name.as_str(), o.detail.as_str())).collect();
    for want in [(Function, "touch", ""), (Procedure, "noop", "IN a integer"), (Sequence, "counter", ""), (Trigger, "t_touch", "on t"), (Type, "mood", "sad, ok"), (Type, "pos", "integer")] {
        assert!(names.contains(&want), "{want:?} missing from {names:?}");
    }
    for o in &objects {
        let ddl = rt.block_on(s.object_ddl("obj_test", o)).unwrap();
        assert!(ddl.starts_with("CREATE"), "{}: {ddl}", o.name);
    }
    rt.block_on(s.execute("DROP SCHEMA obj_test CASCADE")).unwrap();
}

/// Results past MAX_ROWS are counted, not kept, and the connection stays usable
#[test]
fn postgres_caps_big_results() {
    let Ok(url) = std::env::var("QUARRY_PG_URL") else { return };
    use quarry_core::session::MAX_ROWS;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(PostgresSession::connect(&url, false)).unwrap();
    let r = rt.block_on(s.execute(&format!("SELECT generate_series(1, {})", MAX_ROWS + 7))).unwrap();
    assert_eq!((r.rows.len(), r.rows_affected, r.truncated()), (MAX_ROWS, MAX_ROWS as u64 + 7, true));
    assert_eq!(rt.block_on(s.execute("SELECT 42")).unwrap().rows[0][0], quarry_core::DbValue::Int(42));
}

/// The MySQL driver passes the shared contract.
/// Runs only when QUARRY_MYSQL_URL is set, e.g. mysql://root:mysql@127.0.0.1:53306/quarry_test
#[cfg(feature = "mysql")]
#[test]
fn mysql_conforms() {
    let Ok(url) = std::env::var("QUARRY_MYSQL_URL") else { return };
    use quarry_core::mysql::MysqlSession;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(MysqlSession::connect(&url, false)).unwrap();
    rt.block_on(quarry_core::conformance::check(&s, "quarry_test"));

    // Types decode exactly; backslashes and quotes survive the dialect's literals
    for sql in [
        "DROP TABLE IF EXISTS types_t",
        "CREATE TABLE types_t (i BIGINT UNSIGNED, d DECIMAL(30,10), f DOUBLE, j JSON, b VARBINARY(4), t TEXT, dt DATETIME(3))",
    ] {
        rt.block_on(s.execute(sql)).unwrap();
    }
    let d = s.dialect();
    let text = quarry_core::DbValue::Text(r"C:\new 'quoted'".into());
    rt.block_on(s.execute(&format!(
        "INSERT INTO types_t VALUES (18446744073709551615, 12345678901234567890.0123456789, 1.5, '{{\"a\":1}}', X'00FF', {}, '2026-01-02 03:04:05.678')",
        d.literal(&text)
    )))
    .unwrap();
    let r = rt.block_on(s.execute("SELECT * FROM types_t")).unwrap();
    use quarry_core::DbValue::*;
    assert_eq!(
        r.rows[0],
        vec![
            Decimal("18446744073709551615".into()),
            Decimal("12345678901234567890.0123456789".into()),
            Float(1.5),
            Json(serde_json::json!({"a": 1})),
            Blob(vec![0, 255]),
            text,
            Text("2026-01-02 03:04:05.678".into()),
        ]
    );
    let monitor = s.monitor().unwrap();
    assert!(matches!(rt.block_on(monitor.snapshot(quarry_core::monitor::ServerView::Activity)), Ok(quarry_core::monitor::Snapshot::Rows(_))));
    rt.block_on(s.execute("DROP TABLE types_t")).unwrap();
}

/// SQL Server passes the shared contract.
/// Runs only when QUARRY_MSSQL_URL is set, e.g. sqlserver://sa:Quarry_test1@127.0.0.1:51433/master
#[cfg(feature = "mssql")]
#[test]
fn mssql_conforms() {
    let Ok(url) = std::env::var("QUARRY_MSSQL_URL") else { return };
    use quarry_core::mssql::MssqlSession;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(MssqlSession::connect(&url, false)).unwrap();
    rt.block_on(quarry_core::conformance::check(&s, "dbo"));

    use quarry_core::DbValue::*;
    let r = rt.block_on(s.execute(
        "SELECT CAST(12345678901234567890.0123456789 AS decimal(30,10)), CAST(0x00FF AS varbinary(2)), N'naïve', \
         CAST('2026-01-02 03:04:05.678' AS datetime2(3)), CAST(1 AS bit), CAST(NULL AS int)",
    ))
    .unwrap();
    assert_eq!(
        r.rows[0],
        vec![Decimal("12345678901234567890.0123456789".into()), Blob(vec![0, 255]), Text("naïve".into()), Text("2026-01-02 03:04:05.678".into()), Boolean(true), Null]
    );
    // A procedure must be alone in its batch: no rowcount is appended
    rt.block_on(s.execute("CREATE OR ALTER PROCEDURE dbo.quarry_p AS BEGIN SELECT 1; END")).unwrap();
    let objects = rt.block_on(s.objects("dbo")).unwrap();
    let p = objects.iter().find(|o| o.name == "quarry_p").expect("procedure listed");
    assert!(rt.block_on(s.object_ddl("dbo", p)).unwrap().contains("SELECT 1"));
    rt.block_on(s.execute("DROP PROCEDURE dbo.quarry_p")).unwrap();

    use quarry_core::monitor::{ServerView, Snapshot};
    let monitor = s.monitor().unwrap();
    for view in [ServerView::Activity, ServerView::Locks] {
        assert!(matches!(rt.block_on(monitor.snapshot(view)), Ok(Snapshot::Rows(_))), "{view:?}");
    }
}

/// Oracle passes the shared contract. Needs Instant Client (ORACLE_CLIENT_LIB_DIR).
/// Runs only when QUARRY_ORACLE_URL is set, e.g. oracle://quarry:quarry@127.0.0.1:51521/FREEPDB1
#[cfg(feature = "oracle")]
#[test]
fn oracle_conforms() {
    let Ok(url) = std::env::var("QUARRY_ORACLE_URL") else { return };
    use quarry_core::oracle::OracleSession;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(OracleSession::connect(&url, false)).unwrap();
    rt.block_on(quarry_core::conformance::check(&s, "QUARRY"));

    use quarry_core::DbValue::*;
    let r = rt.block_on(s.execute("SELECT 12345678901234567890.0123456789, HEXTORAW('00FF'), 'naïve', CAST(NULL AS NUMBER), 42 FROM dual;")).unwrap();
    assert_eq!(r.rows[0], vec![Decimal("12345678901234567890.0123456789".into()), Blob(vec![0, 255]), Text("naïve".into()), Null, Int(42)]);
    // PL/SQL keeps its semicolons; plain SQL loses its trailing one
    rt.block_on(s.execute("CREATE OR REPLACE PROCEDURE quarry_p AS BEGIN NULL; END;")).unwrap();
    let objects = rt.block_on(s.objects("QUARRY")).unwrap();
    let p = objects.iter().find(|o| o.name == "QUARRY_P").expect("procedure listed");
    assert!(rt.block_on(s.object_ddl("QUARRY", p)).unwrap().contains("PROCEDURE"));
    rt.block_on(s.execute("DROP PROCEDURE quarry_p")).unwrap();
}

/// Without Instant Client, connecting explains what to install instead of failing obscurely.
/// Runs only when QUARRY_ORACLE_NO_CLIENT is set (and no client is findable).
#[cfg(feature = "oracle")]
#[test]
fn oracle_without_client_says_what_to_install() {
    if std::env::var("QUARRY_ORACLE_NO_CLIENT").is_err() {
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    let res = rt.block_on(quarry_core::oracle::OracleSession::connect("oracle://u:p@127.0.0.1:51521/FREEPDB1", false));
    let msg = res.err().expect("no client, no connection").to_string();
    assert!(msg.contains("Instant Client isn't installed"), "{msg}");
}
