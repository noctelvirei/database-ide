//! Table metadata assembled from ordered catalog rows without truncating composite keys.

use std::collections::BTreeMap;

use serde::Deserialize;
use sqlparser::dialect::MySqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::{Error, literal, queries, query, quote, rows};
use crate::objects::{Column, Constraint, ConstraintKind, Index, Kind, Object, ObjectId};
use crate::session::{Session, Value};

/// Reads live columns in declaration order after the caller has validated the node.
pub(super) async fn columns(
    session: &mut dyn Session,
    object: &Object,
) -> Result<Vec<Column>, Error> {
    query(
        session,
        &queries::COLUMNS.replace("{table}", &literal(&object.name)?),
    )
    .await
}

#[derive(Deserialize)]
struct IndexPart {
    name: String,
    column: Option<String>,
    expression: Option<String>,
    prefix: Option<u64>,
    descending: u8,
    non_unique: u8,
    method: String,
    visible: u8,
}

impl IndexPart {
    fn sql(&self) -> Result<String, Error> {
        let mut sql = match (&self.column, &self.expression) {
            (Some(column), _) => quote(column)?,
            (_, Some(expression)) => format!("({expression})"),
            _ => {
                return Err(Error::Catalog(
                    "Index part has no column or expression".into(),
                ));
            }
        };
        // A SPATIAL key part reports SUB_PART 32, but MySQL rejects a prefix on one.
        if let Some(prefix) = self.prefix.filter(|_| self.method != "SPATIAL") {
            sql.push_str(&format!("({prefix})"));
        }
        if self.descending != 0 {
            sql.push_str(" DESC");
        }
        Ok(sql)
    }
}

/// Reads all key parts, preserving expressions, prefix lengths, direction and visibility.
pub(super) async fn indexes(
    session: &mut dyn Session,
    table: &Object,
) -> Result<Vec<Index>, Error> {
    let parts: Vec<IndexPart> = query(
        session,
        &queries::INDEXES.replace("{table}", &literal(&table.name)?),
    )
    .await?;
    let mut grouped: BTreeMap<String, Vec<IndexPart>> = BTreeMap::new();
    for part in parts {
        grouped.entry(part.name.clone()).or_default().push(part);
    }
    let mut indexes = Vec::new();
    for (name, parts) in grouped {
        let first = &parts[0];
        let columns = parts
            .iter()
            .map(IndexPart::sql)
            .collect::<Result<Vec<_>, _>>()?;
        let qualified = format!("{}.{}", quote(&table.id.database)?, quote(&table.name)?);
        let keys = columns.join(", ");
        let method = if matches!(first.method.as_str(), "BTREE" | "HASH") {
            format!(" USING {}", first.method)
        } else {
            String::new()
        };
        let mut definition = if name == "PRIMARY" {
            format!("ALTER TABLE {qualified} ADD PRIMARY KEY ({keys}){method}")
        } else {
            let modifier = match first.method.as_str() {
                "FULLTEXT" => "FULLTEXT ",
                "RTREE" | "SPATIAL" => "SPATIAL ",
                _ if first.non_unique == 0 => "UNIQUE ",
                _ => "",
            };
            format!(
                "CREATE {modifier}INDEX {} ON {qualified} ({keys}){method}",
                quote(&name)?
            )
        };
        if first.method == "FULLTEXT" {
            definition.push_str(&fulltext_parser(session, &qualified, &name).await?);
        }
        if first.visible == 0 {
            definition.push_str(" INVISIBLE");
        }
        let hex = |value: &str| {
            value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>()
        };
        indexes.push(Index {
            object: Object {
                id: ObjectId {
                    database: table.id.database.clone(),
                    key: format!("index:{}:{}", hex(&table.name), hex(&name)),
                },
                schema: Some(table.id.database.clone()),
                name: name.clone(),
                kind: Kind::Index,
                system: false,
                signature: None,
            },
            columns,
            unique: first.non_unique == 0,
            method: first.method.to_lowercase(),
            definition,
            constraint_backed: first.non_unique == 0,
        });
    }
    Ok(indexes)
}

