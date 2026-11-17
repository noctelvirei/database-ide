//! GO boundaries and their original script locations, without rewriting SQL.

/// A nonempty SQL batch; repeat count does not duplicate its source text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Batch<'a> {
    /// Original text, including leading whitespace and trailing newline.
    pub sql: &'a str,
    /// Lines preceding this batch in the original script.
    pub line_offset: u64,
    /// Positive execution count from GO, or one without a count.
    pub repeat: u32,
}

/// Invalid GO repeat counts fail before any SQL is sent.
#[derive(Debug, thiserror::Error)]
#[error("GO repeat count must be a positive 32-bit integer at line {0}")]
pub(crate) struct Error(u64);

/// Splits only standalone GO lines outside quoted text, identifiers and nested comments.
pub(crate) fn split(script: &str) -> Result<Vec<Batch<'_>>, Error> {
    let script = script.strip_prefix('\u{feff}').unwrap_or(script);
    let mut result = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut start_line = 0;
    let mut quote = None;
    let mut depth = 0u32;
    for (line, text) in script.split_inclusive('\n').enumerate() {
        // SSMS accepts a trailing `--` comment after GO and its count.
        let mut words = text.split("--").next().unwrap_or(text).split_whitespace();
        let go = quote.is_none()
            && depth == 0
            && words
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("GO"));
        if go {
            let count = words.next();
            if words.next().is_none() {
                let repeat = match count {
                    None => 1,
                    Some(count) => count
                        .parse::<u32>()
                        .ok()
                        .filter(|count| *count > 0)
                        .ok_or(Error(line as u64 + 1))?,
                };
                if !script[start..offset].trim().is_empty() {
                    result.push(Batch {
                        sql: &script[start..offset],
                        line_offset: start_line,
                        repeat,
                    });
                }
                start = offset + text.len();
                start_line = line as u64 + 1;
                offset = start;
                continue;
            }
        }
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            let next = bytes.get(index + 1).copied();
            if let Some(end) = quote {
                if byte == end {
                    if next == Some(end) {
                        index += 1;
                    } else {
                        quote = None;
                    }
                }
            } else if depth != 0 {
                if byte == b'/' && next == Some(b'*') {
                    depth += 1;
                    index += 1;
                } else if byte == b'*' && next == Some(b'/') {
                    depth -= 1;
                    index += 1;
                }
            } else {
                match (byte, next) {
                    (b'-', Some(b'-')) => break,
                    (b'/', Some(b'*')) => {
                        depth = 1;
                        index += 1;
                    }
                    (b'\'', _) => quote = Some(b'\''),
                    (b'"', _) => quote = Some(b'"'),
                    (b'[', _) => quote = Some(b']'),
                    _ => {}
                }
            }
            index += 1;
        }
        offset += text.len();
    }
    if !script[start..].trim().is_empty() {
        result.push(Batch {
            sql: &script[start..],
            line_offset: start_line,
            repeat: 1,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_repeats_preserve_original_line_offsets() {
        assert_eq!(
            split("SELECT 1\r\ngO 3\r\n\nSELECT 2\nGO\n").unwrap(),
            vec![
                Batch {
                    sql: "SELECT 1\r\n",
                    line_offset: 0,
                    repeat: 3
                },
                Batch {
                    sql: "\nSELECT 2\n",
                    line_offset: 2,
                    repeat: 1
                },
            ]
        );
    }

    #[test]
    fn quoted_go_and_nested_comments_never_split() {
        let sql = "SELECT 'it''s\nGO\ntext', [a]]\nGO\nb], \"a\nGO\nb\";\n/* outer\n/* inner */\nGO\n*/\n-- GO\nSELECT 2";
        assert_eq!(
            split(sql).unwrap(),
            vec![Batch {
                sql,
                line_offset: 0,
                repeat: 1
            }]
        );
    }

    #[test]
    fn go_with_a_trailing_comment_still_splits() {
        assert_eq!(
            split(
                "SELECT 1
GO 2 -- twice
SELECT 2
GO--end
"
            )
            .unwrap(),
            vec![
                Batch {
                    sql: "SELECT 1
",
                    line_offset: 0,
                    repeat: 2
                },
                Batch {
                    sql: "SELECT 2
",
                    line_offset: 2,
                    repeat: 1
                },
            ]
        );
    }

    #[test]
    fn invalid_repeat_is_rejected_before_running() {
        assert_eq!(
            split("SELECT 1\nGO 0").unwrap_err().to_string(),
            "GO repeat count must be a positive 32-bit integer at line 2"
        );
        assert!(split("SELECT 1\nGO 4294967296").is_err());
    }

    #[test]
    fn go_in_sql_is_sent_to_the_server() {
        let sql = "SELECT go FROM t\nSELECT 2";
        assert_eq!(
            split(sql).unwrap(),
            vec![Batch {
                sql,
                line_offset: 0,
                repeat: 1
            }]
        );
    }
    #[test]
    fn leading_bom_is_not_sql_and_embedded_bom_is_preserved() {
        assert_eq!(
            split("\u{feff}SELECT '\u{feff}'\nGO\nSELECT 2").unwrap(),
            vec![
                Batch {
                    sql: "SELECT '\u{feff}'\n",
                    line_offset: 0,
                    repeat: 1
                },
                Batch {
                    sql: "SELECT 2",
                    line_offset: 2,
                    repeat: 1
                },
            ]
        );
        assert!(split("\u{feff}").unwrap().is_empty());
    }
}
