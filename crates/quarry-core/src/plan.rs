// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! A query plan as a tree, read from EXPLAIN output by the dialect (`Dialect::plan`)

use crate::session::QueryResult;
use crate::value::DbValue;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanNode {
    /// What the step does: "Seq Scan", "Hash Join", "SCAN users"
    pub op: String,
    /// On what and how: table, index, condition
    pub detail: String,
    /// Planner's total cost for this step and everything under it
    pub cost: Option<f64>,
    pub est_rows: Option<f64>,
    /// EXPLAIN ANALYZE: time for this step and everything under it, all loops
    pub ms: Option<f64>,
    pub rows: Option<f64>,
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    /// Time spent in this step alone (its total minus its children's)
    pub fn self_ms(&self) -> Option<f64> {
        let children: f64 = self.children.iter().filter_map(|c| c.ms).sum();
        self.ms.map(|ms| (ms - children).max(0.0))
    }

    /// Estimated vs actual rows off by 10× or more: the usual cause of a bad plan
    pub fn misestimated(&self) -> bool {
        match (self.est_rows, self.rows) {
            (Some(e), Some(a)) => (e.max(1.0) / a.max(1.0)).max(a.max(1.0) / e.max(1.0)) >= 10.0,
            _ => false,
        }
    }

    /// Depth-first, with each node's depth
    pub fn flatten(&self) -> Vec<(usize, &PlanNode)> {
        fn walk<'a>(n: &'a PlanNode, depth: usize, out: &mut Vec<(usize, &'a PlanNode)>) {
            out.push((depth, n));
            n.children.iter().for_each(|c| walk(c, depth + 1, out));
        }
        let mut out = Vec::new();
        walk(self, 0, &mut out);
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub root: PlanNode,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
}

/// Postgres `EXPLAIN (FORMAT JSON)`: one cell holding `[{"Plan": {...}, "Execution Time": ...}]`
pub fn from_postgres_json(r: &QueryResult) -> Option<Plan> {
    let json = match r.rows.first()?.first()? {
        DbValue::Json(j) => j.clone(),
        DbValue::Text(t) => serde_json::from_str(t).ok()?,
        _ => return None,
    };
    let top = json.get(0)?;
    let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64());
    fn node(v: &serde_json::Value) -> PlanNode {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        let f = |k: &str| v.get(k).and_then(|x| x.as_f64());
        let mut op = s("Node Type").to_string();
        if !s("Join Type").is_empty() && op.contains("Join") {
            op = format!("{} {}", s("Join Type"), op);
        }
        let mut detail = Vec::new();
        if !s("Relation Name").is_empty() {
            let alias = s("Alias");
            detail.push(if alias.is_empty() || alias == s("Relation Name") {
                format!("on {}", s("Relation Name"))
            } else {
                format!("on {} {alias}", s("Relation Name"))
            });
        }
        if !s("Index Name").is_empty() {
            detail.push(format!("using {}", s("Index Name")));
        }
        for k in [
            "Index Cond",
            "Hash Cond",
            "Merge Cond",
            "Join Filter",
            "Filter",
            "Sort Key",
            "Group Key",
        ] {
            match v.get(k) {
                Some(serde_json::Value::String(c)) => {
                    detail.push(format!("{}: {c}", k.to_lowercase()))
                }
                Some(serde_json::Value::Array(a)) => {
                    let items: Vec<&str> = a.iter().filter_map(|x| x.as_str()).collect();
                    detail.push(format!("{}: {}", k.to_lowercase(), items.join(", ")));
                }
                _ => {}
            }
        }
        let loops = f("Actual Loops").unwrap_or(1.0);
        PlanNode {
            op,
            detail: detail.join(" · "),
            cost: f("Total Cost"),
            est_rows: f("Plan Rows"),
            // Per-loop figures: times and rows repeat for every loop
            ms: f("Actual Total Time").map(|t| t * loops),
            rows: f("Actual Rows").map(|r| r * loops),
            children: v
                .get("Plans")
                .and_then(|p| p.as_array())
                .map(|a| a.iter().map(node).collect())
                .unwrap_or_default(),
        }
    }
    Some(Plan {
        root: node(top.get("Plan")?),
        planning_ms: num(top, "Planning Time"),
        execution_ms: num(top, "Execution Time"),
    })
}

