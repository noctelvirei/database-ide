use std::{fs, path::PathBuf};

use dbide_engine::{
    plan::{self, Operator, showplan},
    session::PlanFormat,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plans")
        .join(name)
}

fn sqlserver(name: &str) -> plan::Plan {
    showplan::open(fixture(name)).unwrap()
}

fn postgres() -> plan::Plan {
    plan::parse(
        include_str!("fixtures/plans/postgres-shop.json"),
        PlanFormat::ExplainJson,
    )
    .unwrap()
}

fn by_id<'a>(root: &'a Operator, id: &str) -> Option<&'a Operator> {
    if root
        .properties
        .iter()
        .any(|property| property.name == "/RelOp/@NodeId" && property.value == id)
    {
        Some(root)
    } else {
        root.children.iter().find_map(|child| by_id(child, id))
    }
}

#[test]
fn sqlserver_actual_plan_has_root_select_with_one_row() {
    let plan = sqlserver("row-count-spool-slow.sqlplan");
    assert_eq!(plan.statements.len(), 1);
    assert_eq!(plan.statements[0].root.name, "SELECT");
    assert_eq!(plan.statements[0].root.actual_rows, Some(1.0));
    assert_eq!(plan.statements[0].root.elapsed_ms, Some(1_570_273.0));
    assert_eq!(plan.statements[0].root.cpu_ms, Some(9_377_568.0));
    assert!(
        plan.statements[0]
            .text
            .as_ref()
            .unwrap()
            .starts_with("SELECT COUNT(*)\r\nFROM #NewUsers")
    );
}

#[test]
fn key_lookup_estimate_error_is_over_1000x() {
    let plan = sqlserver("serially-parallel.sqlplan");
    let seek = by_id(&plan.statements[0].root, "19").unwrap();
    assert_eq!(seek.actual_rows, Some(21.0));
    assert_eq!(seek.estimated_rows_total, 22_748.6);
    assert!((seek.estimate_error().unwrap() - 1083.2666666666667).abs() < 1e-9);
}

#[test]
fn spilling_hash_match_carries_a_warning() {
    let plan = sqlserver("row-count-spool-slow.sqlplan");
    let hash = by_id(&plan.statements[0].root, "6").unwrap();
    assert_eq!(hash.name, "Hash Match");
    assert!(hash.warnings[0].contains("SpillToTempDb"));
    assert!(hash.warnings[0].contains("SpillLevel=1"));
}

#[test]
fn nested_loops_inner_side_estimate_is_multiplied_by_executions() {
    let plan = sqlserver("serially-parallel.sqlplan");
    let seek = by_id(&plan.statements[0].root, "18").unwrap();
    assert_eq!(seek.estimated_rows, 0.00210044);
    assert!((seek.estimated_rows_total - 22748.60747644).abs() < 1e-8);
    assert_eq!(seek.actual_rows, Some(27_026.0));
}

#[test]
fn parallel_counters_sum_rows_cpu_and_reads_but_take_max_elapsed() {
    let plan = sqlserver("row-count-spool-slow.sqlplan");
    let hash = by_id(&plan.statements[0].root, "6").unwrap();
    assert_eq!(hash.actual_rows, Some(1_435_072.0));
    assert_eq!(hash.cpu_ms, Some(3407.0));
    assert_eq!(hash.elapsed_ms, Some(1060.0));
    assert_eq!(hash.logical_reads, Some(144));
    assert_eq!(hash.physical_reads, Some(18));
    assert!(hash.parallel);
}

#[test]
fn lookup_exposes_table_index_reads_and_statement_cost_share() {
    let plan = sqlserver("serially-parallel.sqlplan");
    let seek = by_id(&plan.statements[0].root, "18").unwrap();
    assert_eq!(
        seek.object.as_ref().unwrap().table.as_deref(),
        Some("[Posts]")
    );
    assert_eq!(
        seek.object.as_ref().unwrap().index.as_deref(),
        Some("[PK_Posts_Id]")
    );
    assert_eq!(seek.logical_reads, Some(41_216_099));
    assert_eq!(seek.subtree_cost, 1712.29);
    assert_eq!(seek.own_cost, 1712.29);
    assert!((seek.cost_percent - 96.04821791175382).abs() < 1e-9);
}

#[test]
fn compute_scalar_inherits_untimed_input_rows_and_elapsed() {
    let plan = sqlserver("row-count-spool-slow.sqlplan");
    let scalar = by_id(&plan.statements[0].root, "1").unwrap();
    assert_eq!(scalar.actual_rows, Some(1.0));
    assert_eq!(scalar.elapsed_ms, Some(1_570_273.0));
    assert_eq!(scalar.cpu_ms, None);
}

