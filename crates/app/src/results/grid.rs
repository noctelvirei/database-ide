//! One result set's grid: gpui-kit's virtual table over the result's spill file.
//!
//! The table draws only the rows on screen and asks for each cell as it draws. Rows come
//! from a page cache filled on the background executor from the spill file, so a million
//! rows cost the page in memory, not the result (docs/spec.md, "Results: spill to disk").
//! A row not loaded yet draws empty for the frame or two its page takes.
//!
//! The UI thread never touches the spill file: the reader's lock is held while the runtime
//! writes rows, so the row count and completion are copied in by `refresh`, the page by
//! `want_row` and the selected cell's value by `selection_changed`, all off the UI thread.
//!
//! The table selects one cell, row or column at a time; the grid adds a block on top of
//! that. The table's own selection is the block's active corner, and the grid keeps the
//! other corner (the anchor), which Shift with an arrow key leaves where it is.

use std::ops::Range;

use dbide_engine::session::{TypeCategory, Value};
use dbide_engine::spill::Reader;
use gpui::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::table::{Column, TableDelegate, TableSelection, TableState};
use serde::Serialize;

/// Rows read from the spill file at a time.
const PAGE_ROWS: usize = 256;
/// How far before the first row asked for a page starts, so scrolling up finds rows ready.
const PAGE_LEAD: usize = 64;
/// The most characters a cell draws; the inspector shows the whole value.
const CELL_CHARS: usize = 200;

/// A rectangle of cells, zero-based and end-exclusive in both directions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Block {
    pub rows: Range<usize>,
    pub columns: Range<usize>,
}

/// The rows the grid holds in memory: one page of the result.
struct Page {
    start: usize,
    rows: Vec<Vec<Value>>,
}

impl Page {
    fn row(&self, row_ix: usize) -> Option<&[Value]> {
        let offset = row_ix.checked_sub(self.start)?;
        self.rows.get(offset).map(Vec::as_slice)
    }
}

/// The table's view of one result set.
pub struct Grid {
    reader: Reader,
    page: Option<Page>,
    /// The page being read, so a frame that asks again doesn't read it twice.
    loading: Option<Range<usize>>,
    /// The block's fixed corner, as (row, column); the table's selection is the other.
    anchor: Option<(usize, usize)>,
    /// True while the grid itself moves the table's selection to grow the block, so the
    /// selection event that follows keeps the anchor.
    extending: bool,
    /// The selected block, kept here because cells draw inside the table's update, where
    /// the table's own selection can't be read.
    block: Option<Block>,
    /// Rows published, as of the last `refresh`.
    rows: usize,
    /// Whether the result was complete at the last `refresh`.
    complete: bool,
    /// True while a `refresh` is reading.
    refreshing: bool,
    /// Set when a `refresh` was asked for during another, which then reads again.
    refresh_again: bool,
    /// The selected cell and its whole value, kept apart from the page so the inspector
    /// still shows it once the grid scrolls away.
    selected: Option<((usize, usize), Value)>,
    /// Why reading rows failed; once set, the grid stops reading and says so.
    failure: Option<String>,
}

impl Grid {
    pub fn new(reader: Reader) -> Self {
        Self {
            reader,
            page: None,
            loading: None,
            anchor: None,
            extending: false,
            block: None,
            rows: 0,
            complete: false,
            refreshing: false,
            refresh_again: false,
            selected: None,
            failure: None,
        }
    }

    pub fn reader(&self) -> &Reader {
        &self.reader
    }

    /// Rows published so far, as of the last `refresh`; it grows while the run streams.
    pub fn row_count(&self) -> usize {
        self.rows
    }