/// Reads the parser from SHOW's executable comment, which STATISTICS does not expose.
async fn fulltext_parser(
    session: &mut dyn Session,
    qualified: &str,
    name: &str,
) -> Result<String, Error> {
    let result = rows(session, &format!("SHOW CREATE TABLE {qualified}")).await?;
    let Some(Value::Text(definition)) = result.first().and_then(|row| row.get(1)) else {
        return Err(Error::Catalog(
            "Missing SHOW CREATE TABLE definition".into(),
        ));
    };
    let tokens = Tokenizer::new(&MySqlDialect {}, definition)
        .tokenize()
        .map_err(|error| Error::Catalog(error.to_string()))?
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let start = tokens
        .windows(3)
        .position(|tokens| {
            matches!((&tokens[0], &tokens[1], &tokens[2]),
            (Token::Word(fulltext), Token::Word(key), Token::Word(index))
            if fulltext.quote_style.is_none() && fulltext.value == "FULLTEXT"
                && key.quote_style.is_none() && key.value == "KEY" && index.value == name)
        })
        .ok_or_else(|| Error::Catalog("Missing FULLTEXT key in SHOW CREATE TABLE".into()))?;
    let mut depth = 0;
    // MySqlDialect expands versioned comments into their executable tokens.
    for tokens in tokens[start + 3..].windows(3) {
        match &tokens[0] {
            Token::LParen => depth += 1,
            Token::RParen if depth > 0 => depth -= 1,
            Token::Comma | Token::RParen if depth == 0 => break,
            Token::Word(with) if depth == 0 && with.value == "WITH" => {
                if let (Token::Word(parser), Token::Word(name)) = (&tokens[1], &tokens[2])
                    && parser.value == "PARSER"
                {
                    return Ok(format!(" WITH PARSER {}", quote(&name.value)?));
                }
            }
            _ => {}
        }
    }
    Ok(String::new())
}

#[derive(Deserialize)]
struct ForeignPart {
    name: String,
    column: String,
    database: String,
    table: String,
    target: String,
    update: String,
    delete: String,
}

/// Builds primary and unique constraints from indexes, then foreign keys and check clauses.
pub(super) async fn constraints(
    session: &mut dyn Session,
    table: &Object,
) -> Result<Vec<Constraint>, Error> {
    let mut constraints = Vec::new();
    for index in indexes(session, table).await? {
        if index.unique {
            let primary = index.object.name == "PRIMARY";
            constraints.push(Constraint {
                name: index.object.name,
                kind: if primary {
                    ConstraintKind::PrimaryKey
                } else {
                    ConstraintKind::Unique
                },
                definition: format!(
                    "{} ({})",
                    if primary { "PRIMARY KEY" } else { "UNIQUE" },
                    index.columns.join(", ")
                ),
            });
        }
    }
    let name = literal(&table.name)?;
    let parts: Vec<ForeignPart> =
        query(session, &queries::FOREIGN_KEYS.replace("{table}", &name)).await?;
    let mut grouped: BTreeMap<String, Vec<ForeignPart>> = BTreeMap::new();
    for part in parts {
        grouped.entry(part.name.clone()).or_default().push(part);
    }
    for (name, parts) in grouped {
        let first = &parts[0];
        let columns = parts
            .iter()
            .map(|part| quote(&part.column))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let targets = parts
            .iter()
            .map(|part| quote(&part.target))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let target = if first.database == table.id.database {
            quote(&first.table)?
        } else {
            format!("{}.{}", quote(&first.database)?, quote(&first.table)?)
        };
        constraints.push(Constraint {
            name,
            kind: ConstraintKind::ForeignKey,
            definition: format!(
                "FOREIGN KEY ({columns}) REFERENCES {target} ({targets}) ON DELETE {} ON UPDATE {}",
                first.delete, first.update
            ),
        });
    }
    constraints
        .extend(query::<Constraint>(session, &queries::CHECKS.replace("{table}", &name)).await?);
    constraints.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(constraints)
}
