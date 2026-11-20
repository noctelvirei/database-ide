//! Keyword checks for batches whose syntax the parser does not understand.

use sqlparser::dialect::Dialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, TokenWithSpan};

use super::{DangerKind, DangerousStatement, selection_reason};

/// Finds keyword rules outside literals, preserving the original batch's token line numbers.
pub(super) fn check(tokens: &[TokenWithSpan], dialect: &dyn Dialect) -> Vec<DangerousStatement> {
    let mut tokens = tokens.to_vec();
    for token in &mut tokens {
        if let Token::Word(word) = &mut token.token {
            word.value.make_ascii_uppercase();
        }
    }
    let tokens = tokens.as_slice();
    let mut starts = Vec::new();
    let mut groups = Vec::new();
    let mut ranges = Vec::new();
    let mut depth = 0_usize;
    let mut command = "";
    let mut command_start = 0;
    for (index, token) in tokens.iter().enumerate() {
        match token.token {
            Token::LParen => {
                depth += 1;
                groups.push(index + 1);
            }
            Token::RParen => {
                depth = depth.saturating_sub(1);
                if let Some(start) = groups.pop() {
                    ranges.push((start, index));
                }
            }
            Token::SemiColon => {
                command = "";
                depth = 0;
                starts.push(index);
            }
            _ if depth == 0 => {
                let word = word(token);
                let previous = index
                    .checked_sub(1)
                    .map(|i| word_at(tokens, i))
                    .unwrap_or("");
                let is_action = matches!(word, "UPDATE" | "DELETE")
                    && (matches!(previous, "THEN" | "KEY" | "FOR") || command == "MERGE");
                let is_alter_drop = word == "DROP"
                    && command == "ALTER"
                    && word_at(tokens, command_start + 1) == "TABLE"
                    && !matches!(
                        word_at(tokens, index + 1),
                        "TABLE"
                            | "VIEW"
                            | "DATABASE"
                            | "SCHEMA"
                            | "FUNCTION"
                            | "PROCEDURE"
                            | "TRIGGER"
                            | "SEQUENCE"
                            | "TYPE"
                            | "USER"
                            | "ROLE"
                    );
                if !is_action
                    && !is_alter_drop
                    && matches!(
                        word,
                        "UPDATE"
                            | "DELETE"
                            | "DROP"
                            | "TRUNCATE"
                            | "ALTER"
                            | "MERGE"
                            | "RESTORE"
                            | "DBCC"
                            | "SHUTDOWN"
                            | "SELECT"
                            | "INSERT"
                            | "CREATE"
                            | "EXEC"
                            | "EXECUTE"
                            | "PRINT"
                    )
                {
                    starts.push(index);
                    command = word;
                    command_start = index;
                }
            }
            _ => {}
        }
    }
    starts.push(tokens.len());
    ranges.extend(groups.into_iter().map(|start| (start, tokens.len())));
    ranges.extend(starts.windows(2).map(|range| (range[0], range[1])));
    ranges.sort_unstable_by_key(|range| range.0);
    let mut findings = Vec::new();
    for (start, end) in ranges {
        let statement = &tokens[start..end];
        if let Some((kind, reason)) = statement_kind(statement, dialect) {
            findings.push(DangerousStatement {
                kind,
                reason,
                line: statement[0].span.start.line,
            });
        }
    }
    findings
}

