//! SQL Server showplan XML and plan files. Saving always uses the original payload text.

use std::{fs, io::Write, path::Path};

use quick_xml::{Reader, XmlVersion, events::Event};

use super::{Object, Operator, Plan, Property, Statement, cost_percent};
use crate::session::PlanFormat;

/// XML, encoding, filesystem and required plan-shape failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A plan file could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// XML syntax was malformed.
    #[error(transparent)]
    Xml(#[from] quick_xml::Error),
    /// The BOM or byte sequence was not valid UTF-8 or UTF-16.
    #[error("invalid plan file encoding: {0}")]
    Encoding(String),
    /// The document is not showplan, or a known counter is invalid.
    #[error("invalid showplan: {0}")]
    Invalid(String),
}

/// Reads UTF-8 (optional BOM) or BOM-marked UTF-16 LE/BE, then parses the entire batch.
pub fn open(path: impl AsRef<Path>) -> Result<Plan, Error> {
    parse(&decode(&fs::read(path)?)?)
}

/// Writes the original XML text as UTF-16 LE with a BOM, replacing the destination file.
/// Refuses non-showplan documents before opening the destination; never serializes operators.
pub fn save(path: impl AsRef<Path>, plan: &Plan) -> Result<(), Error> {
    if plan.format != PlanFormat::ShowplanXml {
        return Err(Error::Invalid(
            "only showplan can be saved as .sqlplan".into(),
        ));
    }
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(plan.payload.encode_utf16().flat_map(u16::to_le_bytes));
    let path = path.as_ref();
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(&bytes)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Parses UTF-8 text already decoded by an Event::Plan producer or by `open`.
/// Unknown elements and attributes are retained. DTDs and nesting beyond 256 are rejected.
pub fn parse(payload: &str) -> Result<Plan, Error> {
    let document = xml(payload)?;
    if document.local_name() != "ShowPlanXML" {
        return Err(Error::Invalid("expected ShowPlanXML document".into()));
    }
    let mut statements = Vec::new();
    statements_in(&document, &mut statements)?;
    if statements.is_empty() {
        return Err(Error::Invalid("document contains no statements".into()));
    }
    let mut properties = Vec::new();
    document.raw("", &mut properties);
    Ok(Plan {
        payload: payload.into(),
        format: PlanFormat::ShowplanXml,
        statements,
        properties,
    })
}

fn decode(bytes: &[u8]) -> Result<String, Error> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !bytes.len().is_multiple_of(2) {
            return Err(Error::Encoding("odd UTF-16 byte count".into()));
        }
        let little_endian = bytes[0] == 0xff;
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if little_endian {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|error| Error::Encoding(error.to_string()))
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        String::from_utf8(bytes.to_vec()).map_err(|error| Error::Encoding(error.to_string()))
    }
}

#[derive(Debug)]
struct Node {
    name: String,
    attributes: Vec<Property>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn local_name(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|property| property.name == name)
            .map(|property| property.value.as_str())
    }

    fn number(&self, name: &str) -> Result<Option<f64>, Error> {
        self.attribute(name)
            .map(|text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .ok_or_else(|| Error::Invalid(format!("{name} must be a nonnegative number")))
            })
            .transpose()
    }

    fn is_statement(&self) -> bool {
        self.local_name().starts_with("Stmt")
    }

    // Descendants belonging to this operator/statement, never a nested operator/statement.
    fn owned(&self) -> Vec<&Node> {
        let mut nodes = vec![self];
        for child in &self.children {
            if child.local_name() != "RelOp" && !child.is_statement() {
                nodes.extend(child.owned());
            }
        }
        nodes
    }

    fn raw(&self, path: &str, properties: &mut Vec<Property>) {
        let path = format!("{path}/{}", self.name);
        properties.push(Property {
            name: path.clone(),
            value: self.text.clone(),
        });
        for attribute in &self.attributes {
            properties.push(Property {
                name: format!("{path}/@{}", attribute.name),
                value: attribute.value.clone(),
            });
        }
        for (index, child) in self.children.iter().enumerate() {
            if child.local_name() != "RelOp" {
                child.raw(&format!("{path}[{index}]"), properties);
            }
        }
    }
}

