//! Script forms using native SHOW CREATE definitions instead of reconstructing table DDL.

use sqlparser::dialect::{Dialect, MySqlDialect};
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer};

use super::{Error, literal, metadata, quote, rows, utf8_literal};
use crate::objects::{Kind, Object, Script};
use crate::session::{Session, Value};

/// Produces the validated object's requested form; no generated DDL runs on the source.
pub(super) async fn script(
    session: &mut dyn Session,
    object: &Object,
    form: Script,
) -> Result<String, Error> {
    let qualified = format!("{}.{}", quote(&object.id.database)?, quote(&object.name)?);
    if form == Script::Select {
        let columns = metadata::columns(session, object)
            .await?
            .iter()
            .map(|column| quote(&column.name))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        return Ok(format!("SELECT {columns} FROM {qualified} LIMIT 1000;\n"));
    }
    if form == Script::Alter && object.kind == Kind::Table {
        // Every line must remain a comment even when an identifier contains a newline.
        let qualified = qualified.replace('\n', "\n-- ").replace('\r', "\r-- ");
        return Ok(format!(
            "-- ALTER TABLE {qualified}\n--     ADD COLUMN column_name data_type;\n"
        ));
    }
    if object.kind == Kind::Index {
        return index_script(session, object).await;
    }
    let keyword = match object.kind {
        Kind::Table => "TABLE",
        Kind::View => "VIEW",
        Kind::Procedure => "PROCEDURE",
        Kind::Function => "FUNCTION",
        _ => {
            return Err(Error::Unsupported {
                operation: "Script as",
                kind: object.kind,
            });
        }
    };
    let settings = rows(
        session,
        "SELECT @@SESSION.sql_mode, CAST(@@SESSION.sql_quote_show_create AS CHAR)",
    )
    .await?;
    let mode = text(settings.first().ok_or(Error::NotFound)?, 0)?;
    if text(settings.first().ok_or(Error::NotFound)?, 1)? != "1" {
        return Err(Error::QuotingDisabled);
    }
    if mode.split(',').any(|mode| mode == "ANSI_QUOTES") {
        return Err(Error::AnsiQuotes);
    }
    let result = rows(session, &format!("SHOW CREATE {keyword} {qualified}")).await?;
    let row = result.first().ok_or(Error::NotFound)?;
    let routine = matches!(object.kind, Kind::Procedure | Kind::Function);
    let mut definition = text(row, if routine { 2 } else { 1 })?.to_owned();
    // Canonical table/view text uses backslash escapes even when SHOW ran with them disabled.
    let mode = if routine {
        text(row, 1)?.to_owned()
    } else {
        mode.split(',')
            .filter(|mode| *mode != "NO_BACKSLASH_ESCAPES")
            .collect::<Vec<_>>()
            .join(",")
    };
    if object.kind == Kind::View {
        definition = view_literals(session, &definition, text(row, 2)?).await?;
    }
    if routine {
        let collation = text(row, 5)?;
        let charset = rows(
            session,
            &format!(
                "SELECT CHARACTER_SET_NAME FROM information_schema.COLLATIONS
             WHERE CONVERT(COLLATION_NAME USING utf8mb4) COLLATE utf8mb4_bin = {}",
                literal(collation)?
            ),
        )
        .await?;
        definition = routine_charsets(
            &definition,
            &mode,
            text(charset.first().ok_or(Error::NotFound)?, 0)?,
            collation,
        )?;
    }
    if form == Script::Alter {
        let rest = definition
            .strip_prefix("CREATE ")
            .ok_or_else(|| Error::Catalog("View definition does not start with CREATE".into()))?;
        definition = format!("ALTER {rest}");
    }
    let mut sql = format!("SET SESSION sql_mode = {};\n", literal(&mode)?);
    if routine || object.kind == Kind::View {
        let offset = if routine { 3 } else { 2 };
        sql.push_str(&format!(
            "SET SESSION character_set_client = 'utf8mb4';\nSET SESSION collation_connection = {};\n",
            literal(text(row, offset + 1)?)?
        ));
    }
    sql.push_str(definition.trim_end_matches(';'));
    sql.push_str(";\n");
    Ok(sql)
}

