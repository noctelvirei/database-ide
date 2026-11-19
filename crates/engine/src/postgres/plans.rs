//! Plan isolation without changing the caller's transaction.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Location, Token};
use tokio::sync::mpsc;
use tokio_postgres::Client;

use super::{Error, Notice, Request, cancel::Cancellation, execution, idle_query};
use crate::session::{Event, PlanMode};

struct ParsedStatement<'a> {
    sql: &'a str,
    line: u64,
}

fn statements(batch: &str) -> Result<Vec<ParsedStatement<'_>>, Error> {
    let parse_error = |error: sqlparser::parser::ParserError| Error::Parse(error.to_string());
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, batch).map_err(parse_error)?;
    let mut positions = Parser::new(&PostgreSqlDialect {})
        .try_with_sql(batch)
        .map_err(parse_error)?;
    let mut result = Vec::with_capacity(statements.len());
    for _ in statements {
        while positions.consume_token(&Token::SemiColon) {}
        let start = positions.peek_token().span.start;
        positions.parse_statement().map_err(parse_error)?;
        let end = positions.peek_token();
        let end = if end.token == Token::EOF {
            batch.len()
        } else {
            byte_offset(batch, end.span.start)
        };
        result.push(ParsedStatement {
            sql: &batch[byte_offset(batch, start)..end],
            line: start.line,
        });
    }
    Ok(result)
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

/// Produces isolated statement plans; Err requires terminating this session's transport.
pub(super) async fn run(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    cancellation: &Cancellation,
    request: &Request,
    mode: PlanMode,
) -> Result<(), ()> {
    if let Err(error) = run_inner(client, notices, cancellation, request, mode).await {
        execution::send(
            request,
            Event::Error {
                text: error.to_string(),
                line: None,
                code: None,
            },
        )
        .await;
        if matches!(error, Error::CancelFailed) {
            return Err(());
        }
    }
    if client.is_closed() { Err(()) } else { Ok(()) }
}

async fn run_inner(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    cancellation: &Cancellation,
    request: &Request,
    mode: PlanMode,
) -> Result<(), Error> {
    let statements = statements(&request.batch)?;
    if mode == PlanMode::Actual {
        // SAVEPOINT fails harmlessly outside a transaction; never BEGIN inside a user's one.
        let guard = format!("dbide_plan_{:016x}", rand::random::<u64>());
        match idle_query(client, notices, &format!("SAVEPOINT {guard}")).await {
            Ok(()) => {
                idle_query(client, notices, &format!("RELEASE SAVEPOINT {guard}")).await?;
                return Err(Error::ActiveTransaction);
            }
            Err(error) if error.code().is_some_and(|code| code.code() == "25P01") => {}
            Err(_) => return Err(Error::ActiveTransaction),
        }
    }
    for statement in statements {
        if request.cancel.is_cancelled() {
            break;
        }
        let actual = mode == PlanMode::Actual;
        let guard = format!("dbide_plan_{:016x}", rand::random::<u64>());
        let savepoint = if actual {
            false
        } else {
            match idle_query(client, notices, &format!("SAVEPOINT {guard}")).await {
                Ok(()) => true,
                Err(error) if error.code().is_some_and(|code| code.code() == "25P01") => false,
                Err(error) => return Err(error.into()),
            }
        };
        if actual {
            idle_query(client, notices, "BEGIN").await?;
        }
        let options = if actual {
            "ANALYZE, BUFFERS, FORMAT JSON"
        } else {
            "FORMAT JSON"
        };
        // Preserve literal spelling and batch line numbers rather than reformatting the AST.
        let sql = format!(
            "EXPLAIN ({options}) {}{}",
            "\n".repeat(statement.line.saturating_sub(1) as usize),
            statement.sql
        );
        let success = execution::run(client, notices, cancellation, request, &sql, None)
            .await
            .map_err(|()| Error::CancelFailed)?;
        // Roll back on errors and cancellation too, independently of the event consumer.
        if actual {
            idle_query(client, notices, "ROLLBACK").await?;
        } else if savepoint {
            if !success {
                idle_query(client, notices, &format!("ROLLBACK TO SAVEPOINT {guard}")).await?;
            }
            idle_query(client, notices, &format!("RELEASE SAVEPOINT {guard}")).await?;
        }
        if !success {
            break;
        }
    }
    Ok(())
}
