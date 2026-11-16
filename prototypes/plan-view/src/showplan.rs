//! PROTOTYPE. Reads a SQL Server actual plan (`.sqlplan` showplan XML, UTF-8 or UTF-16) into
//! the shared operator tree. Only the first statement with a plan is read.

use roxmltree::{Document, Node};

use crate::model::{Engine, Op, Plan};

/// Decodes the file's bytes: UTF-16 LE with a BOM (what SSMS saves) or UTF-8.
fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)).into()
    }
}

fn is(node: &Node, tag: &str) -> bool {
    node.is_element() && node.tag_name().name() == tag
}

fn num(node: &Node, attr: &str) -> f64 {
    node.attribute(attr)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.)
}

/// Descendants of `relop` that belong to it and not to a nested operator.
fn own<'a, 'i>(relop: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    relop.descendants().skip(1).filter(move |d| {
        d.ancestors()
            .skip(1)
            .find(|a| is(a, "RelOp"))
            .map(|a| a.id())
            == Some(relop.id())
    })
}

fn icon_for(physical: &str, lookup: bool) -> &'static str {
    if lookup {
        return "bookmark_lookup";
    }
    match physical {
        "Compute Scalar" => "compute_scalar",
        "Stream Aggregate" => "stream_aggregate",
        "Parallelism" => "parallelism",
        "Hash Match" => "hash_match",
        "Table Scan" => "table_scan",
        "Nested Loops" => "nested_loops",
        "Top" => "top",
        "Row Count Spool" => "row_count_spool",
        "Table Spool" => "table_spool",
        "Sort" => "sort",
        "Index Scan" => "index_scan",
        "Index Seek" => "index_seek",
        "Clustered Index Seek" => "clustered_index_seek",
        "Clustered Index Scan" => "clustered_index_scan",
        "Merge Join" => "merge_join",
        "Filter" => "filter",
        "Bitmap" => "bitmap",
        _ => "iterator_catch_all",
    }
}

fn unbracket(s: &str) -> String {
    s.replace(['[', ']'], "")
}

/// Turns a `<Warnings>` child into words a DBA reads.
fn warning_text(w: &Node) -> String {
    match w.tag_name().name() {
        "SpillToTempDb" => format!(
            "Spill to tempdb (level {})",
            w.attribute("SpillLevel").unwrap_or("?")
        ),
        "HashSpillDetails" => format!(
            "Hash spill: {} pages written to tempdb",
            w.attribute("WritesToTempDb").unwrap_or("?")
        ),
        "SortSpillDetails" => "Sort spilled to tempdb".into(),
        "NoJoinPredicate" => "No join predicate".into(),
        "ColumnsWithNoStatistics" => "Columns with no statistics".into(),
        "PlanAffectingConvert" => "Implicit conversion affects the plan".into(),
        "MemoryGrantWarning" => "Memory grant warning".into(),
        other => other.to_string(),
    }
}

/// Adds `relop` and its operator children to `ops` in pre-order; returns its subtree cost.
fn walk(relop: Node, parent: Option<usize>, ops: &mut Vec<Op>, subtree: &mut Vec<f64>) {
    let physical = relop.attribute("PhysicalOp").unwrap_or("?");
    let mine: Vec<Node> = own(relop).collect();
    let lookup = mine
        .iter()
        .any(|n| is(n, "IndexScan") && matches!(n.attribute("Lookup"), Some("1" | "true")));
    let threads: Vec<Node> = mine
        .iter()
        .filter(|n| is(n, "RunTimeCountersPerThread"))
        .copied()
        .collect();
    let sum = |attr: &str| threads.iter().map(|t| num(t, attr)).fold(0., |a, b| a + b);
    let object = mine
        .iter()
        .find(|n| is(n, "Object"))
        .map(|o| {
            let table = unbracket(o.attribute("Table").unwrap_or(""));
            match o.attribute("Index") {
                Some(ix) => format!("{table}.{}", unbracket(ix)),
                None => table,
            }
        })
        .unwrap_or_default();
    let mut warnings: Vec<String> = mine
        .iter()
        .filter(|n| is(n, "Warnings"))
        .flat_map(|w| {
            w.children()
                .filter(|c| c.is_element())
                .map(|c| warning_text(&c))
        })
        .collect();
    if mine
        .iter()
        .any(|n| is(n, "Warnings") && n.attribute("NoJoinPredicate") == Some("true"))
    {
        warnings.push("No join predicate".into());
    }
    warnings.dedup();
    let mut details = Vec::new();
    for (tag, label) in [
        ("SeekPredicates", "Seek predicate"),
        ("Predicate", "Predicate"),
    ] {
        if let Some(p) = mine.iter().find(|n| is(n, tag)) {
            if let Some(s) = p.descendants().find_map(|d| d.attribute("ScalarString")) {
                details.push((label.to_string(), unbracket(s)));
            }
        }
    }
    details.push((
        "Estimated subtree cost".into(),
        relop
            .attribute("EstimatedTotalSubtreeCost")
            .unwrap_or("")
            .into(),
    ));
    details.push((
        "Estimated rows per execution".into(),
        relop.attribute("EstimateRows").unwrap_or("").into(),
    ));
    details.push((
        "Estimated rebinds / rewinds".into(),
        format!(
            "{} / {}",
            num(&relop, "EstimateRebinds"),
            num(&relop, "EstimateRewinds")
        ),
    ));
    if !threads.is_empty() {
        details.push(("Actual CPU".into(), format!("{} ms", sum("ActualCPUms"))));
        details.push(("Threads".into(), threads.len().to_string()));
    }

    let est = num(&relop, "EstimateRows")
        * (num(&relop, "EstimateRebinds") + num(&relop, "EstimateRewinds") + 1.);
    let index = ops.len();
    ops.push(Op {
        name: if lookup {
            "Key Lookup".into()
        } else {
            physical.into()
        },
        logical: relop.attribute("LogicalOp").unwrap_or("").into(),
        object,
        icon: icon_for(physical, lookup),
        parent,
        children: vec![],
        est_rows: est,
        act_rows: sum("ActualRows"),
        executions: sum("ActualExecutions"),
        cost_pct: 0.,
        subtree_cost_pct: 0.,
        self_ms: 0.,
        total_ms: threads
            .iter()
            .map(|t| num(t, "ActualElapsedms"))
            .fold(0., f64::max),
        reads: sum("ActualLogicalReads"),
        parallel: matches!(relop.attribute("Parallel"), Some("1" | "true")),
        warnings,
        details,
    });
    subtree.push(num(&relop, "EstimatedTotalSubtreeCost"));
    for child in mine.iter().filter(|n| is(n, "RelOp")) {
        walk(*child, Some(index), ops, subtree);
    }
    // Operators SQL Server doesn't time (Compute Scalar and friends) pass their first
    // input's rows and time through, as SSMS shows them.
    if threads.is_empty() && ops.len() > index + 1 {
        let (rows, ms, execs) = (
            ops[index + 1].act_rows,
            ops[index + 1].total_ms,
            ops[index + 1].executions,
        );
        let op = &mut ops[index];
        (op.act_rows, op.total_ms, op.executions) = (rows, ms, execs);
    }
}

