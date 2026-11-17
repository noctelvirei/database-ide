//! Read-only PostgreSQL catalogs over `Session`, without exposing or using driver types.
//!
//! Lists fetch only their requested level. Catalog keys include the catalog and OID, scoped to the
//! session's database and server. Scripts are single-object definitions, not backups:
//! referenced schemas, types, routines and tables must already exist at the destination.

use std::fmt::Display;
use std::future::Future;

use futures::StreamExt;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::tokenizer::{Location, Token, Tokenizer};

use crate::objects::{
    Column, Constraint, Index, Kind, Object, Script, Search, SearchReport, SkippedDatabase,
};
use crate::session::{Event, Session, Value};

mod queries;

/// Catalog, session and unsupported Script as failures, ready for display.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A catalog run failed; preserves SQLSTATE and line information from Session.
    #[error("PostgreSQL catalog: {text}")]
    Server {
        /// Server or transport diagnostic.
        text: String,
        /// SQLSTATE, when supplied by the session.
        code: Option<String>,
        /// One-based line in the catalog query, when supplied.
        line: Option<std::num::NonZeroU64>,
    },
    /// The session did not return the expected JSON row shape.
    #[error("Unexpected PostgreSQL catalog response: {0}")]
    Catalog(String),
    /// PostgreSQL text cannot contain a zero byte.
    #[error("PostgreSQL catalog names and filters cannot contain a zero byte")]
    Nul,
    /// The object came from another database or has an invalid engine key.
    #[error("Object identity does not belong to this PostgreSQL database")]
    Identity,
    /// The object was dropped or its kind changed after it was listed.
    #[error("Object no longer exists; refresh the object explorer")]
    NotFound,
    /// This kind has no meaning for the requested script or child list.
    #[error("{operation} is not offered for {kind:?}")]
    Unsupported {
        /// Requested operation.
        operation: &'static str,
        /// Object kind whose operation was unavailable.
        kind: Kind,
    },
    /// Recreating inherited or partition child tables needs a multi-object script.
    #[error("Script as CREATE does not support inherited tables or partition children")]
    InheritedTable,
    /// Physical temporary namespaces cannot be recreated in another session.
    #[error("Script as DDL does not support temporary objects")]
    TemporaryObject,
    /// A connector returned a session for a different database.
    #[error("Connector opened {actual} instead of {requested}")]
    Database {
        /// Database requested from the connector.
        requested: String,
        /// Database actually opened.
        actual: String,
    },
}

/// Lists non-template databases by name, including databases that refuse connections.
/// No per-database connection is opened; run failures are returned as Error::Server.
pub async fn databases(session: &mut dyn Session) -> Result<Vec<Object>, Error> {
    query(session, r#"SELECT pg_catalog.json_build_object(
        'id', pg_catalog.json_build_object('database', datname, 'key', 'database:' || oid::text),
        'schema', NULL, 'name', datname, 'kind', 'database', 'system', false, 'signature', NULL)::text
        FROM pg_catalog.pg_database WHERE NOT datistemplate ORDER BY datname COLLATE "C""#).await
}

/// Lists the current database's schemas by name, flagging pg_* and information_schema.
/// Does not fetch objects in any schema.
pub async fn schemas(session: &mut dyn Session) -> Result<Vec<Object>, Error> {
    catalog(session, "kind = 'schema'", None).await
}

/// Lists a schema's tables, views, materialized views, routines and sequences by name.
/// Indexes are loaded separately per relation; a missing schema has no children.
pub async fn objects(session: &mut dyn Session, schema: &str) -> Result<Vec<Object>, Error> {
    catalog(
        session,
        &format!("schema = {} AND kind <> 'index'", literal(schema)?),
        None,
    )
    .await
}

/// Lists live columns in declaration order for a table, view or materialized view.
/// Rejects identities from another database and kinds without columns.
pub async fn columns(session: &mut dyn Session, object: &Object) -> Result<Vec<Column>, Error> {
    relation(object, "Columns")?;
    let key = key(session, object)?;
    query(
        session,
        &format!(
            "{} WHERE a.attrelid = {key} AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum",
            queries::COLUMNS
        ),
    )
    .await
}

