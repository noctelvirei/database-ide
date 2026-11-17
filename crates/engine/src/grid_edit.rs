//! Editable results, pending changes and optimistic Save scripts.

use std::collections::BTreeSet;

use sqlparser::ast::{SetExpr, Statement as SqlStatement, TableFactor};
use sqlparser::dialect::{Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;

use crate::danger::Engine;
use crate::session::{Column, Session, Value};
use crate::sql::{identifier, literal};

mod mssql;
mod mysql;
mod postgres;

/// Invalid pending changes, catalog failures and Save failures, ready for display.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The engine has no catalog lookup or Save runner yet.
    #[error("Grid editing is not supported yet for {0:?}")]
    Unsupported(Engine),
    /// Values or identifiers cannot be safely represented in SQL.
    #[error(transparent)]
    Sql(#[from] crate::sql::Error),
    /// The pending change does not match this result's columns or editing policy.
    #[error("Invalid pending change: {0}")]
    InvalidChange(String),
    /// A catalog response did not have its documented shape.
    #[error("Unexpected grid editing catalog response: {0}")]
    Catalog(String),
    /// The session failed, optionally while running a specific pending change.
    #[error("Grid Save: {text} (pending change {change:?})")]
    Server {
        /// Server or transport diagnostic.
        text: String,
        /// SQLSTATE, when the session supplied it.
        code: Option<String>,
        /// Zero-based pending change index; None for catalog or transaction commands.
        change: Option<usize>,
    },
    /// The caller dropped Save before completion; the worker rolls back its changes.
    #[error("Grid Save was cancelled")]
    Cancelled,
    /// Recovery failed; the session is discarded rather than reused in an unknown state.
    #[error("{original}; Save rollback failed: {recovery}")]
    Recovery {
        /// Failure that triggered recovery.
        original: Box<Error>,
        /// Diagnostic from rollback or savepoint release.
        recovery: String,
    },
}

/// Whether the grid can enter edit mode; refusal reasons are ready for the UI.
#[derive(Debug)]
pub enum Editability {
    /// The engine has identified a table and a complete non-null key.
    Editable(EditableResult),
    /// The grid stays read-only, displaying this reason.
    ReadOnly(String),
}

/// Editing policy for one result column, in result order.
#[derive(Debug, Clone)]
pub struct EditColumn {
    /// Unquoted base column; None for expressions and unknown provenance.
    pub name: Option<String>,
    /// Expressions, identity, generated and binary columns cannot be filled or edited.
    pub read_only: bool,
    /// Equality is available for optimistic predicates; false columns are omitted on DELETE.
    /// Updating these columns is refused because their originals cannot be checked safely.
    pub comparable: bool,
}

/// A catalog-verified result. Build scripts from the same result's original row values.
#[derive(Debug)]
pub struct EditableResult {
    engine: Engine,
    table: Vec<String>,
    columns: Vec<EditColumn>,
    key: Vec<usize>,
    comparison_types: Vec<Option<String>>,
    comparison_collations: Vec<Option<String>>,
}

/// One explicitly filled cell; omitted cells in an INSERT use server defaults.
#[derive(Debug, Clone)]
pub struct Cell {
    /// Zero-based result column index, never a label or an alias.
    pub column: usize,
    /// Explicit SQL NULL is distinct from leaving a cell blank.
    pub value: Value,
}

/// One pending change; original rows have exactly one value per result column.
#[derive(Debug, Clone)]
pub enum Change {
    /// Updates the listed cells and checks their originals as well as the original key.
    Update {
        /// The complete row as read, before any local edits.
        original: Vec<Value>,
        /// Nonempty, distinct result columns to update.
        cells: Vec<Cell>,
    },
    /// Inserts only explicitly filled cells; an empty list inserts default values.
    Insert {
        /// Distinct result columns filled by the user.
        cells: Vec<Cell>,
    },
    /// Deletes by the original key and all comparable original base columns.
    Delete {
        /// The complete row as read, before any local edits.
        original: Vec<Value>,
    },
}

/// The operation performed by a generated statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One existing row was edited.
    Update,
    /// One new row was added.
    Insert,
    /// One existing row was deleted.
    Delete,
}

/// One generated statement; its index matches the caller's pending change index.
#[derive(Debug, Clone)]
pub struct Statement {
    sql: String,
    kind: Kind,
}

impl Statement {
    /// SQL shown in the confirm and run unchanged, including its terminating semicolon.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Operation used to attribute the affected row count.
    pub fn kind(&self) -> Kind {
        self.kind
    }
}

/// An immutable confirm script plus statements, kept together so preview and execution agree.
#[derive(Debug, Clone)]
pub struct Script {
    pub(crate) engine: Engine,
    statements: Vec<Statement>,
    sql: String,
}

impl Script {
    /// The complete preview; each statement ends with a semicolon and newline.
    /// Transaction commands are managed by Save, not included in the preview.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Statements in pending-change order; Save checks exactly one affected row for each.
    pub fn statements(&self) -> &[Statement] {
        &self.statements
    }
}