/// SQLite `EXPLAIN QUERY PLAN`: rows of (id, parent, notused, detail)
pub fn from_sqlite_rows(r: &QueryResult) -> Option<Plan> {
    if r.columns.len() < 4 || r.columns[3] != "detail" {
        return None;
    }
    let rows: Vec<(i64, i64, String)> = r
        .rows
        .iter()
        .map(|row| {
            let int = |v: &DbValue| if let DbValue::Int(i) = v { *i } else { 0 };
            (int(&row[0]), int(&row[1]), row[3].to_display_string())
        })
        .collect();
    fn build(parent: i64, rows: &[(i64, i64, String)]) -> Vec<PlanNode> {
        rows.iter()
            .filter(|(_, p, _)| *p == parent)
            .map(|(id, _, detail)| {
                // "SEARCH users USING INDEX x (id=?)" → op "SEARCH", detail "users USING INDEX x (id=?)"
                let (op, rest) = detail.split_once(' ').unwrap_or((detail, ""));
                PlanNode {
                    op: op.to_string(),
                    detail: rest.to_string(),
                    children: build(*id, rows),
                    ..Default::default()
                }
            })
            .collect()
    }
    let mut top = build(0, &rows);
    let root = if top.len() == 1 {
        top.remove(0)
    } else {
        PlanNode {
            op: "QUERY PLAN".into(),
            children: top,
            ..Default::default()
        }
    };
    Some(Plan {
        root,
        ..Default::default()
    })
}

/// MySQL `EXPLAIN FORMAT=JSON`: one cell holding `{"query_block": {...}}`, where steps nest as
/// ordering/grouping wrappers around a `nested_loop` of `table`s
pub fn from_mysql_json(r: &QueryResult) -> Option<Plan> {
    let json: serde_json::Value = match r.rows.first()?.first()? {
        DbValue::Json(j) => j.clone(),
        DbValue::Text(t) => serde_json::from_str(t).ok()?,
        _ => return None,
    };
    // Costs and row counts come as numbers or as strings like "0.90"
    fn num(v: Option<&serde_json::Value>) -> Option<f64> {
        v.and_then(|x| x.as_f64().or_else(|| x.as_str()?.parse().ok()))
    }
    fn table(t: &serde_json::Value) -> PlanNode {
        let s = |k: &str| t.get(k).and_then(|x| x.as_str()).unwrap_or("");
        let op = match s("access_type") {
            "ALL" => "Full scan",
            "index" => "Index scan",
            "range" => "Range scan",
            "ref" | "eq_ref" | "ref_or_null" => "Index lookup",
            "const" | "system" => "Single row",
            other => other,
        };
        let mut detail = vec![format!("on {}", s("table_name"))];
        if !s("key").is_empty() {
            detail.push(format!("using {}", s("key")));
        }
        if !s("attached_condition").is_empty() {
            detail.push(format!("filter: {}", s("attached_condition")));
        }
        PlanNode {
            op: op.to_string(),
            detail: detail.join(" · "),
            cost: num(t.pointer("/cost_info/prefix_cost")),
            est_rows: num(t.get("rows_produced_per_join")).or(num(t.get("rows_examined_per_scan"))),
            children: t
                .pointer("/materialized_from_subquery/query_block")
                .map(|q| vec![node(q, "Subquery")])
                .unwrap_or_default(),
            ..Default::default()
        }
    }
    /// One step: `v` is the object holding it, `op` what to call it
    fn node(v: &serde_json::Value, op: &str) -> PlanNode {
        let mut n = PlanNode {
            op: op.to_string(),
            cost: num(v.pointer("/cost_info/query_cost"))
                .or(num(v.pointer("/cost_info/sort_cost"))),
            ..Default::default()
        };
        let flag = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        n.detail = [
            ("using_filesort", "filesort"),
            ("using_temporary_table", "temporary table"),
        ]
        .iter()
        .filter(|(k, _)| flag(k))
        .map(|(_, d)| *d)
        .collect::<Vec<_>>()
        .join(" · ");
        for (key, name) in [
            ("ordering_operation", "Sort"),
            ("grouping_operation", "Group"),
            ("duplicates_removal", "Distinct"),
            ("windowing", "Window"),
        ] {
            if let Some(inner) = v.get(key) {
                n.children.push(node(inner, name));
            }
        }
        if let Some(loops) = v.get("nested_loop").and_then(|l| l.as_array()) {
            let tables: Vec<PlanNode> = loops
                .iter()
                .filter_map(|l| l.get("table"))
                .map(table)
                .collect();
            // One table needs no join step around it
            if tables.len() == 1 {
                n.children.extend(tables);
            } else {
                let hash = loops.iter().any(|l| {
                    l.pointer("/table/using_join_buffer")
                        .and_then(|b| b.as_str())
                        == Some("hash join")
                });
                let cost = tables.last().and_then(|t| t.cost);
                n.children.push(PlanNode {
                    op: if hash { "Hash join" } else { "Nested loop" }.into(),
                    cost,
                    children: tables,
                    ..Default::default()
                });
            }
        }
        if let Some(t) = v.get("table") {
            n.children.push(table(t));
        }
        if let Some(specs) = v
            .pointer("/union_result/query_specifications")
            .and_then(|q| q.as_array())
        {
            n.children.push(PlanNode {
                op: "Union".into(),
                children: specs
                    .iter()
                    .filter_map(|q| q.get("query_block"))
                    .map(|q| node(q, "Select"))
                    .collect(),
                ..Default::default()
            });
        }
        // A wrapper with one step under it and nothing of its own collapses into that step
        if n.op == "Query" && n.children.len() == 1 && n.detail.is_empty() {
            // The whole query's cost beats the step's own
            let mut only = n.children.remove(0);
            only.cost = n.cost.or(only.cost);
            return only;
        }
        n
    }
    Some(Plan {
        root: node(json.get("query_block")?, "Query"),
        ..Default::default()
    })
}

