//! SQL Server DDL assembly and module-header rewriting without modifying module bodies.

use serde::Deserialize;
use sqlparser::dialect::MsSqlDialect;
use sqlparser::tokenizer::{Location, Token, Tokenizer};

use super::{
    Error, catalog, constraint_rows, index_rows, qualified, queries, query, quote, resolve,
    supports,
};
use crate::objects::{Kind, Object, Script};
use crate::session::Session;

/// Produces SQL only after re-resolving the catalog identity and checking the requested form.
pub(super) async fn script(
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
    let key = resolve(session, object).await?;
    match (object.kind, form) {
        (Kind::Schema, Script::Create) => {
            Ok(format!("CREATE SCHEMA {};\nGO\n", quote(&object.name)))
        }
        (Kind::Table, Script::Alter) => Ok(format!(
            "-- ALTER TABLE {}\n--     ADD [column_name] data_type;\n",
            qualified(object)?
        )),
        (_, Script::Select) => {
            let columns = super::columns(session, object)
                .await?
                .iter()
                .map(|c| quote(&c.name))
                .collect::<Vec<_>>()
                .join(", ");
            Ok(format!(
                "SELECT TOP (1000) {columns}\nFROM {};\n",
                qualified(object)?
            ))
        }
        (Kind::Table, Script::Create) => table(session, object, key).await,
        (Kind::Index, Script::Create) => {
            let parent = catalog(session, &format!("[key] = N'object:{key}'"), Some(1))
                .await?
                .into_iter()
                .next()
                .ok_or(Error::NotFound)?;
            let index = index_rows(session, key)
                .await?
                .into_iter()
                .find(|i| object.id.key == format!("index:{key}:{}", i.index_id))
                .ok_or(Error::NotFound)?;
            let index = index.into_index(&parent, key)?;
            if index.constraint_backed {
                return Err(Error::Catalog(
                    "Script the owning table to recreate a constraint-backed index".into(),
                ));
            }
            Ok(format!("{INDEX_SETTINGS}\nGO\n{}\nGO\n", index.definition))
        }
        _ => {
            let modules: Vec<Module> = query(
                session,
                &queries::MODULES.replace("{filter}", &format!("o.object_id = {key}")),
            )
            .await?;
            modules.first().ok_or(Error::NotFound)?.script(form, None)
        }
    }
}

const INDEX_SETTINGS: &str = "SET ANSI_NULLS ON;\nSET QUOTED_IDENTIFIER ON;\nSET ANSI_PADDING ON;\nSET ANSI_WARNINGS ON;\nSET ARITHABORT ON;\nSET CONCAT_NULL_YIELDS_NULL ON;\nSET NUMERIC_ROUNDABORT OFF;";

