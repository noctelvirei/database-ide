//! PostgreSQL EXPLAIN JSON, preserving unknown fields without schema validation.

use serde_json::Value;

use super::{Object, Operator, Plan, Statement, cost_percent, raw_json};
use crate::session::PlanFormat;

/// JSON syntax and required plan-shape failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// JSON was malformed, with the decoder's line and column.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A required container or operator field was absent or had the wrong type.
    #[error("invalid EXPLAIN plan: {0}")]
    Invalid(String),
}

/// Parses a JSON array containing one EXPLAIN object per statement, with optional ANALYZE.
/// Unknown fields are retained; invalid required fields and negative counters are errors.
pub fn parse(payload: &str) -> Result<Plan, Error> {
    let json: Value = serde_json::from_str(payload)?;
    let entries = json
        .as_array()
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| Error::Invalid("expected a nonempty statement array".into()))?;
    let mut statements = Vec::new();
    let mut properties = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        raw_json(entry, &format!("[{index}]"), Some("Plan"), &mut properties);
        let node = entry
            .get("Plan")
            .ok_or_else(|| Error::Invalid("statement has no Plan".into()))?;
        let total = number(node, "Total Cost")?.unwrap_or(0.0);
        statements.push(Statement {
            text: string(entry, "Query Text"),
            root: operator(node, total, false, 0)?,
        });
    }
    Ok(Plan {
        payload: payload.into(),
        format: PlanFormat::ExplainJson,
        statements,
        properties,
    })
}

fn number(node: &Value, key: &str) -> Result<Option<f64>, Error> {
    node.get(key)
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| Error::Invalid(format!("{key} must be a nonnegative number")))
        })
        .transpose()
}

fn string(node: &Value, key: &str) -> Option<String> {
    node.get(key).and_then(Value::as_str).map(String::from)
}

fn reads(node: &Value, keys: &[&str]) -> Result<Option<u64>, Error> {
    let mut total = None::<u64>;
    for key in keys {
        if let Some(value) = node.get(key) {
            let count = value
                .as_u64()
                .ok_or_else(|| Error::Invalid(format!("{key} must be an unsigned integer")))?;
            total = Some(
                total
                    .unwrap_or(0)
                    .checked_add(count)
                    .ok_or_else(|| Error::Invalid("buffer count overflow".into()))?,
            );
        }
    }
    Ok(total)
}

fn operator(
    node: &Value,
    statement_cost: f64,
    in_worker: bool,
    depth: usize,
) -> Result<Operator, Error> {
    if depth > 256 {
        return Err(Error::Invalid("operator nesting exceeds 256".into()));
    }
    let name = string(node, "Node Type")
        .ok_or_else(|| Error::Invalid("operator has no Node Type".into()))?;
    let gathers_workers = matches!(name.as_str(), "Gather" | "Gather Merge");
    let rows = number(node, "Plan Rows")?
        .ok_or_else(|| Error::Invalid("operator has no Plan Rows".into()))?;
    let loops = number(node, "Actual Loops")?.unwrap_or(1.0);
    let subtree_cost = number(node, "Total Cost")?
        .ok_or_else(|| Error::Invalid("operator has no Total Cost".into()))?;
    let mut children = Vec::new();
    if let Some(plans) = node.get("Plans") {
        let plans = plans
            .as_array()
            .ok_or_else(|| Error::Invalid("Plans must be an array".into()))?;
        for child in plans {
            let separate_plan = matches!(
                child.get("Parent Relationship").and_then(Value::as_str),
                Some("InitPlan" | "SubPlan")
            );
            children.push(operator(
                child,
                statement_cost,
                (in_worker || gathers_workers) && !separate_plan,
                depth + 1,
            )?);
        }
    }
    let own_cost =
        (subtree_cost - children.iter().map(|child| child.subtree_cost).sum::<f64>()).max(0.0);
    let mut properties = Vec::new();
    raw_json(node, "", Some("Plans"), &mut properties);
    let mut warnings = Vec::new();
    if node.get("Sort Space Type").and_then(Value::as_str) == Some("Disk")
        || number(node, "Hash Batches")?.is_some_and(|batches| batches > 1.0)
        || number(node, "Disk Usage")?.is_some_and(|usage| usage > 0.0)
    {
        warnings.push("Spill to disk".into());
    }
    if let Some(warning) = node.get("Warnings") {
        warnings.push(warning.to_string());
    }
    let object = Object {
        database: None,
        schema: string(node, "Schema"),
        table: string(node, "Relation Name"),
        index: string(node, "Index Name"),
    };
    Ok(Operator {
        kind: string(node, "Join Type")
            .or_else(|| string(node, "Strategy"))
            .unwrap_or_else(|| name.clone()),
        name,
        estimated_rows: rows,
        estimated_rows_total: rows * loops,
        actual_rows: number(node, "Actual Rows")?.map(|rows| rows * loops),
        subtree_cost,
        own_cost,
        cost_percent: cost_percent(own_cost, statement_cost),
        elapsed_ms: number(node, "Actual Total Time")?.map(|time| time * loops),
        cpu_ms: None,
        logical_reads: reads(
            node,
            &[
                "Shared Hit Blocks",
                "Shared Read Blocks",
                "Local Hit Blocks",
                "Local Read Blocks",
            ],
        )?,
        physical_reads: reads(node, &["Shared Read Blocks", "Local Read Blocks"])?,
        warnings,
        object: (object.table.is_some() || object.index.is_some()).then_some(object),
        parallel: in_worker
            || gathers_workers
            || node.get("Parallel Aware").and_then(Value::as_bool) == Some(true),
        children,
        properties,
    })
}
