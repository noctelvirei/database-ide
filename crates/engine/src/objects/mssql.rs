//! Read-only SQL Server catalogs through Session. Scripts exclude data, grants and dependencies.
//! Cross-database search uses caller-owned connections and never switches the original session.

use std::fmt::Display;
use std::future::Future;

use futures::StreamExt;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::objects::{
    Column, Constraint, Index, Kind, Object, ObjectId, Script, Search, SearchReport,
    SkippedDatabase,
};
use crate::session::{Event, Session, Value};

mod queries;
mod scripting;

/// Catalog failures retain server diagnostics and leave the run drained before returning.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Server or transport failure from the session.
    #[error("SQL Server catalog: {text}")]
    Server {
        /// Displayable diagnostic.
        text: String,
        /// Native SQL Server error number, when available.
        code: Option<String>,
        /// One-based line in the submitted SQL, when available.
        line: Option<std::num::NonZeroU64>,
    },
    /// A catalog response or definition cannot be represented faithfully.
    #[error("SQL Server catalog: {0}")]
    Catalog(String),
    /// Names and filters cannot contain zero bytes.
    #[error("SQL Server catalog names and filters cannot contain a zero byte")]
    Nul,
    /// The engine key is malformed or belongs to another database.
    #[error("Object identity does not belong to this SQL Server database")]
    Identity,
    /// The object was dropped, renamed or changed kind since enumeration.
    #[error("Object no longer exists; refresh the object explorer")]
    NotFound,
    /// This script or child list does not apply to the supplied kind.
    #[error("{operation} is not offered for {kind:?}")]
    Unsupported {
        /// Requested operation.
        operation: &'static str,
        /// Supplied object kind.
        kind: Kind,
    },
}

/// SQL Server column details supplement the common explorer column without changing its contract.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ColumnDetails {
    /// Common type, nullability, expression and identity policy in declaration order.
    #[serde(flatten)]
    pub column: Column,
    /// Original identity seed as exact decimal text; absent for nonidentity columns.
    pub identity_seed: Option<String>,
    /// Identity increment as exact decimal text; absent for nonidentity columns.
    pub identity_increment: Option<String>,
    /// Whether replication inserts bypass identity generation.
    pub identity_not_for_replication: Option<bool>,
    /// Named default constraint, absent when there is no default.
    pub default_constraint: Option<String>,
    /// Character collation, absent for noncharacter columns.
    pub collation: Option<String>,
    /// Whether SQL Server stores this column sparsely.
    pub sparse: bool,
    /// Whether this uniqueidentifier column is the table's ROWGUIDCOL.
    pub rowguidcol: bool,
    #[serde(default)]
    unsupported: bool,
}

/// Lists user databases by binary name order, including databases the login cannot open.
pub async fn databases(session: &mut dyn Session) -> Result<Vec<Object>, Error> {
    query(
        session,
        r#"SELECT (SELECT name AS [id.database], CONCAT('database:',database_id) AS [id.key],
        name, 'database' AS kind, CAST(0 AS bit) AS [system] FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
        FROM sys.databases WHERE name NOT IN (N'master',N'tempdb',N'model',N'msdb')
        ORDER BY name COLLATE Latin1_General_100_BIN2"#,
    )
    .await
}

/// Lists only the current database's schemas, flagging SQL Server's system namespaces.
pub async fn schemas(session: &mut dyn Session) -> Result<Vec<Object>, Error> {
    catalog(session, "kind = 'schema'", None).await
}

/// Lists one schema's tables, views, procedures and functions; indexes load per relation.
pub async fn objects(session: &mut dyn Session, schema: &str) -> Result<Vec<Object>, Error> {
    catalog(
        session,
        &format!("[schema] = {} AND kind <> 'index'", literal(schema)?),
        None,
    )
    .await
}

/// Returns visible columns of a table or view in declaration order; rejects stale identities.
pub async fn columns(session: &mut dyn Session, object: &Object) -> Result<Vec<Column>, Error> {
    Ok(column_details(session, object)
        .await?
        .into_iter()
        .map(|c| c.column)
        .collect())
}

/// Returns visible columns with identity seed/increment and named defaults, in declaration order.
pub async fn column_details(
    session: &mut dyn Session,
    object: &Object,
) -> Result<Vec<ColumnDetails>, Error> {
    require(object, &[Kind::Table, Kind::View], "Columns")?;
    let key = resolve(session, object).await?;
    query(
        session,
        &queries::COLUMNS.replace("{key}", &key.to_string()),
    )
    .await
}

/// Lists primary, unique, foreign and CHECK constraints in binary name order.
/// Default constraint names are returned by column_details, alongside their column's expression.
pub async fn constraints(
    session: &mut dyn Session,
    object: &Object,
) -> Result<Vec<Constraint>, Error> {
    require(object, &[Kind::Table], "Constraints")?;
    let key = resolve(session, object).await?;
    Ok(constraint_rows(session, key)
        .await?
        .into_iter()
        .map(|r| r.constraint)
        .collect())
}

