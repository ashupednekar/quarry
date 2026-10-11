// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Schema diff: the statements that make `target` match `source`, for review. Nothing here runs SQL.
//! Covers tables and columns (type, nullability, default); drops are written commented out.

use crate::dialect::Dialect;
use crate::schema::{ColumnSchema, SchemaCatalog, TableSchema};

fn key(t: &TableSchema) -> (&str, &str) {
    (&t.schema_name, &t.table_name)
}

fn same_type(a: &ColumnSchema, b: &ColumnSchema) -> bool {
    a.raw_type_name.eq_ignore_ascii_case(&b.raw_type_name)
}

/// A script that turns `target` into `source`, or `None` when they already match
pub fn diff(source: &SchemaCatalog, target: &SchemaCatalog, d: &dyn Dialect) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    let src: Vec<&TableSchema> = source.tables.iter().filter(|t| !t.is_view).collect();
    let dst: Vec<&TableSchema> = target.tables.iter().filter(|t| !t.is_view).collect();

    for t in &src {
        let Some(other) = dst.iter().find(|o| key(o) == key(t)) else {
            out.push(format!("-- New table\n{}", t.create_sql(d)));
            continue;
        };
        let name = d.qualified(&t.schema_name, &t.table_name);
        let mut alters = Vec::new();
        for c in &t.columns {
            let col = d.quote_ident(&c.name);
            let Some(o) = other.columns.iter().find(|o| o.name == c.name) else {
                let mut add = format!("ALTER TABLE {name} ADD COLUMN {col} {}", c.raw_type_name.to_lowercase());
                if let Some(def) = &c.default_value {
                    add += &format!(" DEFAULT {def}");
                }
                if !c.is_nullable {
                    add += " NOT NULL";
                }
                alters.push(add + ";");
                continue;
            };
            if !same_type(c, o) {
                alters.push(format!("ALTER TABLE {name} ALTER COLUMN {col} TYPE {}; -- was {}", c.raw_type_name.to_lowercase(), o.raw_type_name.to_lowercase()));
            }
            if c.is_nullable != o.is_nullable {
                alters.push(format!("ALTER TABLE {name} ALTER COLUMN {col} {} NOT NULL;", if c.is_nullable { "DROP" } else { "SET" }));
            }
            if c.default_value != o.default_value {
                alters.push(match &c.default_value {
                    Some(def) => format!("ALTER TABLE {name} ALTER COLUMN {col} SET DEFAULT {def};"),
                    None => format!("ALTER TABLE {name} ALTER COLUMN {col} DROP DEFAULT;"),
                });
            }
        }
        for o in other.columns.iter().filter(|o| !t.columns.iter().any(|c| c.name == o.name)) {
            alters.push(format!("-- ALTER TABLE {name} DROP COLUMN {}; -- only in the target; uncomment to drop it and its data", d.quote_ident(&o.name)));
        }
        if !alters.is_empty() {
            out.push(format!("-- {}.{}\n{}", t.schema_name, t.table_name, alters.join("\n")));
        }
    }
    let extra: Vec<String> = dst
        .iter()
        .filter(|o| !src.iter().any(|t| key(t) == key(o)))
        .map(|o| format!("-- DROP TABLE {};", d.qualified(&o.schema_name, &o.table_name)))
        .collect();
    if !extra.is_empty() {
        out.push(format!("-- Only in the target; uncomment to drop them and their data\n{}", extra.join("\n")));
    }
    (!out.is_empty()).then(|| out.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Postgres;
    use crate::schema::ColumnType;

    fn col(name: &str, ty: &str, nullable: bool, default: Option<&str>) -> ColumnSchema {
        ColumnSchema { name: name.into(), data_type: ColumnType::from_sql_str(ty), raw_type_name: ty.into(), is_nullable: nullable, is_primary_key: false, default_value: default.map(String::from), ordinal_position: 0 }
    }

    fn table(name: &str, columns: Vec<ColumnSchema>) -> TableSchema {
        TableSchema { schema_name: "public".into(), table_name: name.into(), is_view: false, columns, primary_keys: vec![], foreign_keys: vec![] }
    }

    fn catalog(tables: Vec<TableSchema>) -> SchemaCatalog {
        SchemaCatalog { schemas: vec!["public".into()], tables, default_schema: None }
    }

    #[test]
    fn writes_what_turns_target_into_source() {
        let source = catalog(vec![
            table("users", vec![col("id", "bigint", false, None), col("email", "text", false, None), col("plan", "text", true, Some("'free'"))]),
            table("audit", vec![col("id", "bigint", false, None)]),
        ]);
        let target = catalog(vec![
            table("users", vec![col("id", "integer", false, None), col("email", "text", true, None), col("legacy", "text", true, None)]),
            table("old_stuff", vec![col("id", "integer", false, None)]),
        ]);
        let script = diff(&source, &target, &Postgres).unwrap();
        for want in [
            r#"ALTER TABLE "public"."users" ALTER COLUMN "id" TYPE bigint; -- was integer"#,
            r#"ALTER TABLE "public"."users" ALTER COLUMN "email" SET NOT NULL;"#,
            r#"ALTER TABLE "public"."users" ADD COLUMN "plan" text DEFAULT 'free';"#,
            r#"-- ALTER TABLE "public"."users" DROP COLUMN "legacy";"#,
            r#"CREATE TABLE "public"."audit""#,
            r#"-- DROP TABLE "public"."old_stuff";"#,
        ] {
            assert!(script.contains(want), "missing {want}\n---\n{script}");
        }
    }

    #[test]
    fn identical_schemas_have_no_diff() {
        let c = catalog(vec![table("t", vec![col("id", "int", false, None)])]);
        assert_eq!(diff(&c, &c, &Postgres), None);
    }
}