fn xml(payload: &str) -> Result<Node, Error> {
    let mut reader = Reader::from_str(payload.trim_start_matches('\u{feff}'));
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                if stack.len() >= 256 {
                    return Err(Error::Invalid("XML nesting exceeds 256".into()));
                }
                let mut attributes = Vec::new();
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(|error| Error::Invalid(error.to_string()))?;
                    attributes.push(Property {
                        name: attribute.key.as_ref().to_owned(),
                        value: attribute
                            .normalized_value(XmlVersion::Implicit1_0)?
                            .into_owned(),
                    });
                }
                stack.push(Node {
                    name: start.name().as_ref().to_owned(),
                    attributes,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Event::End(_) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| Error::Invalid("unexpected closing element".into()))?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else if root.replace(node).is_some() {
                    return Err(Error::Invalid("multiple document roots".into()));
                }
            }
            Event::Text(text) => {
                let text = text.xml_content(XmlVersion::Implicit1_0);
                append_text(&mut stack, &text)?;
            }
            Event::CData(text) => {
                append_text(&mut stack, &text)?;
            }
            Event::GeneralRef(reference) => {
                let escaped = format!("&{};", &*reference);
                let text = quick_xml::escape::unescape(&escaped).map_err(quick_xml::Error::from)?;
                append_text(&mut stack, &text)?;
            }
            Event::DocType(_) => return Err(Error::Invalid("DTDs are not supported".into())),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(Error::Invalid("unclosed XML element".into()));
    }
    root.ok_or_else(|| Error::Invalid("empty document".into()))
}

fn append_text(stack: &mut [Node], text: &str) -> Result<(), Error> {
    if let Some(node) = stack.last_mut() {
        node.text.push_str(text);
    } else if !text.trim().is_empty() {
        return Err(Error::Invalid("text outside document root".into()));
    }
    Ok(())
}

fn inputs<'a>(node: &'a Node, operators: &mut Vec<&'a Node>) {
    for child in &node.children {
        if child.local_name() == "RelOp" {
            operators.push(child);
        } else if !child.is_statement() {
            inputs(child, operators);
        }
    }
}

fn statements_in(node: &Node, statements: &mut Vec<Statement>) -> Result<(), Error> {
    if node.is_statement() {
        let statement_cost = node.number("StatementSubTreeCost")?.unwrap_or(0.0);
        let mut roots = Vec::new();
        inputs(node, &mut roots);
        let children: Vec<Operator> = roots
            .iter()
            .map(|root| operator(root, statement_cost))
            .collect::<Result<_, _>>()?;
        let name = node.attribute("StatementType").unwrap_or(node.local_name());
        let owned = node.owned();
        let timing = owned
            .iter()
            .find(|node| node.local_name() == "QueryTimeStats");
        let first = children.first();
        let rows = node
            .number("StatementEstRows")?
            .unwrap_or_else(|| first.map(|child| child.estimated_rows_total).unwrap_or(0.0));
        let mut properties = Vec::new();
        node.raw("", &mut properties);
        statements.push(Statement {
            text: node.attribute("StatementText").map(String::from),
            root: Operator {
                name: name.into(),
                kind: name.into(),
                estimated_rows: rows,
                estimated_rows_total: rows,
                actual_rows: first.and_then(|child| child.actual_rows),
                subtree_cost: statement_cost,
                own_cost: 0.0,
                cost_percent: 0.0,
                elapsed_ms: timing
                    .map(|timing| timing.number("ElapsedTime"))
                    .transpose()?
                    .flatten()
                    .or_else(|| first.and_then(|child| child.elapsed_ms)),
                cpu_ms: timing
                    .map(|timing| timing.number("CpuTime"))
                    .transpose()?
                    .flatten(),
                logical_reads: None,
                physical_reads: None,
                warnings: warnings(&owned),
                object: None,
                parallel: false,
                children,
                properties,
            },
        });
    }
    for child in &node.children {
        statements_in(child, statements)?;
    }
    Ok(())
}

