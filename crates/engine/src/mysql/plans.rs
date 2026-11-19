//! MySQL iterator plans, isolated from user transactions and rolled back before reuse.

use mysql_async::{Conn, consts::StatusFlags, prelude::Queryable};
use sqlparser::{
    ast::Statement,
    dialect::MySqlDialect,
    parser::{Parser, ParserError},
    tokenizer::{Location, Token, Tokenizer},
};

use super::{Request, diagnostics, execution::send};
use crate::session::{Event, PlanFormat, PlanMode};

struct ParsedStatement<'a> {
    statement: Statement,
    sql: &'a str,
}

fn statements(batch: &str) -> Result<Vec<ParsedStatement<'_>>, ParserError> {
    let dialect = MySqlDialect {};
    let tokens = Tokenizer::new(&dialect, batch)
        .tokenize_with_location()
        .map_err(|error| ParserError::TokenizerError(error.to_string()))?;
    let mut parser = Parser::new(&dialect).with_tokens_with_locations(tokens);
    let mut statements = Vec::new();
    loop {
        while parser.consume_token(&Token::SemiColon) {}
        let start = parser.peek_token();
        if start.token == Token::EOF {
            break;
        }
        let statement = parser.parse_statement()?;
        let end = parser.peek_token();
        if end.token != Token::SemiColon && end.token != Token::EOF {
            return Err(ParserError::ParserError(
                "expected a statement delimiter".into(),
            ));
        }
        statements.push(ParsedStatement {
            statement,
            sql: &batch[byte_offset(batch, start.span.start)..byte_offset(batch, end.span.start)],
        });
    }
    Ok(statements)
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

/// Runs one EXPLAIN per statement; cleanup failure discards the session connection.
pub(super) async fn run(conn: &mut Conn, request: &Request, mode: PlanMode) -> Result<(), ()> {
    let statements = match statements(&request.batch) {
        Ok(statements) => statements,
        Err(error) => {
            refuse(request, &error.to_string()).await;
            return Ok(());
        }
    };
    // Allowlisting also rejects transaction control, CALL and implicit-commit administrative SQL.
    if mode == PlanMode::Actual
        && statements.iter().any(|statement| {
            !matches!(
                statement.statement,
                Statement::Query(_)
                    | Statement::Update(_)
                    | Statement::Delete(_)
                    | Statement::Insert(_)
            )
        })
    {
        refuse(
            request,
            "MySQL actual plans refuse DDL and other statements that cannot be safely rolled back",
        )
        .await;
        return Ok(());
    }
    let password = conn.opts().pass().map(str::to_owned);
    if let Err(error) = conn.query_drop("DO 0").await {
        send(
            request,
            diagnostics::error(&error, None, password.as_deref()),
        )
        .await;
        return if error.is_fatal() { Err(()) } else { Ok(()) };
    }
    if mode == PlanMode::Actual
        && conn.last_ok_packet().is_none_or(|packet| {
            let status = packet.status_flags();
            status.contains(StatusFlags::SERVER_STATUS_IN_TRANS)
                || !status.contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT)
        })
    {
        refuse(
            request,
            "MySQL actual plans require no active transaction and autocommit enabled",
        )
        .await;
        return Ok(());
    }
    // Version 1 cannot represent iterator runtime counters; version 2 supports both modes.
    let previous: Option<u8> = match conn
        .query_first("SELECT @@session.explain_json_format_version")
        .await
    {
        Ok(version) => version,
        Err(error) => {
            send(
                request,
                diagnostics::error(&error, None, password.as_deref()),
            )
            .await;
            return if error.is_fatal() { Err(()) } else { Ok(()) };
        }
    };
    let result = explain(conn, request, mode, &statements).await;
    // Restore session settings even on cancellation or an EXPLAIN error.
    let restore = match previous {
        Some(version) => {
            conn.query_drop(format!("SET SESSION explain_json_format_version={version}"))
                .await
        }
        None => {
            refuse(request, "MySQL did not report explain_json_format_version").await;
            return Err(());
        }
    };
    if let Err(error) = restore {
        send(
            request,
            diagnostics::error(&error, None, password.as_deref()),
        )
        .await;
        return Err(());
    }
    result
}

async fn explain(
    conn: &mut Conn,
    request: &Request,
    mode: PlanMode,
    statements: &[ParsedStatement<'_>],
) -> Result<(), ()> {
    let password = conn.opts().pass().map(str::to_owned);
    if let Err(error) = conn
        .query_drop("SET SESSION explain_json_format_version=2")
        .await
    {
        send(
            request,
            diagnostics::error(&error, None, password.as_deref()),
        )
        .await;
        return if error.is_fatal() { Err(()) } else { Ok(()) };
    }
    for (index, statement) in statements.iter().enumerate() {
        if request.cancel.is_cancelled() || request.stopped.is_cancelled() {
            break;
        }
        let actual = mode == PlanMode::Actual;
        if actual && let Err(error) = conn.query_drop("START TRANSACTION").await {
            send(
                request,
                diagnostics::error(&error, None, password.as_deref()),
            )
            .await;
            return if error.is_fatal() { Err(()) } else { Ok(()) };
        }
        let analyze = if actual { "ANALYZE " } else { "" };
        // Server-side preparation enforces one statement even when sql_mode changes quoting.
        let payload = conn
            .exec_first::<String, _, _>(
                format!("EXPLAIN {analyze}FORMAT=JSON {}", statement.sql),
                (),
            )
            .await;
        // The worker keeps this future alive after KILL QUERY or a dropped event consumer.
        if actual && let Err(error) = conn.query_drop("ROLLBACK").await {
            send(
                request,
                diagnostics::error(&error, None, password.as_deref()),
            )
            .await;
            return Err(());
        }
        match payload {
            Ok(Some(payload)) => {
                send(
                    request,
                    Event::Plan {
                        payload,
                        format: PlanFormat::MySqlJson,
                    },
                )
                .await
            }
            Ok(None) => {
                refuse(request, "MySQL returned no execution plan").await;
                break;
            }
            Err(error) => {
                send(
                    request,
                    diagnostics::error(
                        &error,
                        request.database_changes.statements.get(index),
                        password.as_deref(),
                    ),
                )
                .await;
                return if error.is_fatal() { Err(()) } else { Ok(()) };
            }
        }
    }
    Ok(())
}

async fn refuse(request: &Request, text: &str) {
    send(
        request,
        Event::Error {
            text: text.into(),
            line: None,
            code: None,
        },
    )
    .await;
}