/// Lists primary, unique, foreign, check and exclusion constraints by name for one table.
/// NOT NULL is represented by columns, not duplicated here.
pub async fn constraints(
    session: &mut dyn Session,
    object: &Object,
) -> Result<Vec<Constraint>, Error> {
    require(object, &[Kind::Table], "Constraints")?;
    let key = key(session, object)?;
    query(session, &format!("{} WHERE conrelid = {key} AND contype IN ('p','u','f','c','x') ORDER BY conname COLLATE \"C\"", queries::CONSTRAINTS)).await
}

/// Lists a table or materialized view's indexes by name, including constraint-backed ones.
pub async fn indexes(session: &mut dyn Session, object: &Object) -> Result<Vec<Index>, Error> {
    require(object, &[Kind::Table, Kind::MaterializedView], "Indexes")?;
    let key = key(session, object)?;
    query(
        session,
        &format!(
            "{} WHERE i.indrelid = {key} ORDER BY c.relname COLLATE \"C\"",
            queries::INDEXES
        ),
    )
    .await
}

/// Searches the current database, including schemas, indexes and system objects.
/// Uses a literal case-insensitive substring, optional kinds and a server-side hit limit.
pub async fn search(session: &mut dyn Session, search: &Search) -> Result<Vec<Object>, Error> {
    if search.limit == 0 {
        return Ok(Vec::new());
    }
    let mut filter = format!("strpos(lower(name), lower({})) > 0", literal(&search.name)?);
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

/// Searches non-template databases sequentially with a short caller-owned session each.
/// The limit is global. Connection and per-database catalog errors are reported and skipped;
/// failure to enumerate databases fails the operation. No connector is called for limit zero.
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

/// Whether this engine offers the form. ALTER is unavailable for materialized views,
/// indexes, sequences, schemas and databases; databases have no script forms here.
pub fn supports(kind: Kind, form: Script) -> bool {
    match form {
        Script::Create => kind != Kind::Database,
        Script::Alter => matches!(
            kind,
            Kind::Table | Kind::View | Kind::Function | Kind::Procedure
        ),
        Script::Select => matches!(kind, Kind::Table | Kind::View),
    }
}

/// Returns SQL for a new editor tab without executing it or changing session settings.
/// CREATE excludes data, ownership, grants and external dependencies. Tables include
/// columns, constraints, row security policies, independent indexes, owned sequences and comments.
/// Inherited tables and partition children are rejected rather than changing their meaning.
/// Temporary objects support SELECT and the table ALTER skeleton, but not executable DDL.
/// Table ALTER is a fully commented skeleton; SELECT names every column and limits to 1000.
/// Executing generated DDL sets the destination search_path to the source's resolved schemas.
/// DDL literals parse identically with standard_conforming_strings on or off.
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
    let key = key(session, object)?;
    // Re-resolve the node before scripting; never trust caller-supplied names as SQL.
    let found = catalog(
        session,
        &format!(
            "key = {} AND kind = '{}'",
            literal(&object.id.key)?,
            kind_name(object.kind)
        ),
        Some(1),
    )
    .await?;
    let found = found.first().ok_or(Error::NotFound)?;
    if found.name != object.name || found.schema != object.schema {
        return Err(Error::NotFound);
    }
    if (form == Script::Create || object.kind != Kind::Table && form == Script::Alter)
        && found
            .schema
            .as_deref()
            .unwrap_or(&found.name)
            .starts_with("pg_temp_")
    {
        return Err(Error::TemporaryObject);
    }
    let sql = match (object.kind, form) {
        (Kind::Table, Script::Create) => table_sql(key),
        (Kind::Table, Script::Alter) => format!(
            r#"
            SELECT pg_catalog.json_build_object('sql', '-- ALTER TABLE ' || qualified
                || E'\n--     ADD COLUMN column_name data_type;\n')::text {from}
            WHERE c.oid = {key}"#,
            from = relation_from()
        ),
        (_, Script::Select) => format!(
            r#"
            SELECT pg_catalog.json_build_object('sql', 'SELECT '
                || COALESCE((SELECT string_agg(pg_catalog.quote_ident(a.attname), ', ' ORDER BY a.attnum)
                    FROM pg_catalog.pg_attribute a WHERE a.attrelid = c.oid
                        AND a.attnum > 0 AND NOT a.attisdropped), '')
                || ' FROM ' || qualified || E' LIMIT 1000;\n')::text {from}
            WHERE c.oid = {key}"#,
            from = relation_from()
        ),
        (Kind::View | Kind::MaterializedView, _) => view_sql(key, object.kind, form),
        (Kind::Function | Kind::Procedure, _) => {
            format!(
                "SELECT pg_catalog.json_build_object('sql', pg_catalog.pg_get_functiondef(oid))::text FROM pg_catalog.pg_proc WHERE oid = {key}"
            )
        }
        (Kind::Index, _) => format!(
            "SELECT pg_catalog.json_build_object('sql', pg_catalog.pg_get_indexdef(oid) || E';\\n')::text FROM pg_catalog.pg_class WHERE oid = {key}"
        ),
        (Kind::Sequence, _) => format!(
            r#"
            SELECT pg_catalog.json_build_object('sql', 'CREATE SEQUENCE ' || qualified
                || ({options}) || E';\n')::text {from}
            JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid WHERE c.oid = {key}
            "#,
            options = queries::SEQUENCE_OPTIONS,
            from = relation_from()
        ),
        (Kind::Schema, _) => format!(
            r#"
            SELECT pg_catalog.json_build_object('sql', 'CREATE SCHEMA '
                || pg_catalog.quote_ident(nspname) || E';\n')::text
            FROM pg_catalog.pg_namespace WHERE oid = {key}"#
        ),
        _ => {
            return Err(Error::Unsupported {
                operation: "Script as",
                kind: object.kind,
            });
        }
    };
    let sql = if form == Script::Select || (form == Script::Alter && object.kind == Kind::Table) {
        sql
    } else {
        // Resolve $user now; a destination login can have a different same-named schema.
        format!(
            r#"SELECT (payload::jsonb || pg_catalog.jsonb_build_object('search_path',
            (SELECT string_agg(CASE WHEN left(name, 8) = 'pg_temp_' THEN 'pg_temp'
                ELSE pg_catalog.quote_ident(name) END, ', ' ORDER BY position)
             FROM unnest(current_schemas(true)) WITH ORDINALITY schemas(name, position)),
            'standard_conforming_strings', current_setting('standard_conforming_strings')))::text
            FROM ({sql}) definition(payload)"#
        )
    };
    let rows: Vec<ScriptRow> = query(session, &sql).await?;
    let row = rows.into_iter().next().ok_or(Error::NotFound)?;
    if row.inherited {
        return Err(Error::InheritedTable);
    }
    let mut sql = row
        .sql
        .ok_or_else(|| Error::Catalog("Missing object definition".into()))?;
    if form == Script::Create && matches!(object.kind, Kind::Function | Kind::Procedure) {
        sql = sql.replacen("CREATE OR REPLACE ", "CREATE ", 1);
    }
    if let Some(search_path) = row.search_path {
        let strings = row
            .standard_conforming_strings
            .ok_or_else(|| Error::Catalog("Missing standard_conforming_strings setting".into()))?;
        sql = portable_literals(&sql, &strings)?;
        sql = format!("SET search_path = {search_path};\n{sql}");
    }
    Ok(sql)
}