/// Parses showplan XML bytes into a plan. Panics on a file without a statement plan, which
/// is fine for the prototype's two bundled samples.
pub fn parse(bytes: &[u8], title: &'static str) -> Plan {
    let text = decode(bytes);
    // roxmltree rejects encoding="utf-16" once the text is already decoded: drop the prolog.
    let body = match text.find("?>") {
        Some(end) if text.trim_start().starts_with("<?xml") => &text[end + 2..],
        _ => &text,
    };
    let doc = Document::parse(body).expect("bundled sample plans are well-formed XML");
    let stmt = doc
        .descendants()
        .find(|n| is(n, "StmtSimple") && n.children().any(|c| is(&c, "QueryPlan")))
        .expect("bundled sample plans hold a statement with a plan");
    let query_plan = stmt
        .children()
        .find(|c| is(c, "QueryPlan"))
        .expect("checked above");
    let top = query_plan
        .children()
        .find(|c| is(c, "RelOp"))
        .expect("a plan has a root operator");

    let mut ops = Vec::new();
    let mut subtree = Vec::new();
    // SSMS shows the statement itself as the leftmost node, with no cost of its own.
    let root_cost = num(&top, "EstimatedTotalSubtreeCost");
    ops.push(Op {
        name: stmt.attribute("StatementType").unwrap_or("SELECT").into(),
        logical: String::new(),
        object: String::new(),
        icon: "result",
        parent: None,
        children: vec![],
        est_rows: num(&stmt, "StatementEstRows"),
        act_rows: 0.,
        executions: 1.,
        cost_pct: 0.,
        subtree_cost_pct: 0.,
        self_ms: 0.,
        total_ms: 0.,
        reads: 0.,
        parallel: false,
        warnings: vec![],
        details: vec![(
            "Optimisation level".into(),
            stmt.attribute("StatementOptmLevel").unwrap_or("").into(),
        )],
    });
    subtree.push(root_cost);
    walk(top, Some(0), &mut ops, &mut subtree);
    ops[0].act_rows = ops[1].act_rows;
    ops[0].total_ms = ops[1].total_ms;

    let mut facts = Vec::new();
    let mut elapsed_ms = ops[1].total_ms;
    if let Some(dop) = query_plan.attribute("DegreeOfParallelism") {
        facts.push(("Degree of parallelism".into(), dop.into()));
    }
    for n in query_plan.children() {
        if is(&n, "MemoryGrantInfo") {
            facts.push((
                "Memory grant".into(),
                format!(
                    "{} KB granted, {} KB used",
                    num(&n, "GrantedMemory"),
                    num(&n, "MaxUsedMemory")
                ),
            ));
        }
        if is(&n, "QueryTimeStats") {
            elapsed_ms = num(&n, "ElapsedTime");
            facts.push(("CPU time".into(), format!("{} ms", num(&n, "CpuTime"))));
        }
        if is(&n, "Warnings") {
            for w in n.children().filter(|c| c.is_element()) {
                facts.push(("Plan warning".into(), warning_text(&w)));
            }
        }
    }
    facts.push((
        "Compile time".into(),
        format!("{} ms", num(&query_plan, "CompileTime")),
    ));
    let mut plan = Plan {
        engine: Engine::SqlServer,
        title,
        statement: stmt
            .attribute("StatementText")
            .unwrap_or("")
            .trim()
            .to_string(),
        ops,
        elapsed_ms,
        facts,
    };
    plan.finish(&subtree, false);
    plan
}