fn statement_kind(
    tokens: &[TokenWithSpan],
    dialect: &dyn Dialect,
) -> Option<(DangerKind, &'static str)> {
    match word_at(tokens, 0) {
        operation @ ("UPDATE" | "DELETE") => {
            let mut depth = 0_usize;
            let where_index = tokens.iter().position(|token| {
                match token.token {
                    Token::LParen => depth += 1,
                    Token::RParen => depth = depth.saturating_sub(1),
                    _ => {}
                }
                depth == 0 && word(token) == "WHERE"
            });
            let reason = if let Some(index) = where_index {
                let mut parser =
                    Parser::new(dialect).with_tokens_with_locations(tokens[index + 1..].to_vec());
                let expression = parser.parse_expr().ok()?;
                if !matches!(parser.peek_token().token, Token::EOF | Token::SemiColon)
                    && !matches!(
                        word(&parser.peek_token()),
                        "RETURNING" | "OUTPUT" | "ORDER" | "LIMIT"
                    )
                {
                    return None;
                }
                selection_reason(Some(&expression), operation)?
            } else {
                selection_reason(None, operation)?
            };
            Some((
                if operation == "UPDATE" {
                    DangerKind::Update
                } else {
                    DangerKind::Delete
                },
                reason,
            ))
        }
        "DROP" => Some((DangerKind::Drop, "DROP removes an object")),
        "TRUNCATE" => Some((DangerKind::Truncate, "TRUNCATE removes all rows")),
        "ALTER"
            if word_at(tokens, 1) == "TABLE"
                && tokens.windows(2).any(|window| {
                    word(&window[0]) == "DROP"
                        && !matches!(
                            word(&window[1]),
                            "CONSTRAINT"
                                | "INDEX"
                                | "KEY"
                                | "PRIMARY"
                                | "FOREIGN"
                                | "PARTITION"
                                | "CHECK"
                        )
                }) =>
        {
            Some((DangerKind::DropColumn, "ALTER TABLE drops a column"))
        }
        "MERGE"
            if tokens.windows(5).enumerate().any(|(index, window)| {
                words_match(window, &["WHEN", "NOT", "MATCHED", "BY", "SOURCE"])
                    && contains(
                        tokens[index + 5..]
                            .split(|token| word(token) == "WHEN")
                            .next()
                            .unwrap_or(&[]),
                        &["THEN", "DELETE"],
                    )
            }) =>
        {
            Some((
                DangerKind::MergeDeleteBySource,
                "MERGE deletes rows not matched by source",
            ))
        }
        "RESTORE" if word_at(tokens, 1) == "DATABASE" => Some((
            DangerKind::RestoreDatabase,
            "RESTORE DATABASE overwrites a database",
        )),
        "ALTER" if word_at(tokens, 1) == "DATABASE" => {
            if contains(tokens, &["SET", "OFFLINE"]) {
                Some((DangerKind::AlterDatabase, "ALTER DATABASE sets OFFLINE"))
            } else if contains(tokens, &["SET", "SINGLE_USER"]) {
                Some((DangerKind::AlterDatabase, "ALTER DATABASE sets SINGLE_USER"))
            } else if contains(tokens, &["SET", "EMERGENCY"]) {
                Some((DangerKind::AlterDatabase, "ALTER DATABASE sets EMERGENCY"))
            } else {
                None
            }
        }
        "DBCC"
            if tokens
                .iter()
                .any(|token| word(token) == "REPAIR_ALLOW_DATA_LOSS") =>
        {
            Some((
                DangerKind::RepairAllowDataLoss,
                "DBCC repair allows data loss",
            ))
        }
        "SHUTDOWN" => Some((DangerKind::Shutdown, "SHUTDOWN stops the server")),
        _ => None,
    }
}

fn contains(tokens: &[TokenWithSpan], words: &[&str]) -> bool {
    tokens
        .windows(words.len())
        .any(|window| words_match(window, words))
}

fn words_match(tokens: &[TokenWithSpan], words: &[&str]) -> bool {
    tokens
        .iter()
        .zip(words)
        .all(|(token, expected)| word(token) == *expected)
}

fn word_at(tokens: &[TokenWithSpan], index: usize) -> &str {
    tokens.get(index).map(word).unwrap_or("")
}

fn word(token: &TokenWithSpan) -> &str {
    match &token.token {
        Token::Word(word) if word.quote_style.is_none() => &word.value,
        _ => "",
    }
}