async fn table(session: &mut dyn Session, object: &Object, key: i32) -> Result<String, Error> {
    #[derive(Deserialize)]
    struct Features {
        unsupported: bool,
    }
    let features: Vec<Features> = query(session, &format!(r#"SELECT (SELECT CAST(CASE WHEN
        t.is_memory_optimized = 1 OR t.temporal_type <> 0 OR t.is_filetable = 1 OR t.ledger_type <> 0
        OR t.is_node = 1 OR t.is_edge = 1
        OR EXISTS (SELECT 1 FROM sys.columns WHERE object_id = t.object_id AND is_hidden = 1)
        OR EXISTS (SELECT 1 FROM sys.indexes i JOIN sys.data_spaces d ON d.data_space_id = i.data_space_id
            WHERE i.object_id = t.object_id AND d.type = 'PS')
        THEN 1 ELSE 0 END AS bit) AS unsupported FOR JSON PATH, WITHOUT_ARRAY_WRAPPER)
        FROM sys.tables t WHERE t.object_id = {key}"#)).await?;
    if features.first().ok_or(Error::NotFound)?.unsupported {
        return Err(Error::Catalog(
            "Script as CREATE does not support this table's special storage or generated columns"
                .into(),
        ));
    }
    let columns = super::column_details(session, object).await?;
    let mut declarations = Vec::new();
    for details in columns {
        if details.unsupported {
            return Err(Error::Catalog(format!(
                "Script as CREATE does not support special attributes of column {}",
                details.column.name
            )));
        }
        let column = details.column;
        let mut sql = quote(&column.name);
        if let Some(generated) = column.generated {
            sql.push_str(&format!(
                " AS {}{}",
                generated.expression,
                if generated.stored { " PERSISTED" } else { "" }
            ));
            if generated.stored && !column.nullable {
                sql.push_str(" NOT NULL");
            }
        } else {
            sql.push_str(&format!(" {}", column.type_name));
            if let Some(collation) = details.collation {
                sql.push_str(&format!(" COLLATE {}", quote(&collation)));
            }
            if let Some(seed) = details.identity_seed {
                let increment = details
                    .identity_increment
                    .ok_or_else(|| Error::Catalog("Missing identity increment".into()))?;
                sql.push_str(&format!(" IDENTITY({seed},{increment})"));
                if details.identity_not_for_replication == Some(true) {
                    sql.push_str(" NOT FOR REPLICATION");
                }
            }
            if details.rowguidcol {
                sql.push_str(" ROWGUIDCOL");
            }
            if details.sparse {
                sql.push_str(" SPARSE");
            }
            sql.push_str(if column.nullable {
                " NULL"
            } else {
                " NOT NULL"
            });
            if let Some(default) = column.default {
                let name = details
                    .default_constraint
                    .ok_or_else(|| Error::Catalog("Missing default constraint name".into()))?;
                sql.push_str(&format!(" CONSTRAINT {} DEFAULT {default}", quote(&name)));
            }
        }
        declarations.push(sql);
    }
    let constraints = constraint_rows(session, key).await?;
    let name = qualified(object)?;
    let mut sql = format!(
        "{INDEX_SETTINGS}\nGO\nCREATE TABLE {name} (\n    {}\n);\nGO\n",
        declarations.join(",\n    ")
    );
    let mut foreign_keys = String::new();
    for row in constraints {
        let constraint = row.constraint;
        // Self-references can target a standalone unique index, created below.
        let output = if constraint.kind == crate::objects::ConstraintKind::ForeignKey {
            &mut foreign_keys
        } else {
            &mut sql
        };
        output.push_str(&format!(
            "ALTER TABLE {name} {}ADD CONSTRAINT {} {};\nGO\n",
            if matches!(
                constraint.kind,
                crate::objects::ConstraintKind::Check | crate::objects::ConstraintKind::ForeignKey
            ) {
                if row.untrusted {
                    "WITH NOCHECK "
                } else {
                    "WITH CHECK "
                }
            } else {
                ""
            },
            quote(&constraint.name),
            constraint.definition
        ));
        if row.disabled {
            output.push_str(&format!(
                "ALTER TABLE {name} NOCHECK CONSTRAINT {};\nGO\n",
                quote(&constraint.name)
            ));
        }
    }
    let mut disabled_indexes = Vec::new();
    for mut index in index_rows(session, key).await? {
        if index.disabled {
            disabled_indexes.push(index.name.clone());
            index.disabled = false;
        }
        if !index.constraint_backed {
            sql.push_str(&format!(
                "{}\nGO\n",
                index.into_index(object, key)?.definition
            ));
        }
    }
    sql.push_str(&foreign_keys);
    let triggers: Vec<Module> = query(
        session,
        &queries::MODULES.replace(
            "{filter}",
            &format!("o.parent_object_id = {key} AND o.type = 'TR'"),
        ),
    )
    .await?;
    for trigger in triggers {
        sql.push_str(&trigger.script(Script::Create, Some(&name))?);
    }
    for index in disabled_indexes {
        sql.push_str(&format!(
            "ALTER INDEX {} ON {name} DISABLE;\nGO\n",
            quote(&index)
        ));
    }
    Ok(sql)
}