/// Inserts inherited string type settings without reformatting the stored routine definition.
fn routine_charsets(
    definition: &str,
    mode: &str,
    charset: &str,
    collation: &str,
) -> Result<String, Error> {
    let dialect = RoutineDialect {
        backslash_escapes: !mode.split(',').any(|mode| mode == "NO_BACKSLASH_ESCAPES"),
        ansi_quotes: mode.split(',').any(|mode| mode == "ANSI_QUOTES"),
    };
    // shortcut: executable comments stay opaque; expand them if declarations inside need scripting.
    let tokens = Tokenizer::new(&dialect, definition)
        .tokenize_with_location()
        .map_err(|error| Error::Catalog(error.to_string()))?
        .into_iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let routine = tokens
        .iter()
        .position(|token| word_is(token, &["PROCEDURE", "FUNCTION"]))
        .ok_or_else(|| Error::Catalog("Expected routine declaration".into()))?;
    let open = (routine + 1..tokens.len())
        .find(|&i| tokens[i].token == Token::LParen)
        .ok_or_else(|| Error::Catalog("Expected routine parameters".into()))?;
    let end = parenthesis_end(&tokens, open)?;
    let mut starts = Vec::new();
    let mut parameter = open + 1;
    let mut depth = 0;
    for i in open + 1..end {
        match tokens[i].token {
            Token::LParen => depth += 1,
            Token::RParen if depth > 0 => depth -= 1,
            Token::Comma | Token::RParen if depth == 0 => {
                let name = parameter
                    + usize::from(
                        tokens
                            .get(parameter)
                            .is_some_and(|token| word_is(token, &["IN", "OUT", "INOUT"])),
                    );
                if name < i {
                    starts.push(name + 1);
                }
                parameter = i + 1;
            }
            _ => {}
        }
    }
    if tokens
        .get(end)
        .is_some_and(|token| word_is(token, &["RETURNS"]))
    {
        starts.push(end + 1);
    }
    for i in end..tokens.len() {
        if word_is(&tokens[i], &["DECLARE"]) {
            let mut name = i + 1;
            while tokens
                .get(name + 1)
                .is_some_and(|token| token.token == Token::Comma)
            {
                name += 2;
            }
            // CURSOR, CONDITION and HANDLER declarations do not have a string type here.
            starts.push(name + 1);
        }
    }
    let charset_clause = format!(" CHARACTER SET {}", quote(charset)?);
    let collation_clause = format!(" COLLATE {}", quote(collation)?);
    let mut sql = String::new();
    let mut copied = 0;
    for start in starts {
        if let Some((end, collate)) = inherited_string_type(&tokens, start)? {
            let offset = byte_offset(definition, tokens[end - 1].span.end);
            sql.push_str(&definition[copied..offset]);
            sql.push_str(&charset_clause);
            if collate {
                sql.push_str(&collation_clause);
            }
            copied = offset;
        }
    }
    sql.push_str(&definition[copied..]);
    Ok(sql)
}

/// Finds the end of a string type and whether its collation is also inherited.
fn inherited_string_type(
    tokens: &[TokenWithSpan],
    start: usize,
) -> Result<Option<(usize, bool)>, Error> {
    let Some(token) = tokens.get(start) else {
        return Ok(None);
    };
    if !word_is(
        token,
        &[
            "CHAR",
            "CHARACTER",
            "VARCHAR",
            "VARCHARACTER",
            "TINYTEXT",
            "TEXT",
            "MEDIUMTEXT",
            "LONGTEXT",
            "ENUM",
            "SET",
            "LONG",
        ],
    ) {
        return Ok(None);
    }
    let mut end = start + 1;
    if word_is(token, &["LONG"]) {
        if tokens
            .get(end)
            .is_some_and(|token| word_is(token, &["VARBINARY"]))
        {
            return Ok(None);
        }
        if tokens
            .get(end)
            .is_some_and(|token| word_is(token, &["VARCHAR", "VARCHARACTER", "CHAR", "CHARACTER"]))
        {
            end += 1;
        }
    }
    if tokens
        .get(end)
        .is_some_and(|token| word_is(token, &["VARYING"]))
    {
        end += 1;
    }
    if tokens
        .get(end)
        .is_some_and(|token| token.token == Token::LParen)
    {
        end = parenthesis_end(tokens, end)?;
    }
    let mut attribute = end;
    let mut collate = true;
    while let Some(token) = tokens.get(attribute) {
        if word_is(token, &["CHARSET", "ASCII", "UNICODE", "BYTE"])
            || (word_is(token, &["CHAR", "CHARACTER"])
                && tokens
                    .get(attribute + 1)
                    .is_some_and(|token| word_is(token, &["SET"])))
        {
            return Ok(None);
        }
        if word_is(token, &["BINARY"]) {
            collate = false;
            attribute += 1;
        } else if word_is(token, &["COLLATE"]) {
            collate = false;
            attribute += 2;
        } else {
            break;
        }
    }
    Ok(Some((end, collate)))
}

