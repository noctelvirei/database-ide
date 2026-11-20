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
    /// Formatting changed a token, the line break between continued string literals, or
    /// the spacing inside an operator.
    #[error("formatting would change SQL tokens; keep the original text")]
    ChangedTokens,
    /// A SQL Server `GO` line is invalid, so the script's batches are unknown.
    #[error("cannot format batches: {0}")]
    BatchSeparator(String),
    /// The input uses a construct whose meaning the formatter cannot preserve.
    #[error("cannot format {0}; keep the original text")]
    Unsupported(&'static str),
}

/// Formats a script or fragment without server access, using four-space indentation and
/// an 80-character compact layout. Only whitespace and listed structural keyword case change;
/// SQL Server `GO` lines are copied unchanged, each on its own line.
/// Returns an error if tokenization fails or the output fails the preservation guard;
/// the borrowed input is never changed. Unrecognised statements use conservative layout;
/// batches with executable MySQL comments or COPY data are returned verbatim.
pub fn format(text: &str, engine: Engine) -> Result<String, Error> {
    match engine {
        Engine::SqlServer => format_script(text),
        Engine::PostgreSql => format_batch(text, &PostgreSqlDialect {}),
        Engine::MySql => format_batch(text, &MySqlDialect {}),
    }
}

/// Formats each SQL Server batch on its own and copies the `GO` lines between them. The
/// batch splitter the runner uses decides what a batch is, so formatting cannot move a
/// boundary: the output must split into the same batches with the same repeat counts.
fn format_script(text: &str) -> Result<String, Error> {
    let separator_error =
        |error: crate::mssql::batches::Error| Error::BatchSeparator(error.to_string());
    let dialect = MsSqlDialect {};
    let (mut output, script) = match text.strip_prefix('\u{feff}') {
        Some(script) => (String::from('\u{feff}'), script),
        None => (String::new(), text),
    };
    let mut formatted = Vec::new();
    let mut cursor = 0;
    for batch in crate::mssql::batches::split(script).map_err(separator_error)? {
        // `split` returns slices of `script`.
        let start = batch.sql.as_ptr() as usize - script.as_ptr() as usize;
        push_separator_lines(&mut output, &script[cursor..start]);
        let sql = format_batch(batch.sql, &dialect)?;
        newline(&mut output, 2);
        output.push_str(&sql);
        formatted.push((sql, batch.repeat));
        cursor = start + batch.sql.len();
    }
    push_separator_lines(&mut output, &script[cursor..]);
    let checked = crate::mssql::batches::split(&output).map_err(separator_error)?;
    if checked.len() != formatted.len()
        || checked
            .iter()
            .zip(&formatted)
            .any(|(batch, (sql, repeat))| batch.sql.trim() != sql.trim() || batch.repeat != *repeat)
    {
        return Err(Error::ChangedTokens);
    }
    Ok(output)
}

/// Copies the nonblank lines of the text between two batches (its `GO` lines) unchanged,
/// each on a line of its own.
fn push_separator_lines(output: &mut String, between: &str) {
    for line in between.lines().filter(|line| !line.trim().is_empty()) {
        newline(output, 1);
        output.push_str(line);
        output.push('\n');
    }
}

