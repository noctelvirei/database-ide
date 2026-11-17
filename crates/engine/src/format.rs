//! SQL layout with byte-preserving literals, identifiers and comments.

use sqlparser::dialect::{Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::tokenizer::{Location, Token, Tokenizer, TokenizerError, Whitespace};

use crate::danger::Engine;

/// A refusal to format; callers should retain the original input on either failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input or generated SQL cannot be tokenized; includes the source line and column.
    #[error("cannot tokenize SQL: {0}")]
    Tokenize(#[from] TokenizerError),
    /// Formatting changed a token or the newline separating adjacent string literals.
    #[error("formatting would change SQL tokens; keep the original text")]
    ChangedTokens,
}

/// Formats a batch or fragment without server access, using four-space indentation and
/// an 80-character compact layout. Only whitespace and listed structural keyword case change.
/// Returns an error if tokenization fails or the output fails the token-preservation guard;
/// the borrowed input is never changed. Unrecognised statements retain their original layout.
pub fn format(text: &str, engine: Engine) -> Result<String, Error> {
    let dialect: &dyn Dialect = match engine {
        Engine::SqlServer => &MsSqlDialect {},
        Engine::PostgreSql => &PostgreSqlDialect {},
        Engine::MySql => &MySqlDialect {},
    };
    let tokens = lex(text, dialect)?;
    let mut output = String::new();
    let mut start = 0;
    let mut depth: usize = 0;
    for (index, token) in tokens.iter().enumerate() {
        if is_go(&tokens, index, engine) {
            append_statement(&mut output, &tokens[start..index]);
            newline(&mut output, 1);
            output.push_str(token.raw);
            newline(&mut output, 1);
            start = index + 1;
        } else {
            match token.token {
                Token::LParen => depth += 1,
                Token::RParen => depth = depth.saturating_sub(1),
                Token::SemiColon if depth == 0 => {
                    append_statement(&mut output, &tokens[start..=index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    append_statement(&mut output, &tokens[start..]);
    let checked = lex(&output, dialect)?;
    if tokens.len() != checked.len()
        || tokens.iter().zip(&checked).any(|(a, b)| {
            a.printed() != b.printed()
                || std::mem::discriminant(&a.token) != std::mem::discriminant(&b.token)
        })
        || tokens.windows(2).zip(checked.windows(2)).any(|(a, b)| {
            a[0].is_string() && a[1].is_string() && has_newline(a[1].gap) != has_newline(b[1].gap)
        })
    {
        return Err(Error::ChangedTokens);
    }
    Ok(output)
}

struct Lexeme<'a> {
    token: Token,
    raw: &'a str,
    gap: &'a str,
    keyword: Option<&'static str>,
}

impl Lexeme<'_> {
    fn printed(&self) -> &str {
        self.keyword.unwrap_or(self.raw)
    }

    fn is_string(&self) -> bool {
        matches!(
            self.token,
            Token::SingleQuotedString(_)
                | Token::DoubleQuotedString(_)
                | Token::DollarQuotedString(_)
                | Token::NationalStringLiteral(_)
                | Token::EscapedStringLiteral(_)
                | Token::UnicodeStringLiteral(_)
                | Token::HexStringLiteral(_)
                | Token::SingleQuotedByteStringLiteral(_)
                | Token::DoubleQuotedByteStringLiteral(_)
        )
    }

    fn line_comment(&self) -> bool {
        matches!(
            self.token,
            Token::Whitespace(Whitespace::SingleLineComment { .. })
        )
    }
}

// Deliberately excludes contextual words and type/function names, even if sqlparser calls them keywords.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "BY",
    "HAVING",
    "ORDER",
    "JOIN",
    "ON",
    "AND",
    "OR",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "ALTER",
    "DROP",
    "AS",
    "DISTINCT",
    "ALL",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "IN",
    "IS",
    "NULL",
    "NOT",
    "LIKE",
    "BETWEEN",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "WITH",
];

fn lex<'a>(text: &'a str, dialect: &dyn Dialect) -> Result<Vec<Lexeme<'a>>, Error> {
    let tokens = Tokenizer::new(dialect, text)
        .with_unescape(false)
        .tokenize_with_location()?;
    // These constructs have synthetic or reordered spans; retain the entire input verbatim.
    if (dialect.is::<MySqlDialect>() && text.contains("/*!"))
        || tokens
            .iter()
            .any(|token| matches!(token.token, Token::CopyData { .. }))
    {
        return Ok(vec![Lexeme {
            token: Token::Char('\0'),
            raw: text,
            gap: "",
            keyword: None,
        }]);
    }
    let mut locations = Vec::with_capacity(text.chars().count() + 1);
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
    let offset = |location| {
        locations
            .binary_search_by_key(&location, |(location, _)| *location)
            .map(|index| locations[index].1)
            .map_err(|_| Error::ChangedTokens)
    };
    let mut result = Vec::new();
    let mut previous_end = 0;
    for token in tokens {
        if matches!(
            token.token,
            Token::Whitespace(Whitespace::Space | Whitespace::Tab | Whitespace::Newline)
        ) {
            continue;
        }
        let start = offset(token.span.start)?;
        let end = offset(token.span.end)?;
        let raw = text.get(start..end).ok_or(Error::ChangedTokens)?;
        let keyword = match &token.token {
            Token::Word(word) if word.quote_style.is_none() => KEYWORDS
                .iter()
                .copied()
                .find(|keyword| raw.eq_ignore_ascii_case(keyword)),
            _ => None,
        };
        result.push(Lexeme {
            token: token.token,
            raw,
            gap: text.get(previous_end..start).ok_or(Error::ChangedTokens)?,
            keyword,
        });
        previous_end = end;
    }
    Ok(result)
}

fn has_newline(text: &str) -> bool {
    text.contains(['\r', '\n'])
}

fn is_go(tokens: &[Lexeme<'_>], index: usize, engine: Engine) -> bool {
    let token = &tokens[index];
    engine == Engine::SqlServer
        && matches!(&token.token, Token::Word(word) if word.quote_style.is_none())
        && token.raw.eq_ignore_ascii_case("GO")
        && (index == 0 || has_newline(token.gap))
        && (index + 1 == tokens.len() || has_newline(tokens[index + 1].gap))
}

fn newline(output: &mut String, count: usize) {
    if !output.is_empty() {
        let existing = output.chars().rev().take_while(|ch| *ch == '\n').count();
        for _ in existing..count {
            output.push('\n');
        }
    }
}

fn append_statement(output: &mut String, tokens: &[Lexeme<'_>]) {
    if tokens.is_empty() {
        return;
    }
    newline(output, 2);
    let compact = inline(tokens);
    if compact.chars().count() <= 80 && !has_newline(&compact) {
        output.push_str(&compact);
    } else if tokens
        .iter()
        .find(|token| !matches!(token.token, Token::Whitespace(_)))
        .is_some_and(|token| {
            matches!(
                token.keyword,
                Some("SELECT" | "FROM" | "WHERE" | "GROUP" | "HAVING" | "ORDER" | "JOIN")
            )
        })
    {
        output.push_str(&expanded(tokens));
    } else {
        for (index, token) in tokens.iter().enumerate() {
            if index > 0 {
                output.push_str(token.gap);
            }
            output.push_str(token.printed());
        }
    }
}

fn separator(previous: &Lexeme<'_>, token: &Lexeme<'_>) -> &'static str {
    if previous.line_comment() {
        return "\n";
    }
    if previous.is_string() && token.is_string() {
        return if has_newline(token.gap) { "\n" } else { " " };
    }
    if matches!(
        token.token,
        Token::Comma | Token::SemiColon | Token::RParen | Token::Period
    ) || matches!(previous.token, Token::LParen | Token::Period)
        || (matches!(token.token, Token::LParen)
            && previous.keyword.is_none()
            && token.gap.is_empty())
    {
        ""
    } else {
        " "
    }
}

fn inline(tokens: &[Lexeme<'_>]) -> String {
    let mut output = String::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            output.push_str(separator(&tokens[index - 1], token));
        }
        output.push_str(token.printed());
    }
    output
}

struct Parenthesis {
    subquery: bool,
    base: usize,
    indent: usize,
    clause: Option<&'static str>,
    item_depth: usize,
}

fn expanded(tokens: &[Lexeme<'_>]) -> String {
    let mut output = String::new();
    // Each parenthesis remembers whether it opens a subquery and the enclosing clause indent.
    let mut parentheses: Vec<Parenthesis> = Vec::new();
    let mut base = 0;
    let mut indent = 0;
    let mut clause = None;
    let mut item_depth = 0;
    let mut case_depth: usize = 0;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let previous = index.checked_sub(1).map(|index| &tokens[index]);
        let clause_start = case_depth == 0
            && parentheses.len() == item_depth
            && matches!(
                token.keyword,
                Some("SELECT" | "FROM" | "WHERE" | "HAVING" | "GROUP" | "ORDER")
            );
        let join_start = case_depth == 0
            && parentheses.len() == item_depth
            && matches!(
                token.keyword,
                Some("JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS")
            );
        let close_query = matches!(token.token, Token::RParen)
            && parentheses
                .last()
                .is_some_and(|parenthesis| parenthesis.subquery);
        let mut break_line = false;
        if close_query {
            let parenthesis = parentheses.pop().expect("closing a recorded subquery");
            base = parenthesis.base;
            indent = parenthesis.indent;
            clause = parenthesis.clause;
            item_depth = parenthesis.item_depth;
            break_line = true;
        }
        if clause_start {
            indent = base;
            break_line = true;
            clause = token.keyword;
        } else if join_start
            && !previous.is_some_and(|prev| {
                matches!(
                    prev.keyword,
                    Some("INNER" | "LEFT" | "RIGHT" | "FULL" | "OUTER" | "CROSS")
                )
            })
        {
            indent = base;
            clause = Some("JOIN");
            break_line = true;
        } else if case_depth == 0
            && parentheses.len() == item_depth
            && matches!(token.keyword, Some("AND" | "OR"))
            && matches!(clause, Some("WHERE" | "HAVING" | "JOIN"))
        {
            indent = base + 1;
            break_line = true;
        } else if previous.is_some_and(|prev| matches!(prev.token, Token::Comma))
            && parentheses.len() == item_depth
            && case_depth == 0
            && !matches!(token.token, Token::Whitespace(_))
        {
            break_line = true;
        }
        if let Some(previous) = previous {
            if previous.line_comment() {
                break_line = true;
            }
            if previous.is_string() && token.is_string() {
                break_line = has_newline(token.gap);
            }
            if break_line {
                newline(&mut output, 1);
            } else if !output.ends_with('\n') {
                output.push_str(separator(previous, token));
            }
        }
        if output.is_empty() || output.ends_with('\n') {
            output.push_str(&"    ".repeat(indent));
        }
        output.push_str(token.printed());
        if clause_start {
            if matches!(token.keyword, Some("GROUP" | "ORDER"))
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.keyword == Some("BY"))
            {
                index += 1;
                output.push_str(" BY");
            }
            indent = base + 1;
            newline(&mut output, 1);
        }
        if token.keyword == Some("CASE") {
            case_depth += 1;
        }
        if token.keyword == Some("END") {
            case_depth = case_depth.saturating_sub(1);
        }
        if matches!(token.token, Token::LParen) {
            let subquery = tokens
                .get(index + 1)
                .is_some_and(|next| next.keyword == Some("SELECT"));
            parentheses.push(Parenthesis { subquery, base, indent, clause, item_depth });
            if subquery {
                base = indent + 1;
                indent = base;
                clause = None;
                item_depth = parentheses.len();
                newline(&mut output, 1);
            }
        } else if matches!(token.token, Token::RParen) && !close_query {
            parentheses.pop();
        }
        index += 1;
    }
    output
}
