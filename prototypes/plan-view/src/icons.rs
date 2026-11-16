//! PROTOTYPE. The vscode-mssql operator icons (MIT, see `icons/NOTICE.md`), embedded as
//! colour SVGs and handed to gpui's `img()`, which rasterises SVG with its fills intact.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use gpui::{Image, ImageFormat};

macro_rules! icon_files {
    ($($name:literal),* $(,)?) => {
        [$(($name, include_bytes!(concat!("../icons/", $name, ".svg")).as_slice())),*]
    };
}

const FILES: [(&str, &[u8]); 23] = icon_files!(
    "result",
    "compute_scalar",
    "stream_aggregate",
    "parallelism",
    "hash_match",
    "table_scan",
    "nested_loops",
    "top",
    "row_count_spool",
    "sort",
    "index_scan",
    "index_seek",
    "clustered_index_seek",
    "clustered_index_scan",
    "bookmark_lookup",
    "bitmap",
    "iterator_catch_all",
    "aggregate",
    "merge_join",
    "filter",
    "table_spool",
    "overlay-warning",
    "overlay-parallelism",
);

/// The icon named `name` (a file stem in `icons/`), or the catch-all operator icon.
pub fn get(name: &str) -> Arc<Image> {
    static IMAGES: OnceLock<HashMap<&'static str, Arc<Image>>> = OnceLock::new();
    let images = IMAGES.get_or_init(|| {
        FILES
            .iter()
            .map(|(n, bytes)| {
                (
                    *n,
                    Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.to_vec())),
                )
            })
            .collect()
    });
    images
        .get(name)
        .or_else(|| images.get("iterator_catch_all"))
        .cloned()
        .expect("catch-all icon is embedded")
}