#[derive(Deserialize)]
struct ScriptRow {
    sql: Option<String>,
    search_path: Option<String>,
    standard_conforming_strings: Option<String>,
    #[serde(default)]
    inherited: bool,
}

// ruleutils.c emits ordinary literals under either setting, never E literals.
fn portable_literals(sql: &str, strings: &str) -> Result<String, Error> {
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .with_unescape(false)
        .tokenize_with_location()
        .map_err(|error| Error::Catalog(error.to_string()))?;
    let mut positions = sql
        .char_indices()
        .scan(Location::new(1, 1), |location, (offset, ch)| {
            let start = *location;
            if ch == '\n' {
                location.line += 1;
                location.column = 1;
            } else {
                location.column += 1;
            }
            Some((start, offset))
        });
    let mut result = String::with_capacity(sql.len());
    let mut copied = 0;
    for token in tokens {
        if let Token::SingleQuotedString(value) = token.token
            && value.contains('\\')
        {
            let (_, start) = positions
                .find(|(location, _)| *location == token.span.start)
                .ok_or_else(|| Error::Catalog("Missing literal position".into()))?;
            result.push_str(&sql[copied..start]);
            result.push_str("E'");
            if strings == "on" {
                result.push_str(&value.replace('\\', "\\\\"));
            } else {
                result.push_str(&value);
            }
            result.push('\'');
            copied = start + value.len() + 2;
        }
    }
    result.push_str(&sql[copied..]);
    Ok(result)
}

