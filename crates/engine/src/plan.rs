//! Execution plans as data, with original payloads retained independently of parsed operators.

use serde::Serialize;
use serde_json::Value;

use crate::session::PlanFormat;

pub mod explain;
pub mod mysql;
pub mod showplan;

/// A raw inspector property. Paths include child indices; repeated names retain their order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Property {
    /// Element/JSON path, with XML attributes prefixed by `@`.
    pub name: String,
    /// Decoded XML text or a JSON value encoded as JSON (including containers).
    pub value: String,
}

/// A table or index referenced by an operator; names retain the server's quoting.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Object {
    /// Database name when supplied.
    pub database: Option<String>,
    /// Schema name when supplied.
    pub schema: Option<String>,
    /// Table or relation name when supplied.
    pub table: Option<String>,
    /// Index name when supplied.
    pub index: Option<String>,
}

/// One operator, with inclusive runtime counters and children in server plan order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Operator {
    /// Physical operator name; SQL Server statements have a synthetic statement root.
    pub name: String,
    /// Logical operator name, or PostgreSQL/MySQL join type/access strategy when supplied.
    pub kind: String,
    /// Estimated rows for one execution, before execution/loop normalization.
    pub estimated_rows: f64,
    /// Estimated rows multiplied by estimated SQL Server executions or actual PostgreSQL/MySQL loops.
    pub estimated_rows_total: f64,
    /// Actual rows summed over threads/executions, or multiplied by PostgreSQL/MySQL loops.
    pub actual_rows: Option<f64>,
    /// Estimated inclusive cost; PostgreSQL/MySQL inner costs use estimated outer cardinalities.
    pub subtree_cost: f64,
    /// Estimated operator cost excluding children, bounded below by zero.
    pub own_cost: f64,
    /// Own cost divided by the statement's inclusive cost, in percent; zero for a zero cost.
    pub cost_percent: f64,
    /// Inclusive milliseconds; maximum thread time on SQL Server, total over PostgreSQL/MySQL loops.
    pub elapsed_ms: Option<f64>,
    /// CPU milliseconds summed over SQL Server threads, absent when not reported.
    pub cpu_ms: Option<f64>,
    /// SQL Server logical reads (including LOB) or PostgreSQL shared/local blocks hit + read.
    pub logical_reads: Option<u64>,
    /// SQL Server physical reads (including LOB) or PostgreSQL shared/local blocks read.
    pub physical_reads: Option<u64>,
    /// Server warnings rendered as text, without applying analysis heuristics.
    pub warnings: Vec<String>,
    /// The operator's table/index, excluding references belonging to child operators.
    pub object: Option<Object>,
    /// Whether marked parallel/parallel aware, a Gather, or part of its worker subtree.
    pub parallel: bool,
    /// Immediate input operators in the order reported by the server.
    pub children: Vec<Operator>,
    /// All raw properties belonging to this operator, excluding child operators.
    pub properties: Vec<Property>,
}

impl Operator {
    /// Larger/smaller row count ratio (at least 1); both zero gives 1, one zero infinity.
    /// Returns None without actual rows. A value of at least 10 flags either direction.
    pub fn estimate_error(&self) -> Option<f64> {
        self.actual_rows.map(|actual| {
            let estimate = self.estimated_rows_total;
            if actual == estimate {
                1.0
            } else {
                actual.max(estimate) / actual.min(estimate)
            }
        })
    }
}

/// One statement in batch order, with the SQL text when the server includes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Statement {
    /// Unescaped statement text, absent when the payload omits it.
    pub text: Option<String>,
    /// Statement root and its ordered inputs.
    pub root: Operator,
}

/// Parsed statements and document metadata, retaining the exact original payload text.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    payload: String,
    format: PlanFormat,
    /// Statements in batch order.
    pub statements: Vec<Statement>,
    /// All document properties outside operator subtrees, including statement metadata.
    pub properties: Vec<Property>,
}

impl Plan {
    /// Returns the original text, including its whitespace and XML declaration.
    pub fn payload(&self) -> &str {
        &self.payload
    }
}

/// Parsing failures retain the source format's actionable error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQL Server XML could not be decoded or interpreted.
    #[error(transparent)]
    Showplan(#[from] showplan::Error),
    /// PostgreSQL JSON could not be decoded or interpreted.
    #[error(transparent)]
    Explain(#[from] explain::Error),
    /// MySQL iterator JSON could not be decoded or interpreted.
    #[error(transparent)]
    MySql(#[from] mysql::Error),
}

/// Parses an Event::Plan payload in one call; malformed payloads return a format-specific error.
pub fn parse(payload: &str, format: PlanFormat) -> Result<Plan, Error> {
    Ok(match format {
        PlanFormat::ShowplanXml => showplan::parse(payload)?,
        PlanFormat::ExplainJson => explain::parse(payload)?,
        PlanFormat::MySqlJson => mysql::parse(payload)?,
    })
}

fn cost_percent(own: f64, statement: f64) -> f64 {
    if statement > 0.0 {
        own / statement * 100.0
    } else {
        0.0
    }
}

fn raw_json(node: &Value, path: &str, excluded: Option<&str>, properties: &mut Vec<Property>) {
    match node {
        Value::Object(fields) => {
            for (name, value) in fields {
                if Some(name.as_str()) != excluded {
                    let path = if path.is_empty() {
                        name.clone()
                    } else {
                        format!("{path}/{name}")
                    };
                    properties.push(Property {
                        name: path.clone(),
                        value: value.to_string(),
                    });
                    raw_json(value, &path, None, properties);
                }
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                let path = format!("{path}[{index}]");
                properties.push(Property {
                    name: path.clone(),
                    value: value.to_string(),
                });
                raw_json(value, &path, None, properties);
            }
        }
        _ => {}
    }
}
