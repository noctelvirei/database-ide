//! A query tab's results pane (docs/spec.md, "Runs and results"): the last run's result
//! sets stacked, each grid with its own header and row count, any one maximised to fill the
//! pane.
//!
//! The pane owns the run's spill files once the run ends, so a result's file lives exactly
//! as long as the tab shows it, and the next run's `clear` deletes it.
//!
//! Keys while a grid has focus: arrows and Home, End, Page Up and Page Down move the table's
//! selection; Ctrl+Home and Ctrl+End go to the first and last cell; Shift with an arrow
//! grows a block; Shift+Space selects the row, Ctrl+Space
//! the column, Ctrl+A everything; Ctrl+C copies the block and Ctrl+Shift+C copies it with
//! headers; Ctrl+Up and Ctrl+Down move between grids; Alt+Enter maximises.

mod grid;

use dbide_engine::spill::{Reader, Store};
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::table::{DataTable, TableEvent, TableSelection, TableState};
use serde::Serialize;

use crate::keymap::{
    CopyBlock, CopyBlockWithHeaders, ExtendDown, ExtendLeft, ExtendRight, ExtendUp, FirstCell,
    LastCell, NextResult, PreviousResult, SelectAll, SelectColumn, SelectRow, ToggleMaximise,
};
use grid::{Block, Grid};

/// The most rows and bytes one copy puts on the clipboard; a bigger block is refused with
/// a message, and an export (U9) is the way to take it.
const COPY_MAX_ROWS: usize = 1_000_000;
const COPY_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Something for the tab's messages strip.
pub enum ResultsEvent {
    Message(SharedString),
}

/// What a user can see of one grid.
#[derive(Debug, Serialize)]
pub struct GridState {
    pub columns: Vec<String>,
    pub rows: usize,
    /// False while the run is still adding rows.
    pub complete: bool,
    /// The rows on screen, zero-based and end-exclusive.
    pub visible_rows: std::ops::Range<usize>,
    pub block: Option<Block>,
    pub focused: bool,
    /// True when some of the grid shows in the pane, which scrolls when the grids don't fit.
    pub on_screen: bool,
}

/// What the inspector shows: the selected cell in full.
#[derive(Debug, Serialize)]
pub struct Inspected {
    pub column: String,
    pub type_name: String,
    /// The whole value as text; `None` for NULL.
    pub text: Option<String>,
}

pub struct Results {
    grids: Vec<Entity<TableState<Grid>>>,
    /// The last run's spill files, kept while their grids show them.
    stores: Vec<Store>,
    /// The grid the keyboard works on.
    focused: usize,
    /// The grid filling the pane, if one is maximised.
    maximised: Option<usize>,
    /// Copies still reading their block from the spill file.
    copying: usize,
    /// An earlier run's spill files that a copy still reads; dropped when the copies end.
    retired: Vec<Store>,
    /// The stacked grids' scroll position.
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<ResultsEvent> for Results {}

impl Focusable for Results {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Results {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            grids: Vec::new(),
            stores: Vec::new(),
            focused: 0,
            maximised: None,
            copying: 0,
            retired: Vec::new(),
            scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Drops the last run's grids and deletes their files, for a new run. Files a copy is
    /// still reading are deleted when the copy ends.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.grids.clear();
        if self.is_copying() {
            self.retired.append(&mut self.stores);
        } else {
            delete(std::mem::take(&mut self.stores), cx);
        }
        self.focused = 0;
        self.maximised = None;
        cx.notify();
    }

    /// Adds a grid for a result set that has just started.
    pub fn add(&mut self, reader: Reader, window: &mut Window, cx: &mut Context<Self>) {
        let table = cx.new(|cx| {
            TableState::new(Grid::new(reader), window, cx)
                .cell_selectable(true)
                .row_selectable(true)
                .col_selectable(true)
                .col_resizable(true)
                // Moving columns needs the grid to remap cells, the inspector and copies.
                .col_movable(false)
        });
        let ix = self.grids.len();
        cx.subscribe(
            &table,
            move |this: &mut Self, table, event: &TableEvent, cx| {
                if matches!(
                    event,
                    TableEvent::SelectCell(..)
                        | TableEvent::SelectRow(_)
                        | TableEvent::SelectColumn(_)
                        | TableEvent::ClearSelection
                ) {
                    table.update(cx, |table, cx| {
                        let selection = table.selection();
                        table.delegate_mut().selection_changed(selection, cx);
                    });
                    this.focused = ix;
                    cx.notify();
                }
            },
        )
        .detach();
        self.grids.push(table);
        cx.notify();
    }

