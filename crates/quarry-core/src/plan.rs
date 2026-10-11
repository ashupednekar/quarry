// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

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
            detail.push(if alias.is_empty() || alias == s("Relation Name") { format!("on {}", s("Relation Name")) } else { format!("on {} {alias}", s("Relation Name")) });
        }
        if !s("Index Name").is_empty() {
            detail.push(format!("using {}", s("Index Name")));
        }
        for k in ["Index Cond", "Hash Cond", "Merge Cond", "Join Filter", "Filter", "Sort Key", "Group Key"] {
            match v.get(k) {
                Some(serde_json::Value::String(c)) => detail.push(format!("{}: {c}", k.to_lowercase())),
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
            children: v.get("Plans").and_then(|p| p.as_array()).map(|a| a.iter().map(node).collect()).unwrap_or_default(),
        }
    }
    Some(Plan { root: node(top.get("Plan")?), planning_ms: num(top, "Planning Time"), execution_ms: num(top, "Execution Time") })
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
                PlanNode { op: op.to_string(), detail: rest.to_string(), children: build(*id, rows), ..Default::default() }
            })
            .collect()
    }
    let mut top = build(0, &rows);
    let root = if top.len() == 1 { top.remove(0) } else { PlanNode { op: "QUERY PLAN".into(), children: top, ..Default::default() } };
    Some(Plan { root, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn result(columns: &[&str], rows: Vec<Vec<DbValue>>) -> QueryResult {
        QueryResult { columns: columns.iter().map(|c| c.to_string()).collect(), rows, rows_affected: 0, execution_time: Duration::ZERO, statement: String::new() }
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
        let plan = from_postgres_json(&result(&["QUERY PLAN"], vec![vec![DbValue::Json(json)]])).unwrap();
        assert_eq!(plan.root.op, "Inner Hash Join");
        assert_eq!(plan.root.detail, "hash cond: (o.user_id = u.id)");
        assert_eq!(plan.root.children[0].detail, "on orders o");
        assert_eq!(plan.root.children[1].detail, "on users · using users_pkey");
        assert_eq!(plan.root.children[1].ms, Some(1.0), "per-loop time × loops");
        assert_eq!(plan.root.self_ms(), Some(1.0));
        assert!(plan.root.misestimated(), "1000 estimated vs 10 actual");
        assert_eq!(plan.execution_ms, Some(3.2));
        assert_eq!(plan.root.flatten().iter().map(|(d, _)| *d).collect::<Vec<_>>(), vec![0, 1, 1]);
    }

    #[test]
    fn reads_sqlite_query_plans() {
        let row = |id: i64, parent: i64, d: &str| vec![DbValue::Int(id), DbValue::Int(parent), DbValue::Int(0), DbValue::Text(d.into())];
        let r = result(&["id", "parent", "notused", "detail"], vec![row(2, 0, "SCAN o"), row(5, 0, "SEARCH u USING INTEGER PRIMARY KEY (rowid=?)")]);
        let plan = from_sqlite_rows(&r).unwrap();
        assert_eq!(plan.root.op, "QUERY PLAN");
        assert_eq!((plan.root.children[1].op.as_str(), plan.root.children[1].detail.as_str()), ("SEARCH", "u USING INTEGER PRIMARY KEY (rowid=?)"));
        assert!(from_sqlite_rows(&result(&["a"], vec![])).is_none());
    }
}
