//! Optimizer estimates for the UPDATE/DELETE statements identified by the danger check.

use futures::StreamExt;
use sqlparser::{
    ast::{SetExpr, Statement},
    dialect::MsSqlDialect,
    parser::Parser,
    tokenizer::Token,
};

use super::{Error, Mssql, batches};
use crate::danger::{DangerKind, DangerousStatement};
use crate::session::{Event, PlanMode, Session};

/// Matches findings to parsed statements in source order, never executing changes.
pub(super) async fn estimate_rows(
    session: &mut Mssql,
    script: &str,
    flagged: &[DangerousStatement],
) -> Result<Vec<Option<f64>>, Error> {
    let mut statements = Vec::new();
    for batch in batches::split(script).map_err(|error| Error::Parse(error.to_string()))? {
        let mut parser = Parser::new(&MsSqlDialect {})
            .try_with_sql(batch.sql)
            .map_err(|error| Error::Parse(error.to_string()))?;
        while parser.peek_token().token != Token::EOF {
            while parser.consume_token(&Token::SemiColon) {}
            if parser.peek_token().token == Token::EOF {
                break;
            }
            let mut line = parser.peek_token().span.start.line + batch.line_offset;
            let statement = parser
                .parse_statement()
                .map_err(|error| Error::Parse(error.to_string()))?;
            // `WITH … DELETE` parses as a query whose body is the data change.
            let change = match &statement {
                Statement::Query(query) => match query.body.as_ref() {
                    SetExpr::Update(inner) | SetExpr::Delete(inner) => inner,
                    _ => &statement,
                },
                _ => &statement,
            };
            let kind = match change {
                Statement::Update(update) => {
                    line = update.update_token.0.span.start.line + batch.line_offset;
                    Some(DangerKind::Update)
                }
                Statement::Delete(delete) => {
                    line = delete.delete_token.0.span.start.line + batch.line_offset;
                    Some(DangerKind::Delete)
                }
                _ => None,
            };
            // Only a statement the danger check flags may take a finding: a safe
            // `DELETE … WHERE` on the same line must not lend its estimate.
            let sql = statement.to_string();
            let kind = kind.filter(|kind| {
                crate::danger::check(&sql, crate::danger::Engine::SqlServer)
                    .statements
                    .iter()
                    .any(|finding| finding.kind == *kind)
            });
            statements.push((kind, line, sql));
        }
    }
    let mut estimates = Vec::with_capacity(flagged.len());
    let mut next = 0;
    for finding in flagged {
        if !matches!(finding.kind, DangerKind::Update | DangerKind::Delete) {
            estimates.push(None);
            continue;
        }
        let found = statements
            .iter()
            .enumerate()
            .skip(next)
            .find(|(_, (kind, line, _))| *kind == Some(finding.kind) && *line == finding.line);
        let Some((index, (_, _, sql))) = found else {
            estimates.push(None);
            continue;
        };
        next = index + 1;
        let mut run = session.plan(sql, PlanMode::Estimated);
        let mut estimate = None;
        while let Some(event) = run.next().await {
            match event {
                Event::Plan { payload, format } => {
                    let plan = crate::plan::parse(&payload, format)
                        .map_err(|error| Error::Parse(error.to_string()))?;
                    estimate = plan
                        .statements
                        .first()
                        .map(|statement| statement.root.estimated_rows);
                }
                Event::Error { text, .. } => return Err(Error::Server(text)),
                _ => {}
            }
        }
        estimates.push(estimate);
    }
    Ok(estimates)
}
