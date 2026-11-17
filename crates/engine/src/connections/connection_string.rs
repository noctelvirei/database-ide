//! Paste ADO.NET, libpq or JDBC strings without putting passwords in saved details or errors.

use super::credentials::Secret;
use super::{Connection, SignInMethod, TlsMode};
use crate::danger::Engine;

/// A malformed or unsupported connection string; messages never echo the input.
#[derive(Debug, thiserror::Error)]
#[error("invalid connection string: {0}")]
pub struct Error(&'static str);

/// Parsed details and an optional password that the caller may explicitly save.
#[derive(Debug)]
pub struct ParsedConnection {
    /// Password-free details; the host is used as the initial display name.
    pub connection: Connection,
    /// A separately held password, redacted in Debug and never serialized.
    pub password: Option<Secret>,
}

/// Detect the engine from libpq/JDBC/URIs or an ADO.NET Provider; otherwise use SQL Server.
/// Quoted/escaped values are decoded. Unsupported options are ignored; malformed syntax,
/// missing hosts and endpoints the single-server model cannot represent are rejected.
pub fn parse(input: &str) -> Result<ParsedConnection, Error> {
    let input = input.trim();
    let mut parsed;
    if let Some(rest) = input.strip_prefix("jdbc:sqlserver://") {
        let (address, options) = rest.split_once(';').unwrap_or((rest, ""));
        parsed = empty(Engine::SqlServer);
        if !address.is_empty() {
            endpoint(&mut parsed.connection, address, ':')?;
        }
        apply(&mut parsed, pairs(options, Syntax::Jdbc)?, true)?;
    } else if let Some(rest) = input.strip_prefix("jdbc:postgresql://") {
        parsed = uri(rest, Engine::PostgreSql, true)?;
    } else if let Some(rest) = input.strip_prefix("jdbc:mysql://") {
        parsed = uri(rest, Engine::MySql, true)?;
    } else if let Some(rest) = input.strip_prefix("mysql://") {
        parsed = uri(rest, Engine::MySql, false)?;
    } else if let Some(rest) = input
        .strip_prefix("postgresql://")
        .or_else(|| input.strip_prefix("postgres://"))
    {
        parsed = uri(rest, Engine::PostgreSql, false)?;
    } else {
        let key = input
            .split_once('=')
            .ok_or(Error("expected key=value or a supported URL"))?
            .0
            .trim();
        let keyword_pairs = pairs(input, Syntax::Libpq);
        let libpq = matches!(
            key,
            "host"
                | "hostaddr"
                | "port"
                | "dbname"
                | "sslmode"
                | "connect_timeout"
                | "application_name"
        ) || keyword_pairs.as_ref().is_ok_and(|pairs| {
            pairs
                .iter()
                .any(|(key, _)| matches!(key.as_str(), "host" | "hostaddr" | "dbname"))
        });
        parsed = empty(if libpq {
            Engine::PostgreSql
        } else {
            Engine::SqlServer
        });
        let options = if libpq {
            keyword_pairs?
        } else {
            pairs(input, Syntax::Ado)?
        };
        apply(&mut parsed, options, false)?;
    }
    if parsed.connection.host.trim().is_empty() {
        return Err(Error("host is required"));
    }
    parsed.connection.name = parsed.connection.host.clone();
    parsed
        .connection
        .validate()
        .map_err(|_| Error("invalid server details"))?;
    Ok(parsed)
}

fn empty(engine: Engine) -> ParsedConnection {
    ParsedConnection {
        connection: Connection::new(engine, ""),
        password: None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Syntax {
    Ado,
    Libpq,
    Jdbc,
}

fn pairs(mut input: &str, syntax: Syntax) -> Result<Vec<(String, String)>, Error> {
    let mut pairs = Vec::new();
    while !input.trim().is_empty() {
        input = input.trim_start();
        if syntax != Syntax::Libpq && input.starts_with(';') {
            input = &input[1..];
            continue;
        }
        let equals = input.find('=').ok_or(Error("expected key=value"))?;
        let key = input[..equals].trim();
        if key.is_empty()
            || key.contains(';')
            || (syntax == Syntax::Libpq && key.chars().any(char::is_whitespace))
        {
            return Err(Error("invalid option name"));
        }
        input = input[equals + 1..].trim_start();
        let mut value = String::new();
        let quote = input.chars().next().filter(|c| match syntax {
            Syntax::Ado => matches!(c, '\'' | '"'),
            Syntax::Libpq => *c == '\'',
            Syntax::Jdbc => *c == '{',
        });
        if let Some(open) = quote {
            input = &input[open.len_utf8()..];
            let close = if open == '{' { '}' } else { open };
            loop {
                let ch = input
                    .chars()
                    .next()
                    .ok_or(Error("unterminated quoted value"))?;
                input = &input[ch.len_utf8()..];
                if ch == close {
                    if syntax != Syntax::Libpq && input.starts_with(close) {
                        value.push(close);
                        input = &input[close.len_utf8()..];
                    } else {
                        break;
                    }
                } else if syntax == Syntax::Libpq && ch == '\\' {
                    let escaped = input.chars().next().ok_or(Error("trailing escape"))?;
                    value.push(escaped);
                    input = &input[escaped.len_utf8()..];
                } else {
                    value.push(ch);
                }
            }
            if !input.is_empty()
                && !input.starts_with(';')
                && !input.starts_with(char::is_whitespace)
            {
                return Err(Error("text after quoted value"));
            }
            if syntax != Syntax::Libpq {
                input = input.trim_start();
                if !input.is_empty() && !input.starts_with(';') {
                    return Err(Error("expected semicolon"));
                }
            }
        } else {
            while let Some(ch) = input.chars().next() {
                if (syntax == Syntax::Libpq && ch.is_whitespace())
                    || (syntax != Syntax::Libpq && ch == ';')
                {
                    break;
                }
                input = &input[ch.len_utf8()..];
                if syntax == Syntax::Libpq && ch == '\\' {
                    let escaped = input.chars().next().ok_or(Error("trailing escape"))?;
                    value.push(escaped);
                    input = &input[escaped.len_utf8()..];
                } else {
                    value.push(ch);
                }
            }
            if syntax != Syntax::Libpq {
                value = value.trim().into();
            }
        }
        pairs.push((key.to_owned(), value));
    }
    Ok(pairs)
}

fn apply(
    parsed: &mut ParsedConnection,
    pairs: Vec<(String, String)>,
    jdbc: bool,
) -> Result<(), Error> {
    let mysql_jdbc = jdbc && parsed.connection.engine == Engine::MySql;
    let has_ssl_mode = pairs
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("sslmode"));
    let mut integrated = parsed.connection.sign_in == SignInMethod::Windows;
    for (key, value) in pairs {
        let key = key.to_ascii_lowercase().replace([' ', '_'], "");
        let connection = &mut parsed.connection;
        match key.as_str() {
            "provider" if value.eq_ignore_ascii_case("mysql") => {
                connection.engine = Engine::MySql;
            }
            "server" | "datasource" | "address" | "addr" | "networkaddress" | "servername" => {
                endpoint(connection, &value, ',')?
            }
            "host" | "hostaddr" => {
                if value.contains(',') {
                    return Err(Error("multiple hosts are unsupported"));
                }
                connection.host = value;
            }
            "port" | "portnumber" => {
                connection.port = Some(port(&value)?);
                connection.instance = None;
            }
            "instancename" => {
                connection.instance = Some(value);
            }
            "database" | "initialcatalog" | "dbname" | "databasename" => {
                connection.database = Some(value)
            }
            "user" | "userid" | "uid" | "username" => connection.user = Some(value),
            "password" | "pwd" => parsed.password = Some(Secret(value)),
            "integratedsecurity" | "trustedconnection" => integrated = boolean(&value, true)?,
            "trustservercertificate" => {
                connection.trust_server_certificate = boolean(&value, false)?
            }
            "encrypt" => {
                connection.tls = match value.to_ascii_lowercase().as_str() {
                    "strict" | "mandatory" | "true" | "yes" => TlsMode::Mandatory,
                    "optional" | "false" | "no" => TlsMode::Optional,
                    "off" => TlsMode::Off,
                    _ => return Err(Error("invalid encryption mode")),
                }
            }
            "sslmode" => {
                let (mode, trust) = match value.to_ascii_lowercase().as_str() {
                    "disable" | "disabled" => (TlsMode::Off, false),
                    "allow" | "prefer" | "preferred" => (TlsMode::Optional, true),
                    "require" | "required" => (TlsMode::Mandatory, true),
                    "verify-ca" | "verify-full" | "verifyca" | "verifyidentity" | "verify_ca"
                    | "verify_identity" => (TlsMode::Mandatory, false),
                    _ => return Err(Error("invalid TLS mode")),
                };
                connection.tls = mode;
                connection.trust_server_certificate = trust;
            }
            "applicationname" | "app" => connection.application_name = value,
            "connecttimeout" | "connectiontimeout" | "logintimeout" | "timeout" => {
                connection.connect_timeout = seconds(&value, mysql_jdbc)?;
            }
            "querytimeout" | "commandtimeout" | "sockettimeout" => {
                connection.query_timeout = seconds(&value, mysql_jdbc && key == "sockettimeout")?;
            }
            "usessl" if !has_ssl_mode => {
                connection.tls = if boolean(&value, false)? {
                    TlsMode::Mandatory
                } else {
                    TlsMode::Off
                }
            }
            "ssl" if !has_ssl_mode => {
                if boolean(&value, false)? {
                    connection.tls = TlsMode::Mandatory;
                    connection.trust_server_certificate = !jdbc;
                } else {
                    connection.tls = TlsMode::Off;
                }
            }
            _ => {}
        }
    }
    parsed.connection.sign_in = if integrated {
        SignInMethod::Windows
    } else if parsed.password.is_some() {
        SignInMethod::Password
    } else {
        SignInMethod::AskEachTime
    };
    Ok(())
}

fn boolean(input: &str, sspi: bool) -> Result<bool, Error> {
    match input.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        "sspi" if sspi => Ok(true),
        _ => Err(Error("invalid boolean")),
    }
}

fn port(input: &str) -> Result<u16, Error> {
    input
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or(Error("invalid port"))
}

fn seconds(input: &str, milliseconds: bool) -> Result<u32, Error> {
    let value: u32 = input.parse().map_err(|_| Error("invalid timeout"))?;
    Ok(if milliseconds {
        value.div_ceil(1000)
    } else {
        value
    })
}

fn endpoint(connection: &mut Connection, input: &str, separator: char) -> Result<(), Error> {
    let input = input.strip_prefix("tcp:").unwrap_or(input).trim();
    let (host, suffix) = if let Some(ipv6) = input.strip_prefix('[') {
        let (host, rest) = ipv6.split_once(']').ok_or(Error("unclosed IPv6 address"))?;
        (host, rest)
    } else {
        let end = input.find(['\\', separator]).unwrap_or(input.len());
        (&input[..end], &input[end..])
    };
    if host.is_empty()
        || host.contains(['/', ',', ';', '?', '#', '@'])
        || host.chars().any(char::is_whitespace)
    {
        return Err(Error("invalid host"));
    }
    connection.host = host.into();
    connection.port = None;
    connection.instance = None;
    if let Some(instance) = suffix.strip_prefix('\\') {
        let (instance, explicit_port) = instance
            .split_once(separator)
            .map_or((instance, None), |(i, p)| (i, Some(p)));
        if instance.is_empty() {
            return Err(Error("empty instance"));
        }
        if let Some(value) = explicit_port {
            connection.port = Some(port(value)?);
        } else {
            connection.instance = Some(instance.into());
        }
    } else if let Some(value) = suffix.strip_prefix(separator) {
        connection.port = Some(port(value)?);
    } else if !suffix.is_empty() {
        return Err(Error("invalid endpoint"));
    }
    Ok(())
}

fn uri(input: &str, engine: Engine, jdbc: bool) -> Result<ParsedConnection, Error> {
    if input.contains('#') {
        return Err(Error("URL fragments are unsupported"));
    }
    let mut parsed = empty(engine);
    let (base, query) = input.split_once('?').unwrap_or((input, ""));
    let (authority, database) = base.split_once('/').unwrap_or((base, ""));
    let host = if let Some((login, host)) = authority.rsplit_once('@') {
        let (user, password) = login.split_once(':').unwrap_or((login, ""));
        parsed.connection.user = Some(percent_decode(user)?);
        if login.contains(':') {
            parsed.password = Some(Secret(percent_decode(password)?));
        }
        host
    } else {
        authority
    };
    if !host.is_empty() {
        endpoint(&mut parsed.connection, host, ':')?;
        parsed.connection.host = percent_decode(&parsed.connection.host)?;
        if parsed.connection.host.contains([',', '/', '@', ';']) {
            return Err(Error("unsupported URL host"));
        }
    }
    if !database.is_empty() {
        parsed.connection.database = Some(percent_decode(database)?);
    }
    let mut options = Vec::new();
    if !query.is_empty() {
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or(Error("invalid URL option"))?;
            let decode = |value: &str| {
                if jdbc {
                    percent_decode(&value.replace('+', " "))
                } else {
                    percent_decode(value)
                }
            };
            options.push((decode(key)?, decode(value)?));
        }
    }
    apply(&mut parsed, options, jdbc)?;
    Ok(parsed)
}

fn percent_decode(input: &str) -> Result<String, Error> {
    let mut bytes = Vec::new();
    let mut remaining = input.as_bytes();
    while let Some((&byte, rest)) = remaining.split_first() {
        if byte == b'%' {
            let hex = rest.get(..2).ok_or(Error("incomplete percent escape"))?;
            let hex = std::str::from_utf8(hex).map_err(|_| Error("invalid percent escape"))?;
            bytes.push(u8::from_str_radix(hex, 16).map_err(|_| Error("invalid percent escape"))?);
            remaining = &rest[2..];
        } else {
            bytes.push(byte);
            remaining = rest;
        }
    }
    String::from_utf8(bytes).map_err(|_| Error("invalid UTF-8 in URL"))
}
