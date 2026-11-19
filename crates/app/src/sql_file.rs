//! A `.sql` file's text and encoding (docs/spec.md, "Files, restored tabs and history"):
//! UTF-8, or UTF-16 when its byte-order mark says so, saved back in the encoding it was
//! opened in. New files are UTF-8 without a byte-order mark.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// How a file's text is stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    #[default]
    Utf8,
    /// UTF-8 starting with the byte-order mark EF BB BF.
    Utf8Bom,
    /// UTF-16 little-endian, starting with FF FE.
    Utf16Le,
    /// UTF-16 big-endian, starting with FE FF.
    Utf16Be,
}

/// Reads `path` as text. Fails when the file can't be read or its bytes aren't valid in the
/// encoding they claim, with a message fit for the messages strip.
pub fn read(path: &Path) -> Result<(String, Encoding), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    decode(&bytes).ok_or_else(|| "it isn't valid UTF-8 or UTF-16 text.".to_owned())
}

/// Writes `text` to `path` in `encoding`, replacing the file only once it is all written.
pub fn write(path: &Path, text: &str, encoding: Encoding) -> std::io::Result<()> {
    crate::replace_file::write(path, &encode(text, encoding))
}

fn decode(bytes: &[u8]) -> Option<(String, Encoding)> {
    fn utf16(body: &[u8], unit: fn([u8; 2]) -> u16) -> Option<String> {
        let (pairs, []) = body.as_chunks::<2>() else {
            return None;
        };
        let units: Vec<u16> = pairs.iter().map(|pair| unit(*pair)).collect();
        String::from_utf16(&units).ok()
    }
    match bytes {
        [0xEF, 0xBB, 0xBF, body @ ..] => {
            Some((String::from_utf8(body.to_vec()).ok()?, Encoding::Utf8Bom))
        }
        [0xFF, 0xFE, body @ ..] => Some((utf16(body, u16::from_le_bytes)?, Encoding::Utf16Le)),
        [0xFE, 0xFF, body @ ..] => Some((utf16(body, u16::from_be_bytes)?, Encoding::Utf16Be)),
        body => Some((String::from_utf8(body.to_vec()).ok()?, Encoding::Utf8)),
    }
}

fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF], text.as_bytes()].concat(),
        Encoding::Utf16Le => [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
        Encoding::Utf16Be => [0xFE, 0xFF]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Encoding, read, write};

    #[test]
    fn a_file_reads_back_in_the_encoding_it_was_written_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query.sql");
        let text = "SELECT N'café'\r\n";
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            write(&path, text, encoding).unwrap();
            assert_eq!(read(&path), Ok((text.to_owned(), encoding)));
        }
    }

    #[test]
    fn a_file_that_is_not_text_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.sql");
        std::fs::write(&path, [0x89, b'P', b'N', b'G', 0xFF, 0x00]).unwrap();

        assert_eq!(
            read(&path),
            Err("it isn't valid UTF-8 or UTF-16 text.".to_owned())
        );
    }
}