    /// Redraws the grids, whose row counts grow on the runtime without telling the UI.
    pub fn show_arrived_rows(&mut self, cx: &mut Context<Self>) {
        for table in &self.grids {
            table.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Takes over the run's spill files when the run ends.
    pub fn keep(&mut self, stores: Vec<Store>) {
        self.stores = stores;
    }

    /// Gives the focused grid keyboard focus, selecting its first cell if nothing is
    /// selected, and scrolls it into view; with no grids, the pane itself takes focus.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.grids.get(self.focused).cloned() else {
            window.focus(&self.focus_handle, cx);
            return;
        };
        self.scroll
            .scroll_to_item(self.shown_position(self.focused));
        table.update(cx, |table, cx| {
            if table.selection() == TableSelection::None && table.delegate().row_count() > 0 {
                table.set_selected_cell(0, 0, cx);
            }
        });
        window.focus(&table.focus_handle(cx), cx);
    }

    /// Where grid `ix` sits among the grids drawn: a maximised grid is the only one.
    fn shown_position(&self, ix: usize) -> usize {
        if self.maximised.is_some() { 0 } else { ix }
    }

    /// True when some of grid `ix` shows inside the pane.
    fn is_on_screen(&self, ix: usize) -> bool {
        if self.maximised.is_some_and(|shown| shown != ix) {
            return false;
        }
        let shown = self.scroll.top_item()..=self.scroll.bottom_item();
        !self.grids.is_empty() && shown.contains(&self.shown_position(ix))
    }

    pub fn state(&self, cx: &App) -> Vec<GridState> {
        self.grids
            .iter()
            .enumerate()
            .map(|(ix, table)| {
                let table = table.read(cx);
                let grid = table.delegate();
                GridState {
                    columns: grid
                        .reader()
                        .columns()
                        .iter()
                        .map(|c| c.name.clone())
                        .collect(),
                    rows: grid.row_count(),
                    complete: grid.is_complete(),
                    visible_rows: table.visible_range().rows().clone(),
                    block: grid.block(),
                    focused: ix == self.focused,
                    on_screen: self.is_on_screen(ix),
                }
            })
            .collect()
    }

    /// The focused grid's selected cell, when one cell is selected and its row is loaded.
    pub fn inspected(&self, cx: &App) -> Option<Inspected> {
        let table = self.grids.get(self.focused)?.read(cx);
        let TableSelection::Cell(row, column) = table.selection() else {
            return None;
        };
        let grid = table.delegate();
        let value = grid.selected_value(row, column)?;
        let header = grid.reader().columns().get(column)?;
        Some(Inspected {
            column: header.name.clone(),
            type_name: header.type_name.clone(),
            text: match value {
                dbide_engine::session::Value::Null => None,
                value => Some(dbide_engine::export::cell_text(value)),
            },
        })
    }

    /// True while a copy is still reading its block.
    pub fn is_copying(&self) -> bool {
        self.copying > 0
    }

    fn focused_table(&self) -> Option<Entity<TableState<Grid>>> {
        self.grids.get(self.focused).cloned()
    }

    /// Moves the block's active corner by one cell, keeping its anchor.
    fn extend(&mut self, rows: isize, columns: isize, cx: &mut Context<Self>) {
        let Some(table) = self.focused_table() else {
            return;
        };
        table.update(cx, |table, cx| {
            let TableSelection::Cell(row, column) = table.selection() else {
                return;
            };
            let grid = table.delegate();
            let last_row = grid.row_count().saturating_sub(1);
            let last_column = grid.reader().columns().len().saturating_sub(1);
            let row = row.saturating_add_signed(rows).min(last_row);
            let column = column.saturating_add_signed(columns).min(last_column);
            table.delegate_mut().extend_next();
            table.set_selected_cell(row, column, cx);
        });
    }

    /// Selects the first cell, or the last one, scrolling it into view.
    fn select_corner(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(table) = self.focused_table() else {
            return;
        };
        table.update(cx, |table, cx| {
            let grid = table.delegate();
            let (rows, columns) = (grid.row_count(), grid.reader().columns().len());
            if rows == 0 || columns == 0 {
                return;
            }
            if last {
                table.set_selected_cell(rows - 1, columns - 1, cx);
            } else {
                table.set_selected_cell(0, 0, cx);
            }
        });
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(table) = self.focused_table() else {
            return;
        };
        table.update(cx, |table, cx| {
            let grid = table.delegate();
            let (rows, columns) = (grid.row_count(), grid.reader().columns().len());
            if rows == 0 || columns == 0 {
                return;
            }
            table.delegate_mut().set_anchor((0, 0));
            table.set_selected_cell(rows - 1, columns - 1, cx);
        });
    }

    fn select_line(&mut self, whole_row: bool, cx: &mut Context<Self>) {
        let Some(table) = self.focused_table() else {
            return;
        };
        table.update(cx, |table, cx| {
            let TableSelection::Cell(row, column) = table.selection() else {
                return;
            };
            if whole_row {
                table.set_selected_row(row, cx);
            } else {
                table.set_selected_col(column, cx);
            }
        });
    }

