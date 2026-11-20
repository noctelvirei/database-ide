//! Isolates flagged data changes and reads their optimizer estimates without running them.

use futures::StreamExt;
use sqlparser::ast::{SetExpr, Statement};
use sqlparser::dialect::{Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Location, Token};

use super::{DangerCheck, DangerKind, Engine};
use crate::session::{Event, PlanFormat, PlanMode, Session};

struct Candidate<'a> {
    kind: DangerKind,
    line: u64,
    sql: Option<&'a str>,
}

/// Estimates flagged UPDATE/DELETE rows without executing them, in finding order.
/// Use the same batch and engine as `check`. Other kinds, statements that cannot be
/// isolated, and unavailable or failed plans return None independently of other findings.
pub async fn estimate_rows(
    session: &mut dyn Session,
    batch: &str,
    check: &DangerCheck,
    engine: Engine,
) -> Vec<Option<f64>> {
    let mut estimates = Vec::with_capacity(check.statements.len());
    estimate_each(session, batch, check, engine, &mut |_, estimate| {
        estimates.push(estimate);
    })
    .await;
    estimates
}

/// Reports each finding's estimate once, in order, as soon as it is resolved.
/// Indices refer to `check.statements`; estimates follow `estimate_rows`' rules.
/// Dropping the future cancels the current plan, preserves the caller's transaction,
/// and leaves estimates already delivered to the callback available to the caller.
/// The callback is `Send` so the future can run on a multi-threaded runtime.
pub async fn estimate_each(
    session: &mut dyn Session,
    batch: &str,
    check: &DangerCheck,
    engine: Engine,
    on_estimate: &mut (dyn FnMut(usize, Option<f64>) + Send),
) {
    let statements = candidates(batch, engine);
    let mut next = 0;
    for (finding_index, finding) in check.statements.iter().enumerate() {
        let found = statements
            .iter()
            .enumerate()
            .skip(next)
            .find(|(_, statement)| {
                statement.kind == finding.kind && statement.line == finding.line
            });
        let Some((index, statement)) = found else {
            on_estimate(finding_index, None);
            continue;
        };
        next = index + 1;
        let Some(sql) = statement.sql else {
            on_estimate(finding_index, None);
            continue;
        };
        let mut run = session.plan(sql, PlanMode::Estimated);
        let mut estimate = None;
        let mut failed = false;
        while let Some(event) = run.next().await {
            match event {
                Event::Plan { payload, format } => {
                    estimate = rows(&payload, format);
                }
                Event::Error { .. } => failed = true,
                _ => {}
            }
        }
        on_estimate(finding_index, if failed { None } else { estimate });
    }
}

fn rows(payload: &str, format: PlanFormat) -> Option<f64> {
    let plan = crate::plan::parse(payload, format).ok()?;
    let mut root = &plan.statements.first()?.root;
    if format == PlanFormat::ExplainJson && root.name == "ModifyTable" {
        root = root.children.iter().find(|child| {
            child.properties.iter().any(|property| {
                property.name == "Parent Relationship" && property.value == "\"Outer\""
            })
        })?;
    }
    if format == PlanFormat::MySqlJson {
        while !root.properties.iter().any(|property| {
            matches!(
                property.name.as_str(),
                "estimated_rows" | "rows_examined_per_scan"
            )
        }) {
            if root.children.len() != 1 {
                return None;
            }
            root = &root.children[0];
        }
    }
    if format == PlanFormat::ShowplanXml
        && root.children.is_empty()
        && !root
            .properties
            .iter()
            .any(|property| property.name.ends_with("/@StatementEstRows"))
    {
        return None;
    }
    Some(root.estimated_rows)
}

fn candidates(batch: &str, engine: Engine) -> Vec<Candidate<'_>> {
    let batches = if engine == Engine::SqlServer {
        let Ok(batches) = crate::mssql::batches::split(batch) else {
            return Vec::new();
        };
        batches
    } else {
        vec![crate::mssql::batches::Batch {
            sql: batch,
            line_offset: 0,
            repeat: 1,
        }]
    };
    let dialect: &dyn Dialect = match engine {
        Engine::SqlServer => &MsSqlDialect {},
        Engine::PostgreSql => &PostgreSqlDialect {},
        Engine::MySql => &MySqlDialect {},
    };
    let mut candidates = Vec::new();
    for batch in batches {
        let Ok(mut parser) = Parser::new(dialect).try_with_sql(batch.sql) else {
            continue;
        };
        loop {
            while parser.consume_token(&Token::SemiColon) {}
            let start = parser.peek_token();
            if start.token == Token::EOF {
                break;
            }
            let Ok(statement) = parser.parse_statement() else {
                break;
            };
            let end = parser.peek_token();
            if engine != Engine::SqlServer && !matches!(end.token, Token::SemiColon | Token::EOF) {
                break;
            }
            let change = match &statement {
                Statement::Query(query) => match query.body.as_ref() {
                    SetExpr::Update(inner) | SetExpr::Delete(inner) => inner,
                    _ => &statement,
                },
                _ => &statement,
            };
            let kind = match change {
                Statement::Update(update)
                    if super::selection_reason(update.selection.as_ref(), "UPDATE").is_some() =>
                {
                    Some(DangerKind::Update)
                }
                Statement::Delete(delete)
                    if super::selection_reason(delete.selection.as_ref(), "DELETE").is_some() =>
                {
                    Some(DangerKind::Delete)
                }
                _ => None,
            };
            let sql = &batch.sql
                [byte_offset(batch.sql, start.span.start)..byte_offset(batch.sql, end.span.start)];
            let checked = super::check(sql, engine);
            let isolated = checked.statements.len() == 1
                && checked
                    .statements
                    .first()
                    .is_some_and(|finding| Some(finding.kind) == kind);
            for finding in checked.statements {
                candidates.push(Candidate {
                    kind: finding.kind,
                    line: finding.line + start.span.start.line - 1 + batch.line_offset,
                    sql: isolated.then_some(sql),
                });
            }
        }
    }
    candidates
}

fn byte_offset(sql: &str, location: Location) -> usize {
    let mut line = 1;
    let mut column = 1;
    for (offset, ch) in sql.char_indices() {
        if line == location.line && column == location.column {
            return offset;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    sql.len()
}