/// Row counts retained by a successful Save; all are zero after an edit conflict.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    /// Rows updated.
    pub updated: u64,
    /// Rows inserted.
    pub inserted: u64,
    /// Rows deleted.
    pub deleted: u64,
}

/// The first pending change whose statement did not affect exactly one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Zero-based index in the supplied pending changes.
    pub change: usize,
    /// Actual affected count; Save has already rolled back these changes.
    pub affected: u64,
}

/// Outcome after commit or rollback; SQL failures instead return Error.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SaveReport {
    /// Changes retained, pending the user's commit if their transaction was open.
    pub rows: Counts,
    /// The first conflict, if any; the whole Save is rolled back before returning.
    pub conflict: Option<Conflict>,
    /// The user's transaction remains open, including after a conflict.
    pub user_transaction_open: bool,
}

/// Looks up keys and column policy on the tab's session. Expressions alongside base
/// columns stay read-only.
/// Supply the single statement that produced these columns; ambiguous sources are read-only.
pub async fn inspect(
    session: &mut dyn Session,
    engine: Engine,
    statement: &str,
    columns: &[Column],
) -> Result<Editability, Error> {
    if engine == Engine::SqlServer {
        if !single_table_query(statement, engine) {
            return Ok(Editability::ReadOnly("joins aren't editable".into()));
        }
        return mssql::inspect(session, statement, columns).await;
    }
    let mut tables = BTreeSet::new();
    for column in columns.iter().filter(|column| column.base_column.is_some()) {
        let Some(table) = &column.base_table else {
            return Ok(Editability::ReadOnly("unknown base table".into()));
        };
        tables.insert(table.as_str());
    }
    if tables.is_empty() {
        return Ok(Editability::ReadOnly("expression columns only".into()));
    }
    // Wire provenance identifies base tables, not distinct instances in a self-join.
    if !single_table_query(statement, engine) {
        return Ok(Editability::ReadOnly("joins aren't editable".into()));
    }
    if tables.len() != 1 {
        return Ok(Editability::ReadOnly(format!(
            "columns from {} tables",
            tables.len()
        )));
    }
    let table = tables.first().expect("one base table remains");
    match engine {
        Engine::PostgreSql => postgres::inspect(session, table, columns).await,
        Engine::MySql => mysql::inspect(session, statement, table, columns).await,
        Engine::SqlServer => Err(Error::Unsupported(engine)),
    }
}

fn single_table_query(statement: &str, engine: Engine) -> bool {
    let dialect: &dyn Dialect = match engine {
        Engine::SqlServer => &MsSqlDialect {},
        Engine::PostgreSql => &PostgreSqlDialect {},
        Engine::MySql => &MySqlDialect {},
    };
    let Ok(statements) = Parser::parse_sql(dialect, statement) else {
        return false;
    };
    let [SqlStatement::Query(query)] = statements.as_slice() else {
        return false;
    };
    if query.with.is_some() {
        return false;
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return false;
    };
    let [table] = select.from.as_slice() else {
        return false;
    };
    table.joins.is_empty() && matches!(table.relation, TableFactor::Table { args: None, .. })
}

impl EditableResult {
    /// Per-column policy in the original result order, including read-only expressions.
    pub fn columns(&self) -> &[EditColumn] {
        &self.columns
    }

    /// Chosen key's zero-based result indexes; composite key order is preserved.
    pub fn key_columns(&self) -> &[usize] {
        &self.key
    }

    /// Generates one statement per pending change in input order, without server writes.
    /// Refuses wrong row widths, NULL keys, duplicate cells and read-only edits.
    pub fn script(&self, changes: &[Change]) -> Result<Script, Error> {
        let table = self
            .table
            .iter()
            .map(|part| identifier(part, self.engine))
            .collect::<Result<Vec<_>, _>>()?
            .join(".");
        let mut statements = Vec::with_capacity(changes.len());
        let mut sql = String::new();
        for change in changes {
            let statement = self.statement(&table, change)?;
            sql.push_str(&statement.sql);
            sql.push('\n');
            statements.push(statement);
        }
        Ok(Script {
            engine: self.engine,
            statements,
            sql,
        })
    }

