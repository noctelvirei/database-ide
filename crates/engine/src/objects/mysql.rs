//! Read-only MySQL catalogs over Session, without driver types or a database switch.
//!
//! Each database has one synthetic schema of the same name. Keys contain names because MySQL
//! exposes no persistent object IDs; refresh after renaming or recreating an object. Definitions
//! preserve SHOW CREATE's dependencies and security context, and are not database backups.

use std::fmt::Display;
use std::future::Future;

use futures::StreamExt;
use serde::de::DeserializeOwned;

use crate::objects::{
    Column, Constraint, Index, Kind, Object, Script, Search, SearchReport, SkippedDatabase,
};
use crate::session::{Event, Session, Value};

mod metadata;
mod queries;
mod scripts;

/// Catalog, session and unsupported operation failures, ready for display.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A failed run, retaining the native MySQL code and statement line.
    #[error("MySQL catalog: {text}")]
    Server {
        /// Server or transport diagnostic.
        text: String,
        /// Native MySQL error number, when available.
        code: Option<String>,
        /// One-based line in the catalog query, when available.
        line: Option<std::num::NonZeroU64>,
    },
    /// A server response did not have the expected shape or available definition.
    #[error("Unexpected MySQL catalog response: {0}")]
    Catalog(String),
    /// Names and filters must not contain zero bytes.
    #[error("MySQL catalog names and filters cannot contain a zero byte")]
    Nul,
    /// The node belongs to another database or its engine key was changed.
    #[error("Object identity does not belong to this MySQL database")]
    Identity,
    /// The object was dropped, renamed or changed kind since listing.
    #[error("Object no longer exists; refresh the object explorer")]
    NotFound,
    /// SHOW CREATE cannot safely represent arbitrary identifiers with quoting disabled.
    #[error("Enable sql_quote_show_create before scripting MySQL DDL")]
    QuotingDisabled,
    /// SHOW CREATE uses double quotes in this mode instead of the required backticks.
    #[error("Disable ANSI_QUOTES before scripting MySQL DDL")]
    AnsiQuotes,
    /// This kind does not offer the requested operation.
    #[error("{operation} is not offered for {kind:?}")]
    Unsupported {
        /// Requested child list or script form.
        operation: &'static str,
        /// Object kind without this operation.
        kind: Kind,
    },
    /// The search connector opened a different database than requested.
    #[error("Connector opened {actual} instead of {requested}")]
    Database {
        /// Database requested from the connector.
        requested: String,
        /// Database actually opened.
        actual: String,
    },
}

/// Lists visible user databases in binary name order, excluding MySQL's four system databases.
pub async fn databases(session: &mut dyn Session) -> Result<Vec<Object>, Error> {
    query(session, queries::DATABASES).await
}

/// Returns the current user database as its single schema; a system database has no children.
pub async fn schemas(session: &mut dyn Session) -> Result<Vec<Object>, Error> {
    catalog(session, "kind = 'schema'", None).await
}

/// Lists tables, views and routines in the current database's synthetic schema by name.
/// Other schema names have no children; indexes are loaded separately per table.
pub async fn objects(session: &mut dyn Session, schema: &str) -> Result<Vec<Object>, Error> {
    catalog(
        session,
        &format!("schema_name = {} AND kind <> 'index'", literal(schema)?),
        None,
    )
    .await
}

/// Lists a table or view's columns in declaration order, including defaults and generation.
/// Rejects foreign identities and stale nodes before reading metadata.
pub async fn columns(session: &mut dyn Session, object: &Object) -> Result<Vec<Column>, Error> {
    require(object, &[Kind::Table, Kind::View], "Columns")?;
    resolve(session, object).await?;
    metadata::columns(session, object).await
}

/// Lists a table's primary, unique, foreign and check constraints in binary name order.
/// NOT NULL belongs to column metadata, rather than this list.
pub async fn constraints(
    session: &mut dyn Session,
    object: &Object,
) -> Result<Vec<Constraint>, Error> {
    require(object, &[Kind::Table], "Constraints")?;
    resolve(session, object).await?;
    metadata::constraints(session, object).await
}

/// Lists a table's indexes by binary name, including primary and unique constraint indexes.
pub async fn indexes(session: &mut dyn Session, object: &Object) -> Result<Vec<Index>, Error> {
    require(object, &[Kind::Table], "Indexes")?;
    resolve(session, object).await?;
    metadata::indexes(session, object).await
}

/// Searches the current user database with a literal case-insensitive name substring.
/// Percent and underscore are ordinary characters; the limit is applied by the server.
pub async fn search(session: &mut dyn Session, search: &Search) -> Result<Vec<Object>, Error> {
    if search.limit == 0 {
        return Ok(Vec::new());
    }
    let mut filter = format!("LOCATE(LOWER({}), LOWER(name)) > 0", literal(&search.name)?);
    if !search.kinds.is_empty() {
        let kinds = search
            .kinds
            .iter()
            .map(|kind| format!("'{}'", kind_name(*kind)))
            .collect::<Vec<_>>()
            .join(", ");
        filter.push_str(&format!(" AND kind IN ({kinds})"));
    }
    catalog(session, &filter, Some(search.limit)).await
}