/// Lists rowstore indexes with keys before included columns, clustering, filters and executable DDL.
/// Unsupported access methods return a diagnostic rather than misleading CREATE INDEX text.
pub async fn indexes(session: &mut dyn Session, object: &Object) -> Result<Vec<Index>, Error> {
    require(object, &[Kind::Table, Kind::View], "Indexes")?;
    let key = resolve(session, object).await?;
    index_rows(session, key)
        .await?
        .into_iter()
        .map(|r| r.into_index(object, key))
        .collect()
}

/// Searches names as literal case-insensitive substrings, with a global per-call hit limit.
pub async fn search(session: &mut dyn Session, search: &Search) -> Result<Vec<Object>, Error> {
    if search.limit == 0 {
        return Ok(Vec::new());
    }
    let mut filter = format!(
        "CHARINDEX({} COLLATE Latin1_General_100_CI_AS, name COLLATE Latin1_General_100_CI_AS) > 0",
        literal(&search.name)?
    );
    if !search.kinds.is_empty() {
        let kinds = search
            .kinds
            .iter()
            .map(|k| format!("'{}'", kind_name(*k)))
            .collect::<Vec<_>>()
            .join(", ");
        filter.push_str(&format!(" AND kind IN ({kinds})"));
    }
    catalog(session, &filter, Some(search.limit)).await
}

/// Searches user databases sequentially through caller-owned short-lived sessions, as PostgreSQL does.
/// Connection and catalog failures are reported per database; the hit limit is global.
/// A connector must open the requested database. Limit zero opens no connections.
pub async fn search_all<S, E, F, Fut>(
    session: &mut dyn Session,
    search: &Search,
    mut connect: F,
) -> Result<SearchReport, Error>
where
    S: Session,
    E: Display,
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<S, E>>,
{
    let mut report = SearchReport {
        objects: Vec::new(),
        skipped: Vec::new(),
    };
    if search.limit == 0 {
        return Ok(report);
    }
    for database in databases(session).await? {
        let outcome = match connect(database.name.clone()).await {
            Ok(mut connection) if connection.current_database() == database.name => self::search(
                &mut connection,
                &Search {
                    limit: search.limit - report.objects.len(),
                    ..search.clone()
                },
            )
            .await
            .map_err(|e| e.to_string()),
            Ok(connection) => Err(format!(
                "Connector opened {} instead of {}",
                connection.current_database(),
                database.name
            )),
            Err(error) => Err(error.to_string()),
        };
        match outcome {
            Ok(objects) => report.objects.extend(objects),
            Err(reason) => report.skipped.push(SkippedDatabase {
                database: database.name,
                reason,
            }),
        }
        if report.objects.len() == search.limit {
            break;
        }
    }
    Ok(report)
}

/// Whether a kind has this Script as form; tables use a commented ALTER skeleton.
pub fn supports(kind: Kind, form: Script) -> bool {
    match form {
        Script::Create => matches!(
            kind,
            Kind::Table
                | Kind::View
                | Kind::Procedure
                | Kind::Function
                | Kind::Index
                | Kind::Schema
        ),
        Script::Alter => matches!(
            kind,
            Kind::Table | Kind::View | Kind::Procedure | Kind::Function
        ),
        Script::Select => matches!(kind, Kind::Table | Kind::View),
    }
}

/// Produces editor-ready SQL without executing it or changing the source session's settings.
/// CREATE TABLE includes named defaults, constraints, rowstore indexes and DML triggers.
/// Dependencies must already exist. Unsupported special table storage is rejected.
/// Module DDL keeps ANSI_NULLS/QUOTED_IDENTIFIER in GO-separated batches; ALTER uses CREATE OR ALTER.
pub async fn script(
    session: &mut dyn Session,
    object: &Object,
    form: Script,
) -> Result<String, Error> {
    scripting::script(session, object, form).await
}