fn relation_from() -> &'static str {
    r#"FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    CROSS JOIN LATERAL (SELECT CASE WHEN c.relpersistence = 't' THEN 'pg_temp'
        ELSE pg_catalog.quote_ident(n.nspname) END || '.'
        || pg_catalog.quote_ident(c.relname) AS qualified) q"#
}

fn view_sql(key: u32, kind: Kind, form: Script) -> String {
    let create = match (kind, form) {
        (Kind::MaterializedView, _) => "CREATE MATERIALIZED VIEW ",
        (_, Script::Alter) => "CREATE OR REPLACE VIEW ",
        _ => "CREATE VIEW ",
    };
    let data = if kind == Kind::MaterializedView {
        " WITH NO DATA"
    } else {
        ""
    };
    format!(
        r#"SELECT pg_catalog.json_build_object('sql', '{create}' || qualified
        || COALESCE(' (' || (SELECT string_agg(pg_catalog.quote_ident(a.attname), ', ' ORDER BY a.attnum)
            FROM pg_catalog.pg_attribute a WHERE a.attrelid = c.oid
                AND a.attnum > 0 AND NOT a.attisdropped) || ')', '')
        || CASE WHEN c.reloptions IS NOT NULL THEN ' WITH ('
            || array_to_string(c.reloptions, ', ') || ')' ELSE '' END
        || ' AS ' || rtrim(pg_catalog.pg_get_viewdef(c.oid, false), E';\n ')
        || E'{data};\n')::text {from} WHERE c.oid = {key}"#,
        from = relation_from()
    )
}

