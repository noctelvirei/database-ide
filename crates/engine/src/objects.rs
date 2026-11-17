//! Lazy object explorer metadata shared by engines; SQL generation stays engine-specific.

use serde::Deserialize;

/// MySQL catalog browsing, search and Script as over the common session interface.
pub mod mysql;

/// PostgreSQL catalog browsing, search and Script as over the common session interface.
pub mod postgres;

/// An object's place in the explorer, independent of catalog storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A database on the connected server.
    Database,
    /// A namespace in one database.
    Schema,
    /// A table, including partitioned tables.
    Table,
    /// A query-backed view.
    View,
    /// A stored view result.
    MaterializedView,
    /// A callable function; overloads have distinct identities.
    Function,
    /// A callable procedure; overloads have distinct identities.
    Procedure,
    /// A number generator.
    Sequence,
    /// An index belonging to a relation.
    Index,
}

/// Identity within one connected server; refresh after dropping or recreating objects.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ObjectId {
    /// Database in which the opaque key is meaningful.
    pub database: String,
    /// Engine-owned catalog key; callers must not interpret it or transfer it to another server.
    pub key: String,
}

/// One lazy tree node or search hit; fetching it never fetches its children.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Object {
    /// Stable across renames, but only for this object's lifetime on this server.
    pub id: ObjectId,
    /// Namespace, absent for databases and schemas.
    pub schema: Option<String>,
    /// Unquoted name for display and filtering.
    pub name: String,
    /// Determines which child lists and script forms are available.
    pub kind: Kind,
    /// True for engine-owned namespaces and their objects.
    pub system: bool,
    /// Displayable identity arguments for overloaded routines; absent for other kinds.
    pub signature: Option<String>,
}

/// How an identity column accepts explicit values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Identity {
    /// Explicit inserts require an override.
    Always,
    /// Explicit values override generated values.
    ByDefault,
}

/// A computed column's expression and persistence.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Generated {
    /// Server SQL expression, without GENERATED syntax.
    pub expression: String,
    /// False means evaluated on read rather than stored.
    pub stored: bool,
}

/// One live column, returned in declaration order; dropped and hidden columns are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Column {
    /// Unquoted column name.
    pub name: String,
    /// Displayable server type, including length or precision modifiers.
    pub type_name: String,
    /// Whether the declaration permits NULL.
    pub nullable: bool,
    /// Server SQL expression; absent for identity and generated columns.
    pub default: Option<String>,
    /// Explicit-value policy, when this is an identity column.
    pub identity: Option<Identity>,
    /// Computation, when this is a generated column.
    pub generated: Option<Generated>,
}

/// The rules shown in a table's keys and constraints list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// The row's primary identity.
    PrimaryKey,
    /// A uniqueness rule, distinct from a standalone unique index.
    Unique,
    /// A reference to another table's key.
    ForeignKey,
    /// A boolean rule checked for each row.
    Check,
    /// A rule excluding conflicting index entries.
    Exclusion,
}

/// A named table rule; its definition preserves engine-specific details.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Constraint {
    /// Unquoted constraint name.
    pub name: String,
    /// Rule category for the explorer.
    pub kind: ConstraintKind,
    /// Server SQL, without the CONSTRAINT name prefix.
    pub definition: String,
}

/// An index with enough metadata for display and Script as.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Index {
    /// Explorer node usable with Script as CREATE.
    pub object: Object,
    /// Key columns or SQL expressions, followed by included columns, in index order.
    pub columns: Vec<String>,
    /// Whether key values must be unique.
    pub unique: bool,
    /// Server access method, such as btree.
    pub method: String,
    /// Server CREATE INDEX statement.
    pub definition: String,
    /// True when the table's constraint creates this index implicitly.
    pub constraint_backed: bool,
}

/// An editor-ready script form; availability depends on the engine and object kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Recreate the object, without its data or external dependencies.
    Create,
    /// Replace a routine or view, or provide an editable table alteration skeleton.
    Alter,
    /// Read all visible columns with a bounded row count.
    Select,
}

/// A case-insensitive literal name substring and optional kind filter.
#[derive(Debug, Clone)]
pub struct Search {
    /// Percent and underscore are ordinary characters, not wildcard syntax.
    pub name: String,
    /// Empty selects all searchable kinds; databases are enumerated separately.
    pub kinds: Vec<Kind>,
    /// Maximum hits across the entire operation; zero returns no hits.
    pub limit: usize,
}

/// One database that refused a connection or failed during catalog search.
#[derive(Debug)]
pub struct SkippedDatabase {
    /// Database that could not be searched.
    pub database: String,
    /// Connector or catalog diagnostic for display.
    pub reason: String,
}

/// Bounded cross-database hits plus failures; one refused database does not lose other hits.
#[derive(Debug)]
pub struct SearchReport {
    /// Hits in database-name order, then schema, name, kind and identity order.
    pub objects: Vec<Object>,
    /// Failures encountered before the global hit limit was reached.
    pub skipped: Vec<SkippedDatabase>,
}
