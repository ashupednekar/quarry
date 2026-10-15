// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use quarry_core::lexer::SqlLexer;
use quarry_core::session::{is_mutation, is_read};
use quarry_core::value::DbValue;

fn bench_value_display(c: &mut Criterion) {
    let sample_values = vec![
        DbValue::Null,
        DbValue::Boolean(true),
        DbValue::Int(123456789),
        DbValue::Float(9876.54321),
        DbValue::Text("Sample customer address line 1, suite 200".to_string()),
        DbValue::Decimal("1234567890.123456".to_string()),
        DbValue::Timestamp(chrono::DateTime::UNIX_EPOCH),
    ];

    c.bench_function("value_to_display_cow_100k", |b| {
        b.iter(|| {
            for _ in 0..15_000 {
                for v in &sample_values {
                    let s = v.to_display_cow();
                    black_box(s);
                }
            }
        })
    });

    c.bench_function("value_format_display_100k", |b| {
        use std::fmt::Write;
        let mut buffer = String::with_capacity(128);
        b.iter(|| {
            for _ in 0..15_000 {
                for v in &sample_values {
                    buffer.clear();
                    let _ = write!(&mut buffer, "{}", v);
                    black_box(&buffer);
                }
            }
        })
    });
}

fn bench_lexer_and_inspection(c: &mut Criterion) {
    let sql_complex = r#"
        -- Top-level comment before query
        WITH ranked_orders AS (
            SELECT o.id, o.amount, o.status,
                   ROW_NUMBER() OVER (PARTITION BY o.customer_id ORDER BY o.created_at DESC) as rn
            FROM orders o
            WHERE o.status != 'cancelled' AND o.created_at >= '2026-01-01'
        )
        SELECT r.id, r.amount, c.name, 'prefix:' || c.code as full_code
        FROM ranked_orders r
        JOIN customers c ON c.id = r.customer_id
        WHERE r.rn = 1
        ORDER BY r.amount DESC
        LIMIT 100;
    "#;

    c.bench_function("sql_lexer_tokenize_complex_query", |b| {
        b.iter(|| {
            let tokens = SqlLexer::new(black_box(sql_complex)).collect::<Vec<_>>();
            black_box(tokens);
        })
    });

    c.bench_function("sql_is_read_classification", |b| {
        b.iter(|| {
            let res = is_read(black_box(sql_complex));
            black_box(res);
        })
    });

    let mutation_sql = "/* note */ UPDATE users SET last_login = NOW() WHERE id = 42;";
    c.bench_function("sql_is_mutation_classification", |b| {
        b.iter(|| {
            let res = is_mutation(black_box(mutation_sql));
            black_box(res);
        })
    });
}

criterion_group!(benches, bench_value_display, bench_lexer_and_inspection);
criterion_main!(benches);
