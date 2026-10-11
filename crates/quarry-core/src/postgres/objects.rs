// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Functions, procedures, sequences, triggers and types for the sidebar, and their definitions

use super::PostgresSession;
use crate::schema::{ObjectKind, SchemaObject};
use crate::session::DbError;

/// One row per object: (kind code, name, detail, oid). Objects that belong to an extension are
/// left out, so installing PostGIS doesn't bury the schema's own functions.
const OBJECTS_SQL: &str = "
    SELECT CASE p.prokind WHEN 'p' THEN 'p' ELSE 'f' END, p.proname, pg_get_function_identity_arguments(p.oid), p.oid::text
    FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname = $1 AND p.prokind IN ('f', 'p')
      AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = p.oid AND d.deptype = 'e')
    UNION ALL
    SELECT 'S', c.relname, '', c.oid::text
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = $1 AND c.relkind = 'S'
      AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype = 'e')
    UNION ALL
    SELECT 't', t.tgname, 'on ' || c.relname, t.oid::text
    FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = $1 AND NOT t.tgisinternal
    UNION ALL
    SELECT 'T', t.typname,
           CASE t.typtype WHEN 'e' THEN array_to_string(ARRAY(SELECT enumlabel FROM pg_enum WHERE enumtypid = t.oid ORDER BY enumsortorder), ', ')
                          ELSE format_type(t.typbasetype, t.typtypmod) END,
           t.oid::text
    FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
    WHERE n.nspname = $1 AND t.typtype IN ('e', 'd')
      AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = t.oid AND d.deptype = 'e')
    ORDER BY 1, 2";

impl PostgresSession {
    pub(super) async fn list_objects(&self, schema: &str) -> Result<Vec<SchemaObject>, DbError> {
        let rows = self.client.query(OBJECTS_SQL, &[&schema]).await.map_err(|e| DbError::SqlError(e.to_string()))?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let kind = match r.get::<_, &str>(0) {
                    "f" => ObjectKind::Function,
                    "p" => ObjectKind::Procedure,
                    "S" => ObjectKind::Sequence,
                    "t" => ObjectKind::Trigger,
                    "T" => ObjectKind::Type,
                    _ => return None,
                };
                Some(SchemaObject { kind, name: r.get(1), detail: r.get::<_, Option<String>>(2).unwrap_or_default(), id: r.get(3) })
            })
            .collect())
    }

    pub(super) async fn definition(&self, obj: &SchemaObject) -> Result<String, DbError> {
        let sql = match obj.kind {
            ObjectKind::Function | ObjectKind::Procedure => "SELECT pg_get_functiondef($1::text::oid)",
            ObjectKind::Trigger => "SELECT pg_get_triggerdef($1::text::oid, true) || ';'",
            ObjectKind::Sequence => {
                "SELECT format('CREATE SEQUENCE %I.%I AS %s INCREMENT %s MINVALUE %s MAXVALUE %s START %s CACHE %s%s;',
                        s.schemaname, s.sequencename, s.data_type::regtype, s.increment_by, s.min_value, s.max_value,
                        s.start_value, s.cache_size, CASE WHEN s.cycle THEN ' CYCLE' ELSE '' END)
                 FROM pg_sequences s JOIN pg_class c ON c.relname = s.sequencename
                 JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = s.schemaname
                 WHERE c.oid = $1::text::oid"
            }
            ObjectKind::Type => {
                "SELECT CASE t.typtype
                   WHEN 'e' THEN format('CREATE TYPE %I.%I AS ENUM (%s);', n.nspname, t.typname,
                        (SELECT string_agg(quote_literal(enumlabel), ', ' ORDER BY enumsortorder) FROM pg_enum WHERE enumtypid = t.oid))
                   ELSE format('CREATE DOMAIN %I.%I AS %s%s%s;', n.nspname, t.typname, format_type(t.typbasetype, t.typtypmod),
                        CASE WHEN t.typnotnull THEN ' NOT NULL' ELSE '' END,
                        COALESCE(' DEFAULT ' || t.typdefault, ''))
                 END
                 FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace WHERE t.oid = $1::text::oid"
            }
        };
        let row = self.client.query_opt(sql, &[&obj.id]).await.map_err(|e| DbError::SqlError(e.to_string()))?;
        row.and_then(|r| r.get::<_, Option<String>>(0)).ok_or_else(|| DbError::SqlError(format!("{} no longer exists", obj.name)))
    }
}