#[test]
fn estimated_showplan_has_no_actuals() {
    let plan = sqlserver("estimated.sqlplan");
    let scan = &plan.statements[0].root.children[0];
    assert_eq!(scan.estimated_rows, 2.0);
    assert_eq!(scan.actual_rows, None);
    assert_eq!(scan.elapsed_ms, None);
    assert_eq!(scan.estimate_error(), None);
    assert_eq!(
        plan.statements[0].text.as_deref(),
        Some("SELECT N'café & tea'")
    );
}

#[test]
fn multi_statement_showplan_keeps_statement_order_and_nested_statements_separate() {
    let plan = sqlserver("multi-statement.sqlplan");
    assert_eq!(plan.statements.len(), 3);
    assert_eq!(plan.statements[0].text.as_deref(), Some("SELECT 1"));
    assert_eq!(plan.statements[1].text.as_deref(), Some("IF 1=1"));
    assert!(plan.statements[1].root.children.is_empty());
    assert_eq!(plan.statements[2].text.as_deref(), Some("SELECT 2"));
}

#[test]
fn unknown_showplan_element_is_kept_as_a_property() {
    let plan = sqlserver("future.sqlplan");
    let scan = &plan.statements[0].root.children[0];
    assert!(
        scan.properties
            .iter()
            .any(|property| property.name.ends_with("/@FutureAttribute")
                && property.value == "kept & decoded")
    );
    assert!(
        scan.properties
            .iter()
            .any(|property| property.name.ends_with("/s:FutureElement")
                && property.value == "first & <raw>")
    );
    assert!(
        scan.properties
            .iter()
            .any(|property| property.name.ends_with("/s:Nested") && property.value == "value")
    );
    assert!(
        scan.properties
            .iter()
            .any(|property| property.name.ends_with("/@Mode") && property.value == "other")
    );
    assert!(
        plan.properties
            .iter()
            .any(|property| property.name.ends_with("/@FutureVersion") && property.value == "next")
    );
    assert!(scan.warnings[0].contains("CONVERT_IMPLICIT"));
}

#[test]
fn postgres_inner_index_scan_estimate_is_multiplied_by_loops() {
    let plan = postgres();
    let scan = &plan.statements[0].root.children[0].children[0].children[0].children[0].children[1];
    assert_eq!(scan.name, "Index Scan");
    assert_eq!(
        scan.object.as_ref().unwrap().index.as_deref(),
        Some("orders_customer_ix")
    );
    assert_eq!(scan.estimated_rows, 4.0);
    assert_eq!(scan.estimated_rows_total, 10_000.0);
    assert_eq!(scan.actual_rows, Some(8800.0));
    assert_eq!(scan.elapsed_ms, Some(12.5));
    assert_eq!(scan.logical_reads, Some(22_500));
    assert_eq!(scan.physical_reads, Some(159));
    assert_eq!(scan.cpu_ms, None);
}

#[test]
fn postgres_inner_cost_preserves_reported_planner_units() {
    let plan = postgres();
    let scan = &plan.statements[0].root.children[0].children[0].children[0].children[0].children[1];
    assert_eq!(scan.subtree_cost, 24.15);
    assert_eq!(scan.own_cost, 24.15);
    assert!((scan.cost_percent - 0.49419449367062696).abs() < 1e-9);
}

#[test]
fn postgres_plan_without_analyze_has_no_actuals() {
    let plan = plan::parse(
        include_str!("fixtures/plans/postgres-estimated.json"),
        PlanFormat::ExplainJson,
    )
    .unwrap();
    assert_eq!(plan.statements[0].root.estimated_rows_total, 12.0);
    assert_eq!(plan.statements[0].root.actual_rows, None);
    assert_eq!(plan.statements[0].root.estimate_error(), None);
    assert_eq!(plan.statements[0].root.elapsed_ms, None);
    assert_eq!(plan.statements[0].root.logical_reads, None);
}

#[test]
fn postgres_unknown_nested_properties_and_statement_metadata_are_preserved() {
    let plan = plan::parse(
        include_str!("fixtures/plans/postgres-estimated.json"),
        PlanFormat::ExplainJson,
    )
    .unwrap();
    assert!(
        plan.statements[0]
            .root
            .properties
            .iter()
            .any(|property| property.name == "Future/items[1]/kind" && property.value == "\"new\"")
    );
    assert!(plan.properties.iter().any(|property| property.name == "[0]/Future Metadata/version" && property.value == "2"));
    assert_eq!(
        plan.statements[0].text.as_deref(),
        Some("SELECT * FROM items")
    );
}

#[test]
fn sqlplan_save_round_trips_bytes_exactly() {
    let plan = sqlserver("serially-parallel.sqlplan");
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("saved.sqlplan");
    showplan::save(&destination, &plan).unwrap();
    assert_eq!(
        fs::read(destination).unwrap(),
        include_bytes!("fixtures/plans/serially-parallel.sqlplan")
    );
}