    /// Puts the focused grid's block on the clipboard as tab-separated text.
    fn copy(&mut self, headers: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.focused_table() else {
            return;
        };
        let grid = table.read(cx).delegate();
        let Some(block) = grid.block() else {
            cx.emit(ResultsEvent::Message("Select cells to copy first.".into()));
            return;
        };
        let reader = grid.reader().clone();
        self.copying += 1;
        cx.spawn_in(window, async move |this, cx| {
            let rows = block.rows.len();
            let copied = cx
                .background_spawn(async move {
                    dbide_engine::export::clipboard_block(
                        &reader,
                        block.rows,
                        block.columns,
                        headers,
                        COPY_MAX_ROWS,
                        COPY_MAX_BYTES,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copying -= 1;
                if this.copying == 0 {
                    delete(std::mem::take(&mut this.retired), cx);
                }
                let message = match copied {
                    Ok(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        match rows {
                            1 => "Copied 1 row.".to_owned(),
                            rows => format!("Copied {rows} rows."),
                        }
                    }
                    Err(error) => format!("Couldn't copy: {error}"),
                };
                cx.emit(ResultsEvent::Message(message.into()));
            });
        })
        .detach();
    }

    /// Moves keyboard focus to the next or previous grid.
    fn step_grid(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.grids.len();
        if count == 0 {
            return;
        }
        self.focused = if forward {
            (self.focused + 1).min(count - 1)
        } else {
            self.focused.saturating_sub(1)
        };
        if self.maximised.is_some() {
            self.maximised = Some(self.focused);
        }
        self.focus(window, cx);
        cx.notify();
    }

    fn toggle_maximise(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.grids.is_empty() {
            return;
        }
        self.maximised = match self.maximised {
            Some(_) => None,
            None => Some(self.focused),
        };
        self.focus(window, cx);
        cx.notify();
    }

    /// One grid with its header line: its number, row count and whether rows are still
    /// arriving.
    fn grid(&self, ix: usize, cx: &App) -> impl IntoElement {
        let table = &self.grids[ix];
        let grid = table.read(cx).delegate();
        let rows = grid.row_count();
        let count = match rows {
            1 => "1 row".to_owned(),
            rows => format!("{rows} rows"),
        };
        let note = match grid.failure() {
            Some(failure) => format!("; couldn't read its rows: {failure}"),
            None if grid.lost_rows() => "; some rows couldn't be kept on disk".to_owned(),
            None if !grid.is_complete() => ", still arriving".to_owned(),
            None => String::new(),
        };
        let heading = format!("Result {}: {count}{note}", ix + 1);
        let theme = cx.theme();
        // Each grid's own id keeps the two tables' element ids apart.
        div()
            .id(("result", ix))
            .flex_1()
            .min_h(px(120.))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(if ix == self.focused {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(heading),
            )
            .child(div().flex_1().child(DataTable::new(table)))
    }
}

/// Deletes spill files on the background executor, keeping the disk I/O off the UI thread.
fn delete(stores: Vec<Store>, cx: &App) {
    if !stores.is_empty() {
        cx.background_spawn(async move { drop(stores) }).detach();
    }
}

impl Render for Results {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown: Vec<usize> = match self.maximised {
            Some(ix) => vec![ix],
            None => (0..self.grids.len()).collect(),
        };
        let grids: Vec<_> = shown.into_iter().map(|ix| self.grid(ix, cx)).collect();
        div()
            .id("results")
            .key_context("Results")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(grids)
            .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.extend(-1, 0, cx)))
            .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.extend(1, 0, cx)))
            .on_action(cx.listener(|this, _: &ExtendLeft, _, cx| this.extend(0, -1, cx)))
            .on_action(cx.listener(|this, _: &ExtendRight, _, cx| this.extend(0, 1, cx)))
            .on_action(cx.listener(|this, _: &FirstCell, _, cx| this.select_corner(false, cx)))
            .on_action(cx.listener(|this, _: &LastCell, _, cx| this.select_corner(true, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &SelectRow, _, cx| this.select_line(true, cx)))
            .on_action(cx.listener(|this, _: &SelectColumn, _, cx| this.select_line(false, cx)))
            .on_action(cx.listener(|this, _: &CopyBlock, w, cx| this.copy(false, w, cx)))
            .on_action(cx.listener(|this, _: &CopyBlockWithHeaders, w, cx| this.copy(true, w, cx)))
            .on_action(cx.listener(|this, _: &NextResult, w, cx| this.step_grid(true, w, cx)))
            .on_action(cx.listener(|this, _: &PreviousResult, w, cx| this.step_grid(false, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleMaximise, w, cx| this.toggle_maximise(w, cx)))
    }
}