#[derive(Deserialize)]
struct Module {
    name: String,
    schema: String,
    definition: Option<String>,
    ansi_nulls: Option<bool>,
    quoted_identifier: Option<bool>,
    disabled: bool,
}

impl Module {
    fn script(&self, form: Script, table: Option<&str>) -> Result<String, Error> {
        let definition = self.definition.as_deref().ok_or_else(|| {
            Error::Catalog("Module definition is encrypted or not visible to this login".into())
        })?;
        let name = format!("{}.{}", quote(&self.schema), quote(&self.name));
        let body = module_header(definition, &name, form)?;
        let ansi = self
            .ansi_nulls
            .ok_or_else(|| Error::Catalog("Missing ANSI_NULLS setting".into()))?;
        let quoted = self
            .quoted_identifier
            .ok_or_else(|| Error::Catalog("Missing QUOTED_IDENTIFIER setting".into()))?;
        let mut sql = format!(
            "SET ANSI_NULLS {};\nGO\nSET QUOTED_IDENTIFIER {};\nGO\n{body}\nGO\n",
            if ansi { "ON" } else { "OFF" },
            if quoted { "ON" } else { "OFF" }
        );
        if self.disabled {
            sql.push_str(&format!(
                "DISABLE TRIGGER {name} ON {};\nGO\n",
                table.ok_or_else(|| Error::Catalog("Missing trigger parent".into()))?
            ));
        }
        Ok(sql)
    }
}

// Rebuild only the module header: ALTER sources and sp_rename leave stale names in sql_modules.
fn module_header(sql: &str, name: &str, form: Script) -> Result<String, Error> {
    let tokens = Tokenizer::new(&MsSqlDialect {}, sql)
        .with_unescape(false)
        .tokenize_with_location()
        .map_err(|e| Error::Catalog(e.to_string()))?;
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect();
    let word = |n: usize, expected: &str| {
        tokens.get(n).is_some_and(|t| matches!(&t.token, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(expected)))
    };
    if !word(0, "CREATE") && !word(0, "ALTER") {
        return Err(Error::Catalog("Unrecognized module header".into()));
    }
    let kind = if word(0, "CREATE") && word(1, "OR") && word(2, "ALTER") {
        3
    } else {
        1
    };
    if !["VIEW", "PROCEDURE", "PROC", "FUNCTION", "TRIGGER"]
        .iter()
        .any(|w| word(kind, w))
    {
        return Err(Error::Catalog("Unrecognized module kind".into()));
    }
    let mut end = kind + 1;
    if !tokens
        .get(end)
        .is_some_and(|t| matches!(t.token, Token::Word(_)))
    {
        return Err(Error::Catalog("Missing module name".into()));
    }
    if tokens
        .get(end + 1)
        .is_some_and(|t| t.token == Token::Period)
    {
        end += 2;
        if !tokens
            .get(end)
            .is_some_and(|t| matches!(t.token, Token::Word(_)))
        {
            return Err(Error::Catalog("Missing qualified module name".into()));
        }
    }
    let start = offset(sql, tokens[0].span.start)?;
    let tail = offset(sql, tokens[end].span.end)?;
    Ok(format!(
        "{}{} {} {}{}",
        &sql[..start],
        if form == Script::Alter {
            "CREATE OR ALTER"
        } else {
            "CREATE"
        },
        tokens[kind].token,
        name,
        &sql[tail..]
    ))
}

fn offset(sql: &str, target: Location) -> Result<usize, Error> {
    let mut location = Location::new(1, 1);
    for (offset, ch) in sql.char_indices() {
        if location == target {
            return Ok(offset);
        }
        if ch == '\n' {
            location.line += 1;
            location.column = 1;
        } else {
            location.column += 1;
        }
    }
    if location == target {
        Ok(sql.len())
    } else {
        Err(Error::Catalog("Invalid module token position".into()))
    }
}
