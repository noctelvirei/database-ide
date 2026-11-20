//! Finds statements that can destroy data or take a database away, without running SQL.

use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, BinaryOperator, Expr, MergeAction, MergeClauseKind, Spanned, Statement,
    Value, Visit, Visitor,
};
use sqlparser::dialect::{Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

pub mod estimates;
mod fallback;
mod literals;

/// The database product whose SQL syntax the batch uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Microsoft SQL Server, using T-SQL syntax.
    SqlServer,
    /// PostgreSQL, including its quoted and dollar-quoted literals.
    PostgreSql,
    /// MySQL, including backtick-quoted identifiers.
    MySql,
}

/// The operation for which the UI should ask for confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerKind {
    /// An UPDATE with no WHERE or a trivially true WHERE.
    Update,
    /// A DELETE with no WHERE or a trivially true WHERE.
    Delete,
    /// A DROP of any object.
    Drop,
    /// A TRUNCATE of a table.
    Truncate,
    /// An ALTER TABLE that drops a column.
    DropColumn,
    /// A MERGE that deletes rows not matched by the source.
    MergeDeleteBySource,
    /// A RESTORE DATABASE operation.
    RestoreDatabase,
    /// An ALTER DATABASE that sets OFFLINE, SINGLE_USER or EMERGENCY.
    AlterDatabase,
    /// A DBCC repair that permits data loss.
    RepairAllowDataLoss,
    /// A SHUTDOWN of the server.
    Shutdown,
}

/// One dangerous statement; findings are returned in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerousStatement {
    /// The operation requiring confirmation.
    pub kind: DangerKind,
    /// A plain explanation suitable for display without formatting SQL.
    pub reason: &'static str,
    /// The statement's starting line in the input text, counted from one.
    pub line: u64,
}

/// Findings for one batch, including whether its syntax was fully understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerCheck {
    /// Every detected dangerous statement, in source order, once per statement.
    pub statements: Vec<DangerousStatement>,
    /// False means keyword fallback was needed; an empty list then does not establish safety.
    pub fully_checked: bool,
}

/// Checks SQL without server access, splitting SQL Server GO batches and reporting input lines.
/// Syntax, tokenization and GO separator failures produce a normal result with
/// `fully_checked = false` and any findings available from keyword fallback.
/// Does not estimate row counts or apply connection settings; GO repeats do not duplicate findings.
pub fn check(batch: &str, engine: Engine) -> DangerCheck {
    if engine != Engine::SqlServer {
        return check_batch(batch, engine);
    }
    let batches = match crate::mssql::batches::split(batch) {
        Ok(batches) => batches,
        Err(_) => {
            let mut result = check_batch(batch, engine);
            result.fully_checked = false;
            return result;
        }
    };
    let mut result = DangerCheck {
        statements: Vec::new(),
        fully_checked: true,
    };
    for batch in batches {
        let checked = check_batch(batch.sql, engine);
        result.fully_checked &= checked.fully_checked;
        result
            .statements
            .extend(checked.statements.into_iter().map(|mut statement| {
                statement.line += batch.line_offset;
                statement
            }));
    }
    result
}

fn check_batch(batch: &str, engine: Engine) -> DangerCheck {
    let dialect: &dyn Dialect = match engine {
        Engine::SqlServer => &MsSqlDialect {},
        Engine::PostgreSql => &PostgreSqlDialect {},
        Engine::MySql => &MySqlDialect {},
    };
    let mut tokens = Vec::new();
    let tokenized = Tokenizer::new(dialect, batch)
        .tokenize_with_location_into_buf(&mut tokens)
        .is_ok();
    tokens.retain(|token| !matches!(token.token, Token::Whitespace(_)));
    let parsed = tokenized
        .then(|| {
            Parser::new(dialect)
                .with_tokens_with_locations(tokens.clone())
                .parse_statements()
        })
        .and_then(Result::ok);
    if let Some(statements) = parsed {
        let mut findings = Findings {
            statements: Vec::new(),
            line: 1,
            tokens: &tokens,
        };
        let mut positions = Parser::new(dialect).with_tokens_with_locations(tokens.clone());
        for statement in statements {
            while positions.consume_token(&Token::SemiColon) {}
            findings.line = positions.peek_token().span.start.line;
            let _ = positions.parse_statement();
            let _ = statement.visit(&mut findings);
        }
        findings.statements.sort_by_key(|statement| statement.line);
        DangerCheck {
            statements: findings.statements,
            fully_checked: true,
        }
    } else {
        DangerCheck {
            statements: fallback::check(&tokens, dialect),
            fully_checked: false,
        }
    }
}

struct Findings<'a> {
    statements: Vec<DangerousStatement>,
    line: u64,
    tokens: &'a [TokenWithSpan],
}