/// Formats text that holds no batch separators.
fn format_batch(text: &str, dialect: &dyn Dialect) -> Result<String, Error> {
    let tokens = lex(text, dialect)?;
    if dialect.is::<PostgreSqlDialect>() && has_unicode_escaped_identifier(&tokens) {
        return Err(Error::Unsupported("PostgreSQL U& identifiers"));
    }
    let mut output = String::new();
    let mut start = 0;
    let mut depth: usize = 0;
    for (index, token) in tokens.iter().enumerate() {
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
    append_statement(&mut output, &tokens[start..]);
    let checked = lex(&output, dialect)?;
    if tokens.len() != checked.len()
        || tokens.iter().zip(&checked).any(|(a, b)| {
            a.printed() != b.printed()
                || std::mem::discriminant(&a.token) != std::mem::discriminant(&b.token)
        })
        || (0..tokens.len()).any(|index| {
            literal_continues_on_new_line(&tokens, index)
                != literal_continues_on_new_line(&checked, index)
                || (joined(&tokens, index) && !checked[index].gap.is_empty())
        })
    {
        return Err(Error::ChangedTokens);
    }
    Ok(output)
}

/// Whether the tokens hold `U&"…"`, which the tokenizer splits into a word, an `&` and a
/// quoted identifier; spacing them out would change the identifier into an expression.
fn has_unicode_escaped_identifier(tokens: &[Lexeme<'_>]) -> bool {
    tokens.windows(3).any(|window| {
        window[0].word_is("U")
            && matches!(window[1].token, Token::Ampersand)
            && window[1].gap.is_empty()
            && window[2].gap.is_empty()
            && matches!(&window[2].token, Token::Word(word) if word.quote_style == Some('"'))
    })
}

struct Lexeme<'a> {
    token: Token,
    raw: &'a str,
    // Original whitespace between this token and the previous significant token.
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

    /// Punctuation that isn't a bracket, separator or dot: the pieces of an operator.
    fn is_operator(&self) -> bool {
        !self.is_string()
            && !matches!(
                self.token,
                Token::Word(_)
                    | Token::Number(..)
                    | Token::Whitespace(_)
                    | Token::LParen
                    | Token::RParen
                    | Token::LBracket
                    | Token::RBracket
                    | Token::LBrace
                    | Token::RBrace
                    | Token::Comma
                    | Token::SemiColon
                    | Token::Period
                    | Token::Placeholder(_)
                    | Token::Char(_)
            )
    }

    fn line_comment(&self) -> bool {
        matches!(
            self.token,
            Token::Whitespace(Whitespace::SingleLineComment { .. })
        )
    }

    fn word_is(&self, value: &str) -> bool {
        matches!(&self.token, Token::Word(word) if word.quote_style.is_none())
            && self.raw.eq_ignore_ascii_case(value)
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
    "OUTER",
    "CROSS",
    "IN",
    "IS",
    "NULL",
    "NOT",
    "LIKE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
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
    let mut after_period = false;
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
            // A reserved word after a dot can be an unquoted MySQL identifier.
            Token::Word(word) if word.quote_style.is_none() && !after_period => KEYWORDS
                .iter()
                .copied()
                .find(|keyword| raw.eq_ignore_ascii_case(keyword)),
            _ => None,
        };
        if !matches!(token.token, Token::Whitespace(_)) {
            after_period = matches!(token.token, Token::Period);
        }
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

/// For a string literal that follows another one with only comments between them, whether
/// a line break separates the two. PostgreSQL joins such literals only across a line break,
/// so this must survive formatting. `None` when `index` doesn't continue a literal.
fn literal_continues_on_new_line(tokens: &[Lexeme<'_>], index: usize) -> Option<bool> {
    if !tokens[index].is_string() {
        return None;
    }
    let mut line_break = has_newline(tokens[index].gap);
    for before in (0..index).rev() {
        let token = &tokens[before];
        if token.is_string() {
            return Some(line_break);
        }
        if !matches!(token.token, Token::Whitespace(_)) {
            return None;
        }
        line_break |= has_newline(token.raw) || has_newline(token.gap);
    }
    None
}

/// Whether `tokens[index]` is written against the token before it and a space between
/// them would change what the server reads. The tokenizer splits some single server tokens
/// in two: PostgreSQL operators such as `===` become several operators, `1e` and `0x41g`
/// (MySQL identifiers, or a T-SQL float) become a literal and a word, MySQL's `@'x'`
/// becomes `@` and a string, and T-SQL money such as `$-2.5` or `£3` becomes a sign and a
/// number.
fn joined(tokens: &[Lexeme<'_>], index: usize) -> bool {
    if index == 0 || !tokens[index].gap.is_empty() {
        return false;
    }
    let (previous, token) = (&tokens[index - 1], &tokens[index]);
    let sigil = |lexeme: &Lexeme<'_>| {
        matches!(lexeme.token, Token::AtSign | Token::Char(_))
            || matches!(&lexeme.token, Token::Placeholder(text) if text == "$")
    };
    (previous.is_operator() && token.is_operator())
        || ((matches!(previous.token, Token::Number(..)) || previous.is_string())
            && matches!(token.token, Token::Word(_) | Token::Number(..)))
        || sigil(previous)
        || sigil(token)
        // The sign in `$-2.5` belongs to the money literal.
        || (index >= 2
            && sigil(&tokens[index - 2])
            && previous.gap.is_empty()
            && matches!(previous.token, Token::Minus | Token::Plus)
            && matches!(token.token, Token::Number(..)))
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

/// The whitespace to put before `tokens[index]` when nothing forces a line break.
fn separator(tokens: &[Lexeme<'_>], index: usize) -> &'static str {
    let (previous, token) = (&tokens[index - 1], &tokens[index]);
    if previous.line_comment() {
        return "\n";
    }
    if let Some(line_break) = literal_continues_on_new_line(tokens, index) {
        return if line_break { "\n" } else { " " };
    }
    if joined(tokens, index) {
        return "";
    }
    // Spaces around a dot can distinguish numeric and operator tokens in a fragment.
    if matches!(previous.token, Token::Period) || matches!(token.token, Token::Period) {
        return if token.gap.is_empty() { "" } else { " " };
    }
    // `1--2` is arithmetic; a space after the `--` would start a MySQL comment.
    if token.gap.is_empty() && matches!(previous.token, Token::Minus) && joined(tokens, index - 1) {
        return "";
    }
    if matches!(token.token, Token::Comma | Token::SemiColon | Token::RParen)
        || matches!(previous.token, Token::LParen)
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
            output.push_str(separator(tokens, index));
        }
        output.push_str(token.printed());
    }
    output
}

struct Parenthesis {
    subquery: bool,
    // Enclosing query's clause indentation, in four-space levels.
    base: usize,
    // Enclosing item's indentation, in four-space levels.
    indent: usize,
    clause: Option<&'static str>,
    // Parenthesis nesting at which commas and clauses belong to the enclosing query.
    item_depth: usize,
}

fn expanded(tokens: &[Lexeme<'_>]) -> String {
    let mut output = String::new();
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
            && (token.word_is("FULL")
                || matches!(
                    token.keyword,
                    Some("JOIN" | "INNER" | "LEFT" | "RIGHT" | "CROSS")
                ));
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
                prev.word_is("FULL")
                    || matches!(
                        prev.keyword,
                        Some("INNER" | "LEFT" | "RIGHT" | "OUTER" | "CROSS")
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
            if let Some(line_break) = literal_continues_on_new_line(tokens, index) {
                break_line = line_break;
            }
            if break_line {
                newline(&mut output, 1);
            } else if !output.ends_with('\n') {
                output.push_str(separator(tokens, index));
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
        if token.word_is("END") {
            case_depth = case_depth.saturating_sub(1);
        }
        if matches!(token.token, Token::LParen) {
            let subquery = tokens
                .get(index + 1)
                .is_some_and(|next| next.keyword == Some("SELECT"));
            parentheses.push(Parenthesis {
                subquery,
                base,
                indent,
                clause,
                item_depth,
            });
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