fn table_sql(key: u32) -> String {
    let options = queries::SEQUENCE_OPTIONS;
    format!(
        r#"
    SELECT pg_catalog.json_build_object('inherited', c.relispartition OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_inherits WHERE inhrelid = c.oid),
        'sql', COALESCE((
            SELECT string_agg('CREATE SEQUENCE ' || pg_catalog.quote_ident(sn.nspname)
                || '.' || pg_catalog.quote_ident(sc.relname) || ({options}) || E';\n', '' ORDER BY sc.relname)
            FROM pg_catalog.pg_depend dep JOIN pg_catalog.pg_sequence s ON s.seqrelid = dep.objid
            JOIN pg_catalog.pg_class sc ON sc.oid = s.seqrelid
            JOIN pg_catalog.pg_namespace sn ON sn.oid = sc.relnamespace
            WHERE dep.refobjid = c.oid AND dep.deptype = 'a'
                AND dep.refclassid = 'pg_catalog.pg_class'::regclass
                AND dep.classid = 'pg_catalog.pg_class'::regclass), '')
        || CASE c.relpersistence WHEN 'u' THEN 'CREATE UNLOGGED TABLE '
            WHEN 't' THEN 'CREATE TEMP TABLE ' ELSE 'CREATE TABLE ' END || qualified
        || E' (\n    ' || concat_ws(E',\n    ', ({columns}), ({constraints})) || E'\n)'
        || CASE WHEN c.relkind = 'p' THEN ' PARTITION BY ' || pg_catalog.pg_get_partkeydef(c.oid) ELSE '' END
        || E';\n' || COALESCE(({indexes}) || E'\n', '')
        || CASE WHEN c.relrowsecurity THEN 'ALTER TABLE ' || qualified
            || E' ENABLE ROW LEVEL SECURITY;\n' ELSE '' END
        || CASE WHEN c.relforcerowsecurity THEN 'ALTER TABLE ' || qualified
            || E' FORCE ROW LEVEL SECURITY;\n' ELSE '' END
        || COALESCE(({policies}), '')
        || COALESCE((SELECT string_agg('ALTER SEQUENCE ' || pg_catalog.quote_ident(sn.nspname)
            || '.' || pg_catalog.quote_ident(sc.relname) || ' OWNED BY ' || qualified || '.'
            || pg_catalog.quote_ident(a.attname) || E';\n', '' ORDER BY sc.relname)
            FROM pg_catalog.pg_depend dep JOIN pg_catalog.pg_sequence s ON s.seqrelid = dep.objid
            JOIN pg_catalog.pg_class sc ON sc.oid = s.seqrelid
            JOIN pg_catalog.pg_namespace sn ON sn.oid = sc.relnamespace
            JOIN pg_catalog.pg_attribute a ON a.attrelid = dep.refobjid AND a.attnum = dep.refobjsubid
            WHERE dep.refobjid = c.oid AND dep.deptype = 'a'
                AND dep.refclassid = 'pg_catalog.pg_class'::regclass
                AND dep.classid = 'pg_catalog.pg_class'::regclass), '')
        || COALESCE(({comments}) || E'\n', ''))::text
    {from} WHERE c.oid = {key}
    "#,
        columns = queries::TABLE_COLUMNS,
        constraints = queries::TABLE_CONSTRAINTS,
        indexes = queries::TABLE_INDEXES,
        policies = queries::TABLE_POLICIES,
        comments = queries::TABLE_COMMENTS,
        from = relation_from()
    )
}

async fn catalog(
    session: &mut dyn Session,
    filter: &str,
    limit: Option<usize>,
) -> Result<Vec<Object>, Error> {
    let limit = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
    query(
        session,
        &format!(
            r#"SELECT ({json})::text FROM ({catalog}) catalog
        WHERE {filter} ORDER BY schema COLLATE "C" NULLS FIRST, name COLLATE "C",
            kind, signature COLLATE "C", key{limit}"#,
            json = queries::OBJECT_JSON,
            catalog = queries::CATALOG
        ),
    )
    .await
}

async fn query<T: DeserializeOwned>(session: &mut dyn Session, sql: &str) -> Result<Vec<T>, Error> {
    let mut run = session.run(sql);
    let mut result = Vec::new();
    let mut failure = None;
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(rows) if failure.is_none() => {
                for row in rows {
                    match row.as_slice() {
                        [Value::Text(json)] => match serde_json::from_str(json) {
                            Ok(value) => result.push(value),
                            Err(error) => {
                                failure = Some(Error::Catalog(error.to_string()));
                                break;
                            }
                        },
                        _ => {
                            failure = Some(Error::Catalog("Expected one text column".into()));
                            break;
                        }
                    }
                }
            }
            Event::Error { text, code, line } if failure.is_none() => {
                failure = Some(Error::Server { text, code, line });
            }
            _ => {}
        }
    }
    // Drain even on failure so the same session remains usable by its caller.
    match failure {
        Some(error) => Err(error),
        None => Ok(result),
    }
}