impl Visitor for Findings<'_> {
    type Break = ();

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        let mut line = self.line;
        let finding = match statement {
            Statement::Update(update) => {
                line = update.update_token.0.span.start.line;
                selection_reason(update.selection.as_ref(), "UPDATE")
                    .map(|reason| (DangerKind::Update, reason))
            }
            Statement::Delete(delete) => {
                line = delete.delete_token.0.span.start.line;
                selection_reason(delete.selection.as_ref(), "DELETE")
                    .map(|reason| (DangerKind::Delete, reason))
            }
            Statement::Drop { .. }
            | Statement::DropFunction(_)
            | Statement::DropDomain(_)
            | Statement::DropProcedure { .. }
            | Statement::DropSecret { .. }
            | Statement::DropPolicy(_)
            | Statement::DropRule(_)
            | Statement::DropConnector { .. }
            | Statement::DropExtension(_)
            | Statement::DropOperator(_)
            | Statement::DropOperatorFamily(_)
            | Statement::DropOperatorClass(_)
            | Statement::DropTrigger(_) => Some((DangerKind::Drop, "DROP removes an object")),
            Statement::Truncate(_) => Some((DangerKind::Truncate, "TRUNCATE removes all rows")),
            Statement::AlterTable(alter)
                if alter.operations.iter().any(|operation| {
                    matches!(operation, AlterTableOperation::DropColumn { .. })
                }) =>
            {
                Some((DangerKind::DropColumn, "ALTER TABLE drops a column"))
            }
            Statement::Merge(merge)
                if merge.clauses.iter().any(|clause| {
                    clause.clause_kind == MergeClauseKind::NotMatchedBySource
                        && matches!(clause.action, MergeAction::Delete { .. })
                }) =>
            {
                line = merge.merge_token.0.span.start.line;
                Some((
                    DangerKind::MergeDeleteBySource,
                    "MERGE deletes rows not matched by source",
                ))
            }
            _ => None,
        };
        if let Some((kind, reason)) = finding {
            let keyword = match kind {
                DangerKind::Drop => Some("DROP"),
                DangerKind::Truncate => Some("TRUNCATE"),
                DangerKind::DropColumn => Some("ALTER"),
                _ => None,
            };
            if let Some(keyword) = keyword {
                line = self.keyword_line(statement, keyword);
            }
            self.statements
                .push(DangerousStatement { kind, reason, line });
        }
        ControlFlow::Continue(())
    }
}

impl Findings<'_> {
    fn keyword_line(&self, statement: &Statement, keyword: &str) -> u64 {
        let anchor = match statement {
            Statement::Drop { names, .. } => names.first().map(Spanned::span),
            Statement::DropFunction(drop) => {
                drop.func_desc.first().map(|function| function.name.span())
            }
            Statement::DropExtension(drop) => drop.names.first().map(|name| name.span),
            Statement::DropOperator(drop) => {
                drop.operators.first().map(|operator| operator.name.span())
            }
            Statement::DropOperatorFamily(drop) => drop.names.first().map(Spanned::span),
            Statement::DropOperatorClass(drop) => drop.names.first().map(Spanned::span),
            Statement::DropDomain(drop) => Some(drop.name.span()),
            Statement::DropProcedure { proc_desc, .. } => {
                proc_desc.first().map(|function| function.name.span())
            }
            Statement::DropSecret { name, .. } | Statement::DropConnector { name, .. } => {
                Some(name.span)
            }
            Statement::DropPolicy(drop) => Some(drop.name.span),
            Statement::DropTrigger(drop) => Some(drop.trigger_name.span()),
            _ => Some(statement.span()),
        };
        anchor.and_then(|anchor| self.tokens.iter().rev().find(|token| {
            token.span.start < anchor.start && matches!(&token.token,
                Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case(keyword))
        })).map(|token| token.span.start.line).unwrap_or(self.line)
    }
}

fn selection_reason(selection: Option<&Expr>, operation: &str) -> Option<&'static str> {
    match (operation, selection) {
        ("UPDATE", None) => Some("UPDATE with no WHERE clause"),
        ("DELETE", None) => Some("DELETE with no WHERE clause"),
        ("UPDATE", Some(expr)) if trivially_true(expr) => {
            Some("UPDATE with a trivially true WHERE clause")
        }
        ("DELETE", Some(expr)) if trivially_true(expr) => {
            Some("DELETE with a trivially true WHERE clause")
        }
        _ => None,
    }
}

fn trivially_true(expr: &Expr) -> bool {
    match expr {
        Expr::Nested(expr) => trivially_true(expr),
        Expr::Value(value) => value.value == Value::Boolean(true),
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } => literals::equal(left, right),
        _ => false,
    }
}