    /// Whether every row has arrived, as of the last `refresh`.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Why the grid couldn't read its rows, if it couldn't.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// True while the row count is being read.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    /// Reads the row count and completion again, off the UI thread, and redraws. Asked
    /// during a read, it reads once more after; once the result is complete, it does nothing.
    pub fn refresh(&mut self, cx: &mut Context<TableState<Self>>) {
        if self.complete {
            return;
        }
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        self.refreshing = true;
        let reader = self.reader.clone();
        cx.spawn(async move |table, cx| {
            let counts = cx
                .background_spawn(async move {
                    // Completion first: rows can still arrive between the two reads, and a
                    // count read after completion is final.
                    let complete = reader.is_complete()?;
                    Ok::<_, dbide_engine::spill::Error>((reader.row_count()?, complete))
                })
                .await;
            let _ = table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                grid.refreshing = false;
                match counts {
                    Ok((rows, complete)) => {
                        grid.rows = rows;
                        grid.complete = complete;
                    }
                    Err(error) => grid.fail(&error),
                }
                if std::mem::take(&mut grid.refresh_again) {
                    grid.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stops reading after `error` and keeps it to show.
    fn fail(&mut self, error: &dbide_engine::spill::Error) {
        self.failure = Some(error.to_string());
        self.complete = true;
    }

    /// The selected cell's whole value, once it is read.
    pub fn selected_value(&self, row: usize, column: usize) -> Option<&Value> {
        let (cell, value) = self.selected.as_ref()?;
        (*cell == (row, column)).then_some(value)
    }

    /// The selected block.
    pub fn block(&self) -> Option<&Block> {
        self.block.as_ref()
    }

    /// The table's selection changed to `selection`: works out the block from it and the
    /// anchor, and reads a single selected cell's value for the inspector. A new single cell
    /// becomes the anchor, unless the grid is growing the block.
    pub fn selection_changed(
        &mut self,
        selection: TableSelection,
        cx: &mut Context<TableState<Self>>,
    ) {
        if let TableSelection::Cell(row, column) = selection
            && !std::mem::take(&mut self.extending)
        {
            self.anchor = Some((row, column));
        }
        self.block = self.block_for(selection);
        self.selected = None;
        if let TableSelection::Cell(row, column) = selection {
            self.read_selected(row, column, cx);
        }
    }

    /// Keeps the value of cell (`row`, `column`), from the page if it is there, otherwise
    /// read from the spill file in the background.
    fn read_selected(&mut self, row: usize, column: usize, cx: &mut Context<TableState<Self>>) {
        if let Some(value) = self.value(row, column) {
            self.selected = Some(((row, column), value.clone()));
            return;
        }
        if self.failure.is_some() {
            return;
        }
        let reader = self.reader.clone();
        cx.spawn(async move |table, cx| {
            let read = cx
                .background_spawn(async move { reader.rows(row..row + 1) })
                .await;
            let _ = table.update(cx, |table, cx| {
                let still_selected = table.selection() == TableSelection::Cell(row, column);
                let grid = table.delegate_mut();
                match read {
                    Ok(mut rows) if still_selected => {
                        if let Some(value) = rows
                            .pop()
                            .and_then(|mut r| (column < r.len()).then(|| r.swap_remove(column)))
                        {
                            grid.selected = Some(((row, column), value));
                        }
                    }
                    Ok(_) => {}
                    Err(error) => grid.fail(&error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn block_for(&self, selection: TableSelection) -> Option<Block> {
        let rows = self.row_count();
        let columns = self.reader.columns().len();
        match selection {
            TableSelection::None => None,
            TableSelection::Row(row) => Some(Block {
                rows: row..row + 1,
                columns: 0..columns,
            }),
            TableSelection::Column(column) => Some(Block {
                rows: 0..rows,
                columns: column..column + 1,
            }),
            TableSelection::Cell(row, column) => {
                let (anchor_row, anchor_column) = self.anchor.unwrap_or((row, column));
                Some(Block {
                    rows: row.min(anchor_row)..row.max(anchor_row) + 1,
                    columns: column.min(anchor_column)..column.max(anchor_column) + 1,
                })
            }
        }
    }

    /// Marks the next selection change as growing the block.
    pub fn extend_next(&mut self) {
        self.extending = true;
    }

    /// Keeps `anchor` as the block's fixed corner, for Ctrl+A and Shift+Space.
    pub fn set_anchor(&mut self, anchor: (usize, usize)) {
        self.anchor = Some(anchor);
        self.extending = true;
    }

    /// One cell's value, if its row is loaded.
    pub fn value(&self, row: usize, column: usize) -> Option<&Value> {
        self.page.as_ref()?.row(row)?.get(column)
    }

    /// Loads the page holding `row_ix` unless it is loaded or loading.
    fn want_row(&mut self, row_ix: usize, cx: &mut Context<TableState<Self>>) {
        let loaded = self.page.as_ref().is_some_and(|p| p.row(row_ix).is_some());
        let loading = self.loading.as_ref().is_some_and(|r| r.contains(&row_ix));
        if loaded || loading || self.failure.is_some() {
            return;
        }
        let start = row_ix.saturating_sub(PAGE_LEAD);
        let range = start..start + PAGE_ROWS;
        self.loading = Some(range.clone());
        let reader = self.reader.clone();
        cx.spawn(async move |table, cx| {
            let read = range.clone();
            let rows = cx.background_spawn(async move { reader.rows(read) }).await;
            let _ = table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                if grid.loading.as_ref() == Some(&range) {
                    grid.loading = None;
                }
                match rows {
                    Ok(rows) => {
                        grid.page = Some(Page {
                            start: range.start,
                            rows,
                        })
                    }
                    // The grid stops reading and its heading says why; the rows draw empty.
                    Err(error) => grid.fail(&error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// How a cell reads in the grid: NULL as the word, text cut at its first line.
pub fn cell_text(value: &Value) -> SharedString {
    match value {
        Value::Null => "NULL".into(),
        value => {
            let text = dbide_engine::export::cell_text(value);
            let line = text.lines().next().unwrap_or_default();
            if line.len() == text.len() && line.chars().count() <= CELL_CHARS {
                text.into()
            } else {
                let cut: String = line.chars().take(CELL_CHARS).collect();
                format!("{cut}…").into()
            }
        }
    }
}

/// A starting width for a column, from its name and kind of value; the user can resize.
fn width(column: &dbide_engine::session::Column) -> f32 {
    let by_kind: f32 = match column.category {
        TypeCategory::Bool => 60.,
        TypeCategory::Integer | TypeCategory::Decimal | TypeCategory::Float => 100.,
        TypeCategory::Date | TypeCategory::Time => 110.,
        TypeCategory::Timestamp => 180.,
        TypeCategory::Uuid => 290.,
        _ => 180.,
    };
    let by_name = column.name.chars().count() as f32 * 8. + 24.;
    by_kind.max(by_name).min(400.)
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.reader.columns().len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.row_count()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let column = &self.reader.columns()[col_ix];
        Column::new(
            SharedString::from(format!("c{col_ix}")),
            column.name.clone(),
        )
        .width(px(width(column)))
        .resizable(true)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.want_row(row_ix, cx);
        let in_block = self
            .block
            .as_ref()
            .is_some_and(|b| b.rows.contains(&row_ix) && b.columns.contains(&col_ix));
        let theme = cx.theme();
        let cell = div().size_full().overflow_hidden().whitespace_nowrap();
        let cell = if in_block {
            cell.bg(theme.table_active)
        } else {
            cell
        };
        match self.value(row_ix, col_ix) {
            Some(Value::Null) => cell
                .italic()
                .text_color(theme.muted_foreground)
                .child("NULL"),
            Some(value) => cell.child(cell_text(value)),
            None => cell,
        }
    }
}