#[test]
fn utf8_plan_save_preserves_payload_text_in_utf16() {
    let plan = sqlserver("estimated.sqlplan");
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("saved.sqlplan");
    showplan::save(&destination, &plan).unwrap();
    assert_eq!(&fs::read(&destination).unwrap()[..2], &[0xff, 0xfe]);
    assert_eq!(
        showplan::open(destination).unwrap().payload(),
        include_str!("fixtures/plans/estimated.sqlplan")
    );
}

#[test]
fn saving_postgres_as_sqlplan_does_not_overwrite_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("saved.sqlplan");
    fs::write(&destination, b"keep me").unwrap();
    assert!(showplan::save(&destination, &postgres()).is_err());
    assert_eq!(fs::read(destination).unwrap(), b"keep me");
}

#[test]
fn malformed_payload_is_an_error_not_a_panic() {
    assert!(
        plan::parse(
            include_str!("fixtures/plans/malformed.sqlplan"),
            PlanFormat::ShowplanXml
        )
        .is_err()
    );
    assert!(plan::parse("[{", PlanFormat::ExplainJson).is_err());
}

#[test]
fn non_plan_documents_are_rejected() {
    assert!(plan::parse("<hello />", PlanFormat::ShowplanXml).is_err());
    assert!(plan::parse("[]", PlanFormat::ExplainJson).is_err());
    assert!(plan::parse("[{}]", PlanFormat::ExplainJson).is_err());
}

#[test]
fn malformed_utf16_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("broken.sqlplan");
    fs::write(&path, [0xff, 0xfe, 0x00, 0xd8]).unwrap();
    assert!(showplan::open(&path).is_err());
    fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    assert!(showplan::open(path).is_err());
}