    fn statement(&self, table: &str, change: &Change) -> Result<Statement, Error> {
        let invalid = |message: &str| Error::InvalidChange(message.into());
        let (original, cells) = match change {
            Change::Update { original, cells } => {
                if cells.is_empty() {
                    return Err(invalid("no edited cells"));
                }
                (Some(original), cells.as_slice())
            }
            Change::Delete { original } => (Some(original), &[][..]),
            Change::Insert { cells } => (None, cells.as_slice()),
        };
        if let Some(row) = original {
            if row.len() != self.columns.len() {
                return Err(invalid("original row width differs from result"));
            }
            if self.key.iter().any(|&index| row[index] == Value::Null) {
                return Err(invalid("original key contains NULL"));
            }
        }
        let mut seen = BTreeSet::new();
        for cell in cells {
            let column = self
                .columns
                .get(cell.column)
                .ok_or_else(|| invalid("column index outside result"))?;
            if column.read_only {
                return Err(invalid("column is read-only"));
            }
            if !seen.insert(column.name.as_deref()) {
                return Err(invalid("duplicate base column"));
            }
            if original.is_some() && !column.comparable {
                return Err(invalid("edited column has no equality comparison"));
            }
        }
        let name = |index: usize| -> Result<String, Error> {
            let column = self.columns[index]
                .name
                .as_deref()
                .ok_or_else(|| invalid("expression column is read-only"))?;
            Ok(identifier(column, self.engine)?)
        };
        let (kind, sql) = match change {
            Change::Insert { .. } if cells.is_empty() => (
                Kind::Insert,
                match self.engine {
                    Engine::MySql => format!("INSERT INTO {table} () VALUES ();"),
                    _ => format!("INSERT INTO {table} DEFAULT VALUES;"),
                },
            ),
            Change::Insert { .. } => {
                let names = cells
                    .iter()
                    .map(|cell| name(cell.column))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                let values = cells
                    .iter()
                    .map(|cell| literal(&cell.value, self.engine))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                (
                    Kind::Insert,
                    format!("INSERT INTO {table} ({names}) VALUES ({values});"),
                )
            }
            Change::Update { original, .. } | Change::Delete { original } => {
                let mut predicates = Vec::new();
                let mut checked = BTreeSet::new();
                let extra: Vec<_> = if matches!(change, Change::Update { .. }) {
                    cells.iter().map(|cell| cell.column).collect()
                } else {
                    self.columns
                        .iter()
                        .enumerate()
                        .filter_map(|(i, column)| {
                            (column.name.is_some() && column.comparable).then_some(i)
                        })
                        .collect()
                };
                for &index in self.key.iter().chain(&extra) {
                    if !checked.insert(self.columns[index].name.as_deref()) {
                        continue;
                    }
                    let column = name(index)?;
                    predicates.push(if original[index] == Value::Null {
                        format!("{column} IS NULL")
                    } else if self.engine == Engine::SqlServer
                        && matches!(original[index], Value::Text(_))
                    {
                        // Text equality must detect collation equivalence and trailing-space changes.
                        format!(
                            "CAST({column} AS varbinary(max)) = CAST({} AS varbinary(max))",
                            self.original(index, &original[index])?
                        )
                    } else if self.engine == Engine::MySql
                        && matches!(original[index], Value::Text(_))
                    {
                        format!(
                            "CAST({column} AS BINARY) = CAST({} AS BINARY)",
                            self.original(index, &original[index])?
                        )
                    } else {
                        format!("{column} = {}", self.original(index, &original[index])?)
                    });
                }
                let predicate = predicates.join(" AND ");
                if matches!(change, Change::Update { .. }) {
                    let assignments = cells
                        .iter()
                        .map(|cell| {
                            Ok(format!(
                                "{} = {}",
                                name(cell.column)?,
                                literal(&cell.value, self.engine)?
                            ))
                        })
                        .collect::<Result<Vec<_>, Error>>()?
                        .join(", ");
                    (
                        Kind::Update,
                        format!("UPDATE {table} SET {assignments} WHERE {predicate};"),
                    )
                } else {
                    (
                        Kind::Delete,
                        format!("DELETE FROM {table} WHERE {predicate};"),
                    )
                }
            }
        };
        Ok(Statement { kind, sql })
    }

    fn original(&self, index: usize, value: &Value) -> Result<String, Error> {
        let mut value = crate::sql::original_literal(value, self.engine)?;
        if self.engine == Engine::MySql {
            return Ok(
                match self.comparison_types.get(index).and_then(Option::as_ref) {
                    Some(ty) => format!("CAST({value} AS {ty})"),
                    None => value,
                },
            );
        }
        if let Some(Some(collation)) = self.comparison_collations.get(index) {
            // SQL Server's COLLATE takes a bare name; a bracketed one is a syntax error.
            if collation.is_empty()
                || !collation
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(Error::Catalog(format!(
                    "unexpected collation name {collation:?}"
                )));
            }
            value = format!("{value} COLLATE {collation}");
        }
        Ok(
            match self.comparison_types.get(index).and_then(Option::as_ref) {
                Some(ty) => {
                    let style = if matches!(
                        ty.split('(').next(),
                        Some(
                            "date"
                                | "time"
                                | "datetime"
                                | "smalldatetime"
                                | "datetime2"
                                | "datetimeoffset"
                        )
                    ) {
                        ", 126"
                    } else {
                        ""
                    };
                    format!("CONVERT({ty}, {value}{style})")
                }
                None => value,
            },
        )
    }
}

#[cfg(test)]
mod tests;
