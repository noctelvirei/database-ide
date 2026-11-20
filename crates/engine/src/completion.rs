//! Caret context and catalog-name spelling for autocomplete, without server access.

use std::ops::Range;

use sqlparser::dialect::{Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, Tokenizer, Whitespace};

use crate::danger::Engine;

mod keywords;

/// The identifier component an accepted suggestion replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// Half-open byte range, including delimiters and any suffix after the caret.
    pub range: Range<usize>,
    /// Unquoted prefix up to the caret, with doubled delimiters decoded.
    pub text: String,
}

/// A table, view or CTE named in the caret's statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The schema component immediately before the name, when written.
    pub schema: Option<String>,
    /// Unquoted object name; no catalog lookup is performed.
    pub name: String,
    /// Unquoted explicit or implicit alias, when given.
    pub alias: Option<String>,
    /// CTE columns must be inferred by the caller rather than read from the catalog.
    pub is_cte: bool,
}

/// Partial SQL knowledge available to the editor at one caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// The component to replace and the prefix used to filter suggestions.
    pub fragment: Fragment,
    /// Unquoted components before the last dot, from left to right.
    pub qualifier: Vec<String>,
    /// Visible named sources and CTE declarations in source order.
    pub tables: Vec<Table>,
    /// False inside a literal, comment or an unrecognized unfinished token.
    pub suggest: bool,
}

/// Inspects SQL at a byte offset without parsing a complete statement. Offsets should be
/// character boundaries; out-of-range offsets are clamped and rounded down. Malformed SQL
/// yields whatever context can be recovered. This never runs SQL or reads the catalog.
pub fn context(text: &str, offset: usize, engine: Engine) -> Context {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let tokens = lex(text, dialect(engine));
    let mut result = Context {
        fragment: Fragment {
            range: offset..offset,
            text: String::new(),
        },
        qualifier: Vec::new(),
        tables: Vec::new(),
        suggest: true,
    };
    let current = tokens.iter().position(|token| {
        token.range.start <= offset && offset <= token.range.end && token.name().is_some()
    });
    if let Some(index) = current {
        let token = &tokens[index];
        result.fragment.range = token.range.clone();
        result.fragment.text = if offset == token.range.end && !token.open {
            token.name().unwrap_or_default().to_owned()
        } else {
            decode(&text[token.range.start..offset], token.quote())
        };
    }
    for token in &tokens {
        if token.range.start < offset
            && (offset < token.range.end || (offset == token.range.end && token.open))
            && token.suppressed
        {
            result.suggest = false;
        }
    }
    let before = current.unwrap_or_else(|| tokens.partition_point(|t| t.range.end <= offset));
    let mut index = before;
    while index >= 2 && matches!(tokens[index - 1].token, Token::Period) {
        let Some(name) = tokens[index - 2].name() else {
            break;
        };
        result.qualifier.push(name.to_owned());
        index -= 2;
    }
    result.qualifier.reverse();

    let batch = if engine == Engine::SqlServer {
        crate::mssql::batches::split(text).ok().and_then(|batches| {
            batches.into_iter().find_map(|batch| {
                let start = batch.sql.as_ptr() as usize - text.as_ptr() as usize;
                let range = start..start + batch.sql.len();
                (range.start <= offset && offset <= range.end).then_some(range)
            })
        })
    } else {
        Some(0..text.len())
    };
    if let Some(mut range) = batch {
        for token in &tokens {
            if matches!(token.token, Token::SemiColon) && range.contains(&token.range.start) {
                if token.range.end <= offset {
                    range.start = token.range.end;
                } else {
                    range.end = token.range.start;
                    break;
                }
            }
        }
        let statement: Vec<_> = tokens
            .iter()
            .filter(|t| range.contains(&t.range.start) && !matches!(t.token, Token::Whitespace(_)))
            .collect();
        result.tables = tables(&statement, offset, engine);
    }
    result
}

/// Spells one catalog component for insertion, escaping delimiters as needed. Empty names
/// and NUL return None. Reserved words use the supported server versions' keyword lists.
pub fn quote_name(name: &str, engine: Engine) -> Option<String> {
    let dialect = dialect(engine);
    let tokens = lex(name, dialect);
    let plain = matches!(tokens.as_slice(), [token]
        if token.range == (0..name.len()) && !token.open
        && matches!(&token.token, Token::Word(word)
            if word.quote_style.is_none() && !keywords::reserved(name, engine)));
    if plain && (engine != Engine::PostgreSql || name == name.to_lowercase()) {
        Some(name.to_owned())
    } else {
        crate::sql::identifier(name, engine).ok()
    }
}

