//! "Loaded" gpui + gpui-component 0.5.1 window for the UI-toolkit benchmark (database-ide #3):
//! a SQL code editor above a virtualised Table holding ROWS x COLS strings in Rust memory.
//! Writes epoch ms to BENCH_FIRST_FRAME on the first render() call (just before first paint).
//! gpui-component 0.5.1 is used because 0.6+ (gpui-pre) needs a newer Rust than the 1.93 installed.
#![windows_subsystem = "windows"]
use gpui::{
    div, prelude::*, px, App, Application, Context, Entity, Window, WindowOptions,
};
use gpui_component::input::{Input, InputState};
use gpui_component::table::{Column, Table, TableDelegate, TableState};
use gpui_component::Root;
use std::time::{SystemTime, UNIX_EPOCH};

const ROWS: usize = 100_000;
const COLS: usize = 10;

struct Rows {
    data: Vec<Vec<String>>,
    columns: Vec<Column>,
}

impl TableDelegate for Rows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.data.len()
    }
    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }
    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.data[row_ix][col_ix].clone()
    }
}

struct Workbench {
    editor: Entity<InputState>,
    table: Entity<TableState<Rows>>,
    marked: bool,
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if !self.marked {
            self.marked = true;
            if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
                let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                let _ = std::fs::write(path, ms.to_string());
            }
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(div().h(px(250.)).child(Input::new(&self.editor).h_full()))
            .child(div().flex_1().child(Table::new(&self.table)))
    }
}

fn main() {
    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let editor = cx.new(|cx| {
                    InputState::new(window, cx)
                        .code_editor("sql")
                        .line_number(true)
                        .default_value(
                            "SELECT o.id, o.created_at, c.name\nFROM orders AS o\nJOIN customers AS c ON c.id = o.customer_id\nWHERE o.created_at > '2026-01-01'\nORDER BY o.created_at DESC;",
                        )
                });
                let data = (0..ROWS)
                    .map(|r| (0..COLS).map(|c| format!("row {r:06} col {c}")).collect())
                    .collect();
                let columns = (0..COLS)
                    .map(|c| Column::new(format!("c{c}"), format!("Column {c}")).width(px(140.)))
                    .collect();
                let table = cx.new(|cx| TableState::new(Rows { data, columns }, window, cx));
                let view = cx.new(|_| Workbench { editor, table, marked: false });
                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