/// Skips a parenthesized type length, value list or routine parameter list.
fn parenthesis_end(tokens: &[TokenWithSpan], open: usize) -> Result<usize, Error> {
    let mut depth = 0;
    for (i, token) in tokens.iter().enumerate().skip(open) {
        match token.token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
    }
    Err(Error::Catalog(
        "Unclosed routine type or parameter list".into(),
    ))
}

fn word_is(token: &TokenWithSpan, words: &[&str]) -> bool {
    matches!(&token.token, Token::Word(word) if word.quote_style.is_none()
        && words.iter().any(|value| word.value.eq_ignore_ascii_case(value)))
}

/// MySQL tokenization with the stored routine's creation-time string escape mode.
#[derive(Debug)]
struct RoutineDialect {
    backslash_escapes: bool,
    ansi_quotes: bool,
}

impl Dialect for RoutineDialect {
    fn dialect(&self) -> std::any::TypeId {
        MySqlDialect {}.dialect()
    }
    fn is_identifier_start(&self, ch: char) -> bool {
        MySqlDialect {}.is_identifier_start(ch)
    }
    fn is_identifier_part(&self, ch: char) -> bool {
        MySqlDialect {}.is_identifier_part(ch)
    }
    fn is_delimited_identifier_start(&self, ch: char) -> bool {
        ch == '`' || (self.ansi_quotes && ch == '"')
    }
    fn supports_string_literal_backslash_escape(&self) -> bool {
        self.backslash_escapes
    }
    fn supports_hash_comment(&self) -> bool {
        MySqlDialect {}.supports_hash_comment()
    }
    fn requires_single_line_comment_whitespace(&self) -> bool {
        MySqlDialect {}.requires_single_line_comment_whitespace()
    }
    fn supports_numeric_prefix(&self) -> bool {
        MySqlDialect {}.supports_numeric_prefix()
    }
}

/// Restores introduced literal bytes after SHOW's result conversion, retaining literal coercibility.
// shortcut: SHOW conversion must be lossless; use a binary SHOW path to support already-lost bytes.
async fn view_literals(
    session: &mut dyn Session,
    definition: &str,
    client_charset: &str,
) -> Result<String, Error> {
    let tokens = Tokenizer::new(&MySqlDialect {}, definition)
        .tokenize_with_location()
        .map_err(|error| Error::Catalog(error.to_string()))?;
    let tokens = tokens
        .iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let mut sql = String::new();
    let mut copied = 0;
    for pair in tokens.windows(2) {
        if let (Token::Word(word), Token::SingleQuotedString(value)) =
            (&pair[0].token, &pair[1].token)
            && word.quote_style.is_none()
            && word.value.starts_with('_')
        {
            // SHOW tags the entire definition with its creation client charset, even mixed literals.
            let result = rows(
                session,
                &format!(
                    "SELECT HEX(CONVERT({} USING {}))",
                    utf8_literal(value),
                    quote(client_charset)?
                ),
            )
            .await?;
            let hex = text(result.first().ok_or(Error::NotFound)?, 0)?;
            if !hex.len().is_multiple_of(2) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Error::Catalog("Expected hexadecimal view literal".into()));
            }
            let start = byte_offset(definition, pair[0].span.start);
            let end = byte_offset(definition, pair[1].span.end);
            sql.push_str(&definition[copied..start]);
            sql.push_str(&format!("{} X'{hex}'", word.value));
            copied = end;
        }
    }
    sql.push_str(&definition[copied..]);
    Ok(sql)
}

fn byte_offset(sql: &str, location: Location) -> usize {
    let mut lines = sql.split_inclusive('\n');
    let preceding = lines
        .by_ref()
        .take(location.line as usize - 1)
        .map(str::len)
        .sum::<usize>();
    preceding
        + lines
            .next()
            .unwrap_or_default()
            .chars()
            .take(location.column as usize - 1)
            .map(char::len_utf8)
            .sum::<usize>()
}