/// Whether a complete typed identifier component refers to a catalog component. PostgreSQL
/// folds bare names to lower case; quoted names compare exactly. The other engines ignore
/// Unicode case, independently of server collation. Malformed names do not match.
pub fn names_equal(typed: &str, catalog: &str, engine: Engine) -> bool {
    let tokens = lex(typed, dialect(engine));
    let [token] = tokens.as_slice() else {
        return false;
    };
    let Some(name) = token.name() else {
        return false;
    };
    if token.open || token.range != (0..typed.len()) {
        return false;
    }
    match engine {
        Engine::PostgreSql if token.quote().is_some() => name == catalog,
        Engine::PostgreSql => name.to_lowercase() == catalog,
        _ => name.to_lowercase() == catalog.to_lowercase(),
    }
}

/// The tokenizer's SQL syntax rules for a connection's engine.
fn dialect(engine: Engine) -> &'static dyn Dialect {
    match engine {
        Engine::SqlServer => &MsSqlDialect {},
        Engine::PostgreSql => &PostgreSqlDialect {},
        Engine::MySql => &MySqlDialect {},
    }
}

/// A recovered token with its original byte extent, including delimiters.
struct Lexeme {
    /// Decoded token; unfinished identifiers are represented as words.
    token: Token,
    /// Half-open byte extent in the caller's original text.
    range: Range<usize>,
    /// Carets inside this token must not trigger suggestions.
    suppressed: bool,
    /// The token continues through end of input rather than ending at a delimiter.
    open: bool,
}

impl Lexeme {
    /// Decoded identifier or keyword spelling, without catalog case folding.
    fn name(&self) -> Option<&str> {
        match &self.token {
            Token::Word(word) => Some(&word.value),
            _ => None,
        }
    }

    /// Opening delimiter for a quoted identifier.
    fn quote(&self) -> Option<char> {
        match &self.token {
            Token::Word(word) => word.quote_style,
            _ => None,
        }
    }

    /// Matches syntax words without interpreting quoted identifiers as keywords.
    fn is(&self, keyword: &str) -> bool {
        self.quote().is_none()
            && self
                .name()
                .is_some_and(|name| name.eq_ignore_ascii_case(keyword))
    }

    /// Whether the token can name a source or alias at this position.
    fn identifier(&self, engine: Engine) -> bool {
        self.name()
            .is_some_and(|name| self.quote().is_some() || !keywords::reserved(name, engine))
    }

    /// Catalog spelling of an identifier, including PostgreSQL's bare-name folding.
    fn catalog_name(&self, engine: Engine) -> Option<String> {
        self.name().map(|name| {
            if engine == Engine::PostgreSql && self.quote().is_none() {
                name.to_lowercase()
            } else {
                name.to_owned()
            }
        })
    }
}

/// Decodes an unfinished identifier prefix; raw includes the opener but no final closer.
fn decode(raw: &str, quote: Option<char>) -> String {
    let Some(quote) = quote else {
        return raw.to_owned();
    };
    let end = if quote == '[' { ']' } else { quote };
    let body = raw.strip_prefix(quote).unwrap_or(raw);
    body.replace(&format!("{end}{end}"), &end.to_string())
}

/// Retains successfully tokenized input and recovers an unfinished identifier at EOF.
fn lex(text: &str, dialect: &dyn Dialect) -> Vec<Lexeme> {
    let mut locations = Vec::new();
    let mut location = Location::new(1, 1);
    for (byte, ch) in text.char_indices() {
        locations.push((location, byte));
        if ch == '\n' {
            location.line += 1;
            location.column = 1;
        } else {
            location.column += 1;
        }
    }
    locations.push((location, text.len()));
    let byte = |location| {
        locations
            .binary_search_by_key(&location, |(loc, _)| *loc)
            .ok()
            .map(|i| locations[i].1)
    };
    let mut parsed = Vec::new();
    let failed = Tokenizer::new(dialect, text)
        .tokenize_with_location_into_buf(&mut parsed)
        .is_err();
    let mut consumed = 0;
    let mut tokens = Vec::new();
    for token in parsed {
        let (Some(start), Some(end)) = (byte(token.span.start), byte(token.span.end)) else {
            continue;
        };
        let Some(raw) = text.get(start..end) else {
            continue;
        };
        consumed = consumed.max(end);
        let comment = matches!(
            token.token,
            Token::Whitespace(
                Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_)
            )
        );
        let literal = !matches!(token.token, Token::Word(_))
            && (raw.contains('\'') || raw.starts_with('"') || raw.starts_with('$'));
        if comment || !matches!(token.token, Token::Whitespace(_)) {
            let open = matches!(
                token.token,
                Token::Whitespace(Whitespace::SingleLineComment { .. })
            ) && !raw.ends_with('\n');
            tokens.push(Lexeme {
                token: token.token,
                range: start..end,
                suppressed: comment || literal,
                open,
            });
        }
    }
    if failed && consumed < text.len() {
        let raw = &text[consumed..];
        let quote = raw
            .chars()
            .next()
            .filter(|ch| dialect.is_delimited_identifier_start(*ch));
        let token = match quote {
            Some(quote) => Token::make_word(&decode(raw, Some(quote)), Some(quote)),
            None => Token::Char('\0'),
        };
        tokens.push(Lexeme {
            token,
            range: consumed..text.len(),
            suppressed: quote.is_none(),
            open: true,
        });
    }
    tokens.sort_by_key(|t| t.range.start);
    tokens
}

