//! MySQL iterator JSON and single-table version 1 estimates, retaining raw properties.

use serde_json::Value;

use super::{Object, Operator, Plan, Statement, cost_percent, raw_json};
use crate::session::PlanFormat;

/// Malformed JSON or invalid known operator fields.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// JSON syntax error with line and column.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A required operator container/name or a nonnegative counter was invalid.
    #[error("invalid MySQL plan: {0}")]
    Invalid(String),
}

/// Parses version 2 EXPLAIN JSON, or version 1 single-table estimated data changes.
/// Rows and elapsed milliseconds are totals over loops; raw counters retain server units.
pub fn parse(payload: &str) -> Result<Plan, Error> {
    let node: Value = serde_json::from_str(payload)?;
    if let Some(block) = node.get("query_block") {
        return single_table(payload, block);
    }
    let cost = number(&node, "estimated_total_cost")?.unwrap_or(0.0);
    Ok(Plan {
        payload: payload.into(),
        format: PlanFormat::MySqlJson,
        statements: vec![Statement {
            text: string(&node, "query"),
            root: operator(&node, cost, 1.0, 0)?,
        }],
        properties: Vec::new(),
    })
}

fn single_table(payload: &str, block: &Value) -> Result<Plan, Error> {
    let table = block
        .get("table")
        .ok_or_else(|| Error::Invalid("no single-table estimate".into()))?;
    let rows = number(table, "rows_examined_per_scan")?
        .ok_or_else(|| Error::Invalid("table has no row estimate".into()))?;
    // Version 1 encodes the percentage as a decimal string, unlike version 2 counters.
    let filtered = match table.get("filtered") {
        Some(value) => value
            .as_str()
            .and_then(|value| value.parse::<f64>().ok())
            .or_else(|| value.as_f64())
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            .ok_or_else(|| Error::Invalid("filtered must be a percentage".into()))?,
        None => 100.0,
    };
    let name = string(table, "access_type").unwrap_or_else(|| "Table access".into());
    let mut properties = Vec::new();
    raw_json(table, "", None, &mut properties);
    Ok(Plan {
        payload: payload.into(),
        format: PlanFormat::MySqlJson,
        statements: vec![Statement {
            text: None,
            root: Operator {
                kind: name.clone(),
                name,
                estimated_rows: rows * (filtered / 100.0),
                estimated_rows_total: rows * (filtered / 100.0),
                actual_rows: None,
                subtree_cost: 0.0,
                own_cost: 0.0,
                cost_percent: 0.0,
                elapsed_ms: None,
                cpu_ms: None,
                logical_reads: None,
                physical_reads: None,
                warnings: Vec::new(),
                object: Some(Object {
                    database: None,
                    schema: None,
                    table: string(table, "table_name"),
                    index: string(table, "key"),
                }),
                parallel: false,
                children: Vec::new(),
                properties,
            },
        }],
        properties: {
            let mut properties = Vec::new();
            raw_json(block, "query_block", Some("table"), &mut properties);
            properties
        },
    })
}

fn number(node: &Value, key: &str) -> Result<Option<f64>, Error> {
    node.get(key)
        .map(|value| {
            value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(|| Error::Invalid(format!("{key} must be a nonnegative number")))
        })
        .transpose()
}

fn string(node: &Value, key: &str) -> Option<String> {
    node.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn operator(node: &Value, total: f64, executions: f64, depth: usize) -> Result<Operator, Error> {
    if depth > 256 {
        return Err(Error::Invalid("operator nesting exceeds 256".into()));
    }
    let name =
        string(node, "operation").ok_or_else(|| Error::Invalid("missing operation".into()))?;
    let loops = number(node, "actual_loops")?.unwrap_or(1.0);
    let rows = number(node, "estimated_rows")?.unwrap_or(0.0);
    let subtree_cost = number(node, "estimated_total_cost")?.unwrap_or(0.0) * executions;
    let mut children = Vec::new();
    let nested_loop = string(node, "join_algorithm").as_deref() == Some("nested_loop");
    let mut child_executions = executions;
    if let Some(inputs) = node.get("inputs") {
        let inputs = inputs
            .as_array()
            .ok_or_else(|| Error::Invalid("inputs must be an array".into()))?;
        for input in inputs {
            children.push(operator(input, total, child_executions, depth + 1)?);
            if nested_loop {
                child_executions *= number(input, "estimated_rows")?.unwrap_or(0.0);
            }
        }
    }
    let own_cost =
        (subtree_cost - children.iter().map(|child| child.subtree_cost).sum::<f64>()).max(0.0);
    let object = Object {
        database: string(node, "schema_name"),
        schema: None,
        table: string(node, "table_name"),
        index: string(node, "index_name"),
    };
    let mut properties = Vec::new();
    raw_json(node, "", Some("inputs"), &mut properties);
    Ok(Operator {
        kind: string(node, "join_type")
            .or_else(|| string(node, "access_type"))
            .unwrap_or_else(|| name.clone()),
        name,
        estimated_rows: rows,
        estimated_rows_total: rows * loops,
        actual_rows: number(node, "actual_rows")?.map(|rows| rows * loops),
        subtree_cost,
        own_cost,
        cost_percent: cost_percent(own_cost, total),
        elapsed_ms: number(node, "actual_last_row_ms")?.map(|ms| ms * loops),
        cpu_ms: None,
        logical_reads: None,
        physical_reads: None,
        warnings: node
            .get("warnings")
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        object: (object.table.is_some() || object.index.is_some()).then_some(object),
        parallel: false,
        children,
        properties,
    })
}