fn warnings(nodes: &[&Node]) -> Vec<String> {
    let mut warnings = Vec::new();
    for node in nodes {
        if matches!(node.local_name(), "Warnings" | "MissingIndexes") {
            let mut properties = Vec::new();
            node.raw("", &mut properties);
            warnings.push(
                properties
                    .iter()
                    .map(|property| format!("{}={}", property.name, property.value))
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
    }
    warnings
}

fn counter_sum(nodes: &[&Node], names: &[&str]) -> Result<Option<f64>, Error> {
    let mut sum = None;
    for node in nodes {
        for name in names {
            if let Some(value) = node.number(name)? {
                sum = Some(sum.unwrap_or(0.0) + value);
            }
        }
    }
    Ok(sum)
}

fn read_sum(nodes: &[&Node], names: &[&str]) -> Result<Option<u64>, Error> {
    let mut sum = None::<u64>;
    for node in nodes {
        for name in names {
            if let Some(value) = node.attribute(name) {
                let value = value
                    .parse::<u64>()
                    .map_err(|_| Error::Invalid(format!("{name} must be an unsigned integer")))?;
                sum = Some(
                    sum.unwrap_or(0)
                        .checked_add(value)
                        .ok_or_else(|| Error::Invalid("read counter overflow".into()))?,
                );
            }
        }
    }
    Ok(sum)
}

fn operator(node: &Node, statement_cost: f64) -> Result<Operator, Error> {
    let name = node
        .attribute("PhysicalOp")
        .ok_or_else(|| Error::Invalid("RelOp has no PhysicalOp".into()))?;
    let rows = node
        .number("EstimateRows")?
        .ok_or_else(|| Error::Invalid("RelOp has no EstimateRows".into()))?;
    let subtree_cost = node
        .number("EstimatedTotalSubtreeCost")?
        .ok_or_else(|| Error::Invalid("RelOp has no EstimatedTotalSubtreeCost".into()))?;
    let executions = 1.0
        + node.number("EstimateRebinds")?.unwrap_or(0.0)
        + node.number("EstimateRewinds")?.unwrap_or(0.0);
    let mut roots = Vec::new();
    inputs(node, &mut roots);
    let children: Vec<Operator> = roots
        .iter()
        .map(|child| operator(child, statement_cost))
        .collect::<Result<_, _>>()?;
    let own_cost =
        (subtree_cost - children.iter().map(|child| child.subtree_cost).sum::<f64>()).max(0.0);
    let owned = node.owned();
    let counters: Vec<&Node> = owned
        .iter()
        .copied()
        .filter(|node| node.local_name() == "RunTimeCountersPerThread")
        .collect();
    let mut actual_rows = counter_sum(&counters, &["ActualRows"])?;
    let mut elapsed_ms = None::<f64>;
    for counter in &counters {
        if let Some(elapsed) = counter.number("ActualElapsedms")? {
            elapsed_ms = Some(elapsed_ms.unwrap_or(0.0).max(elapsed));
        }
    }
    // SQL Server defers Compute Scalar evaluation to its input, so it omits runtime counters.
    if name == "Compute Scalar" && actual_rows.is_none() {
        actual_rows = children.first().and_then(|child| child.actual_rows);
        elapsed_ms = children.first().and_then(|child| child.elapsed_ms);
    }
    let object = owned
        .iter()
        .find(|node| node.local_name() == "Object")
        .map(|node| Object {
            database: node.attribute("Database").map(String::from),
            schema: node.attribute("Schema").map(String::from),
            table: node.attribute("Table").map(String::from),
            index: node.attribute("Index").map(String::from),
        });
    let mut properties = Vec::new();
    node.raw("", &mut properties);
    Ok(Operator {
        name: name.into(),
        kind: node.attribute("LogicalOp").unwrap_or(name).into(),
        estimated_rows: rows,
        estimated_rows_total: rows * executions,
        actual_rows,
        subtree_cost,
        own_cost,
        cost_percent: cost_percent(own_cost, statement_cost),
        elapsed_ms,
        cpu_ms: counter_sum(&counters, &["ActualCPUms"])?,
        logical_reads: read_sum(&counters, &["ActualLogicalReads", "ActualLobLogicalReads"])?,
        physical_reads: read_sum(
            &counters,
            &["ActualPhysicalReads", "ActualLobPhysicalReads"],
        )?,
        warnings: warnings(&owned),
        object,
        parallel: node.attribute("Parallel") == Some("true"),
        children,
        properties,
    })
}