// Session exposes simple-query SQL only. E literals do not depend on standard_conforming_strings.
fn literal(value: &str) -> Result<String, Error> {
    if value.contains('\0') {
        return Err(Error::Nul);
    }
    Ok(format!(
        "E'{}'",
        value.replace('\\', "\\\\").replace('\'', "''")
    ))
}

fn key(session: &dyn Session, object: &Object) -> Result<u32, Error> {
    if object.id.database != session.current_database() {
        return Err(Error::Identity);
    }
    let prefix = match object.kind {
        Kind::Database => "database",
        Kind::Schema => "schema",
        Kind::Function | Kind::Procedure => "routine",
        _ => "relation",
    };
    let (catalog, oid) = object.id.key.split_once(':').ok_or(Error::Identity)?;
    if catalog != prefix {
        return Err(Error::Identity);
    }
    oid.parse().map_err(|_| Error::Identity)
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

fn relation(object: &Object, operation: &'static str) -> Result<(), Error> {
    require(
        object,
        &[Kind::Table, Kind::View, Kind::MaterializedView],
        operation,
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_literals_escape_backslashes_from_either_source_setting() {
        assert_eq!(
            portable_literals(r"DEFAULT 'pear\tree''s\'::text", "on").unwrap(),
            r"DEFAULT E'pear\\tree''s\\'::text"
        );
        assert_eq!(
            portable_literals(r"DEFAULT 'pear\\tree''s\\'::text", "off").unwrap(),
            r"DEFAULT E'pear\\tree''s\\'::text"
        );
    }

    #[test]
    fn ddl_literal_conversion_preserves_other_sql_verbatim() {
        let sql = "-- 'pear\\tree'\n/* 'pear\\tree' /* nested */ */\n\
            CREATE VIEW \"果実\\箱\" AS SELECT E'pear\\\\tree', $body$'pear\\tree'$body$,\n\
            'pear', '果実\\木', 'more\\fruit';\n";
        assert_eq!(
            portable_literals(sql, "on").unwrap(),
            "-- 'pear\\tree'\n/* 'pear\\tree' /* nested */ */\n\
            CREATE VIEW \"果実\\箱\" AS SELECT E'pear\\\\tree', $body$'pear\\tree'$body$,\n\
            'pear', E'果実\\\\木', E'more\\\\fruit';\n"
        );
    }

    #[test]
    fn invalid_ddl_is_rejected_instead_of_returning_partial_sql() {
        assert!(matches!(
            portable_literals("SELECT 'unterminated", "on"),
            Err(Error::Catalog(_))
        ));
    }

    #[test]
    fn literals_escape_quotes_backslashes_and_sql_payloads() {
        assert_eq!(literal("pear").unwrap(), "E'pear'");
        assert_eq!(literal("O'Brien\\pear").unwrap(), "E'O''Brien\\\\pear'");
        assert_eq!(
            literal("'; DROP TABLE fruit; --").unwrap(),
            "E'''; DROP TABLE fruit; --'"
        );
        assert_eq!(literal("").unwrap(), "E''");
        assert_eq!(literal("果実\n").unwrap(), "E'果実\n'");
    }

    #[test]
    fn literals_reject_zero_bytes_instead_of_truncating_names() {
        assert!(matches!(literal("pear\0box"), Err(Error::Nul)));
    }

    #[test]
    fn alter_and_select_availability_matches_postgresql_object_kinds() {
        assert!(supports(Kind::Table, Script::Alter));
        assert!(supports(Kind::View, Script::Alter));
        assert!(supports(Kind::Function, Script::Alter));
        assert!(supports(Kind::Procedure, Script::Alter));
        assert!(!supports(Kind::MaterializedView, Script::Alter));
        assert!(!supports(Kind::Index, Script::Alter));
        assert!(!supports(Kind::Sequence, Script::Alter));
        assert!(!supports(Kind::Schema, Script::Alter));
        assert!(!supports(Kind::Database, Script::Create));
        assert!(!supports(Kind::Function, Script::Select));
    }
}
