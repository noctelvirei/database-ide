//! PROTOTYPE (copied from the gpui spike). Results grid paged from a file on disk.
//!
//! The data file holds 1,000,000 rows of 10 tab-separated text fields, one row per line.
//! The index file holds one little-endian u64 byte offset per row into the data file,
//! plus one final offset at the end of the data. Neither file is ever held in memory:
//! the grid reads one page of rows at a time and keeps a small, bounded cache of pages.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use gpui::{App, Context, IntoElement, SharedString, Window, px};
use gpui_component::table::{Column, TableDelegate, TableState};

pub const ROWS: usize = 1_000_000;
pub const COLS: usize = 10;
/// Rows read from disk in one go.
const PAGE_ROWS: usize = 128;
/// Pages kept in memory at most: 32 x 128 = 4,096 rows, whatever the grid's position.
const CACHE_PAGES: usize = 32;

const COLUMN_NAMES: [&str; COLS] = [
    "order_id",
    "customer",
    "created_at",
    "status",
    "amount",
    "currency",
    "region",
    "warehouse",
    "sku",
    "note",
];

/// The folder that holds the data and index files: `PROTOTYPE_DATA_DIR`, or a folder in the
/// system temp directory.
pub fn data_dir() -> PathBuf {
    std::env::var_os("PROTOTYPE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("database-ide-gpui-spike"))
}

fn data_path(dir: &Path) -> PathBuf {
    dir.join("rows.dat")
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join("rows.idx")
}

/// Writes the data and index files unless both already exist.
pub fn ensure_data_file(dir: &Path) -> anyhow::Result<()> {
    if data_path(dir).exists() && index_path(dir).exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let mut data = BufWriter::with_capacity(1 << 20, File::create(data_path(dir))?);
    let mut index = BufWriter::with_capacity(1 << 20, File::create(index_path(dir))?);
    let statuses = ["open", "paid", "shipped", "cancelled", "refunded"];
    let regions = ["north", "south", "east", "west", "central"];
    let mut offset: u64 = 0;
    let mut line = String::with_capacity(256);
    for r in 0..ROWS {
        index.write_all(&offset.to_le_bytes())?;
        line.clear();
        use std::fmt::Write as _;
        write!(
            line,
            "{r}\tcustomer {c:05}\t2026-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}\t{st}\t{a}.{ac:02}\tEUR\t{rg}\tWH-{w:03}\tSKU-{sku:08}\tline {r} of the spike data\n",
            c = (r * 7919) % 100_000,
            m = r % 12 + 1,
            d = r % 28 + 1,
            h = r % 24,
            mi = r % 60,
            s = (r * 7) % 60,
            st = statuses[r % statuses.len()],
            a = (r * 31) % 100_000,
            ac = r % 100,
            rg = regions[(r / 3) % regions.len()],
            w = r % 250,
            sku = (r * 104_729) % 100_000_000,
        )?;
        data.write_all(line.as_bytes())?;
        offset += line.len() as u64;
    }
    index.write_all(&offset.to_le_bytes())?;
    data.flush()?;
    index.flush()?;
    Ok(())
}

type Page = Vec<[SharedString; COLS]>;

/// Table delegate that reads rows from the data file on demand.
pub struct FileRows {
    data: File,
    index: File,
    columns: Vec<Column>,
    pages: HashMap<usize, Page>,
    /// Page numbers, oldest first, for eviction.
    order: VecDeque<usize>,
    /// Pages read from disk since startup.
    pub pages_read: u64,
}

impl FileRows {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        let columns = COLUMN_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Column::new(format!("c{i}"), *name).width(px(match i { 9 => 220., 2 => 170., _ => 130. }))
            })
            .collect();
        Ok(Self {
            data: File::open(data_path(dir))?,
            index: File::open(index_path(dir))?,
            columns,
            pages: HashMap::new(),
            order: VecDeque::new(),
            pages_read: 0,
        })
    }

    pub fn cached_pages(&self) -> usize {
        self.pages.len()
    }

    fn offset_of(&mut self, row: usize) -> anyhow::Result<u64> {
        let mut buf = [0u8; 8];
        self.index.seek(SeekFrom::Start(row as u64 * 8))?;
        self.index.read_exact(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    fn read_page(&mut self, page: usize) -> anyhow::Result<Page> {
        let first = page * PAGE_ROWS;
        let end_row = (first + PAGE_ROWS).min(ROWS);
        let start = self.offset_of(first)?;
        let end = self.offset_of(end_row)?;
        let mut bytes = vec![0u8; (end - start) as usize];
        self.data.seek(SeekFrom::Start(start))?;
        self.data.read_exact(&mut bytes)?;
        let text = String::from_utf8(bytes)?;
        Ok(text
            .lines()
            .map(|line| {
                let mut fields = line.split('\t');
                std::array::from_fn(|_| SharedString::from(fields.next().unwrap_or("").to_string()))
            })
            .collect())
    }

    fn cell(&mut self, row: usize, col: usize) -> SharedString {
        let page = row / PAGE_ROWS;
        if !self.pages.contains_key(&page) {
            let rows = self.read_page(page).unwrap_or_default();
            self.pages_read += 1;
            if self.order.len() >= CACHE_PAGES
                && let Some(old) = self.order.pop_front()
            {
                self.pages.remove(&old);
            }
            self.pages.insert(page, rows);
            self.order.push_back(page);
        }
        self.pages[&page]
            .get(row % PAGE_ROWS)
            .map(|r| r[col].clone())
            .unwrap_or_default()
    }
}

impl TableDelegate for FileRows {
    fn columns_count(&self, _: &App) -> usize {
        COLS
    }

    fn rows_count(&self, _: &App) -> usize {
        ROWS
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.cell(row_ix, col_ix)
    }
}