async fn catalog(
    session: &mut dyn Session,
    filter: &str,
    limit: Option<usize>,
) -> Result<Vec<Object>, Error> {
    let top = limit.map(|n| format!("TOP ({n}) ")).unwrap_or_default();
    query(session, &format!(r#"SELECT {top}(SELECT DB_NAME() AS [id.database], [key] AS [id.key],
        [schema], name, kind, [system] FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
        FROM ({}) catalog WHERE {filter}
        ORDER BY [schema] COLLATE Latin1_General_100_BIN2, name COLLATE Latin1_General_100_BIN2, kind, [key]"#, queries::CATALOG)).await
}

async fn resolve(session: &mut dyn Session, object: &Object) -> Result<i32, Error> {
    if object.id.database != session.current_database() {
        return Err(Error::Identity);
    }
    let parts: Vec<_> = object.id.key.split(':').collect();
    let expected = match object.kind {
        Kind::Schema => "schema",
        Kind::Index => "index",
        _ => "object",
    };
    if parts.first() != Some(&expected)
        || parts.len() != if object.kind == Kind::Index { 3 } else { 2 }
        || parts[1..]
            .iter()
            .any(|p| p.parse::<i32>().map_or(true, |n| n <= 0))
    {
        return Err(Error::Identity);
    }
    let key = parts[1].parse().map_err(|_| Error::Identity)?;
    let found = catalog(
        session,
        &format!(
            "[key] = {} AND kind = '{}'",
            literal(&object.id.key)?,
            kind_name(object.kind)
        ),
        Some(1),
    )
    .await?;
    if found.first() != Some(object) {
        return Err(Error::NotFound);
    }
    Ok(key)
}

async fn query<T: DeserializeOwned>(session: &mut dyn Session, sql: &str) -> Result<Vec<T>, Error> {
    let mut run = session.run(sql);
    let mut rows = Vec::new();
    let mut failure = None;
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(batch) if failure.is_none() => {
                for row in batch {
                    match row.as_slice() {
                        [Value::Text(json)] => match serde_json::from_str(json) {
                            Ok(row) => rows.push(row),
                            Err(e) => {
                                failure = Some(Error::Catalog(e.to_string()));
                                break;
                            }
                        },
                        _ => {
                            failure = Some(Error::Catalog("Expected one JSON text column".into()));
                            break;
                        }
                    }
                }
            }
            Event::Error { text, code, line } if failure.is_none() => {
                failure = Some(Error::Server { text, code, line })
            }
            _ => {}
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(rows),
    }
}

fn literal(value: &str) -> Result<String, Error> {
    if value.contains('\0') {
        return Err(Error::Nul);
    }
    Ok(format!("N'{}'", value.replace('\'', "''")))
}

fn quote(value: &str) -> String {
    format!("[{}]", value.replace(']', "]]"))
}

fn qualified(object: &Object) -> Result<String, Error> {
    Ok(format!(
        "{}.{}",
        quote(object.schema.as_deref().ok_or(Error::Identity)?),
        quote(&object.name)
    ))
}

fn require(object: &Object, kinds: &[Kind], operation: &'static str) -> Result<(), Error> {
    if kinds.contains(&object.kind) {
        Ok(())
    } else {
        Err(Error::Unsupported {
            operation,
            kind: object.kind,
        })
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Database => "database",
        Kind::Schema => "schema",
        Kind::Table => "table",
        Kind::View => "view",
        Kind::Procedure => "procedure",
        Kind::Function => "function",
        Kind::Index => "index",
        Kind::Sequence => "sequence",
        Kind::MaterializedView => "materialized_view",
    }
}

#[derive(Deserialize)]
struct ConstraintRow {
    #[serde(flatten)]
    constraint: Constraint,
    disabled: bool,
    untrusted: bool,
}

async fn constraint_rows(session: &mut dyn Session, key: i32) -> Result<Vec<ConstraintRow>, Error> {
    query(
        session,
        &queries::CONSTRAINTS.replace("{key}", &key.to_string()),
    )
    .await
}

#[derive(Deserialize)]
struct IndexColumn {
    name: String,
    descending: bool,
    included: bool,
}

#[derive(Deserialize)]
struct IndexRow {
    index_id: i32,
    name: String,
    method: String,
    unique: bool,
    constraint_backed: bool,
    filter: Option<String>,
    disabled: bool,
    columns: Vec<IndexColumn>,
}

async fn index_rows(session: &mut dyn Session, key: i32) -> Result<Vec<IndexRow>, Error> {
    query(
        session,
        &queries::INDEXES.replace("{key}", &key.to_string()),
    )
    .await
}

impl IndexRow {
    fn into_index(self, table: &Object, key: i32) -> Result<Index, Error> {
        if !matches!(self.method.as_str(), "CLUSTERED" | "NONCLUSTERED") {
            return Err(Error::Catalog(format!(
                "Script as does not support {} indexes",
                self.method
            )));
        }
        let keys = self
            .columns
            .iter()
            .filter(|c| !c.included)
            .map(|c| {
                format!(
                    "{} {}",
                    quote(&c.name),
                    if c.descending { "DESC" } else { "ASC" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let included = self
            .columns
            .iter()
            .filter(|c| c.included)
            .map(|c| quote(&c.name))
            .collect::<Vec<_>>();
        let mut definition = format!(
            "CREATE {}{} INDEX {} ON {} ({keys})",
            if self.unique { "UNIQUE " } else { "" },
            self.method,
            quote(&self.name),
            qualified(table)?
        );
        if !included.is_empty() {
            definition.push_str(&format!(" INCLUDE ({})", included.join(", ")));
        }
        if let Some(filter) = self.filter {
            definition.push_str(&format!(" WHERE {filter}"));
        }
        definition.push(';');
        if self.disabled {
            definition.push_str(&format!(
                "\nALTER INDEX {} ON {} DISABLE;",
                quote(&self.name),
                qualified(table)?
            ));
        }
        Ok(Index {
            object: Object {
                id: ObjectId {
                    database: table.id.database.clone(),
                    key: format!("index:{key}:{}", self.index_id),
                },
                schema: table.schema.clone(),
                name: self.name,
                kind: Kind::Index,
                system: table.system,
                signature: None,
            },
            columns: self.columns.into_iter().map(|c| c.name).collect(),
            unique: self.unique,
            method: self.method,
            definition,
            constraint_backed: self.constraint_backed,
        })
    }
}
