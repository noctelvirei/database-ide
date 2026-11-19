//! The plan graph's operator icons: vscode-mssql's execution plan PNGs (MIT; the notice is in
//! `assets/plan-icons/LICENSE.txt`), embedded in the binary, and which one each operator gets.
//! SQL Server's operators match an icon by name; PostgreSQL's and MySQL's by the words their
//! names hold; anything else gets the catch-all icon.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use dbide_engine::plan::Operator;
use gpui::{Image, ImageFormat};

/// Every icon by its file name without `.png`, sorted.
const ICONS: [(&str, &[u8]); 113] = [
    (
        "adaptive_join",
        include_bytes!("../../../assets/plan-icons/adaptive_join.png"),
    ),
    (
        "aggregate",
        include_bytes!("../../../assets/plan-icons/aggregate.png"),
    ),
    (
        "apply",
        include_bytes!("../../../assets/plan-icons/apply.png"),
    ),
    (
        "arithmetic_expression",
        include_bytes!("../../../assets/plan-icons/arithmetic_expression.png"),
    ),
    (
        "assert",
        include_bytes!("../../../assets/plan-icons/assert.png"),
    ),
    (
        "assign",
        include_bytes!("../../../assets/plan-icons/assign.png"),
    ),
    (
        "batch_hash_table_build",
        include_bytes!("../../../assets/plan-icons/batch_hash_table_build.png"),
    ),
    (
        "bitmap",
        include_bytes!("../../../assets/plan-icons/bitmap.png"),
    ),
    (
        "bookmark_lookup",
        include_bytes!("../../../assets/plan-icons/bookmark_lookup.png"),
    ),
    (
        "broadcast",
        include_bytes!("../../../assets/plan-icons/broadcast.png"),
    ),
    (
        "clustered_index_delete",
        include_bytes!("../../../assets/plan-icons/clustered_index_delete.png"),
    ),
    (
        "clustered_index_insert",
        include_bytes!("../../../assets/plan-icons/clustered_index_insert.png"),
    ),
    (
        "clustered_index_merge",
        include_bytes!("../../../assets/plan-icons/clustered_index_merge.png"),
    ),
    (
        "clustered_index_scan",
        include_bytes!("../../../assets/plan-icons/clustered_index_scan.png"),
    ),
    (
        "clustered_index_seek",
        include_bytes!("../../../assets/plan-icons/clustered_index_seek.png"),
    ),
    (
        "clustered_index_update",
        include_bytes!("../../../assets/plan-icons/clustered_index_update.png"),
    ),
    (
        "clustered_update",
        include_bytes!("../../../assets/plan-icons/clustered_update.png"),
    ),
    (
        "collapse",
        include_bytes!("../../../assets/plan-icons/collapse.png"),
    ),
    (
        "columnstore_index_delete",
        include_bytes!("../../../assets/plan-icons/columnstore_index_delete.png"),
    ),
    (
        "columnstore_index_insert",
        include_bytes!("../../../assets/plan-icons/columnstore_index_insert.png"),
    ),
    (
        "columnstore_index_merge",
        include_bytes!("../../../assets/plan-icons/columnstore_index_merge.png"),
    ),
    (
        "columnstore_index_scan",
        include_bytes!("../../../assets/plan-icons/columnstore_index_scan.png"),
    ),
    (
        "columnstore_index_update",
        include_bytes!("../../../assets/plan-icons/columnstore_index_update.png"),
    ),
    (
        "compute_scalar",
        include_bytes!("../../../assets/plan-icons/compute_scalar.png"),
    ),
    (
        "compute_to_control_node",
        include_bytes!("../../../assets/plan-icons/compute_to_control_node.png"),
    ),
    (
        "concatenation",
        include_bytes!("../../../assets/plan-icons/concatenation.png"),
    ),
    (
        "const_table_get",
        include_bytes!("../../../assets/plan-icons/const_table_get.png"),
    ),
    (
        "constant_scan",
        include_bytes!("../../../assets/plan-icons/constant_scan.png"),
    ),
    (
        "control_to_compute_nodes",
        include_bytes!("../../../assets/plan-icons/control_to_compute_nodes.png"),
    ),
    (
        "convert",
        include_bytes!("../../../assets/plan-icons/convert.png"),
    ),
    (
        "cursor_catch_all",
        include_bytes!("../../../assets/plan-icons/cursor_catch_all.png"),
    ),
    (
        "declare",
        include_bytes!("../../../assets/plan-icons/declare.png"),
    ),
    (
        "delete",
        include_bytes!("../../../assets/plan-icons/delete.png"),
    ),
    (
        "deleted_scan",
        include_bytes!("../../../assets/plan-icons/deleted_scan.png"),
    ),
    (
        "dynamic",
        include_bytes!("../../../assets/plan-icons/dynamic.png"),
    ),
    (
        "external_broadcast",
        include_bytes!("../../../assets/plan-icons/external_broadcast.png"),
    ),
    (
        "external_export",
        include_bytes!("../../../assets/plan-icons/external_export.png"),
    ),
    (
        "external_local_streaming",
        include_bytes!("../../../assets/plan-icons/external_local_streaming.png"),
    ),
    (
        "external_round_robin",
        include_bytes!("../../../assets/plan-icons/external_round_robin.png"),
    ),
    (
        "external_shuffle",
        include_bytes!("../../../assets/plan-icons/external_shuffle.png"),
    ),
    (
        "fetch_query",
        include_bytes!("../../../assets/plan-icons/fetch_query.png"),
    ),
    (
        "filter",
        include_bytes!("../../../assets/plan-icons/filter.png"),
    ),
    (
        "foreign_key_references_check",
        include_bytes!("../../../assets/plan-icons/foreign_key_references_check.png"),
    ),
    ("get", include_bytes!("../../../assets/plan-icons/get.png")),
    (
        "group_by_aggregate",
        include_bytes!("../../../assets/plan-icons/group_by_aggregate.png"),
    ),
    (
        "hash_match",
        include_bytes!("../../../assets/plan-icons/hash_match.png"),
    ),
    (
        "hash_match_root",
        include_bytes!("../../../assets/plan-icons/hash_match_root.png"),
    ),
    (
        "hash_match_team",
        include_bytes!("../../../assets/plan-icons/hash_match_team.png"),
    ),
    ("if", include_bytes!("../../../assets/plan-icons/if.png")),
    (
        "index_delete",
        include_bytes!("../../../assets/plan-icons/index_delete.png"),
    ),
    (
        "index_insert",
        include_bytes!("../../../assets/plan-icons/index_insert.png"),
    ),
    (
        "index_scan",
        include_bytes!("../../../assets/plan-icons/index_scan.png"),
    ),
    (
        "index_seek",
        include_bytes!("../../../assets/plan-icons/index_seek.png"),
    ),
    (
        "index_spool",
        include_bytes!("../../../assets/plan-icons/index_spool.png"),
    ),
    (
        "index_update",
        include_bytes!("../../../assets/plan-icons/index_update.png"),
    ),
    (
        "insert",
        include_bytes!("../../../assets/plan-icons/insert.png"),
    ),
    (
        "inserted_scan",
        include_bytes!("../../../assets/plan-icons/inserted_scan.png"),
    ),
    (
        "intrinsic",
        include_bytes!("../../../assets/plan-icons/intrinsic.png"),
    ),
    (
        "iterator_catch_all",
        include_bytes!("../../../assets/plan-icons/iterator_catch_all.png"),
    ),
    (
        "join",
        include_bytes!("../../../assets/plan-icons/join.png"),
    ),
    (
        "keyset",
        include_bytes!("../../../assets/plan-icons/keyset.png"),
    ),
    (
        "language_construct_catch_all",
        include_bytes!("../../../assets/plan-icons/language_construct_catch_all.png"),
    ),
    (
        "locate",
        include_bytes!("../../../assets/plan-icons/locate.png"),
    ),
    (
        "log_row_scan",
        include_bytes!("../../../assets/plan-icons/log_row_scan.png"),
    ),
    (
        "merge_interval",
        include_bytes!("../../../assets/plan-icons/merge_interval.png"),
    ),
    (
        "merge_join",
        include_bytes!("../../../assets/plan-icons/merge_join.png"),
    ),
    (
        "nested_loops",
        include_bytes!("../../../assets/plan-icons/nested_loops.png"),
    ),
    (
        "parallelism",
        include_bytes!("../../../assets/plan-icons/parallelism.png"),
    ),
    (
        "parameter_table_scan",
        include_bytes!("../../../assets/plan-icons/parameter_table_scan.png"),
    ),
    (
        "population_query",
        include_bytes!("../../../assets/plan-icons/population_query.png"),
    ),
    (
        "predict",
        include_bytes!("../../../assets/plan-icons/predict.png"),
    ),
    (
        "print",
        include_bytes!("../../../assets/plan-icons/print.png"),
    ),
    (
        "project",
        include_bytes!("../../../assets/plan-icons/project.png"),
    ),
    (
        "rank",
        include_bytes!("../../../assets/plan-icons/rank.png"),
    ),
    (
        "refresh_query",
        include_bytes!("../../../assets/plan-icons/refresh_query.png"),
    ),
    (
        "remote_delete",
        include_bytes!("../../../assets/plan-icons/remote_delete.png"),
    ),
    (
        "remote_index_scan",
        include_bytes!("../../../assets/plan-icons/remote_index_scan.png"),
    ),
    (
        "remote_index_seek",
        include_bytes!("../../../assets/plan-icons/remote_index_seek.png"),
    ),
    (
        "remote_insert",
        include_bytes!("../../../assets/plan-icons/remote_insert.png"),
    ),
    (
        "remote_query",
        include_bytes!("../../../assets/plan-icons/remote_query.png"),
    ),
    (
        "remote_scan",
        include_bytes!("../../../assets/plan-icons/remote_scan.png"),
    ),
    (
        "remote_update",
        include_bytes!("../../../assets/plan-icons/remote_update.png"),
    ),
    (
        "result",
        include_bytes!("../../../assets/plan-icons/result.png"),
    ),
    (
        "rid_lookup",
        include_bytes!("../../../assets/plan-icons/rid_lookup.png"),
    ),
    (
        "row_count_spool",
        include_bytes!("../../../assets/plan-icons/row_count_spool.png"),
    ),
    (
        "segment",
        include_bytes!("../../../assets/plan-icons/segment.png"),
    ),
    (
        "sequence",
        include_bytes!("../../../assets/plan-icons/sequence.png"),
    ),
    (
        "sequence_project",
        include_bytes!("../../../assets/plan-icons/sequence_project.png"),
    ),
    (
        "set_function",
        include_bytes!("../../../assets/plan-icons/set_function.png"),
    ),
    (
        "shuffle",
        include_bytes!("../../../assets/plan-icons/shuffle.png"),
    ),
    (
        "single_source_round_robin",
        include_bytes!("../../../assets/plan-icons/single_source_round_robin.png"),
    ),
    (
        "single_source_shuffle",
        include_bytes!("../../../assets/plan-icons/single_source_shuffle.png"),
    ),
    (
        "snapshot",
        include_bytes!("../../../assets/plan-icons/snapshot.png"),
    ),
    (
        "sort",
        include_bytes!("../../../assets/plan-icons/sort.png"),
    ),
    (
        "split",
        include_bytes!("../../../assets/plan-icons/split.png"),
    ),
    (
        "spool",
        include_bytes!("../../../assets/plan-icons/spool.png"),
    ),
    ("sql", include_bytes!("../../../assets/plan-icons/sql.png")),
    (
        "stream_aggregate",
        include_bytes!("../../../assets/plan-icons/stream_aggregate.png"),
    ),
    (
        "switch",
        include_bytes!("../../../assets/plan-icons/switch.png"),
    ),
    (
        "table_delete",
        include_bytes!("../../../assets/plan-icons/table_delete.png"),
    ),
    (
        "table_insert",
        include_bytes!("../../../assets/plan-icons/table_insert.png"),
    ),
    (
        "table_merge",
        include_bytes!("../../../assets/plan-icons/table_merge.png"),
    ),
    (
        "table_scan",
        include_bytes!("../../../assets/plan-icons/table_scan.png"),
    ),
    (
        "table_spool",
        include_bytes!("../../../assets/plan-icons/table_spool.png"),
    ),
    (
        "table_update",
        include_bytes!("../../../assets/plan-icons/table_update.png"),
    ),
    (
        "table_valued_function",
        include_bytes!("../../../assets/plan-icons/table_valued_function.png"),
    ),
    ("top", include_bytes!("../../../assets/plan-icons/top.png")),
    (
        "trim",
        include_bytes!("../../../assets/plan-icons/trim.png"),
    ),
    ("udx", include_bytes!("../../../assets/plan-icons/udx.png")),
    (
        "union",
        include_bytes!("../../../assets/plan-icons/union.png"),
    ),
    (
        "union_all",
        include_bytes!("../../../assets/plan-icons/union_all.png"),
    ),
    (
        "update",
        include_bytes!("../../../assets/plan-icons/update.png"),
    ),
    (
        "window_aggregate",
        include_bytes!("../../../assets/plan-icons/window_aggregate.png"),
    ),
];