fn text(row: &[Value], index: usize) -> Result<&str, Error> {
    match row.get(index) {
        Some(Value::Text(text)) => Ok(text),
        _ => Err(Error::Catalog(
            "Missing SHOW CREATE definition or creation setting; check privileges".into(),
        )),
    }
}

async fn index_script(session: &mut dyn Session, object: &Object) -> Result<String, Error> {
    let (_, rest) = object.id.key.split_once(':').ok_or(Error::Identity)?;
    let (table_hex, _) = rest.split_once(':').ok_or(Error::Identity)?;
    // Resolve the parent via the catalog rather than decoding caller-supplied names into SQL.
    let parents = super::catalog(
        session,
        &format!(
            "object_key = CONCAT('table:', {}) AND kind = 'table'",
            literal(table_hex)?
        ),
        Some(1),
    )
    .await?;
    let table = parents.first().ok_or(Error::NotFound)?;
    let index = metadata::indexes(session, table)
        .await?
        .into_iter()
        .find(|index| index.object == *object)
        .ok_or(Error::NotFound)?;
    Ok(format!("{};\n", index.definition))
}

#[cfg(test)]
mod tests {
    use super::routine_charsets;

    #[test]
    fn inherited_string_types_and_synonyms_receive_the_source_settings() {
        assert_eq!(
            routine_charsets(
                "CREATE PROCEDURE p(IN a CHAR, OUT b CHARACTER(5), INOUT c CHARACTER VARYING(10), d VARCHAR(5), e TINYTEXT, f TEXT(100), g MEDIUMTEXT, h LONGTEXT, i ENUM('a,b',')'), j SET('x','y'), k LONG, l LONG VARCHAR, m LONG CHARACTER VARYING) SELECT 1",
                "", "latin1", "latin1_swedish_ci",
            ).unwrap(),
            "CREATE PROCEDURE p(IN a CHAR CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, OUT b CHARACTER(5) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, INOUT c CHARACTER VARYING(10) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, d VARCHAR(5) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, e TINYTEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, f TEXT(100) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, g MEDIUMTEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, h LONGTEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, i ENUM('a,b',')') CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, j SET('x','y') CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, k LONG CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, l LONG VARCHAR CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, m LONG CHARACTER VARYING CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`) SELECT 1",
        );
    }

    #[test]
    fn binary_national_and_explicit_charset_types_are_unchanged() {
        let definition = "CREATE PROCEDURE p(a BINARY(5), b VARBINARY(5), c TINYBLOB, d BLOB, e MEDIUMBLOB, f LONGBLOB, g LONG VARBINARY, h NATIONAL CHAR(5), i NCHAR(5), j NVARCHAR(5), k CHAR(5) CHARACTER SET utf8mb4, l CHAR(5) CHARSET utf8mb4, m CHAR ASCII, n CHAR UNICODE, o CHAR BYTE, q CHAR BINARY CHARACTER SET utf8mb4) SELECT 1";
        assert_eq!(
            routine_charsets(definition, "", "latin1", "latin1_swedish_ci").unwrap(),
            definition
        );
    }

    #[test]
    fn binary_and_collate_attributes_receive_only_the_charset() {
        assert_eq!(
            routine_charsets(
                "CREATE PROCEDURE p(a CHAR(5) BINARY, b VARCHAR(10) COLLATE latin1_bin) SELECT 1",
                "",
                "latin1",
                "latin1_swedish_ci"
            )
            .unwrap(),
            "CREATE PROCEDURE p(a CHAR(5) CHARACTER SET `latin1` BINARY, b VARCHAR(10) CHARACTER SET `latin1` COLLATE latin1_bin) SELECT 1",
        );
    }

    #[test]
    fn return_and_nested_variable_lists_receive_the_source_settings() {
        assert_eq!(
            routine_charsets("CREATE FUNCTION p() RETURNS TEXT DETERMINISTIC BEGIN DECLARE a,b VARCHAR(10) DEFAULT 'pear'; BEGIN DECLARE c CHAR; END; RETURN a; END", "", "latin1", "latin1_swedish_ci").unwrap(),
            "CREATE FUNCTION p() RETURNS TEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci` DETERMINISTIC BEGIN DECLARE a,b VARCHAR(10) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci` DEFAULT 'pear'; BEGIN DECLARE c CHAR CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; END; RETURN a; END",
        );
    }

