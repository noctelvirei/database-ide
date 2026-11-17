//! Server error locations and statement classification without modifying submitted SQL.

use std::num::NonZeroU64;

use sqlparser::ast::{SetExpr, Statement as SqlStatement, Use};
use sqlparser::dialect::MySqlDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Location, Token, Tokenizer};

use crate::session::Event;

/// Best-effort positions; unrecognized syntax never prevents execution on the server.
pub(super) struct Statement {
    line: u64,
    pub(super) changes_rows: bool,
    pub(super) multiple_results: bool,
    pub(super) database: Option<String>,
}

/// Parses only for diagnostic offsets and command counts, never to send or rewrite SQL.
pub(super) fn statements(batch: &str) -> Vec<Statement> {
    let mut tokens = Vec::new();
    let _ = Tokenizer::new(&MySqlDialect {}, batch).tokenize_with_location_into_buf(&mut tokens);
    let mut parser = Parser::new(&MySqlDialect {}).with_tokens_with_locations(tokens);
    let mut result = Vec::new();
    let mut line = 1;
    loop {
        // Empty statements do not produce a separate server result.
        while parser.peek_token().token == Token::SemiColon {
            let end = parser.next_token().span.end;
            line = next_line(batch, end);
        }
        let token = parser.peek_token();
        if token.token == Token::EOF {
            break;
        }
        let changes_rows = matches!(&token.token, Token::Word(word)
            if ["INSERT", "UPDATE", "DELETE", "REPLACE", "LOAD"].iter()
                .any(|keyword| word.value.eq_ignore_ascii_case(keyword)));
        let multiple_results =
            matches!(&token.token, Token::Word(word) if word.value.eq_ignore_ascii_case("CALL"));
        let parsed = parser.parse_statement();
        let changes_rows = changes_rows
            || matches!(
                &parsed,
                Ok(SqlStatement::Update(_) | SqlStatement::Delete(_) | SqlStatement::Insert(_))
            )
            || matches!(&parsed, Ok(SqlStatement::Query(query))
                if matches!(query.body.as_ref(), SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Insert(_)));
        let database = match &parsed {
            Ok(SqlStatement::Use(Use::Object(name))) if name.0.len() == 1 => {
                name.0[0].as_ident().map(|name| name.value.clone())
            }
            _ => None,
        };
        result.push(Statement {
            line,
            changes_rows,
            multiple_results,
            database,
        });
        if parsed.is_err() {
            break;
        }
        let end = parser.peek_token();
        if end.token != Token::SemiColon {
            break;
        }
        parser.next_token();
        line = next_line(batch, end.span.end);
    }
    result
}

fn next_line(batch: &str, location: Location) -> u64 {
    let mut line = 1;
    let mut column = 1;
    for (offset, ch) in batch.char_indices() {
        if line == location.line && column == location.column {
            // MySQL strips whitespace after a batch delimiter, but preserves comments.
            return line
                + batch[offset..]
                    .chars()
                    .take_while(char::is_ascii_whitespace)
                    .filter(|ch| *ch == '\n')
                    .count() as u64;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    line
}

/// Keeps the native error code and adds the server's relative line to a known statement start.
pub(super) fn error(
    error: &mysql_async::Error,
    statement: Option<&Statement>,
    password: Option<&str>,
) -> Event {
    match error {
        mysql_async::Error::Server(server) => Event::Error {
            text: redact(&server.message, password),
            line: line(&server.message, statement.map(|statement| statement.line)),
            code: Some(server.code.to_string()),
        },
        _ => Event::Error {
            text: redact(&error.to_string(), password),
            line: None,
            code: None,
        },
    }
}

fn line(message: &str, statement_line: Option<u64>) -> Option<NonZeroU64> {
    let relative = message.rsplit_once("at line ")?.1;
    let digits: String = relative.chars().take_while(char::is_ascii_digit).collect();
    let relative = digits.parse::<u64>().ok().filter(|line| *line != 0)?;
    NonZeroU64::new(statement_line?.checked_add(relative - 1)?)
}

/// Removes the supplied sign-in secret even if a server diagnostic unexpectedly echoes it.
pub(super) fn redact(message: &str, password: Option<&str>) -> String {
    match password.filter(|password| !password.is_empty()) {
        Some(password) => message.replace(password, "[redacted]"),
        None => message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_line_maps_the_second_statement_after_unicode_and_quoted_semicolons() {
        let statements = statements("SELECT 'é;elm';\n\nSELEC 2");
        assert_eq!(
            line("syntax near 'SELEC' at line 1", Some(statements[1].line)),
            NonZeroU64::new(3)
        );
    }

    #[test]
    fn error_line_counts_comments_and_relative_newlines() {
        let statements = statements("SELECT 1;\n/* semicolon ;\ncomment */ SELECT\nbroken");
        assert_eq!(
            line("syntax at line 3", Some(statements[1].line)),
            NonZeroU64::new(4)
        );
    }

    #[test]
    fn error_line_preserves_initial_blank_lines() {
        let statements = statements("\n\nSELEC 2");
        assert_eq!(
            line("syntax at line 3", Some(statements[0].line)),
            NonZeroU64::new(3)
        );
    }

    #[test]
    fn cte_updates_are_classified_as_row_changes() {
        let statements =
            statements("WITH ids AS (SELECT 1 AS id) UPDATE trees SET id = 2 WHERE false");
        assert!(statements[0].changes_rows);
    }

    #[test]
    fn use_retains_the_unquoted_database_name() {
        let statements = statements("USE `invented``trees`");
        assert_eq!(statements[0].database.as_deref(), Some("invented`trees"));
    }

    #[test]
    fn error_line_is_unknown_without_a_positive_server_line_or_statement_offset() {
        assert_eq!(line("Access denied", Some(1)), None);
        assert_eq!(line("syntax at line 0", Some(1)), None);
        assert_eq!(line("syntax at line 2", None), None);
        assert_eq!(line("syntax at line 2", Some(u64::MAX)), None);
    }
}