#[test]
fn nested_future_fields_named_plan_and_plans_are_preserved() {
    let plan = plan::parse(r#"[{"Future":{"Plan":7},"Plan":{"Node Type":"Result","Plan Rows":1,"Total Cost":1,"Future":{"Plans":{"version":2}}}}]"#, PlanFormat::ExplainJson).unwrap();
    assert!(
        plan.properties
            .iter()
            .any(|property| property.name == "[0]/Future/Plan" && property.value == "7")
    );
    assert!(
        plan.statements[0]
            .root
            .properties
            .iter()
            .any(|property| property.name == "Future/Plans/version" && property.value == "2")
    );
}

#[test]
fn postgres_batch_has_one_tree_per_statement() {
    let plan = plan::parse(r#"[{"Plan":{"Node Type":"Result","Plan Rows":1,"Total Cost":0}},{"Plan":{"Node Type":"Seq Scan","Plan Rows":2,"Total Cost":5}}]"#, PlanFormat::ExplainJson).unwrap();
    assert_eq!(plan.statements.len(), 2);
    assert_eq!(plan.statements[0].root.name, "Result");
    assert_eq!(plan.statements[0].root.cost_percent, 0.0);
    assert_eq!(plan.statements[1].root.name, "Seq Scan");
    assert_eq!(plan.statements[1].root.cost_percent, 100.0);
}

#[test]
fn zero_actual_loops_mean_zero_total_rows_and_time() {
    let plan = plan::parse(r#"[{"Plan":{"Node Type":"Result","Plan Rows":5,"Total Cost":1,"Actual Rows":0,"Actual Loops":0,"Actual Total Time":0}}]"#, PlanFormat::ExplainJson).unwrap();
    assert_eq!(plan.statements[0].root.estimated_rows_total, 0.0);
    assert_eq!(plan.statements[0].root.actual_rows, Some(0.0));
    assert_eq!(plan.statements[0].root.elapsed_ms, Some(0.0));
    assert_eq!(plan.statements[0].root.estimate_error(), Some(1.0));
}

#[test]
fn zero_estimate_with_actual_rows_has_infinite_estimate_error() {
    let plan = plan::parse(r#"[{"Plan":{"Node Type":"Result","Plan Rows":0,"Total Cost":1,"Actual Rows":2,"Actual Loops":1}}]"#, PlanFormat::ExplainJson).unwrap();
    assert_eq!(
        plan.statements[0].root.estimate_error(),
        Some(f64::INFINITY)
    );
}

#[test]
fn invalid_known_plan_fields_are_errors() {
    assert!(
        plan::parse(
            r#"[{"Plan":{"Node Type":"Result","Plan Rows":-1,"Total Cost":1}}]"#,
            PlanFormat::ExplainJson
        )
        .is_err()
    );
    assert!(
        plan::parse(
            r#"[{"Plan":{"Node Type":"Result","Plan Rows":1,"Total Cost":1,"Plans":{}}}]"#,
            PlanFormat::ExplainJson
        )
        .is_err()
    );
    assert!(plan::parse(r#"<ShowPlanXML><StmtSimple><QueryPlan><RelOp PhysicalOp="Scan" EstimateRows="NaN" EstimatedTotalSubtreeCost="1" /></QueryPlan></StmtSimple></ShowPlanXML>"#, PlanFormat::ShowplanXml).is_err());
}

#[test]
fn malformed_document_boundaries_are_errors() {
    assert!(
        plan::parse(
            "<ShowPlanXML><StmtSimple /></ShowPlanXML><extra />",
            PlanFormat::ShowplanXml
        )
        .is_err()
    );
    assert!(plan::parse("<ShowPlanXML><StmtSimple />", PlanFormat::ShowplanXml).is_err());
    assert!(
        plan::parse(
            "<!DOCTYPE ShowPlanXML><ShowPlanXML><StmtSimple /></ShowPlanXML>",
            PlanFormat::ShowplanXml
        )
        .is_err()
    );
}

#[test]
fn utf16_big_endian_and_utf8_bom_files_are_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bom.sqlplan");
    fs::write(
        &path,
        [
            b"\xef\xbb\xbf".as_slice(),
            include_bytes!("fixtures/plans/estimated.sqlplan"),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(
        showplan::open(&path).unwrap().statements[0]
            .root
            .estimated_rows,
        2.0
    );
    let mut bytes = vec![0xfe, 0xff];
    bytes.extend(
        include_str!("fixtures/plans/estimated.sqlplan")
            .encode_utf16()
            .flat_map(u16::to_be_bytes),
    );
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        showplan::open(&path).unwrap().statements[0]
            .root
            .estimated_rows,
        2.0
    );
}

#[test]
fn second_sqlplan_fixture_also_round_trips_bytes_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("saved.sqlplan");
    showplan::save(&destination, &sqlserver("row-count-spool-slow.sqlplan")).unwrap();
    assert_eq!(
        fs::read(destination).unwrap(),
        include_bytes!("fixtures/plans/row-count-spool-slow.sqlplan")
    );
}

#[test]
fn materialized_inner_scan_keeps_its_one_time_cost() {
    let plan = plan::parse(r#"[{"Plan":{"Node Type":"Nested Loop","Plan Rows":50,"Total Cost":49.36,"Plans":[{"Node Type":"Bitmap Heap Scan","Plan Rows":10,"Total Cost":39.38},{"Node Type":"Materialize","Parent Relationship":"Inner","Plan Rows":10,"Total Cost":8.51,"Plans":[{"Node Type":"Index Scan","Plan Rows":10,"Total Cost":8.46}]}]}}]"#, PlanFormat::ExplainJson).unwrap();
    let scan = &plan.statements[0].root.children[1].children[0];
    assert_eq!(scan.subtree_cost, 8.46);
    assert_eq!(scan.own_cost, 8.46);
    assert!((scan.cost_percent - 17.13938411669368).abs() < 1e-9);
}

#[test]
fn postgres_disk_sort_carries_a_warning() {
    let plan = plan::parse(r#"[{"Plan":{"Node Type":"Sort","Plan Rows":12,"Total Cost":2,"Sort Space Type":"Disk","Sort Space Used":64}}]"#, PlanFormat::ExplainJson).unwrap();
    assert_eq!(plan.statements[0].root.warnings, ["Spill to disk"]);
}

#[test]
fn missing_index_warning_is_attached_to_the_statement() {
    let plan = plan::parse(r#"<ShowPlanXML><StmtSimple StatementType="SELECT"><QueryPlan><MissingIndexes><MissingIndexGroup Impact="90"><MissingIndex Table="[items]" /></MissingIndexGroup></MissingIndexes><RelOp PhysicalOp="Table Scan" EstimateRows="1" EstimatedTotalSubtreeCost="1" /></QueryPlan></StmtSimple></ShowPlanXML>"#, PlanFormat::ShowplanXml).unwrap();
    assert!(plan.statements[0].root.warnings[0].contains("MissingIndex"));
    assert!(plan.statements[0].root.warnings[0].contains("[items]"));
    assert!(plan.statements[0].root.children[0].warnings.is_empty());
}

#[test]
fn postgres_gather_and_its_worker_subtree_are_parallel() {
    let plan = plan::parse(r#"[{"Plan":{"Node Type":"Gather","Parallel Aware":false,"Workers Planned":2,"Plan Rows":10,"Total Cost":20,"Plans":[{"Node Type":"Result","Parallel Aware":false,"Parent Relationship":"Outer","Plan Rows":5,"Total Cost":10},{"Node Type":"Result","Parent Relationship":"InitPlan","Plan Rows":1,"Total Cost":1}]}}]"#, PlanFormat::ExplainJson).unwrap();
    assert!(plan.statements[0].root.parallel);
    assert!(plan.statements[0].root.children[0].parallel);
    assert!(!plan.statements[0].root.children[1].parallel);
}