    #[test]
    fn nonvariable_declarations_and_default_expressions_are_unchanged() {
        let definition = "CREATE PROCEDURE p() BEGIN DECLARE c CONDITION FOR SQLSTATE '45000'; DECLARE cur CURSOR FOR SELECT CAST('pear' AS CHAR(5)); DECLARE CONTINUE HANDLER FOR NOT FOUND SET @v = 'DECLARE v VARCHAR(5)'; END";
        assert_eq!(
            routine_charsets(definition, "", "latin1", "latin1_swedish_ci").unwrap(),
            definition
        );
    }

    #[test]
    fn strings_comments_and_unicode_identifiers_keep_their_original_bytes() {
        assert_eq!(
            routine_charsets("CREATE PROCEDURE `poireé`() BEGIN\r\n DECLARE `DECLARE VARCHAR(5)` VARCHAR /*é*/ (10); # DECLARE a VARCHAR(5)\r\n -- DECLARE b VARCHAR(5)\n SELECT 'DECLARE VARCHAR(5)', \"DECLARE VARCHAR(5)\"; /* DECLARE c VARCHAR(5) */ END", "", "latin1", "latin1_swedish_ci").unwrap(),
            "CREATE PROCEDURE `poireé`() BEGIN\r\n DECLARE `DECLARE VARCHAR(5)` VARCHAR /*é*/ (10) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; # DECLARE a VARCHAR(5)\r\n -- DECLARE b VARCHAR(5)\n SELECT 'DECLARE VARCHAR(5)', \"DECLARE VARCHAR(5)\"; /* DECLARE c VARCHAR(5) */ END",
        );
    }

    #[test]
    fn no_backslash_escapes_keeps_a_trailing_backslash_inside_its_literal() {
        assert_eq!(
            routine_charsets(
                r"CREATE PROCEDURE p() BEGIN DECLARE a VARCHAR(5) DEFAULT '\'; DECLARE b TEXT; END",
                "NO_BACKSLASH_ESCAPES",
                "latin1",
                "latin1_swedish_ci"
            )
            .unwrap(),
            r"CREATE PROCEDURE p() BEGIN DECLARE a VARCHAR(5) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci` DEFAULT '\'; DECLARE b TEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; END",
        );
    }

    #[test]
    fn backslash_escaped_quotes_keep_declaration_text_inside_its_literal() {
        assert_eq!(
            routine_charsets(
                r"CREATE PROCEDURE p() BEGIN SELECT '\'DECLARE a VARCHAR(5)'; DECLARE b TEXT; END",
                "",
                "latin1",
                "latin1_swedish_ci"
            )
            .unwrap(),
            r"CREATE PROCEDURE p() BEGIN SELECT '\'DECLARE a VARCHAR(5)'; DECLARE b TEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; END",
        );
    }

    #[test]
    fn numeric_identifier_prefixes_and_adjacent_minus_operators_are_preserved() {
        assert_eq!(
            routine_charsets(
                "CREATE PROCEDURE p(1fruit VARCHAR(5)) BEGIN SET @v=1--1; BEGIN DECLARE b TEXT; END; END",
                "",
                "latin1",
                "latin1_swedish_ci"
            )
            .unwrap(),
            "CREATE PROCEDURE p(1fruit VARCHAR(5) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`) BEGIN SET @v=1--1; BEGIN DECLARE b TEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; END; END",
        );
    }

    #[test]
    fn ansi_quoted_identifiers_use_the_routine_creation_mode() {
        assert_eq!(
            routine_charsets(r#"CREATE PROCEDURE p() BEGIN DECLARE "v\" VARCHAR(10); DECLARE "DECLARE" TEXT; END"#, "ANSI_QUOTES", "latin1", "latin1_swedish_ci").unwrap(),
            r#"CREATE PROCEDURE p() BEGIN DECLARE "v\" VARCHAR(10) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; DECLARE "DECLARE" TEXT CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`; END"#,
        );
    }

    #[test]
    fn varcharacter_and_char_set_aliases_use_native_type_rules() {
        assert_eq!(
            routine_charsets("CREATE PROCEDURE p(a VARCHARACTER(5), b LONG VARCHARACTER, c CHAR(5) CHAR SET utf8mb4) SELECT 1", "", "latin1", "latin1_swedish_ci").unwrap(),
            "CREATE PROCEDURE p(a VARCHARACTER(5) CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, b LONG VARCHARACTER CHARACTER SET `latin1` COLLATE `latin1_swedish_ci`, c CHAR(5) CHAR SET utf8mb4) SELECT 1",
        );
    }
}