/// The icon for an operator nothing else matches.
const CATCH_ALL: &str = "iterator_catch_all";

/// Words in a PostgreSQL or MySQL operator's name, lower case, and the icon they get. The
/// first match wins, so longer phrases come before the words they hold.
const BY_WORDS: [(&str, &str); 37] = [
    ("nested loop", "nested_loops"),
    ("hash join", "hash_match"),
    ("hash semijoin", "hash_match"),
    ("hash antijoin", "hash_match"),
    ("merge join", "merge_join"),
    ("index only scan", "index_scan"),
    ("covering index", "index_scan"),
    ("index scan", "index_seek"),
    ("index lookup", "index_seek"),
    ("index range scan", "index_seek"),
    ("single-row index", "index_seek"),
    ("bitmap index scan", "index_seek"),
    ("bitmap heap scan", "rid_lookup"),
    ("seq scan", "table_scan"),
    ("table scan", "table_scan"),
    ("hashaggregate", "hash_match"),
    ("groupaggregate", "stream_aggregate"),
    ("windowagg", "window_aggregate"),
    ("window", "window_aggregate"),
    ("aggregate", "aggregate"),
    ("group", "stream_aggregate"),
    ("incremental sort", "sort"),
    ("sort", "sort"),
    ("limit", "top"),
    ("gather", "parallelism"),
    ("materialize", "table_spool"),
    ("memoize", "index_spool"),
    ("cte scan", "table_spool"),
    ("function scan", "table_valued_function"),
    ("values scan", "constant_scan"),
    ("append", "concatenation"),
    ("union", "union"),
    ("unique", "stream_aggregate"),
    ("filter", "filter"),
    ("hash", "hash_match"),
    ("result", "compute_scalar"),
    ("modifytable", "update"),
];

/// The decoded icons, made once: an image's ID is a hash of its bytes.
static IMAGES: LazyLock<HashMap<&'static str, Arc<Image>>> = LazyLock::new(|| {
    ICONS
        .iter()
        .map(|(name, bytes)| {
            let image = Image::from_bytes(ImageFormat::Png, bytes.to_vec());
            (*name, Arc::new(image))
        })
        .collect()
});

/// The icon for `operator`; `root` says it is a SQL Server statement's synthetic root.
pub fn for_operator(operator: &Operator, root: bool) -> Arc<Image> {
    IMAGES[name_for(operator, root)].clone()
}

/// The icon's file name for `operator`.
fn name_for(operator: &Operator, root: bool) -> &'static str {
    if root {
        return "result";
    }
    let snake = operator.name.to_lowercase().replace(' ', "_");
    if let Ok(ix) = ICONS.binary_search_by(|(name, _)| name.cmp(&snake.as_str())) {
        return ICONS[ix].0;
    }
    let words = format!("{} {}", operator.name, operator.kind).to_lowercase();
    BY_WORDS
        .iter()
        .find(|(phrase, _)| words.contains(phrase))
        .map_or(CATCH_ALL, |(_, icon)| icon)
}