/// Oracle `PLAN_TABLE` rows: (ID, PARENT_ID, OPERATION, OPTIONS, OBJECT_NAME, COST, CARDINALITY)
pub fn from_oracle_plan_table(r: &QueryResult) -> Option<Plan> {
    let cols: Vec<String> = r.columns.iter().map(|c| c.to_ascii_uppercase()).collect();
    if cols
        != [
            "ID",
            "PARENT_ID",
            "OPERATION",
            "OPTIONS",
            "OBJECT_NAME",
            "COST",
            "CARDINALITY",
        ]
        || r.rows.is_empty()
    {
        return None;
    }
    let text = |v: &DbValue| {
        if matches!(v, DbValue::Null) {
            String::new()
        } else {
            v.to_display_string()
        }
    };
    let num = |v: &DbValue| text(v).parse::<f64>().ok();
    // (id, parent, node); the root has no parent
    let rows: Vec<(Option<f64>, Option<f64>, PlanNode)> = r
        .rows
        .iter()
        .map(|row| {
            let op = format!("{} {}", text(&row[2]), text(&row[3]))
                .trim()
                .to_string();
            let object = text(&row[4]);
            let node = PlanNode {
                op,
                detail: if object.is_empty() {
                    String::new()
                } else {
                    format!("on {object}")
                },
                cost: num(&row[5]),
                est_rows: num(&row[6]),
                ..Default::default()
            };
            (num(&row[0]), num(&row[1]), node)
        })
        .collect();
    fn build(parent: Option<f64>, rows: &[(Option<f64>, Option<f64>, PlanNode)]) -> Vec<PlanNode> {
        rows.iter()
            .filter(|(_, p, _)| *p == parent)
            .map(|(id, _, n)| PlanNode {
                children: build(*id, rows),
                ..n.clone()
            })
            .collect()
    }
    let mut top = build(None, &rows);
    let root = if top.len() == 1 {
        top.remove(0)
    } else {
        PlanNode {
            op: "PLAN".into(),
            children: top,
            ..Default::default()
        }
    };
    Some(Plan {
        root,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn result(columns: &[&str], rows: Vec<Vec<DbValue>>) -> QueryResult {
        QueryResult {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: rows.into(),
            rows_affected: 0,
            execution_time: Duration::ZERO,
            statement: String::new(),
        }
    }

    #[test]
    fn reads_postgres_json_plans() {
        let json = serde_json::json!([{
            "Plan": {"Node Type": "Hash Join", "Join Type": "Inner", "Hash Cond": "(o.user_id = u.id)", "Total Cost": 40.5, "Plan Rows": 1000,
                     "Actual Total Time": 3.0, "Actual Rows": 10, "Actual Loops": 1,
                     "Plans": [
                        {"Node Type": "Seq Scan", "Relation Name": "orders", "Alias": "o", "Total Cost": 20.0, "Plan Rows": 1000, "Actual Total Time": 1.0, "Actual Rows": 1000, "Actual Loops": 1},
                        {"Node Type": "Index Scan", "Relation Name": "users", "Alias": "users", "Index Name": "users_pkey", "Total Cost": 8.0, "Plan Rows": 1, "Actual Total Time": 0.25, "Actual Rows": 1, "Actual Loops": 4}
                     ]},
            "Planning Time": 0.1, "Execution Time": 3.2
        }]);
        let plan =
            from_postgres_json(&result(&["QUERY PLAN"], vec![vec![DbValue::Json(json)]])).unwrap();
        assert_eq!(plan.root.op, "Inner Hash Join");
        assert_eq!(plan.root.detail, "hash cond: (o.user_id = u.id)");
        assert_eq!(plan.root.children[0].detail, "on orders o");
        assert_eq!(plan.root.children[1].detail, "on users · using users_pkey");
        assert_eq!(plan.root.children[1].ms, Some(1.0), "per-loop time × loops");
        assert_eq!(plan.root.self_ms(), Some(1.0));
        assert!(plan.root.misestimated(), "1000 estimated vs 10 actual");
        assert_eq!(plan.execution_ms, Some(3.2));
        assert_eq!(
            plan.root
                .flatten()
                .iter()
                .map(|(d, _)| *d)
                .collect::<Vec<_>>(),
            vec![0, 1, 1]
        );
    }

    #[test]
    fn reads_sqlite_query_plans() {
        let row = |id: i64, parent: i64, d: &str| {
            vec![
                DbValue::Int(id),
                DbValue::Int(parent),
                DbValue::Int(0),
                DbValue::Text(d.into()),
            ]
        };
        let r = result(
            &["id", "parent", "notused", "detail"],
            vec![
                row(2, 0, "SCAN o"),
                row(5, 0, "SEARCH u USING INTEGER PRIMARY KEY (rowid=?)"),
            ],
        );
        let plan = from_sqlite_rows(&r).unwrap();
        assert_eq!(plan.root.op, "QUERY PLAN");
        assert_eq!(
            (
                plan.root.children[1].op.as_str(),
                plan.root.children[1].detail.as_str()
            ),
            ("SEARCH", "u USING INTEGER PRIMARY KEY (rowid=?)")
        );
        assert!(from_sqlite_rows(&result(&["a"], vec![])).is_none());
    }

    #[test]
    fn reads_mysql_json_plans() {
        // Trimmed from MySQL 8.4: books join authors order by name
        let json = r#"{"query_block": {"select_id": 1, "cost_info": {"query_cost": "3.11"},
            "ordering_operation": {"using_temporary_table": true, "using_filesort": true, "cost_info": {"sort_cost": "2.00"},
              "nested_loop": [
                {"table": {"table_name": "b", "access_type": "ALL", "rows_produced_per_join": 2, "cost_info": {"prefix_cost": "0.45"}}},
                {"table": {"table_name": "a", "access_type": "eq_ref", "key": "PRIMARY", "rows_produced_per_join": 2, "using_join_buffer": "hash join",
                           "cost_info": {"prefix_cost": "1.12"}, "attached_condition": "(a.id = b.author_id)"}}]}}}"#;
        let plan = from_mysql_json(&result(
            &["EXPLAIN"],
            vec![vec![DbValue::Text(json.into())]],
        ))
        .unwrap();
        assert_eq!(
            (
                plan.root.op.as_str(),
                plan.root.detail.as_str(),
                plan.root.cost
            ),
            ("Sort", "filesort · temporary table", Some(3.11))
        );
        let join = &plan.root.children[0];
        assert_eq!((join.op.as_str(), join.cost), ("Hash join", Some(1.12)));
        assert_eq!(
            (
                join.children[0].op.as_str(),
                join.children[0].detail.as_str(),
                join.children[0].est_rows
            ),
            ("Full scan", "on b", Some(2.0))
        );
        assert_eq!(
            join.children[1].detail,
            "on a · using PRIMARY · filter: (a.id = b.author_id)"
        );
        assert!(from_mysql_json(&result(
            &["x"],
            vec![vec![DbValue::Text("not json".into())]]
        ))
        .is_none());
    }

    #[test]
    fn reads_oracle_plan_table() {
        let row = |id: i64, parent: Option<i64>, op: &str, opts: &str, obj: &str, cost: i64| {
            vec![
                DbValue::Int(id),
                parent.map_or(DbValue::Null, DbValue::Int),
                DbValue::Text(op.into()),
                DbValue::Text(opts.into()),
                if obj.is_empty() {
                    DbValue::Null
                } else {
                    DbValue::Text(obj.into())
                },
                DbValue::Int(cost),
                DbValue::Int(2),
            ]
        };
        let r = result(
            &[
                "ID",
                "PARENT_ID",
                "OPERATION",
                "OPTIONS",
                "OBJECT_NAME",
                "COST",
                "CARDINALITY",
            ],
            vec![
                row(0, None, "SELECT STATEMENT", "", "", 4),
                row(1, Some(0), "HASH JOIN", "", "", 4),
                row(2, Some(1), "TABLE ACCESS", "FULL", "BOOKS", 2),
                row(3, Some(1), "TABLE ACCESS", "FULL", "AUTHORS", 2),
            ],
        );
        let plan = from_oracle_plan_table(&r).unwrap();
        assert_eq!(plan.root.op, "SELECT STATEMENT");
        let join = &plan.root.children[0];
        assert_eq!(join.op, "HASH JOIN");
        assert_eq!(
            (
                join.children[1].op.as_str(),
                join.children[1].detail.as_str(),
                join.children[1].cost
            ),
            ("TABLE ACCESS FULL", "on AUTHORS", Some(2.0))
        );
    }
}