/// A named source and the next unread token after its optional alias.
struct Object {
    /// Catalog components and alias recovered from the source.
    table: Table,
    /// Token index immediately following the source.
    next: usize,
}

/// Reads a dotted source and its alias; incomplete dotted names return None.
fn object(tokens: &[&Lexeme], mut index: usize, engine: Engine) -> Option<Object> {
    if !tokens.get(index)?.identifier(engine) {
        return None;
    }
    let mut parts = vec![tokens.get(index)?.catalog_name(engine)?];
    index += 1;
    while tokens
        .get(index)
        .is_some_and(|t| matches!(t.token, Token::Period))
    {
        parts.push(tokens.get(index + 1)?.catalog_name(engine)?);
        index += 2;
    }
    let name = parts.pop()?;
    let schema = parts.pop();
    if tokens.get(index).is_some_and(|t| t.is("AS")) {
        index += 1;
    }
    let alias = tokens.get(index).and_then(|token| {
        if token.identifier(engine) && !matches!(&token.token, Token::Word(word)
            if word.quote_style.is_none() && (sqlparser::keywords::RESERVED_FOR_TABLE_ALIAS.contains(&word.keyword)
                || matches!(word.keyword, Keyword::AS | Keyword::VALUES | Keyword::INTO))) {
            index += 1;
            token.catalog_name(engine)
        } else {
            None
        }
    });
    Some(Object {
        table: Table {
            schema,
            name,
            alias,
            is_cte: false,
        },
        next: index,
    })
}

/// Collects named sources in source order, excluding nested queries outside the caret.
fn tables(tokens: &[&Lexeme], offset: usize, engine: Engine) -> Vec<Table> {
    let mut closes = vec![usize::MAX; tokens.len()];
    let mut openings = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.token {
            Token::LParen => openings.push(index),
            Token::RParen => {
                if let Some(start) = openings.pop() {
                    closes[start] = token.range.start;
                }
            }
            _ => {}
        }
    }
    let mut result: Vec<Table> = Vec::new();
    let mut ctes: Vec<String> = Vec::new();
    let mut cte_name = tokens.first().is_some_and(|t| t.is("WITH"));
    let mut cte_list = cte_name;
    let mut depth = 0usize;
    let mut visible = vec![true];
    let mut from = vec![false];
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if matches!(token.token, Token::LParen) {
            visible.push(visible[depth] && token.range.end <= offset && offset <= closes[index]);
            depth += 1;
            from.push(false);
        } else if matches!(token.token, Token::RParen) && depth > 0 {
            visible.pop();
            depth -= 1;
            from.pop();
        }
        if cte_name && depth == 0 && index > 0 && !token.is("RECURSIVE") {
            if let Some(name) = token.catalog_name(engine) {
                ctes.push(name.clone());
                result.push(Table {
                    schema: None,
                    name,
                    alias: None,
                    is_cte: true,
                });
            }
            cte_name = false;
        }
        if depth == 0
            && ["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE"]
                .iter()
                .any(|word| token.is(word))
        {
            cte_list = false;
        }
        if cte_list
            && depth == 0
            && matches!(token.token, Token::Comma)
            && index > 0
            && matches!(tokens[index - 1].token, Token::RParen)
            && !ctes.is_empty()
        {
            cte_name = true;
        }
        let source = token.is("FROM")
            || token.is("JOIN")
            || (token.is("UPDATE") && !tokens.get(index + 1).is_some_and(|t| t.is("SET")))
            || token.is("INTO")
            || token.is("USING")
            || (matches!(token.token, Token::Comma) && from[depth]);
        if token.is("FROM") {
            from[depth] = true;
        }
        if [
            "WHERE",
            "SET",
            "VALUES",
            "RETURNING",
            "GROUP",
            "ORDER",
            "HAVING",
            "UNION",
            "SELECT",
        ]
        .iter()
        .any(|word| token.is(word))
        {
            from[depth] = false;
        }
        if visible[depth]
            && source
            && let Some(Object { mut table, next }) = object(tokens, index + 1, engine)
        {
            table.is_cte = table.schema.is_none()
                && ctes
                    .iter()
                    .any(|name| names_equal(&tokens[index + 1].token.to_string(), name, engine));
            if !result.contains(&table) {
                result.push(table);
            }
            index = next;
            continue;
        }
        index += 1;
    }
    result
}