/// Searches user databases sequentially using a caller-owned session for each database.
/// The limit is global; connection and catalog errors are reported without losing other hits.
/// Enumeration failures fail the operation; limit zero never calls the connector.
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
            Ok(mut connection) => {
                if connection.current_database() != database.name {
                    Err(Error::Database {
                        requested: database.name.clone(),
                        actual: connection.current_database().into(),
                    }
                    .to_string())
                } else {
                    let remaining = Search {
                        limit: search.limit - report.objects.len(),
                        ..search.clone()
                    };
                    self::search(&mut connection, &remaining)
                        .await
                        .map_err(|error| error.to_string())
                }
            }
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

/// CREATE is available for tables, views, routines and indexes; ALTER for tables and views.
/// Routine bodies cannot be replaced by MySQL ALTER, so no destructive drop/recreate is offered.
/// SELECT is available for tables and views only; synthetic schemas have no script forms.
pub fn supports(kind: Kind, form: Script) -> bool {
    match form {
        Script::Create => matches!(
            kind,
            Kind::Table | Kind::View | Kind::Procedure | Kind::Function | Kind::Index
        ),
        Script::Alter | Script::Select => matches!(kind, Kind::Table | Kind::View),
    }
}

/// Returns SQL for an editor without executing it or changing the source session settings.
/// SHOW CREATE preserves table options, keys, defaults, routine bodies and definers. Dependencies
/// must exist at the destination; qualified references keep their original database names.
/// Executing DDL sets the destination SQL mode and, for views/routines, its connection collation.
/// The client character set stays UTF8 to match Session's SQL bytes.
/// DDL requires sql_quote_show_create enabled; otherwise an actionable error is returned.
/// SHOW CREATE also requires ANSI_QUOTES disabled so emitted identifiers use backticks.
/// Unqualified created objects use the destination's current database. Routine SQL is sent as one
/// server statement, without mysql-client DELIMITER commands. Table ALTER is a commented skeleton;
/// SELECT quotes every visible column and is limited to 1000 rows.
pub async fn script(
    session: &mut dyn Session,
    object: &Object,
    form: Script,
) -> Result<String, Error> {
    if !supports(object.kind, form) {
        return Err(Error::Unsupported {
            operation: "Script as",
            kind: object.kind,
        });
    }
    resolve(session, object).await?;
    scripts::script(session, object, form).await
}

/// Resolves a caller's node again so supplied names never bypass identity validation.
pub(super) async fn resolve(session: &mut dyn Session, object: &Object) -> Result<(), Error> {
    if object.id.database != session.current_database() {
        return Err(Error::Identity);
    }
    let found = catalog(
        session,
        &format!(
            "object_key = {} AND kind = '{}'",
            literal(&object.id.key)?,
            kind_name(object.kind)
        ),
        Some(1),
    )
    .await?;
    match found.first() {
        Some(found) if found == object => Ok(()),
        Some(_) => Err(Error::Identity),
        None => Err(Error::NotFound),
    }
}

async fn catalog(
    session: &mut dyn Session,
    filter: &str,
    limit: Option<usize>,
) -> Result<Vec<Object>, Error> {
    let limit = limit
        .map(|limit| format!(" LIMIT {limit}"))
        .unwrap_or_default();
    query(
        session,
        &format!(
            "SELECT CAST({} AS CHAR CHARACTER SET utf8mb4) FROM ({}) catalog
        WHERE DATABASE() NOT IN ('mysql', 'sys', 'information_schema', 'performance_schema')
        AND {filter} ORDER BY BINARY schema_name, BINARY name, kind, BINARY object_key{limit}",
            queries::OBJECT_JSON,
            queries::CATALOG
        ),
    )
    .await
}

/// Drains a catalog run even after failure, leaving the session ready for its next caller.
pub(super) async fn rows(session: &mut dyn Session, sql: &str) -> Result<Vec<Vec<Value>>, Error> {
    let mut run = session.run(sql);
    let mut result = Vec::new();
    let mut failure = None;
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(rows) if failure.is_none() => result.extend(rows),
            Event::Error { text, code, line } if failure.is_none() => {
                failure = Some(Error::Server { text, code, line })
            }
            _ => {}
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(result),
    }
}

/// Decodes one JSON text column per row, rejecting unexpected catalog shapes.
pub(super) async fn query<T: DeserializeOwned>(
    session: &mut dyn Session,
    sql: &str,
) -> Result<Vec<T>, Error> {
    rows(session, sql)
        .await?
        .into_iter()
        .map(|row| match row.as_slice() {
            [Value::Text(json)] => {
                serde_json::from_str(json).map_err(|error| Error::Catalog(error.to_string()))
            }
            _ => Err(Error::Catalog("Expected one text column".into())),
        })
        .collect()
}

/// Produces a UTF8 hex literal independent of SQL mode, with no interpolated SQL syntax.
pub(super) fn literal(value: &str) -> Result<String, Error> {
    if value.contains('\0') {
        return Err(Error::Nul);
    }
    let hex = value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("_utf8mb4 X'{hex}'"))
}

/// Quotes an identifier component, including embedded backticks, without treating it as SQL.
pub(super) fn quote(value: &str) -> Result<String, Error> {
    if value.contains('\0') {
        return Err(Error::Nul);
    }
    Ok(format!("`{}`", value.replace('`', "``")))
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
        Kind::MaterializedView => "materialized_view",
        Kind::Function => "function",
        Kind::Procedure => "procedure",
        Kind::Sequence => "sequence",
        Kind::Index => "index",
    }
}
