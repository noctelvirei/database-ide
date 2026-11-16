//! PROTOTYPE. Reads PostgreSQL `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` output into the
//! shared operator tree. PostgreSQL reports rows and times per loop; they are multiplied out
//! here so they compare with SQL Server's totals.

use serde_json::Value;

use crate::model::{Engine, Op, Plan};

fn f(node: &Value, key: &str) -> f64 {
    node.get(key).and_then(Value::as_f64).unwrap_or(0.)
}

fn s<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The name EXPLAIN's text format would show, e.g. "HashAggregate" rather than "Aggregate".
fn display_name(node: &Value) -> String {
    let kind = s(node, "Node Type");
    match (kind, s(node, "Strategy")) {
        ("Aggregate", "Hashed") => "HashAggregate".into(),
        ("Aggregate", "Sorted") => "GroupAggregate".into(),
        _ => kind.into(),
    }
}

fn icon_for(name: &str, node: &Value) -> &'static str {
    match name {
        "Seq Scan" => "table_scan",
        "Index Scan" | "Index Only Scan" if node.get("Index Cond").is_some() => "index_seek",
        "Index Scan" | "Index Only Scan" => "index_scan",
        "Bitmap Heap Scan" => "table_scan",
        "Bitmap Index Scan" => "bitmap",
        "Nested Loop" => "nested_loops",
        "Hash Join" | "Hash" | "HashAggregate" => "hash_match",
        "Merge Join" => "merge_join",
        "Sort" | "Incremental Sort" => "sort",
        "GroupAggregate" => "stream_aggregate",
        "Aggregate" => "aggregate",
        "Limit" => "top",
        "Gather" | "Gather Merge" => "parallelism",
        "Materialize" | "Memoize" => "table_spool",
        _ => "iterator_catch_all",
    }
}

