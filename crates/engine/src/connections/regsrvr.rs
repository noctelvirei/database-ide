//! SSMS SFC Registered Servers documents, shared by RegSrvr.xml and exported .regsrvr files.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;

use super::{Connections, SignInMethod, connection_string};

/// Import failures omit XML contents, which may contain encrypted or plaintext passwords.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The explicit import file could not be read.
    #[error("registered servers file: {0}")]
    Io(#[from] std::io::Error),
    /// The document has invalid encoding, XML or SFC hierarchy.
    #[error("invalid Registered Servers document")]
    Invalid,
    /// A registered server has a malformed connection string.
    #[error(transparent)]
    ConnectionString(#[from] connection_string::Error),
    /// A name or connection violates the saved-tree contract.
    #[error(transparent)]
    Connections(#[from] super::Error),
}

/// Import only the explicit file, accepting UTF-8 and BOM-marked UTF-16 in either byte order.
/// Passwords are discarded; this never accesses Credential Manager or the saved connections file.
pub fn open(path: impl AsRef<Path>) -> Result<Connections, Error> {
    let bytes = std::fs::read(path)?;
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !bytes.len().is_multiple_of(2) {
            return Err(Error::Invalid);
        }
        let little = bytes[0] == 0xff;
        let words = bytes[2..].as_chunks::<2>().0.iter().map(|b| {
            if little {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        });
        char::decode_utf16(words)
            .collect::<Result<String, _>>()
            .map_err(|_| Error::Invalid)?
    } else {
        String::from_utf8(bytes).map_err(|_| Error::Invalid)?
    };
    parse(&text)
}

/// Parse decoded SSMS SFC XML. Database Engine groups become folders; other server types are ignored.
/// References to parents outside an exported subtree become roots; internal cycles are errors.
/// No password is returned or retained, and SQL logins are set to ask each time.
pub fn parse(xml: &str) -> Result<Connections, Error> {
    let records = records(xml)?;
    let aliases: HashSet<_> = records.iter().map(|r| r.alias.as_str()).collect();
    if aliases.len() != records.len() || aliases.contains("") {
        return Err(Error::Invalid);
    }
    let mut tree = Connections::default();
    let mut folders: HashMap<&str, String> = HashMap::new();
    let mut pending: Vec<_> = records.iter().filter(|r| r.kind == "ServerGroup").collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for record in pending {
            if aliases.contains(record.parent.as_str())
                && !folders.contains_key(record.parent.as_str())
            {
                next.push(record);
                continue;
            }
            let parent = folders.get(record.parent.as_str()).map(String::as_str);
            let id = tree.add_folder(record.required("Name")?, parent)?;
            folders.insert(&record.alias, id);
        }
        if before == next.len() {
            return Err(Error::Invalid);
        }
        pending = next;
    }
    for record in records.iter().filter(|r| r.kind == "RegisteredServer") {
        let string = record
            .fields
            .get("ConnectionStringWithEncryptedPassword")
            .or_else(|| record.fields.get("ConnectionString"))
            .ok_or(Error::Invalid)?;
        let mut connection = connection_string::parse(string)?.connection;
        connection.name = record.required("Name")?.into();
        if connection.sign_in != SignInMethod::Windows {
            connection.sign_in = SignInMethod::AskEachTime;
        }
        if record
            .fields
            .get("UseCustomConnectionColor")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        {
            let argb = record
                .required("CustomConnectionColorArgb")?
                .parse::<i32>()
                .map_err(|_| Error::Invalid)?;
            connection.colour = Some(format!("#{:06X}", argb as u32 & 0x00ff_ffff));
        }
        tree.add_connection(
            connection,
            folders.get(record.parent.as_str()).map(String::as_str),
        )?;
    }
    Ok(tree)
}

#[derive(Default)]
struct Record {
    alias: String,
    kind: String,
    parent: String,
    fields: HashMap<String, String>,
}

impl Record {
    fn required(&self, name: &str) -> Result<&str, Error> {
        self.fields
            .get(name)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(Error::Invalid)
    }
}

fn records(xml: &str) -> Result<Vec<Record>, Error> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut path = Vec::new();
    let mut documents: Vec<Record> = Vec::new();
    let mut records = Vec::new();
    let mut roots = 0;
    let mut saw_model = false;
    loop {
        match reader.read_event().map_err(|_| Error::Invalid)? {
            Event::Start(tag) => {
                let name = tag.local_name().as_ref().to_owned();
                if path.is_empty() {
                    roots += 1;
                    saw_model = name == "model";
                }
                if name == "document" {
                    documents.push(Record::default());
                }
                if matches!(name.as_str(), "ServerGroup" | "RegisteredServer") {
                    let record = documents.last_mut().ok_or(Error::Invalid)?;
                    if !record.kind.is_empty() {
                        return Err(Error::Invalid);
                    }
                    record.kind = name.clone();
                }
                path.push(name);
            }
            Event::End(tag) => {
                let name = path.pop().ok_or(Error::Invalid)?;
                if tag.local_name().as_ref() != name {
                    return Err(Error::Invalid);
                }
                if name == "document" {
                    let record = documents.pop().ok_or(Error::Invalid)?;
                    if !record.kind.is_empty()
                        && record
                            .fields
                            .get("ServerType")
                            .is_none_or(|v| v == "DatabaseEngine")
                    {
                        records.push(record);
                    }
                }
            }
            Event::Text(text) => append(
                &mut documents,
                &path,
                &text.xml_content(XmlVersion::Implicit1_0),
            )?,
            Event::CData(text) => append(&mut documents, &path, &text)?,
            Event::GeneralRef(reference) => {
                let escaped = format!("&{};", &*reference);
                let value = quick_xml::escape::unescape(&escaped).map_err(|_| Error::Invalid)?;
                append(&mut documents, &path, &value)?;
            }
            Event::DocType(_) => return Err(Error::Invalid),
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() || !documents.is_empty() || roots != 1 || !saw_model {
        return Err(Error::Invalid);
    }
    Ok(records)
}

fn append(documents: &mut [Record], path: &[String], text: &str) -> Result<(), Error> {
    let Some(record) = documents.last_mut() else {
        if path.is_empty() && !text.trim().is_empty() {
            return Err(Error::Invalid);
        }
        return Ok(());
    };
    let name = path.last().map(String::as_str).unwrap_or("");
    if name == "alias" {
        record.alias.push_str(text);
    } else if name == "Uri" && path.iter().any(|p| p == "Parent") {
        record.parent.push_str(text);
    } else if matches!(
        name,
        "Name"
            | "ServerType"
            | "ConnectionStringWithEncryptedPassword"
            | "ConnectionString"
            | "UseCustomConnectionColor"
            | "CustomConnectionColorArgb"
    ) && !record.kind.is_empty()
    {
        record.fields.entry(name.into()).or_default().push_str(text);
    }
    Ok(())
}
