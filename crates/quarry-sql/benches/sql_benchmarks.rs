// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use quarry_core::schema::{ColumnSchema, ColumnType, SchemaCatalog, TableSchema};
use quarry_sql::{bind_params, find_params, split_statements, SchemaCompleter};
use std::collections::HashMap;

fn generate_sql_script(num_statements: usize) -> String {
    let mut script = String::with_capacity(num_statements * 128);
    for i in 0..num_statements {
        script.push_str(&format!(
            "-- statement index {i}\nSELECT id, name, created_at FROM table_{i} WHERE status = 'active' AND val > {i};\n"
        ));
    }
    script
}

fn build_large_catalog(num_tables: usize) -> SchemaCatalog {
    let mut tables = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let cols = vec![
            ColumnSchema {
                name: "id".to_string(),
                data_type: ColumnType::Integer,
                raw_type_name: "integer".to_string(),
                is_nullable: false,
                is_primary_key: true,
                default_value: None,
                ordinal_position: 1,
            },
            ColumnSchema {
                name: "customer_id".to_string(),
                data_type: ColumnType::Integer,
                raw_type_name: "integer".to_string(),
                is_nullable: false,
                is_primary_key: false,
                default_value: None,
                ordinal_position: 2,
            },
            ColumnSchema {
                name: "name".to_string(),
                data_type: ColumnType::Text,
                raw_type_name: "varchar(255)".to_string(),
                is_nullable: true,
                is_primary_key: false,
                default_value: None,
                ordinal_position: 3,
            },
            ColumnSchema {
                name: "amount".to_string(),
                data_type: ColumnType::Float,
                raw_type_name: "numeric(12,2)".to_string(),
                is_nullable: true,
                is_primary_key: false,
                default_value: None,
                ordinal_position: 4,
            },
        ];
        tables.push(TableSchema {
            schema_name: "public".to_string(),
            table_name: format!("tbl_order_records_{i}"),
            is_view: false,
            columns: cols,
            primary_keys: vec!["id".to_string()],
            foreign_keys: Vec::new(),
        });
    }
    SchemaCatalog {
        schemas: vec!["public".to_string()],
        tables,
        default_schema: Some("public".to_string()),
        unloaded: Vec::new(),
    }
}

fn bench_sql_splitter(c: &mut Criterion) {
    let script_1000 = generate_sql_script(1000);
    c.bench_function("sql_split_1000_statements", |b| {
        b.iter(|| {
            let spans = split_statements(black_box(&script_1000));
            black_box(spans);
        })
    });

    let dollar_sql = r#"
        CREATE OR REPLACE FUNCTION process_audit() RETURNS trigger AS $body$
        BEGIN
            INSERT INTO audit_log (table_name, operation, timestamp)
            VALUES (TG_TABLE_NAME, TG_OP, NOW());
            RETURN NEW;
        END;
        $body$ LANGUAGE plpgsql;
        SELECT * FROM audit_log WHERE timestamp > NOW() - INTERVAL '1 day';
    "#;
    c.bench_function("sql_split_dollar_quoted", |b| {
        b.iter(|| {
            let spans = split_statements(black_box(dollar_sql));
            black_box(spans);
        })
    });
}

fn bench_completion(c: &mut Criterion) {
    let catalog = build_large_catalog(1000);

    c.bench_function("completion_1000_tables_prefix", |b| {
        b.iter(|| {
            let res = SchemaCompleter::complete(
                black_box("SELECT * FROM tbl_order_records_5"),
                34,
                black_box(&catalog),
            );
            black_box(res);
        })
    });

    c.bench_function("completion_column_after_alias", |b| {
        b.iter(|| {
            let res = SchemaCompleter::complete(
                black_box("SELECT o.cust FROM tbl_order_records_100 o WHERE o."),
                51,
                black_box(&catalog),
            );
            black_box(res);
        })
    });
}

fn bench_params(c: &mut Criterion) {
    let sql = "SELECT * FROM users WHERE id = :id AND org_id = :org_id AND status = :status AND flag = $1";
    let mut values = HashMap::new();
    values.insert(":id".to_string(), "42".to_string());
    values.insert(":org_id".to_string(), "99".to_string());
    values.insert(":status".to_string(), "active".to_string());
    values.insert("$1".to_string(), "true".to_string());

    c.bench_function("params_find_and_bind", |b| {
        b.iter(|| {
            let params = find_params(black_box(sql));
            let bound = bind_params(black_box(sql), black_box(&values));
            black_box((params, bound));
        })
    });
}

criterion_group!(benches, bench_sql_splitter, bench_completion, bench_params);
criterion_main!(benches);