/// Adds `node` and its children to `ops` in pre-order, with `subtree` getting its Total Cost
/// over all the executions the planner expected. PostgreSQL costs a node once per loop, so a
/// nested loop's inner side is multiplied by the outer side's estimated rows (`est_loops`).
fn walk(
    node: &Value,
    parent: Option<usize>,
    est_loops: f64,
    ops: &mut Vec<Op>,
    subtree: &mut Vec<f64>,
) {
    let name = display_name(node);
    let loops = f(node, "Actual Loops").max(1.);
    let object = match (s(node, "Relation Name"), s(node, "Index Name")) {
        ("", "") => String::new(),
        (rel, "") => rel.to_string(),
        ("", ix) => ix.to_string(),
        (rel, ix) => format!("{rel}.{ix}"),
    };
    let logical = match s(node, "Join Type") {
        "" => String::new(),
        j => format!("{j} Join"),
    };
    let mut warnings = Vec::new();
    if s(node, "Sort Space Type") == "Disk" {
        warnings.push(format!(
            "Sort spilled to disk ({} kB)",
            f(node, "Sort Space Used")
        ));
    }
    if f(node, "Hash Batches") > 1. {
        warnings.push(format!(
            "Hash spilled into {} batches",
            f(node, "Hash Batches")
        ));
    }
    if f(node, "Disk Usage") > 0. {
        warnings.push(format!(
            "Aggregate spilled to disk ({} kB)",
            f(node, "Disk Usage")
        ));
    }
    let mut details = Vec::new();
    for key in [
        "Filter",
        "Index Cond",
        "Hash Cond",
        "Join Filter",
        "Merge Cond",
        "Recheck Cond",
    ] {
        if let Some(v) = node.get(key).and_then(Value::as_str) {
            details.push((key.to_string(), v.to_string()));
        }
    }
    for key in ["Sort Key", "Group Key"] {
        if let Some(Value::Array(keys)) = node.get(key) {
            let keys: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
            details.push((key.to_string(), keys.join(", ")));
        }
    }
    if let Some(m) = node.get("Sort Method").and_then(Value::as_str) {
        details.push((
            "Sort method".into(),
            format!("{m}, {} kB", f(node, "Sort Space Used")),
        ));
    }
    if node.get("Peak Memory Usage").is_some() {
        details.push((
            "Peak memory".into(),
            format!("{} kB", f(node, "Peak Memory Usage")),
        ));
    }
    if node.get("Rows Removed by Filter").is_some() {
        details.push((
            "Rows removed by filter (per loop)".into(),
            f(node, "Rows Removed by Filter").to_string(),
        ));
    }
    details.push((
        "Rows per loop (est / actual)".into(),
        format!("{} / {}", f(node, "Plan Rows"), f(node, "Actual Rows")),
    ));
    details.push((
        "Cost (startup .. total)".into(),
        format!("{} .. {}", f(node, "Startup Cost"), f(node, "Total Cost")),
    ));
    details.push((
        "Buffers (shared hit / read)".into(),
        format!(
            "{} / {}",
            f(node, "Shared Hit Blocks"),
            f(node, "Shared Read Blocks")
        ),
    ));

    let index = ops.len();
    ops.push(Op {
        icon: icon_for(&name, node),
        name,
        logical,
        object,
        parent,
        children: vec![],
        est_rows: f(node, "Plan Rows") * loops,
        act_rows: f(node, "Actual Rows") * loops,
        executions: loops,
        cost_pct: 0.,
        subtree_cost_pct: 0.,
        self_ms: 0.,
        total_ms: f(node, "Actual Total Time") * loops,
        reads: f(node, "Shared Hit Blocks") + f(node, "Shared Read Blocks"),
        parallel: node
            .get("Parallel Aware")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        warnings,
        details,
    });
    subtree.push(f(node, "Total Cost") * est_loops);
    if let Some(Value::Array(children)) = node.get("Plans") {
        let nested_loop = s(node, "Node Type") == "Nested Loop";
        for (i, child) in children.iter().enumerate() {
            let loops = if nested_loop && i == 1 {
                est_loops * f(&children[0], "Plan Rows")
            } else {
                est_loops
            };
            walk(child, Some(index), loops, ops, subtree);
        }
    }
}

/// Parses EXPLAIN JSON text (an array holding one object) into a plan, with `statement` as
/// the query text. Panics on malformed input, fine for the prototype's bundled sample.
pub fn parse(json: &str, statement: &str, title: &'static str) -> Plan {
    let doc: Value = serde_json::from_str(json).expect("bundled sample is valid JSON");
    let top = &doc[0];
    let root = &top["Plan"];
    let mut ops = Vec::new();
    let mut subtree = Vec::new();
    // A statement node like SSMS's, so both engines' trees start the same way.
    ops.push(Op {
        name: "SELECT".into(),
        logical: String::new(),
        object: String::new(),
        icon: "result",
        parent: None,
        children: vec![],
        est_rows: f(root, "Plan Rows"),
        act_rows: f(root, "Actual Rows"),
        executions: 1.,
        cost_pct: 0.,
        subtree_cost_pct: 0.,
        self_ms: 0.,
        total_ms: f(root, "Actual Total Time"),
        reads: 0.,
        parallel: false,
        warnings: vec![],
        details: vec![],
    });
    subtree.push(f(root, "Total Cost"));
    walk(root, Some(0), 1., &mut ops, &mut subtree);
    let mut plan = Plan {
        engine: Engine::Postgres,
        title,
        statement: statement.trim().to_string(),
        ops,
        elapsed_ms: f(top, "Execution Time"),
        facts: vec![
            (
                "Planning time".into(),
                format!("{} ms", f(top, "Planning Time")),
            ),
            (
                "Execution time".into(),
                format!("{} ms", f(top, "Execution Time")),
            ),
        ],
    };
    plan.finish(&subtree, true);
    // The synthetic statement node's reads would be the whole plan's; it does no work itself.
    plan.ops[0].reads = 0.;
    plan
}
